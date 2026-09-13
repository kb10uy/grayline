//! Named-value interpolation for Grayline text.
//!
//! One `${name}` syntax for the whole family. The SSTV templates were written
//! against it first and the RTTY macros then wanted the same thing, so it
//! lives here rather than in either: an operator who has written a template
//! has written a macro, and a name that means something in one means the same
//! in the other.
//!
//! What the names *are* is the caller's business. This crate holds the
//! expression syntax, the rule for what may be named, and the values a name
//! can stand for; which names exist belongs to whatever is being written.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod interpolate;
mod value;

pub use interpolate::{DEFAULT_TIMESTAMP_FORMAT, References, interpolate, references, valid_variable_name};
pub use value::{VariableValue, Variables};

use thiserror::Error;

/// A failure reading a `${...}` expression.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum VariableError {
    /// An expression was opened and never closed.
    #[error("unterminated variable interpolation")]
    Unterminated,
    /// A name no expression could hold.
    #[error("invalid variable name `{0}`")]
    InvalidName(String),
    /// A name nothing was provided for.
    #[error("variable `{0}` was not provided")]
    Missing(String),
    /// A format the referenced value cannot take.
    #[error("cannot format variable `{name}`: {message}")]
    Format {
        /// Referenced variable name.
        name: String,
        /// Why the format was rejected.
        message: String,
    },
}
