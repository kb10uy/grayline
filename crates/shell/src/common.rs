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

use std::path::PathBuf;

use directories::BaseDirs;
use toml_edit::{Item, Table, value};

use crate::{config::Document, i18n::Locale, platform::FAMILY_DIRECTORY};

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
fn default_common_path() -> Option<PathBuf> {
    Some(BaseDirs::new()?.config_dir().join(FAMILY_DIRECTORY).join(COMMON_FILE))
}

/// The shared file, kept as the document it was read from.
///
/// Saving assigns the two keys this owns rather than rewriting the document,
/// so a comment the operator left beside one survives, and so does whatever
/// another application in the family has written there.
#[derive(Debug)]
pub struct CommonConfig {
    document: Document,
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
        let document = Document::load(path);
        let saved = read(document.table());
        Self { document, saved }
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
            document: Document::detached(),
            saved: CommonSettings::default(),
        }
    }

    /// Returns what went wrong reading or writing the file, if anything did.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.document.error()
    }

    /// Writes `settings` back, unless they are what the file already says.
    ///
    /// Every application calls this at the end of every frame, which is why
    /// the comparison is here rather than in each of them. The family
    /// directory is made if it is not there yet: each application creates its
    /// own directory under it as it starts, but this file sits above all of
    /// them, and whichever saves first has to put the directory there.
    pub fn save(&mut self, settings: &CommonSettings) {
        let settings = settings.clamped();
        if settings == self.saved {
            return;
        }
        self.saved = settings;
        self.document.save(|table| {
            table["language"] = value(settings.locale.tag());
            // Rounded on the way out: widening the f32 directly writes the
            // likes of 1.2999999523162842 into a file meant to be read by hand.
            table["ui-scale"] = value((f64::from(settings.ui_scale) * 100.0).round() / 100.0);
        });
    }
}

/// Reads what the family understands, leaving everything else alone.
///
/// A value that is the wrong type, or names a language this build does not
/// carry, is dropped rather than rejected: the file is the operator's, and one
/// unusable key is not a reason to start on nothing.
fn read(table: &Table) -> CommonSettings {
    let mut settings = CommonSettings::default();
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
    use std::fs;

    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_missing_file_starts_on_defaults_and_can_be_saved() {
        let root = TempDir::new().unwrap();
        let mut config = CommonConfig::load(root.path().join(COMMON_FILE));
        assert_eq!(config.settings(), CommonSettings::default());
        assert!(config.error().is_none());

        config.save(&CommonSettings {
            locale: Locale::Ja,
            ui_scale: 1.25,
        });

        let written = fs::read_to_string(root.path().join(COMMON_FILE)).expect("the file was written");
        assert!(written.contains("language = \"ja\""), "{written}");
        assert!(written.contains("ui-scale = 1.25"), "{written}");
    }

    #[test]
    fn stored_settings_survive_a_round_trip() {
        let root = TempDir::new().unwrap();
        let wanted = CommonSettings {
            locale: Locale::Ja,
            ui_scale: 1.5,
        };
        CommonConfig::load(root.path().join(COMMON_FILE)).save(&wanted);

        assert_eq!(CommonConfig::load(root.path().join(COMMON_FILE)).settings(), wanted);
    }

    #[test]
    fn the_family_directory_is_made_on_the_way_out() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("not-there-yet").join(COMMON_FILE);
        CommonConfig::load(path.clone()).save(&CommonSettings {
            locale: Locale::Ja,
            ui_scale: DEFAULT_UI_SCALE,
        });

        assert!(path.is_file(), "{}", path.display());
    }

    #[test]
    fn saving_preserves_comments_and_unknown_keys() {
        let root = TempDir::new().unwrap();
        fs::write(
            root.path().join(COMMON_FILE),
            "# the station's own note\nunknown = 7\nlanguage = \"ja\"\n",
        )
        .unwrap();

        let mut config = CommonConfig::load(root.path().join(COMMON_FILE));
        assert_eq!(config.settings().locale, Locale::Ja);
        config.save(&CommonSettings {
            ui_scale: 1.1,
            ..config.settings()
        });

        let written = fs::read_to_string(root.path().join(COMMON_FILE)).unwrap();
        assert!(written.contains("# the station's own note"), "{written}");
        assert!(written.contains("unknown = 7"), "{written}");
    }

    #[test]
    fn a_save_that_changes_nothing_does_not_touch_the_file() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(COMMON_FILE), "language = \"ja\"\n").unwrap();

        let mut config = CommonConfig::load(root.path().join(COMMON_FILE));
        config.save(&config.settings());

        assert_eq!(
            fs::read_to_string(root.path().join(COMMON_FILE)).unwrap(),
            "language = \"ja\"\n"
        );
    }

    #[test]
    fn an_unparsable_file_is_never_written_over() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(COMMON_FILE), "this is not toml = = =\n").unwrap();

        let mut config = CommonConfig::load(root.path().join(COMMON_FILE));
        assert!(config.error().is_some());
        assert_eq!(config.settings(), CommonSettings::default());

        config.save(&CommonSettings {
            locale: Locale::Ja,
            ui_scale: 2.0,
        });
        assert_eq!(
            fs::read_to_string(root.path().join(COMMON_FILE)).unwrap(),
            "this is not toml = = =\n"
        );
    }

    #[test]
    fn unusable_values_fall_back_and_a_scale_is_brought_into_range() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(COMMON_FILE), "language = 7\nui-scale = 99\n").unwrap();

        let settings = CommonConfig::load(root.path().join(COMMON_FILE)).settings();
        assert_eq!(settings.locale, Locale::default());
        assert_eq!(settings.ui_scale, *UI_SCALE_RANGE.end());
    }

    /// TOML spells `nan` and `inf`, and a NaN passes a range clamp unchanged.
    #[rstest]
    #[case("ui-scale = nan\n")]
    #[case("ui-scale = inf\n")]
    fn a_non_finite_scale_reads_as_the_default(#[case] contents: &str) {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(COMMON_FILE), contents).unwrap();

        assert_eq!(
            CommonConfig::load(root.path().join(COMMON_FILE)).settings().ui_scale,
            DEFAULT_UI_SCALE
        );
    }

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
