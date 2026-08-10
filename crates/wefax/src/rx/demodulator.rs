use alloc::vec::Vec;

use crate::{
    error::WefaxError,
    format::WefaxBand,
    rx::{
        apt::{AptDetector, AptEvent, AptStrengths},
        frontend::FrontEnd,
        input::DemodulatedBlock,
    },
};

/// Lowest capture rate the front end demodulates.
pub const MINIMUM_SAMPLE_RATE_HZ: u32 = 6_000;

/// One packet's worth of demodulated frequencies, with its absolute position.
///
/// There is one frequency per input sample: the receive path neither resamples
/// nor decimates, so a caller can convert between a sample position and a
/// position in this array by subtracting [`first_sample`].
///
/// [`first_sample`]: Self::first_sample
#[derive(Clone, Debug, PartialEq)]
pub struct DemodulatedChunk {
    first_sample: u64,
    frequency_hz: Vec<f32>,
    apt_events: Vec<AptEvent>,
}

impl DemodulatedChunk {
    /// Returns the absolute position of the first demodulated sample.
    pub const fn first_sample(&self) -> u64 {
        self.first_sample
    }

    /// Returns the demodulated frequencies in hertz.
    pub fn frequency_hz(&self) -> &[f32] {
        &self.frequency_hz
    }

    /// Returns the framing tones decided within this packet, in order.
    pub fn apt_events(&self) -> &[AptEvent] {
        &self.apt_events
    }

    /// Returns how many samples the chunk carries.
    pub fn len(&self) -> usize {
        self.frequency_hz.len()
    }

    /// Returns whether the chunk carries no samples.
    pub fn is_empty(&self) -> bool {
        self.frequency_hz.is_empty()
    }

    /// Borrows the whole chunk as a block a decoder can take.
    pub fn block(&self) -> DemodulatedBlock<'_> {
        self.block_from(0)
    }

    /// Borrows the chunk from `offset` onwards.
    pub fn block_from(&self, offset: usize) -> DemodulatedBlock<'_> {
        let offset = offset.min(self.frequency_hz.len());
        DemodulatedBlock::already_checked(self.first_sample + offset as u64, &self.frequency_hz[offset..])
    }
}

/// A stateful WEFAX audio demodulator.
///
/// Accepts contiguous packets of normalized mono PCM and reports the
/// instantaneous frequency of each sample. Packets may be any length; the
/// result does not depend on how the stream was divided.
#[derive(Clone, Debug)]
pub struct Demodulator {
    front_end: FrontEnd,
    apt: AptDetector,
    band: WefaxBand,
    sample_rate_hz: u32,
    next_sample: u64,
}

impl Demodulator {
    /// Creates a demodulator for a capture rate, on the standard shift.
    pub fn new(sample_rate_hz: u32) -> Result<Self, WefaxError> {
        Self::with_band(sample_rate_hz, WefaxBand::WIDE)
    }

    /// Creates a demodulator for a capture rate and a shift.
    pub fn with_band(sample_rate_hz: u32, band: WefaxBand) -> Result<Self, WefaxError> {
        if sample_rate_hz < MINIMUM_SAMPLE_RATE_HZ {
            return Err(WefaxError::SampleRateTooLow(sample_rate_hz));
        }
        Ok(Self {
            front_end: FrontEnd::new(f64::from(sample_rate_hz))?,
            apt: AptDetector::new(f64::from(sample_rate_hz))?,
            band,
            sample_rate_hz,
            next_sample: 0,
        })
    }

    /// Returns the capture rate the demodulator was created for.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns the shift the gray scale is read against.
    pub const fn band(&self) -> WefaxBand {
        self.band
    }

    /// Changes the shift the gray scale is read against.
    pub const fn set_band(&mut self, band: WefaxBand) {
        self.band = band;
    }

    /// Returns how strongly each framing tone is currently present.
    pub fn apt_strengths(&self) -> AptStrengths {
        self.apt.strengths()
    }

    /// Returns the absolute position the next packet begins at.
    pub const fn next_sample(&self) -> u64 {
        self.next_sample
    }

    /// Returns the smoothed root-mean-square level of the received band.
    pub fn signal_level(&self) -> f32 {
        self.front_end.level() as f32
    }

    /// Demodulates one packet, continuing from wherever the last one ended.
    pub fn process(&mut self, samples: &[f32]) -> Result<DemodulatedChunk, WefaxError> {
        let count = samples.len() as u64;
        if self.next_sample.checked_add(count).is_none() {
            return Err(WefaxError::SamplePositionOverflow);
        }
        let first_sample = self.next_sample;
        let mut frequency_hz = Vec::with_capacity(samples.len());
        let mut apt_events = Vec::new();
        for (index, sample) in samples.iter().enumerate() {
            if !sample.is_finite() {
                return Err(WefaxError::NonFiniteSample { index });
            }
            let frequency = self.front_end.process(f64::from(*sample));
            let position = first_sample + index as u64;
            if let Some(event) = self.apt.process(self.band.normalized(frequency), position) {
                apt_events.push(event);
            }
            frequency_hz.push(frequency as f32);
        }
        self.next_sample += count;
        Ok(DemodulatedChunk {
            first_sample,
            frequency_hz,
            apt_events,
        })
    }

    /// Clears every stage and restarts the sample timeline.
    pub fn reset(&mut self) {
        self.front_end.reset();
        self.apt.reset();
        self.next_sample = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{APT_STOP_HZ, Format, Ioc, WefaxBand};
    use core::f64::consts::TAU;
    use rstest::rstest;

    fn tone(rate: u32, frequency_hz: f64, seconds: f64) -> Vec<f32> {
        let count = (f64::from(rate) * seconds) as usize;
        let mut phase = 0.0_f64;
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            samples.push(libm::sin(phase) as f32);
            phase = libm::fmod(phase + TAU * frequency_hz / f64::from(rate), TAU);
        }
        samples
    }

    /// Mean and standard deviation of the second half of a demodulated tone.
    fn settled(rate: u32, frequency_hz: f64) -> (f64, f64) {
        let mut demodulator = Demodulator::new(rate).unwrap();
        let chunk = demodulator.process(&tone(rate, frequency_hz, 0.5)).unwrap();
        let tail = &chunk.frequency_hz()[chunk.len() / 2..];
        let mean = tail.iter().map(|value| f64::from(*value)).sum::<f64>() / tail.len() as f64;
        let variance = tail
            .iter()
            .map(|value| {
                let error = f64::from(*value) - mean;
                error * error
            })
            .sum::<f64>()
            / tail.len() as f64;
        (mean, libm::sqrt(variance))
    }

    #[rstest]
    fn the_picture_band_demodulates_to_its_own_frequencies(
        #[values(8_000, 11_025, 48_000)] rate: u32,
        #[values(1_500.0, 1_900.0, 2_300.0)] frequency_hz: f64,
    ) {
        let (mean, _) = settled(rate, frequency_hz);
        assert!(
            (mean - frequency_hz).abs() < 3.0,
            "{frequency_hz} Hz at {rate} Hz produced {mean} Hz"
        );
    }

    #[rstest]
    fn a_steady_tone_leaves_little_residual_ripple(
        #[values(8_000, 11_025, 48_000)] rate: u32,
        #[values(1_500.0, 1_900.0, 2_300.0)] frequency_hz: f64,
    ) {
        // A gray level is 800 Hz / 255 = 3.1 Hz on the standard shift, and the
        // per-sample ripple sits above that at the black end: the ripple is at
        // twice the carrier, so it is closest to the output filter where the
        // carrier is lowest. This bound pins the filter choice; what a picture
        // actually reads is the averaged figure below.
        let (_, deviation) = settled(rate, frequency_hz);
        assert!(
            deviation < 5.0,
            "{frequency_hz} Hz at {rate} Hz rippled by {deviation} Hz"
        );
    }

    #[rstest]
    fn averaging_one_pixel_puts_the_ripple_under_a_gray_level(
        #[values(8_000, 11_025, 48_000)] rate: u32,
        #[values(1_500.0, 1_900.0, 2_300.0)] frequency_hz: f64,
    ) {
        let window = Format::MARINE.samples_per_pixel(rate) as usize;
        let mut demodulator = Demodulator::new(rate).unwrap();
        let chunk = demodulator.process(&tone(rate, frequency_hz, 0.5)).unwrap();
        let pixels: Vec<f64> = chunk.frequency_hz()[chunk.len() / 2..]
            .chunks_exact(window)
            .map(|samples| samples.iter().map(|value| f64::from(*value)).sum::<f64>() / window as f64)
            .collect();
        let mean = pixels.iter().sum::<f64>() / pixels.len() as f64;
        let variance = pixels.iter().map(|value| (value - mean) * (value - mean)).sum::<f64>() / pixels.len() as f64;
        let deviation = libm::sqrt(variance);
        let gray_level = WefaxBand::WIDE.deviation_hz * 2.0 / 255.0;
        assert!(
            deviation < gray_level,
            "{frequency_hz} Hz at {rate} Hz rippled by {deviation} Hz across {window}-sample pixels"
        );
    }

    #[test]
    fn packet_division_does_not_change_the_result() {
        let rate = 11_025;
        let samples = tone(rate, 2_100.0, 0.2);
        let whole = Demodulator::new(rate).unwrap().process(&samples).unwrap();

        let mut split = Demodulator::new(rate).unwrap();
        let mut collected = Vec::new();
        for packet in samples.chunks(73) {
            collected.extend_from_slice(split.process(packet).unwrap().frequency_hz());
        }
        assert_eq!(collected, whole.frequency_hz());
        assert_eq!(split.next_sample(), samples.len() as u64);
    }

    #[test]
    fn positions_run_from_zero_across_packets() {
        let mut demodulator = Demodulator::new(11_025).unwrap();
        assert_eq!(demodulator.process(&[0.0; 10]).unwrap().first_sample(), 0);
        assert_eq!(demodulator.process(&[0.0; 10]).unwrap().first_sample(), 10);
        assert_eq!(demodulator.next_sample(), 20);
        demodulator.reset();
        assert_eq!(demodulator.next_sample(), 0);
    }

    #[test]
    fn silence_settles_inside_the_band() {
        // The band-pass rings down for its own order after the tone stops, so
        // the discriminator reads that decaying tail before it reads nothing
        // at all. The saturation to the band sits ahead of the output filter,
        // so that filter's transient can carry a reading briefly outside it —
        // which costs nothing, because a decoder clamps a reading to the gray
        // scale anyway. What has to hold is that silence settles rather than
        // leaving the reading somewhere it can never come back from.
        let rate = 11_025;
        let mut demodulator = Demodulator::new(rate).unwrap();
        demodulator.process(&tone(rate, 2_300.0, 0.3)).unwrap();
        let quiet = demodulator.process(&[0.0; 4_096]).unwrap();
        let readings = quiet.frequency_hz();
        assert!(readings.iter().all(|value| value.is_finite()));
        for (offset, value) in readings.iter().enumerate().skip(readings.len() / 2) {
            assert!(
                (1_000.0..=2_800.0).contains(&f64::from(*value)),
                "settled silence read {value} Hz at offset {offset}"
            );
        }
    }

    #[test]
    fn the_level_meter_follows_the_input_amplitude() {
        let rate = 11_025;
        let mut demodulator = Demodulator::new(rate).unwrap();
        demodulator.process(&tone(rate, 1_900.0, 1.0)).unwrap();
        let loud = demodulator.signal_level();

        let mut quiet_source = Demodulator::new(rate).unwrap();
        let quiet_tone: Vec<f32> = tone(rate, 1_900.0, 1.0).iter().map(|value| value * 0.1).collect();
        quiet_source.process(&quiet_tone).unwrap();
        let quiet = quiet_source.signal_level();

        assert!(loud > quiet * 5.0, "loud={loud} quiet={quiet}");
    }

    /// The audio of a picture keyed black and white at `alternation_hz`.
    fn keyed_carrier(rate: u32, band: WefaxBand, alternation_hz: f64, seconds: f64) -> Vec<f32> {
        let count = (f64::from(rate) * seconds) as usize;
        let mut carrier = 0.0_f64;
        let mut keying = 0.0_f64;
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            let frequency = if keying < 0.5 { band.black_hz() } else { band.white_hz() };
            samples.push(libm::sin(carrier) as f32);
            carrier = libm::fmod(carrier + TAU * frequency / f64::from(rate), TAU);
            keying += alternation_hz / f64::from(rate);
            if keying >= 1.0 {
                keying -= 1.0;
            }
        }
        samples
    }

    #[test]
    fn a_keyed_carrier_swings_between_the_band_edges() {
        let rate = 11_025;
        let mut demodulator = Demodulator::new(rate).unwrap();
        let chunk = demodulator
            .process(&keyed_carrier(rate, WefaxBand::WIDE, 300.0, 0.5))
            .unwrap();
        let tail = &chunk.frequency_hz()[chunk.len() / 2..];
        let minimum = tail.iter().fold(f64::MAX, |low, value| low.min(f64::from(*value)));
        let maximum = tail.iter().fold(f64::MIN, |high, value| high.max(f64::from(*value)));
        assert!(minimum < 1_650.0, "minimum was {minimum} Hz");
        assert!(maximum > 2_150.0, "maximum was {maximum} Hz");
    }

    #[rstest]
    #[case(300.0, Ioc::Ioc576)]
    #[case(675.0, Ioc::Ioc288)]
    fn a_start_tone_reaches_the_demodulator_through_the_audio(#[case] alternation_hz: f64, #[case] expected: Ioc) {
        let rate = 11_025;
        let band = WefaxBand::WIDE;
        let mut demodulator = Demodulator::new(rate).unwrap();
        let chunk = demodulator
            .process(&keyed_carrier(rate, band, alternation_hz, 5.0))
            .unwrap();
        let accepted = chunk.apt_events().first().copied();
        assert!(
            matches!(accepted, Some(AptEvent::StartAccepted { ioc, .. }) if ioc == expected),
            "{alternation_hz} Hz produced {accepted:?}"
        );
    }

    #[test]
    fn a_stop_tone_reaches_the_demodulator_through_the_audio() {
        let rate = 11_025;
        let mut demodulator = Demodulator::new(rate).unwrap();
        let chunk = demodulator
            .process(&keyed_carrier(rate, WefaxBand::WIDE, APT_STOP_HZ, 5.0))
            .unwrap();
        assert!(matches!(
            chunk.apt_events().first(),
            Some(AptEvent::StopAccepted { .. })
        ));
    }

    #[test]
    fn a_steady_picture_tone_frames_nothing() {
        let rate = 11_025;
        let mut demodulator = Demodulator::new(rate).unwrap();
        let chunk = demodulator.process(&tone(rate, 1_900.0, 10.0)).unwrap();
        assert!(chunk.apt_events().is_empty(), "{:?}", chunk.apt_events());
    }

    #[test]
    fn a_rate_below_the_minimum_is_rejected() {
        assert_eq!(
            Demodulator::new(4_000).unwrap_err(),
            WefaxError::SampleRateTooLow(4_000)
        );
    }

    #[test]
    fn a_non_finite_sample_is_rejected_with_its_offset() {
        let mut demodulator = Demodulator::new(11_025).unwrap();
        let samples = [0.0, 0.0, f32::NAN, 0.0];
        assert_eq!(
            demodulator.process(&samples).unwrap_err(),
            WefaxError::NonFiniteSample { index: 2 }
        );
    }
}
