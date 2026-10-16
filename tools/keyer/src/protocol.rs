use std::fmt::Write as _;

use grayline_rtty::{BaudRate, Parity, StopElement, TxCode, TxConfig, tx::Diddle, tx::Framer};

/// Hamlib's status codes, negated as `rigctld` reports them.
pub mod code {
    /// Done.
    pub const OK: i32 = 0;
    /// An argument was malformed or out of range.
    pub const INVALID: i32 = -1;
    /// The command is not one this keyer knows.
    pub const NOT_IMPLEMENTED: i32 = -4;
    /// The request conflicts with what is already going on.
    pub const REJECTED: i32 = -9;
    /// The request would not fit.
    pub const TRUNCATED: i32 = -10;
}

/// The longest run of codes one `\rtty_send` may carry.
pub const MAXIMUM_CODES_PER_LINE: usize = 4_096;

/// The digits a five-bit code is written with, by value.
const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";

/// One request a client can make.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Whether every command wants a VFO argument; never, here.
    ChkVfo,
    /// Open an RTTY stream framed by this configuration.
    RttyOpen(TxConfig),
    /// Queue codes on the open stream.
    RttySend(Vec<TxCode>),
    /// No more codes are coming.
    RttyEnd,
    /// Report how the last stream stands.
    Status,
    /// Cut the open stream short.
    Stop,
    /// Close the connection.
    Quit,
}

impl Command {
    /// The long name the command answers under.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ChkVfo => "chk_vfo",
            Self::RttyOpen(_) => "rtty_open",
            Self::RttySend(_) => "rtty_send",
            Self::RttyEnd => "rtty_end",
            Self::Status => "status",
            Self::Stop => "stop",
            Self::Quit => "quit",
        }
    }
}

/// A line as a client sent it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// Whether the client asked for the extended answer form with `+`.
    pub extended: bool,
    /// The command, or the answer that refuses it.
    pub command: Result<Command, Reply>,
}

/// What is sent back for one request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reply {
    name: String,
    lines: Vec<String>,
    code: i32,
}

impl Reply {
    /// Success, with `lines` of `Label: value` for a query.
    pub fn new(name: &str, lines: Vec<String>) -> Self {
        Self {
            name: name.to_owned(),
            lines,
            code: code::OK,
        }
    }

    /// A refusal with Hamlib's status `code`.
    pub fn refuse(name: &str, code: i32) -> Self {
        Self {
            name: name.to_owned(),
            lines: Vec::new(),
            code,
        }
    }

    /// The status the reply ends with.
    pub const fn code(&self) -> i32 {
        self.code
    }

    /// The reply as it goes on the wire.
    ///
    /// The extended form names the command and always ends in `RPRT`, which
    /// is what lets a client read an answer of any length. The plain form
    /// follows `rigctld`: a successful query is its values alone, and
    /// everything else is a bare `RPRT`.
    pub fn render(&self, extended: bool) -> String {
        let mut text = String::new();
        if extended {
            let _ = writeln!(text, "{}:", self.name);
            for line in &self.lines {
                let _ = writeln!(text, "{line}");
            }
            let _ = writeln!(text, "RPRT {}", self.code);
        } else if self.code == code::OK && !self.lines.is_empty() {
            for line in &self.lines {
                let value = line.split_once(": ").map_or(line.as_str(), |(_, value)| value);
                let _ = writeln!(text, "{value}");
            }
        } else {
            let _ = writeln!(text, "RPRT {}", self.code);
        }
        text
    }
}

/// Reads one line of the protocol.
pub fn parse(line: &str) -> Request {
    let line = line.trim();
    let (extended, line) = line.strip_prefix('+').map_or((false, line), |rest| (true, rest));
    let mut words = line.split_whitespace();
    let name = words.next().unwrap_or_default();
    let arguments: Vec<&str> = words.collect();
    Request {
        extended,
        command: parse_command(name, &arguments),
    }
}

fn parse_command(name: &str, arguments: &[&str]) -> Result<Command, Reply> {
    let bare = |command: Command| {
        if arguments.is_empty() {
            Ok(command)
        } else {
            Err(Reply::refuse(command.name(), code::INVALID))
        }
    };
    match name {
        "\\chk_vfo" => bare(Command::ChkVfo),
        "\\rtty_open" => parse_open(arguments)
            .map(Command::RttyOpen)
            .ok_or_else(|| Reply::refuse("rtty_open", code::INVALID)),
        "\\rtty_send" => match arguments {
            [codes] if codes.len() <= MAXIMUM_CODES_PER_LINE => decode_codes(codes)
                .map(Command::RttySend)
                .ok_or_else(|| Reply::refuse("rtty_send", code::INVALID)),
            [_] => Err(Reply::refuse("rtty_send", code::TRUNCATED)),
            _ => Err(Reply::refuse("rtty_send", code::INVALID)),
        },
        "\\rtty_end" => bare(Command::RttyEnd),
        "\\status" => bare(Command::Status),
        "\\stop" => bare(Command::Stop),
        "q" | "Q" => bare(Command::Quit),
        _ => Err(Reply::refuse(name.trim_start_matches('\\'), code::NOT_IMPLEMENTED)),
    }
}

/// Reads `key=value` framing arguments over the defaults.
fn parse_open(arguments: &[&str]) -> Option<TxConfig> {
    let mut config = TxConfig::default();
    for argument in arguments {
        let (key, value) = argument.split_once('=')?;
        match key {
            "baud" => config.framing.baud = BaudRate::new(value.parse().ok()?).ok()?,
            "stop" => {
                config.framing.stop = match value {
                    "1" => StopElement::One,
                    "1.5" => StopElement::OneAndAHalf,
                    "2" => StopElement::Two,
                    _ => return None,
                }
            }
            "parity" => {
                config.framing.parity = match value {
                    "none" => Parity::None,
                    "even" => Parity::Even,
                    "odd" => Parity::Odd,
                    "mark" => Parity::Mark,
                    "space" => Parity::Space,
                    _ => return None,
                }
            }
            "gap" => config.char_gap_bits = value.parse().ok()?,
            "diddle" => {
                config.diddle = match value {
                    "none" => Diddle::None,
                    "ltrs" => Diddle::Ltrs,
                    "blank" => Diddle::Blank,
                    _ => return None,
                }
            }
            "lead_in" => config.lead_in_seconds = value.parse().ok()?,
            "tail" => config.tail_seconds = value.parse().ok()?,
            _ => return None,
        }
    }
    Framer::new(1_000_000, &config).ok()?;
    Some(config)
}

/// Writes the arguments `\rtty_open` reads back as `config`'s framing.
pub fn encode_open(config: &TxConfig) -> String {
    let stop = match config.framing.stop {
        StopElement::One => "1",
        StopElement::OneAndAHalf => "1.5",
        StopElement::Two => "2",
    };
    let parity = match config.framing.parity {
        Parity::None => "none",
        Parity::Even => "even",
        Parity::Odd => "odd",
        Parity::Mark => "mark",
        Parity::Space => "space",
    };
    let diddle = match config.diddle {
        Diddle::None => "none",
        Diddle::Ltrs => "ltrs",
        Diddle::Blank => "blank",
    };
    format!(
        "baud={} stop={stop} parity={parity} gap={} diddle={diddle} lead_in={} tail={}",
        config.framing.baud.bits_per_second(),
        config.char_gap_bits,
        config.lead_in_seconds,
        config.tail_seconds,
    )
}

/// Writes codes in the form `\rtty_send` reads.
pub fn encode_codes(codes: &[TxCode]) -> String {
    codes
        .iter()
        .map(|code| match code {
            TxCode::Character(value) => char::from(DIGITS[usize::from(value & 0x1f)]),
            TxCode::HoldMark => 'h',
            TxCode::CarrierOff => 'x',
            TxCode::DisableDiddle => 'd',
            TxCode::EnableDiddle => 'e',
        })
        .collect()
}

/// Reads codes written by [`encode_codes`].
pub fn decode_codes(text: &str) -> Option<Vec<TxCode>> {
    text.bytes()
        .map(|byte| match byte {
            b'h' => Some(TxCode::HoldMark),
            b'x' => Some(TxCode::CarrierOff),
            b'd' => Some(TxCode::DisableDiddle),
            b'e' => Some(TxCode::EnableDiddle),
            _ => DIGITS
                .iter()
                .position(|digit| *digit == byte)
                .and_then(|value| u8::try_from(value).ok())
                .map(TxCode::Character),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn every_code_survives_a_round_trip() {
        let mut codes: Vec<TxCode> = (0..32).map(TxCode::Character).collect();
        codes.extend([
            TxCode::HoldMark,
            TxCode::CarrierOff,
            TxCode::DisableDiddle,
            TxCode::EnableDiddle,
        ]);
        let text = encode_codes(&codes);
        assert_eq!(text, "0123456789ABCDEFGHIJKLMNOPQRSTUVhxde");
        assert_eq!(decode_codes(&text), Some(codes));
    }

    #[rstest]
    #[case("W")]
    #[case("a")]
    #[case("1 2")]
    #[case("Ｒ")]
    fn a_character_outside_the_alphabet_is_refused(#[case] text: &str) {
        assert_eq!(decode_codes(text), None);
    }

    #[rstest]
    #[case("+\\chk_vfo", true, Ok(Command::ChkVfo))]
    #[case("\\rtty_send 0Vh", false, Ok(Command::RttySend(vec![TxCode::Character(0), TxCode::Character(31), TxCode::HoldMark])))]
    #[case("+\\rtty_end", true, Ok(Command::RttyEnd))]
    #[case("\\status", false, Ok(Command::Status))]
    #[case("+\\stop", true, Ok(Command::Stop))]
    #[case("q", false, Ok(Command::Quit))]
    #[case("T 1", false, Err(Reply::refuse("T", code::NOT_IMPLEMENTED)))]
    #[case("+\\get_ptt", true, Err(Reply::refuse("get_ptt", code::NOT_IMPLEMENTED)))]
    #[case("+\\status now", true, Err(Reply::refuse("status", code::INVALID)))]
    #[case("+\\rtty_send", true, Err(Reply::refuse("rtty_send", code::INVALID)))]
    #[case("+\\rtty_send 0Z", true, Err(Reply::refuse("rtty_send", code::INVALID)))]
    #[case("+\\get_freq", true, Err(Reply::refuse("get_freq", code::NOT_IMPLEMENTED)))]
    fn commands_are_read_as_rigctld_spells_them(
        #[case] line: &str,
        #[case] extended: bool,
        #[case] command: Result<Command, Reply>,
    ) {
        assert_eq!(parse(line), Request { extended, command });
    }

    #[test]
    fn an_overlong_run_of_codes_is_refused_as_truncated() {
        let line = format!("+\\rtty_send {}", "0".repeat(MAXIMUM_CODES_PER_LINE + 1));
        assert_eq!(parse(&line).command, Err(Reply::refuse("rtty_send", code::TRUNCATED)));
    }

    #[test]
    fn open_arguments_override_the_defaults() {
        let Ok(Command::RttyOpen(config)) =
            parse("+\\rtty_open baud=50 stop=2 parity=odd gap=1.5 diddle=ltrs lead_in=0.1 tail=0").command
        else {
            panic!("not an open");
        };
        assert_eq!(config.framing.baud.bits_per_second(), 50.0);
        assert_eq!(config.framing.stop, StopElement::Two);
        assert_eq!(config.framing.parity, Parity::Odd);
        assert_eq!(config.char_gap_bits, 1.5);
        assert_eq!(config.diddle, Diddle::Ltrs);
        assert_eq!(config.lead_in_seconds, 0.1);
        assert_eq!(config.tail_seconds, 0.0);
    }

    #[test]
    fn written_open_arguments_read_back_as_the_same_framing() {
        let mut config = TxConfig {
            char_gap_bits: 2.0,
            diddle: Diddle::Blank,
            lead_in_seconds: 0.25,
            tail_seconds: 0.125,
            ..TxConfig::default()
        };
        config.framing.parity = Parity::Even;
        config.framing.stop = StopElement::One;
        let line = format!("\\rtty_open {}", encode_open(&config));
        let Ok(Command::RttyOpen(read)) = parse(&line).command else {
            panic!("not an open: {line}");
        };
        assert_eq!(read.framing, config.framing);
        assert_eq!(read.char_gap_bits, config.char_gap_bits);
        assert_eq!(read.diddle, config.diddle);
        assert_eq!(read.lead_in_seconds, config.lead_in_seconds);
        assert_eq!(read.tail_seconds, config.tail_seconds);
    }

    #[rstest]
    #[case("baud=0")]
    #[case("baud=fast")]
    #[case("stop=3")]
    #[case("gap=-1")]
    #[case("tail=-0.5")]
    #[case("colour=blue")]
    #[case("baud")]
    fn malformed_open_arguments_are_refused(#[case] argument: &str) {
        let line = format!("+\\rtty_open {argument}");
        assert_eq!(parse(&line).command, Err(Reply::refuse("rtty_open", code::INVALID)));
    }

    #[test]
    fn the_extended_form_names_the_command_and_always_reports() {
        let reply = Reply::new("chk_vfo", vec!["ChkVFO: 0".to_owned()]);
        assert_eq!(reply.render(true), "chk_vfo:\nChkVFO: 0\nRPRT 0\n");
        assert_eq!(Reply::refuse("stop", code::REJECTED).render(true), "stop:\nRPRT -9\n");
    }

    #[test]
    fn the_plain_form_answers_a_query_with_its_values_alone() {
        let reply = Reply::new("chk_vfo", vec!["ChkVFO: 0".to_owned()]);
        assert_eq!(reply.render(false), "0\n");
        assert_eq!(Reply::new("stop", Vec::new()).render(false), "RPRT 0\n");
        assert_eq!(Reply::refuse("stop", code::REJECTED).render(false), "RPRT -9\n");
    }
}
