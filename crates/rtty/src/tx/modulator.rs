use alloc::collections::VecDeque;
use core::f64::consts::FRAC_PI_2;

use grayline_dsp::{
    filter::{Fir, FirDesign, FirKind, MovingAverage, MovingAverageDesign},
    oscillator::Vco,
};

use crate::{
    RttyError,
    code::LTRS,
    params::BitLength,
    tx::{
        config::{Diddle, TxConfig},
        encoder::TxCode,
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

/// How many bit periods one held CW element lasts.
const CONTROL_HOLD_BITS: f64 = 3.0;
/// Width added on each side of the tones by the transmit band-pass, in hertz.
const BAND_PASS_MARGIN_HZ: f64 = 150.0;
/// Taps in the transmit band-pass, MMTTY's own count.
const BAND_PASS_ORDER: usize = 48;
/// The fewest samples a majority-vote receiver needs from one bit.
const MINIMUM_SAMPLES_PER_BIT: f64 = 8.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Stream,
    Tail,
    Done,
}

/// A pull-based RTTY transmitter: codes in, normalized PCM out.
pub struct Transmitter<I: Iterator<Item = TxCode>> {
    codes: I,
    stage: Stage,
    pending: VecDeque<(Keying, f64)>,
    diddle_enabled: bool,
    keying: Keying,
    position: u64,
    boundary: f64,
    samples_per_bit: f64,
    character_bits: f64,
    vco: Vco,
    smoothing: Option<MovingAverage>,
    band_pass: Option<Fir>,
    reverse: bool,
    amplitude: f64,
    ramp_samples: f64,
    tail_samples: f64,
    fade_out_end: Option<f64>,
    config: TxConfig,
}

impl<I: Iterator<Item = TxCode>> Transmitter<I> {
    /// Creates a transmitter that consumes `codes` at the given sample rate.
    pub fn new(codes: I, sample_rate_hz: u32, config: TxConfig) -> Result<Self, RttyError> {
        let rate = f64::from(sample_rate_hz);
        config.tones.validate(rate)?;
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
        if !config.amplitude.is_finite() || config.amplitude <= 0.0 {
            return Err(RttyError::Dsp(grayline_dsp::DspError::InvalidLevel));
        }
        let vco = Vco::new(
            rate,
            config.tones.space_hz,
            config.tones.mark_hz - config.tones.space_hz,
        )?;
        let smoothing = config
            .smoothing_hz
            .map(|smoothing_hz| {
                MovingAverage::new(MovingAverageDesign {
                    sample_rate_hz: rate,
                    smoothing_hz,
                })
            })
            .transpose()?;
        let band_pass = config
            .band_pass
            .then(|| {
                Fir::from_design(FirDesign {
                    kind: FirKind::BandPass,
                    order: BAND_PASS_ORDER,
                    sample_rate_hz: rate,
                    lower_frequency_hz: (config.tones.low_hz() - BAND_PASS_MARGIN_HZ).max(10.0),
                    upper_frequency_hz: (config.tones.high_hz() + BAND_PASS_MARGIN_HZ).min(rate * 0.5 - 1.0),
                    attenuation_db: 20.0,
                    gain: 1.0,
                })
            })
            .transpose()?;
        let parity_bits = if config.framing.parity.transmitted_bit(0).is_some() {
            1.0
        } else {
            0.0
        };
        let character_bits =
            1.0 + f64::from(config.framing.bits.bits()) + parity_bits + 1.0 + config.framing.stop.extra_bits();
        let mut pending = VecDeque::new();
        if config.lead_in_seconds > 0.0 {
            pending.push_back((Keying::Mark, config.lead_in_seconds * rate));
        }
        Ok(Self {
            codes,
            stage: Stage::Stream,
            pending,
            diddle_enabled: true,
            keying: Keying::Mark,
            position: 0,
            boundary: 0.0,
            samples_per_bit,
            character_bits,
            vco,
            smoothing,
            band_pass,
            reverse: config.reverse,
            amplitude: config.amplitude,
            ramp_samples: config.ramp_seconds * rate,
            tail_samples: config.tail_seconds * rate,
            fade_out_end: None,
            config,
        })
    }

    /// Fills as much of `output` as the remaining code stream permits.
    ///
    /// The returned count is the initialized prefix length. Once the stream
    /// is exhausted, subsequent calls return zero, including for nonempty
    /// buffers.
    pub fn process(&mut self, output: &mut [f32]) -> Result<usize, RttyError> {
        let mut written = 0;
        'samples: while written < output.len() {
            while (self.position as f64) >= self.boundary {
                if !self.advance() {
                    break 'samples;
                }
            }
            output[written] = (self.synthesize()? * self.gate_gain()) as f32;
            written += 1;
            self.position += 1;
        }
        Ok(written)
    }

    fn advance(&mut self) -> bool {
        loop {
            if let Some((keying, duration)) = self.pending.pop_front() {
                self.keying = keying;
                self.boundary += duration;
                return true;
            }
            match self.stage {
                Stage::Stream => match self.codes.next() {
                    Some(code) => self.schedule(code),
                    None => {
                        self.stage = Stage::Tail;
                        if self.tail_samples > 0.0 {
                            self.pending.push_back((Keying::Mark, self.tail_samples));
                            self.fade_out_end = Some(self.boundary + self.tail_samples);
                        }
                    }
                },
                Stage::Tail | Stage::Done => {
                    self.stage = Stage::Done;
                    return false;
                }
            }
        }
    }

    fn schedule(&mut self, code: TxCode) {
        match code {
            TxCode::Character(code) => {
                self.schedule_character(code);
                self.schedule_gap();
            }
            TxCode::HoldMark => self
                .pending
                .push_back((Keying::Mark, CONTROL_HOLD_BITS * self.samples_per_bit)),
            TxCode::CarrierOff => self
                .pending
                .push_back((Keying::Muted, CONTROL_HOLD_BITS * self.samples_per_bit)),
            TxCode::DisableDiddle => self.diddle_enabled = false,
            TxCode::EnableDiddle => self.diddle_enabled = true,
        }
    }

    /// Frames one character: start, data b1-first, parity, then the stop
    /// element as one mark of one bit plus the configured extra.
    fn schedule_character(&mut self, code: u8) {
        let bit = self.samples_per_bit;
        self.pending.push_back((Keying::Space, bit));
        let bits = self.config.framing.bits.bits();
        let mut marks = 0;
        for index in (0..bits).rev() {
            let is_mark = (code >> index) & 1 == 1;
            marks += u32::from(is_mark);
            self.pending
                .push_back((if is_mark { Keying::Mark } else { Keying::Space }, bit));
        }
        if let Some(parity) = self.config.framing.parity.transmitted_bit(marks) {
            self.pending
                .push_back((if parity { Keying::Mark } else { Keying::Space }, bit));
        }
        self.pending
            .push_back((Keying::Mark, (1.0 + self.config.framing.stop.extra_bits()) * bit));
    }

    /// Fills the character gap with diddle characters where they fit, and
    /// mark idle for whatever is left.
    fn schedule_gap(&mut self) {
        let mut remaining = self.config.char_gap_bits;
        if remaining <= 0.0 {
            return;
        }
        let diddle = if self.diddle_enabled {
            self.config.diddle
        } else {
            Diddle::None
        };
        let filler = match diddle {
            Diddle::None => None,
            Diddle::Ltrs => Some(LTRS),
            Diddle::Blank => Some(0),
        };
        if let Some(code) = filler {
            while remaining >= self.character_bits {
                self.schedule_character(code);
                remaining -= self.character_bits;
            }
        }
        if remaining > 0.0 {
            self.pending.push_back((Keying::Mark, remaining * self.samples_per_bit));
        }
    }

    fn synthesize(&mut self) -> Result<f64, RttyError> {
        let control = match self.keying {
            Keying::Mark => 1.0,
            Keying::Space => 0.0,
            // The line conceptually idles at mark while the carrier is off,
            // so that is what the smoothing filter keeps settling toward.
            Keying::Muted => 1.0,
        };
        let control = if self.reverse { 1.0 - control } else { control };
        let control = match &mut self.smoothing {
            Some(filter) => filter.process_sample(control),
            None => control,
        };
        let mut sample = if self.keying == Keying::Muted {
            0.0
        } else {
            self.vco.process_sample(control)?
        };
        if let Some(filter) = &mut self.band_pass {
            sample = filter.process_sample(sample);
        }
        Ok(sample * self.amplitude)
    }

    /// The transmit on-gate: a quarter-sine fade at each end of the stream.
    ///
    /// This shapes only the beginning and the end of the transmission; a
    /// mark-space transition is the smoothing filter's job in the frequency
    /// domain, and ramping amplitude there would amplitude-modulate the FSK.
    fn gate_gain(&self) -> f64 {
        let position = self.position as f64;
        let mut gain = 1.0;
        if self.ramp_samples > 0.0 && position < self.ramp_samples {
            gain = libm::sin(FRAC_PI_2 * position / self.ramp_samples);
        }
        if let Some(end) = self.fade_out_end {
            let fade = self.ramp_samples.min(self.tail_samples);
            let remaining = end - position;
            if fade > 0.0 && remaining < fade {
                gain = gain.min(libm::sin(FRAC_PI_2 * remaining.max(0.0) / fade));
            }
        }
        gain
    }
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use super::*;
    use crate::params::{Parity, StopElement, ToneSet};
    use rstest::rstest;

    const RATE: u32 = 48_000;

    fn config() -> TxConfig {
        TxConfig {
            smoothing_hz: None,
            band_pass: false,
            ramp_seconds: 0.0,
            lead_in_seconds: 0.0,
            tail_seconds: 0.0,
            amplitude: 1.0,
            ..TxConfig::default()
        }
    }

    fn render(codes: &[TxCode], config: TxConfig, chunk: usize) -> Vec<f32> {
        let mut transmitter = Transmitter::new(codes.iter().copied(), RATE, config).unwrap();
        let mut result = Vec::new();
        loop {
            let mut block = vec![f32::NAN; chunk];
            let count = transmitter.process(&mut block).unwrap();
            if count == 0 {
                return result;
            }
            result.extend_from_slice(&block[..count]);
        }
    }

    /// Counts positive-going zero crossings over each expected bit window.
    fn crossings_per_bit(samples: &[f32], bits: usize) -> Vec<f64> {
        let per_bit = samples.len() / bits;
        (0..bits)
            .map(|bit| {
                let window = &samples[bit * per_bit..(bit + 1) * per_bit];
                let mut crossings = 0;
                for pair in window.windows(2) {
                    if pair[0] < 0.0 && pair[1] >= 0.0 {
                        crossings += 1;
                    }
                }
                crossings as f64 * f64::from(RATE) / per_bit as f64
            })
            .collect()
    }

    #[test]
    fn held_mark_and_space_sit_on_the_configured_tones() {
        let samples = render(&[TxCode::HoldMark], config(), 512);
        let expected = 3.0 * f64::from(RATE) / 45.45;
        assert!((samples.len() as f64 - expected).abs() <= 1.0);
        let tone = crossings_per_bit(&samples, 1)[0];
        assert!((tone - 2_125.0).abs() <= 20.0, "mark measured {tone}");

        let reversed = TxConfig {
            reverse: true,
            ..config()
        };
        let samples = render(&[TxCode::HoldMark], reversed, 512);
        let tone = crossings_per_bit(&samples, 1)[0];
        assert!((tone - 2_295.0).abs() <= 20.0, "reversed mark measured {tone}");
    }

    #[test]
    fn a_character_keys_start_data_and_stop_in_order() {
        // E is 10000: start space, mark, four spaces, then 1.5 bits of stop.
        let samples = render(&[TxCode::Character(0b10000)], config(), 512);
        let tones = crossings_per_bit(&samples[..(samples.len() * 6 / 15) * 2], 6);
        let expected = [2_295.0, 2_125.0, 2_295.0, 2_295.0, 2_295.0, 2_295.0];
        // One bit holds about 50 tone cycles, so the crossing count
        // quantizes the measurement to about 45 Hz; 60 Hz still separates
        // tones 170 Hz apart.
        for (index, (tone, expected)) in tones.iter().zip(expected).enumerate() {
            assert!((tone - expected).abs() <= 60.0, "bit {index} measured {tone}");
        }
    }

    #[test]
    fn the_stop_element_length_follows_the_configuration() {
        for (stop, bits) in [
            (StopElement::One, 7.0),
            (StopElement::OneAndAHalf, 7.5),
            (StopElement::Two, 8.0),
        ] {
            let mut with_stop = config();
            with_stop.framing.stop = stop;
            let samples = render(&[TxCode::Character(0)], with_stop, 512);
            let expected = bits * f64::from(RATE) / 45.45;
            assert!(
                (samples.len() as f64 - expected).abs() <= 1.0,
                "{stop:?}: {} vs {expected}",
                samples.len()
            );
        }
    }

    #[test]
    fn a_parity_bit_lengthens_the_character() {
        let mut with_parity = config();
        with_parity.framing.parity = Parity::Even;
        let plain = render(&[TxCode::Character(0)], config(), 512).len();
        let parity = render(&[TxCode::Character(0)], with_parity, 512).len();
        let bit = f64::from(RATE) / 45.45;
        assert!(((parity - plain) as f64 - bit).abs() <= 1.0);
    }

    #[test]
    fn muted_keying_is_exactly_zero_and_freezes_phase() {
        // 50 baud puts a whole number of samples in each bit, so the element
        // boundaries land exactly on sample indices.
        let mut exact = config();
        exact.framing.baud = crate::params::BaudRate::new(50.0).unwrap();
        let samples = render(&[TxCode::HoldMark, TxCode::CarrierOff, TxCode::HoldMark], exact, 512);
        let third = samples.len() / 3;
        assert!(samples[third..2 * third].iter().all(|&sample| sample == 0.0));

        let uninterrupted = render(&[TxCode::HoldMark, TxCode::HoldMark], exact, 512);
        assert_eq!(&samples[2 * third..], &uninterrupted[third..]);
    }

    #[test]
    fn the_gate_ramps_only_the_ends_of_the_stream() {
        let gated = TxConfig {
            ramp_seconds: 0.01,
            lead_in_seconds: 0.05,
            tail_seconds: 0.05,
            ..config()
        };
        let samples = render(&[TxCode::Character(0b10101)], gated, 512);
        assert_eq!(samples[0], 0.0);
        assert!(samples[samples.len() - 1].abs() < 1.0e-2);

        // Peaks inside the character body all reach full amplitude: the
        // mark-space transitions are not amplitude-shaped.
        let lead = (0.05 * f64::from(RATE)) as usize;
        let body = &samples[lead..samples.len() - lead];
        let cycle = RATE as usize / 2_000;
        let mut minimum_peak = f64::MAX;
        for window in body.chunks_exact(cycle * 2) {
            let peak = window
                .iter()
                .fold(0.0_f64, |peak, &sample| peak.max(sample.abs().into()));
            minimum_peak = minimum_peak.min(peak);
        }
        assert!(minimum_peak > 0.95, "a body window peaked at {minimum_peak}");
    }

    #[rstest]
    #[case(1)]
    #[case(73)]
    #[case(4_093)]
    fn output_is_independent_of_chunk_size(#[case] chunk: usize) {
        let codes = [
            TxCode::Character(LTRS),
            TxCode::Character(0b11000),
            TxCode::Character(0b00100),
        ];
        let full = TxConfig {
            smoothing_hz: Some(100.0),
            band_pass: true,
            ramp_seconds: 0.005,
            lead_in_seconds: 0.02,
            tail_seconds: 0.02,
            amplitude: 0.9,
            ..TxConfig::default()
        };
        assert_eq!(render(&codes, full, chunk), render(&codes, full, 1_024));
    }

    #[test]
    fn diddle_fills_the_character_gap_with_whole_characters() {
        let mut gapped = config();
        gapped.char_gap_bits = 16.0;
        let idle = render(&[TxCode::Character(0)], gapped, 512);

        gapped.diddle = Diddle::Ltrs;
        let diddled = render(&[TxCode::Character(0)], gapped, 512);
        assert_eq!(idle.len(), diddled.len());

        // Two 7.5-bit LTRS characters fit in 16 bits, so the gap contains
        // space keying where the idle version holds mark.
        let bit = f64::from(RATE) / 45.45;
        let gap_start = (7.5 * bit) as usize;
        let gap = &diddled[gap_start + cycle_guard()..];
        let idle_gap = &idle[gap_start + cycle_guard()..];
        assert!(crossings_per_bit(gap, 1)[0] > crossings_per_bit(idle_gap, 1)[0] + 10.0);
    }

    fn cycle_guard() -> usize {
        RATE as usize / 1_000
    }

    #[test]
    fn disable_diddle_suppresses_the_filler() {
        let mut gapped = config();
        gapped.char_gap_bits = 16.0;
        gapped.diddle = Diddle::Ltrs;
        let diddled = render(&[TxCode::Character(0)], gapped, 512);
        let disabled = render(&[TxCode::DisableDiddle, TxCode::Character(0)], gapped, 512);
        assert_eq!(diddled.len(), disabled.len());
        assert_ne!(diddled, disabled);
    }

    #[test]
    fn an_exhausted_stream_returns_zero_forever() {
        let mut transmitter = Transmitter::new(core::iter::empty(), RATE, config()).unwrap();
        assert_eq!(transmitter.process(&mut [0.0; 16]).unwrap(), 0);
        assert_eq!(transmitter.process(&mut [0.0; 16]).unwrap(), 0);
    }

    #[test]
    fn construction_rejects_bad_configurations() {
        let too_high = TxConfig {
            tones: ToneSet {
                mark_hz: 2_125.0,
                space_hz: 30_000.0,
            },
            ..config()
        };
        assert_eq!(
            Transmitter::new(core::iter::empty(), RATE, too_high).err().unwrap(),
            RttyError::InvalidToneSet
        );

        let negative_gap = TxConfig {
            char_gap_bits: -1.0,
            ..config()
        };
        assert_eq!(
            Transmitter::new(core::iter::empty(), RATE, negative_gap).err().unwrap(),
            RttyError::InvalidTransmitTiming
        );

        let mut too_fast = config();
        too_fast.framing.baud = crate::params::BaudRate::new(20_000.0).unwrap();
        assert_eq!(
            Transmitter::new(core::iter::empty(), RATE, too_fast).err().unwrap(),
            RttyError::TooFewSamplesPerBit
        );
    }
}
