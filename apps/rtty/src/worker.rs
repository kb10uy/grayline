//! The thread that runs a reception, and the audio device it is attached to.
//!
//! The interface never blocks on any of this. The worker owns its own state
//! and publishes a snapshot the interface reads once per frame.

pub mod audio;
pub mod contact;
pub mod keyer;
pub mod receive;
pub mod transmit;
