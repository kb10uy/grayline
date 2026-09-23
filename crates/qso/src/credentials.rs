use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

use toml_edit::{DocumentMut, value};

use crate::error::QsoError;

/// The environment variable that names the instance, for a station run from a
/// script or a container rather than from a file.
pub const URL_VARIABLE: &str = "GRAYLINE_WAVELOG_URL";

/// The environment variable that carries the key, for the same reason.
pub const KEY_VARIABLE: &str = "GRAYLINE_WAVELOG_KEY";

const SECTION: &str = "wavelog";

const TEMPLATE: &str = "\
# Credentials for the Grayline contact directory.
#
# This file is read and never written back, and it is deliberately not the
# application's settings file: that one is rewritten whenever a setting
# changes, and it is the one you are invited to open, copy and paste into a
# bug report. An API key belongs in neither of those.
#
# Both entries may be given in the environment instead, as
# GRAYLINE_WAVELOG_URL and GRAYLINE_WAVELOG_KEY, which take precedence.
#
# Either API is accepted, and the key says which: an API v2 token, which
# Wavelog issues under the wl2_ prefix and which needs the lookup:read scope,
# or a v1 API key, which needs read access. There is nothing else to set.

[wavelog]
url = \"\"
key = \"\"
";

/// What is needed to reach the operator's own logger.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Credentials {
    /// The base URL of the instance, when the file or the environment named
    /// one. An application that keeps the address in its own settings uses
    /// this only as a fallback.
    pub url: Option<String>,
    /// The API key.
    pub key: Option<String>,
}

impl Credentials {
    /// Reads `path`, or answers empty when there is no file there.
    ///
    /// A missing file is not a failure: it is what an operator who has not
    /// configured a logger has, and the directory works from the store alone.
    pub fn read(path: &Path) -> Result<Self, QsoError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(credentials_error(path, &error.to_string())),
        };
        let document: DocumentMut = text
            .parse()
            .map_err(|error| credentials_error(path, &format!("{error}")))?;
        let section = document.get(SECTION);
        Ok(Self {
            url: entry(section, "url"),
            key: entry(section, "key"),
        })
    }

    /// Takes whatever the environment says in preference to what was read.
    #[must_use]
    pub fn with_environment(self) -> Self {
        Self {
            url: from_environment(URL_VARIABLE).or(self.url),
            key: from_environment(KEY_VARIABLE).or(self.key),
        }
    }

    /// Writes a credentials file, refusing to replace one that is already
    /// there.
    ///
    /// Refusing rather than merging, for the same reason the SSTV application
    /// refuses to replace a rig script it did not write: what is there is the
    /// operator's, and a key overwritten is one they have to go and fetch
    /// again.
    pub fn write(path: &Path, url: Option<&str>, key: &str) -> Result<(), QsoError> {
        if let Some(directory) = path.parent()
            && !directory.as_os_str().is_empty()
        {
            fs::create_dir_all(directory).map_err(|error| credentials_error(path, &error.to_string()))?;
        }

        let mut document: DocumentMut = TEMPLATE.parse().expect("the template is valid TOML");
        document[SECTION]["key"] = value(key.trim());
        if let Some(url) = url {
            document[SECTION]["url"] = value(url.trim().trim_end_matches('/'));
        }

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .map_err(|error| credentials_error(path, &error.to_string()))?;
        file.write_all(document.to_string().as_bytes())
            .map_err(|error| credentials_error(path, &error.to_string()))
    }
}

fn entry(section: Option<&toml_edit::Item>, name: &str) -> Option<String> {
    let text = section?.get(name)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn from_environment(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn credentials_error(path: &Path, detail: &str) -> QsoError {
    QsoError::Credentials {
        path: path.display().to_string(),
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_file_that_is_not_there_reads_as_no_credentials_at_all() {
        let directory = TempDir::new();

        let credentials = Credentials::read(&directory.path().join("nothing.toml")).expect("a read");

        assert_eq!(credentials, Credentials::default());
    }

    #[test]
    fn what_the_file_says_is_read_back() {
        let directory = TempDir::new();
        let path = directory.path().join("credentials.toml");
        fs::write(
            &path,
            "[wavelog]\nurl = \"https://log.example.org\"\nkey = \"secret\"\n",
        )
        .expect("a write");

        let credentials = Credentials::read(&path).expect("a read");

        assert_eq!(credentials.url.as_deref(), Some("https://log.example.org"));
        assert_eq!(credentials.key.as_deref(), Some("secret"));
    }

    #[test]
    fn an_empty_entry_is_the_same_as_no_entry() {
        let directory = TempDir::new();
        let path = directory.path().join("credentials.toml");
        fs::write(&path, "[wavelog]\nurl = \"\"\nkey = \"   \"\n").expect("a write");

        assert_eq!(Credentials::read(&path).expect("a read"), Credentials::default());
    }

    #[test]
    fn a_written_file_reads_back_and_is_not_replaced() {
        let directory = TempDir::new();
        let path = directory.path().join("credentials.toml");

        Credentials::write(&path, Some("https://log.example.org/"), " secret ").expect("a write");
        let credentials = Credentials::read(&path).expect("a read");

        assert_eq!(credentials.url.as_deref(), Some("https://log.example.org"));
        assert_eq!(credentials.key.as_deref(), Some("secret"));
        assert!(Credentials::write(&path, None, "other").is_err());
        assert_eq!(Credentials::read(&path).expect("a read").key.as_deref(), Some("secret"));
    }

    #[test]
    fn a_written_file_keeps_the_note_explaining_itself() {
        let directory = TempDir::new();
        let path = directory.path().join("credentials.toml");

        Credentials::write(&path, None, "secret").expect("a write");

        let text = fs::read_to_string(&path).expect("a read");
        assert!(
            text.starts_with("# Credentials for the Grayline contact directory."),
            "{text}"
        );
        assert!(text.contains(KEY_VARIABLE));
    }
}
