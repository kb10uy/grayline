//! The failures the interface reports, carried as values rather than text.
//!
//! A worker runs on its own thread and publishes what went wrong in a
//! snapshot, so an error has to survive being cloned out of a mutex. Every
//! error the application shows is one of these, which keeps the point where a
//! failure is described apart from the point where it is displayed.

use std::sync::Arc;

use grayline_audio::AudioError;
use grayline_dsp::DspError;
use grayline_qso::QsoError;
use grayline_rig::RigError;
use grayline_sstv::SstvError;
use grayline_sstv_rx::DemodulatorError;
use grayline_sstv_template::TemplateError;
use grayline_tone_tx::ModulatorError;
use thiserror::Error;

use crate::worker::rig::script::ScriptError;

#[derive(Clone, Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Demodulator(#[from] DemodulatorError),
    #[error(transparent)]
    Modulator(#[from] ModulatorError),
    #[error(transparent)]
    Dsp(#[from] DspError),
    #[error(transparent)]
    Sstv(#[from] SstvError),
    #[error(transparent)]
    Rig(#[from] RigError),
    #[error(transparent)]
    Script(#[from] ScriptError),
    /// A template could not be parsed, rendered, or composed.
    ///
    /// Held behind an [`Arc`] because it carries KDL, image, and SVG errors
    /// that are not themselves cloneable.
    #[error("{0}")]
    Template(Arc<TemplateError>),
    /// A contact lookup failed.
    ///
    /// Behind an [`Arc`] for the same reason: it carries a store failure that
    /// is not cloneable, and the snapshot holding it is cloned every frame.
    #[error("{0}")]
    Qso(Arc<QsoError>),

    #[error("no output device is selected")]
    NoOutputDevice,
    #[error("playback closed before the transmission started")]
    PlaybackClosed,
    #[error("receive decoding failed at sample {sample}: {source}")]
    Decode {
        /// Absolute PCM sample position the decoder stopped at.
        sample: u64,
        #[source]
        source: SstvError,
    },
    #[error("staging the refinement tail failed: {0}")]
    RefinementStaging(#[source] SstvError),
    #[error("slant refinement failed: {0}")]
    Refinement(#[source] SstvError),
    #[error("reception could not be restarted after a capture overrun")]
    CaptureRestartFailed,
    #[error("audio playback ran out of samples")]
    PlaybackUnderrun,
    /// A worker panicked while holding its snapshot, so its state is unknown.
    #[error("{0} state is unavailable")]
    WorkerUnavailable(&'static str),
}

impl From<TemplateError> for AppError {
    fn from(error: TemplateError) -> Self {
        Self::Template(Arc::new(error))
    }
}

impl From<QsoError> for AppError {
    fn from(error: QsoError) -> Self {
        Self::Qso(Arc::new(error))
    }
}
