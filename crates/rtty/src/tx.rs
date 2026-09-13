//! Text encoding, keying, and PCM synthesis.

mod config;
mod encoder;
mod modulator;
mod schedule;

pub use config::{Diddle, TxConfig};
pub use encoder::{TxCode, encode_text};
pub use modulator::{Keying, Transmitter};
pub use schedule::TxSchedule;
