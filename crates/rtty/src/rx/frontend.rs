use grayline_dsp::{
    filter::{
        Fir, FirDesign, FirKind, Iir, IirLowPassDesign, IirResponse, MovingAverage, MovingAverageDesign, Resonator,
    },
    level::{PeakNormalizer, PeakNormalizerDesign},
};

use crate::{
    RttyError,
    params::ToneSet,
    rx::{
        atc::Atc,
        config::{IntegratorDesign, RxConfig},
        framing::Bit,
        monitor::ChannelLevels,
    },
};

/// Base order of the input band-pass at 11025 Hz, scaled linearly with the
/// capture rate as the other receive front ends do.
const BAND_PASS_BASE_ORDER: f64 = 24.0;
/// Seconds the peak normalizer's gain estimate decays over: 4.5 bit times at
/// 45.45 baud, slow enough not to pump within one character.
const NORMALIZER_DECAY_SECONDS: f64 = 0.1;

pub(crate) struct FrontEndOutput {
    pub(crate) bit: Bit,
    /// What the comparator compared, for the squelch and the monitor tap.
    pub(crate) levels: ChannelLevels,
}

enum Integrator {
    Average(MovingAverage),
    LowPass(Iir),
}

impl Integrator {
    fn process_sample(&mut self, sample: f64) -> f64 {
        match self {
            Self::Average(filter) => filter.process_sample(sample),
            Self::LowPass(filter) => filter.process_sample(sample),
        }
    }
}

struct Channel {
    resonator: Resonator,
    integrator: Integrator,
    atc: Option<Atc>,
}

impl Channel {
    fn process_sample(&mut self, sample: f64) -> f64 {
        let envelope = self
            .integrator
            .process_sample(self.resonator.process_sample(sample).abs());
        match &mut self.atc {
            Some(atc) => atc.process_sample(envelope),
            None => envelope,
        }
    }
}

/// The per-sample receive chain up to the comparator.
pub(crate) struct FrontEnd {
    band_pass: Fir,
    normalizer: PeakNormalizer,
    mark: Channel,
    space: Channel,
    tones: ToneSet,
}

impl FrontEnd {
    /// Builds the chain for `tones`, which the caller has already reversed
    /// if the sideband is inverted.
    pub(crate) fn new(sample_rate_hz: f64, tones: ToneSet, config: &RxConfig) -> Result<Self, RttyError> {
        tones.validate(sample_rate_hz)?;
        let mut order = libm::round(BAND_PASS_BASE_ORDER * sample_rate_hz / 11_025.0) as usize;
        order = order.max(12);
        if !order.is_multiple_of(2) {
            order += 1;
        }
        let band_pass = Fir::from_design(FirDesign {
            kind: FirKind::BandPass,
            order,
            sample_rate_hz,
            lower_frequency_hz: (tones.low_hz() - config.band_width_hz).max(10.0),
            upper_frequency_hz: (tones.high_hz() + config.band_width_hz).min(sample_rate_hz * 0.5 - 1.0),
            attenuation_db: 20.0,
            gain: 1.0,
        })?;
        let channel = |frequency_hz: f64| -> Result<Channel, RttyError> {
            let integrator = match config.integrator {
                IntegratorDesign::Average { smoothing_hz } => {
                    Integrator::Average(MovingAverage::new(MovingAverageDesign {
                        sample_rate_hz,
                        smoothing_hz,
                    })?)
                }
                IntegratorDesign::LowPass { order, cutoff_hz } => {
                    Integrator::LowPass(Iir::from_low_pass(IirLowPassDesign {
                        order,
                        sample_rate_hz,
                        cutoff_hz,
                        response: IirResponse::Butterworth,
                    })?)
                }
            };
            Ok(Channel {
                resonator: Resonator::new(sample_rate_hz, frequency_hz, config.detector_bandwidth_hz)?,
                integrator,
                atc: config.atc.map(|design| Atc::new(sample_rate_hz, design)).transpose()?,
            })
        };
        Ok(Self {
            band_pass,
            normalizer: PeakNormalizer::new(PeakNormalizerDesign {
                sample_rate_hz,
                decay_seconds: NORMALIZER_DECAY_SECONDS,
                floor: 1.0e-6,
            })?,
            mark: channel(tones.mark_hz)?,
            space: channel(tones.space_hz)?,
            tones,
        })
    }

    /// Runs one sample through the chain to a comparator decision.
    pub(crate) fn process(&mut self, input: f64) -> FrontEndOutput {
        let filtered = self.band_pass.process_sample(input);
        let normalized = self.normalizer.process_sample(filtered);
        let mark = self.mark.process_sample(normalized);
        let space = self.space.process_sample(normalized);
        FrontEndOutput {
            bit: if mark >= space { Bit::Mark } else { Bit::Space },
            levels: ChannelLevels { mark, space },
        }
    }

    /// Returns the pair the resonators sit on.
    pub(crate) fn tones(&self) -> ToneSet {
        self.tones
    }

    /// Retunes both resonators, keeping their state, as AFC requires.
    ///
    /// The input band-pass keeps its original design: a FIR cannot retune
    /// without discarding its state, and the default width leaves the AFC
    /// hundreds of hertz of passband to move in.
    pub(crate) fn set_tones(&mut self, tones: ToneSet) -> Result<(), RttyError> {
        self.mark.resonator.set_frequency(tones.mark_hz)?;
        self.space.resonator.set_frequency(tones.space_hz)?;
        self.tones = tones;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use core::f64::consts::TAU;

    use super::*;
    use crate::params::ToneSet;
    use rstest::rstest;

    fn settled_bits(rate: f64, tone_hz: f64, tones: ToneSet, config: &RxConfig) -> (usize, usize) {
        let mut front_end = FrontEnd::new(rate, tones, config).unwrap();
        let mut marks = 0;
        let mut spaces = 0;
        for index in 0..(rate as usize) {
            let output = front_end.process(libm::sin(TAU * tone_hz * index as f64 / rate));
            if index > rate as usize / 2 {
                match output.bit {
                    Bit::Mark => marks += 1,
                    Bit::Space => spaces += 1,
                }
            }
        }
        (marks, spaces)
    }

    #[rstest]
    #[case(8_000.0, 170.0)]
    #[case(11_025.0, 170.0)]
    #[case(11_025.0, 200.0)]
    #[case(11_025.0, 450.0)]
    #[case(44_100.0, 170.0)]
    #[case(48_000.0, 450.0)]
    fn a_held_mark_tone_dominates_the_mark_channel(#[case] rate: f64, #[case] shift: f64) {
        let tones = ToneSet::from_center_and_shift(2_210.0, shift);
        let config = RxConfig::default();
        let (marks, spaces) = settled_bits(rate, tones.mark_hz, tones, &config);
        assert!(marks > spaces * 50, "marks {marks}, spaces {spaces}");
        let (marks, spaces) = settled_bits(rate, tones.space_hz, tones, &config);
        assert!(spaces > marks * 50, "marks {marks}, spaces {spaces}");
    }

    #[rstest]
    #[case(IntegratorDesign::Average { smoothing_hz: 70.0 })]
    #[case(IntegratorDesign::LowPass { order: 5, cutoff_hz: 40.0 })]
    fn both_integrator_forms_settle(#[case] integrator: IntegratorDesign) {
        let config = RxConfig {
            integrator,
            ..RxConfig::default()
        };
        let tones = ToneSet::AFSK_170;
        let (marks, spaces) = settled_bits(11_025.0, tones.mark_hz, tones, &config);
        assert!(marks > spaces * 50, "marks {marks}, spaces {spaces}");
    }

    #[test]
    fn retuning_moves_the_decision_with_the_tones() {
        let offset = ToneSet {
            mark_hz: 2_155.0,
            space_hz: 2_325.0,
        };
        let mut front_end = FrontEnd::new(11_025.0, ToneSet::AFSK_170, &RxConfig::default()).unwrap();
        front_end.set_tones(offset).unwrap();
        assert_eq!(front_end.tones(), offset);
        let mut spaces = 0;
        for index in 0..11_025 {
            // 2225 Hz sits on the space side of the old pair's midpoint but
            // on the mark side of the retuned pair's, so a detector still on
            // the old pair would call it space.
            let output = front_end.process(libm::sin(TAU * 2_225.0 * index as f64 / 11_025.0));
            if index > 5_512 && output.bit == Bit::Space {
                spaces += 1;
            }
        }
        assert!(spaces < 500, "still spacing {spaces} samples");
    }
}
