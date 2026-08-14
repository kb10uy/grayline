//! Audio front end, start-stop framing, and streaming text decoding.

mod afc;
mod atc;
mod config;
mod event;
mod framing;
mod frontend;
mod pipeline;
mod squelch;

pub use afc::{AfcConfig, AfcMode};
pub use config::{AtcDesign, IntegratorDesign, RxConfig};
pub use event::{RxEvent, RxOutcome};
pub use framing::{Bit, FramingOutcome};
pub use pipeline::{MINIMUM_SAMPLE_RATE_HZ, ReceivePipeline};
