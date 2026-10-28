use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use grayline_audio::{InputDevice, OutputDevice, PlaybackWriter, StreamFault};
use grayline_sstv::{
    image::RgbImage,
    mode::{Mode, Support},
};
use grayline_sstv_fskid::{FskId, FskNumber};
use grayline_sstv_template::valid_variable_name;
use jiff::{Timestamp, Zoned};

use grayline_logger_link::{FormUpdate, LoggerLink};
use grayline_qso::{ContactEditor, ContactPaths};
use grayline_sstv_rx::SyncStart;

use grayline_rig_control::{
    Reading, RigControlError, RigSnapshot, RigState, RigWorker,
    bands::{BandDefinition, BandPlan},
    script,
};
use grayline_shell::{
    common::{CommonConfig, CommonSettings, UI_SCALE_RANGE},
    i18n::{I18n, Locale, owned},
    platform::{self, Activity, Platform},
    worker::Waker,
};

use crate::{
    storage::{
        config::{Config, Settings},
        paths::{AppPaths, Folder},
    },
    ui::raster::{Raster, test_pattern_image},
    worker::{
        audio::AudioState,
        compose::{ComposeRequest, Composer},
        receive::{Frame, RxProgress, RxSettings},
        transmit::{Identification, TUNE_FREQUENCY_HZ, TUNE_LIMIT, TxGain, TxProgress, TxWorker},
    },
};

pub mod outgoing;

use outgoing::{Ending, Kind, Outgoing};

pub use crate::storage::config::{DspFlags, FIRST_QSO_NUMBER};
pub use crate::storage::library::Entry;
use crate::storage::library::{LibraryScan, stock_entries, template_entries};

const PLAYBACK_QUEUE_SAMPLES: usize = 48_000;

const LIVE_INTERVAL: Duration = Duration::from_millis(33);

const COMPOSE_POLL: Duration = Duration::from_millis(100);

/// How often the interface looks at what nothing reports on its own.
///
/// A device that stops is left on the capture stream rather than published,
/// and rig control answers on its own schedule; neither is worth a faster
/// frame than the operator would notice one at.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tab {
    #[default]
    Receive,
    Transmit,
}

impl Tab {
    pub const ALL: [Self; 2] = [Self::Receive, Self::Transmit];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Receive => "tab-receive",
            Self::Transmit => "tab-transmit",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dsp {
    Afc,
    Lms,
    Slant,
}

impl Dsp {
    pub const ALL: [Self; 3] = [Self::Afc, Self::Lms, Self::Slant];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Afc => "dsp-afc",
            Self::Lms => "dsp-lms",
            Self::Slant => "dsp-slant",
        }
    }
}

/// How the interface reaches each persisted flag, kept beside the panel that
/// shows them rather than with the storage the flags live in.
impl DspFlags {
    pub const fn get(self, dsp: Dsp) -> bool {
        match dsp {
            Dsp::Afc => self.afc,
            Dsp::Lms => self.lms_filter,
            Dsp::Slant => self.live_slant,
        }
    }

    const fn toggle(&mut self, dsp: Dsp) {
        match dsp {
            Dsp::Afc => self.afc = !self.afc,
            Dsp::Lms => self.lms_filter = !self.lms_filter,
            Dsp::Slant => self.live_slant = !self.live_slant,
        }
    }
}

pub const DEFAULT_RSV: &str = "595";
/// The report the identifier's contest number is read as.
///
/// Only the number travels over the air; the readability and strength in front
/// of it are what MMSSTV assumes of a station that got through at all.
const RECEIVED_REPORT: &str = "595";

#[derive(Clone, Debug)]
pub struct Qso {
    pub call: String,
    /// The report the other station gave, which `${report.received}` reads.
    pub rsv_received: String,
    pub rsv: String,
    /// The serial number being sent.
    ///
    /// Text rather than a count, because the buttons under it are a
    /// convenience rather than the only way it is worked: an exchange that is
    /// not a plain serial is typed here too.
    pub number: String,
}

impl Default for Qso {
    fn default() -> Self {
        Self {
            call: String::new(),
            rsv_received: String::new(),
            rsv: DEFAULT_RSV.to_owned(),
            number: FIRST_QSO_NUMBER.to_owned(),
        }
    }
}

pub struct App {
    pub tab: Tab,
    pub i18n: I18n,
    pub audio: AudioState,
    /// What the operator chose, as it is saved.
    ///
    /// The panels and the menus edit this directly. The station, the serial
    /// number, the library selection, and the devices are kept where they are
    /// worked with and copied in when the settings are persisted. Only whether
    /// to connect the rig, whether to look a contact up, and whether to send
    /// it to the logger are the interface's to change: the rest of those
    /// sections belongs to the station and is edited in the configuration
    /// file.
    pub settings: Settings,
    pub rx_modes: Vec<Mode>,
    pub tx_modes: Vec<Mode>,
    pub qso: Qso,
    pub station: Station,
    pub station_dialog_open: bool,
    pub variables_dialog_open: bool,
    /// The rows that dialog is editing.
    ///
    /// Kept as a list rather than edited in the map directly, because a map
    /// reorders itself the moment a name is typed into and the row under the
    /// cursor would move out from under it.
    pub variables_draft: Vec<(String, String)>,
    pub library: Library,
    library_scan: Option<mpsc::Receiver<LibraryScan>>,
    /// Receptions still being written out, joined before the interface goes.
    history_writers: Vec<thread::JoinHandle<()>>,
    /// A success the status line reports, as opposed to a failure: writing
    /// out the rig script or the band plan says where the file went.
    pub notice: Option<String>,
    /// The last failure doing something on the operator's behalf outside the
    /// application: opening a folder or the manual, writing a file out, or
    /// reading the band plan. Shown on the status line.
    pub error: Option<String>,
    pub rx_raster: Raster,
    pub tx_raster: Raster,
    /// The transmission going out, and how the last one ended.
    pub outgoing: Outgoing,
    pub tx_error: Option<String>,
    /// The device fault waiting to be acknowledged, if one is.
    ///
    /// A lost device is reported in front of the interface rather than on the
    /// status line: it stops reception outright, and the operator has to
    /// choose a device before anything works again.
    pub device_fault: Option<StreamFault>,
    /// How much the whole interface is scaled.
    ///
    /// Held here rather than read from egui because it has to be restored
    /// before the first frame is laid out.
    pub ui_scale: f32,
    /// The contact directory, what it last answered, and the dialog editing it.
    pub directory: ContactEditor,
    /// Where the QSO panel is sent for the operator's logger to fill its entry
    /// form from, and where that form comes back from.
    logger: LoggerLink,
    /// The received report and exchange as the logger last held them.
    ///
    /// The panel holds them as one field, and splitting it again reads an
    /// exchange the logger holds without a report, `012`, as a report. While
    /// the field still says what the logger said, the logger's own pair is
    /// what is sent.
    logger_received: Option<(String, String)>,
    /// What rig control last reported, read once per frame.
    pub rig_snapshot: RigSnapshot,
    /// Where each band is, and what the script does on it.
    ///
    /// Shared with the worker rather than copied into it, because the plan is
    /// read once and the interface and the script both work from the same one.
    pub bands: Arc<BandPlan>,
    paths: AppPaths,
    config: Config,
    /// The settings as last written to disk.
    ///
    /// An immediate mode interface mutates state in place, so there is no
    /// message to hang a save off. Comparing against this at the end of the
    /// frame catches every change without the interface having to remember to
    /// announce one.
    saved: Settings,
    /// The language and the scale, which the whole family shares.
    common: CommonConfig,
    composition: Composition,
    rig_worker: Option<RigWorker>,
    /// How many decoded station identifiers have reached the QSO panel.
    ///
    /// The worker republishes every identifier it has decoded on each
    /// snapshot, so the count is what distinguishes a new arrival from the
    /// same list observed again.
    adopted_callsigns: usize,
    /// How many decoded contest numbers have reached the QSO panel, followed
    /// the same way as the identifiers beside them.
    adopted_numbers: usize,
    /// The receive session those counts were taken in.
    ///
    /// A reopened device starts a new worker with empty lists, and a count
    /// carried over from the old session would swallow as many new arrivals
    /// as it had already adopted.
    adopted_session: u64,
    platform: Box<dyn Platform>,
    /// What the platform was last told the application is doing.
    ///
    /// Compared against each frame so a transition is reported once instead of
    /// restated on every frame.
    activity: Activity,
}

/// Set once for the operator rather than per contact, which is why it is
/// edited in a dialog of its own instead of in the panel beside the image.
pub struct Station {
    pub callsign: String,
    pub qth: String,
    pub grid: String,
}

/// The lists the operator picks a template and a background from.
///
/// The selections are indices rather than names because the lists are rebuilt
/// from the directories they name, and what is stored between runs is the name
/// the index stood for.
pub struct Library {
    pub templates: Vec<Entry>,
    pub template: Option<usize>,
    pub stocks: Vec<Entry>,
    pub stock: Option<usize>,
    /// The last failure reading either directory, shown on the status line.
    pub error: Option<String>,
}

/// The composed transmit frame, and everything it was composed from.
///
/// A frame is only true for the moment it was made in: it can print the clock
/// and what the rig is tuned to, so what it was made from is kept beside it
/// and compared against each frame to decide whether to compose again.
struct Composition {
    composer: Composer,
    generation: u64,
    template_generation: u64,
    stock_generation: u64,
    /// The last reception worth keeping, for `rximage` template layers.
    ///
    /// Held in memory rather than read back from the received folder: the
    /// layer shows what was just received, which is true whether or not the
    /// operator saves receptions at all. It starts as a test pattern so a
    /// template built around a reception composes before the first one
    /// arrives.
    received_image: Arc<RgbImage>,
    /// When that image was adopted, which `${rx.timestamp...}` reads.
    ///
    /// The test pattern counts as adopted at startup, so the variable resolves
    /// from the first launch for the same reason the layer itself shows
    /// something before any reception has arrived.
    received_at: Zoned,
    /// The composed image the transmit tab shows, and the one a transmission
    /// starting now would send.
    frame: Option<Arc<RgbImage>>,
    /// Set when something changed the composition while a transmission was
    /// running, so the change is made once the transmission ends.
    deferred: bool,
    /// Set while the worker has been asked for a frame and has not answered.
    ///
    /// The composer has no way to ask for a repaint of its own, so this is what
    /// keeps the interface looking until the frame it asked for arrives.
    pending: bool,
    /// Whether the composed frame shows the time, and the minute it was
    /// composed in.
    ///
    /// A frame that prints the clock is only true for the minute it was made
    /// in, so the pair together say when it has to be made again.
    shows_clock: bool,
    minute: i64,
    /// Whether the composed frame prints what the rig is tuned to, and what it
    /// was tuned to when it was composed.
    ///
    /// The pair says the same thing about the rig as the two above say about
    /// the clock: a frame showing a frequency is only true while the rig is
    /// still on it.
    shows_frequency: bool,
    reading: Option<Reading>,
}

/// The two settings files an application is built out of.
///
/// Its own, and the one the whole family shares. Passed together because they
/// are read together and saved together, and because a builder with one
/// parameter for each of them is a builder nobody can read the call to.
pub(crate) struct Stored {
    pub config: Config,
    pub common: CommonConfig,
}

impl App {
    pub fn new(paths: AppPaths, waker: Waker) -> Self {
        let config = Config::load(paths.config_file());
        let settings = config.settings();
        let audio = AudioState::new(
            settings.input_device.as_deref(),
            settings.output_device.as_deref(),
            rx_settings(&settings),
            waker.clone(),
        );
        let mut app = Self::from_parts(
            audio,
            paths,
            Stored {
                config,
                common: CommonConfig::discover(),
            },
            &settings,
            ContactPaths {
                store: grayline_qso::default_store_path(),
                credentials: grayline_qso::default_credentials_path(),
            },
            waker,
            platform::host(),
        );
        app.open_logger();
        app.refresh_library();
        app.restore_selection(&settings);
        app.request_composition();
        app.sync_settings();
        app.saved = app.settings.clone();
        app
    }

    #[cfg(test)]
    pub(crate) fn headless() -> Self {
        Self::headless_on(Box::new(platform::QuietPlatform))
    }

    /// Builds a headless interface reporting activity to `platform`.
    ///
    /// The directories are under the suite's scratch directory rather than
    /// empty. An empty path is relative, so anything the interface wrote
    /// landed in the working directory, which for these tests is the package's
    /// own source tree.
    #[cfg(test)]
    pub(crate) fn headless_on(platform: Box<dyn Platform>) -> Self {
        let scratch = crate::test_util::scratch_dir();
        let settings = Settings {
            logger: crate::test_util::logger_sink(),
            ..Settings::default()
        };
        Self::from_parts(
            AudioState::disconnected(rx_settings(&settings)),
            AppPaths::from_roots(grayline_shell::paths::Roots::under(scratch)),
            Stored {
                config: Config::detached(),
                common: CommonConfig::detached(),
            },
            &settings,
            // Nothing named, so the store is held in memory and no logger is
            // reached: the suite must neither write into the directory the
            // operator's applications share nor put a question to the logger
            // whose key sits beside it.
            ContactPaths::default(),
            Waker::default(),
            platform,
        )
    }

    fn from_parts(
        audio: AudioState,
        paths: AppPaths,
        Stored { config, common }: Stored,
        settings: &Settings,
        contact_paths: ContactPaths,
        waker: Waker,
        platform: Box<dyn Platform>,
    ) -> Self {
        let (bands, bands_error) = BandPlan::load(paths.config_dir());
        let shared = common.settings();
        Self {
            tab: Tab::default(),
            i18n: I18n::new(shared.locale, &crate::locales::CATALOG),
            audio,
            settings: settings.clone(),
            rx_modes: modes(|mode| mode.spec().decode_support()),
            tx_modes: modes(|mode| mode.spec().encode_support()),
            qso: Qso {
                number: settings.qso_number.trim().to_owned(),
                ..Qso::default()
            },
            station: Station {
                callsign: settings.station_callsign.trim().to_ascii_uppercase(),
                qth: settings.station_qth.trim().to_owned(),
                grid: settings.station_grid.trim().to_owned(),
            },
            station_dialog_open: false,
            variables_dialog_open: false,
            variables_draft: Vec::new(),
            library: Library {
                templates: Vec::new(),
                template: None,
                stocks: Vec::new(),
                stock: None,
                error: None,
            },
            library_scan: None,
            history_writers: Vec::new(),
            notice: None,
            error: bands_error,
            rx_raster: Raster::blank(settings.rx_mode),
            tx_raster: Raster::test_pattern(settings.tx_mode),
            outgoing: Outgoing::new(settings.tx_volume),
            tx_error: None,
            device_fault: None,
            ui_scale: shared.ui_scale,
            directory: ContactEditor::new(&settings.contact, contact_paths, "grayline-sstv-contact".to_owned(), {
                let waker = waker.clone();
                move || waker.wake()
            }),
            logger: LoggerLink::new("grayline-sstv-logger", move || waker.wake()),
            logger_received: None,
            rig_snapshot: RigSnapshot::default(),
            bands: Arc::new(bands),
            paths,
            config,
            saved: settings.clone(),
            common,
            composition: Composition {
                composer: Composer::spawn(),
                generation: 0,
                template_generation: 0,
                stock_generation: 0,
                received_image: Arc::new(test_pattern_image(settings.rx_mode)),
                received_at: Zoned::now(),
                frame: None,
                deferred: false,
                pending: false,
                shows_clock: false,
                minute: current_minute(),
                shows_frequency: false,
                reading: None,
            },
            rig_worker: None,
            adopted_callsigns: 0,
            adopted_numbers: 0,
            adopted_session: 0,
            platform,
            activity: Activity::default(),
        }
    }

    pub fn title(&self) -> String {
        crate::identity::IDENTITY.title()
    }

    /// Reselects the library entries named by `settings`.
    ///
    /// A name that no longer exists leaves the selection the library scan
    /// already made, so a deleted file does not empty the panel.
    fn restore_selection(&mut self, settings: &Settings) {
        self.library.template =
            index_of(&self.library.templates, settings.template.as_deref()).or(self.library.template);
        self.library.stock = index_of(&self.library.stocks, settings.stock.as_deref()).or(self.library.stock);
    }

    /// Copies what is kept elsewhere into the settings: the devices that are
    /// actually open, the station, the serial number, and the library
    /// selection.
    fn sync_settings(&mut self) {
        self.settings.input_device = self.audio.device.as_ref().map(|device| device.name().to_owned());
        self.settings.output_device = self.audio.output_device.as_ref().map(|device| device.name().to_owned());
        self.settings.station_callsign.clone_from(&self.station.callsign);
        self.settings.station_qth.clone_from(&self.station.qth);
        self.settings.station_grid.clone_from(&self.station.grid);
        self.settings.qso_number.clone_from(&self.qso.number);
        self.settings.template = selected_name(&self.library.templates, self.library.template);
        self.settings.stock = selected_name(&self.library.stocks, self.library.stock);
    }

    /// Writes the settings back when anything the interface owns has changed.
    ///
    /// Called once at the end of every frame; a frame that changed nothing
    /// does not touch the disk. Both files: the shared one skips a write that
    /// would change nothing itself, because the other applications write to it
    /// too.
    pub fn persist(&mut self) {
        self.common.save(&CommonSettings {
            locale: self.i18n.locale(),
            ui_scale: self.ui_scale,
        });
        self.sync_settings();
        if self.settings != self.saved {
            self.config.store(&self.settings);
            self.saved = self.settings.clone();
        }
    }

    /// What went wrong reading either settings file, if anything did.
    ///
    /// The first of the two is the one reported: a status line carrying both
    /// would say the same sentence twice, and either one is reason enough to
    /// look at what is on disk.
    pub fn config_error(&self) -> Option<&str> {
        self.config.error().or_else(|| self.common.error())
    }

    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end());
    }

    /// Steps the scale, rounded so repeated steps stay on tidy values.
    pub fn zoom_by(&mut self, step: f32) {
        self.set_ui_scale(((self.ui_scale + step) * 10.0).round() / 10.0);
    }

    pub fn select_locale(&mut self, locale: Locale) {
        if locale != self.i18n.locale() {
            self.i18n = I18n::new(locale, &crate::locales::CATALOG);
        }
    }

    pub fn select_device(&mut self, device: InputDevice) {
        self.audio.select(device);
    }

    pub fn select_output_device(&mut self, device: OutputDevice) {
        self.audio.select_output(device);
    }

    pub fn select_device_named(&mut self, name: &str) {
        if let Some(device) = self.audio.devices.iter().find(|device| device.name() == name).cloned() {
            self.select_device(device);
        }
    }

    pub fn select_output_device_named(&mut self, name: &str) {
        if let Some(device) = self
            .audio
            .output_devices
            .iter()
            .find(|device| device.name() == name)
            .cloned()
        {
            self.select_output_device(device);
        }
    }

    pub fn select_rx_mode(&mut self, mode: Mode) {
        if mode != self.settings.rx_mode {
            self.settings.rx_mode = mode;
            self.rx_raster = Raster::blank(mode);
        }
    }

    pub fn select_tx_mode(&mut self, mode: Mode) {
        if mode != self.settings.tx_mode {
            self.settings.tx_mode = mode;
            self.tx_raster = Raster::test_pattern(mode);
            self.composition.frame = None;
            self.request_composition();
        }
    }

    /// Chooses whether a VIS header may start a reception over.
    ///
    /// Pushed to the receive worker at once rather than on the next frame, so
    /// the change applies to the reception in progress.
    pub fn set_vis_restart(&mut self, enabled: bool) {
        self.settings.vis_restart = enabled;
        self.push_rx_settings();
    }

    /// Chooses whether a VIS detection requires the leader tone as well.
    ///
    /// Pushed to the receive worker at once rather than on the next frame, so
    /// the change applies to the signal search in progress.
    pub fn set_vis_strict(&mut self, enabled: bool) {
        self.settings.vis_strict = enabled;
        self.push_rx_settings();
    }

    /// Hands the receive worker what the settings now say about reception.
    fn push_rx_settings(&mut self) {
        self.audio.set_rx_settings(rx_settings(&self.settings));
    }

    /// Abandons the reception in progress and starts the search over.
    ///
    /// The receiver reaches the same place on its own once synchronization is
    /// lost or nothing has progressed for twenty seconds. This is for the
    /// operator who can already see that the picture arriving is not one:
    /// started on a mode it should not have been, or on a signal that was
    /// never a picture at all.
    pub fn reset_reception(&mut self) {
        self.audio.reset_reception();
    }

    pub fn toggle_dsp(&mut self, dsp: Dsp) {
        self.settings.dsp.toggle(dsp);
        if dsp == Dsp::Slant {
            self.push_rx_settings();
        }
    }

    /// Opens one of the application's directories in the file manager.
    ///
    /// Received images and everything else the application stores live in the
    /// operator's own directories rather than in a session of its own, so
    /// browsing them is the file manager's job and the interface only has to
    /// point at the folder.
    pub fn reveal(&mut self, folder: Folder) {
        let message = self.i18n.text("error-open-folder");
        let opened = self.platform.open_path(self.paths.folder(folder));
        self.error = opened.err().map(|error| format!("{message}: {error}"));
    }

    /// Opens the operator's manual.
    ///
    /// The manual is published on the web and goes to the operator's browser
    /// rather than to a viewer of the application's own, which would be a
    /// worse browser reachable from one program.
    pub fn open_manual(&mut self) {
        let url = grayline_shell::manual_url(&crate::identity::IDENTITY);
        let message = self.i18n.text("error-open-manual");
        let opened = self.platform.open_url(&url);
        self.error = opened.err().map(|error| format!("{message}: {error}"));
    }

    pub fn normalize_call(&mut self) {
        if self.qso.call.chars().any(char::is_lowercase) {
            self.qso.call = self.qso.call.to_uppercase();
        }
    }

    /// Takes up the QSO fields once the operator has left one.
    ///
    /// Each of them is a value rather than text — a callsign, a report, a
    /// serial — and none of them means anything with spaces around it, so what
    /// is taken up is the trimmed field. Trimmed on leaving rather than on
    /// every keystroke, because a space deleted from under the cursor is one
    /// the operator is still typing.
    pub fn finish_qso_edit(&mut self) {
        let mut trimmed = trim_in_place(&mut self.qso.call);
        trimmed |= trim_in_place(&mut self.qso.rsv);
        trimmed |= trim_in_place(&mut self.qso.rsv_received);
        trimmed |= trim_in_place(&mut self.qso.number);
        if trimmed {
            self.qso_changed();
        }
        // Outside the branch above, because leaving the field is what commits
        // the callsign whether or not trimming changed anything, and the
        // request guards itself against being made twice for one station.
        self.look_up_contact();
        self.reflect_to_logger();
    }

    fn look_up_contact(&mut self) {
        self.directory.look_up(&self.qso.call);
    }

    pub fn refresh_contact(&mut self) {
        self.directory.refresh(&self.qso.call);
    }

    pub fn can_refresh_contact(&self) -> bool {
        ContactEditor::can_refresh(&self.settings.contact, &self.qso.call)
    }

    pub fn open_contact(&mut self) {
        self.directory.open(&self.settings.contact.fields);
    }

    pub fn add_contact_field(&mut self) {
        self.directory.add_field();
    }

    pub fn commit_contact(&mut self) {
        self.directory.commit(&self.qso.call);
    }

    /// Turns asking the operator's own logger on or off, and asks again.
    pub fn set_contact_lookup(&mut self, lookup: bool) {
        if self.settings.contact.lookup == lookup {
            return;
        }
        self.settings.contact.lookup = lookup;
        self.directory.reconfigure(&self.settings.contact);
        self.look_up_contact();
    }

    /// Writes the credentials file out for the operator to edit.
    ///
    /// Refuses to write over one that is already there, the way the rig script
    /// does: the file only exists because someone put a key in it.
    pub fn write_contact_credentials(&mut self) {
        let Some(path) = self.directory.paths().credentials.clone() else {
            return;
        };
        let written = if path.exists() {
            Ok(path)
        } else {
            grayline_qso::Credentials::write(&path, Some(&self.settings.contact.wavelog_url), "")
                .map(|()| path)
                .map_err(|error| io::Error::other(error.to_string()))
        };
        self.report_written(written);
    }

    fn poll_contact(&mut self) {
        if self.directory.poll() {
            self.request_composition();
        }
    }

    /// Turns sending the QSO panel to the operator's logger on or off, and
    /// sends it at once when it was turned on.
    pub fn set_logger_enabled(&mut self, enabled: bool) {
        if self.settings.logger.enabled == enabled {
            return;
        }
        self.settings.logger.enabled = enabled;
        self.open_logger();
        self.reflect_to_logger();
    }

    fn open_logger(&mut self) {
        if let Err(error) = self.logger.reconfigure(&self.settings.logger) {
            self.report_logger_error(&error);
        }
    }

    /// Whether the logger is sharing its entry form, which the QSO panel then
    /// follows instead of being edited.
    pub fn logger_synced(&self) -> bool {
        self.logger.is_synced()
    }

    /// Takes the logger's entry form into the QSO panel.
    ///
    /// The logger's form is the one the contact is logged from, so it wins.
    /// Only when the logger has just been found again does a field it has
    /// nothing for keep what the panel holds, which is then sent to it: the
    /// panel was the form while the logger was away. The serial number being
    /// sent is the logger's whenever it counts one.
    fn poll_logger(&mut self) {
        let Some(FormUpdate { form, resumed }) = self.logger.take_form() else {
            return;
        };
        let pick = |theirs: &str, own: &str| if resumed && theirs.is_empty() { own } else { theirs }.to_owned();
        let (own_report, own_number) = received_fields(&self.qso.rsv_received, self.logger_received.as_ref());
        let report = pick(form.get("RST_RCVD"), own_report);
        let number = pick(form.get("SRX_STRING"), own_number);
        let call = pick(form.get("CALL"), &self.qso.call);
        let rsv = pick(form.get("RST_SENT"), &self.qso.rsv);
        let serial = match form.get("STX_STRING") {
            "" => self.qso.number.clone(),
            serial => serial.to_owned(),
        };
        let received = format!("{report}{number}");
        let new_call = call != self.qso.call;
        let changed = new_call || rsv != self.qso.rsv || received != self.qso.rsv_received || serial != self.qso.number;
        self.qso = Qso {
            call,
            rsv_received: received,
            rsv,
            number: serial,
        };
        self.logger_received = Some((report, number));
        if changed {
            self.qso_changed();
        }
        if new_call {
            self.look_up_contact();
        }
        self.reflect_to_logger();
    }

    /// Takes the logger's form in until `done` holds, for a test whose fake
    /// logger answers on its own thread.
    #[cfg(test)]
    pub(crate) fn await_logger(&mut self, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            self.poll_logger();
            if done(self) {
                return;
            }
            assert!(Instant::now() < deadline, "the logger's form never arrived");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Fills the logger's entry form in from the QSO panel.
    ///
    /// The frequency and the mode are left to the logger, which reads them
    /// from the rig itself, and so is the serial number being sent, which the
    /// logger counts for its own contest. Called where a field is settled
    /// rather than on every keystroke, so the logger is not handed half a
    /// callsign.
    fn reflect_to_logger(&mut self) {
        let (report, number) = received_fields(&self.qso.rsv_received, self.logger_received.as_ref());
        let fields = [
            ("CALL", self.qso.call.as_str()),
            ("RST_SENT", self.qso.rsv.as_str()),
            ("RST_RCVD", report),
            ("SRX_STRING", number),
        ];
        if let Err(error) = self.logger.reflect(fields) {
            self.report_logger_error(&error);
        }
    }

    fn report_logger_error(&mut self, error: &grayline_logger_link::LinkError) {
        let message = self.i18n.text("error-logger");
        self.error = Some(format!("{message}: {error}"));
    }

    pub fn normalize_station(&mut self) {
        let normalized = self.station.callsign.trim().to_ascii_uppercase();
        if normalized != self.station.callsign {
            self.station.callsign = normalized;
        }
        trim_in_place(&mut self.station.qth);
        trim_in_place(&mut self.station.grid);
        self.request_composition();
    }

    pub fn qso_changed(&mut self) {
        self.request_composition();
    }

    /// Counts the serial number on for the next contact.
    ///
    /// Padded back to three digits, and allowed to grow past them: a contest
    /// worked past its thousandth contact is still counting. A number that is
    /// not one is left alone, because there is nothing to count it to.
    pub fn increment_number(&mut self) {
        let Ok(number) = self.qso.number.trim().parse::<u32>() else {
            return;
        };
        self.qso.number = format!("{:03}", number.saturating_add(1));
        self.qso_changed();
    }

    pub fn reset_number(&mut self) {
        self.qso.number = FIRST_QSO_NUMBER.to_owned();
        self.qso_changed();
    }

    pub fn open_custom_variables(&mut self) {
        self.variables_draft = self
            .settings
            .custom_variables
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.variables_dialog_open = true;
    }

    pub fn add_custom_variable(&mut self) {
        self.variables_draft.push((String::new(), String::new()));
    }

    /// Takes the edited rows as the variables templates may read.
    ///
    /// A row whose name no `${...}` expression could hold is left in the
    /// dialog to be corrected but kept out of the composition, so an unusable
    /// name never becomes a missing-variable error against a template that
    /// never asked for it.
    pub fn commit_custom_variables(&mut self) {
        let variables: BTreeMap<_, _> = self
            .variables_draft
            .iter()
            .filter(|(name, _)| valid_variable_name(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if variables != self.settings.custom_variables {
            self.settings.custom_variables = variables;
            self.request_composition();
        }
    }

    /// Reads the library on this thread, for startup and for a scan that
    /// could not be given a thread of its own.
    fn refresh_library(&mut self) {
        let scan = LibraryScan {
            templates: template_entries(self.paths.templates_dir()),
            stocks: stock_entries(self.paths.stocks_dir()),
        };
        self.adopt_library_scan(scan);
    }

    /// Reads the library again without making the interface wait for it.
    ///
    /// A stock folder is read file by file for each image's geometry, which on
    /// a networked folder takes longer than a frame has. The scan runs on a
    /// thread of its own and is adopted on the frame its result arrives.
    pub fn start_library_scan(&mut self) {
        let (sender, receiver) = mpsc::channel();
        let templates_dir = self.paths.templates_dir().to_path_buf();
        let stocks_dir = self.paths.stocks_dir().to_path_buf();
        let spawned = thread::Builder::new()
            .name("grayline-sstv-library".to_owned())
            .spawn(move || {
                let _ = sender.send(LibraryScan {
                    templates: template_entries(&templates_dir),
                    stocks: stock_entries(&stocks_dir),
                });
            });
        match spawned {
            Ok(_) => self.library_scan = Some(receiver),
            Err(_) => self.refresh_library(),
        }
    }

    fn poll_library_scan(&mut self) {
        let Some(receiver) = self.library_scan.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(scan) => {
                self.library_scan = None;
                self.adopt_library_scan(scan);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.library_scan = None,
        }
    }

    fn adopt_library_scan(&mut self, scan: LibraryScan) {
        let mut errors = Vec::new();
        match scan.templates {
            Ok(entries) => replace_entries(&mut self.library.templates, &mut self.library.template, entries),
            Err(error) => errors.push(error.to_string()),
        }
        match scan.stocks {
            Ok(entries) => {
                replace_entries(&mut self.library.stocks, &mut self.library.stock, entries);
            }
            Err(error) => errors.push(error.to_string()),
        }
        self.library.error = (!errors.is_empty()).then(|| errors.join("; "));
        self.composition.template_generation = self.composition.template_generation.wrapping_add(1);
        self.composition.stock_generation = self.composition.stock_generation.wrapping_add(1);
        self.request_composition();
    }

    /// Writes a completed reception out without making the interface wait.
    ///
    /// Encoding a lossless frame and writing it to disk takes longer than a
    /// frame has, and it would land exactly when the operator is looking at
    /// the finished picture. The writer is joined when the interface goes
    /// down, so quitting right after a reception still keeps it.
    fn save_history_in_background(&mut self, candidate: crate::worker::receive::HistoryCandidate) {
        let directory = self.paths.received_dir().to_path_buf();
        let format = self.settings.history_format;
        let spawned = thread::Builder::new()
            .name("grayline-sstv-history".to_owned())
            .spawn(move || {
                if let Err(error) = crate::storage::history::save(&directory, candidate, format) {
                    grayline_shell::log::note(&format!("failed to save receive history: {error}"));
                }
            });
        match spawned {
            Ok(writer) => self.history_writers.push(writer),
            Err(_) => grayline_shell::log::note("failed to start the receive history writer"),
        }
    }

    #[cfg(test)]
    fn wait_for_history_writers(&mut self) {
        for writer in self.history_writers.drain(..) {
            let _ = writer.join();
        }
    }

    #[cfg(test)]
    fn wait_for_library_scan(&mut self) {
        if let Some(receiver) = self.library_scan.take()
            && let Ok(scan) = receiver.recv()
        {
            self.adopt_library_scan(scan);
        }
    }

    /// How long the interface may sit still before it has to draw again.
    ///
    /// A reception asks for its own frames through the receive worker's waker,
    /// so what is left here is everything that has no producer to ask on its
    /// behalf: state only the clock changes, a worker read by polling, and a
    /// device whose failure arrives on the stream rather than in a snapshot.
    /// `None` leaves the interface asleep until the operator touches it.
    pub fn repaint_after(&self) -> Option<Duration> {
        let mut soonest = None;
        let mut at_most = |interval: Duration| {
            soonest = Some(soonest.map_or(interval, |current: Duration| current.min(interval)));
        };

        // A transmission is read from its worker and from the playback queue,
        // neither of which announces anything, and the row being sent moves
        // continuously while it runs.
        if self.outgoing.is_active() {
            at_most(LIVE_INTERVAL);
        }
        if self.composition.pending {
            at_most(COMPOSE_POLL);
        }
        // The library scan has no way to ask for a repaint of its own either.
        if self.library_scan.is_some() {
            at_most(COMPOSE_POLL);
        }
        if self.composition.shows_clock {
            at_most(Duration::from_secs(
                60 - Timestamp::now().as_second().rem_euclid(60) as u64,
            ));
        }
        // Nothing in a snapshot reports a device that stopped, and a rig is
        // read by polling, so both are watched for rather than waited on.
        if self.audio.is_capturing() || self.settings.rig.enabled {
            at_most(WATCH_INTERVAL);
        }
        soonest
    }

    /// Adopts anything any worker produced since the last frame.
    ///
    /// The receive snapshot, the finished history write, the library scan,
    /// the rig, the transmit state machine, and the composition are all read
    /// by polling, and this is the once-per-frame pump that reads them.
    pub fn poll_workers(&mut self) {
        self.poll_device_fault();
        if let Some(frame) = self.audio.poll() {
            self.rx_raster.set_frame(&frame);
        }
        if let Some(candidate) = self.audio.take_history() {
            self.adopt_received_image(&candidate.frame);
            if self.settings.auto_history {
                self.save_history_in_background(candidate);
            }
        }
        self.history_writers.retain(|writer| !writer.is_finished());
        self.poll_library_scan();
        self.poll_logger();
        if self.audio.session() != self.adopted_session {
            self.adopted_session = self.audio.session();
            self.adopted_callsigns = 0;
            self.adopted_numbers = 0;
        }
        self.adopt_decoded_callsign();
        self.adopt_decoded_number();
        self.push_rx_settings();
        self.outgoing.set_level(self.settings.tx_volume);
        if self.settings.auto_mode
            && let Some(mode) = self.audio.snapshot().mode
        {
            self.select_rx_mode(mode);
        }
        self.poll_rig();
        self.poll_contact();
        self.poll_composition();
        self.poll_transmit();
        // Reception stops for as long as anything is going out. The station's
        // own signal comes back off the antenna, and what would be decoded from
        // it is the picture being sent. Derived from the phase rather than
        // switched at either end of a transmission, so a tone, a picture, and a
        // transmission that failed all release it the same way.
        self.audio.set_muted_for_transmit(self.outgoing.is_active());
        self.refresh_timed_composition();
        self.refresh_tuned_composition();
        self.report_activity();
    }

    /// Starts, stops, and reads rig control.
    ///
    /// The switch in the menu is the whole of the decision, so it is acted on
    /// here rather than through a path of its own: a worker exists exactly
    /// while the operator has asked for one.
    fn poll_rig(&mut self) {
        match (self.settings.rig.enabled, self.rig_worker.is_some()) {
            (true, false) => {
                let worker = RigWorker::spawn(
                    &self.settings.rig,
                    self.paths.config_dir(),
                    Arc::clone(&self.bands),
                    grayline_shell::log::note,
                );
                self.rig_worker = Some(worker);
                self.rig_snapshot = RigSnapshot::default();
            }
            (false, true) => {
                // A rig switched off mid-transmission is still a keyed rig,
                // and the request has to go before the channel closes for the
                // worker to see it at all.
                self.outgoing.release_rig(self.rig_worker.as_ref());
                if let Some(worker) = self.rig_worker.take() {
                    worker.stop_in_background();
                }
                self.rig_snapshot = RigSnapshot::default();
            }
            _ => {}
        }
        if let Some(worker) = self.rig_worker.as_ref() {
            self.rig_snapshot = worker.latest();
        }
    }

    /// Switches rig control on or off.
    ///
    /// The whole of the decision: the next frame is what starts or stops the
    /// worker, and switching off mid-transmission is what gives the keying
    /// back before the connection goes.
    pub const fn set_rig_enabled(&mut self, enabled: bool) {
        self.settings.rig.enabled = enabled;
    }

    /// Makes the connection again after it failed.
    ///
    /// Giving up the worker is the whole of it: while rig control is switched
    /// on, the next frame is what starts one.
    pub fn retry_rig(&mut self) {
        self.outgoing.release_rig(self.rig_worker.as_ref());
        if let Some(worker) = self.rig_worker.take() {
            worker.stop_in_background();
        }
        self.rig_snapshot = RigSnapshot::default();
    }

    /// Whether the rig may be tuned from the interface.
    ///
    /// Not while transmitting: moving a rig that is on the air moves the
    /// transmission with it.
    pub fn can_tune(&self) -> bool {
        self.rig_snapshot.state == RigState::Receiving && !self.bands.is_empty()
    }

    pub fn tuned_band(&self) -> Option<&BandDefinition> {
        let name = self.rig_snapshot.reading.as_ref()?.band.as_deref()?;
        self.bands.by_name(name)
    }

    /// Moves the rig to `name`, handing the script the whole band.
    pub fn change_band(&mut self, name: &str) {
        if !self.can_tune() {
            return;
        }
        if let Some(worker) = self.rig_worker.as_ref() {
            worker.change_band(name);
        }
    }

    /// Where a press of the step buttons would put the rig, if anywhere.
    ///
    /// Stepping out of the band is not tuning, so the edge is where it stops
    /// rather than somewhere the operator is not licensed to be. A band with
    /// no step has nowhere to go, which is what disables the buttons.
    pub fn stepped_frequency(&self, steps: i64) -> Option<u64> {
        let frequency_hz = self.rig_snapshot.reading.as_ref()?.frequency_hz;
        let band = self.tuned_band()?;
        let moved = i64::try_from(frequency_hz).ok()? + steps.checked_mul(i64::try_from(band.step_hz()?).ok()?)?;
        let moved = u64::try_from(moved).ok()?;
        band.contains(moved).then_some(moved)
    }

    pub fn step_frequency(&mut self, steps: i64) {
        if !self.can_tune() {
            return;
        }
        let Some(target_hz) = self.stepped_frequency(steps) else {
            return;
        };
        if let Some(worker) = self.rig_worker.as_ref() {
            worker.set_frequency(target_hz);
        }
    }

    /// Writes the default script out for the operator to edit.
    ///
    /// The result is reported on the status line rather than in front of the
    /// interface, because nothing was lost either way.
    pub fn write_rig_script(&mut self) {
        let written = script::write_default(self.paths.config_dir());
        self.report_written(written);
    }

    /// Writes the built-in band plan out for the operator to edit.
    ///
    /// Refuses to write over one that is already there, for the same reason.
    /// The plan in use is not reloaded: a file being edited is not one to act
    /// on halfway through, so the next start is when it takes effect.
    pub fn write_band_plan(&mut self) {
        let written = BandPlan::write_default(self.paths.config_dir());
        self.report_written(written);
    }

    fn report_written(&mut self, written: io::Result<PathBuf>) {
        match written {
            Ok(path) => {
                self.notice = Some(
                    self.i18n
                        .text_with("rig-script-written", &[("path", owned(path.display().to_string()))]),
                );
                self.error = None;
                self.reveal(Folder::Config);
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.notice = None;
            }
        }
    }

    /// Puts a newly decoded station identifier in the QSO contact field.
    ///
    /// The identifier names the station on the air right now, so it takes the
    /// field outright: an operator who has just heard a new station wants that
    /// call, not the one left over from the previous contact. Only an arrival
    /// writes, so the field stays editable between receptions.
    fn adopt_decoded_callsign(&mut self) {
        let decoded = self.audio.snapshot().callsigns.len();
        if decoded == self.adopted_callsigns {
            return;
        }
        self.adopted_callsigns = decoded;
        // The FSKID alphabet has no lowercase, but it does have spaces, and an
        // identifier that is nothing else says nothing about who is calling.
        let Some(call) = self
            .audio
            .snapshot()
            .callsigns
            .last()
            .map(|call| call.trim().to_owned())
            .filter(|call| !call.is_empty())
        else {
            return;
        };
        if call == self.qso.call {
            return;
        }
        self.qso.call = call;
        self.qso_changed();
        // The identifier path does not go through `finish_qso_edit`, so the
        // lookup a typed callsign gets has to be asked for here too.
        self.look_up_contact();
        self.reflect_to_logger();
    }

    /// Puts a newly decoded contest number in the received report field.
    ///
    /// The number is all the identifier carries, so the report it is read as
    /// is filled in around it the way MMSSTV fills it in. It is followed the
    /// same way as the identifier beside it: only an arrival writes, and the
    /// count is followed down as well as up.
    fn adopt_decoded_number(&mut self) {
        let decoded = self.audio.snapshot().numbers.len();
        if decoded == self.adopted_numbers {
            return;
        }
        self.adopted_numbers = decoded;
        let Some(number) = self
            .audio
            .snapshot()
            .numbers
            .last()
            .map(|number| number.trim().to_owned())
            .filter(|number| !number.is_empty())
        else {
            return;
        };
        let report = format!("{RECEIVED_REPORT}{number}");
        if report == self.qso.rsv_received {
            return;
        }
        self.qso.rsv_received = report;
        self.qso_changed();
        self.reflect_to_logger();
    }

    /// Keeps a finished reception as the image `rximage` layers show.
    ///
    /// The receive worker only offers a reception that completed, or that was
    /// interrupted with enough of the raster decoded to be worth keeping, so
    /// the layer follows the same rule as the received folder without being
    /// tied to whether anything was written there.
    fn adopt_received_image(&mut self, frame: &Frame) {
        let Some(image) = frame.to_image() else {
            return;
        };
        self.composition.received_image = Arc::new(image);
        self.composition.received_at = Zoned::now();
        self.request_composition();
    }

    /// Picks up a device that stopped and puts it in front of the operator.
    ///
    /// The device lists are enumerated again at the same time, so whatever the
    /// operator reaches for next describes what is attached now rather than
    /// what was attached when the application started.
    fn poll_device_fault(&mut self) {
        let Some(fault) = self.audio.take_capture_fault() else {
            return;
        };
        grayline_shell::log::note(&format!("capture stopped: {fault}"));
        self.audio.rescan();
        self.device_fault = Some(fault);
    }

    /// Opens the device again after a fault, keeping the report up if it is
    /// still not there.
    pub fn retry_device(&mut self) {
        self.audio.rescan();
        if self.audio.reopen() {
            self.device_fault = None;
        }
    }

    pub fn dismiss_device_fault(&mut self) {
        self.device_fault = None;
    }

    /// Tells the platform what the application is doing, when that changes.
    ///
    /// Transmission outranks reception: the two cannot overlap on one device,
    /// and a transmission in progress is the stronger claim on the machine.
    fn report_activity(&mut self) {
        let activity = if self.outgoing.is_active() {
            Activity::Transmitting
        } else if matches!(
            self.audio.snapshot().progress,
            RxProgress::Acquiring | RxProgress::Decoding { .. }
        ) {
            Activity::Receiving
        } else {
            Activity::Idle
        };

        if activity != self.activity {
            self.activity = activity;
            self.platform.set_activity(activity);
        }
    }

    #[cfg(test)]
    pub(crate) const fn activity(&self) -> Activity {
        self.activity
    }

    pub fn decoded_fraction(&self) -> f32 {
        match self.tab {
            Tab::Receive => self.audio.snapshot().display_fraction,
            Tab::Transmit => {
                if self.outgoing.is_sending_picture() {
                    self.tx_progress().fraction()
                } else {
                    1.0
                }
            }
        }
    }

    pub const fn active_mode(&self) -> Mode {
        match self.tab {
            Tab::Transmit => self.settings.tx_mode,
            Tab::Receive => self.settings.rx_mode,
        }
    }

    pub const fn active_raster_mut(&mut self) -> &mut Raster {
        match self.tab {
            Tab::Transmit => &mut self.tx_raster,
            Tab::Receive => &mut self.rx_raster,
        }
    }

    pub fn template_changed(&mut self) {
        self.composition.template_generation = self.composition.template_generation.wrapping_add(1);
        self.request_composition();
    }

    pub fn stock_changed(&mut self) {
        self.composition.stock_generation = self.composition.stock_generation.wrapping_add(1);
        self.request_composition();
    }

    /// Asks the worker for the image the transmit tab should be showing.
    ///
    /// The result replaces the transmit image outright: what is on the tab is
    /// what a transmission would send, so there is nothing to confirm between
    /// choosing a template and keying up.
    fn request_composition(&mut self) {
        // Recorded before anything can turn back, so a composition that is put
        // off is not also asked for again on every frame that follows.
        self.composition.minute = current_minute();
        self.composition.shows_clock = false;
        self.composition.reading = self.rig_snapshot.reading.clone();
        self.composition.shows_frequency = false;
        // A transmission is sending the frame currently on the tab. Replacing
        // it would show something that is not going out, so the change waits
        // until the transmission is over.
        if self.outgoing.is_active() {
            self.composition.deferred = true;
            return;
        }
        let (Some(template), Some(stock)) = (
            self.library
                .template
                .and_then(|index| self.library.templates.get(index)),
            self.library.stock.and_then(|index| self.library.stocks.get(index)),
        ) else {
            self.composition.frame = None;
            return;
        };
        self.composition.generation = self.composition.generation.wrapping_add(1);
        self.composition.frame = None;
        self.composition.pending = true;
        self.tx_error = None;
        self.composition.composer.request(ComposeRequest {
            generation: self.composition.generation,
            template_generation: self.composition.template_generation,
            stock_generation: self.composition.stock_generation,
            template_path: template.path.clone(),
            background_path: stock.path.clone(),
            assets_dir: self.paths.assets_dir().to_path_buf(),
            mode: self.settings.tx_mode,
            received_image: self.composition.received_image.clone(),
            received_at: self.composition.received_at.clone(),
            station_callsign: self.station.callsign.clone(),
            station_qth: self.station.qth.clone(),
            station_grid: self.station.grid.clone(),
            contact_callsign: self.qso.call.clone(),
            contact: self.directory.snapshot.fields.clone(),
            report: self.qso.rsv.clone(),
            number: self.qso.number.clone(),
            report_received: self.qso.rsv_received.clone(),
            custom: self.settings.custom_variables.clone(),
            radio: self.composition.reading.clone(),
        });
    }

    /// Composes again once a frame that shows the time has fallen behind it.
    ///
    /// The finest unit a composition can print is the minute, so the clock
    /// moving on is exactly when the frame on the transmit tab stops being
    /// what a transmission should send.
    fn refresh_timed_composition(&mut self) {
        if self.composition.shows_clock && current_minute() != self.composition.minute {
            self.request_composition();
        }
    }

    /// Composes again once the rig has moved off what a frame printed.
    ///
    /// Only for a frame that prints it: a template that says nothing about the
    /// frequency has no reason to be composed again every time the operator
    /// turns the dial.
    fn refresh_tuned_composition(&mut self) {
        if self.composition.shows_frequency && self.rig_snapshot.reading != self.composition.reading {
            self.request_composition();
        }
    }

    /// Adopts the picture the composer finished, if it is the one asked for
    /// last.
    fn poll_composition(&mut self) {
        if let Some(result) = self.composition.composer.latest()
            && result.generation == self.composition.generation
        {
            self.composition.pending = false;
            self.composition.shows_clock = result.uses_timestamps;
            self.composition.shows_frequency = result.uses_radio;
            match result.frame {
                Ok(frame) => {
                    self.tx_raster.set_image(&frame);
                    self.composition.frame = Some(frame);
                    self.tx_error = None;
                }
                Err(error) => {
                    self.composition.frame = None;
                    self.tx_error = Some(error);
                }
            }
        }
    }

    fn poll_transmit(&mut self) {
        if let Some(ending) = self.outgoing.poll(&self.rig_snapshot, Instant::now()) {
            self.end_transmission(ending);
        }
    }

    pub fn can_transmit(&self) -> bool {
        self.transmit_problem().is_none() && !self.outgoing.is_active()
    }

    pub fn transmit_problem(&self) -> Option<String> {
        if self.is_tuning() {
            return Some(self.i18n.text("error-tone-active"));
        }
        // Reported first, and required whether or not the identifier is sent:
        // a transmission is made by a station, and this is the only thing that
        // names it. The rest is about this transmission; this is about being
        // allowed to make one at all.
        if let Err(error) = FskId::new(self.station.callsign.trim()) {
            return Some(
                self.i18n
                    .text_with("error-invalid-station-call", &[("error", owned(error.to_string()))]),
            );
        }
        if self.composition.frame.is_none() {
            return Some(self.i18n.text("error-no-transmit-frame"));
        }
        if self.audio.output_device.is_none() {
            return Some(self.i18n.text("error-no-output-device"));
        }
        self.rig_problem()
    }

    /// Reports why the rig could not be handed a transmission, if it could not.
    ///
    /// A rig that was asked to key and did not leaves a transmission nobody
    /// hears, so it stops one rather than being let through quietly.
    fn rig_problem(&self) -> Option<String> {
        if !self.settings.rig.enabled || self.rig_snapshot.state.is_ready() {
            return None;
        }
        let detail = self.rig_snapshot.error.as_ref().map_or_else(
            || self.i18n.text(self.rig_snapshot.state.label_key()),
            RigControlError::to_string,
        );
        Some(
            self.i18n
                .text_with("error-rig-unavailable", &[("error", owned(detail))]),
        )
    }

    fn station_id(&self) -> Option<Result<FskId, grayline_sstv_fskid::FskIdError>> {
        self.settings
            .send_fskid
            .then(|| FskId::new(self.station.callsign.trim()))
    }

    /// Returns the contest number that identifier would carry, if any.
    ///
    /// Only in contest mode, and only for a number the record can hold. The
    /// text is filtered and uppercased the way MMSSTV filters it, and a number
    /// that still does not fit is left off rather than refused: the picture is
    /// what the transmission is for.
    fn contest_number(&self) -> Option<FskNumber> {
        if !self.settings.contest_mode {
            return None;
        }
        let text: String = self
            .qso
            .number
            .to_ascii_uppercase()
            .chars()
            .filter(|character| *character >= '0')
            .collect();
        FskNumber::new(&text).ok()
    }

    pub fn start_transmit(&mut self) {
        if let Some(error) = self.transmit_problem() {
            self.tx_error = Some(error);
            return;
        }
        let identification = match self.station_id().transpose() {
            Ok(station_id) => station_id.map(|station_id| Identification {
                station_id,
                number: self.contest_number(),
            }),
            Err(error) => {
                self.tx_error = Some(error.to_string());
                return;
            }
        };
        let frame = self
            .composition
            .frame
            .clone()
            .expect("transmit availability requires a prepared frame");
        let mode = self.settings.tx_mode;
        self.begin_transmission(Kind::Picture, |writer, gain| {
            TxWorker::spawn(writer, mode, frame, identification, gain)
        });
    }

    /// Claims the output stream and the rig for something about to go out.
    ///
    /// When the stream could not be opened nothing starts, and the reason is
    /// put in front of the operator.
    fn begin_transmission(&mut self, kind: Kind, spawn: impl FnOnce(PlaybackWriter, Arc<TxGain>) -> TxWorker) {
        let (playback, writer) = match self.audio.open_playback(PLAYBACK_QUEUE_SAMPLES) {
            Ok(playback) => playback,
            Err(error) => {
                self.tx_error = Some(error.to_string());
                return;
            }
        };
        self.tx_error = None;
        self.outgoing.set_level(self.settings.tx_volume);
        let worker = spawn(writer, self.outgoing.gain());
        self.outgoing.begin(kind, playback, worker, self.rig_worker.as_ref());
    }

    /// Reports why the tune tone could not be sent, if it could not.
    ///
    /// Less stands in its way than in a picture's: a tone carries no image and
    /// names no station, so only the output device and the rig are asked.
    pub fn tune_problem(&self) -> Option<String> {
        if self.outgoing.is_sending_picture() {
            return Some(self.i18n.text("error-transmit-active"));
        }
        if self.audio.output_device.is_none() {
            return Some(self.i18n.text("error-no-output-device"));
        }
        self.rig_problem()
    }

    /// Keys the rig and sends the steady tone a repeater is opened with.
    pub fn start_tune(&mut self) {
        if let Some(error) = self.tune_problem() {
            self.tx_error = Some(error);
            return;
        }
        self.begin_transmission(
            Kind::Tune {
                until: Instant::now() + TUNE_LIMIT,
            },
            |writer, gain| TxWorker::spawn_tune(writer, TUNE_FREQUENCY_HZ, gain),
        );
    }

    pub fn stop_tune(&mut self) {
        if self.is_tuning() {
            self.end_transmission(Ending::Cancelled);
        }
    }

    pub fn is_tuning(&self) -> bool {
        self.outgoing.is_tuning()
    }

    /// Puts the interface in the state a running tone leaves it in.
    ///
    /// Tests have no device to send one on, and what is asserted about a tone
    /// is what the operator can do while it runs rather than its audio.
    #[cfg(test)]
    pub(crate) fn tune_for_test(&mut self) {
        self.outgoing.begin_for_test(
            Kind::Tune {
                until: Instant::now() + TUNE_LIMIT,
            },
            crate::worker::transmit::TxPhase::Producing,
        );
    }

    pub fn stop_transmit(&mut self) {
        if self.outgoing.is_active() {
            self.end_transmission(Ending::Cancelled);
        }
    }

    /// Ends whatever is going out, however it ended.
    ///
    /// Every way a transmission ends comes through here, so each of them gives
    /// the rig back and makes the composition that was put off while it ran.
    fn end_transmission(&mut self, ending: Ending) {
        if let Ending::Failed(error) = &ending {
            self.tx_error = Some(error.clone());
        }
        self.outgoing.end(&ending, self.rig_worker.as_ref());
        if self.composition.deferred {
            self.composition.deferred = false;
            self.request_composition();
        }
    }

    pub fn tx_progress(&self) -> TxProgress {
        self.outgoing.progress()
    }

    pub fn output_sample_rate_hz(&self) -> Option<u32> {
        self.outgoing.sample_rate_hz()
    }
}

/// What `settings` say about reception, in the form the receive worker reads.
///
/// Which modes a reception may start in without a VIS header follows the mode
/// selection. A transmission joined after its header, or that never sent one,
/// can only be identified by the spacing of its sync pulses, and a period can
/// be matched by more than one mode. So the operator's own choice decides how
/// far the inference may reach: with automatic detection on, any supported
/// mode may be inferred, and with it off only the selected mode is
/// considered, which confirms rather than overrides that choice.
fn rx_settings(settings: &Settings) -> RxSettings {
    RxSettings {
        live_slant: settings.dsp.live_slant,
        vis_restart: settings.vis_restart,
        vis_strict: settings.vis_strict,
        sync_start: if settings.auto_mode {
            SyncStart::Any
        } else {
            SyncStart::Only(settings.rx_mode)
        },
    }
}

/// The received report and exchange `received` holds, read as the logger's
/// pair while it still says what `logger` said.
fn received_fields<'a>(received: &'a str, logger: Option<&'a (String, String)>) -> (&'a str, &'a str) {
    match logger {
        Some((report, number)) if received.strip_prefix(report.as_str()) == Some(number.as_str()) => (report, number),
        _ => split_received_report(received),
    }
}

/// Splits the received report into the RSV and the contest number after it.
///
/// The panel holds the two as one field, `595012`, the way MMSSTV writes them
/// and the way an identifier's number is filled in. An RSV is always three
/// digits, so a field that does not start with three is taken as a report
/// alone.
fn split_received_report(received: &str) -> (&str, &str) {
    match received.get(..3) {
        Some(report) if report.bytes().all(|byte| byte.is_ascii_digit()) => (report, received[3..].trim()),
        _ => (received, ""),
    }
}

/// Takes the spaces off a finished field, reporting whether any were there.
///
/// The operator's own template variables are the exception and are never put
/// through this: what they hold is text the operator wrote, and its spaces are
/// theirs to keep.
fn trim_in_place(text: &mut String) -> bool {
    let trimmed = text.trim();
    if trimmed.len() == text.len() {
        return false;
    }
    let trimmed = trimmed.to_owned();
    *text = trimmed;
    true
}

/// Which minute the clock is in, counted from the epoch.
///
/// A composition is compared against this rather than against a wall-clock
/// field, so it follows the same instant in every zone and needs no calendar.
fn current_minute() -> i64 {
    Timestamp::now().as_second().div_euclid(60)
}

fn modes(support: fn(Mode) -> Support) -> Vec<Mode> {
    Mode::ALL
        .into_iter()
        .filter(|mode| support(*mode) == Support::Supported)
        .collect()
}

impl Drop for App {
    fn drop(&mut self) {
        // A reception may still be on its way to disk, and the interface
        // going down is the last chance to wait for it.
        for writer in self.history_writers.drain(..) {
            let _ = writer.join();
        }
    }
}

fn index_of(entries: &[Entry], name: Option<&str>) -> Option<usize> {
    let name = name?;
    entries.iter().position(|entry| entry.name == name)
}

fn selected_name(entries: &[Entry], selected: Option<usize>) -> Option<String> {
    entries.get(selected?).map(|entry| entry.name.clone())
}

fn replace_entries(entries: &mut Vec<Entry>, selected: &mut Option<usize>, next: Vec<Entry>) {
    let selected_path = selected.and_then(|index| entries.get(index).map(|entry| &entry.path));
    let next_selected = selected_path
        .and_then(|path| next.iter().position(|entry| entry.path == *path))
        .or_else(|| (!next.is_empty()).then_some(0));
    *entries = next;
    *selected = next_selected;
}

#[cfg(test)]
mod tests;
