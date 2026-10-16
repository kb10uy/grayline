use crate::{
    RttyError,
    code::{FIGS, LTRS},
    params::TxFraming,
    tx::{
        config::{Diddle, TxConfig},
        encoder::TxCode,
        schedule::{CONTROL_HOLD_BITS, Timing},
    },
};

/// The three states a transmitter's output can be in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Keying {
    /// The mark tone, a logical one and the idle line.
    Mark,
    /// The space tone, a logical zero.
    Space,
    /// No carrier at all: the sample is exactly zero and the oscillator's
    /// phase does not advance. This is a keying state of its own, distinct
    /// from the transmit gate the amplitude ramp shapes; MMTTY cuts it just
    /// as hard (`Rtty.cpp:623`).
    Muted,
}

/// Lays a code stream out as keying elements, timed in a caller's own ticks.
///
/// What a code becomes on the line does not depend on how the line is driven:
/// the same start, data, parity, and stop elements key a tone generator one
/// sample at a time and a serial control line one deadline at a time. A tick
/// is whatever the caller counts in, a sample for audio and a microsecond for
/// a keyer, so the two cannot frame a character from different arithmetic.
///
/// The lead-in and tail are the caller's to place. A transmitter knows where
/// its stream ends, while a keyer fed a code at a time can run dry and has to
/// idle at mark until the next one arrives, which only it can tell apart from
/// the end.
#[derive(Clone, Debug)]
pub struct Framer {
    timing: Timing,
    framing: TxFraming,
    char_gap_bits: f64,
    diddle: Diddle,
    diddle_enabled: bool,
    /// The shift the stream last announced, which a diddle filler must not
    /// contradict: the codes are already encoded, so nothing downstream
    /// would re-announce a case a stray LTRS knocked over.
    announced_shift: u8,
}

impl Framer {
    /// Creates a framer counting in `ticks_per_second`.
    pub fn new(ticks_per_second: u32, config: &TxConfig) -> Result<Self, RttyError> {
        Ok(Self {
            timing: Timing::new(ticks_per_second, config)?,
            framing: config.framing,
            char_gap_bits: config.char_gap_bits,
            diddle: config.diddle,
            diddle_enabled: true,
            announced_shift: LTRS,
        })
    }

    /// One bit period, in ticks.
    pub fn ticks_per_bit(&self) -> f64 {
        self.timing.samples_per_bit
    }

    /// The mark idle before the first character, in ticks.
    pub fn lead_in_ticks(&self) -> f64 {
        self.timing.lead_in_samples
    }

    /// The mark idle after the last character, in ticks.
    pub fn tail_ticks(&self) -> f64 {
        self.timing.tail_samples
    }

    pub(crate) const fn timing(&self) -> Timing {
        self.timing
    }

    /// Appends the elements `code` keys to `elements`, each with its length.
    ///
    /// A code that only changes the framer's state appends nothing.
    pub fn frame(&mut self, code: TxCode, elements: &mut impl Extend<(Keying, f64)>) {
        match code {
            TxCode::Character(code) => {
                if code == LTRS || code == FIGS {
                    self.announced_shift = code;
                }
                self.frame_character(code, elements);
                self.frame_gap(elements);
            }
            TxCode::HoldMark => elements.extend([(Keying::Mark, CONTROL_HOLD_BITS * self.timing.samples_per_bit)]),
            TxCode::CarrierOff => elements.extend([(Keying::Muted, CONTROL_HOLD_BITS * self.timing.samples_per_bit)]),
            TxCode::DisableDiddle => self.diddle_enabled = false,
            TxCode::EnableDiddle => self.diddle_enabled = true,
        }
    }

    /// Frames one character: start, data b1-first, parity, then the stop
    /// element as one mark of one bit plus the configured extra.
    fn frame_character(&self, code: u8, elements: &mut impl Extend<(Keying, f64)>) {
        let bit = self.timing.samples_per_bit;
        elements.extend([(Keying::Space, bit)]);
        let bits = self.framing.bits.bits();
        let mut marks = 0;
        for index in (0..bits).rev() {
            let is_mark = (code >> index) & 1 == 1;
            marks += u32::from(is_mark);
            elements.extend([(if is_mark { Keying::Mark } else { Keying::Space }, bit)]);
        }
        if let Some(parity) = self.framing.parity.transmitted_bit(marks) {
            elements.extend([(if parity { Keying::Mark } else { Keying::Space }, bit)]);
        }
        elements.extend([(Keying::Mark, (1.0 + self.framing.stop.extra_bits()) * bit)]);
    }

    /// Fills the character gap with diddle characters where they fit, and
    /// mark idle for whatever is left.
    fn frame_gap(&self, elements: &mut impl Extend<(Keying, f64)>) {
        let mut remaining = self.char_gap_bits;
        if remaining <= 0.0 {
            return;
        }
        let diddle = if self.diddle_enabled { self.diddle } else { Diddle::None };
        let filler = match diddle {
            Diddle::None => None,
            // The filler repeats whichever shift the stream last announced:
            // a bare LTRS in the middle of figures text would silently move
            // the receiver's case out from under the remaining codes.
            Diddle::Ltrs => Some(self.announced_shift),
            Diddle::Blank => Some(0),
        };
        if let Some(code) = filler {
            while remaining >= self.timing.character_bits {
                self.frame_character(code, elements);
                remaining -= self.timing.character_bits;
            }
        }
        if remaining > 0.0 {
            elements.extend([(Keying::Mark, remaining * self.timing.samples_per_bit)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use rstest::rstest;

    use super::*;
    use crate::params::{Parity, StopElement};

    /// One tick per bit, so element lengths read as bit counts.
    const BITS: u32 = 1_000;

    fn config() -> TxConfig {
        let mut config = TxConfig {
            lead_in_seconds: 0.0,
            tail_seconds: 0.0,
            ..TxConfig::default()
        };
        config.framing.baud = crate::params::BaudRate::new(f64::from(BITS)).unwrap();
        config
    }

    fn framed(codes: &[TxCode], config: &TxConfig) -> Vec<(Keying, f64)> {
        let mut framer = Framer::new(BITS * BITS, config).unwrap();
        let mut elements = Vec::new();
        for code in codes {
            framer.frame(*code, &mut elements);
        }
        elements
            .into_iter()
            .map(|(keying, ticks)| (keying, ticks / framer.ticks_per_bit()))
            .collect()
    }

    #[test]
    fn a_character_frames_start_data_b1_first_and_stop() {
        use Keying::{Mark, Space};
        assert_eq!(
            framed(&[TxCode::Character(0b10000)], &config()),
            vec![
                (Space, 1.0),
                (Mark, 1.0),
                (Space, 1.0),
                (Space, 1.0),
                (Space, 1.0),
                (Space, 1.0),
                (Mark, 1.5),
            ]
        );
    }

    #[rstest]
    #[case(StopElement::One, 1.0)]
    #[case(StopElement::OneAndAHalf, 1.5)]
    #[case(StopElement::Two, 2.0)]
    fn the_stop_element_follows_the_configuration(#[case] stop: StopElement, #[case] bits: f64) {
        let mut config = config();
        config.framing.stop = stop;
        let elements = framed(&[TxCode::Character(0)], &config);
        assert_eq!(elements.last(), Some(&(Keying::Mark, bits)));
    }

    #[test]
    fn parity_sits_between_the_data_and_the_stop() {
        let mut config = config();
        config.framing.parity = Parity::Mark;
        let elements = framed(&[TxCode::Character(0)], &config);
        assert_eq!(elements.len(), 8);
        assert_eq!(elements[6], (Keying::Mark, 1.0));
    }

    #[test]
    fn control_codes_hold_for_three_bits_or_frame_nothing() {
        let elements = framed(
            &[
                TxCode::HoldMark,
                TxCode::CarrierOff,
                TxCode::DisableDiddle,
                TxCode::EnableDiddle,
            ],
            &config(),
        );
        assert_eq!(elements, vec![(Keying::Mark, 3.0), (Keying::Muted, 3.0)]);
    }

    #[test]
    fn diddle_repeats_the_announced_shift_and_idles_the_remainder() {
        let mut config = config();
        config.char_gap_bits = 16.0;
        config.diddle = Diddle::Ltrs;
        let elements = framed(&[TxCode::Character(FIGS)], &config);
        let total: f64 = elements.iter().map(|(_, bits)| bits).sum();
        assert!((total - 23.5).abs() < 1.0e-9);
        assert_eq!(elements.last(), Some(&(Keying::Mark, 1.0)));
        let figs = framed(
            &[TxCode::Character(FIGS)],
            &TxConfig {
                char_gap_bits: 0.0,
                ..config
            },
        );
        assert_eq!(&elements[7..14], &figs[..]);
        assert_eq!(&elements[14..21], &figs[..]);
    }

    #[test]
    fn disabled_diddle_leaves_the_gap_as_mark_idle() {
        let mut config = config();
        config.char_gap_bits = 16.0;
        config.diddle = Diddle::Ltrs;
        let elements = framed(&[TxCode::DisableDiddle, TxCode::Character(0)], &config);
        assert_eq!(elements.len(), 8);
        assert_eq!(elements.last(), Some(&(Keying::Mark, 16.0)));
    }
}
