use std::path::PathBuf;

use thiserror::Error;

/// A failure reported by the contact directory.
#[derive(Debug, Error)]
pub enum QsoError {
    /// The text handed in was not a callsign.
    #[error("`{0}` is not a callsign")]
    Callsign(String),
    /// The store file could not be opened or created.
    #[error("the contact store at {path} could not be opened: {detail}")]
    Open {
        /// The file that was tried.
        path: PathBuf,
        /// What SQLite or the platform said about the attempt.
        detail: String,
    },
    /// A statement against an open store failed.
    #[error("the contact store failed: {0}")]
    Store(String),
    /// The store was written by a build that knows more about it than this one.
    ///
    /// Refused rather than opened, because the columns this build cannot see
    /// are ones it would drop on the next write.
    #[error("the contact store is at version {found}, which this build ({known}) does not understand")]
    Version {
        /// The `user_version` the file carries.
        found: i64,
        /// The `user_version` this build writes.
        known: i64,
    },
    /// The configured base URL named no host a lookup could reach.
    #[error("`{0}` is not an address a lookup can reach")]
    Address(String),
    /// Nothing was listening, or the connection could not be established.
    ///
    /// The host is named and the rest of the address is not: the request body
    /// carries the API key, and an error that quoted the whole exchange would
    /// put it on the status line.
    #[error("the lookup could not reach {host}: {detail}")]
    Connect {
        /// The host that was tried.
        host: String,
        /// What the transport said about the attempt.
        detail: String,
    },
    /// The lookup reached the server and the server refused it.
    #[error("the lookup was refused with status {0}")]
    Refused(u16),
    /// The answer was not JSON, or was JSON this crate could not read.
    #[error("the lookup answered with nothing this crate could read")]
    Unreadable,
    /// A remote lookup was asked for with no API key configured.
    #[error("no API key is configured for the lookup")]
    NoKey,
    /// The credentials file could not be read or written.
    #[error("{path}: {detail}")]
    Credentials {
        /// The file, as the caller named it.
        path: String,
        /// What was wrong with it.
        detail: String,
    },
    /// The named character encoding is not one this build knows.
    #[error("`{0}` is not a character encoding this build knows")]
    Encoding(String),
    /// An ADIF document could not be read.
    #[error("{path}: {detail}")]
    Adif {
        /// The document, named as the caller named it.
        path: String,
        /// What was wrong with it.
        detail: String,
    },
}
