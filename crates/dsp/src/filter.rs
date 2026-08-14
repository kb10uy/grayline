//! Filters that shape a signal's spectrum while leaving its domain unchanged.

mod fir;
mod iir;
mod moving_average;
mod resonator;

pub use fir::{Fir, FirDesign, FirKind};
pub use iir::{Iir, IirLowPassDesign, IirResponse, SosCoefficients};
pub use moving_average::{MovingAverage, MovingAverageDesign};
pub use resonator::Resonator;
