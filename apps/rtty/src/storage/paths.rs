use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use directories::BaseDirs;

use grayline_shell::platform::FAMILY_DIRECTORY;

use crate::identity::APP_DIRECTORY;

/// The settings file, named here because a test writes one directly.
pub const CONFIG_FILE: &str = "config.toml";

/// The buttons under the message field, and the list beside them.
///
/// Beside the settings rather than in them: they are written by hand and
/// never by the application, and one file each is what makes them findable
/// in the directory the File menu opens.
pub const MACRO_FILE: &str = "macros.toml";
pub const TEMPLATE_FILE: &str = "templates.toml";

/// The rolling log the application writes under its state directory.
const LOG_FILE: &str = "grayline-rtty.log";

const DEFAULT_CONFIG: &str = "";

/// One of the directories the application keeps for the operator.
///
/// Only the configuration for now: RTTY writes nothing else. The received
/// text log arrives with its own entry when it does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Folder {
    Config,
}

impl Folder {
    pub const ALL: [Self; 1] = [Self::Config];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Config => "menu-open-config",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    config_file: PathBuf,
    log_file: PathBuf,
}

impl AppPaths {
    pub fn discover() -> io::Result<Self> {
        let base_dirs = BaseDirs::new()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not determine the user directories"))?;

        // The log belongs to the machine it was written on rather than to the
        // account, for the reason recorded beside the SSTV application's own
        // copy of this: on Windows `data_dir` is the roaming profile.
        let state_dir = base_dirs.state_dir().unwrap_or_else(|| base_dirs.data_local_dir());

        Ok(Self::from_roots(
            base_dirs.config_dir().join(FAMILY_DIRECTORY).join(APP_DIRECTORY),
            state_dir.join(FAMILY_DIRECTORY).join(APP_DIRECTORY),
        ))
    }

    pub(crate) fn from_roots(config_dir: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            config_file: config_dir.join(CONFIG_FILE),
            log_file: state_dir.join("logs").join(LOG_FILE),
        }
    }

    pub fn initialize(&self) -> io::Result<()> {
        let config_dir = self.config_file.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the configuration file has no parent directory",
            )
        })?;
        let log_dir = self
            .log_file
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the log file has no parent directory"))?;

        for directory in [config_dir, log_dir] {
            fs::create_dir_all(directory)?;
        }
        create_default_config(&self.config_file)
    }

    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    pub fn config_dir(&self) -> &Path {
        self.config_file.parent().unwrap_or(&self.config_file)
    }

    pub fn macros_file(&self) -> PathBuf {
        self.config_dir().join(MACRO_FILE)
    }

    pub fn templates_file(&self) -> PathBuf {
        self.config_dir().join(TEMPLATE_FILE)
    }

    /// Returns the directory `folder` names.
    ///
    /// The configuration answers with the directory holding the file rather
    /// than the file itself: the application rewrites it as settings change,
    /// and a `.toml` has no dependable handler on every platform.
    pub fn folder(&self, folder: Folder) -> &Path {
        match folder {
            Folder::Config => self.config_dir(),
        }
    }

    pub fn log_file(&self) -> &Path {
        &self.log_file
    }
}

fn create_default_config(path: &Path) -> io::Result<()> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => io::Write::write_all(&mut file, DEFAULT_CONFIG.as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if path.is_file() {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("configuration path is not a file: {}", path.display()),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn initialize_creates_application_directories_and_default_config() {
        let root = TempDir::new();
        let paths = AppPaths::from_roots(root.path().join("config"), root.path().join("state"));

        paths.initialize().unwrap();

        assert_eq!(fs::read_to_string(&paths.config_file).unwrap(), DEFAULT_CONFIG);
        assert!(paths.log_file.parent().unwrap().is_dir());
    }

    /// The log describes one machine's hardware, so it must not be written
    /// where the account's roaming profile would synchronize it.
    #[test]
    fn the_log_is_kept_apart_from_the_roaming_data_directory() {
        let paths = AppPaths::from_roots(PathBuf::from("config"), PathBuf::from("state"));
        assert!(paths.log_file.starts_with("state"));
    }

    #[test]
    fn initialize_does_not_replace_existing_config() {
        let root = TempDir::new();
        let paths = AppPaths::from_roots(root.path().join("config"), root.path().join("state"));
        fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
        fs::write(&paths.config_file, "baud = 75.0\n").unwrap();

        paths.initialize().unwrap();

        assert_eq!(fs::read_to_string(&paths.config_file).unwrap(), "baud = 75.0\n");
    }
}
