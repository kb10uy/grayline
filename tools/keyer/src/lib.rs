//! Serial-line keying for Grayline.
//!
//! A keyer owns a serial port and times FSK and PTT on its control lines
//! itself, because a bit at RTTY speeds is shorter than a socket round trip
//! can be trusted to be. Clients reach it over the line protocol `rigctld`
//! speaks, so the same transport and script that reach the rig reach this.

#![deny(unsafe_code)]

/// The time base keying is laid out against.
pub mod clock;
/// Errors from the keyer's own hardware and socket.
pub mod error;
/// The outputs a keyer switches.
pub mod line;
/// Ways of putting a code stream on the air.
pub mod method;
/// The keying thread and what it shares with the connections.
pub mod player;
/// The line protocol clients speak.
pub mod protocol;
/// The listening socket and one thread per client.
pub mod server;
/// What clients have asked of the keyer.
pub mod session;
/// One stream from its first code to its tail.
pub mod stream;

#[cfg(test)]
mod test_util;

pub use error::KeyerError;

/// Where the keyer reports what happened.
pub type Logger = fn(&str);
