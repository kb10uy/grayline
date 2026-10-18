use std::{
    io,
    path::{Path, PathBuf},
};

use grayline_shell::paths::{self, Roots};

use crate::identity::IDENTITY;

/// The buttons under the message field, and the list beside them.
///
/// Beside the settings rather than in them: they are written by hand and
/// never by the application, and one file each is what makes them findable
/// in the directory the File menu opens.
pub const MACRO_FILE: &str = "macros.toml";
pub const TEMPLATE_FILE: &str = "templates.toml";

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
        Roots::discover(&IDENTITY).map(Self::from_roots)
    }

    pub(crate) fn from_roots(roots: Roots) -> Self {
        Self {
            config_file: roots.config_file(),
            log_file: roots.log_file(&IDENTITY),
        }
    }

    pub fn initialize(&self) -> io::Result<()> {
        paths::initialize(&self.config_file, &self.log_file, &[])
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
