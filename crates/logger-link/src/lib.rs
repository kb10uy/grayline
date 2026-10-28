//! Sharing the entry form of the operator's logger with an application.
//!
//! The logger is jl1his-logger's desktop client, which takes JSON datagrams on
//! a local UDP port and fills its entry form field by field from them. What an
//! application knows about the station being worked — its callsign, the
//! reports, a name and a QTH — is sent there as it is settled, so the contact
//! is logged without being typed twice. The frequency and the mode are not:
//! the logger reads them from the rig itself.
//!
//! The link also subscribes to the form. A logger sharing it answers with the
//! whole form and sends it again whenever it changes, so the application can
//! work from the logger's form rather than keeping one of its own.
//!
//! ```text
//! let mut link = LoggerLink::new("logger", move || waker.wake());
//! link.reconfigure(&settings)?;
//! link.reflect([("CALL", "JA1ABC"), ("RST_RCVD", "599")])?;
//! if let Some(update) = link.take_form() {
//!     call = update.form.get("CALL").to_owned();
//! }
//! ```

#![deny(missing_docs)]

mod link;
mod settings;
mod subscription;

pub use link::{LinkError, LoggerLink, set_message};
pub use settings::{DEFAULT_ADDRESS, LoggerSettings};
pub use subscription::{FormUpdate, LoggerForm};
