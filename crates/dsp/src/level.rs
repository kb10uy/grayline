//! Level tracking and normalization.

use crate::DspError;

/// Parameters for a decaying peak-follower normalizer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakNormalizerDesign {
    /// Sampling frequency in hertz.
    pub sample_rate_hz: f64,
    /// Time constant the tracked peak decays over, in seconds.
    pub decay_seconds: f64,
    /// Smallest peak the normalizer will divide by, and the peak it starts from.
    pub floor: f64,
}

/// A soft limiter that divides a signal by its own decaying peak.
///
/// The tracked peak is raised instantly by any louder sample and decays
/// exponentially otherwise, so the output settles near full scale regardless
/// of the input level while never exceeding `[-1, 1]`. The floor keeps
/// silence from being amplified into noise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakNormalizer {
    peak: f64,
    decay: f64,
    floor: f64,
}

impl PeakNormalizer {
    /// Designs and creates a normalizer.
    pub fn new(design: PeakNormalizerDesign) -> Result<Self, DspError> {
        if !design.sample_rate_hz.is_finite() || design.sample_rate_hz <= 0.0 {
            return Err(DspError::InvalidSampleRate);
        }
        if !design.decay_seconds.is_finite() || design.decay_seconds <= 0.0 {
            return Err(DspError::InvalidDuration);
        }
        if !design.floor.is_finite() || design.floor <= 0.0 {
            return Err(DspError::InvalidLevel);
        }
        Ok(Self {
            peak: design.floor,
            decay: libm::exp(-1.0 / (design.sample_rate_hz * design.decay_seconds)),
            floor: design.floor,
        })
    }

    /// Normalizes one sample against the tracked peak.
    pub fn process_sample(&mut self, sample: f64) -> f64 {
        self.peak = (self.peak * self.decay).max(sample.abs());
        (sample / self.peak.max(self.floor)).clamp(-1.0, 1.0)
    }

    /// Replaces a block with its normalized samples.
    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }

    /// Returns the currently tracked peak.
    pub fn peak(&self) -> f64 {
        self.peak
    }

    /// Restores the tracked peak to the floor.
    pub fn reset(&mut self) {
        self.peak = self.floor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const SAMPLE_RATE: f64 = 11_025.0;

    fn normalizer(decay_seconds: f64, floor: f64) -> PeakNormalizer {
        PeakNormalizer::new(PeakNormalizerDesign {
            sample_rate_hz: SAMPLE_RATE,
            decay_seconds,
            floor,
        })
        .unwrap()
    }

    #[test]
    fn the_peak_decays_by_the_designed_constant() {
        let mut normalizer = normalizer(0.1, 1.0e-6);
        normalizer.process_sample(1.0);
        normalizer.process_sample(0.0);
        assert_eq!(normalizer.peak(), libm::exp(-1.0 / (SAMPLE_RATE * 0.1)));
    }

    #[test]
    fn output_stays_within_unit_range() {
        let mut normalizer = normalizer(0.1, 1.0e-6);
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..10_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let sample = ((state >> 11) as f64 / (1_u64 << 53) as f64 - 0.5) * 2.0e6;
            let output = normalizer.process_sample(sample);
            assert!((-1.0..=1.0).contains(&output));
        }
    }

    #[test]
    fn the_floor_keeps_silence_and_noise_from_self_amplifying() {
        let mut normalizer = normalizer(0.1, 1.0e-6);
        for _ in 0..10_000 {
            assert_eq!(normalizer.process_sample(0.0), 0.0);
        }
        let output = normalizer.process_sample(1.0e-9);
        assert!(output.abs() <= 1.0e-3, "amplified to {output}");
    }

    #[test]
    fn the_sstv_front_end_formula_is_reproduced_exactly() {
        // The inline peak follower this type replaced in the SSTV receive
        // front end; the lift must not change a single output value.
        let mut level_peak = 1.0e-6_f64;
        let level_decay = libm::exp(-1.0 / (SAMPLE_RATE * 0.1));
        let mut normalizer = normalizer(0.1, 1.0e-6);
        let mut state = 0x853c_49e6_748f_ea9b_u64;
        for _ in 0..50_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let sample = ((state >> 11) as f64 / (1_u64 << 53) as f64 - 0.5) * 2.0;
            level_peak = (level_peak * level_decay).max(sample.abs());
            let reference = (sample / level_peak.max(1.0e-6)).clamp(-1.0, 1.0);
            assert_eq!(normalizer.process_sample(sample), reference);
        }
    }

    #[test]
    fn reset_restores_the_floor() {
        let mut normalizer = normalizer(0.1, 1.0e-6);
        normalizer.process_sample(1.0);
        normalizer.reset();
        assert_eq!(normalizer.peak(), 1.0e-6);
    }

    #[rstest]
    #[case(0.0, 0.1, 1.0e-6, DspError::InvalidSampleRate)]
    #[case(SAMPLE_RATE, 0.0, 1.0e-6, DspError::InvalidDuration)]
    #[case(SAMPLE_RATE, f64::NAN, 1.0e-6, DspError::InvalidDuration)]
    #[case(SAMPLE_RATE, 0.1, 0.0, DspError::InvalidLevel)]
    #[case(SAMPLE_RATE, 0.1, f64::INFINITY, DspError::InvalidLevel)]
    fn invalid_designs_are_rejected(
        #[case] sample_rate_hz: f64,
        #[case] decay_seconds: f64,
        #[case] floor: f64,
        #[case] expected: DspError,
    ) {
        let result = PeakNormalizer::new(PeakNormalizerDesign {
            sample_rate_hz,
            decay_seconds,
            floor,
        });
        assert_eq!(result.unwrap_err(), expected);
    }
}
