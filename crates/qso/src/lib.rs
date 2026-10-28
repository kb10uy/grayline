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
//! in by hand or by importing an ADIF the operator already has, and read back
//! the next time the same station is worked.
//! An application reads it so a transmit template can print the other
//! operator's name without the operator typing it again.

#![deny(missing_docs)]

mod adif;
mod directory;
mod editor;
mod error;
mod paths;
mod record;
mod settings;
mod store;
mod worker;

pub use adif::{Adif, AdifRecord, ImportReport, import, read_adif};
pub use directory::{Directory, Lookup};
pub use editor::{ContactEditor, ContactRow};
pub use error::QsoError;
pub use paths::{FAMILY_DIRECTORY, STORE_FILE, default_store_path};
pub use record::{
    DEFAULT_FIELDS, FIELD_GROUPS, GROUP_PREFIX, Record, WELL_KNOWN_KEYS, expand_fields, field_group,
    normalize_callsign, valid_key,
};
pub use settings::ContactSettings;
pub use store::{Origin, Store};
pub use worker::{ContactPaths, ContactSnapshot, ContactState, ContactWorker};
