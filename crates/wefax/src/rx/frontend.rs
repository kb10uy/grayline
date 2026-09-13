use grayline_dsp::{
    filter::{Fir, FirDesign, FirKind, Iir, IirLowPassDesign, IirResponse},
    frequency::{HilbertDiscriminator, HilbertDiscriminatorDesign},
};

use crate::error::WefaxError;

/// Lower edge of the receive band-pass, in hertz.
///
/// The picture occupies 1500 to 2300 Hz, but the automatic picture
/// transmission tones key that carrier at up to 675 Hz, which puts first-order
/// sidebands near 825 and 2975 Hz. A band cut to the picture alone would blunt
/// the start-tone detection this front end exists to feed.
const BAND_LOWER_HZ: f64 = 1_000.0;
/// Upper edge of the receive band-pass, in hertz.
const BAND_UPPER_HZ: f64 = 2_800.0;
/// Margin kept between the band-pass and Nyquist, in hertz.
const BAND_MARGIN_HZ: f64 = 100.0;
/// Stopband attenuation of the receive band-pass, in decibels.
const BAND_ATTENUATION_DB: f64 = 20.0;
/// Band-pass order at 11 025 Hz, scaled linearly with the sampling frequency.
const BAND_REFERENCE_ORDER: f64 = 24.0;
/// Sampling frequency the reference order is stated at, in hertz.
const BAND_REFERENCE_RATE_HZ: f64 = 11_025.0;
const BAND_MINIMUM_ORDER: usize = 12;

const LEVEL_CUTOFF_HZ: f64 = 5.0;

/// Cutoff of the discriminator's output filter, in hertz.
///
/// Both quantities this sits between are properties of the signal rather than
/// of the capture rate, so the cutoff does not scale with the latter. Above
/// it is the discriminator's own ripple at twice the carrier, which starts at
/// 3000 Hz for black. Below it is the picture: an IOC 576 line at 120 lines
/// per minute is 3620 pixels per second, so 1810 Hz carries it whole. That
/// this lands on the figure the SSTV front end uses is not a coincidence —
/// the same discriminator is reading the same audio band.
const OUTPUT_CUTOFF_HZ: f64 = 1_800.0;

/// The audio stage: band-pass, frequency discrimination, and a level meter.
///
/// There is no automatic gain control here, unlike the SSTV front end. The
/// tone detection WEFAX needs runs on the demodulated stream rather than on
/// the waveform, and that stream carries no amplitude to normalize.
#[derive(Clone, Debug)]
pub(crate) struct FrontEnd {
    band_pass: Fir,
    discriminator: HilbertDiscriminator,
    level: Iir,
    level_power: f64,
}

impl FrontEnd {
    pub(crate) fn new(sample_rate_hz: f64) -> Result<Self, WefaxError> {
        let mut order = libm::round(BAND_REFERENCE_ORDER * sample_rate_hz / BAND_REFERENCE_RATE_HZ) as usize;
        order = order.max(BAND_MINIMUM_ORDER);
        if !order.is_multiple_of(2) {
            order += 1;
        }
        let upper_frequency_hz = BAND_UPPER_HZ.min(sample_rate_hz * 0.5 - BAND_MARGIN_HZ);
        Ok(Self {
            band_pass: Fir::from_design(FirDesign {
                kind: FirKind::BandPass,
                order,
                sample_rate_hz,
                lower_frequency_hz: BAND_LOWER_HZ,
                upper_frequency_hz,
                attenuation_db: BAND_ATTENUATION_DB,
                gain: 1.0,
            })?,
            discriminator: HilbertDiscriminator::new(HilbertDiscriminatorDesign {
                sample_rate_hz,
                minimum_hz: BAND_LOWER_HZ,
                maximum_hz: BAND_UPPER_HZ,
                output_cutoff_hz: OUTPUT_CUTOFF_HZ,
                initial_hz: (BAND_LOWER_HZ + BAND_UPPER_HZ) * 0.5,
            })?,
            level: Iir::from_low_pass(IirLowPassDesign {
                order: 2,
                sample_rate_hz,
                cutoff_hz: LEVEL_CUTOFF_HZ,
                response: IirResponse::Butterworth,
            })?,
            level_power: 0.0,
        })
    }

    /// Reports the instantaneous frequency of one input sample, in hertz.
    pub(crate) fn process(&mut self, input: f64) -> f64 {
        let filtered = self.band_pass.process_sample(input);
        self.level_power = self.level.process_sample(filtered * filtered).max(0.0);
        self.discriminator.process_sample(filtered)
    }

    /// Returns the smoothed root-mean-square level of the received band.
    pub(crate) fn level(&self) -> f64 {
        libm::sqrt(self.level_power)
    }

    pub(crate) fn reset(&mut self) {
        self.band_pass.reset();
        self.discriminator.reset();
        self.level.reset();
        self.level_power = 0.0;
    }
}
