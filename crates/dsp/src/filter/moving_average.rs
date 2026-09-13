use alloc::{vec, vec::Vec};

use crate::DspError;

/// Parameters for a boxcar moving-average filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovingAverageDesign {
    /// Sampling frequency in hertz.
    pub sample_rate_hz: f64,
    /// Reciprocal of the averaging period, in hertz.
    ///
    /// The window holds `round(sample_rate_hz / smoothing_hz)` samples,
    /// matching how MMTTY's `CSmooz` sizes its ring from a frequency.
    pub smoothing_hz: f64,
}

/// A boxcar moving average over a fixed window of samples.
///
/// The port of MMTTY's `CSmooz`. The original re-sums its whole ring on every
/// sample, an O(window) cost per sample; this implementation keeps a running
/// sum instead and re-sums once per lap around the ring, so the cost is
/// amortized O(1) and the floating-point drift a running sum accumulates is
/// discarded every lap.
///
/// Until the window has filled, the average divides by the number of samples
/// seen rather than the window length, as the original does. Dividing by the
/// window length would under-report the first window of a reception and a
/// downstream comparator would see a false space.
#[derive(Clone, Debug, PartialEq)]
pub struct MovingAverage {
    history: Vec<f64>,
    index: usize,
    sum: f64,
    filled: usize,
}

impl MovingAverage {
    /// Designs and creates a filter whose window covers one smoothing period.
    pub fn new(design: MovingAverageDesign) -> Result<Self, DspError> {
        if !design.sample_rate_hz.is_finite() || design.sample_rate_hz <= 0.0 {
            return Err(DspError::InvalidSampleRate);
        }
        if !design.smoothing_hz.is_finite() || design.smoothing_hz <= 0.0 {
            return Err(DspError::InvalidFrequency);
        }
        let window = libm::round(design.sample_rate_hz / design.smoothing_hz) as usize;
        if window == 0 {
            return Err(DspError::InvalidFrequency);
        }
        Self::with_window(window)
    }

    /// Creates a filter averaging over exactly `window_samples` samples.
    pub fn with_window(window_samples: usize) -> Result<Self, DspError> {
        if window_samples == 0 {
            return Err(DspError::InvalidOrder);
        }
        Ok(Self {
            history: vec![0.0; window_samples],
            index: 0,
            sum: 0.0,
            filled: 0,
        })
    }

    /// Returns the window length in samples.
    pub fn window_samples(&self) -> usize {
        self.history.len()
    }

    /// Reports the average of the most recent window for one sample.
    pub fn process_sample(&mut self, sample: f64) -> f64 {
        self.sum += sample - self.history[self.index];
        self.history[self.index] = sample;
        self.index += 1;
        if self.filled < self.history.len() {
            self.filled += 1;
        }
        if self.index >= self.history.len() {
            self.index = 0;
            // The original re-sums every sample at O(window); doing it once per
            // lap keeps the amortized cost O(1) while still discarding the
            // drift the running sum accumulates.
            self.sum = self.history.iter().sum();
        }
        self.sum / self.filled as f64
    }

    /// Replaces a block with its per-sample averages.
    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }

    /// Clears the history without changing the window length.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.index = 0;
        self.sum = 0.0;
        self.filled = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(5_512.5, 70.0, 79)]
    #[case(11_025.0, 70.0, 158)]
    #[case(48_000.0, 100.0, 480)]
    fn window_length_rounds_the_period(
        #[case] sample_rate_hz: f64,
        #[case] smoothing_hz: f64,
        #[case] expected: usize,
    ) {
        let filter = MovingAverage::new(MovingAverageDesign {
            sample_rate_hz,
            smoothing_hz,
        })
        .unwrap();
        assert_eq!(filter.window_samples(), expected);
    }

    #[test]
    fn dc_gain_is_exactly_one() {
        let mut filter = MovingAverage::with_window(79).unwrap();
        let mut output = 0.0;
        for _ in 0..1_000 {
            output = filter.process_sample(1.0);
        }
        assert_eq!(output, 1.0);
    }

    #[test]
    fn a_step_reaches_its_target_exactly_at_the_window_length() {
        let window = 64;
        let mut filter = MovingAverage::with_window(window).unwrap();
        for _ in 0..window {
            filter.process_sample(0.0);
        }
        for step in 1..window {
            assert!(filter.process_sample(1.0) < 1.0, "early at step {step}");
        }
        assert_eq!(filter.process_sample(1.0), 1.0);
    }

    #[test]
    fn the_divisor_grows_with_the_history_before_the_window_fills() {
        let mut filter = MovingAverage::with_window(8).unwrap();
        assert_eq!(filter.process_sample(1.0), 1.0);
        assert_eq!(filter.process_sample(0.0), 0.5);
        assert_eq!(filter.process_sample(0.5), 0.5);
    }

    #[test]
    fn lapped_resummation_tracks_a_per_sample_reference() {
        let window = 97;
        let mut filter = MovingAverage::with_window(window).unwrap();
        let mut history = vec![0.0; window];
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        for index in 0..1_000_000_usize {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let sample = (state >> 11) as f64 / (1_u64 << 53) as f64 - 0.5;
            history[index % window] = sample;
            let seen = (index + 1).min(window);
            let reference = history[..seen].iter().sum::<f64>() / seen as f64;
            let output = filter.process_sample(sample);
            assert!(
                (output - reference).abs() <= 1.0e-12,
                "diverged at {index}: {output} vs {reference}"
            );
        }
    }

    #[rstest]
    #[case(0.0, 70.0, DspError::InvalidSampleRate)]
    #[case(f64::NAN, 70.0, DspError::InvalidSampleRate)]
    #[case(11_025.0, 0.0, DspError::InvalidFrequency)]
    #[case(11_025.0, -70.0, DspError::InvalidFrequency)]
    #[case(11_025.0, f64::INFINITY, DspError::InvalidFrequency)]
    #[case(11_025.0, 1.0e9, DspError::InvalidFrequency)]
    fn invalid_designs_are_rejected(
        #[case] sample_rate_hz: f64,
        #[case] smoothing_hz: f64,
        #[case] expected: DspError,
    ) {
        let result = MovingAverage::new(MovingAverageDesign {
            sample_rate_hz,
            smoothing_hz,
        });
        assert_eq!(result.unwrap_err(), expected);
    }

    #[test]
    fn a_zero_length_window_is_rejected() {
        assert_eq!(MovingAverage::with_window(0).unwrap_err(), DspError::InvalidOrder);
    }

    #[test]
    fn reset_restores_the_initial_state() {
        let mut filter = MovingAverage::with_window(4).unwrap();
        filter.process_sample(1.0);
        filter.process_sample(-1.0);
        filter.reset();
        assert_eq!(filter.process_sample(0.5), 0.5);
    }
}
