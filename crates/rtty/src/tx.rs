//! Text encoding, keying, and PCM synthesis.

mod config;
mod encoder;
mod modulator;

pub use config::{Diddle, TxConfig};
pub use encoder::{TxCode, encode_text};
pub use modulator::{Keying, Transmitter};
