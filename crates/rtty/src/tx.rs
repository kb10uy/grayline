//! Text encoding, keying, and PCM synthesis.

mod config;
mod encoder;
mod framer;
mod modulator;
mod schedule;

pub use config::{Diddle, TxConfig};
pub use encoder::{TxCode, encode_text};
pub use framer::{Framer, Keying};
pub use modulator::Transmitter;
pub use schedule::TxSchedule;
