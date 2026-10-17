//! The line protocol `gl-keyer` speaks, shared by the keyer and its clients.
//!
//! The keyer answers in `rigctld`'s framing, so a client reaches it through
//! [`grayline_rig::Rigctld`] like any rig. What that framing does not settle is
//! what the keyer's own commands carry: how a code is spelled, how a stream's
//! framing is written as arguments, and what a status reply's state means.
//! Those live here, so the two ends cannot spell them differently.
//!
//! ```text
//! KeyerClient::connect(address)
//!   client.open(&config)    the stream starts keying its lead-in
//!   client.send(&codes)
//!   client.end()            the tail follows once the codes run out
//!   client.status()         how far it has gone out
//! ```

#![deny(missing_docs)]

mod client;
mod codes;
mod framing;
mod state;

pub use client::{DEFAULT_ADDRESS, KeyerClient, Status};
pub use codes::{MAXIMUM_CODES_PER_LINE, decode_codes, encode_codes};
pub use framing::{decode_open, encode_open};
pub use state::StreamState;
