//! The rules for text that goes on the air.
//!
//! What the transmitter can send and what shape a message is held in are
//! facts about the protocol and the draft, not about the widget the operator
//! types into, so they sit with the application state rather than the view.

use grayline_rtty::code::Ita2Encoder;

/// Whether the transmitter has a code for `character`.
///
/// The line endings are sendable although no single code carries them: the
/// encoder turns a bare line feed into the carriage return and line feed pair
/// the protocol's own line ending uses.
pub fn is_sendable(character: char) -> bool {
    matches!(character, '\n' | '\r') || Ita2Encoder::maps(character)
}

/// The first character of `text` that could not be sent.
pub fn first_unsendable(text: &str) -> Option<char> {
    text.chars().find(|character| !is_sendable(*character))
}

/// Brings text arriving in a block into the shape the field holds.
///
/// Upper cased because that is what leaves the transmitter whatever was
/// written, and a field that showed one and sent the other would be lying
/// about the message. Line endings are reduced to the single character the
/// field stores, so a paste from a document does not leave stray carriage
/// returns between its lines. What has no code at all is deliberately kept.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            characters.next_if_eq(&'\n');
            out.push('\n');
            continue;
        }
        out.push(character.to_ascii_uppercase());
    }
    out
}

/// Brings one keystroke's text into the shape the field holds, dropping what
/// has no code.
pub fn typed(text: &str) -> String {
    text.chars()
        .map(|character| character.to_ascii_uppercase())
        .filter(|character| is_sendable(*character))
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case('A', true)]
    #[case('a', true)]
    #[case('1', true)]
    #[case(' ', true)]
    #[case('-', true)]
    #[case('/', true)]
    #[case('\n', true)]
    #[case('\r', true)]
    #[case('%', false)]
    #[case('{', false)]
    #[case('}', false)]
    #[case('あ', false)]
    #[case('*', false)]
    fn the_sendable_characters_are_the_ones_the_encoder_has_a_code_for(
        #[case] character: char,
        #[case] expected: bool,
    ) {
        assert_eq!(is_sendable(character), expected);
    }

    #[test]
    fn typing_drops_what_cannot_be_sent_and_upper_cases_the_rest() {
        assert_eq!(typed("cq"), "CQ");
        assert_eq!(typed("%"), "");
        assert_eq!(typed("de jl1his"), "DE JL1HIS");
    }

    /// Upper casing is the encoder's own, which folds ASCII and leaves the
    /// rest alone rather than turning one character into two.
    #[test]
    fn upper_casing_does_not_invent_characters() {
        assert_eq!(normalize("ß"), "ß");
        assert_eq!(first_unsendable(&normalize("ß")), Some('ß'));
    }

    #[test]
    fn pasting_keeps_what_cannot_be_sent() {
        assert_eq!(normalize("100% copy"), "100% COPY");
        assert_eq!(first_unsendable(&normalize("100% copy")), Some('%'));
    }

    #[test]
    fn pasted_line_endings_become_the_one_the_field_holds() {
        assert_eq!(normalize("CQ\r\nDE\rTEST\n"), "CQ\nDE\nTEST\n");
    }

    #[test]
    fn a_message_with_nothing_wrong_in_it_has_no_refused_character() {
        assert_eq!(first_unsendable("CQ CQ DE JL1HIS K"), None);
        assert_eq!(first_unsendable(""), None);
    }

    #[test]
    fn the_first_refused_character_is_the_one_reported() {
        assert_eq!(first_unsendable("OK % AND *"), Some('%'));
    }
}
