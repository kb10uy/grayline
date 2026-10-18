use std::fmt::Write;

use jiff::fmt::strtime;

use crate::{
    VariableError,
    value::{VariableValue, Variables},
};

/// How a timestamp is written when an expression names no format.
pub const DEFAULT_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M";

/// Replaces every `${name}` in `source` with what `variables` says it is.
///
/// `$$` is a literal dollar sign, and a dollar sign that does not open an
/// expression is one too. A name nothing was provided for is a failure rather
/// than a gap: text that silently lost part of itself is worse than text that
/// does not render, and the caller knows which names it offered.
pub fn interpolate(source: &str, variables: &Variables) -> Result<String, VariableError> {
    let mut output = String::with_capacity(source.len());
    for piece in Pieces::new(source) {
        match piece? {
            Piece::Text(text) => output.push_str(text),
            Piece::Expression(expression) => {
                let reference = Reference::parse(expression)?;
                let value = variables
                    .get(reference.name)
                    .ok_or_else(|| VariableError::Missing(reference.name.to_owned()))?;
                reference.write(&mut output, value)?;
            }
        }
    }
    Ok(output)
}

/// The variable names one expression reads, in the order they appear.
///
/// Malformed expressions are passed over rather than reported: this walk
/// answers what a piece of text would read, and [`interpolate`] is what
/// refuses to render text it cannot resolve.
pub fn references(source: &str) -> References<'_> {
    References {
        pieces: Pieces::new(source),
    }
}

/// The names [`references`] walks.
pub struct References<'a> {
    pieces: Pieces<'a>,
}

impl<'a> Iterator for References<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.pieces.next()? {
                Ok(Piece::Expression(expression)) => return Some(Reference::split(expression).0),
                Ok(Piece::Text(_)) => {}
                Err(_) => return None,
            }
        }
    }
}

/// One stretch of a source, as the `${...}` grammar divides it.
enum Piece<'a> {
    /// Text that stands for itself, a `$$` or a lone `$` already reduced to
    /// the dollar sign it means.
    Text(&'a str),
    /// What sits between `${` and `}`.
    Expression(&'a str),
}

/// Walks a source's pieces, ending after the first unterminated expression.
struct Pieces<'a> {
    rest: &'a str,
}

impl<'a> Pieces<'a> {
    const fn new(source: &'a str) -> Self {
        Self { rest: source }
    }
}

impl<'a> Iterator for Pieces<'a> {
    type Item = Result<Piece<'a>, VariableError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let dollar = self.rest.find('$').unwrap_or(self.rest.len());
        if dollar > 0 {
            let (text, rest) = self.rest.split_at(dollar);
            self.rest = rest;
            return Some(Ok(Piece::Text(text)));
        }
        let (sign, after) = self.rest.split_at(1);
        if let Some(after_escape) = after.strip_prefix('$') {
            self.rest = after_escape;
            return Some(Ok(Piece::Text(sign)));
        }
        let Some(expression) = after.strip_prefix('{') else {
            self.rest = after;
            return Some(Ok(Piece::Text(sign)));
        };
        let Some(end) = expression.find('}') else {
            self.rest = "";
            return Some(Err(VariableError::Unterminated));
        };
        self.rest = &expression[end + 1..];
        Some(Ok(Piece::Expression(&expression[..end])))
    }
}

struct Reference<'a> {
    name: &'a str,
    format: Option<&'a str>,
}

impl<'a> Reference<'a> {
    fn parse(expression: &'a str) -> Result<Self, VariableError> {
        let (name, format) = Self::split(expression);
        if !valid_variable_name(name) {
            return Err(VariableError::InvalidName(name.to_owned()));
        }
        if format.is_some_and(str::is_empty) {
            return Err(VariableError::Format {
                name: name.to_owned(),
                message: "the format is empty".into(),
            });
        }
        Ok(Self { name, format })
    }

    /// Separates the name from the format without judging either.
    ///
    /// The first colon ends the name, and everything after it is the format,
    /// so a format containing colons such as `%H:%M` needs no escaping.
    fn split(expression: &'a str) -> (&'a str, Option<&'a str>) {
        match expression.split_once(':') {
            Some((name, format)) => (name, Some(format)),
            None => (expression, None),
        }
    }

    fn write(&self, output: &mut String, value: &VariableValue) -> Result<(), VariableError> {
        match (value, self.format) {
            (VariableValue::Timestamp(zoned), format) => {
                let format = format.unwrap_or(DEFAULT_TIMESTAMP_FORMAT);
                let formatted = strtime::format(format, zoned).map_err(|error| VariableError::Format {
                    name: self.name.to_owned(),
                    message: error.to_string(),
                })?;
                output.push_str(&formatted);
            }
            (_, Some(_)) => {
                return Err(VariableError::Format {
                    name: self.name.to_owned(),
                    message: "only a timestamp takes a format".into(),
                });
            }
            (value, None) => write!(output, "{value}").unwrap(),
        }
        Ok(())
    }
}

/// Whether `name` can be referenced by a `${...}` text expression.
///
/// Exported because a caller that lets an operator name a variable has to
/// refuse the names no template could ever read.
pub fn valid_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
                && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use rstest::rstest;

    use super::*;

    fn timestamp() -> VariableValue {
        VariableValue::Timestamp(
            date(2026, 8, 4)
                .at(9, 5, 0, 0)
                .in_tz("UTC")
                .expect("UTC is a known time zone"),
        )
    }

    #[test]
    fn interpolates_values_and_escaped_dollars() {
        let mut variables = Variables::new();
        variables.insert("contact.callsign", VariableValue::Text("JA1ABC".into()));
        assert_eq!(
            interpolate("To ${contact.callsign}; $${literal}", &variables).unwrap(),
            "To JA1ABC; ${literal}"
        );
        assert!(matches!(
            interpolate("${station.callsign}", &variables),
            Err(VariableError::Missing(_))
        ));
    }

    #[test]
    fn formats_a_timestamp_through_the_expression() {
        let mut variables = Variables::new();
        variables.insert("tx.timestamp.utc", timestamp());
        assert_eq!(
            interpolate("${tx.timestamp.utc:%d %b %Y %H:%MZ}", &variables).unwrap(),
            "04 Aug 2026 09:05Z"
        );
    }

    #[test]
    fn writes_a_timestamp_without_a_format() {
        let mut variables = Variables::new();
        variables.insert("tx.timestamp.utc", timestamp());
        assert_eq!(
            interpolate("${tx.timestamp.utc}", &variables).unwrap(),
            "2026-08-04 09:05"
        );
    }

    #[test]
    fn rejects_an_unusable_format() {
        let mut variables = Variables::new();
        variables.insert("tx.timestamp.utc", timestamp());
        variables.insert("station.callsign", VariableValue::Text("JA1ABC".into()));
        assert!(matches!(
            interpolate("${tx.timestamp.utc:%J}", &variables),
            Err(VariableError::Format { .. })
        ));
        assert!(matches!(
            interpolate("${station.callsign:%Y}", &variables),
            Err(VariableError::Format { .. })
        ));
        assert!(matches!(
            interpolate("${tx.timestamp.utc:}", &variables),
            Err(VariableError::Format { .. })
        ));
    }

    #[rstest]
    #[case("plain", "plain")]
    #[case("a$", "a$")]
    #[case("$$", "$")]
    #[case("x$y", "x$y")]
    #[case("$${a}$", "${a}$")]
    #[case("[${a}]", "[A]")]
    fn dollar_signs_that_open_nothing_are_kept(#[case] source: &str, #[case] expected: &str) {
        let mut variables = Variables::new();
        variables.insert("a", VariableValue::Text("A".into()));
        assert_eq!(interpolate(source, &variables).unwrap(), expected);
    }

    #[test]
    fn an_unterminated_expression_fails_and_ends_the_walk() {
        assert!(matches!(
            interpolate("${a", &Variables::new()),
            Err(VariableError::Unterminated)
        ));
        let names: Vec<_> = references("${a} $${b} ${c").collect();
        assert_eq!(names, ["a"]);
    }

    #[test]
    fn lists_the_names_a_text_reads() {
        let names: Vec<_> = references("${a} $${b} plain ${c.d:%H:%M} $x").collect();
        assert_eq!(names, ["a", "c.d"]);
    }
}
