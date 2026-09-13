//! The two lists of messages, each in a file of its own beside the settings.
//!
//! Out of `config.toml` because they are not settings: nothing in the window
//! changes them, they are the longest thing in the file by far, and the file
//! is opened from the menu to be written in rather than to be read back. A
//! file of their own also means the application only ever creates them — once,
//! with what ships — and never rewrites what the operator wrote there.

use std::{fs, io, path::Path};

use grayline_shell::log;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::app::macros::{Macro, Template, default_macros, default_templates};

const MACROS_KEY: &str = "macros";
const TEMPLATES_KEY: &str = "templates";

/// Reads the buttons under the message field.
pub fn load_macros(path: &Path) -> Vec<Macro> {
    load(path, MACROS_KEY, default_macros(), read_macro, write_macro)
}

/// Reads the set messages listed beside the buttons.
pub fn load_templates(path: &Path) -> Vec<Template> {
    load(path, TEMPLATES_KEY, default_templates(), read_template, write_template)
}

/// Reads one file, creating it from `shipped` when there is none.
///
/// A file that is there but holds no list at all is an operator who emptied
/// it, and they get the empty list they asked for. A file that could not be
/// read is not: they get what ships, and nothing is written over whatever is
/// in the file.
fn load<T>(
    path: &Path,
    key: &str,
    shipped: Vec<T>,
    read: fn(&Table) -> Option<T>,
    write: fn(&T, &mut Table),
) -> Vec<T> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create(path, key, &shipped, write);
            return shipped;
        }
        Err(error) => {
            log::note(&format!("could not read {}: {error}", path.display()));
            return shipped;
        }
    };
    match text.parse::<DocumentMut>() {
        Ok(document) => document
            .as_table()
            .get(key)
            .and_then(Item::as_array_of_tables)
            .map(|entries| entries.iter().filter_map(read).collect())
            .unwrap_or_default(),
        Err(error) => {
            log::note(&format!("could not read {}: {error}", path.display()));
            shipped
        }
    }
}

fn create<T>(path: &Path, key: &str, entries: &[T], write: fn(&T, &mut Table)) {
    let mut document = DocumentMut::new();
    let mut array = ArrayOfTables::new();
    for entry in entries {
        let mut table = Table::new();
        write(entry, &mut table);
        array.push(table);
    }
    document[key] = Item::ArrayOfTables(array);
    if let Err(error) = fs::write(path, document.to_string()) {
        log::note(&format!("could not save {}: {error}", path.display()));
    }
}

/// Reads one macro, skipping an entry with nothing to press or to send.
///
/// A button with no text behind it would do nothing, and one with no label
/// would be a button nobody could tell apart from the next; either is a
/// half-written entry rather than a reason to start on no macros at all.
fn read_macro(entry: &Table) -> Option<Macro> {
    let label = field(entry, "label")?;
    let text = field(entry, "text")?;
    if label.is_empty() || text.is_empty() {
        return None;
    }
    Some(Macro {
        label: label.to_owned(),
        text: text.to_owned(),
        send: entry
            .get("send")
            .and_then(Item::as_value)
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    })
}

fn write_macro(shipped: &Macro, entry: &mut Table) {
    entry["label"] = value(shipped.label.as_str());
    entry["text"] = value(shipped.text.as_str());
    entry["send"] = value(shipped.send);
}

/// Reads one set message, skipping an entry that could not be picked or sent.
fn read_template(entry: &Table) -> Option<Template> {
    let name = field(entry, "name")?;
    let text = field(entry, "text")?;
    if name.is_empty() || text.is_empty() {
        return None;
    }
    Some(Template {
        name: name.to_owned(),
        text: text.to_owned(),
    })
}

fn write_template(shipped: &Template, entry: &mut Table) {
    entry["name"] = value(shipped.name.as_str());
    entry["text"] = value(shipped.text.as_str());
}

fn field<'a>(entry: &'a Table, key: &str) -> Option<&'a str> {
    entry.get(key).and_then(Item::as_value).and_then(|value| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_missing_file_is_written_with_what_ships() {
        let root = TempDir::new();
        let path = root.path().join("macros.toml");

        assert_eq!(load_macros(&path), default_macros());
        assert_eq!(load_macros(&path), default_macros());
        assert!(fs::read_to_string(&path).unwrap().contains("[[macros]]"));
    }

    #[test]
    fn the_set_messages_survive_being_written_and_read_back() {
        let root = TempDir::new();
        let path = root.path().join("templates.toml");

        load_templates(&path);

        assert_eq!(load_templates(&path), default_templates());
    }

    #[test]
    fn the_file_is_what_the_buttons_are_read_from() {
        let root = TempDir::new();
        let path = root.path().join("macros.toml");
        fs::write(
            &path,
            "[[macros]]
label = \"TEST\"
text = \"RY RY\"
send = true
",
        )
        .unwrap();

        assert_eq!(
            load_macros(&path),
            [Macro {
                label: "TEST".to_owned(),
                text: "RY RY".to_owned(),
                send: true,
            }]
        );
    }

    #[test]
    fn a_file_with_no_list_in_it_is_an_empty_list() {
        let root = TempDir::new();
        let path = root.path().join("macros.toml");
        fs::write(&path, "# nothing here\n").unwrap();

        assert!(load_macros(&path).is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_written_over() {
        let root = TempDir::new();
        let path = root.path().join("templates.toml");
        fs::write(&path, "[[templates]\nname = \"broken\"\n").unwrap();

        assert_eq!(load_templates(&path), default_templates());
        assert!(fs::read_to_string(&path).unwrap().starts_with("[[templates]"));
    }

    #[test]
    fn an_entry_missing_a_half_is_dropped() {
        let root = TempDir::new();
        let path = root.path().join("templates.toml");
        fs::write(
            &path,
            "[[templates]]\nname = \"AGN\"\ntext = \"PSE AGN\"\n\n[[templates]]\nname = \"\"\ntext = \"NO\"\n\n[[templates]]\nname = \"NOTEXT\"\n",
        )
        .unwrap();

        let read = load_templates(&path);

        assert_eq!(read.len(), 1);
        assert_eq!(read[0].name, "AGN");
    }
}
