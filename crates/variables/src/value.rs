use std::{collections::BTreeMap, fmt};

use jiff::Zoned;

use crate::interpolate::DEFAULT_TIMESTAMP_FORMAT;

/// A typed value available to text interpolation.
#[derive(Clone, Debug, PartialEq)]
pub enum VariableValue {
    /// Text copied without additional formatting.
    Text(String),
    /// A signed decimal integer.
    Integer(i64),
    /// A finite decimal number.
    Decimal(f64),
    /// A boolean rendered as `true` or `false`.
    Boolean(bool),
    /// An instant in a named time zone, rendered through a format expression.
    Timestamp(Zoned),
}

impl fmt::Display for VariableValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => formatter.write_str(value),
            Self::Integer(value) => value.fmt(formatter),
            Self::Decimal(value) => value.fmt(formatter),
            Self::Boolean(value) => value.fmt(formatter),
            Self::Timestamp(value) => value.strftime(DEFAULT_TIMESTAMP_FORMAT).fmt(formatter),
        }
    }
}

/// Named values supplied for text interpolation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variables {
    values: BTreeMap<String, VariableValue>,
}

impl Variables {
    /// Creates an empty variable collection.
    pub const fn new() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    /// Inserts or replaces a named value.
    pub fn insert(&mut self, name: impl Into<String>, value: VariableValue) -> Option<VariableValue> {
        self.values.insert(name.into(), value)
    }

    /// Returns a named value.
    pub fn get(&self, name: &str) -> Option<&VariableValue> {
        self.values.get(name)
    }
}
