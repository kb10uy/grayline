use std::{collections::BTreeMap, fs, io, path::PathBuf};

use grayline_shell::log;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::app::macros::{Station, valid_variable_name};

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

/// Bounds on the transmit level, as fader travel.
pub const MINIMUM_TX_LEVEL: f64 = 0.0;
pub const MAXIMUM_TX_LEVEL: f64 = 1.0;

/// Everything the application remembers between sessions.
///
/// The language and the interface scale are not here: they are the same in
/// every application of this family and are kept once, in
/// `grayline_shell::common`.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub device: Option<String>,
    /// Where a transmission is played out.
    pub output_device: Option<String>,
    /// The tone a mark is heard at; space sits `shift_hz` above it.
    pub mark_hz: f64,
    pub shift_hz: f64,
    pub baud: f64,
    /// Swaps the tones, for a signal received on the other sideband.
    pub reverse: bool,
    pub afc: bool,
    /// The reading a signal has to beat to be printed; zero is no squelch.
    pub squelch_threshold: f64,
    pub unshift_on_space: bool,
    pub atc: bool,
    /// Whether a transmission re-announces figures after a space.
    pub tx_unshift_on_space: bool,
    /// Transmit level, as fader travel in `0..=1`.
    pub tx_level: f64,
    /// Who this station is, for the macros that say so.
    pub station: Station,
    /// The operator's own fields, read from a macro as `${custom.<name>}`.
    pub custom_variables: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device: None,
            output_device: None,
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
            squelch_threshold: 0.25,
            unshift_on_space: true,
            atc: false,
            // On, unlike the receive setting's counterpart in MMTTY, because
            // this one is about the station being sent to: a receiver running
            // unshift-on-space prints a figures group after a space as
            // letters unless the transmission says otherwise.
            tx_unshift_on_space: true,
            tx_level: 0.95,
            station: Station::default(),
            custom_variables: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Brings every figure back inside what the receiver can be built from.
    ///
    /// Applied to what is read from the file and to what the panel produces
    /// alike, so a hand-edited value arrives in the shape a typed one does.
    pub fn clamped(mut self) -> Self {
        self.mark_hz = self.mark_hz.clamp(MINIMUM_MARK_HZ, MAXIMUM_MARK_HZ);
        self.shift_hz = self.shift_hz.clamp(SHIFTS_HZ[0], SHIFTS_HZ[SHIFTS_HZ.len() - 1]);
        self.baud = self.baud.clamp(BAUD_RATES[0], BAUD_RATES[BAUD_RATES.len() - 1]);
        self.squelch_threshold = self.squelch_threshold.clamp(MINIMUM_SQUELCH, MAXIMUM_SQUELCH);
        self.tx_level = if self.tx_level.is_finite() {
            self.tx_level.clamp(MINIMUM_TX_LEVEL, MAXIMUM_TX_LEVEL)
        } else {
            Self::default().tx_level
        };
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
        if self.path.is_none() || self.read_only {
            return;
        }
        let table = self.document.as_table_mut();
        for (key, name) in [("device", &settings.device), ("output_device", &settings.output_device)] {
            match name {
                Some(name) => table[key] = value(name.as_str()),
                None => {
                    table.remove(key);
                }
            }
        }
        table["mark_hz"] = value(settings.mark_hz);
        table["shift_hz"] = value(settings.shift_hz);
        table["baud"] = value(settings.baud);
        table["reverse"] = value(settings.reverse);
        table["afc"] = value(settings.afc);
        table["squelch_threshold"] = value(settings.squelch_threshold);
        table["unshift_on_space"] = value(settings.unshift_on_space);
        table["atc"] = value(settings.atc);
        table["tx_unshift_on_space"] = value(settings.tx_unshift_on_space);
        table["tx_level"] = value(settings.tx_level);

        let station = table
            .entry("station")
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut();
        if let Some(station_table) = station {
            station_table["callsign"] = value(settings.station.callsign.as_str());
            station_table["name"] = value(settings.station.name.as_str());
            station_table["qth"] = value(settings.station.qth.as_str());
            station_table["grid"] = value(settings.station.grid.as_str());
        }

        // Written once and then left alone. There is no editor for them here,
        // so the file is where they are changed, and rewriting the array on
        // every save would reformat what the operator had written in it.
        store_custom_variables(table, &settings.custom_variables);

        self.write();
    }

    fn write(&self) {
        let Some(path) = self.path.as_ref() else { return };
        if self.read_only {
            return;
        }
        if let Err(error) = fs::write(path, self.document.to_string()) {
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

    for (key, target) in [
        ("device", &mut settings.device),
        ("output_device", &mut settings.output_device),
    ] {
        if let Some(name) = get(key).and_then(|value| value.as_str()) {
            *target = Some(name.to_owned());
        }
    }
    // Written as floats, because a shift and a speed are: read as either, so
    // a hand-edited `shift_hz = 170` is not thrown away for having no point
    // in it.
    for (key, target) in [
        ("mark_hz", &mut settings.mark_hz),
        ("shift_hz", &mut settings.shift_hz),
        ("baud", &mut settings.baud),
        ("squelch_threshold", &mut settings.squelch_threshold),
        ("tx_level", &mut settings.tx_level),
    ] {
        if let Some(number) = get(key).and_then(number) {
            *target = number;
        }
    }
    for (key, target) in [
        ("reverse", &mut settings.reverse),
        ("afc", &mut settings.afc),
        ("unshift_on_space", &mut settings.unshift_on_space),
        ("atc", &mut settings.atc),
        ("tx_unshift_on_space", &mut settings.tx_unshift_on_space),
    ] {
        if let Some(flag) = get(key).and_then(|value| value.as_bool()) {
            *target = flag;
        }
    }
    if let Some(station) = table.get("station").and_then(Item::as_table) {
        let field = |key: &str| {
            station
                .get(key)
                .and_then(Item::as_value)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        settings.station = Station {
            callsign: field("callsign"),
            name: field("name"),
            qth: field("qth"),
            grid: field("grid"),
        };
    }
    settings.custom_variables = read_custom_variables(table);
    settings.clamped()
}

/// Reads the operator's own fields.
///
/// A name no `${...}` expression could ever hold is dropped rather than
/// carried around unreachable, which is the treatment every other unusable
/// value in the file gets.
fn read_custom_variables(table: &toml_edit::Table) -> BTreeMap<String, String> {
    let Some(variables) = table.get("variables").and_then(Item::as_table) else {
        return BTreeMap::new();
    };
    variables
        .iter()
        .filter(|(name, _)| valid_variable_name(name))
        .filter_map(|(name, item)| Some((name.to_owned(), item.as_value()?.as_str()?.to_owned())))
        .collect()
}

/// Writes the fields back, leaving the keys that survived in place.
///
/// Keys are assigned rather than the table being rebuilt, so a comment written
/// beside one by hand outlives a save that did not touch it.
fn store_custom_variables(table: &mut Table, variables: &BTreeMap<String, String>) {
    if variables.is_empty() {
        table.remove("variables");
        return;
    }
    let Some(stored) = table
        .entry("variables")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
    else {
        return;
    };
    stored.retain(|name, _| variables.contains_key(name));
    for (name, text) in variables {
        stored[name.as_str()] = value(text.as_str());
    }
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
        assert!(written.contains("mark_hz = 2125.0"));
    }

    #[test]
    fn stored_settings_survive_a_round_trip() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        let wanted = Settings {
            device: Some("Line In".to_owned()),
            output_device: Some("Line Out".to_owned()),
            mark_hz: 1_275.0,
            shift_hz: 850.0,
            baud: 75.0,
            reverse: true,
            afc: false,
            squelch_threshold: 0.4,
            unshift_on_space: false,
            atc: true,
            tx_unshift_on_space: false,
            tx_level: 0.5,
            station: Station {
                callsign: "JL1HIS".to_owned(),
                name: "YU".to_owned(),
                qth: "TOKYO".to_owned(),
                grid: "PM95UQ".to_owned(),
            },
            custom_variables: BTreeMap::from([("grid".to_owned(), "PM95".to_owned())]),
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
        fs::write(&path, "# the station's own note\nunknown = 7\nbaud = 75.0\n").unwrap();

        let (mut config, settings) = Config::load(path.clone());
        assert_eq!(settings.baud, 75.0);
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
        fs::write(&path, "mark_hz = \"low\"\nbaud = nan\nreverse = true\n").unwrap();

        let (_, settings) = Config::load(path);
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
            "mark_hz = 40000.0\nshift_hz = 1.0\nbaud = 9000.0\nsquelch_threshold = 4.0\n",
        )
        .unwrap();

        let (_, settings) = Config::load(path);
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

    /// Editing the file by hand is how macros are written, so the fields they
    /// read have to survive being written and read back.
    #[test]
    fn operator_fields_survive_a_round_trip() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        let wanted = Settings {
            custom_variables: BTreeMap::from([
                ("grid".to_owned(), "PM95UQ".to_owned()),
                ("club".to_owned(), "JARL".to_owned()),
            ]),
            ..Settings::default()
        };
        Config::load(path.clone()).0.save(&wanted);

        let (_, read_back) = Config::load(path);
        assert_eq!(read_back.custom_variables, wanted.custom_variables);
    }

    /// A name no macro could ever name is dropped rather than carried around
    /// unreachable, the way every other unusable value in the file is.
    #[test]
    fn an_unusable_field_name_is_dropped() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            "[variables]
grid = \"PM95UQ\"
\"my grid\" = \"NO\"
",
        )
        .unwrap();

        let (_, settings) = Config::load(path);

        assert_eq!(settings.custom_variables.len(), 1);
        assert_eq!(settings.custom_variables["grid"], "PM95UQ");
    }

    /// A field struck out in the window has to leave the file too.
    #[test]
    fn a_removed_field_leaves_the_file() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        let mut settings = Settings {
            custom_variables: BTreeMap::from([
                ("grid".to_owned(), "PM95UQ".to_owned()),
                ("club".to_owned(), "JARL".to_owned()),
            ]),
            ..Settings::default()
        };
        let (mut config, _) = Config::load(path.clone());
        config.save(&settings);

        settings.custom_variables.remove("club");
        config.save(&settings);

        let (_, read_back) = Config::load(path.clone());
        assert_eq!(read_back.custom_variables.len(), 1);
        assert!(!fs::read_to_string(&path).unwrap().contains("JARL"));
    }

    /// With nothing in it the table goes altogether, rather than an empty
    /// heading being left behind in the operator's file.
    #[test]
    fn no_fields_leaves_no_table() {
        let root = TempDir::new();
        let path = root.path().join("config.toml");
        let mut settings = Settings {
            custom_variables: BTreeMap::from([("grid".to_owned(), "PM95UQ".to_owned())]),
            ..Settings::default()
        };
        let (mut config, _) = Config::load(path.clone());
        config.save(&settings);

        settings.custom_variables.clear();
        config.save(&settings);

        assert!(!fs::read_to_string(&path).unwrap().contains("variables"));
    }
}
