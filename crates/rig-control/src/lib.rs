//! Rig control for Grayline: when the rig is keyed, and what it is told.
//!
//! [`grayline_rig`] is how a rig is reached; this crate is the layer above it
//! that every transmitting application needs and no mode decides. It owns the
//! thread that holds the transports, the host for the operator's own
//! `rigcontrol.lua`, and the band plan that script reads.
//!
//! The division the layer below records holds here too: what to send a rig is
//! the operator's script, because what a rig wants around a transmission is a
//! property of that rig and of the station around it. What this crate decides
//! is only *when* the script is called — the two sides of a transmission, a
//! tuning request, and the frequency poll.
//!
//! An application supplies the settings, the directory the script and the
//! plan are read from, and the moments:
//!
//! ```text
//! RigWorker::spawn(&settings, config_dir, bands)
//!   worker.transmit()   before the first sample goes out
//!   worker.receive()    once the last sample has been played
//! ```
//!
//! Each application reads its own directory, so a station running two of them
//! has a script per application rather than one the two could disagree over.

#![deny(missing_docs)]

use std::sync::Mutex;

pub mod bands;
mod error;
pub mod script;
mod settings;
mod worker;

#[cfg(test)]
mod test_util;

pub use error::RigControlError;
pub use grayline_rig::{DEFAULT_ADDRESS, RigError};
pub use settings::{
    DEFAULT_LEAD_IN_SECONDS, DEFAULT_POLL_SECONDS, DEFAULT_PORT_NAME, DEFAULT_TAIL_SECONDS, PortSettings, RigSettings,
};
pub use worker::{Reading, RigSnapshot, RigState, RigWorker};

/// Applies a change to a published snapshot, leaving it alone if the worker
/// that owns it panicked while holding the lock.
pub(crate) fn update<T>(snapshot: &Mutex<T>, update: impl FnOnce(&mut T)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}
