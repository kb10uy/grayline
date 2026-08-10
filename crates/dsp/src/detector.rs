//! Envelope detection of narrow-band tones.

use crate::{
    DspError,
    filter::{Iir, IirLowPassDesign, IirResponse, Resonator},
};

/// Parameters for a resonator-and-envelope tone detector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneDetectorDesign {
    /// Sampling frequency in hertz.
    pub sample_rate_hz: f64,
    /// Center frequency in hertz.
    pub frequency_hz: f64,
    /// Detection bandwidth in hertz.
    pub bandwidth_hz: f64,
    /// Cutoff of the second-order Butterworth envelope filter, in hertz.
    ///
    /// This is what trades the envelope's response time against how much of
    /// the tone's own cycle survives into the output.
    pub envelope_cutoff_hz: f64,
}

/// A narrow-band tone detector reporting the envelope of one frequency.
///
/// A [`Resonator`] selects the tone and a low-pass filter follows its
/// rectified output, so the reported value rises with how much of that one
/// frequency the input carries. The input is not required to be audio: the
/// resonator is a plain two-pole band-pass, so a demodulated stream is as
/// valid an input as a waveform.
#[derive(Clone, Debug)]
pub struct ToneDetector {
    resonator: Resonator,
    envelope: Iir,
}

impl ToneDetector {
    /// Designs and creates a detector.
    pub fn new(design: ToneDetectorDesign) -> Result<Self, DspError> {
        Ok(Self {
            resonator: Resonator::new(design.sample_rate_hz, design.frequency_hz, design.bandwidth_hz)?,
            envelope: Iir::from_low_pass(IirLowPassDesign {
                order: 2,
                sample_rate_hz: design.sample_rate_hz,
                cutoff_hz: design.envelope_cutoff_hz,
                response: IirResponse::Butterworth,
            })?,
        })
    }

    /// Reports the tone envelope for one sample.
    ///
    /// The envelope filter can undershoot on a falling edge, so the result is
    /// held at zero rather than reporting a negative amount of a tone.
    pub fn process_sample(&mut self, sample: f64) -> f64 {
        self.envelope
            .process_sample(self.resonator.process_sample(sample).abs())
            .max(0.0)
    }

    /// Replaces a block with its per-sample envelope.
    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }

    /// Returns the current center frequency in hertz.
    pub fn frequency_hz(&self) -> f64 {
        self.resonator.frequency_hz()
    }

    /// Retunes the center frequency while preserving detector state.
    pub fn set_frequency(&mut self, frequency_hz: f64) -> Result<(), DspError> {
        self.resonator.set_frequency(frequency_hz)
    }

    /// Clears both stages without changing the tuning.
    pub fn reset(&mut self) {
        self.resonator.reset();
        self.envelope.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::TAU;

    const SAMPLE_RATE: f64 = 11_025.0;

    fn design(frequency_hz: f64, envelope_cutoff_hz: f64) -> ToneDetectorDesign {
        ToneDetectorDesign {
            sample_rate_hz: SAMPLE_RATE,
            frequency_hz,
            bandwidth_hz: 80.0,
            envelope_cutoff_hz,
        }
    }

    fn settled_envelope(detector: &mut ToneDetector, input_frequency: f64, samples: usize) -> f64 {
        let mut envelope = 0.0;
        for index in 0..samples {
            envelope = detector.process_sample(libm::sin(TAU * input_frequency * index as f64 / SAMPLE_RATE));
        }
        envelope
    }

    #[test]
    fn center_frequency_dominates_an_adjacent_tone() {
        let mut center = ToneDetector::new(design(1_200.0, 50.0)).unwrap();
        let mut adjacent = ToneDetector::new(design(1_200.0, 50.0)).unwrap();
        let matched = settled_envelope(&mut center, 1_200.0, 11_025);
        let offset = settled_envelope(&mut adjacent, 1_320.0, 11_025);
        assert!(matched > offset * 2.0, "matched={matched} offset={offset}");
    }

    #[test]
    fn envelope_cutoff_sets_how_much_of_the_tone_survives() {
        // Rectifying a 1200 Hz tone leaves a 2400 Hz ripple, so a wider
        // envelope filter passes proportionally more of it.
        let ripple = |envelope_cutoff_hz: f64| {
            let mut detector = ToneDetector::new(design(1_200.0, envelope_cutoff_hz)).unwrap();
            let mut sum = 0.0;
            let mut squared = 0.0;
            let mut count = 0.0;
            for index in 0..11_025 {
                let envelope = detector.process_sample(libm::sin(TAU * 1_200.0 * index as f64 / SAMPLE_RATE));
                if index >= 5_512 {
                    sum += envelope;
                    squared += envelope * envelope;
                    count += 1.0;
                }
            }
            let mean = sum / count;
            libm::sqrt(squared / count - mean * mean)
        };
        let (wide, narrow) = (ripple(400.0), ripple(50.0));
        assert!(wide > narrow * 4.0, "wide={wide} narrow={narrow}");
    }

    #[test]
    fn set_frequency_preserves_envelope_state() {
        let mut detector = ToneDetector::new(design(1_200.0, 50.0)).unwrap();
        settled_envelope(&mut detector, 1_200.0, 11_025);
        detector.set_frequency(1_320.0).unwrap();
        assert_eq!(detector.frequency_hz(), 1_320.0);
        assert!(detector.process_sample(0.0) > 0.0);
    }

    #[test]
    fn reset_clears_both_stages() {
        let mut detector = ToneDetector::new(design(1_200.0, 50.0)).unwrap();
        settled_envelope(&mut detector, 1_200.0, 11_025);
        detector.reset();
        assert_eq!(detector.process_sample(0.0), 0.0);
    }

    #[test]
    fn design_validation_rejects_a_negative_bandwidth() {
        let invalid = ToneDetectorDesign {
            bandwidth_hz: -1.0,
            ..design(1_200.0, 50.0)
        };
        assert_eq!(ToneDetector::new(invalid).unwrap_err(), DspError::InvalidBandwidth);
    }
}
