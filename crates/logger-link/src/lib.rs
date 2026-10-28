//! Filling the entry form of the operator's logger from an application.
//!
//! The logger is jl1his-logger's desktop client, which takes JSON datagrams on
//! a local UDP port and fills its entry form field by field from them. What an
//! application knows about the station being worked — its callsign, the
//! reports, a name and a QTH — is sent there as it is settled, so the contact
//! is logged without being typed twice. The frequency and the mode are not:
//! the logger reads them from the rig itself.
//!
//! ```text
//! let mut link = LoggerLink::open(&settings)?;
//! link.reflect([("CALL", "JA1ABC"), ("RST_RCVD", "599")])?;
//! ```
//!
//! Nothing comes back. The logger answers no datagram, so a link has no state
//! beyond what it last sent, and a logger that is not running loses nothing
//! but the form it was not showing.

#![deny(missing_docs)]

mod link;
mod settings;

pub use link::{LinkError, LoggerLink, set_message};
pub use settings::{DEFAULT_ADDRESS, LoggerSettings};
