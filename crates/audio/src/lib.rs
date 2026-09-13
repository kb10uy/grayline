//! Host audio adapters for Grayline.
//!
//! This crate owns device enumeration, stream formats, WAV input, and callback
//! scheduling. It exposes normalized mono `f32` samples with stream positions
//! and nothing from the underlying host API, so the SSTV core and the
//! application never depend on a platform audio type.

#![deny(missing_docs)]

mod capture;
mod device;
mod error;
mod host;
mod playback;
mod wav;

pub use capture::{Capture, CaptureReader, CaptureWriter, Reading, synthetic_capture};
pub use device::{InputDevice, OutputDevice};
pub use error::{AudioError, FaultKind, FaultSlot, StreamFault, WavError};
pub use host::AudioHost;
pub use playback::{Playback, PlaybackReader, PlaybackWriter, synthetic_playback};
pub use wav::WavSource;

/// Capture rate preferred by the rest of the project.
pub const PREFERRED_SAMPLE_RATE_HZ: u32 = 48_000;

/// Lowest rate the SSTV receive front end accepts.
pub const MINIMUM_SAMPLE_RATE_HZ: u32 = 6_000;
