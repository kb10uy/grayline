use std::{
    io,
    path::{Path, PathBuf},
};

use grayline_shell::paths::{self, Roots};

use crate::identity::IDENTITY;

/// One of the directories the application keeps for the operator.
///
/// Every one of them is somewhere the operator is expected to work with a
/// file manager, so the interface only has to name them and point at them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Folder {
    Received,
    Sent,
    Stocks,
    Templates,
    Assets,
    Config,
}

impl Folder {
    pub const ALL: [Self; 6] = [
        Self::Received,
        Self::Sent,
        Self::Stocks,
        Self::Templates,
        Self::Assets,
        Self::Config,
    ];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Received => "menu-open-received",
            Self::Sent => "menu-open-sent",
            Self::Stocks => "menu-open-stocks",
            Self::Templates => "menu-open-templates",
            Self::Assets => "menu-open-assets",
            Self::Config => "menu-open-config",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    config_file: PathBuf,
    templates_dir: PathBuf,
    assets_dir: PathBuf,
    stocks_dir: PathBuf,
    sent_dir: PathBuf,
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
            templates_dir: roots.data_dir.join("templates"),
            assets_dir: roots.data_dir.join("assets"),
            stocks_dir: roots.pictures_dir.join("Stocks"),
            sent_dir: roots.pictures_dir.join("Sent"),
            received_dir: roots.pictures_dir.join("Received"),
            log_file: roots.log_file(&IDENTITY),
        }
    }

    pub fn initialize(&self) -> io::Result<()> {
        paths::initialize(
            &self.config_file,
            &self.log_file,
            &[
                &self.templates_dir,
                &self.assets_dir,
                &self.stocks_dir,
                &self.sent_dir,
                &self.received_dir,
            ],
        )
    }

    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    pub fn config_dir(&self) -> &Path {
        self.config_file.parent().unwrap_or(&self.config_file)
    }

    pub fn templates_dir(&self) -> &Path {
        &self.templates_dir
    }

    pub fn assets_dir(&self) -> &Path {
        &self.assets_dir
    }

    pub fn stocks_dir(&self) -> &Path {
        &self.stocks_dir
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
            Folder::Sent => &self.sent_dir,
            Folder::Stocks => &self.stocks_dir,
            Folder::Templates => &self.templates_dir,
            Folder::Assets => &self.assets_dir,
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

    /// The contact store is shared by every application in the family, and the
    /// crate that owns it restates the family directory rather than depending
    /// on the shell to learn one string. This is the one place both are in
    /// view, so it is where they are held to each other.
    #[test]
    fn the_contact_directory_agrees_with_the_shell_on_where_the_family_lives() {
        assert_eq!(
            grayline_qso::FAMILY_DIRECTORY,
            grayline_shell::platform::FAMILY_DIRECTORY
        );
    }
}
