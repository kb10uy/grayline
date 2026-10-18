use grayline_rtty::{
    BaudRate, Parity, StopElement, TxConfig,
    tx::{Diddle, Framer},
};

/// Writes the arguments `\rtty_open` reads back as `config`'s framing.
///
/// Only what decides timing is written. Tones, level, and reverse are the
/// audio transmitter's: a keyed line has two states and the rig decides what
/// they sound like.
pub(crate) fn encode_open(config: &TxConfig) -> String {
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

/// Reads `key=value` framing arguments over the defaults.
///
/// Refuses an unknown key, a malformed value, and a framing no keyer could
/// time, so a stream that opens is one that can be keyed.
pub fn decode_open(arguments: &[&str]) -> Option<TxConfig> {
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn arguments_override_the_defaults() {
        let config = decode_open(&[
            "baud=50",
            "stop=2",
            "parity=odd",
            "gap=1.5",
            "diddle=ltrs",
            "lead_in=0.1",
            "tail=0",
        ])
        .unwrap();
        assert_eq!(config.framing.baud.bits_per_second(), 50.0);
        assert_eq!(config.framing.stop, StopElement::Two);
        assert_eq!(config.framing.parity, Parity::Odd);
        assert_eq!(config.char_gap_bits, 1.5);
        assert_eq!(config.diddle, Diddle::Ltrs);
        assert_eq!(config.lead_in_seconds, 0.1);
        assert_eq!(config.tail_seconds, 0.0);
    }

    #[test]
    fn no_arguments_is_the_default_framing() {
        let config = decode_open(&[]).unwrap();
        assert_eq!(config.framing, TxConfig::default().framing);
    }

    #[test]
    fn written_arguments_read_back_as_the_same_framing() {
        let mut config = TxConfig {
            char_gap_bits: 2.0,
            diddle: Diddle::Blank,
            lead_in_seconds: 0.25,
            tail_seconds: 0.125,
            ..TxConfig::default()
        };
        config.framing.parity = Parity::Even;
        config.framing.stop = StopElement::One;
        let written = encode_open(&config);
        let arguments: Vec<&str> = written.split_whitespace().collect();
        let read = decode_open(&arguments).unwrap();
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
    fn malformed_arguments_are_refused(#[case] argument: &str) {
        assert_eq!(decode_open(&[argument]), None);
    }
}
