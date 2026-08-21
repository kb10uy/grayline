use std::collections::BTreeMap;

/// The keys this crate fills in from an upstream source, and that an
/// application offers by name.
///
/// A record is not limited to these: whatever the operator files under a
/// callsign keeps its own name and is read back under it. This is the set that
/// an import and a lookup normalize onto, so that the same fact arrives under
/// the same name whichever source it came from.
pub const WELL_KNOWN_KEYS: [&str; 14] = [
    "name",
    "qth",
    "grid",
    "dxcc",
    "dxcc_id",
    "cq_zone",
    "itu_zone",
    "continent",
    "state",
    "county",
    "iota",
    "qsl_manager",
    "email",
    "note",
];

/// The key a record refuses, because the callsign is what a record is filed
/// under rather than something filed in one.
const RESERVED_KEY: &str = "callsign";

/// How short and how long a callsign may be to be taken for one.
const CALLSIGN_LENGTH: core::ops::RangeInclusive<usize> = 3..=16;

/// What is known about one station, as a table from key to value.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Record {
    callsign: String,
    fields: BTreeMap<String, String>,
}

impl Record {
    /// Creates an empty record for `callsign`, or nothing if that is not one.
    pub fn new(callsign: &str) -> Option<Self> {
        Some(Self {
            callsign: normalize_callsign(callsign)?,
            fields: BTreeMap::new(),
        })
    }

    /// The callsign this record is filed under, normalized.
    pub fn callsign(&self) -> &str {
        &self.callsign
    }

    /// The value filed under `key`, if there is one.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// Files `value` under `key`, and reports whether it was taken.
    ///
    /// A key no template could read is refused rather than stored, because a
    /// value nothing can print is one the operator would keep looking for. An
    /// empty value is refused for a different reason: it would read the same as
    /// an absent one and would occupy the place a real value should later fill.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let value = value.trim();
        if !valid_key(key) || key == RESERVED_KEY || value.is_empty() {
            return false;
        }
        self.fields.insert(key.to_owned(), value.to_owned());
        true
    }

    /// Drops `key` from this record, answering what it held.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.fields.remove(key)
    }

    /// Whether nothing at all is filed under this callsign.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// How many fields are filed under this callsign.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Every field, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// Whether `key` names something a template could read.
///
/// A key is read as `${contact.<key>}`, so it has to be one segment of a
/// template variable name: a key holding a hyphen or a dot would be filed
/// perfectly well and then be unreachable from the only place it is wanted.
/// The rule is the one `grayline_sstv_template::valid_variable_name` applies to
/// each segment, restated here rather than depended on so that this crate stays
/// clear of the rendering stack.
pub fn valid_key(key: &str) -> bool {
    let mut characters = key.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// Reads `text` as a callsign, or answers nothing when it is not one.
///
/// Answering nothing rather than uppercasing whatever arrived is what keeps a
/// half-typed field and a garbled identifier from becoming a request to
/// somebody's logger. The test is deliberately loose — every callsign carries a
/// digit and is built from letters, digits and the portable slash, and anything
/// stricter would start refusing real ones.
pub fn normalize_callsign(text: &str) -> Option<String> {
    let callsign = text.trim().to_ascii_uppercase();
    let usable = CALLSIGN_LENGTH.contains(&callsign.len())
        && callsign.chars().all(|c| c.is_ascii_alphanumeric() || c == '/')
        && callsign.chars().any(|c| c.is_ascii_digit())
        && callsign.chars().any(|c| c.is_ascii_alphabetic());
    usable.then_some(callsign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("name")]
    #[case("_private")]
    #[case("cq_zone")]
    #[case("x1")]
    fn a_key_of_one_identifier_segment_is_usable(#[case] key: &str) {
        assert!(valid_key(key));
    }

    #[rstest]
    #[case("")]
    #[case("cq-zone")]
    #[case("contact.name")]
    #[case("1st")]
    #[case("naïve")]
    #[case("has space")]
    fn a_key_no_template_could_read_is_refused(#[case] key: &str) {
        assert!(!valid_key(key));
    }

    #[test]
    fn every_well_known_key_is_usable() {
        for key in WELL_KNOWN_KEYS {
            assert!(valid_key(key), "{key}");
        }
    }

    #[rstest]
    #[case("ja1abc", "JA1ABC")]
    #[case("  JA1ABC  ", "JA1ABC")]
    #[case("ja1abc/p", "JA1ABC/P")]
    #[case("dl/ja1abc", "DL/JA1ABC")]
    fn a_callsign_is_taken_up_uppercased_and_trimmed(#[case] typed: &str, #[case] expected: &str) {
        assert_eq!(normalize_callsign(typed).as_deref(), Some(expected));
    }

    #[rstest]
    #[case("")]
    #[case("JA")]
    #[case("ABCDEF")]
    #[case("JA1ABC!")]
    #[case("1234")]
    #[case("JA1ABCDEFGHIJKLMNOP")]
    fn text_that_is_not_a_callsign_is_refused(#[case] typed: &str) {
        assert_eq!(normalize_callsign(typed), None);
    }

    #[test]
    fn a_record_is_filed_under_the_normalized_callsign() {
        let record = Record::new(" ja1abc ").expect("that is a callsign");
        assert_eq!(record.callsign(), "JA1ABC");
        assert!(record.is_empty());
    }

    #[test]
    fn a_record_refuses_a_callsign_that_is_not_one() {
        assert_eq!(Record::new("??"), None);
    }

    #[test]
    fn a_field_is_taken_up_trimmed() {
        let mut record = Record::new("JA1ABC").expect("that is a callsign");
        assert!(record.set("name", "  Taro  "));
        assert_eq!(record.get("name"), Some("Taro"));
    }

    #[rstest]
    #[case("cq-zone", "14")]
    #[case("callsign", "JX1ZZZ")]
    #[case("name", "   ")]
    fn a_field_nothing_could_use_is_refused(#[case] key: &str, #[case] value: &str) {
        let mut record = Record::new("JA1ABC").expect("that is a callsign");
        assert!(!record.set(key, value));
        assert!(record.is_empty());
    }

    #[test]
    fn fields_are_read_back_in_key_order() {
        let mut record = Record::new("JA1ABC").expect("that is a callsign");
        record.set("qth", "Tokyo");
        record.set("name", "Taro");
        let read: Vec<_> = record.iter().collect();
        assert_eq!(read, [("name", "Taro"), ("qth", "Tokyo")]);
    }
}
