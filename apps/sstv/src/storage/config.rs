//! Settings that survive a restart, and the file holding them.
//!
//! The configuration file is edited in place through a format preserving
//! document, so comments and hand-written layout are not lost when the
//! application saves a changed selection.

use std::{collections::BTreeMap, path::Path};

use grayline_logger_link::LoggerSettings;
use grayline_qso::ContactSettings;
use grayline_shell::config::{self, Document};
use grayline_sstv::mode::Mode;
use grayline_sstv_template::valid_variable_name;
use toml_edit::{Item, Table, value};

use crate::storage::history::HistoryFormat;

/// What the `[rig]` section holds, and the names its ports are reached by.
///
/// Re-exported rather than defined here: the file is the application's, while
/// what the settings in it mean belongs to rig control.
pub use grayline_rig_control::RigSettings;

/// The serial number a fresh log starts from.
///
/// Three digits, which is how a serial is given whatever its value, and how
/// the counted FSKID record carries it.
pub const FIRST_QSO_NUMBER: &str = "001";

/// The receive processing switches the DSP panel offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DspFlags {
    pub afc: bool,
    pub lms_filter: bool,
    pub live_slant: bool,
}

impl Default for DspFlags {
    fn default() -> Self {
        Self {
            afc: true,
            lms_filter: false,
            live_slant: true,
        }
    }
}

pub const DEFAULT_RX_MODE: Mode = Mode::Pd120;
pub const DEFAULT_TX_MODE: Mode = Mode::Scottie2;
/// Transmit level a first run starts at.
///
/// Full scale, because the modulator already produces normalized PCM and the
/// operator's own output mixer is what the level is usually set against.
pub const DEFAULT_TX_VOLUME: f32 = 1.0;

/// Where one setting sits in the file: its table and its key.
///
/// Every key is named once here and read and written through the same
/// constant, so a key renamed on one side cannot leave the other reading the
/// default forever.
#[derive(Clone, Copy)]
struct Key(&'static str, &'static str);

const AUDIO_INPUT_DEVICE: Key = Key("audio", "input-device");
const AUDIO_OUTPUT_DEVICE: Key = Key("audio", "output-device");
const STATION_CALLSIGN: Key = Key("station", "callsign");
const STATION_QTH: Key = Key("station", "qth");
const STATION_GRID: Key = Key("station", "grid");
const QSO_NUMBER: Key = Key("qso", "number");
const LIBRARY_TEMPLATE: Key = Key("library", "template");
const LIBRARY_STOCK: Key = Key("library", "stock");
const RECEIVE_MODE: Key = Key("receive", "mode");
const TRANSMIT_MODE: Key = Key("transmit", "mode");
const RECEIVE_AUTO_VIS: Key = Key("receive", "auto-vis");
const RECEIVE_AFC: Key = Key("receive", "afc");
const RECEIVE_LMS: Key = Key("receive", "lms");
const RECEIVE_SLANT: Key = Key("receive", "slant");
const RECEIVE_VIS_RESTART: Key = Key("receive", "vis-restart");
const RECEIVE_VIS_STRICT: Key = Key("receive", "vis-strict");
const TRANSMIT_FSKID: Key = Key("transmit", "fskid");
const TRANSMIT_CONTEST: Key = Key("transmit", "contest");
const TRANSMIT_VOLUME: Key = Key("transmit", "volume");
const RECEIVE_AUTO_HISTORY: Key = Key("receive", "auto-history");
const RECEIVE_HISTORY_FORMAT: Key = Key("receive", "history-format");

/// Everything the application restores on the next start.
///
/// Selections that name something outside the configuration file, such as a
/// capture device or a library file, are stored by name: the identifiers
/// behind them are assigned per run and mean nothing to a later one.
///
/// The language and the interface scale are not here: they are the same in
/// every application of this family and are kept once, in
/// `grayline_shell::common`.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub station_callsign: String,
    /// Where the station is operating from, in plain words.
    pub station_qth: String,
    /// The station's Maidenhead grid locator.
    pub station_grid: String,
    /// The serial number the QSO panel is counting from.
    ///
    /// Kept across runs because a contest outlives a session: an operator who
    /// restarts the application in the middle of one is still on the number
    /// they had reached. Stored as text, because the field it comes from holds
    /// whatever exchange the contest asks for and its leading zeros are part
    /// of what is sent.
    pub qso_number: String,
    /// Names the operator defined, read by templates as `${custom.<name>}`.
    ///
    /// Ordered so the file the application writes back stays in the order the
    /// operator reads it in, rather than being shuffled on every save.
    pub custom_variables: BTreeMap<String, String>,
    pub template: Option<String>,
    pub stock: Option<String>,
    pub rx_mode: Mode,
    pub tx_mode: Mode,
    pub auto_mode: bool,
    pub dsp: DspFlags,
    pub vis_restart: bool,
    /// Whether a VIS detection requires the header's leader tone as well.
    pub vis_strict: bool,
    pub send_fskid: bool,
    /// Whether the QSO panel's serial number is worked and sent.
    pub contest_mode: bool,
    pub tx_volume: f32,
    pub auto_history: bool,
    pub history_format: HistoryFormat,
    pub rig: RigSettings,
    pub contact: ContactSettings,
    pub logger: LoggerSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            station_callsign: String::new(),
            station_qth: String::new(),
            station_grid: String::new(),
            qso_number: FIRST_QSO_NUMBER.to_owned(),
            custom_variables: BTreeMap::new(),
            template: None,
            stock: None,
            rx_mode: DEFAULT_RX_MODE,
            tx_mode: DEFAULT_TX_MODE,
            auto_mode: true,
            dsp: DspFlags::default(),
            vis_restart: true,
            // Loose detection is MMSSTV's behavior and what weak-signal
            // reception wants; strictness is the opt-in.
            vis_strict: false,
            send_fskid: true,
            contest_mode: false,
            tx_volume: DEFAULT_TX_VOLUME,
            auto_history: true,
            history_format: HistoryFormat::default(),
            rig: RigSettings::default(),
            contact: ContactSettings::default(),
            logger: LoggerSettings::default(),
        }
    }
}

/// The configuration file, kept open as a document across saves.
#[derive(Debug)]
pub struct Config {
    document: Document,
}

impl Config {
    /// Reads `path`, falling back to an empty document.
    ///
    /// Neither a missing nor a malformed file is fatal: the application starts
    /// on defaults and reports why, rather than refusing to run because one
    /// hand-edited value is wrong.
    pub fn load(path: &Path) -> Self {
        Self {
            document: Document::load(path.to_owned()),
        }
    }

    #[cfg(test)]
    pub fn detached() -> Self {
        Self {
            document: Document::detached(),
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.document.error()
    }

    /// Reads the stored settings, substituting the default for anything
    /// missing, mistyped, or unrecognized.
    pub fn settings(&self) -> Settings {
        let defaults = Settings::default();
        let document = self.document.table();
        Settings {
            input_device: owned(document, AUDIO_INPUT_DEVICE),
            output_device: owned(document, AUDIO_OUTPUT_DEVICE),
            station_callsign: owned(document, STATION_CALLSIGN).unwrap_or_default(),
            station_qth: owned(document, STATION_QTH).unwrap_or_default(),
            station_grid: owned(document, STATION_GRID).unwrap_or_default(),
            qso_number: owned(document, QSO_NUMBER).unwrap_or(defaults.qso_number),
            custom_variables: config::read_strings(document, "variables", valid_variable_name),
            template: owned(document, LIBRARY_TEMPLATE),
            stock: owned(document, LIBRARY_STOCK),
            rx_mode: string(document, RECEIVE_MODE)
                .and_then(mode_by_name)
                .unwrap_or(defaults.rx_mode),
            tx_mode: string(document, TRANSMIT_MODE)
                .and_then(mode_by_name)
                .unwrap_or(defaults.tx_mode),
            auto_mode: boolean(document, RECEIVE_AUTO_VIS).unwrap_or(defaults.auto_mode),
            dsp: DspFlags {
                afc: boolean(document, RECEIVE_AFC).unwrap_or(defaults.dsp.afc),
                lms_filter: boolean(document, RECEIVE_LMS).unwrap_or(defaults.dsp.lms_filter),
                live_slant: boolean(document, RECEIVE_SLANT).unwrap_or(defaults.dsp.live_slant),
            },
            vis_restart: boolean(document, RECEIVE_VIS_RESTART).unwrap_or(defaults.vis_restart),
            vis_strict: boolean(document, RECEIVE_VIS_STRICT).unwrap_or(defaults.vis_strict),
            send_fskid: boolean(document, TRANSMIT_FSKID).unwrap_or(defaults.send_fskid),
            contest_mode: boolean(document, TRANSMIT_CONTEST).unwrap_or(defaults.contest_mode),
            tx_volume: float(document, TRANSMIT_VOLUME)
                .map(|volume| volume.clamp(0.0, 1.0))
                .unwrap_or(defaults.tx_volume),
            auto_history: boolean(document, RECEIVE_AUTO_HISTORY).unwrap_or(defaults.auto_history),
            history_format: string(document, RECEIVE_HISTORY_FORMAT)
                .and_then(HistoryFormat::from_config)
                .unwrap_or(defaults.history_format),
            rig: RigSettings::read(document.get("rig").and_then(Item::as_table), RigSettings::default()),
            contact: ContactSettings::read(document.get("qso").and_then(Item::as_table), ContactSettings::default()),
            logger: LoggerSettings::read(
                document.get("logger").and_then(Item::as_table),
                LoggerSettings::default(),
            ),
        }
    }

    /// Writes `settings` back, leaving every unrelated key untouched.
    pub fn store(&mut self, settings: &Settings) {
        self.document.save(|document| {
            set(
                document,
                AUDIO_INPUT_DEVICE,
                settings.input_device.as_deref().map(value),
            );
            set(
                document,
                AUDIO_OUTPUT_DEVICE,
                settings.output_device.as_deref().map(value),
            );
            for (key, text) in [
                (STATION_CALLSIGN, &settings.station_callsign),
                (STATION_QTH, &settings.station_qth),
                (STATION_GRID, &settings.station_grid),
            ] {
                set(document, key, (!text.is_empty()).then(|| value(text)));
            }
            set(document, QSO_NUMBER, Some(value(settings.qso_number.as_str())));
            config::write_strings(document, "variables", &settings.custom_variables);
            set(document, LIBRARY_TEMPLATE, settings.template.as_deref().map(value));
            set(document, LIBRARY_STOCK, settings.stock.as_deref().map(value));
            set(document, RECEIVE_MODE, Some(value(settings.rx_mode.spec().name())));
            set(document, RECEIVE_AUTO_VIS, Some(value(settings.auto_mode)));
            set(document, RECEIVE_AFC, Some(value(settings.dsp.afc)));
            set(document, RECEIVE_LMS, Some(value(settings.dsp.lms_filter)));
            set(document, RECEIVE_SLANT, Some(value(settings.dsp.live_slant)));
            set(document, RECEIVE_VIS_RESTART, Some(value(settings.vis_restart)));
            set(document, RECEIVE_VIS_STRICT, Some(value(settings.vis_strict)));
            set(document, TRANSMIT_FSKID, Some(value(settings.send_fskid)));
            set(document, TRANSMIT_CONTEST, Some(value(settings.contest_mode)));
            set(
                document,
                TRANSMIT_VOLUME,
                Some(value((f64::from(settings.tx_volume) * 100.0).round() / 100.0)),
            );
            set(document, RECEIVE_AUTO_HISTORY, Some(value(settings.auto_history)));
            set(
                document,
                RECEIVE_HISTORY_FORMAT,
                Some(value(settings.history_format.config_name())),
            );
            set(document, TRANSMIT_MODE, Some(value(settings.tx_mode.spec().name())));
            settings.rig.write(table_mut(document, "rig"));
            settings.contact.write(table_mut(document, "qso"));
            settings.logger.write(table_mut(document, "logger"));
        });
    }
}

fn mode_by_name(name: &str) -> Option<Mode> {
    Mode::ALL
        .into_iter()
        .find(|mode| mode.spec().name().eq_ignore_ascii_case(name))
}

fn get(document: &Table, Key(table, key): Key) -> Option<&Item> {
    document.get(table)?.as_table()?.get(key)
}

fn string(document: &Table, key: Key) -> Option<&str> {
    get(document, key)?.as_str()
}

fn owned(document: &Table, key: Key) -> Option<String> {
    string(document, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn boolean(document: &Table, key: Key) -> Option<bool> {
    get(document, key)?.as_bool()
}

/// Reads a number, accepting the integer a hand-edited file may hold.
///
/// TOML also spells `nan` and `inf`, which no setting here can act on: `nan`
/// slips through every clamp and panics the first `Duration` built from it.
/// They read as absent, so the default stands in like it does for any other
/// unusable value.
fn float(document: &Table, key: Key) -> Option<f32> {
    let item = get(document, key)?;
    item.as_float()
        .or_else(|| item.as_integer().map(|value| value as f64))
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
}

fn set(document: &mut Table, Key(table, key): Key, item: Option<Item>) {
    let Some(item) = item else {
        // An absent table stays absent: a setting with nothing to store is not
        // a reason to write out an empty section.
        if let Some(target) = document.get_mut(table).and_then(Item::as_table_mut) {
            target.remove(key);
        }
        return;
    };
    table_mut(document, table)[key] = item;
}

fn table_mut<'a>(document: &'a mut Table, name: &str) -> &'a mut Table {
    let entry = document.entry(name).or_insert(Item::Table(Table::new()));
    if !entry.is_table() {
        *entry = Item::Table(Table::new());
    }
    entry.as_table_mut().expect("the entry holds a table")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use grayline_rig_control::{DEFAULT_ADDRESS, DEFAULT_LEAD_IN_SECONDS, DEFAULT_POLL_SECONDS, PortSettings};
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;

    fn config_path(dir: &TempDir) -> PathBuf {
        dir.path().join(grayline_shell::paths::CONFIG_FILE)
    }

    fn populated() -> Settings {
        Settings {
            input_device: Some("Line In (Interface)".to_owned()),
            output_device: Some("Speakers (Interface)".to_owned()),
            station_callsign: "JA1ABC".to_owned(),
            station_qth: "Chiyoda, Tokyo".to_owned(),
            station_grid: "PM95uq".to_owned(),
            qso_number: "042".to_owned(),
            custom_variables: BTreeMap::from([
                ("club".to_owned(), "JARL".to_owned()),
                ("rig".to_owned(), "FT-991A".to_owned()),
            ]),
            template: Some("field-day.kdl".to_owned()),
            stock: Some("antenna.png".to_owned()),
            rx_mode: Mode::Robot36,
            tx_mode: Mode::Martin1,
            auto_mode: false,
            vis_restart: false,
            vis_strict: true,
            send_fskid: false,
            contest_mode: true,
            tx_volume: 0.5,
            dsp: DspFlags {
                afc: false,
                lms_filter: true,
                live_slant: false,
            },
            auto_history: false,
            history_format: HistoryFormat::Jpeg,
            rig: RigSettings {
                enabled: true,
                ports: BTreeMap::from([
                    (
                        "rig".to_owned(),
                        PortSettings {
                            address: "192.168.0.8:4532".to_owned(),
                        },
                    ),
                    (
                        "amplifier".to_owned(),
                        PortSettings {
                            address: "127.0.0.1:4533".to_owned(),
                        },
                    ),
                ]),
                poll_seconds: 2.5,
                lead_in_seconds: 0.3,
                tail_seconds: 0.1,
            },
            contact: ContactSettings {
                lookup: false,
                wavelog_url: "https://log.example.org".to_owned(),
                fields: vec!["!ja".to_owned(), "dxcc".to_owned()],
            },
            logger: LoggerSettings {
                enabled: true,
                address: "127.0.0.1:2237".to_owned(),
            },
        }
    }

    #[test]
    fn a_missing_file_reads_as_the_defaults() {
        let root = TempDir::new().unwrap();
        let config = Config::load(&config_path(&root));
        assert_eq!(config.settings(), Settings::default());
        assert!(config.error().is_none());
    }

    #[test]
    fn stored_settings_survive_a_reload() {
        let root = TempDir::new().unwrap();
        let settings = populated();

        let mut config = Config::load(&config_path(&root));
        config.store(&settings);
        assert!(config.error().is_none());

        assert_eq!(Config::load(&config_path(&root)).settings(), settings);
    }

    #[test]
    fn a_field_list_is_stored_the_way_it_was_written() {
        let root = TempDir::new().unwrap();
        let mut config = Config::load(&config_path(&root));
        config.store(&populated());

        let written = fs::read_to_string(config_path(&root)).expect("a written file");
        assert!(written.contains("\"!ja\""), "{written}");
        assert_eq!(
            Config::load(&config_path(&root)).settings().contact.fields,
            ["!ja", "dxcc"]
        );
    }

    #[test]
    fn an_empty_field_list_is_taken_as_written() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(config_path(&root).parent().expect("a parent")).expect("a directory");
        fs::write(
            config_path(&root),
            "[qso]
fields = []
",
        )
        .expect("a write");

        assert!(Config::load(&config_path(&root)).settings().contact.fields.is_empty());
    }

    #[test]
    fn a_file_naming_no_fields_asks_for_the_default_ones() {
        let root = TempDir::new().unwrap();

        assert_eq!(
            Config::load(&config_path(&root)).settings().contact.fields,
            ContactSettings::default().fields
        );
    }

    #[test]
    fn every_well_known_contact_key_is_a_name_a_template_can_read() {
        for key in grayline_qso::WELL_KNOWN_KEYS {
            assert!(valid_variable_name(&format!("contact.{key}")), "{key}");
        }
    }

    #[test]
    fn the_qso_section_names_its_instance_even_while_it_is_empty() {
        let root = TempDir::new().unwrap();
        let mut config = Config::load(&config_path(&root));
        config.store(&Settings::default());

        let written = fs::read_to_string(config_path(&root)).expect("a written file");
        assert!(written.contains("[qso.wavelog]"), "{written}");
        assert!(written.contains("url"), "{written}");
    }

    #[test]
    fn the_qso_section_carries_no_key() {
        let root = TempDir::new().unwrap();
        let mut config = Config::load(&config_path(&root));
        config.store(&populated());

        let written = fs::read_to_string(config_path(&root)).expect("a written file");
        assert!(!written.contains("key"), "{written}");
    }

    /// TOML allows `nan`, which passes every range clamp unchanged and would
    /// panic the first `Duration::from_secs_f32` built from it.
    #[rstest]
    #[case("[rig]\nlead-in = nan\n")]
    #[case("[rig]\ntail = -inf\n")]
    #[case("[rig]\npoll = nan\n")]
    #[case("[transmit]\nvolume = nan\n")]
    fn a_non_finite_number_reads_as_the_default(#[case] contents: &str) {
        let root = TempDir::new().unwrap();
        fs::write(config_path(&root), contents).unwrap();

        let settings = Config::load(&config_path(&root)).settings();
        let defaults = Settings::default();
        assert_eq!(settings, defaults);
    }

    #[test]
    fn saving_keeps_comments_and_unrelated_keys() {
        let root = TempDir::new().unwrap();
        fs::write(
            config_path(&root),
            "# rig settings, edited by hand\nrig-port = \"COM3\"\n",
        )
        .unwrap();

        let mut config = Config::load(&config_path(&root));
        config.store(&Settings::default());

        let stored = fs::read_to_string(config_path(&root)).unwrap();
        assert!(stored.contains("# rig settings, edited by hand"));
        assert!(stored.contains("rig-port = \"COM3\""));
        assert!(stored.contains("[receive]"));
    }

    #[test]
    fn a_setting_with_no_value_is_removed_rather_than_emptied() {
        let root = TempDir::new().unwrap();
        let mut config = Config::load(&config_path(&root));
        config.store(&populated());
        config.store(&Settings::default());

        let stored = fs::read_to_string(config_path(&root)).unwrap();
        assert!(!stored.contains("input-device"));
        assert!(!stored.contains("template"));
        assert_eq!(Config::load(&config_path(&root)).settings(), Settings::default());
    }

    #[test]
    fn a_malformed_file_reports_the_error_and_is_not_overwritten() {
        let root = TempDir::new().unwrap();
        fs::write(config_path(&root), "input-device = \n").unwrap();

        let mut config = Config::load(&config_path(&root));
        assert!(config.error().is_some());
        assert_eq!(config.settings(), Settings::default());

        config.store(&populated());
        assert_eq!(fs::read_to_string(config_path(&root)).unwrap(), "input-device = \n");
    }

    #[test]
    fn unrecognized_values_fall_back_to_the_defaults() {
        let root = TempDir::new().unwrap();
        fs::write(
            config_path(&root),
            concat!(
                "[receive]\n",
                "mode = \"Scottie 9\"\n",
                "afc = \"yes\"\n",
                "[library]\n",
                "template = \"   \"\n",
            ),
        )
        .unwrap();

        let config = Config::load(&config_path(&root));
        assert!(config.error().is_none());
        assert_eq!(config.settings(), Settings::default());
    }

    #[test]
    fn the_default_port_is_written_out_under_the_name_the_script_reaches_it_by() {
        let root = TempDir::new().unwrap();
        let mut config = Config::load(&config_path(&root));
        config.store(&Settings::default());

        let stored = fs::read_to_string(config_path(&root)).unwrap();
        assert!(stored.contains("[rig.ports.rig]"), "{stored}");
        assert!(stored.contains(r#"address = "127.0.0.1:4532""#), "{stored}");
    }

    #[test]
    fn every_named_port_is_read_back() {
        let root = TempDir::new().unwrap();
        fs::write(
            config_path(&root),
            concat!(
                "[rig.ports.rig]\n",
                "kind = \"rigctld\"\n",
                "address = \"192.168.0.8:4532\"\n",
                "[rig.ports.amplifier]\n",
                "kind = \"rigctld\"\n",
            ),
        )
        .unwrap();

        let ports = Config::load(&config_path(&root)).settings().rig.ports;

        assert_eq!(
            ports.get("rig"),
            Some(&PortSettings {
                address: "192.168.0.8:4532".to_owned()
            })
        );
        // A port that names no address is still a port; it is where rigctld
        // listens unless it was told otherwise.
        assert_eq!(
            ports.get("amplifier"),
            Some(&PortSettings {
                address: DEFAULT_ADDRESS.to_owned()
            })
        );
    }

    #[test]
    fn an_entry_that_is_not_a_section_is_not_a_port() {
        let root = TempDir::new().unwrap();
        fs::write(config_path(&root), "[rig.ports]\nrig = 3\n").unwrap();

        assert!(Config::load(&config_path(&root)).settings().rig.ports.is_empty());
    }

    #[test]
    fn an_unrecognized_port_key_is_left_alone() {
        let root = TempDir::new().unwrap();
        fs::write(
            config_path(&root),
            "[rig.ports.rig]\nnote = \"shack\"\naddress = \"192.168.0.8:4532\"\n",
        )
        .unwrap();

        let mut config = Config::load(&config_path(&root));
        let settings = config.settings();
        assert_eq!(
            settings.rig.ports.get("rig"),
            Some(&PortSettings {
                address: "192.168.0.8:4532".to_owned()
            })
        );

        config.store(&settings);
        let stored = fs::read_to_string(config_path(&root)).unwrap();
        assert!(stored.contains("note = \"shack\""), "{stored}");
        assert!(stored.contains("192.168.0.8:4532"), "{stored}");
    }

    #[test]
    fn a_ports_key_that_is_not_a_section_leaves_the_default_port() {
        let root = TempDir::new().unwrap();
        fs::write(config_path(&root), "[rig]\nports = 3\n").unwrap();

        assert_eq!(
            Config::load(&config_path(&root)).settings().rig.ports,
            RigSettings::default().ports
        );
    }

    #[rstest]
    #[case("poll-interval = 2.5\n", 2.5, DEFAULT_LEAD_IN_SECONDS)]
    #[case("poll-interval = -4\n", 0.0, DEFAULT_LEAD_IN_SECONDS)]
    #[case("poll-interval = 600\n", 60.0, DEFAULT_LEAD_IN_SECONDS)]
    #[case("lead-in = 99\n", DEFAULT_POLL_SECONDS, 5.0)]
    #[case("lead-in = \"soon\"\n", DEFAULT_POLL_SECONDS, DEFAULT_LEAD_IN_SECONDS)]
    fn rig_timings_are_read_within_range(
        #[case] written: &str,
        #[case] expected_poll: f32,
        #[case] expected_lead_in: f32,
    ) {
        let root = TempDir::new().unwrap();
        fs::write(config_path(&root), format!("[rig]\n{written}")).unwrap();

        let rig = Config::load(&config_path(&root)).settings().rig;

        assert_eq!(rig.poll_seconds, expected_poll);
        assert_eq!(rig.lead_in_seconds, expected_lead_in);
    }

    #[test]
    fn mode_names_are_matched_without_regard_to_case() {
        assert_eq!(mode_by_name("pd120"), Some(Mode::Pd120));
        assert_eq!(mode_by_name("SCOTTIE 2"), Some(Mode::Scottie2));
        assert_eq!(mode_by_name("Scottie2"), None);
    }

    #[test]
    fn a_conflicting_table_name_is_replaced() {
        let root = TempDir::new().unwrap();
        fs::write(config_path(&root), "receive = 3\n").unwrap();

        let mut config = Config::load(&config_path(&root));
        config.store(&populated());

        assert_eq!(Config::load(&config_path(&root)).settings(), populated());
    }
}
