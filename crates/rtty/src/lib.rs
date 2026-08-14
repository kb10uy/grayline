//! Allocation-backed, `no_std` RTTY primitives for Grayline.
//!
//! RTTY carries text rather than pictures: the crate holds the ITA2 code, the
//! start-stop framing, an IIR-resonator receive path, and a three-state FSK
//! transmitter, with WAV handling and user interfaces left to its callers.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

extern crate alloc;

/// ITA2 character conversion and shift tracking.
pub mod code;
/// Errors reported while validating RTTY values and streaming input.
pub mod error;
/// Baud rate, tone pairs, and start-stop framing parameters.
pub mod params;
/// Audio front end, start-stop framing, and streaming text decoding.
pub mod rx;
/// Text encoding, keying, and PCM synthesis.
pub mod tx;

pub use error::RttyError;
pub use params::{BaudRate, BitLength, Parity, RxFraming, StopElement, StopTolerance, ToneSet, TxFraming};
pub use rx::{ReceivePipeline, RxConfig, RxEvent, RxOutcome};
pub use tx::{Transmitter, TxCode, TxConfig, encode_text};
