use std::{
    io,
    path::{Path, PathBuf},
};

use grayline_shell::paths::{self, Roots};

use crate::identity::IDENTITY;

/// One of the directories the application keeps for the operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Folder {
    Received,
    Config,
}

impl Folder {
    pub const ALL: [Self; 2] = [Self::Received, Self::Config];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Received => "menu-open-received",
            Self::Config => "menu-open-config",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    config_file: PathBuf,
    received_dir: PathBuf,
    log_file: PathBuf,
}

impl AppPaths {
    pub fn discover() -> io::Result<Self> {
        Roots::discover(&IDENTITY).map(Self::from_roots)
    }

    pub(crate) fn from_roots(roots: Roots) -> Self {
        Self {
            config_file: roots.config_file(),
            received_dir: roots.pictures_dir.join("Received"),
            log_file: roots.log_file(&IDENTITY),
        }
    }

    pub fn initialize(&self) -> io::Result<()> {
        paths::initialize(&self.config_file, &self.log_file, &[&self.received_dir])
    }

    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    pub fn config_dir(&self) -> &Path {
        self.config_file.parent().unwrap_or(&self.config_file)
    }

    pub fn received_dir(&self) -> &Path {
        &self.received_dir
    }

    /// Returns the directory `folder` names.
    ///
    /// The configuration answers with the directory holding the file rather
    /// than the file itself: the application rewrites it as settings change,
    /// and a `.toml` has no dependable handler on every platform.
    pub fn folder(&self, folder: Folder) -> &Path {
        match folder {
            Folder::Received => &self.received_dir,
            Folder::Config => self.config_dir(),
        }
    }

    pub fn log_file(&self) -> &Path {
        &self.log_file
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn initialize_creates_every_directory_the_application_keeps() {
        let root = TempDir::new().unwrap();
        let paths = AppPaths::from_roots(Roots::under(root.path()));

        paths.initialize().unwrap();

        assert!(paths.config_file.is_file());
        for folder in Folder::ALL {
            assert!(paths.folder(folder).is_dir(), "{folder:?}");
        }
    }
}
