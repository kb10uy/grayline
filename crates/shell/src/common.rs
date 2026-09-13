//! The settings every application in this family shares.
//!
//! The language and the interface scale are not properties of one mode: an
//! operator who reads Japanese reads it in all three applications, and a
//! display that wants everything drawn half again as large wants it drawn that
//! way whichever one is open. So they are kept once, in `common.toml` beside
//! the per-application directories, rather than three times in files that
//! would disagree with each other the moment one was changed.
//!
//! Everything else stays where it is. What an application stores about tones,
//! modes, or devices means nothing to the others, and this file is only for
//! what all of them answer the same way.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use toml_edit::{DocumentMut, Item, value};

use crate::{i18n::Locale, log, platform::FAMILY_DIRECTORY};

/// The shared settings file, beside the per-application directories.
pub const COMMON_FILE: &str = "common.toml";

/// The zoom a first run is laid out at.
pub const DEFAULT_UI_SCALE: f32 = 1.0;

/// How far the interface may be scaled.
///
/// A stored value is clamped to this, so a hand-edited file cannot shrink the
/// interface past the point where the setting could be changed back.
pub const UI_SCALE_RANGE: core::ops::RangeInclusive<f32> = 0.5..=3.0;

/// What every application in the family reads out of the shared file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommonSettings {
    /// The language every interface in the family is drawn in.
    pub locale: Locale,
    /// The zoom every interface in the family is laid out at.
    pub ui_scale: f32,
}

impl Default for CommonSettings {
    fn default() -> Self {
        Self {
            locale: Locale::default(),
            ui_scale: DEFAULT_UI_SCALE,
        }
    }
}

impl CommonSettings {
    /// Brings the scale back inside what the interface can be drawn at.
    ///
    /// Applied to what is read from the file and to what a menu produces
    /// alike, so a hand-edited value arrives in the shape a chosen one does.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.ui_scale = self.ui_scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end());
        self
    }
}

/// Where the shared file sits, if the user directories can be found at all.
#[must_use]
pub fn default_common_path() -> Option<PathBuf> {
    Some(BaseDirs::new()?.config_dir().join(FAMILY_DIRECTORY).join(COMMON_FILE))
}

/// The shared file, kept as the document it was read from.
///
/// Saving assigns the two keys this owns rather than rewriting the document,
/// so a comment the operator left beside one survives, and so does whatever
/// another application in the family has written there.
#[derive(Debug)]
pub struct CommonConfig {
    path: Option<PathBuf>,
    document: DocumentMut,
    /// Set when the file could not be parsed.
    ///
    /// The application starts on defaults and refuses to save, because
    /// overwriting a file it could not read would throw away whatever the
    /// operator had written in it.
    read_only: bool,
    error: Option<String>,
    /// What the file already says, so a frame that changed nothing writes
    /// nothing.
    saved: CommonSettings,
}

impl CommonConfig {
    /// Reads the shared file from wherever this platform keeps it.
    ///
    /// A machine whose user directories cannot be found gets a configuration
    /// with nowhere to write rather than a failure: an application that cannot
    /// remember a language should still run in one.
    #[must_use]
    pub fn discover() -> Self {
        match default_common_path() {
            Some(path) => Self::load(path),
            None => Self::detached(),
        }
    }

    /// Reads `path`, falling back to defaults it will not save over.
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Self::unreadable(path, &error.to_string()),
        };
        match text.parse::<DocumentMut>() {
            Ok(document) => {
                let settings = read(&document);
                Self {
                    path: Some(path),
                    document,
                    read_only: false,
                    error: None,
                    saved: settings,
                }
            }
            Err(error) => Self::unreadable(path, &error.to_string()),
        }
    }

    /// What the file says, or the defaults where it said nothing usable.
    #[must_use]
    pub const fn settings(&self) -> CommonSettings {
        self.saved
    }

    /// A configuration with nowhere to write, for tests and headless runs.
    #[must_use]
    pub fn detached() -> Self {
        Self {
            path: None,
            document: DocumentMut::new(),
            read_only: false,
            error: None,
            saved: CommonSettings::default(),
        }
    }

    fn unreadable(path: PathBuf, error: &str) -> Self {
        log::note(&format!("could not read {}: {error}", path.display()));
        Self {
            path: Some(path),
            document: DocumentMut::new(),
            read_only: true,
            error: Some(error.to_owned()),
            saved: CommonSettings::default(),
        }
    }

    /// Returns what went wrong reading the file, if anything did.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Writes `settings` back, unless they are what the file already says.
    ///
    /// Every application calls this at the end of every frame, which is why
    /// the comparison is here rather than in each of them.
    pub fn save(&mut self, settings: &CommonSettings) {
        let settings = settings.clamped();
        if self.read_only || settings == self.saved {
            return;
        }
        self.saved = settings;
        let Some(path) = self.path.clone() else { return };
        let table = self.document.as_table_mut();
        table["language"] = value(settings.locale.tag());
        // Rounded on the way out: widening the f32 directly writes the likes
        // of 1.2999999523162842 into a file meant to be read by hand.
        table["ui-scale"] = value((f64::from(settings.ui_scale) * 100.0).round() / 100.0);

        if let Err(error) = write(&path, &self.document.to_string()) {
            log::note(&format!("could not save {}: {error}", path.display()));
        }
    }
}

/// Writes the document, making the family directory if it is not there yet.
///
/// Each application creates its own directory under that one as it starts, but
/// this file sits above all of them, and whichever application saves first is
/// the one that has to put the directory there.
fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

/// Reads what the family understands, leaving everything else alone.
///
/// A value that is the wrong type, or names a language this build does not
/// carry, is dropped rather than rejected: the file is the operator's, and one
/// unusable key is not a reason to start on nothing.
fn read(document: &DocumentMut) -> CommonSettings {
    let mut settings = CommonSettings::default();
    let table = document.as_table();
    let get = |key: &str| table.get(key).and_then(Item::as_value);

    if let Some(tag) = get("language").and_then(|value| value.as_str())
        && let Some(locale) = Locale::from_tag(tag)
    {
        settings.locale = locale;
    }
    // Read as either a float or an integer, so a hand-written `ui-scale = 1`
    // is not thrown away for having no point in it. A non-finite one is
    // dropped: TOML spells `nan` and `inf`, a NaN passes the clamp unchanged,
    // and an interface laid out at one would not be laid out at all.
    if let Some(scale) = get("ui-scale")
        .and_then(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|integer| integer as f64))
        })
        .filter(|scale| scale.is_finite())
    {
        settings.ui_scale = scale as f32;
    }
    settings.clamped()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A scratch directory that removes itself, so a test that writes a file
    /// leaves nothing behind.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("grayline-shell-{name}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join(COMMON_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_file_starts_on_defaults_and_can_be_saved() {
        let root = TempDir::new("missing");
        let mut config = CommonConfig::load(root.file());
        assert_eq!(config.settings(), CommonSettings::default());
        assert!(config.error().is_none());

        config.save(&CommonSettings {
            locale: Locale::Ja,
            ui_scale: 1.25,
        });

        let written = fs::read_to_string(root.file()).expect("the file was written");
        assert!(written.contains("language = \"ja\""), "{written}");
        assert!(written.contains("ui-scale = 1.25"), "{written}");
    }

    #[test]
    fn stored_settings_survive_a_round_trip() {
        let root = TempDir::new("round-trip");
        let wanted = CommonSettings {
            locale: Locale::Ja,
            ui_scale: 1.5,
        };
        CommonConfig::load(root.file()).save(&wanted);

        assert_eq!(CommonConfig::load(root.file()).settings(), wanted);
    }

    /// The file sits above every application's own directory, so the first
    /// application to save is the one that has to create it.
    #[test]
    fn the_family_directory_is_made_on_the_way_out() {
        let root = TempDir::new("make-directory");
        let path = root.0.join("not-there-yet").join(COMMON_FILE);
        CommonConfig::load(path.clone()).save(&CommonSettings {
            locale: Locale::Ja,
            ui_scale: DEFAULT_UI_SCALE,
        });

        assert!(path.is_file(), "{}", path.display());
    }

    /// Another application in the family may have written keys this build has
    /// never heard of, and a comment beside one is the operator's.
    #[test]
    fn saving_preserves_comments_and_unknown_keys() {
        let root = TempDir::new("comments");
        fs::write(
            root.file(),
            "# the station's own note\nunknown = 7\nlanguage = \"ja\"\n",
        )
        .unwrap();

        let mut config = CommonConfig::load(root.file());
        assert_eq!(config.settings().locale, Locale::Ja);
        config.save(&CommonSettings {
            ui_scale: 1.1,
            ..config.settings()
        });

        let written = fs::read_to_string(root.file()).unwrap();
        assert!(written.contains("# the station's own note"), "{written}");
        assert!(written.contains("unknown = 7"), "{written}");
    }

    /// Every application saves at the end of every frame and they share this
    /// file: one that rewrote it on a frame that changed nothing would be
    /// writing over what another had just put there.
    #[test]
    fn a_save_that_changes_nothing_does_not_touch_the_file() {
        let root = TempDir::new("unchanged");
        fs::write(root.file(), "language = \"ja\"\n").unwrap();

        let mut config = CommonConfig::load(root.file());
        config.save(&config.settings());

        assert_eq!(fs::read_to_string(root.file()).unwrap(), "language = \"ja\"\n");
    }

    /// Overwriting a file that could not be parsed would throw away whatever
    /// the operator had written in it.
    #[test]
    fn an_unparsable_file_is_never_written_over() {
        let root = TempDir::new("unparsable");
        fs::write(root.file(), "this is not toml = = =\n").unwrap();

        let mut config = CommonConfig::load(root.file());
        assert!(config.error().is_some());
        assert_eq!(config.settings(), CommonSettings::default());

        config.save(&CommonSettings {
            locale: Locale::Ja,
            ui_scale: 2.0,
        });
        assert_eq!(fs::read_to_string(root.file()).unwrap(), "this is not toml = = =\n");
    }

    #[test]
    fn unusable_values_fall_back_and_a_scale_is_brought_into_range() {
        let root = TempDir::new("unusable");
        fs::write(root.file(), "language = 7\nui-scale = 99\n").unwrap();

        let settings = CommonConfig::load(root.file()).settings();
        assert_eq!(settings.locale, Locale::default());
        assert_eq!(settings.ui_scale, *UI_SCALE_RANGE.end());
    }

    /// TOML spells `nan` and `inf`, and a NaN passes a range clamp unchanged.
    #[rstest]
    #[case("nan", "ui-scale = nan\n")]
    #[case("inf", "ui-scale = inf\n")]
    fn a_non_finite_scale_reads_as_the_default(#[case] name: &str, #[case] contents: &str) {
        let root = TempDir::new(&format!("non-finite-{name}"));
        fs::write(root.file(), contents).unwrap();

        assert_eq!(CommonConfig::load(root.file()).settings().ui_scale, DEFAULT_UI_SCALE);
    }

    /// A configuration with nowhere to write still has to answer, because a
    /// machine whose user directories cannot be found still runs.
    #[test]
    fn a_detached_configuration_writes_nothing() {
        let mut config = CommonConfig::detached();
        config.save(&CommonSettings {
            locale: Locale::Ja,
            ui_scale: 1.5,
        });
        assert!(config.error().is_none());
    }
}
