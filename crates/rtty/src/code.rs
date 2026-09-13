//! ITA2 character conversion and shift tracking.
//!
//! Both tables are indexed with the first-transmitted bit, b1, as the most
//! significant bit of the five-bit code, exactly as MMTTY's `_LTR` and `_FIG`
//! are. Getting that convention wrong flips every character, which is why it
//! is stated once here and pinned by the tests below.

use crate::RttyError;

/// The letters-case shift code.
pub const LTRS: u8 = 0b11111;
/// The figures-case shift code.
pub const FIGS: u8 = 0b11011;
/// The code for BELL in the S-BELL convention, FIGS-S.
const S_BELL_CODE: u8 = 0b10100;
/// The code for the apostrophe in the S-BELL convention, FIGS-J.
const S_APOSTROPHE_CODE: u8 = 0b11010;
const BELL: char = '\u{7}';

/// The shift case a decoder is reading in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Case {
    /// Letters, the idle-line default.
    #[default]
    Letters,
    /// Figures, entered by FIGS and left by LTRS.
    Figures,
}

/// Which of the two BELL conventions is in use.
///
/// US teleprinter code puts BELL at FIGS-S and the apostrophe at FIGS-J; the
/// international assignment swaps them. A station using the wrong one prints
/// an apostrophe where a bell was meant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CodeSet {
    /// BELL at FIGS-S, the US and amateur norm.
    #[default]
    SBell,
    /// BELL at FIGS-J, the international assignment.
    JBell,
}

/// The letters column, indexed by the code with b1 as the MSB.
const LETTERS: [Option<char>; 32] = [
    None, // NUL
    Some('T'),
    Some('\r'),
    Some('O'),
    Some(' '),
    Some('H'),
    Some('N'),
    Some('M'),
    Some('\n'),
    Some('L'),
    Some('R'),
    Some('G'),
    Some('I'),
    Some('P'),
    Some('C'),
    Some('V'),
    Some('E'),
    Some('Z'),
    Some('D'),
    Some('B'),
    Some('S'),
    Some('Y'),
    Some('F'),
    Some('X'),
    Some('A'),
    Some('W'),
    Some('J'),
    None, // FIGS
    Some('U'),
    Some('Q'),
    Some('K'),
    None, // LTRS
];

/// The figures column in the S-BELL arrangement, indexed like [`LETTERS`].
///
/// FIGS-H prints `#`, the US teleprinter assignment; MMTTY prints `h` there
/// only because `#` is one of its own macro metacharacters, a constraint this
/// project does not have.
const FIGURES: [Option<char>; 32] = [
    None, // NUL
    Some('5'),
    Some('\r'),
    Some('9'),
    Some(' '),
    Some('#'),
    Some(','),
    Some('.'),
    Some('\n'),
    Some(')'),
    Some('4'),
    Some('&'),
    Some('8'),
    Some('0'),
    Some(':'),
    Some(';'),
    Some('3'),
    Some('"'),
    Some('$'),
    Some('?'),
    Some(BELL),
    Some('6'),
    Some('!'),
    Some('/'),
    Some('-'),
    Some('2'),
    Some('\''),
    None, // FIGS
    Some('7'),
    Some('1'),
    Some('('),
    None, // LTRS
];

/// A stateful ITA2-to-character decoder.
#[derive(Clone, Copy, Debug)]
pub struct Ita2Decoder {
    case: Case,
    code_set: CodeSet,
    unshift_on_space: bool,
}

impl Ita2Decoder {
    /// Creates a decoder starting in letters case.
    pub const fn new(code_set: CodeSet, unshift_on_space: bool) -> Self {
        Self {
            case: Case::Letters,
            code_set,
            unshift_on_space,
        }
    }

    /// Returns the character a code prints, or nothing for LTRS, FIGS, and NUL.
    ///
    /// LTRS and FIGS update the case instead of printing, which the caller
    /// observes through [`Ita2Decoder::case`]. With unshift-on-space, a space
    /// received in figures case also returns the decoder to letters.
    pub fn decode(&mut self, code: u8) -> Option<char> {
        let code = code & 0b11111;
        match code {
            LTRS => {
                self.case = Case::Letters;
                None
            }
            FIGS => {
                self.case = Case::Figures;
                None
            }
            _ => {
                let character = match self.case {
                    Case::Letters => LETTERS[usize::from(code)],
                    Case::Figures => figure_at(code, self.code_set),
                };
                if self.unshift_on_space && self.case == Case::Figures && character == Some(' ') {
                    self.case = Case::Letters;
                }
                character
            }
        }
    }

    /// Returns the case the decoder is reading in.
    pub const fn case(&self) -> Case {
        self.case
    }

    /// Returns to letters case, as an idle line eventually would.
    pub fn reset(&mut self) {
        self.case = Case::Letters;
    }
}

fn figure_at(code: u8, code_set: CodeSet) -> Option<char> {
    let code = match (code_set, code) {
        (CodeSet::JBell, S_BELL_CODE) => S_APOSTROPHE_CODE,
        (CodeSet::JBell, S_APOSTROPHE_CODE) => S_BELL_CODE,
        _ => code,
    };
    FIGURES[usize::from(code)]
}

/// The case a transmitter has last announced.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxCase {
    /// No shift has been sent, or TX unshift-on-space discarded the last one,
    /// so the next cased character must carry a fresh shift.
    #[default]
    Unknown,
    /// LTRS was announced.
    Letters,
    /// FIGS was announced.
    Figures,
}

/// A stateful character-to-ITA2 encoder.
#[derive(Clone, Copy, Debug)]
pub struct Ita2Encoder {
    case: TxCase,
    code_set: CodeSet,
    double_shift: bool,
    tx_unshift_on_space: bool,
}

impl Ita2Encoder {
    /// Creates an encoder that has not yet announced a case.
    pub const fn new(code_set: CodeSet, double_shift: bool, tx_unshift_on_space: bool) -> Self {
        Self {
            case: TxCase::Unknown,
            code_set,
            double_shift,
            tx_unshift_on_space,
        }
    }

    /// Returns whether a character has an ITA2 code at all.
    pub fn maps(character: char) -> bool {
        lookup(character.to_ascii_uppercase(), CodeSet::SBell).is_some()
    }

    /// Writes the codes one character needs, shift included, and returns how many.
    ///
    /// Lowercase letters encode as their uppercase codes. An unmappable
    /// character is an error carrying the character itself; the caller knows
    /// where in its input it was.
    pub fn encode(&mut self, character: char, out: &mut [u8; 3]) -> Result<usize, RttyError> {
        let (code, target) = lookup(character.to_ascii_uppercase(), self.code_set)
            .ok_or(RttyError::UnmappableCharacter { character, offset: 0 })?;
        let mut written = 0;
        if let Some(target) = target
            && self.case != target
        {
            let shift = match target {
                TxCase::Letters => LTRS,
                _ => FIGS,
            };
            out[written] = shift;
            written += 1;
            if self.double_shift {
                out[written] = shift;
                written += 1;
            }
            self.case = target;
        }
        out[written] = code;
        written += 1;
        if self.tx_unshift_on_space && code == 0b00100 && self.case == TxCase::Figures {
            // TX unshift-on-space: forget the announced case after a space in
            // figures, so the next cased character re-announces it and a UOS
            // receiver stays in step.
            self.case = TxCase::Unknown;
        }
        Ok(written)
    }

    /// Forgets the announced case, as the start of a transmission does.
    pub fn reset(&mut self) {
        self.case = TxCase::Unknown;
    }
}

/// Maps an uppercase character to its code and the case it lives in.
///
/// A character present in both columns needs no case at all and returns
/// `None` there, so it never forces a shift.
fn lookup(character: char, code_set: CodeSet) -> Option<(u8, Option<TxCase>)> {
    for code in 0..32_u8 {
        if LETTERS[usize::from(code)] == Some(character) {
            return if figure_at(code, code_set) == Some(character) {
                Some((code, None))
            } else {
                Some((code, Some(TxCase::Letters)))
            };
        }
    }
    for code in 0..32_u8 {
        if figure_at(code, code_set) == Some(character) {
            return Some((code, Some(TxCase::Figures)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn decoded(code_set: CodeSet, case: Case, code: u8) -> Option<char> {
        let mut decoder = Ita2Decoder::new(code_set, false);
        decoder.decode(if case == Case::Figures { FIGS } else { LTRS });
        decoder.decode(code)
    }

    /// Asymmetric bit patterns that would flip if the b1-as-MSB convention
    /// were reversed anywhere.
    #[rstest]
    #[case(0b00001, 'T', '5')]
    #[case(0b10000, 'E', '3')]
    #[case(0b00010, '\r', '\r')]
    #[case(0b01000, '\n', '\n')]
    #[case(0b01101, 'P', '0')]
    #[case(0b10110, 'F', '!')]
    #[case(0b00101, 'H', '#')]
    #[case(0b11000, 'A', '-')]
    #[case(0b00011, 'O', '9')]
    #[case(0b11100, 'U', '7')]
    fn codes_index_with_b1_as_the_most_significant_bit(#[case] code: u8, #[case] letter: char, #[case] figure: char) {
        assert_eq!(decoded(CodeSet::SBell, Case::Letters, code), Some(letter));
        assert_eq!(decoded(CodeSet::SBell, Case::Figures, code), Some(figure));
    }

    #[rstest]
    #[case(CodeSet::SBell, 0b10100, '\u{7}', 0b11010, '\'')]
    #[case(CodeSet::JBell, 0b10100, '\'', 0b11010, '\u{7}')]
    fn the_code_set_places_bell(
        #[case] code_set: CodeSet,
        #[case] s_code: u8,
        #[case] s_char: char,
        #[case] j_code: u8,
        #[case] j_char: char,
    ) {
        assert_eq!(decoded(code_set, Case::Figures, s_code), Some(s_char));
        assert_eq!(decoded(code_set, Case::Figures, j_code), Some(j_char));
    }

    #[test]
    fn shifts_and_nul_print_nothing_and_move_the_case() {
        let mut decoder = Ita2Decoder::new(CodeSet::SBell, false);
        assert_eq!(decoder.decode(0), None);
        assert_eq!(decoder.case(), Case::Letters);
        assert_eq!(decoder.decode(FIGS), None);
        assert_eq!(decoder.case(), Case::Figures);
        assert_eq!(decoder.decode(0), None);
        assert_eq!(decoder.case(), Case::Figures);
        assert_eq!(decoder.decode(LTRS), None);
        assert_eq!(decoder.case(), Case::Letters);
    }

    #[test]
    fn unshift_on_space_clears_figures_and_leaves_letters_alone() {
        let mut decoder = Ita2Decoder::new(CodeSet::SBell, true);
        decoder.decode(FIGS);
        assert_eq!(decoder.decode(0b00100), Some(' '));
        assert_eq!(decoder.case(), Case::Letters);
        assert_eq!(decoder.decode(0b00100), Some(' '));
        assert_eq!(decoder.case(), Case::Letters);
    }

    #[test]
    fn without_uos_a_space_stays_in_figures() {
        let mut decoder = Ita2Decoder::new(CodeSet::SBell, false);
        decoder.decode(FIGS);
        assert_eq!(decoder.decode(0b00100), Some(' '));
        assert_eq!(decoder.case(), Case::Figures);
    }

    #[rstest]
    #[case(CodeSet::SBell)]
    #[case(CodeSet::JBell)]
    fn every_character_round_trips(#[case] code_set: CodeSet) {
        let mut encoder = Ita2Encoder::new(code_set, false, false);
        let mut decoder = Ita2Decoder::new(code_set, false);
        let mut checked = 0;
        for candidate in 0..128_u8 {
            let character = char::from(candidate);
            if !Ita2Encoder::maps(character) {
                continue;
            }
            let mut codes = [0; 3];
            let count = encoder.encode(character, &mut codes).unwrap();
            let printed: Option<char> = codes[..count].iter().find_map(|&code| decoder.decode(code));
            assert_eq!(printed, Some(character.to_ascii_uppercase()), "code {candidate:#x}");
            checked += 1;
        }
        // 26 letters twice, 10 digits, the shared and figures punctuation.
        assert!(checked > 60, "only {checked} characters mapped");
    }

    #[test]
    fn shifts_are_emitted_only_on_case_changes() {
        let mut encoder = Ita2Encoder::new(CodeSet::SBell, false, false);
        let mut out = [0; 3];
        assert_eq!(encoder.encode('A', &mut out).unwrap(), 2);
        assert_eq!(out[0], LTRS);
        assert_eq!(encoder.encode('B', &mut out).unwrap(), 1);
        assert_eq!(encoder.encode('1', &mut out).unwrap(), 2);
        assert_eq!(out[0], FIGS);
        assert_eq!(encoder.encode('2', &mut out).unwrap(), 1);
        assert_eq!(encoder.encode('C', &mut out).unwrap(), 2);
        assert_eq!(out[0], LTRS);
    }

    #[test]
    fn caseless_characters_never_force_a_shift() {
        let mut encoder = Ita2Encoder::new(CodeSet::SBell, false, false);
        let mut out = [0; 3];
        for character in [' ', '\r', '\n'] {
            assert_eq!(encoder.encode(character, &mut out).unwrap(), 1, "{character:?}");
        }
        assert_eq!(encoder.encode('E', &mut out).unwrap(), 2);
    }

    #[test]
    fn double_shift_duplicates_the_shift_character() {
        let mut encoder = Ita2Encoder::new(CodeSet::SBell, true, false);
        let mut out = [0; 3];
        assert_eq!(encoder.encode('9', &mut out).unwrap(), 3);
        assert_eq!(&out[..2], &[FIGS, FIGS]);
    }

    #[test]
    fn tx_unshift_on_space_forces_a_fresh_shift_after_a_figures_space() {
        let mut encoder = Ita2Encoder::new(CodeSet::SBell, false, true);
        let mut out = [0; 3];
        encoder.encode('1', &mut out).unwrap();
        encoder.encode(' ', &mut out).unwrap();
        assert_eq!(encoder.encode('2', &mut out).unwrap(), 2);
        assert_eq!(out[0], FIGS);
    }

    #[test]
    fn lowercase_encodes_as_uppercase() {
        let mut encoder = Ita2Encoder::new(CodeSet::SBell, false, false);
        let mut lower = [0; 3];
        let count = encoder.encode('q', &mut lower).unwrap();
        let mut upper = [0; 3];
        let mut reference = Ita2Encoder::new(CodeSet::SBell, false, false);
        assert_eq!(reference.encode('Q', &mut upper).unwrap(), count);
        assert_eq!(lower, upper);
    }

    #[test]
    fn unmappable_characters_are_an_error() {
        let mut encoder = Ita2Encoder::new(CodeSet::SBell, false, false);
        let mut out = [0; 3];
        assert_eq!(
            encoder.encode('%', &mut out).unwrap_err(),
            RttyError::UnmappableCharacter {
                character: '%',
                offset: 0
            }
        );
        assert!(!Ita2Encoder::maps('%'));
    }

    #[test]
    fn the_underscore_family_is_ordinary_unmappable_text() {
        // MMTTY burns these four onto its transmit control codes; here the
        // control codes are typed, so the characters are simply absent from
        // ITA2 like any other unmappable input.
        for character in ['_', '~', '[', ']'] {
            assert!(!Ita2Encoder::maps(character), "{character:?}");
        }
    }
}
