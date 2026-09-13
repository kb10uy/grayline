use alloc::vec::Vec;

use crate::{
    RttyError,
    params::{BitLength, TxFraming},
    tx::{config::TxConfig, encoder::TxCode},
};

/// How many bit periods one held control element lasts.
pub(crate) const CONTROL_HOLD_BITS: f64 = 3.0;
/// The fewest samples a majority-vote receiver needs from one bit.
const MINIMUM_SAMPLES_PER_BIT: f64 = 8.0;

/// The lengths a transmitter lays its stream out from, in samples.
///
/// Shared with [`Transmitter`](crate::tx::Transmitter) so that a schedule and
/// the audio it describes cannot be derived from different arithmetic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Timing {
    pub samples_per_bit: f64,
    /// The bit periods one framed character occupies, gap excluded.
    pub character_bits: f64,
    pub lead_in_samples: f64,
    pub tail_samples: f64,
    pub ramp_samples: f64,
}

impl Timing {
    pub(crate) fn new(sample_rate_hz: u32, config: &TxConfig) -> Result<Self, RttyError> {
        let rate = f64::from(sample_rate_hz);
        if config.framing.bits != BitLength::FIVE {
            return Err(RttyError::UnsupportedBitLength);
        }
        let samples_per_bit = rate / config.framing.baud.bits_per_second();
        if !samples_per_bit.is_finite() || samples_per_bit < MINIMUM_SAMPLES_PER_BIT {
            return Err(RttyError::TooFewSamplesPerBit);
        }
        for seconds in [
            config.char_gap_bits,
            config.ramp_seconds,
            config.lead_in_seconds,
            config.tail_seconds,
        ] {
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(RttyError::InvalidTransmitTiming);
            }
        }
        Ok(Self {
            samples_per_bit,
            character_bits: character_bits(config.framing),
            lead_in_samples: config.lead_in_seconds * rate,
            tail_samples: config.tail_seconds * rate,
            ramp_samples: config.ramp_seconds * rate,
        })
    }

    /// How long `code` occupies the transmission, in samples.
    ///
    /// A character carries its trailing gap, whose length does not depend on
    /// what fills it: whole diddle characters and the mark idle after them
    /// together come to the configured gap either way.
    pub(crate) fn code_samples(self, code: TxCode, char_gap_bits: f64) -> f64 {
        match code {
            TxCode::Character(_) => (self.character_bits + char_gap_bits) * self.samples_per_bit,
            TxCode::HoldMark | TxCode::CarrierOff => CONTROL_HOLD_BITS * self.samples_per_bit,
            TxCode::DisableDiddle | TxCode::EnableDiddle => 0.0,
        }
    }
}

/// The bit periods one framed character occupies: start, data, parity, stop.
pub(crate) fn character_bits(framing: TxFraming) -> f64 {
    let parity_bits = if framing.parity.transmitted_bit(0).is_some() {
        1.0
    } else {
        0.0
    };
    1.0 + f64::from(framing.bits.bits()) + parity_bits + 1.0 + framing.stop.extra_bits()
}

/// Where each character of a message goes out, in output samples.
///
/// A playback device reports how far it has played, and that number means
/// nothing about the message on its own: the encoder inserts shift characters
/// that are codes with no character behind them, and the lead-in idles before
/// any of it. This maps one onto the other, so a transmission can be underlined
/// as it goes out, echoed a character at a time, and cut off with whatever is
/// left still known.
#[derive(Clone, Debug, PartialEq)]
pub struct TxSchedule {
    ends: Vec<u64>,
    total_samples: u64,
}

impl TxSchedule {
    /// Lays `text` out against the audio a transmitter would produce from it.
    ///
    /// Fails wherever [`encode_text`](crate::tx::encode_text) and
    /// [`Transmitter::new`](crate::tx::Transmitter::new) would, so a schedule
    /// exists only for a message that can actually be sent.
    pub fn new(text: &str, sample_rate_hz: u32, config: &TxConfig) -> Result<Self, RttyError> {
        let timing = Timing::new(sample_rate_hz, config)?;
        let mut position = timing.lead_in_samples;
        let mut ends = Vec::new();
        crate::tx::encoder::encode_each(text, config, |codes| {
            for code in codes {
                position += timing.code_samples(*code, config.char_gap_bits);
            }
            ends.push(libm::ceil(position) as u64);
        })?;
        let total_samples = libm::ceil(position + timing.tail_samples) as u64;
        Ok(Self { ends, total_samples })
    }

    /// The whole transmission, lead-in and tail included.
    pub fn total_samples(&self) -> u64 {
        self.total_samples
    }

    /// The number of characters in the message this was built from.
    pub fn character_count(&self) -> usize {
        self.ends.len()
    }

    /// Where each character finishes going out, by its position in the text.
    pub fn character_ends(&self) -> &[u64] {
        &self.ends
    }

    /// How many characters have finished going out by `played_samples`.
    ///
    /// Counted on the character being finished rather than started, because
    /// what this marks is text that has left: a character half keyed when the
    /// operator cuts the transmission was not sent.
    pub fn characters_sent_by(&self, played_samples: u64) -> usize {
        self.ends.partition_point(|end| *end <= played_samples)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use rstest::rstest;

    use super::*;
    use crate::tx::{Transmitter, encode_text};

    const RATE: u32 = 8_000;

    fn produced_samples(text: &str, config: &TxConfig) -> u64 {
        let codes = encode_text(text, config).unwrap();
        let mut transmitter = Transmitter::new(codes.into_iter(), RATE, *config).unwrap();
        let mut block = [0.0_f32; 1_024];
        let mut total = 0_u64;
        loop {
            let written = transmitter.process(&mut block).unwrap();
            if written == 0 {
                return total;
            }
            total += written as u64;
        }
    }

    /// The schedule exists to describe audio somebody else produces, so the
    /// length it claims has to be the length that comes out.
    #[rstest]
    #[case(TxConfig::default())]
    #[case(TxConfig { char_gap_bits: 2.5, diddle: crate::tx::Diddle::Ltrs, ..TxConfig::default() })]
    #[case(TxConfig { char_gap_bits: 1.0, diddle: crate::tx::Diddle::None, ..TxConfig::default() })]
    #[case(TxConfig { lead_in_seconds: 0.0, tail_seconds: 0.0, ..TxConfig::default() })]
    #[case(TxConfig { framing: TxFraming { parity: crate::params::Parity::Even, ..TxFraming::default() }, ..TxConfig::default() })]
    #[case(TxConfig { framing: TxFraming { stop: crate::params::StopElement::Two, ..TxFraming::default() }, ..TxConfig::default() })]
    fn the_scheduled_length_is_the_length_that_is_produced(#[case] config: TxConfig) {
        let text = "CQ CQ DE JL1HIS 599\r\n";
        let schedule = TxSchedule::new(text, RATE, &config).unwrap();
        let produced = produced_samples(text, &config);
        // One sample of slack: the transmitter starts a sample once its
        // position is inside a segment, so a boundary landing mid-sample
        // rounds the same way at most one place.
        let difference = schedule.total_samples().abs_diff(produced);
        assert!(
            difference <= 1,
            "scheduled {} but produced {produced}",
            schedule.total_samples()
        );
    }

    #[test]
    fn a_character_is_scheduled_for_every_character_of_the_text() {
        let schedule = TxSchedule::new("CQ DE", RATE, &TxConfig::default()).unwrap();
        assert_eq!(schedule.character_count(), 5);
    }

    /// A newline is one character of the message and two codes on the air, and
    /// the shift before a figure is a code with no character at all: neither
    /// may shift the text out from under the positions.
    #[test]
    fn inserted_codes_do_not_displace_the_characters_they_serve() {
        let config = TxConfig::default();
        let schedule = TxSchedule::new("A1\nB", RATE, &config).unwrap();
        assert_eq!(schedule.character_count(), 4);

        let timing = Timing::new(RATE, &config).unwrap();
        let character = timing.code_samples(TxCode::Character(0), config.char_gap_bits);
        let ends = schedule.character_ends();
        // LTRS then A, FIGS then 1, CR LF, LTRS then B.
        let counts = [2.0, 2.0, 2.0, 2.0];
        let mut expected = timing.lead_in_samples;
        for (end, count) in ends.iter().zip(counts) {
            expected += count * character;
            assert_eq!(*end, libm::ceil(expected) as u64);
        }
    }

    #[test]
    fn nothing_has_been_sent_while_the_lead_in_is_still_playing() {
        let config = TxConfig::default();
        let schedule = TxSchedule::new("RY", RATE, &config).unwrap();
        let lead_in = (config.lead_in_seconds * f64::from(RATE)) as u64;
        assert_eq!(schedule.characters_sent_by(0), 0);
        assert_eq!(schedule.characters_sent_by(lead_in), 0);
    }

    #[test]
    fn a_played_position_counts_the_characters_that_finished_before_it() {
        let schedule = TxSchedule::new("RYRY", RATE, &TxConfig::default()).unwrap();
        let ends = schedule.character_ends().to_vec();
        for (index, end) in ends.iter().enumerate() {
            assert_eq!(schedule.characters_sent_by(*end), index + 1);
            assert_eq!(schedule.characters_sent_by(end - 1), index);
        }
        assert_eq!(schedule.characters_sent_by(schedule.total_samples()), ends.len());
    }

    #[test]
    fn an_empty_message_schedules_nothing_but_still_has_a_length() {
        let schedule = TxSchedule::new("", RATE, &TxConfig::default()).unwrap();
        assert_eq!(schedule.character_count(), 0);
        assert!(schedule.total_samples() > 0);
        assert_eq!(schedule.characters_sent_by(u64::MAX), 0);
    }

    /// A message that cannot be encoded has no schedule, rather than one that
    /// stops short of the text it was given.
    #[test]
    fn an_unmappable_character_is_refused_where_the_encoder_refuses_it() {
        let error = TxSchedule::new("OK %", RATE, &TxConfig::default()).unwrap_err();
        assert_eq!(
            error,
            RttyError::UnmappableCharacter {
                character: '%',
                offset: 3
            }
        );
    }

    #[test]
    fn a_rate_too_low_for_the_speed_has_no_schedule() {
        let error = TxSchedule::new("RY", 100, &TxConfig::default()).unwrap_err();
        assert_eq!(error, RttyError::TooFewSamplesPerBit);
    }

    /// The gap is the same length whether diddle characters fill it or mark
    /// idle does, which is what lets one duration cover a character.
    #[test]
    fn a_filled_gap_and_an_idle_one_are_the_same_length() {
        let text = "RYRY";
        let idle = TxConfig {
            char_gap_bits: 9.0,
            diddle: crate::tx::Diddle::None,
            ..TxConfig::default()
        };
        let filled = TxConfig {
            diddle: crate::tx::Diddle::Ltrs,
            ..idle
        };
        assert_eq!(
            TxSchedule::new(text, RATE, &idle).unwrap(),
            TxSchedule::new(text, RATE, &filled).unwrap()
        );
    }

    /// Long messages are what an operator actually sends, and a position that
    /// drifted from the audio would put the underline in the wrong place by
    /// the end of one.
    #[test]
    fn positions_hold_against_the_audio_over_a_long_message() {
        let config = TxConfig::default();
        let text: String = core::iter::repeat_n("RYRYRYRY ", 40).collect();
        let schedule = TxSchedule::new(&text, RATE, &config).unwrap();
        let produced = produced_samples(&text, &config);
        assert!(schedule.total_samples().abs_diff(produced) <= 1);
        assert_eq!(schedule.character_count(), text.chars().count());
    }

    #[test]
    fn control_codes_hold_the_line_for_three_bits() {
        let config = TxConfig::default();
        let timing = Timing::new(RATE, &config).unwrap();
        let hold = CONTROL_HOLD_BITS * timing.samples_per_bit;
        for code in [TxCode::HoldMark, TxCode::CarrierOff] {
            assert_eq!(timing.code_samples(code, config.char_gap_bits), hold);
        }
        for code in [TxCode::DisableDiddle, TxCode::EnableDiddle] {
            assert_eq!(timing.code_samples(code, config.char_gap_bits), 0.0);
        }
    }

    #[rstest]
    #[case(TxFraming::default(), 7.5)]
    #[case(TxFraming { stop: crate::params::StopElement::One, ..TxFraming::default() }, 7.0)]
    #[case(TxFraming { stop: crate::params::StopElement::Two, ..TxFraming::default() }, 8.0)]
    #[case(TxFraming { parity: crate::params::Parity::Odd, ..TxFraming::default() }, 8.5)]
    fn a_framed_character_is_as_long_as_its_framing(#[case] framing: TxFraming, #[case] expected: f64) {
        assert_eq!(character_bits(framing), expected);
    }

    #[test]
    fn timing_is_refused_for_a_length_the_transmitter_cannot_key() {
        let config = TxConfig {
            framing: TxFraming {
                bits: BitLength::new(8).unwrap(),
                ..TxFraming::default()
            },
            ..TxConfig::default()
        };
        assert_eq!(Timing::new(RATE, &config).unwrap_err(), RttyError::UnsupportedBitLength);
    }

    #[rstest]
    #[case(TxConfig { lead_in_seconds: -1.0, ..TxConfig::default() })]
    #[case(TxConfig { tail_seconds: f64::NAN, ..TxConfig::default() })]
    #[case(TxConfig { ramp_seconds: -0.1, ..TxConfig::default() })]
    #[case(TxConfig { char_gap_bits: -1.0, ..TxConfig::default() })]
    fn timing_is_refused_for_a_length_that_is_not_a_length(#[case] config: TxConfig) {
        assert_eq!(
            Timing::new(RATE, &config).unwrap_err(),
            RttyError::InvalidTransmitTiming
        );
    }

    #[test]
    fn a_schedule_covers_every_code_the_encoder_emitted() {
        let config = TxConfig::default();
        let text = "1A2B";
        let codes = encode_text(text, &config).unwrap();
        let timing = Timing::new(RATE, &config).unwrap();
        let coded: f64 = codes
            .iter()
            .map(|code| timing.code_samples(*code, config.char_gap_bits))
            .sum();
        let schedule = TxSchedule::new(text, RATE, &config).unwrap();
        let last = *schedule.character_ends().last().unwrap();
        assert_eq!(last, libm::ceil(timing.lead_in_samples + coded) as u64);
        assert_eq!(codes.len(), 8);
    }
}
