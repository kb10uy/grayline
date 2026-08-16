use std::{fs, io, path::PathBuf};

use grayline_shell::{i18n::Locale, log};
use toml_edit::{DocumentMut, Item, value};

/// The zoom the interface is laid out at.
pub const DEFAULT_UI_SCALE: f32 = 1.0;
/// Bounds on the zoom.
pub const MINIMUM_UI_SCALE: f32 = 0.5;
pub const MAXIMUM_UI_SCALE: f32 = 3.0;

/// Bounds on the mark tone, in hertz.
///
/// Wide enough for every audio pair a receiver produces, and no wider: the
/// front end checks the pair against Nyquist itself, but a figure typed into
/// the panel should be refused where it is typed rather than by a decoder
/// that then has nothing to decode.
pub const MINIMUM_MARK_HZ: f64 = 300.0;
pub const MAXIMUM_MARK_HZ: f64 = 3_000.0;

/// The separations the panel offers, in hertz.
///
/// The first four are what the automatic frequency control's amateur mode
/// snaps to; 425 and 850 are the commercial shifts a listener still meets.
pub const SHIFTS_HZ: [f64; 6] = [170.0, 200.0, 220.0, 240.0, 425.0, 850.0];

/// The speeds the panel offers, in baud.
///
/// 45.45 is amateur RTTY; the rest are the commercial and weather speeds
/// MMTTY's own selector carries.
pub const BAUD_RATES: [f64; 5] = [45.45, 50.0, 56.88, 75.0, 100.0];

/// Bounds on the squelch threshold, on the normalized `|mark − space|` scale
/// the core documents: noise alone peaks near 0.15 and a clean signal reads
/// about 0.5.
pub const MINIMUM_SQUELCH: f64 = 0.0;
pub const MAXIMUM_SQUELCH: f64 = 1.0;

/// Everything the application remembers between sessions.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub locale: Locale,
    pub ui_scale: f32,
    pub device: Option<String>,
    /// The tone a mark is heard at; space sits `shift_hz` above it.
    pub mark_hz: f64,
    pub shift_hz: f64,
    pub baud: f64,
    /// Swaps the tones, for a signal received on the other sideband.
    pub reverse: bool,
    pub afc: bool,
    pub squelch: bool,
    pub squelch_threshold: f64,
    pub unshift_on_space: bool,
    pub atc: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: Locale::default(),
            ui_scale: DEFAULT_UI_SCALE,
            device: None,
            mark_hz: 2_125.0,
            shift_hz: 170.0,
            baud: 45.45,
            reverse: false,
            // Off, unlike MMTTY, which ships it on. This core's frequency
            // control searches the whole configured range for the two
            // strongest peaks rather than sweeping outward from the pair it
            // is on (`docs/memo/grayline/rtty.md`), and on an empty band the
            // strongest pair in the noise is whatever the receiver's own hum
            // happens to be: a watch left running with it on walks off the
            // pair it was put on. Switched on for a signal, it does what it
            // is for.
            afc: false,
            squelch: true,
            squelch_threshold: 0.25,
            unshift_on_space: true,
            atc: false,
        }
    }
}

impl Settings {
    /// Brings every figure back inside what the receiver can be built from.
    ///
    /// Applied to what is read from the file and to what the panel produces
    /// alike, so a hand-edited value arrives in the shape a typed one does.
    pub fn clamped(mut self) -> Self {
        self.ui_scale = self.ui_scale.clamp(MINIMUM_UI_SCALE, MAXIMUM_UI_SCALE);
        self.mark_hz = self.mark_hz.clamp(MINIMUM_MARK_HZ, MAXIMUM_MARK_HZ);
        self.shift_hz = self.shift_hz.clamp(SHIFTS_HZ[0], SHIFTS_HZ[SHIFTS_HZ.len() - 1]);
        self.baud = self.baud.clamp(BAUD_RATES[0], BAUD_RATES[BAUD_RATES.len() - 1]);
        self.squelch_threshold = self.squelch_threshold.clamp(MINIMUM_SQUELCH, MAXIMUM_SQUELCH);
        self
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
        table["mark_hz"] = value(settings.mark_hz);
        table["shift_hz"] = value(settings.shift_hz);
        table["baud"] = value(settings.baud);
        table["reverse"] = value(settings.reverse);
        table["afc"] = value(settings.afc);
        table["squelch"] = value(settings.squelch);
        table["squelch_threshold"] = value(settings.squelch_threshold);
        table["unshift_on_space"] = value(settings.unshift_on_space);
        table["atc"] = value(settings.atc);

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
        settings.ui_scale = scale as f32;
    }
    if let Some(device) = get("device").and_then(|value| value.as_str()) {
        settings.device = Some(device.to_owned());
    }
    // Written as floats, because a shift and a speed are: read as either, so
    // a hand-edited `shift_hz = 170` is not thrown away for having no point
    // in it.
    for (key, target) in [
        ("mark_hz", &mut settings.mark_hz),
        ("shift_hz", &mut settings.shift_hz),
        ("baud", &mut settings.baud),
        ("squelch_threshold", &mut settings.squelch_threshold),
    ] {
        if let Some(number) = get(key).and_then(number) {
            *target = number;
        }
    }
    for (key, target) in [
        ("reverse", &mut settings.reverse),
        ("afc", &mut settings.afc),
        ("squelch", &mut settings.squelch),
        ("unshift_on_space", &mut settings.unshift_on_space),
        ("atc", &mut settings.atc),
    ] {
        if let Some(flag) = get(key).and_then(|value| value.as_bool()) {
            *target = flag;
        }
    }
    settings.clamped()
}

/// Reads a TOML number however it was written, ignoring one that is not
/// finite: a decoder built from a NaN would fail on every sample.
fn number(item: &toml_edit::Value) -> Option<f64> {
    item.as_float()
        .or_else(|| item.as_integer().map(|integer| integer as f64))
        .filter(|number| number.is_finite())
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
        assert!(written.contains("mark_hz = 2125.0"));
    }

    #[test]
    fn stored_settings_survive_a_round_trip() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        let wanted = Settings {
            locale: Locale::Ja,
            ui_scale: 1.25,
            device: Some("Line In".to_owned()),
            mark_hz: 1_275.0,
            shift_hz: 850.0,
            baud: 75.0,
            reverse: true,
            afc: false,
            squelch: false,
            squelch_threshold: 0.4,
            unshift_on_space: false,
            atc: true,
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
        fs::write(&path, "language = 7\nmark_hz = \"low\"\nbaud = nan\nreverse = true\n").unwrap();

        let (_, settings) = Config::load(path);
        assert_eq!(settings.locale, Locale::default());
        assert_eq!(settings.mark_hz, Settings::default().mark_hz);
        assert_eq!(settings.baud, Settings::default().baud);
        assert!(settings.reverse);
    }

    /// A figure the receiver could not be built from must not reach it.
    #[test]
    fn stored_figures_are_brought_back_into_range() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            "ui_scale = 99.0\nmark_hz = 40000.0\nshift_hz = 1.0\nbaud = 9000.0\nsquelch_threshold = 4.0\n",
        )
        .unwrap();

        let (_, settings) = Config::load(path);
        assert_eq!(settings.ui_scale, MAXIMUM_UI_SCALE);
        assert_eq!(settings.mark_hz, MAXIMUM_MARK_HZ);
        assert_eq!(settings.shift_hz, SHIFTS_HZ[0]);
        assert_eq!(settings.baud, BAUD_RATES[BAUD_RATES.len() - 1]);
        assert_eq!(settings.squelch_threshold, MAXIMUM_SQUELCH);
    }

    /// A shift written without a decimal point is still a shift.
    #[test]
    fn a_whole_number_is_read_as_the_figure_it_is() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        fs::write(&path, "shift_hz = 425\nbaud = 75\n").unwrap();

        let (_, settings) = Config::load(path);
        assert_eq!(settings.shift_hz, 425.0);
        assert_eq!(settings.baud, 75.0);
    }
}
