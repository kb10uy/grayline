//! The contact directory for Grayline.
//!
//! A directory of stations, not a log of contacts. What this answers is what is
//! known about a callsign, which is a property of the station; who was worked,
//! when, on what band, and with what report is a property of a contact and
//! belongs in the operator's own logger. Nothing here records that a contact
//! took place, and the fields an upstream logger offers about one are dropped
//! on the way in rather than stored.
//!
//! What it holds is a table from a callsign to what is filed under it, filled
//! in by hand, by importing an ADIF the operator already has, or by asking
//! their own Wavelog, and read back the next time the same station is worked.
//! An application reads it so a transmit template can print the other
//! operator's name without the operator typing it again.

#![deny(missing_docs)]

mod error;
mod record;

pub use error::QsoError;
pub use record::{Record, WELL_KNOWN_KEYS, normalize_callsign, valid_key};
