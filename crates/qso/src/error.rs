use std::path::PathBuf;

use thiserror::Error;

/// A failure reported by the contact directory.
#[derive(Debug, Error)]
pub enum QsoError {
    /// The contact worker could not start or its snapshot lock was poisoned.
    #[error("the contact directory state is unavailable")]
    WorkerUnavailable,
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
