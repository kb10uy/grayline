use alloc::vec::Vec;

use crate::{RttyError, code::Ita2Encoder, tx::config::TxConfig};

/// One item in a transmit stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxCode {
    /// One five-bit character to frame and key.
    Character(u8),
    /// Hold mark for three bit periods, one CW identification element.
    HoldMark,
    /// Hold the carrier off for three bit periods.
    CarrierOff,
    /// Suspend diddle until re-enabled.
    DisableDiddle,
    /// Resume diddle.
    EnableDiddle,
}

/// Encodes text into the codes a [`Transmitter`](crate::tx::Transmitter) keys.
///
/// A bare `\n` becomes CR then LF, the order the protocol's own line ending
/// uses; an existing CR LF pair passes through unchanged. An unmappable
/// character is an error naming it and its byte offset in `text`.
pub fn encode_text(text: &str, config: &TxConfig) -> Result<Vec<TxCode>, RttyError> {
    let mut codes = Vec::new();
    encode_each(text, config, |emitted| codes.extend_from_slice(emitted))?;
    Ok(codes)
}

/// Encodes `text`, handing `emit` the codes each source character produced.
///
/// The two sequences are not parallel: the encoder inserts a shift where the
/// case changes and a carriage return before a bare line feed, so a character
/// can become several codes or, once the case already agrees, exactly one.
/// Anything that has to map a position in the keyed stream back onto the text
/// goes through this rather than counting codes.
pub(crate) fn encode_each(text: &str, config: &TxConfig, mut emit: impl FnMut(&[TxCode])) -> Result<(), RttyError> {
    let mut encoder = Ita2Encoder::new(config.code_set, config.double_shift, config.tx_unshift_on_space);
    let mut codes = Vec::new();
    let mut previous = None;
    for (offset, character) in text.char_indices() {
        codes.clear();
        if character == '\n' && previous != Some('\r') {
            push(&mut encoder, '\r', offset, &mut codes)?;
        }
        push(&mut encoder, character, offset, &mut codes)?;
        emit(&codes);
        previous = Some(character);
    }
    Ok(())
}

fn push(encoder: &mut Ita2Encoder, character: char, offset: usize, codes: &mut Vec<TxCode>) -> Result<(), RttyError> {
    let mut out = [0; 3];
    let count = encoder.encode(character, &mut out).map_err(|error| match error {
        RttyError::UnmappableCharacter { character, .. } => RttyError::UnmappableCharacter { character, offset },
        other => other,
    })?;
    codes.extend(out[..count].iter().map(|&code| TxCode::Character(code)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{FIGS, LTRS};

    #[test]
    fn a_bare_newline_expands_to_cr_lf() {
        let codes = encode_text("A\nB", &TxConfig::default()).unwrap();
        assert_eq!(
            codes,
            [
                TxCode::Character(LTRS),
                TxCode::Character(0b11000),
                TxCode::Character(0b00010),
                TxCode::Character(0b01000),
                TxCode::Character(0b10011),
            ]
        );
    }

    #[test]
    fn an_existing_cr_lf_passes_through_unchanged() {
        let bare = encode_text("A\nB", &TxConfig::default()).unwrap();
        let explicit = encode_text("A\r\nB", &TxConfig::default()).unwrap();
        assert_eq!(bare, explicit);
    }

    #[test]
    fn an_unmappable_character_reports_its_byte_offset() {
        let error = encode_text("OK %", &TxConfig::default()).unwrap_err();
        assert_eq!(
            error,
            RttyError::UnmappableCharacter {
                character: '%',
                offset: 3
            }
        );
    }

    #[test]
    fn shifts_appear_where_the_case_changes() {
        let codes = encode_text("A1", &TxConfig::default()).unwrap();
        assert_eq!(codes[0], TxCode::Character(LTRS));
        assert_eq!(codes[2], TxCode::Character(FIGS));
        assert_eq!(codes.len(), 4);
    }
}
