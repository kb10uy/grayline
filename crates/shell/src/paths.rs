use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use directories::{BaseDirs, UserDirs};

use crate::{Identity, platform::FAMILY_DIRECTORY};

/// The settings file every application keeps in its configuration directory.
pub const CONFIG_FILE: &str = "config.toml";

/// The directories one application keeps its files under.
///
/// Each is already the application's own: the family directory and the
/// application's beneath it are joined on, apart from the pictures directory,
/// which the operator browses themselves and which is therefore flat and
/// spelled the way the application is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Roots {
    /// Where the settings live.
    pub config_dir: PathBuf,
    /// Where data the application reads but the operator rarely opens lives.
    pub data_dir: PathBuf,
    /// Where pictures the operator works with live.
    pub pictures_dir: PathBuf,
    /// Where what belongs to this machine rather than to the account lives.
    pub state_dir: PathBuf,
}

impl Roots {
    /// Finds the directories the platform keeps for `identity`.
    pub fn discover(identity: &Identity) -> io::Result<Self> {
        let base_dirs = BaseDirs::new()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not determine the user directories"))?;
        let pictures_dir = UserDirs::new()
            .and_then(|directories| directories.picture_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| base_dirs.home_dir().join("Pictures"));

        // The log belongs to the machine it was written on, not to the
        // account: on Windows `data_dir` is the roaming profile, which would
        // synchronize a log describing hardware the other machine does not
        // have. Linux keeps a directory for exactly this; elsewhere the local
        // half of the data directory is the closest equivalent.
        let state_dir = base_dirs.state_dir().unwrap_or_else(|| base_dirs.data_local_dir());

        // Every application in the family nests under one directory, so the
        // settings they share have somewhere to sit beside them and the whole
        // family can be backed up or removed as a unit.
        let own = |root: &Path| root.join(FAMILY_DIRECTORY).join(identity.app_directory);
        Ok(Self {
            config_dir: own(base_dirs.config_dir()),
            data_dir: own(base_dirs.data_dir()),
            pictures_dir: pictures_dir.join(identity.pictures_directory),
            state_dir: own(state_dir),
        })
    }

    /// Lays the four directories out side by side under `directory`, for a
    /// test or a headless interface that must not touch the operator's own.
    pub fn under(directory: &Path) -> Self {
        Self {
            config_dir: directory.join("config"),
            data_dir: directory.join("data"),
            pictures_dir: directory.join("pictures"),
            state_dir: directory.join("state"),
        }
    }

    /// The settings file.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE)
    }

    /// The rolling log, named after the executable so a log sent to someone
    /// else still says which application wrote it.
    pub fn log_file(&self, identity: &Identity) -> PathBuf {
        self.state_dir
            .join("logs")
            .join(format!("{}.log", identity.process_name))
    }
}

/// Creates what an application needs before it starts.
///
/// That is the directories holding `config_file` and `log_file`, each of
/// `directories`, and an empty `config_file` unless one is already there; an
/// existing one is never replaced.
pub fn initialize(config_file: &Path, log_file: &Path, directories: &[&Path]) -> io::Result<()> {
    let parent = |file: &Path, what: &str| {
        file.parent().map(Path::to_path_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the {what} has no parent directory"),
            )
        })
    };
    fs::create_dir_all(parent(config_file, "configuration file")?)?;
    fs::create_dir_all(parent(log_file, "log file")?)?;
    for directory in directories {
        fs::create_dir_all(directory)?;
    }
    create_default_config(config_file)
}

fn create_default_config(path: &Path) -> io::Result<()> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_) => Ok(()),
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
    use tempfile::TempDir;

    use super::*;

    const TEST_IDENTITY: Identity = Identity {
        app_directory: "test",
        display_name: "Grayline Test",
        version: "0.0.0",
        process_name: "grayline-test",
        pictures_directory: "Grayline Test",
        manual_url: "https://grayline.jl1his.radio/test/",
        app_user_model_id: "kb10uy.GraylineTest",
        icon_png: &[],
    };

    #[test]
    fn initialize_creates_the_directories_and_an_empty_config() {
        let root = TempDir::new().unwrap();
        let roots = Roots::under(root.path());
        let extra = roots.pictures_dir.join("Received");

        initialize(&roots.config_file(), &roots.log_file(&TEST_IDENTITY), &[&extra]).unwrap();

        assert_eq!(fs::read_to_string(roots.config_file()).unwrap(), "");
        assert!(extra.is_dir());
        assert!(
            roots.log_file(&TEST_IDENTITY).parent().unwrap().is_dir(),
            "the log has to have somewhere to be written before anything reports"
        );
    }

    #[test]
    fn initialize_does_not_replace_existing_config() {
        let root = TempDir::new().unwrap();
        let roots = Roots::under(root.path());
        fs::create_dir_all(&roots.config_dir).unwrap();
        fs::write(roots.config_file(), "language = \"ja\"\n").unwrap();

        initialize(&roots.config_file(), &roots.log_file(&TEST_IDENTITY), &[]).unwrap();

        assert_eq!(fs::read_to_string(roots.config_file()).unwrap(), "language = \"ja\"\n");
    }

    #[test]
    fn the_log_is_kept_apart_from_the_roaming_data_directory() {
        let roots = Roots::under(Path::new(""));
        let log = roots.log_file(&TEST_IDENTITY);

        assert!(log.starts_with("state"));
        assert!(!log.starts_with("data"));
        assert_eq!(log.file_name().unwrap(), "grayline-test.log");
    }
}
