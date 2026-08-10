//! Measurement of the instantaneous frequency of a signal.

mod fm;
mod pll;
mod zero_crossing;

pub use fm::{HilbertDiscriminator, HilbertDiscriminatorDesign};
pub use pll::{Pll, PllDesign};
pub use zero_crossing::ZeroCrossingFrequency;
