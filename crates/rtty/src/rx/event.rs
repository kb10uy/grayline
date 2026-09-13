use crate::{code::Case, params::ToneSet};

/// Something a reception produced, stamped with the input sample it happened at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RxEvent {
    /// A character was decoded.
    Character {
        /// The decoded character.
        character: char,
        /// The absolute input sample position.
        sample: u64,
    },
    /// A shift character moved the decoder to the other case.
    CaseChanged {
        /// The case now being read.
        case: Case,
        /// The absolute input sample position.
        sample: u64,
    },
    /// A stop element failed its majority vote.
    FramingError {
        /// The absolute input sample position.
        sample: u64,
    },
    /// A parity bit contradicted its data bits.
    ParityError {
        /// The absolute input sample position.
        sample: u64,
    },
    /// The squelch opened or closed.
    SquelchChanged {
        /// Whether bits are now being framed.
        open: bool,
        /// The absolute input sample position.
        sample: u64,
    },
    /// Automatic frequency control moved the tone pair.
    TonesAdjusted {
        /// The pair now being detected.
        tones: ToneSet,
        /// The absolute input sample position.
        sample: u64,
    },
}

/// The summary a finished reception hands back.
///
/// Deliberately not the text: characters stream out as
/// [`RxEvent::Character`] and the caller keeps what it wants, so a
/// long-running receiver holds no unbounded buffer here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RxOutcome {
    /// How many characters were decoded.
    pub characters: usize,
    /// How many stop elements failed.
    pub framing_errors: usize,
    /// How many parity bits contradicted their data.
    pub parity_errors: usize,
    /// The tone pair in effect at the end, after any AFC movement.
    pub tones: ToneSet,
    /// The case the decoder ended in.
    pub case: crate::code::Case,
}
