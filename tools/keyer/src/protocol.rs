use std::fmt::Write as _;

use grayline_keyer_protocol::{MAXIMUM_CODES_PER_LINE, decode_codes, decode_open};
use grayline_rtty::{TxCode, TxConfig};

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
        "\\rtty_open" => decode_open(arguments)
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

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
    fn open_arguments_are_the_shared_framing_arguments() {
        let arguments = ["baud=50", "stop=2", "parity=odd", "lead_in=0.1"];
        let line = format!("+\\rtty_open {}", arguments.join(" "));
        assert_eq!(
            parse(&line).command,
            Ok(Command::RttyOpen(decode_open(&arguments).unwrap()))
        );
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
