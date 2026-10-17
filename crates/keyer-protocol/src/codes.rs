use grayline_rtty::TxCode;

/// The longest run of codes one `\rtty_send` may carry.
pub const MAXIMUM_CODES_PER_LINE: usize = 4_096;

/// The digits a five-bit code is written with, by value.
const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";

/// Writes codes in the form `\rtty_send` reads.
///
/// A five-bit code is one digit, `0`–`9` then `A`–`V` by value, and each
/// control code is a lower-case letter, so a run of codes is one word.
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
}
