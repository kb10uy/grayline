use std::path::PathBuf;

use directories::BaseDirs;

/// The directory every application in this family keeps its files under.
///
/// The same rule `grayline_shell::platform::FAMILY_DIRECTORY` applies, restated
/// here rather than depended on: this crate is reached by a command-line tool
/// that has no reason to pull in a window toolkit, a font database and a
/// localization runtime to learn one string. An application that has both in
/// view is where the two are checked against each other.
pub const FAMILY_DIRECTORY: &str = if cfg!(target_os = "linux") {
    "grayline"
} else {
    "Grayline"
};

/// The store, named where it sits beside the per-application directories.
pub const STORE_FILE: &str = "contacts.sqlite3";

/// The store every application in the family shares.
///
/// Under the data directory rather than the configuration one, because this is
/// accumulated data rather than a setting, and because it should follow the
/// operator between machines — the opposite of a log, which describes the
/// machine it was written on.
pub fn default_store_path() -> Option<PathBuf> {
    Some(BaseDirs::new()?.data_dir().join(FAMILY_DIRECTORY).join(STORE_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_sits_beside_the_per_application_directories() {
        let Some(store) = default_store_path() else {
            return;
        };

        assert_eq!(store.file_name().and_then(|name| name.to_str()), Some(STORE_FILE));
        assert_eq!(
            store
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some(FAMILY_DIRECTORY)
        );
    }
}
