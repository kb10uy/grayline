use toml_edit::{Array, Item, Table};

use crate::record::DEFAULT_FIELDS;

/// What the contact dialog offers.
///
/// Written under `[qso]` by every application that keeps a contact directory:
/// one operator's applications file the same fields about the same stations,
/// and a station set up once should not have to be set up again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContactSettings {
    /// Which fields the contact dialog offers, as written: keys, and groups
    /// spelled `!name`.
    ///
    /// Kept as written rather than expanded, so the file the operator reads
    /// back says `!ja` where they wrote `!ja`. What the dialog lays out is
    /// [`expand_fields`](crate::expand_fields) of this.
    pub fields: Vec<String>,
}

impl Default for ContactSettings {
    fn default() -> Self {
        Self::offering(&DEFAULT_FIELDS)
    }
}

impl ContactSettings {
    /// The settings a first run starts on, offering `fields`.
    pub fn offering(fields: &[&str]) -> Self {
        Self {
            fields: fields.iter().map(|field| (*field).to_owned()).collect(),
        }
    }

    /// Reads a `[qso]` section over `defaults`.
    ///
    /// A field list that is present is taken as written, an empty one
    /// included: an operator who wrote `fields = []` asked for a dialog holding
    /// only what the station already has, and handing the default back would
    /// be arguing with them. Entries that are not strings are dropped.
    pub fn read(section: Option<&Table>, defaults: Self) -> Self {
        let Some(section) = section else {
            return defaults;
        };
        Self {
            fields: section
                .get("fields")
                .and_then(Item::as_array)
                .map_or(defaults.fields, |fields| {
                    fields
                        .iter()
                        .filter_map(|entry| entry.as_str().map(str::trim).map(str::to_owned))
                        .collect()
                }),
        }
    }

    /// Writes these into a `[qso]` section.
    ///
    /// Keys this does not own, such as an application's serial number, are
    /// left where they are. The Wavelog lookup's switch and address, which
    /// earlier builds wrote here, are taken out: nothing reads them, and a
    /// file that still offered them would be offering a switch that does
    /// nothing.
    pub fn write(&self, section: &mut Table) {
        section.remove("lookup");
        section.remove("wavelog");
        let mut fields = Array::new();
        fields.extend(self.fields.iter().map(String::as_str));
        section["fields"] = Item::Value(fields.into());
    }
}

#[cfg(test)]
mod tests {
    use toml_edit::DocumentMut;

    use super::*;

    fn read(text: &str) -> ContactSettings {
        let document: DocumentMut = text.parse().unwrap();
        ContactSettings::read(document.get("qso").and_then(Item::as_table), ContactSettings::default())
    }

    #[test]
    fn a_missing_section_reads_as_the_defaults() {
        assert_eq!(read(""), ContactSettings::default());
    }

    #[test]
    fn a_written_section_reads_back_beside_keys_it_does_not_own() {
        let wanted = ContactSettings {
            fields: vec!["name".to_owned(), "!latin".to_owned()],
        };
        let mut document: DocumentMut = "[qso]\nnumber = \"042\"\n".parse().unwrap();
        wanted.write(document["qso"].as_table_mut().unwrap());

        let written = document.to_string();
        assert!(written.contains("number = \"042\""), "{written}");
        assert_eq!(read(&written), wanted);
    }

    #[test]
    fn the_lookup_an_earlier_build_wrote_is_taken_out() {
        let mut document: DocumentMut = "[qso]\nlookup = true\n\n[qso.wavelog]\nurl = \"https://log.example\"\n"
            .parse()
            .unwrap();
        ContactSettings::default().write(document["qso"].as_table_mut().unwrap());

        let written = document.to_string();
        assert!(!written.contains("lookup"), "{written}");
        assert!(!written.contains("wavelog"), "{written}");
    }

    #[test]
    fn an_empty_field_list_is_taken_as_written() {
        assert!(read("[qso]\nfields = []\n").fields.is_empty());
    }

    #[test]
    fn a_field_list_drops_what_is_not_a_string_and_trims_the_rest() {
        assert_eq!(read("[qso]\nfields = [\" name \", 3]\n").fields, ["name"]);
    }
}
