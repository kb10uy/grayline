//! Allocation-backed, `no_std` WEFAX receive primitives for Grayline.
//!
//! WEFAX is received, not transmitted: the signal is a weather service's
//! broadcast, so this crate carries a demodulator and a decoder and no
//! encoder. Test signals are synthesized by the tests that need them.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

extern crate alloc;

/// Errors reported while validating WEFAX values and streaming input.
pub mod error;
/// Index of cooperation, line rate, frequency band, and derived geometry.
pub mod format;
/// Allocation-backed grayscale rasters.
pub mod image;
/// Audio front end and streaming line decoding.
pub mod rx;

pub use error::WefaxError;
pub use format::{Format, Ioc, LinesPerMinute, WefaxBand};
pub use image::GrayRaster;
pub use rx::{Demodulator, ReceivePipeline, RxConfig, WefaxDecoder};
