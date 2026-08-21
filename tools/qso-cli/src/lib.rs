//! What `gl-qso` does with a contact store, apart from parsing a command line.

use grayline_qso::{FIELD_GROUPS, GROUP_PREFIX, Record, WELL_KNOWN_KEYS, valid_key};

/// Reads one `key=value` argument.
///
/// The value may hold `=`, because a note or a QSL route can; only the first
/// one separates.
pub fn parse_assignment(text: &str) -> Result<(String, String), String> {
    let (key, value) = text
        .split_once('=')
        .ok_or_else(|| format!("`{text}` is not `key=value`"))?;
    let key = key.trim().to_ascii_lowercase();
    if !valid_key(&key) {
        return Err(format!(
            "`{key}` is not a key a template could read: one letter or underscore, then letters, digits and underscores"
        ));
    }
    Ok((key, value.trim().to_owned()))
}

/// Builds a record out of `key=value` arguments.
pub fn record_from_assignments(callsign: &str, assignments: &[String]) -> Result<Record, String> {
    let mut record = Record::new(callsign).ok_or_else(|| format!("`{callsign}` is not a callsign"))?;
    for assignment in assignments {
        let (key, value) = parse_assignment(assignment)?;
        if !record.set(&key, &value) {
            return Err(format!("`{key}` cannot be filed under a callsign"));
        }
    }
    Ok(record)
}

/// Writes a record as aligned `key  value` lines.
pub fn render_table(record: &Record) -> String {
    let width = record.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    record
        .iter()
        .map(|(key, value)| format!("{key:width$}  {value}\n"))
        .collect()
}

/// Writes a record as one JSON object, for a caller that is a script.
///
/// Written by hand rather than through a serializer: the document is flat, the
/// keys are already known to be identifiers, and only the values need escaping.
pub fn render_json(record: &Record) -> String {
    let fields: Vec<String> = record
        .iter()
        .map(|(key, value)| format!("\"{key}\":{}", quote(value)))
        .collect();
    format!("{{\"callsign\":{},{}}}", quote(record.callsign()), fields.join(","))
}

/// The keys this build names and the groups a configuration can ask for.
///
/// The groups are printed beside the keys because a field list is written out
/// of both, and an operator who only ever saw the keys would write six entries
/// where one would do.
pub fn render_keys() -> String {
    let mut rendered: String = WELL_KNOWN_KEYS.iter().map(|key| format!("{key}\n")).collect();
    rendered.push_str("\ngroups, for a `fields` list:\n");
    for (group, keys) in FIELD_GROUPS {
        rendered.push_str(&format!("{GROUP_PREFIX}{group}  {}\n", keys.join(", ")));
    }
    rendered
}

fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if control < ' ' => quoted.push_str(&format!("\\u{:04x}", control as u32)),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn an_assignment_splits_on_the_first_equals() {
        assert_eq!(
            parse_assignment("note=worked=twice").expect("an assignment"),
            ("note".to_owned(), "worked=twice".to_owned())
        );
    }

    #[test]
    fn an_assignment_takes_its_key_lowercased_and_trimmed() {
        assert_eq!(
            parse_assignment("  QTH = Tokyo ").expect("an assignment"),
            ("qth".to_owned(), "Tokyo".to_owned())
        );
    }

    #[rstest]
    #[case("qth")]
    #[case("cq-zone=14")]
    #[case("=Tokyo")]
    fn an_assignment_nothing_could_use_is_refused(#[case] text: &str) {
        assert!(parse_assignment(text).is_err());
    }

    #[test]
    fn a_record_is_built_out_of_assignments() {
        let record =
            record_from_assignments("ja1abc", &["name=Taro".to_owned(), "qth=Tokyo".to_owned()]).expect("a record");

        assert_eq!(record.callsign(), "JA1ABC");
        assert_eq!(record.get("name"), Some("Taro"));
    }

    #[test]
    fn the_callsign_cannot_be_filed_as_a_field_of_itself() {
        let error = record_from_assignments("JA1ABC", &["callsign=JH1XYZ".to_owned()]).expect_err("a refusal");

        assert!(error.contains("callsign"), "{error}");
    }

    #[test]
    fn a_table_lines_the_values_up() {
        let record =
            record_from_assignments("JA1ABC", &["name=Taro".to_owned(), "qth=Tokyo".to_owned()]).expect("a record");

        assert_eq!(render_table(&record), "name  Taro\nqth   Tokyo\n");
    }

    #[test]
    fn a_json_record_names_the_callsign_and_escapes_its_values() {
        let record = record_from_assignments("JA1ABC", &["note=said \"hi\"".to_owned()]).expect("a record");

        assert_eq!(render_json(&record), r#"{"callsign":"JA1ABC","note":"said \"hi\""}"#);
    }

    #[test]
    fn every_well_known_key_and_group_is_offered() {
        let rendered = render_keys();

        for key in WELL_KNOWN_KEYS {
            assert!(rendered.contains(key), "{key}");
        }
        for (group, _) in FIELD_GROUPS {
            assert!(rendered.contains(&format!("{GROUP_PREFIX}{group}")), "{group}");
        }
    }
}
