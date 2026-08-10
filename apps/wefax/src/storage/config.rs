use std::{fs, io, path::PathBuf};

use grayline_shell::{i18n::Locale, log};
use grayline_wefax::{Ioc, LinesPerMinute};
use toml_edit::{DocumentMut, Item, value};

/// The zoom the interface is laid out at.
pub const DEFAULT_UI_SCALE: f32 = 1.0;
/// Bounds on the zoom.
pub const MINIMUM_UI_SCALE: f32 = 0.5;
pub const MAXIMUM_UI_SCALE: f32 = 3.0;

/// Everything the application remembers between sessions.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub locale: Locale,
    pub ui_scale: f32,
    pub device: Option<String>,
    pub ioc: Ioc,
    pub lines_per_minute: LinesPerMinute,
    pub auto_start: bool,
    pub auto_stop: bool,
    pub infer_lines_per_minute: bool,
    pub slant_tracking: bool,
    pub inverted: bool,
    pub narrow_shift: bool,
    pub auto_save: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: Locale::default(),
            ui_scale: DEFAULT_UI_SCALE,
            device: None,
            ioc: Ioc::Ioc576,
            lines_per_minute: LinesPerMinute::L120,
            auto_start: true,
            auto_stop: true,
            infer_lines_per_minute: true,
            slant_tracking: true,
            inverted: false,
            narrow_shift: false,
            auto_save: true,
        }
    }
}

/// The settings file, kept as the document it was read from.
///
/// Saving assigns the keys the application owns rather than rewriting the
/// document, so a comment the operator left beside an untouched key survives a
/// save that did not concern it.
#[derive(Debug)]
pub struct Config {
    path: Option<PathBuf>,
    document: DocumentMut,
    /// Set when the file could not be parsed.
    ///
    /// The application starts on defaults and refuses to save, because
    /// overwriting a file it could not read would throw away whatever the
    /// operator had written in it.
    read_only: bool,
    error: Option<String>,
}

impl Config {
    /// Reads the settings file, falling back to defaults it will not save over.
    pub fn load(path: PathBuf) -> (Self, Settings) {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return (Self::unreadable(path, &error.to_string()), Settings::default());
            }
        };
        match text.parse::<DocumentMut>() {
            Ok(document) => {
                let settings = read(&document);
                (
                    Self {
                        path: Some(path),
                        document,
                        read_only: false,
                        error: None,
                    },
                    settings,
                )
            }
            Err(error) => (Self::unreadable(path, &error.to_string()), Settings::default()),
        }
    }

    /// A configuration with nowhere to write, for tests and headless runs.
    #[cfg(test)]
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

    /// Returns what went wrong reading the file, if anything did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Writes `settings` into the document and saves it.
    pub fn save(&mut self, settings: &Settings) {
        let Some(path) = self.path.clone() else { return };
        if self.read_only {
            return;
        }
        let table = self.document.as_table_mut();
        table["language"] = value(settings.locale.tag());
        table["ui_scale"] = value(f64::from(settings.ui_scale));
        match &settings.device {
            Some(device) => table["device"] = value(device.as_str()),
            None => {
                table.remove("device");
            }
        }
        table["ioc"] = value(i64::from(settings.ioc.index()));
        table["lines_per_minute"] = value(i64::from(settings.lines_per_minute.as_lpm()));
        table["auto_start"] = value(settings.auto_start);
        table["auto_stop"] = value(settings.auto_stop);
        table["infer_lines_per_minute"] = value(settings.infer_lines_per_minute);
        table["slant_tracking"] = value(settings.slant_tracking);
        table["inverted"] = value(settings.inverted);
        table["narrow_shift"] = value(settings.narrow_shift);
        table["auto_save"] = value(settings.auto_save);

        if let Err(error) = fs::write(&path, self.document.to_string()) {
            log::note(&format!("could not save {}: {error}", path.display()));
        }
    }
}

/// Reads what the application understands, leaving everything else alone.
///
/// A value that is the wrong type, or names something this build does not
/// support, is dropped rather than rejected: the file is the operator's, and
/// one unusable key is not a reason to start on nothing.
fn read(document: &DocumentMut) -> Settings {
    let mut settings = Settings::default();
    let table = document.as_table();
    let get = |key: &str| table.get(key).and_then(Item::as_value);

    if let Some(tag) = get("language").and_then(|value| value.as_str())
        && let Some(locale) = Locale::from_tag(tag)
    {
        settings.locale = locale;
    }
    if let Some(scale) = get("ui_scale").and_then(|value| value.as_float()) {
        settings.ui_scale = (scale as f32).clamp(MINIMUM_UI_SCALE, MAXIMUM_UI_SCALE);
    }
    if let Some(device) = get("device").and_then(|value| value.as_str()) {
        settings.device = Some(device.to_owned());
    }
    if let Some(index) = get("ioc").and_then(|value| value.as_integer())
        && let Some(ioc) = Ioc::ALL.into_iter().find(|ioc| i64::from(ioc.index()) == index)
    {
        settings.ioc = ioc;
    }
    if let Some(lpm) = get("lines_per_minute").and_then(|value| value.as_integer())
        && let Some(rate) = LinesPerMinute::ALL
            .into_iter()
            .find(|rate| i64::from(rate.as_lpm()) == lpm)
    {
        settings.lines_per_minute = rate;
    }
    for (key, target) in [
        ("auto_start", &mut settings.auto_start),
        ("auto_stop", &mut settings.auto_stop),
        ("infer_lines_per_minute", &mut settings.infer_lines_per_minute),
        ("slant_tracking", &mut settings.slant_tracking),
        ("inverted", &mut settings.inverted),
        ("narrow_shift", &mut settings.narrow_shift),
        ("auto_save", &mut settings.auto_save),
    ] {
        if let Some(flag) = get(key).and_then(|value| value.as_bool()) {
            *target = flag;
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_missing_file_starts_on_defaults_and_can_be_saved() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        let (mut config, settings) = Config::load(path.clone());
        assert_eq!(settings, Settings::default());
        assert!(config.error().is_none());

        config.save(&settings);
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("language = \"en\""));
        assert!(written.contains("ioc = 576"));
    }

    #[test]
    fn stored_settings_survive_a_round_trip() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        let wanted = Settings {
            locale: Locale::Ja,
            ui_scale: 1.25,
            device: Some("Line In".to_owned()),
            ioc: Ioc::Ioc288,
            lines_per_minute: LinesPerMinute::L240,
            auto_start: false,
            auto_stop: false,
            infer_lines_per_minute: false,
            slant_tracking: false,
            inverted: true,
            narrow_shift: true,
            auto_save: false,
        };
        Config::load(path.clone()).0.save(&wanted);

        let (_, read_back) = Config::load(path);
        assert_eq!(read_back, wanted);
    }

    /// A comment beside a key the application does not own has to survive a
    /// save, or editing the file by hand is pointless.
    #[test]
    fn saving_preserves_comments_and_unknown_keys() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        fs::write(&path, "# the station's own note\nunknown = 7\nlanguage = \"ja\"\n").unwrap();

        let (mut config, settings) = Config::load(path.clone());
        assert_eq!(settings.locale, Locale::Ja);
        config.save(&settings);

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("# the station's own note"));
        assert!(written.contains("unknown = 7"));
    }

    /// Overwriting a file that could not be parsed would throw away whatever
    /// the operator had written in it.
    #[test]
    fn an_unparsable_file_is_never_written_over() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        fs::write(&path, "this is not toml = = =\n").unwrap();

        let (mut config, settings) = Config::load(path.clone());
        assert!(config.error().is_some());
        assert_eq!(settings, Settings::default());

        config.save(&Settings::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), "this is not toml = = =\n");
    }

    #[test]
    fn unusable_values_fall_back_without_discarding_the_rest() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            "language = 7\nioc = 999\nlines_per_minute = 5\ninverted = true\n",
        )
        .unwrap();

        let (_, settings) = Config::load(path);
        assert_eq!(settings.locale, Locale::default());
        assert_eq!(settings.ioc, Ioc::Ioc576);
        assert_eq!(settings.lines_per_minute, LinesPerMinute::L120);
        assert!(settings.inverted);
    }

    #[test]
    fn the_stored_zoom_is_bounded() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        fs::write(&path, "ui_scale = 99.0\n").unwrap();
        assert_eq!(Config::load(path).1.ui_scale, MAXIMUM_UI_SCALE);
    }
}
