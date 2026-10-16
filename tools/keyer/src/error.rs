use thiserror::Error;

/// What went wrong with the keyer's own hardware or socket.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum KeyerError {
    /// The serial port could not be opened or shared.
    #[error("serial port {port}: {detail}")]
    Port {
        /// The port as it was named.
        port: String,
        /// What the platform said.
        detail: String,
    },
    /// A control line refused to change.
    #[error("the {line} line could not be driven: {detail}")]
    Line {
        /// The line that was being driven.
        line: &'static str,
        /// What the platform said.
        detail: String,
    },
    /// The stream's framing was refused by the RTTY core.
    #[error("the stream cannot be framed: {0}")]
    Framing(String),
    /// The address to listen on could not be bound.
    #[error("cannot listen on {address}: {detail}")]
    Listen {
        /// The address as it was given.
        address: String,
        /// What the platform said.
        detail: String,
    },
}
