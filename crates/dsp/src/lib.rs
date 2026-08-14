//! Allocation-backed, `no_std` signal-processing primitives for Grayline.
//!
//! Processors own their state and allocate only during construction or explicit
//! reconfiguration. Per-sample and in-place block processing do not allocate.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

extern crate alloc;

/// Envelope detection of narrow-band tones.
pub mod detector;
/// Errors reported while validating DSP configurations.
pub mod error;
/// Finite and infinite impulse response filters, and narrow-band resonators.
pub mod filter;
/// Zero-crossing, phase-locked, and Hilbert phase-difference frequency measurement.
pub mod frequency;
/// Level tracking and normalization.
pub mod level;
/// Oscillators and voltage-controlled oscillators.
pub mod oscillator;
/// Discrete Fourier and Hilbert transforms.
pub mod transform;

mod validate;

pub use error::DspError;
