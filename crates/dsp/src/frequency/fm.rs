use core::f64::consts::TAU;

use crate::{
    DspError,
    filter::{Iir, IirLowPassDesign, IirResponse},
    transform::{AnalyticSample, HilbertTransformer},
};

/// Largest phase difference the discriminator measures over, in samples.
const MAXIMUM_PHASE_LAG: usize = 4;
/// Margin between the Hilbert passband and direct current or Nyquist, in hertz.
const HILBERT_MARGIN_HZ: f64 = 100.0;
/// Analytic magnitude below which a reading is discarded as silence.
const MAGNITUDE_FLOOR: f64 = 1.0e-8;
/// Fraction of the sampling frequency the output filter may be placed at.
const OUTPUT_CUTOFF_LIMIT: f64 = 0.45;

/// Parameters for a Hilbert phase-difference frequency discriminator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HilbertDiscriminatorDesign {
    /// Sampling frequency in hertz.
    pub sample_rate_hz: f64,
    /// Lowest frequency the output may report, in hertz.
    pub minimum_hz: f64,
    /// Highest frequency the output may report, in hertz.
    pub maximum_hz: f64,
    /// Cutoff of the third-order Butterworth output filter, in hertz.
    ///
    /// Bounded to 45 percent of the sampling frequency, so a caller may state
    /// the cutoff its signal wants without also checking the rate it runs at.
    pub output_cutoff_hz: f64,
    /// Frequency reported before the first usable measurement.
    pub initial_hz: f64,
}

/// A Hilbert phase-difference frequency discriminator.
///
/// The input is converted to an analytic signal, and the frequency is the
/// phase advance across a lag chosen from the sampling frequency: one sample
/// below 16 kHz, two below 40 kHz, and four above. A wider lag resolves a
/// smaller phase step, which is what keeps the reading steady once a sample is
/// a small fraction of a cycle.
///
/// [`Pll`](super::Pll) is the other discriminator here; the difference is the
/// method rather than the purpose.
#[derive(Clone, Debug)]
pub struct HilbertDiscriminator {
    transformer: HilbertTransformer,
    minimum_hz: f64,
    maximum_hz: f64,
    initial_hz: f64,
    analytic_history: [AnalyticSample; MAXIMUM_PHASE_LAG],
    history_len: usize,
    next_index: usize,
    phase_lag: usize,
    frequency_scale: f64,
    output_filter: Iir,
    held_frequency: f64,
}

impl HilbertDiscriminator {
    /// Designs and creates a discriminator.
    pub fn new(design: HilbertDiscriminatorDesign) -> Result<Self, DspError> {
        validate(&design)?;
        let sample_rate_hz = design.sample_rate_hz;
        let (order, phase_lag) = if sample_rate_hz < 16_000.0 {
            (12, 1)
        } else if sample_rate_hz < 40_000.0 {
            (24, 2)
        } else {
            (48, 4)
        };
        let upper_frequency_hz = sample_rate_hz * 0.5 - HILBERT_MARGIN_HZ;
        Ok(Self {
            transformer: HilbertTransformer::new(sample_rate_hz, order, HILBERT_MARGIN_HZ, upper_frequency_hz)?,
            minimum_hz: design.minimum_hz,
            maximum_hz: design.maximum_hz,
            initial_hz: design.initial_hz,
            analytic_history: [AnalyticSample {
                in_phase: 0.0,
                quadrature: 0.0,
            }; MAXIMUM_PHASE_LAG],
            history_len: 0,
            next_index: 0,
            phase_lag,
            frequency_scale: sample_rate_hz / (TAU * phase_lag as f64),
            output_filter: Iir::from_low_pass(IirLowPassDesign {
                order: 3,
                sample_rate_hz,
                cutoff_hz: design.output_cutoff_hz.min(sample_rate_hz * OUTPUT_CUTOFF_LIMIT),
                response: IirResponse::Butterworth,
            })?,
            held_frequency: design.initial_hz,
        })
    }

    /// Reports the estimated input frequency in hertz for one sample.
    ///
    /// A sample whose analytic magnitude is indistinguishable from silence
    /// leaves the held frequency where it was, so a gap between tones reads as
    /// the last tone rather than as an excursion.
    pub fn process_sample(&mut self, sample: f64) -> f64 {
        let analytic = self.transformer.process_sample(sample);
        let previous = (self.history_len == self.phase_lag).then_some(self.analytic_history[self.next_index]);
        self.analytic_history[self.next_index] = analytic;
        self.next_index += 1;
        if self.next_index == self.phase_lag {
            self.next_index = 0;
        }
        self.history_len = (self.history_len + 1).min(self.phase_lag);
        let magnitude_squared = analytic.in_phase * analytic.in_phase + analytic.quadrature * analytic.quadrature;
        if let Some(previous) = previous
            && magnitude_squared > MAGNITUDE_FLOOR * MAGNITUDE_FLOOR
        {
            // The conjugate product against the lagged sample carries the
            // phase advance directly, so the difference arrives already
            // wrapped into -PI..PI without either phase being measured.
            let dot = analytic.in_phase * previous.in_phase + analytic.quadrature * previous.quadrature;
            let cross = analytic.quadrature * previous.in_phase - analytic.in_phase * previous.quadrature;
            let delta = libm::atan2(cross, dot);
            self.held_frequency = (delta.abs() * self.frequency_scale).clamp(self.minimum_hz, self.maximum_hz);
        }
        self.output_filter.process_sample(self.held_frequency)
    }

    /// Replaces a block with its per-sample frequency estimates.
    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }

    /// Returns the phase lag the sampling frequency selected, in samples.
    pub const fn phase_lag(&self) -> usize {
        self.phase_lag
    }

    /// Clears both signal paths and restores the initial frequency.
    pub fn reset(&mut self) {
        self.transformer.reset();
        self.output_filter.reset();
        self.analytic_history = [AnalyticSample {
            in_phase: 0.0,
            quadrature: 0.0,
        }; MAXIMUM_PHASE_LAG];
        self.history_len = 0;
        self.next_index = 0;
        self.held_frequency = self.initial_hz;
    }
}

fn validate(design: &HilbertDiscriminatorDesign) -> Result<(), DspError> {
    if !design.sample_rate_hz.is_finite() || design.sample_rate_hz <= 0.0 {
        return Err(DspError::InvalidSampleRate);
    }
    let finite = design.minimum_hz.is_finite()
        && design.maximum_hz.is_finite()
        && design.output_cutoff_hz.is_finite()
        && design.initial_hz.is_finite();
    if !finite || design.minimum_hz < 0.0 || design.maximum_hz <= design.minimum_hz || design.output_cutoff_hz <= 0.0 {
        return Err(DspError::InvalidFrequency);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn design(sample_rate_hz: f64) -> HilbertDiscriminatorDesign {
        HilbertDiscriminatorDesign {
            sample_rate_hz,
            minimum_hz: 0.0,
            maximum_hz: 3_000.0,
            output_cutoff_hz: 1_800.0,
            initial_hz: 1_900.0,
        }
    }

    fn settled_estimate(discriminator: &mut HilbertDiscriminator, rate: u32, frequency: f64) -> f64 {
        let mut phase = 0.0_f64;
        let mut sum = 0.0;
        for sample in 0..rate / 5 {
            let estimate = discriminator.process_sample(libm::sin(phase));
            phase = libm::fmod(phase + TAU * frequency / f64::from(rate), TAU);
            if sample >= rate / 10 {
                sum += estimate;
            }
        }
        sum / f64::from(rate / 10)
    }

    #[rstest]
    #[case(8_000, 1)]
    #[case(11_025, 1)]
    #[case(16_000, 2)]
    #[case(22_050, 2)]
    #[case(40_000, 4)]
    #[case(48_000, 4)]
    fn hilbert_phase_lag_tracks_sample_rate(#[case] rate: u32, #[case] expected: usize) {
        let discriminator = HilbertDiscriminator::new(design(f64::from(rate))).unwrap();
        assert_eq!(discriminator.phase_lag(), expected);
    }

    #[rstest]
    #[case(8_000)]
    #[case(11_025)]
    #[case(16_000)]
    #[case(22_050)]
    #[case(40_000)]
    #[case(48_000)]
    fn hilbert_phase_lag_preserves_frequency_scale(#[case] rate: u32) {
        let expected = 1_900.0;
        let mut discriminator = HilbertDiscriminator::new(design(f64::from(rate))).unwrap();
        let estimate = settled_estimate(&mut discriminator, rate, expected);
        assert!((estimate - expected).abs() < 2.0, "{rate} Hz produced {estimate} Hz");
    }

    #[rstest]
    #[case(1_500.0)]
    #[case(1_550.0)]
    #[case(1_900.0)]
    #[case(2_300.0)]
    fn hilbert_image_tones_have_low_residual_ripple(#[case] frequency: f64) {
        let rate = 48_000;
        let mut discriminator = HilbertDiscriminator::new(design(f64::from(rate))).unwrap();
        let mut phase = 0.0_f64;
        let mut sum = 0.0;
        let mut squared = 0.0;
        let mut count = 0.0;
        for sample in 0..rate / 2 {
            let estimate = discriminator.process_sample(libm::sin(phase));
            phase = libm::fmod(phase + TAU * frequency / f64::from(rate), TAU);
            if sample >= rate / 4 {
                sum += estimate;
                squared += estimate * estimate;
                count += 1.0;
            }
        }
        let mean = sum / count;
        let standard_deviation = libm::sqrt(squared / count - mean * mean);
        assert!(
            standard_deviation < 6.0,
            "{frequency} Hz residual ripple was {standard_deviation} Hz"
        );
    }

    #[test]
    fn readings_are_clamped_to_the_design_range() {
        let rate = 48_000;
        let mut high = HilbertDiscriminator::new(HilbertDiscriminatorDesign {
            maximum_hz: 2_400.0,
            ..design(f64::from(rate))
        })
        .unwrap();
        let estimate = settled_estimate(&mut high, rate, 2_800.0);
        assert!((estimate - 2_400.0).abs() < 1.0, "estimate={estimate}");

        let mut low = HilbertDiscriminator::new(HilbertDiscriminatorDesign {
            minimum_hz: 1_000.0,
            ..design(f64::from(rate))
        })
        .unwrap();
        let estimate = settled_estimate(&mut low, rate, 400.0);
        assert!((estimate - 1_000.0).abs() < 1.0, "estimate={estimate}");
    }

    #[test]
    fn output_cutoff_bounds_the_tracking_rate() {
        let rate: u32 = 48_000;
        let span = |cutoff_hz: f64| {
            let mut discriminator = HilbertDiscriminator::new(HilbertDiscriminatorDesign {
                output_cutoff_hz: cutoff_hz,
                ..design(f64::from(rate))
            })
            .unwrap();
            let mut phase = 0.0_f64;
            let mut minimum = f64::MAX;
            let mut maximum = f64::MIN;
            for sample in 0..rate / 2 {
                // A 300 Hz alternation between 1500 and 2300 Hz, which is what
                // a keyed picture carrier looks like.
                let frequency = if (sample * 600 / rate).is_multiple_of(2) {
                    1_500.0
                } else {
                    2_300.0
                };
                let estimate = discriminator.process_sample(libm::sin(phase));
                phase = libm::fmod(phase + TAU * frequency / f64::from(rate), TAU);
                if sample >= rate / 4 {
                    minimum = minimum.min(estimate);
                    maximum = maximum.max(estimate);
                }
            }
            maximum - minimum
        };
        assert!(span(2_400.0) > span(100.0) * 2.0);
    }

    #[test]
    fn reset_restores_the_initial_frequency() {
        let rate = 48_000;
        let mut discriminator = HilbertDiscriminator::new(design(f64::from(rate))).unwrap();
        settled_estimate(&mut discriminator, rate, 1_500.0);
        discriminator.reset();
        assert_eq!(discriminator.held_frequency, 1_900.0);
        let mut fresh = HilbertDiscriminator::new(design(f64::from(rate))).unwrap();
        assert_eq!(discriminator.process_sample(0.25), fresh.process_sample(0.25));
    }

    #[test]
    fn design_validation_rejects_an_inverted_range() {
        let invalid = HilbertDiscriminatorDesign {
            minimum_hz: 2_000.0,
            maximum_hz: 1_000.0,
            ..design(48_000.0)
        };
        assert_eq!(
            HilbertDiscriminator::new(invalid).unwrap_err(),
            DspError::InvalidFrequency
        );
    }

    #[test]
    fn design_validation_rejects_a_non_finite_cutoff() {
        let invalid = HilbertDiscriminatorDesign {
            output_cutoff_hz: f64::NAN,
            ..design(48_000.0)
        };
        assert_eq!(
            HilbertDiscriminator::new(invalid).unwrap_err(),
            DspError::InvalidFrequency
        );
    }

    #[test]
    fn design_validation_rejects_a_non_positive_sample_rate() {
        assert_eq!(
            HilbertDiscriminator::new(design(0.0)).unwrap_err(),
            DspError::InvalidSampleRate
        );
    }
}
