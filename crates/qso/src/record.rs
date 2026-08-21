use std::collections::BTreeMap;

/// The keys this crate names.
///
/// Naming a key is not the same as filling it in and not the same as showing
/// it: an import and a lookup normalize onto some of these so that the same
/// fact arrives under the same name whichever source it came from, while
/// others are only ever typed. Which of them an application puts a field on
/// screen for is a third question, and the operator's — see [`FIELD_GROUPS`].
///
/// A record is not limited to these. Whatever the operator files under a
/// callsign keeps its own name and is read back under it.
pub const WELL_KNOWN_KEYS: [&str; 17] = [
    "name",
    "name_latin",
    "qth",
    "qth_latin",
    "grid",
    "jcc",
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

/// The named sets of keys a configuration can ask for as `!name`.
///
/// A field list is written as keys and groups mixed together, so an operator
/// says `["!core", "!latin", "dxcc"]` rather than naming six keys and being
/// unable to say why they belong together.
///
/// Two of them are parts and one is a whole. `core` is what a contact is worth
/// looking up for at all. `latin` exists because ITA2 carries no kanji and a
/// station working RTTY needs somewhere to keep a name the mode can actually
/// send — a problem shared by every non-Latin script rather than a Japanese
/// one. `ja` is the set a station in Japan would otherwise assemble out of
/// those two and a JCC code, offered whole so that the common case is one
/// entry rather than three.
///
/// None of them is a country's whole convention, and the list is not where a
/// convention should end up. A station wanting anything else names the keys,
/// and the store files them without being told they exist.
pub const FIELD_GROUPS: [(&str, &[&str]); 3] = [
    ("core", &["name", "qth", "grid"]),
    ("latin", &["name_latin", "qth_latin"]),
    ("ja", &["name", "name_latin", "qth", "qth_latin", "grid", "jcc"]),
];

/// What a configuration that names no fields asks for.
///
/// The three a contact is worth looking up for at all. Everything else is
/// something a particular station has a use for rather than something every
/// station does, and a dialog that offered all seventeen would be one nobody
/// reads.
pub const DEFAULT_FIELDS: [&str; 1] = ["!core"];

/// The prefix that marks a group rather than a key.
pub const GROUP_PREFIX: char = '!';

/// The keys one group holds, or nothing when no group is named that.
pub fn field_group(name: &str) -> Option<&'static [&'static str]> {
    FIELD_GROUPS
        .iter()
        .find(|(group, _)| *group == name)
        .map(|(_, keys)| *keys)
}

/// Reads a configured field list into the keys it asks for.
///
/// Groups expand where they are written, so the order is the operator's own.
/// A key named twice, or named once in its own right and once inside a group,
/// appears once and where it first appeared. A group nobody defines and a key
/// no template could read are both passed over rather than reported: this
/// answers what to put on screen, and a dialog is a poor place to learn that a
/// configuration file has a typo in it.
pub fn expand_fields<'a>(spec: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    let mut push = |key: &str| {
        if valid_key(key) && key != RESERVED_KEY && !fields.iter().any(|held| held == key) {
            fields.push(key.to_owned());
        }
    };
    for entry in spec {
        let entry = entry.trim();
        match entry.strip_prefix(GROUP_PREFIX) {
            Some(group) => {
                for key in field_group(group).unwrap_or_default() {
                    push(key);
                }
            }
            None => push(entry),
        }
    }
    fields
}

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

    /// A group naming a key the crate does not is one an application would
    /// offer a field for and never be able to label.
    #[test]
    fn every_grouped_key_is_a_well_known_one() {
        for (group, keys) in FIELD_GROUPS {
            for key in keys {
                assert!(WELL_KNOWN_KEYS.contains(key), "`{key}` in `!{group}`");
            }
        }
    }

    #[test]
    fn the_default_field_list_expands_to_something() {
        assert_eq!(expand_fields(DEFAULT_FIELDS), ["name", "qth", "grid"]);
    }

    #[test]
    fn a_group_expands_where_it_is_written() {
        assert_eq!(
            expand_fields(["dxcc", "!latin", "email"]),
            ["dxcc", "name_latin", "qth_latin", "email"]
        );
    }

    /// `!ja` is offered so the common case is one entry rather than three, so
    /// it has to stay the same thing those three add up to.
    #[test]
    fn the_japanese_set_is_what_its_parts_come_to() {
        let assembled = expand_fields(["!core", "!latin", "jcc"]);
        let whole = expand_fields(["!ja"]);

        assert_eq!(
            assembled.iter().collect::<std::collections::BTreeSet<_>>(),
            whole.iter().collect::<std::collections::BTreeSet<_>>()
        );
    }

    /// The order is the operator's, so a key already placed stays where it was
    /// rather than moving to wherever it was named again.
    #[test]
    fn a_key_named_twice_appears_once_and_where_it_first_appeared() {
        assert_eq!(expand_fields(["name", "!core", "qth"]), ["name", "qth", "grid"]);
    }

    #[rstest]
    #[case("!nobody-defines-this")]
    #[case("cq-zone")]
    #[case("callsign")]
    #[case("")]
    fn an_entry_nothing_could_use_is_passed_over(#[case] entry: &str) {
        assert_eq!(expand_fields(["name", entry]), ["name"]);
    }

    #[test]
    fn a_group_is_named_without_its_prefix_when_it_is_asked_for_directly() {
        assert_eq!(field_group("latin"), Some(["name_latin", "qth_latin"].as_slice()));
        assert_eq!(field_group("!latin"), None);
        assert_eq!(field_group("nothing"), None);
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
