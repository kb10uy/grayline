use std::{fs, io, path::PathBuf};

use toml_edit::{DocumentMut, Table};

use crate::log;

/// A settings file, kept as the document it was read from.
///
/// Saving assigns the keys an application owns rather than rewriting the
/// document, so a comment the operator left beside one survives, and so does
/// whatever else is in the file.
#[derive(Debug)]
pub struct Document {
    path: Option<PathBuf>,
    document: DocumentMut,
    /// Set when the file could not be read or parsed.
    ///
    /// The application starts on defaults and refuses to save, because
    /// overwriting a file it could not read would throw away whatever the
    /// operator had written in it.
    read_only: bool,
    error: Option<String>,
}

impl Document {
    /// Reads `path`, falling back to an empty document.
    ///
    /// A missing file is an empty one. Neither a file that cannot be read nor
    /// one that does not parse is fatal: the application starts on defaults
    /// and reports why, rather than refusing to run because one hand-edited
    /// value is wrong.
    pub fn load(path: PathBuf) -> Self {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Self::unreadable(path, &error.to_string()),
        };
        match text.parse::<DocumentMut>() {
            Ok(document) => Self {
                path: Some(path),
                document,
                read_only: false,
                error: None,
            },
            Err(error) => Self::unreadable(path, &error.to_string()),
        }
    }

    /// A document with nowhere to write, for tests and headless runs.
    pub fn detached() -> Self {
        Self {
            path: None,
            document: DocumentMut::new(),
            read_only: false,
            error: None,
        }
    }

    fn unreadable(path: PathBuf, error: &str) -> Self {
        log::note(&format!("could not read {}: {error}", path.display()));
        Self {
            path: Some(path),
            document: DocumentMut::new(),
            read_only: true,
            error: Some(error.to_owned()),
        }
    }

    /// What went wrong reading or last writing the file, if anything did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The document's top level, to read settings out of.
    pub fn table(&self) -> &Table {
        self.document.as_table()
    }

    /// Applies `edit` to the document and writes the file.
    ///
    /// Nothing happens to a detached document or to one that could not be
    /// read. The directory the file sits in is made if it is not there yet. A
    /// failed write is logged and kept as the [`error`](Self::error), and the
    /// next save tries again.
    pub fn save(&mut self, edit: impl FnOnce(&mut Table)) {
        let Some(path) = self.path.as_ref() else { return };
        if self.read_only {
            return;
        }
        edit(self.document.as_table_mut());
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(path, self.document.to_string()));
        self.error = match written {
            Ok(()) => None,
            Err(error) => {
                log::note(&format!("could not save {}: {error}", path.display()));
                Some(error.to_string())
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use toml_edit::value;

    use super::*;

    #[test]
    fn a_missing_file_is_an_empty_document_that_can_be_saved() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("not-there-yet").join("config.toml");
        let mut document = Document::load(path.clone());
        assert!(document.table().is_empty());
        assert!(document.error().is_none());

        document.save(|table| table["key"] = value(true));

        assert_eq!(fs::read_to_string(path).unwrap(), "key = true\n");
    }

    #[test]
    fn a_save_keeps_comments_beside_keys_it_did_not_touch() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "# kept\nother = 1\n").unwrap();

        Document::load(path.clone()).save(|table| table["key"] = value(2));

        let written = fs::read_to_string(path).unwrap();
        assert!(written.contains("# kept\nother = 1\n"), "{written}");
        assert!(written.contains("key = 2"), "{written}");
    }

    #[test]
    fn an_unparsable_file_is_reported_and_never_saved_over() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "this is not toml = = =\n").unwrap();

        let mut document = Document::load(path.clone());
        document.save(|table| table["key"] = value(true));

        assert!(document.error().is_some());
        assert_eq!(fs::read_to_string(path).unwrap(), "this is not toml = = =\n");
    }

    #[test]
    fn a_detached_document_writes_nothing() {
        let mut document = Document::detached();
        document.save(|table| table["key"] = value(true));
        assert!(document.table().is_empty());
    }
}
