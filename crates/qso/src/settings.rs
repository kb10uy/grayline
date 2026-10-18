use toml_edit::{Array, Item, Table, value};

use crate::record::DEFAULT_FIELDS;

/// Where the contact directory looks a callsign up.
///
/// Written under `[qso]` by every application that keeps a contact directory:
/// one operator's applications ask the same logger about the same stations,
/// and a station set up once should not have to be set up again.
///
/// The API key is deliberately absent, and is read from the family's own
/// credentials file instead. The settings file is the one the operator is
/// invited to open from the menu and the one an application rewrites at the
/// end of any frame that changed a setting; a credential belongs in neither.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContactSettings {
    /// Whether the operator's own logger is asked about a callsign the store
    /// has nothing recent to say about.
    pub lookup: bool,
    /// The base URL of that logger.
    pub wavelog_url: String,
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
    ///
    /// The lookup starts on: with no instance and no key there is nobody to
    /// ask and the store answers alone, so a switch that started off would
    /// only leave an operator who wrote a credentials file wondering why
    /// nothing happened.
    pub fn offering(fields: &[&str]) -> Self {
        Self {
            lookup: true,
            wavelog_url: String::new(),
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
            lookup: section.get("lookup").and_then(Item::as_bool).unwrap_or(defaults.lookup),
            wavelog_url: section
                .get("wavelog")
                .and_then(Item::as_table)
                .and_then(|wavelog| wavelog.get("url"))
                .and_then(Item::as_str)
                .map_or(defaults.wavelog_url, |url| url.trim().to_owned()),
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

    /// Writes these into a `[qso]` section, the instance's address included.
    ///
    /// The address is written even while it is empty: the file is where it is
    /// edited, and an operator who has to invent the key before they can
    /// change it has no way to learn it exists. Keys this does not own, such
    /// as an application's serial number, are left where they are.
    pub fn write(&self, section: &mut Table) {
        section["lookup"] = value(self.lookup);
        let mut fields = Array::new();
        fields.extend(self.fields.iter().map(String::as_str));
        section["fields"] = Item::Value(fields.into());
        let wavelog = section.entry("wavelog").or_insert(Item::Table(Table::new()));
        if !wavelog.is_table() {
            *wavelog = Item::Table(Table::new());
        }
        wavelog["url"] = value(self.wavelog_url.as_str());
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
            lookup: false,
            wavelog_url: "https://log.example".to_owned(),
            fields: vec!["name".to_owned(), "!latin".to_owned()],
        };
        let mut document: DocumentMut = "[qso]\nnumber = \"042\"\n".parse().unwrap();
        wanted.write(document["qso"].as_table_mut().unwrap());

        let written = document.to_string();
        assert!(written.contains("number = \"042\""), "{written}");
        assert_eq!(read(&written), wanted);
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
