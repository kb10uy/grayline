use crate::{
    code::CodeSet,
    params::{RxFraming, ToneSet},
    rx::afc::AfcConfig,
};

/// How the rectified channels are integrated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IntegratorDesign {
    /// A moving average whose window is the reciprocal of this frequency.
    Average {
        /// Reciprocal of the averaging window, in hertz. MMTTY's default is
        /// 70 Hz.
        smoothing_hz: f64,
    },
    /// A Butterworth low-pass, whose cutoff must be set lower than the
    /// equivalent averaging setting because this one really is a cutoff.
    LowPass {
        /// Filter order; MMTTY uses 5.
        order: usize,
        /// Cutoff in hertz; MMTTY uses 40.
        cutoff_hz: f64,
    },
}

impl Default for IntegratorDesign {
    fn default() -> Self {
        Self::Average { smoothing_hz: 70.0 }
    }
}

/// Parameters for the automatic threshold corrector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AtcDesign {
    /// How many 64-sample extremum blocks the corrector remembers beyond the
    /// current one. MMTTY's default is 4.
    pub blocks: usize,
}

impl Default for AtcDesign {
    fn default() -> Self {
        Self { blocks: 4 }
    }
}

/// Everything a receiver needs to know.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RxConfig {
    /// The mark and space tones.
    pub tones: ToneSet,
    /// The start-stop framing to expect.
    pub framing: RxFraming,
    /// Swaps the tones, for an inverted sideband.
    pub reverse: bool,
    /// Which BELL convention the decoder prints.
    pub code_set: CodeSet,
    /// Returns to letters case on a space received in figures.
    pub unshift_on_space: bool,
    /// Bandwidth of each tone resonator in hertz; MMTTY's `IIRBW` is 60.
    pub detector_bandwidth_hz: f64,
    /// How the rectified channels are integrated.
    pub integrator: IntegratorDesign,
    /// The automatic threshold corrector, off by default as MMTTY ships it.
    pub atc: Option<AtcDesign>,
    /// The squelch threshold on the smoothed channel difference, in the
    /// normalized scale, or `None` for no squelch.
    ///
    /// MMTTY's threshold lives on its limiter's integer scale and cannot be
    /// copied; this default was calibrated against the integration tests:
    /// broadband noise alone peaks near 0.15 on this scale while a clean or
    /// 10 dB signal reads about 0.5, at every supported capture rate.
    pub squelch_threshold: Option<f64>,
    /// Automatic frequency control, off by default for file decoding.
    pub afc: Option<AfcConfig>,
    /// Width added on each side of the tones by the input band-pass, in hertz.
    pub band_width_hz: f64,
    /// Keeps a character whose stop element failed, still counting the error.
    pub ignore_framing_errors: bool,
}

impl Default for RxConfig {
    fn default() -> Self {
        Self {
            tones: ToneSet::default(),
            framing: RxFraming::default(),
            reverse: false,
            code_set: CodeSet::default(),
            unshift_on_space: true,
            detector_bandwidth_hz: 60.0,
            integrator: IntegratorDesign::default(),
            atc: None,
            squelch_threshold: Some(0.25),
            afc: None,
            band_width_hz: 250.0,
            ignore_framing_errors: false,
        }
    }
}
