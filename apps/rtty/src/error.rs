//! The failures the interface reports, carried as values rather than text.
//!
//! The receive worker runs on its own thread and publishes what went wrong in
//! a snapshot, so an error has to survive being cloned out of a mutex.

use grayline_audio::AudioError;
use grayline_rtty::RttyError;
use thiserror::Error;

#[derive(Clone, Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Rtty(#[from] RttyError),
    /// Decoding stopped part way through a block of audio.
    #[error("receive decoding failed at sample {sample}: {source}")]
    Decode {
        /// Absolute sample position the decoder stopped at.
        sample: u64,
        #[source]
        source: RttyError,
    },
    #[error("reception could not be restarted after a capture overrun")]
    CaptureRestartFailed,
    /// A recording could not be opened or read.
    #[error("{0}")]
    Wav(String),
    /// A worker panicked while holding its snapshot, so its state is unknown.
    #[error("{0} state is unavailable")]
    WorkerUnavailable(&'static str),
}
