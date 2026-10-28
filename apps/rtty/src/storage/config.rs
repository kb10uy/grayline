use std::{collections::BTreeMap, path::PathBuf};

use grayline_keyer_protocol::DEFAULT_ADDRESS as DEFAULT_KEYER_ADDRESS;
use grayline_logger_link::LoggerSettings;
use grayline_qso::{ContactSettings, DEFAULT_FIELDS};
use grayline_shell::config::{self, Document};
use toml_edit::{Item, Table, value};

use crate::app::macros::{Station, valid_variable_name};

/// What the `[rig]` section holds, and the names its ports are reached by.
///
/// Re-exported rather than defined here: the file is the application's, while
/// what the settings in it mean belongs to rig control. The section is spelled
/// as the SSTV application spells it, so a station that keys both can copy one
/// block between the two files.
pub use grayline_rig_control::RigSettings;

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

/// The top-level keys, each named once so the reader and the writer cannot
/// spell one differently.
const DEVICE: &str = "device";
const OUTPUT_DEVICE: &str = "output_device";
const MARK_HZ: &str = "mark_hz";
const SHIFT_HZ: &str = "shift_hz";
const BAUD: &str = "baud";
const REVERSE: &str = "reverse";
const AFC: &str = "afc";
const SQUELCH_THRESHOLD: &str = "squelch_threshold";
const UNSHIFT_ON_SPACE: &str = "unshift_on_space";
const ATC: &str = "atc";
const TX_UNSHIFT_ON_SPACE: &str = "tx_unshift_on_space";
const TX_LEVEL: &str = "tx_level";
const SCOPE: &str = "scope";

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
    /// Where the contact directory looks a callsign up.
    pub contact: ContactSettings,
    /// Whether, and where, the contact being worked is sent to the logger.
    pub logger: LoggerSettings,
    /// How the station's rig is keyed, if it is keyed at all.
    pub rig: RigSettings,
    /// Whether a transmission keys the rig's FSK input through `gl-keyer`
    /// rather than being played as audio.
    pub keyer: KeyerSettings,
    /// Whether the scope window was open.
    ///
    /// A display rather than a receiver setting, and remembered for the same
    /// reason the device is: an operator who works with it open should not
    /// have to open it again every session.
    pub scope: bool,
}

/// The contact directory a first run starts on.
///
/// The Latin spellings are offered as well as the core three, which is where
/// this parts from SSTV. ITA2 carries no kanji, so the name this mode can
/// actually send is the one `!latin` keeps; a station that files only a kanji
/// name has nothing a macro could put on the air.
fn default_contact() -> ContactSettings {
    ContactSettings {
        fields: DEFAULT_FIELDS
            .iter()
            .copied()
            .chain(["!latin"])
            .map(str::to_owned)
            .collect(),
        ..ContactSettings::default()
    }
}

/// Where a transmission goes when it is keyed as FSK rather than played.
///
/// Written under `[keyer]` and, like `[rig]`, edited only in the file: which
/// line the rig's FSK input hangs off is wired once for a station and then
/// left alone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyerSettings {
    /// Whether a transmission is streamed to the keyer in place of the audio.
    pub enabled: bool,
    /// Where `gl-keyer serve` listens.
    pub address: String,
}

impl Default for KeyerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            address: DEFAULT_KEYER_ADDRESS.to_owned(),
        }
    }
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
            contact: default_contact(),
            logger: LoggerSettings::default(),
            rig: RigSettings {
                // Nothing here reads a frequency back yet, so the poll starts
                // off rather than at the interval the SSTV application uses.
                poll_seconds: 0.0,
                ..RigSettings::default()
            },
            keyer: KeyerSettings::default(),
            scope: false,
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
    document: Document,
}

impl Config {
    /// Reads the settings file, falling back to defaults it will not save over.
    pub fn load(path: PathBuf) -> (Self, Settings) {
        let document = Document::load(path);
        let settings = read(document.table());
        (Self { document }, settings)
    }

    /// A configuration with nowhere to write, for tests and headless runs.
    #[cfg(test)]
    pub fn detached() -> Self {
        Self {
            document: Document::detached(),
        }
    }

    /// Returns what went wrong reading or writing the file, if anything did.
    pub fn error(&self) -> Option<&str> {
        self.document.error()
    }

    /// Writes `settings` into the document and saves it.
    pub fn save(&mut self, settings: &Settings) {
        self.document.save(|table| {
            for (key, name) in [(DEVICE, &settings.device), (OUTPUT_DEVICE, &settings.output_device)] {
                match name {
                    Some(name) => table[key] = value(name.as_str()),
                    None => {
                        table.remove(key);
                    }
                }
            }
            table[MARK_HZ] = value(settings.mark_hz);
            table[SHIFT_HZ] = value(settings.shift_hz);
            table[BAUD] = value(settings.baud);
            table[REVERSE] = value(settings.reverse);
            table[AFC] = value(settings.afc);
            table[SQUELCH_THRESHOLD] = value(settings.squelch_threshold);
            table[UNSHIFT_ON_SPACE] = value(settings.unshift_on_space);
            table[ATC] = value(settings.atc);
            table[TX_UNSHIFT_ON_SPACE] = value(settings.tx_unshift_on_space);
            table[TX_LEVEL] = value(settings.tx_level);
            table[SCOPE] = value(settings.scope);

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
            config::write_strings(table, "variables", &settings.custom_variables);
            if let Some(qso) = table
                .entry("qso")
                .or_insert_with(|| Item::Table(Table::new()))
                .as_table_mut()
            {
                settings.contact.write(qso);
            }
            if let Some(logger) = table
                .entry("logger")
                .or_insert_with(|| Item::Table(Table::new()))
                .as_table_mut()
            {
                settings.logger.write(logger);
            }
            if let Some(rig) = table
                .entry("rig")
                .or_insert_with(|| Item::Table(Table::new()))
                .as_table_mut()
            {
                settings.rig.write(rig);
            }
            store_keyer(table, &settings.keyer);
        });
    }
}

/// Reads what the application understands, leaving everything else alone.
///
/// A value that is the wrong type, or names something this build does not
/// support, is dropped rather than rejected: the file is the operator's, and
/// one unusable key is not a reason to start on nothing.
fn read(table: &Table) -> Settings {
    let mut settings = Settings::default();
    let get = |key: &str| table.get(key).and_then(Item::as_value);

    for (key, target) in [
        (DEVICE, &mut settings.device),
        (OUTPUT_DEVICE, &mut settings.output_device),
    ] {
        if let Some(name) = get(key).and_then(|value| value.as_str()) {
            *target = Some(name.to_owned());
        }
    }
    // Written as floats, because a shift and a speed are: read as either, so
    // a hand-edited `shift_hz = 170` is not thrown away for having no point
    // in it.
    for (key, target) in [
        (MARK_HZ, &mut settings.mark_hz),
        (SHIFT_HZ, &mut settings.shift_hz),
        (BAUD, &mut settings.baud),
        (SQUELCH_THRESHOLD, &mut settings.squelch_threshold),
        (TX_LEVEL, &mut settings.tx_level),
    ] {
        if let Some(number) = get(key).and_then(number) {
            *target = number;
        }
    }
    for (key, target) in [
        (REVERSE, &mut settings.reverse),
        (AFC, &mut settings.afc),
        (UNSHIFT_ON_SPACE, &mut settings.unshift_on_space),
        (ATC, &mut settings.atc),
        (TX_UNSHIFT_ON_SPACE, &mut settings.tx_unshift_on_space),
        (SCOPE, &mut settings.scope),
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
    settings.custom_variables = config::read_strings(table, "variables", valid_variable_name);
    settings.contact = ContactSettings::read(table.get("qso").and_then(Item::as_table), Settings::default().contact);
    settings.logger = LoggerSettings::read(table.get("logger").and_then(Item::as_table), Settings::default().logger);
    settings.rig = RigSettings::read(table.get("rig").and_then(Item::as_table), Settings::default().rig);
    settings.keyer = read_keyer(table);
    settings.clamped()
}

/// Reads the `[keyer]` section.
fn read_keyer(table: &Table) -> KeyerSettings {
    let defaults = KeyerSettings::default();
    let Some(keyer) = table.get("keyer").and_then(Item::as_table) else {
        return defaults;
    };
    KeyerSettings {
        enabled: keyer
            .get("enabled")
            .and_then(Item::as_value)
            .and_then(|flag| flag.as_bool())
            .unwrap_or(defaults.enabled),
        address: keyer
            .get("address")
            .and_then(Item::as_value)
            .and_then(|address| address.as_str())
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map_or(defaults.address, str::to_owned),
    }
}

/// Writes the `[keyer]` section back, at its defaults too, for the reason the
/// `[rig]` section is.
fn store_keyer(table: &mut Table, keyer: &KeyerSettings) {
    let section = table
        .entry("keyer")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut();
    let Some(section) = section else { return };
    section["enabled"] = value(keyer.enabled);
    section["address"] = value(keyer.address.as_str());
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
    use std::fs;

    use grayline_rig_control::PortSettings;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_missing_file_starts_on_defaults_and_can_be_saved() {
        let root = TempDir::new().unwrap();
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
        let root = TempDir::new().unwrap();
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
            contact: ContactSettings {
                lookup: false,
                wavelog_url: "https://log.example/".to_owned(),
                fields: vec!["!ja".to_owned(), "note".to_owned()],
            },
            logger: LoggerSettings {
                enabled: true,
                address: "127.0.0.1:2237".to_owned(),
            },
            rig: RigSettings {
                enabled: true,
                ports: BTreeMap::from([
                    (
                        "rig".to_owned(),
                        PortSettings {
                            address: "127.0.0.1:4532".to_owned(),
                        },
                    ),
                    (
                        "amplifier".to_owned(),
                        PortSettings {
                            address: "192.168.0.8:4532".to_owned(),
                        },
                    ),
                ]),
                poll_seconds: 0.0,
                lead_in_seconds: 0.4,
                tail_seconds: 0.1,
            },
            keyer: KeyerSettings {
                enabled: true,
                address: "192.168.0.8:4534".to_owned(),
            },
            scope: true,
        };
        Config::load(path.clone()).0.save(&wanted);

        let (_, read_back) = Config::load(path);
        assert_eq!(read_back, wanted);
    }

    #[test]
    fn the_rig_section_is_read_as_the_sstv_application_writes_it() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            concat!(
                "[rig]
",
                "enabled = true
",
                "lead-in = 99
",
                "tail = 0.08
",
                "poll-interval = 2
",
                "[rig.ports.rig]
",
                "address = \"192.168.0.8:4532\"
",
            ),
        )
        .unwrap();

        let rig = Config::load(path).1.rig;

        assert!(rig.enabled);
        // Clamped where it is read, as every other hand-edited figure is.
        assert_eq!(rig.lead_in_seconds, *grayline_rig_control::KEYING_SECONDS_RANGE.end());
        assert_eq!(rig.tail_seconds, 0.08);
        assert_eq!(rig.poll_seconds, 2.0);
        assert_eq!(
            rig.ports.get("rig"),
            Some(&PortSettings {
                address: "192.168.0.8:4532".to_owned()
            })
        );
    }

    /// The keys have no editor, so the file is where they are set up: a
    /// station that never touched them should still find them written out.
    #[test]
    fn a_first_save_writes_the_keying_settings_out() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        let (mut config, settings) = Config::load(path.clone());

        config.save(&settings);

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("[rig]"), "{written}");
        assert!(written.contains("lead-in = 0.2"), "{written}");
        assert!(written.contains("[rig.ports.rig]"), "{written}");
        assert!(written.contains("[keyer]"), "{written}");
        assert!(written.contains("address = \"127.0.0.1:4534\""), "{written}");
    }

    #[test]
    fn a_blank_keyer_address_falls_back_to_the_default() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "[keyer]\nenabled = true\naddress = \"  \"\n").unwrap();

        let keyer = Config::load(path).1.keyer;

        assert!(keyer.enabled);
        assert_eq!(keyer.address, KeyerSettings::default().address);
    }

    #[test]
    fn saving_preserves_comments_and_unknown_keys() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "# the station's own note\nunknown = 7\nbaud = 75.0\n").unwrap();

        let (mut config, settings) = Config::load(path.clone());
        assert_eq!(settings.baud, 75.0);
        config.save(&settings);

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("# the station's own note"));
        assert!(written.contains("unknown = 7"));
    }

    #[test]
    fn an_unparsable_file_is_never_written_over() {
        let root = TempDir::new().unwrap();
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
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "mark_hz = \"low\"\nbaud = nan\nreverse = true\n").unwrap();

        let (_, settings) = Config::load(path);
        assert_eq!(settings.mark_hz, Settings::default().mark_hz);
        assert_eq!(settings.baud, Settings::default().baud);
        assert!(settings.reverse);
    }

    #[test]
    fn stored_figures_are_brought_back_into_range() {
        let root = TempDir::new().unwrap();
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

    #[test]
    fn a_whole_number_is_read_as_the_figure_it_is() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "shift_hz = 425\nbaud = 75\n").unwrap();

        let (_, settings) = Config::load(path);
        assert_eq!(settings.shift_hz, 425.0);
        assert_eq!(settings.baud, 75.0);
    }

    #[test]
    fn operator_fields_survive_a_round_trip() {
        let root = TempDir::new().unwrap();
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

    #[test]
    fn an_unusable_field_name_is_dropped() {
        let root = TempDir::new().unwrap();
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

    #[test]
    fn a_removed_field_leaves_the_file() {
        let root = TempDir::new().unwrap();
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

    #[test]
    fn no_fields_leaves_no_table() {
        let root = TempDir::new().unwrap();
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
    #[test]
    fn a_file_that_says_nothing_about_the_directory_gets_the_defaults() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "baud = 75.0\n").unwrap();

        let (_, settings) = Config::load(path);

        assert_eq!(settings.contact, default_contact());
    }

    /// ITA2 has no kanji, so the spelling this mode can send is one the dialog has
    /// to offer somewhere to put.
    #[test]
    fn the_latin_spellings_are_offered_by_default() {
        let offered = grayline_qso::expand_fields(default_contact().fields.iter().map(String::as_str));

        for key in ["name", "name_latin", "qth", "qth_latin"] {
            assert!(offered.iter().any(|field| field == key), "{key} is not offered");
        }
    }

    #[test]
    fn an_empty_field_list_is_taken_as_written() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "[qso]\nfields = []\n").unwrap();

        let (_, settings) = Config::load(path);

        assert!(settings.contact.fields.is_empty());
    }

    #[test]
    fn the_logger_address_is_written_out_even_while_it_is_empty() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("config.toml");
        let (mut config, settings) = Config::load(path.clone());

        config.save(&settings);

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("[qso.wavelog]"), "{written}");
        assert!(written.contains("url = \"\""), "{written}");
    }

    #[test]
    fn every_well_known_contact_key_is_a_name_a_macro_can_read() {
        for key in grayline_qso::WELL_KNOWN_KEYS {
            assert!(valid_variable_name(&format!("contact.{key}")), "{key}");
        }
    }
}
