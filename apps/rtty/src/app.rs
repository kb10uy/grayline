//! The application's state, and every command the interface can issue.
//!
//! The interface reads these fields and calls these methods; nothing here
//! draws. Keeping the two apart is what lets the whole of the state be driven
//! from a test without a window.

use std::{sync::Arc, time::Duration};

use grayline_logger_link::{FormUpdate, LoggerLink};
use grayline_qso::{ContactEditor, ContactPaths};
use grayline_rig_control::{Keying, RigControlError, RigSnapshot, RigState, RigWorker, bands::BandPlan};
use grayline_rtty::{BaudRate, ToneSet, TxConfig, TxFraming, TxSchedule, encode_text};
use grayline_shell::{
    common::{CommonConfig, CommonSettings, UI_SCALE_RANGE},
    i18n::{I18n, Locale, owned},
    log,
    platform::{Activity, Platform},
    worker::Waker,
};

use crate::{
    app::{
        macros::{Contact, Macro, MacroContext, Template, expand, valid_variable_name},
        transmit::{Ending, Sending, Transmit},
    },
    error::AppError,
    locales::CATALOG,
    storage::{
        config::{Config, MAXIMUM_MARK_HZ, MINIMUM_MARK_HZ, Settings},
        library,
        paths::{AppPaths, Folder},
    },
    ui::{scope::Scope, scrollback::Scrollback},
    worker::{
        audio::AudioState,
        keyer::{KEYER_TICKS_PER_SECOND, KeyerWorker},
        receive::{ColumnSnapshot, DecodePath, WorkerSettings},
        transmit::{Output, TxWorker},
    },
};

pub mod macros;
pub mod text;
pub mod transmit;

/// How far one point of drag moves the mark tone, in hertz.
///
/// Five, because that is about the width of the band-pass's tolerance for
/// being wrong: a pair set to within five hertz decodes, and a field that
/// moved by one would take a hundred drags to cross a mistuning.
pub const MARK_STEP_HZ: f64 = 5.0;

/// One second of playback queue at the preferred rate.
const PLAYBACK_CAPACITY_SAMPLES: usize = 48_000;

#[derive(Default)]
struct Library {
    macros: Vec<Macro>,
    templates: Vec<Template>,
}

/// Everything that was read from a file before the application was built.
///
/// Passed in together rather than read here, because the application is also
/// built for tests that touch no disk at all.
struct Stored {
    config: Config,
    common: CommonConfig,
    library: Library,
}

/// Everything the application is, other than the pixels on screen.
pub struct App {
    pub i18n: I18n,
    pub audio: AudioState,
    /// What each decode path has printed, in the order the paths are listed.
    pub columns: Vec<Scrollback>,
    pub ui_scale: f32,

    /// What the operator chose, as it is saved.
    ///
    /// The panels, the menus, and the dialogs edit this directly; the receive
    /// worker is handed its part through [`push_settings`](Self::push_settings).
    pub settings: Settings,

    /// The draft, the queue behind it, and the message on the air.
    pub transmit: Transmit,
    /// The station being worked, as entered beside the received text.
    pub contact: Contact,
    /// The buttons under the message field.
    pub macros: Vec<Macro>,
    /// The set messages listed beside the buttons, in the order they are
    /// offered.
    pub templates: Vec<Template>,
    /// The contact directory, what it last answered, and the dialog editing it.
    pub directory: ContactEditor,
    /// Where the contact fields are sent for the operator's logger to fill its
    /// entry form from, and where that form comes back from.
    logger: LoggerLink,
    /// Whether the window naming this station is open.
    pub station_dialog_open: bool,
    /// The rows the window is editing.
    ///
    /// Edited apart from `custom_variables` because a name is half typed for
    /// as long as it takes to type it, and a half-typed name is a different
    /// field: the rows are taken up once they are usable, and an unusable one
    /// stays on screen to be corrected rather than disappearing.
    pub variables_draft: Vec<(String, String)>,

    /// The window the band and the two channels are drawn in.
    ///
    /// Held here rather than by the interface because it is a thing the
    /// operator opened, not a thing a frame drew: it outlives any one of
    /// them, and what it shows is fed to it from here.
    pub scope: Scope,

    /// The last thing worth telling the operator, shown on the status bar.
    pub notice: Option<String>,

    paths: AppPaths,
    config: Config,
    /// The settings last written, so a frame that changed nothing writes
    /// nothing.
    saved: Settings,
    /// The language and the scale, which the whole family shares.
    common: CommonConfig,
    platform: Box<dyn Platform>,
    activity: Activity,
    /// The thread that keys the rig, once the file has asked for one.
    rig_worker: Option<RigWorker>,
    /// What rig control last published, read once per frame.
    pub rig_snapshot: RigSnapshot,
    /// Whether the rig has been asked to key and not yet asked to stop.
    ///
    /// One transmission is not one message: the queue keeps the rig keyed
    /// across the messages behind the one on the air, because unkeying between
    /// them would drop the carrier in the middle of what the other station is
    /// reading as one transmission.
    rig_keyed: bool,
    /// The session the text on screen belongs to.
    ///
    /// A device change starts a new worker whose decoders begin again, so the
    /// scrollback is cleared rather than appended to.
    session: u64,
}

impl App {
    pub fn new(paths: AppPaths, waker: Waker) -> Self {
        let (config, settings) = Config::load(paths.config_file().to_path_buf());
        let macros = library::load_macros(&paths.macros_file());
        let templates = library::load_templates(&paths.templates_file());
        let common = CommonConfig::discover();
        let audio = AudioState::new(
            settings.device.as_deref(),
            settings.output_device.as_deref(),
            worker_settings(&settings),
            waker.clone(),
        );
        let mut app = Self::from_parts(
            audio,
            paths,
            Stored {
                config,
                common,
                library: Library { macros, templates },
            },
            &settings,
            ContactPaths {
                store: grayline_qso::default_store_path(),
            },
            waker,
            grayline_shell::platform::host(),
        );
        app.open_logger();
        // Either file can be the unreadable one, and the first of them the
        // operator is told about is the one worth reporting: a second notice
        // would only replace the first before it had been read.
        if let Some(error) = app.config.error().or_else(|| app.common.error()) {
            let message = app.i18n.text("error-config");
            app.notice = Some(format!("{message}: {error}"));
        }
        app
    }

    fn from_parts(
        mut audio: AudioState,
        paths: AppPaths,
        Stored {
            config,
            common,
            library,
        }: Stored,
        settings: &Settings,
        contact_paths: ContactPaths,
        waker: Waker,
        platform: Box<dyn Platform>,
    ) -> Self {
        let session = audio.session();
        let shared = common.settings();
        audio.set_scope(settings.scope);
        let mut scope = Scope::default();
        scope.set_open(settings.scope);
        Self {
            i18n: I18n::new(shared.locale, &CATALOG),
            audio,
            columns: DecodePath::ALL.map(|_| Scrollback::default()).into_iter().collect(),
            ui_scale: shared.ui_scale,
            settings: settings.clone(),
            transmit: Transmit::default(),
            contact: Contact::default(),
            macros: library.macros,
            templates: library.templates,
            directory: ContactEditor::new(contact_paths, "grayline-rtty-contact".to_owned(), {
                let waker = waker.clone();
                move || waker.wake()
            }),
            logger: LoggerLink::new("grayline-rtty-logger", move || waker.wake()),
            station_dialog_open: false,
            variables_draft: Vec::new(),
            scope,
            notice: None,
            session,
            paths,
            config,
            saved: settings.clone(),
            common,
            platform,
            activity: Activity::Idle,
            rig_worker: None,
            rig_snapshot: RigSnapshot::default(),
            rig_keyed: false,
        }
    }

    /// Builds an application that touches neither the host nor the disk.
    #[cfg(test)]
    pub fn headless() -> Self {
        Self::headless_on(Box::new(grayline_shell::platform::QuietPlatform))
    }

    #[cfg(test)]
    pub fn headless_on(platform: Box<dyn Platform>) -> Self {
        let settings = Settings {
            logger: crate::test_util::logger_sink(),
            ..Settings::default()
        };
        let scratch = crate::test_util::scratch_dir();
        Self::from_parts(
            AudioState::disconnected(worker_settings(&settings)),
            AppPaths::from_roots(grayline_shell::paths::Roots::under(&scratch)),
            Stored {
                config: Config::detached(),
                common: CommonConfig::detached(),
                library: Library {
                    macros: crate::app::macros::default_macros(),
                    templates: crate::app::macros::default_templates(),
                },
            },
            &settings,
            ContactPaths::default(),
            Waker::default(),
            platform,
        )
    }

    /// The window title.
    ///
    /// It carries no state, unlike the other two applications': a reception
    /// here is not an event with a beginning and an end but a line that is
    /// either printing or idle, and a title that followed the squelch would
    /// flicker in the taskbar all evening.
    pub fn title(&self) -> String {
        crate::identity::IDENTITY.title()
    }

    /// The pair the receiver is listening for, before any AFC movement.
    pub fn tones(&self) -> ToneSet {
        ToneSet {
            mark_hz: self.settings.mark_hz,
            space_hz: self.settings.mark_hz + self.settings.shift_hz,
        }
    }

    /// What one decode path last reported, if the worker has said anything.
    pub fn column(&self, index: usize) -> Option<&ColumnSnapshot> {
        self.audio.snapshot().columns.get(index)
    }

    /// Whether anything is being framed on any path right now.
    pub fn is_printing(&self) -> bool {
        self.audio.snapshot().columns.iter().any(|column| column.squelch_open)
    }

    /// Collects whatever the receive worker published since the last frame.
    pub fn poll_workers(&mut self) {
        if let Some(fault) = self.audio.take_capture_fault() {
            self.audio.rescan();
            // A device that came back is opened again rather than left for the
            // operator to reselect: a listening watch runs for hours, and a
            // stream that stopped for a moment should not need a click.
            if !self.audio.reopen() {
                let message = self.i18n.text("error-device-lost");
                self.notice = Some(format!("{message}: {fault}"));
            }
        }
        let session = self.audio.session();
        if session != self.session {
            self.session = session;
            self.clear();
        }

        if let Some(printed) = self.audio.poll() {
            for (column, text) in self.columns.iter_mut().zip(printed) {
                column.push_str(&text);
            }
        }
        // After the snapshot has been adopted, so that what the window draws
        // is the block the text on screen was decoded from.
        self.pump_scope();
        if let Some(error) = self.audio.snapshot().error.clone() {
            self.report(&error);
        }

        self.poll_rig();
        self.poll_transmit();
        self.poll_logger();
        self.poll_contact();

        let activity = if self.is_transmitting() {
            Activity::Transmitting
        } else if self.is_printing() {
            Activity::Receiving
        } else {
            Activity::Idle
        };
        if activity != self.activity {
            self.activity = activity;
            self.platform.set_activity(activity);
        }
    }

    /// Feeds the scope window, and closes it when the operator did.
    ///
    /// Everything it draws is taken from the same snapshot the rest of the
    /// interface reads, so the two cannot disagree about what is arriving.
    fn pump_scope(&mut self) {
        if self.scope.take_close_request() {
            self.set_scope_open(false);
        }
        if !self.scope.is_open() {
            return;
        }
        self.scope.describe(&self.i18n);
        let tones = self.column(0).map_or_else(|| self.tones(), |column| column.tones);
        self.scope.retune(tones);
        if let Some(frame) = self.audio.take_scope_frame() {
            self.scope.push(frame);
        }
    }

    /// Opens or closes the scope window.
    ///
    /// The receiver is told as well as the window: what it costs to tap the
    /// channels and transform the band is only worth paying while something
    /// is drawing them.
    pub fn set_scope_open(&mut self, open: bool) {
        self.scope.set_open(open);
        self.audio.set_scope(open);
        self.settings.scope = open;
    }

    /// Returns how long the interface may sleep for.
    ///
    /// The worker asks for a frame itself when it has something worth showing,
    /// so this only covers the readouts nothing else would redraw. A
    /// transmission is the exception: nothing wakes the interface as the
    /// underline advances, so it is drawn at a rate the eye reads as motion.
    pub fn repaint_after(&self) -> Option<Duration> {
        if self.transmit.on_air().is_some() {
            return Some(Duration::from_millis(50));
        }
        self.audio.is_capturing().then(|| Duration::from_secs(1))
    }

    /// Writes the settings when a frame changed one.
    ///
    /// Both files: the shared one skips a write that would change nothing
    /// itself, because the other applications write to it too.
    pub fn persist(&mut self) {
        self.common.save(&CommonSettings {
            locale: self.i18n.locale(),
            ui_scale: self.ui_scale,
        });
        self.settings.device = self.audio.device.as_ref().map(|device| device.name().to_owned());
        self.settings.output_device = self.audio.output_device.as_ref().map(|device| device.name().to_owned());
        self.settings = core::mem::take(&mut self.settings).clamped();
        if self.settings == self.saved {
            return;
        }
        self.config.save(&self.settings);
        self.saved = self.settings.clone();
    }

    /// Hands the receive settings to the worker.
    ///
    /// Called after anything on the panel changes, rather than on every frame,
    /// so a setting the operator did not touch is not rewritten — and, because
    /// a receiver cannot be retuned in place, so a frame that changed nothing
    /// does not restart the decoders.
    pub fn push_settings(&mut self) {
        self.settings = core::mem::take(&mut self.settings).clamped();
        self.audio.set_settings(worker_settings(&self.settings));
    }

    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end());
    }

    pub fn zoom_by(&mut self, delta: f32) {
        self.set_ui_scale(self.ui_scale + delta);
    }

    /// Adopts the pair the automatic frequency control has moved to.
    ///
    /// AFC follows the transmission, and what it found is lost when the
    /// receiver is next rebuilt — a settings change, a device change, an
    /// overrun. This is how the operator keeps it.
    pub fn adopt_detected_tones(&mut self) {
        let Some(column) = self.column(0) else { return };
        let tones = column.tones;
        // The detected pair is reported in the roles the receiver is using,
        // so a reversed one has already been swapped back.
        let (mark, space) = if self.settings.reverse {
            (tones.space_hz, tones.mark_hz)
        } else {
            (tones.mark_hz, tones.space_hz)
        };
        self.settings.mark_hz = mark.clamp(MINIMUM_MARK_HZ, MAXIMUM_MARK_HZ);
        self.settings.shift_hz = (space - mark).abs();
        self.push_settings();
    }

    pub fn select_locale(&mut self, locale: Locale) {
        if locale != self.i18n.locale() {
            self.i18n = I18n::new(locale, &CATALOG);
        }
    }

    pub fn select_device_named(&mut self, name: &str) {
        let Some(device) = self.audio.devices.iter().find(|device| device.name() == name).cloned() else {
            return;
        };
        if self.audio.device.as_ref() == Some(&device) {
            return;
        }
        self.audio.select(device);
    }

    /// Chooses where transmissions are played out.
    ///
    /// Unlike the capture device, this closes nothing: what it changes is
    /// where the next message opens its stream.
    pub fn select_output_device_named(&mut self, name: &str) {
        let Some(device) = self
            .audio
            .output_devices
            .iter()
            .find(|device| device.name() == name)
            .cloned()
        else {
            return;
        };
        self.audio.select_output(device);
    }

    /// The transmitter's settings, taken from the panel the receiver uses.
    ///
    /// One pair of tones and one speed for both directions, because that is
    /// what a transceiver on one frequency does; reverse is shared for the
    /// same reason, since the sideband that inverts a received signal inverts
    /// a sent one.
    pub fn tx_config(&self) -> TxConfig {
        let default = TxConfig::default();
        TxConfig {
            tones: self.tones(),
            framing: TxFraming {
                baud: BaudRate::new(self.settings.baud).unwrap_or_default(),
                ..default.framing
            },
            reverse: self.settings.reverse,
            tx_unshift_on_space: self.settings.tx_unshift_on_space,
            amplitude: tx_amplitude(self.settings.tx_level),
            ..default
        }
    }

    /// The first character of the draft the transmitter has no code for.
    ///
    /// Typing cannot produce one, because the field refuses the keystroke;
    /// pasted text and an expanded macro can, and this is what holds the send
    /// button until the operator has dealt with it.
    pub fn unsendable_character(&self) -> Option<char> {
        text::first_unsendable(&self.transmit.draft)
    }

    /// Opens the window naming this station, with its fields loaded.
    pub fn open_station(&mut self) {
        self.variables_draft = self
            .settings
            .custom_variables
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.station_dialog_open = true;
    }

    pub fn add_custom_variable(&mut self) {
        self.variables_draft.push((String::new(), String::new()));
    }

    /// Takes the edited rows as the fields the macros may read.
    ///
    /// A row whose name no `${...}` expression could hold is kept in the
    /// window to be corrected but left out of what the macros see, so a name
    /// still being typed never briefly becomes a field of its own.
    pub fn commit_custom_variables(&mut self) {
        self.settings.custom_variables = self
            .variables_draft
            .iter()
            .filter(|(name, _)| valid_variable_name(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
    }

    /// Writes a macro out with this station's and the contact's details in it.
    ///
    /// Filled in when the button is pressed rather than when the message is
    /// keyed, so the time it names is the time the operator wrote it and so
    /// what is about to go out can still be read and edited.
    pub fn expand_macro(&self, index: usize) -> Option<Result<String, AppError>> {
        Some(self.written(&self.macros.get(index)?.text))
    }

    /// The same, for one of the set messages listed beside the buttons.
    pub fn expand_template(&self, index: usize) -> Option<Result<String, AppError>> {
        Some(self.written(&self.templates.get(index)?.text))
    }

    /// Fills a message's names in from the station, the contact, and the clock.
    fn written(&self, text: &str) -> Result<String, AppError> {
        let now = jiff::Zoned::now();
        expand(
            text,
            &MacroContext {
                station: &self.settings.station,
                contact: &self.contact,
                directory: &self.directory.snapshot.fields,
                custom: &self.settings.custom_variables,
                now: &now,
            },
        )
        .map(|text| text::normalize(&text))
        .map_err(AppError::from)
    }

    /// Presses a macro button.
    ///
    /// One that sends goes out as its own message rather than joining the
    /// draft, so a call that is pressed while a reply is half written does not
    /// take the reply with it. Everything else is written into the draft at
    /// `insert_at`, which is where the caret was.
    ///
    /// Returns where the caret should end up, for a macro that was written
    /// into the draft rather than sent.
    pub fn apply_macro(&mut self, index: usize, insert_at: usize) -> Option<usize> {
        let text = match self.expand_macro(index)? {
            Ok(text) => text,
            Err(error) => {
                self.report(&error);
                return None;
            }
        };
        if self.macros.get(index).is_some_and(|template| template.send) {
            if self.lacks_output() {
                self.notice = Some(self.i18n.text("error-no-output"));
                return None;
            }
            self.transmit.queue(text.trim_end_matches(['\r', '\n']).to_owned());
            return None;
        }
        Some(self.write_into_draft(&text, insert_at))
    }

    /// Picks one of the set messages out of the list beside the buttons.
    ///
    /// It replaces the draft rather than joining it, which is the difference
    /// between the two lists: a macro is pressed to add a line to what is
    /// being written, while one of these is picked because it is the whole of
    /// what is about to be said. Never sent, though — it lands in the field to
    /// be read once more, and edited if the moment has moved on.
    ///
    /// Returns where the caret should end up, which is the end of it.
    pub fn apply_template(&mut self, index: usize) -> Option<usize> {
        let text = match self.expand_template(index)? {
            Ok(text) => text,
            Err(error) => {
                self.report(&error);
                return None;
            }
        };
        self.transmit.draft = text;
        Some(self.transmit.draft.chars().count())
    }

    /// Writes `text` into the draft at `insert_at`, and says where that left
    /// the caret.
    fn write_into_draft(&mut self, text: &str, insert_at: usize) -> usize {
        let at = insert_at.min(self.transmit.draft.chars().count());
        let byte = self
            .transmit
            .draft
            .char_indices()
            .nth(at)
            .map_or(self.transmit.draft.len(), |(index, _)| index);
        self.transmit.draft.insert_str(byte, text);
        at + text.chars().count()
    }

    /// Takes the callsign of the station being worked from the received text.
    ///
    /// This path does not go through the field, so it asks the directory for
    /// itself: a callsign double-clicked out of the received text is as much a
    /// station being worked as one that was typed.
    pub fn set_contact_callsign(&mut self, callsign: &str) {
        self.contact.callsign = text::normalize(callsign);
        self.look_up_contact();
        self.reflect_to_logger();
    }

    /// Takes up the contact fields once the operator has left one.
    ///
    /// Each of them is a value rather than text — a callsign, a name, a
    /// report — and none of them means anything with spaces around it, so what
    /// is taken up is the trimmed field. Trimmed on leaving rather than on
    /// every keystroke, because a space deleted from under the cursor is one
    /// the operator is still typing.
    pub fn finish_contact_edit(&mut self) {
        for field in [
            &mut self.contact.callsign,
            &mut self.contact.name,
            &mut self.contact.qth,
            &mut self.contact.rst_sent,
            &mut self.contact.rst_received,
        ] {
            trim_in_place(field);
        }
        // Leaving the field is what commits the callsign whether or not
        // anything was trimmed, and the request guards itself against being
        // made twice for one station.
        self.look_up_contact();
        self.reflect_to_logger();
    }

    /// Empties the contact fields, ready for the next station.
    ///
    /// The guard goes with them, so the station just cleared can be typed
    /// again and asked about again.
    pub fn clear_contact(&mut self) {
        self.contact.clear();
        self.directory.forget_request();
        self.reflect_to_logger();
    }

    fn look_up_contact(&mut self) {
        self.directory.look_up(&self.contact.callsign);
    }

    pub fn open_contact(&mut self) {
        self.directory.open(&self.settings.contact.fields);
    }

    pub fn add_contact_field(&mut self) {
        self.directory.add_field();
    }

    pub fn commit_contact(&mut self) {
        self.directory.commit(&self.contact.callsign);
    }

    fn poll_contact(&mut self) {
        if self.directory.poll() {
            self.adopt_contact_fields();
            self.reflect_to_logger();
        }
    }

    /// Turns sending the contact fields to the operator's logger on or off,
    /// and sends them at once when it was turned on.
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

    /// Whether the logger is sharing its entry form, which the contact fields
    /// then follow instead of being edited.
    pub fn logger_synced(&self) -> bool {
        self.logger.is_synced()
    }

    /// Takes the logger's entry form into the contact fields.
    ///
    /// The logger's form is the one the contact is logged from, so it wins.
    /// Only when the logger has just been found again does a field it has
    /// nothing for keep what the panel holds, which is then sent to it: the
    /// panel was the form while the logger was away.
    fn poll_logger(&mut self) {
        let Some(FormUpdate { form, resumed }) = self.logger.take_form() else {
            return;
        };
        let pick = |theirs: &str, own: &str| if resumed && theirs.is_empty() { own } else { theirs }.to_owned();
        let contact = Contact {
            callsign: pick(form.get("CALL"), &self.contact.callsign),
            name: pick(form.get("NAME"), &self.contact.name),
            qth: pick(form.get("QTH"), &self.contact.qth),
            rst_sent: pick(form.get("RST_SENT"), &self.contact.rst_sent),
            rst_received: pick(form.get("RST_RCVD"), &self.contact.rst_received),
        };
        let new_call = contact.callsign != self.contact.callsign;
        self.contact = contact;
        if new_call {
            self.look_up_contact();
        }
        self.reflect_to_logger();
    }

    /// Takes the logger's form in until `done` holds, for a test whose fake
    /// logger answers on its own thread.
    #[cfg(test)]
    pub(crate) fn await_logger(&mut self, done: impl Fn(&Self) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            self.poll_logger();
            if done(self) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "the logger's form never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Fills the logger's entry form in from the contact fields.
    ///
    /// The frequency and the mode are left to the logger, which reads them
    /// from the rig itself. Called where a field is settled rather than on
    /// every keystroke, so the logger is not handed half a callsign.
    fn reflect_to_logger(&mut self) {
        let fields = [
            ("CALL", self.contact.callsign.as_str()),
            ("NAME", self.contact.name.as_str()),
            ("QTH", self.contact.qth.as_str()),
            ("RST_SENT", self.contact.rst_sent.as_str()),
            ("RST_RCVD", self.contact.rst_received.as_str()),
        ];
        if let Err(error) = self.logger.reflect(fields) {
            self.report_logger_error(&error);
        }
    }

    fn report_logger_error(&mut self, error: &grayline_logger_link::LinkError) {
        let message = self.i18n.text("error-logger");
        self.notice = Some(format!("{message}: {error}"));
    }

    /// Writes what the directory answered into the fields left empty.
    ///
    /// Only the empty ones: what is in the panel is what the operator heard on
    /// the air, and an answer out of a store is not a reason to argue with it.
    /// A station the operator has since moved on from is passed over too — the
    /// answer is about whoever was asked about, which is not necessarily who
    /// is being worked by the time it arrives.
    fn adopt_contact_fields(&mut self) {
        if grayline_qso::normalize_callsign(&self.contact.callsign).as_deref()
            != Some(self.directory.snapshot.callsign.as_str())
        {
            return;
        }
        if self.contact.name.is_empty()
            && let Some(name) = self.sendable_field(["name_latin", "name"])
        {
            self.contact.name = name;
        }
        if self.contact.qth.is_empty()
            && let Some(qth) = self.sendable_field(["qth_latin", "qth"])
        {
            self.contact.qth = qth;
        }
    }

    /// The first of `keys` the directory answered with something this mode
    /// could actually send.
    ///
    /// The Latin spelling is asked for first because ITA2 carries no kanji: a
    /// name filed in one is perfectly good in the store, which the SSTV
    /// application prints as an image, and unsendable here. A value with no
    /// code for it is passed over rather than written into a field that would
    /// then hold the send button down.
    fn sendable_field(&self, keys: [&str; 2]) -> Option<String> {
        keys.into_iter()
            .filter_map(|key| self.directory.snapshot.fields.get(key))
            .map(|value| text::normalize(value.trim()))
            .find(|value| !value.is_empty() && text::first_unsendable(value).is_none())
    }

    /// Whether there is a message to send and a way to send it.
    pub fn can_send(&self) -> bool {
        !self.transmit.draft.trim().is_empty()
            && self.unsendable_character().is_none()
            && !self.lacks_output()
            && self.rig_problem().is_none()
    }

    /// Whether a transmission has nowhere to go.
    ///
    /// Only the audio path needs a device. A keyer that is not running is
    /// found out by the transmission that tries it, because nothing here holds
    /// a connection to it between messages.
    pub fn lacks_output(&self) -> bool {
        !self.settings.keyer.enabled && self.audio.output_device.is_none()
    }

    /// How a transmission reaches the rig.
    pub const fn keying(&self) -> Keying {
        if self.settings.keyer.enabled {
            Keying::Fsk
        } else {
            Keying::Audio
        }
    }

    /// Queues the draft.
    ///
    /// It goes out when whatever is ahead of it has finished, which is what
    /// lets the operator keep writing while the rig is keying.
    pub fn send_draft(&mut self) {
        if !self.can_send() {
            if self.lacks_output() {
                let message = self.i18n.text("error-no-output");
                self.notice = Some(message);
            } else if let Some(problem) = self.rig_problem() {
                self.notice = Some(problem);
            }
            return;
        }
        self.transmit.queue_draft();
    }

    /// Stops everything and gives the unsent text back to the draft.
    ///
    /// The playback stream is dropped rather than drained, so the carrier
    /// stops where the operator pressed rather than at the end of what had
    /// already been generated.
    pub fn abort_transmission(&mut self) {
        if !self.transmit.is_busy() {
            return;
        }
        let unsent = self.transmit.abandon();
        self.return_to_draft(unsent);
    }

    /// Puts text that was never sent back in front of what is being written.
    ///
    /// In front rather than after, because it is the older of the two: the
    /// operator queued it before typing whatever is in the field now.
    fn return_to_draft(&mut self, unsent: String) {
        if unsent.is_empty() {
            return;
        }
        if self.transmit.draft.is_empty() {
            self.transmit.draft = unsent;
        } else {
            self.transmit.draft = format!("{unsent}\n{}", self.transmit.draft);
        }
    }

    /// Whether a message is being keyed right now.
    pub fn is_transmitting(&self) -> bool {
        self.transmit
            .on_air()
            .is_some_and(|on_air| on_air.output().snapshot().phase.is_active())
    }

    /// How long the message on the air has left, if one is on the air.
    pub fn transmission_remaining(&self) -> Option<Duration> {
        Some(self.transmit.on_air()?.remaining())
    }

    /// How much of the message on the air has left for the rig.
    pub fn sent_progress(&self) -> Option<transmit::SentProgress> {
        Some(self.transmit.on_air()?.progress())
    }

    /// Starts rig control if the file asked for it, and reads what it says.
    ///
    /// The worker is started here rather than at construction because nothing
    /// in a test asks for a rig: the setting is off by default, and a station
    /// that switched it on gets its transports opened on the first frame.
    fn poll_rig(&mut self) {
        if self.settings.rig.enabled && self.rig_worker.is_none() {
            let (plan, problem) = BandPlan::load(self.paths.config_dir());
            // Logged rather than shown: nothing here draws a band, so a plan
            // that would not parse costs the script its `ctx.band` and costs
            // the operator nothing they are looking at.
            if let Some(problem) = problem {
                log::note(&problem);
            }
            self.rig_worker = Some(RigWorker::spawn(
                &self.settings.rig,
                self.paths.config_dir(),
                Arc::new(plan),
                log::note,
            ));
        }
        if let Some(worker) = self.rig_worker.as_ref() {
            self.rig_snapshot = worker.latest();
        }
        if self.should_unkey() {
            if let Some(worker) = self.rig_worker.as_ref() {
                worker.receive();
            }
            self.rig_keyed = false;
        }
    }

    /// Whether the audio may start.
    ///
    /// A transmission that keyed the rig waits for the rig to say it is
    /// transmitting, because the lead-in the rig needs to switch over is time
    /// anything sent into it is simply lost. The transmitter's own mark lead-in
    /// is generated either way: it is there for the receiving station's
    /// integrators, which is a different half-second from this one.
    pub(crate) const fn rig_ready_to_send(&self) -> bool {
        !self.rig_keyed || matches!(self.rig_snapshot.state, RigState::Transmitting)
    }

    /// Whether the rig has nothing left to send and may be unkeyed.
    ///
    /// The queue counts as well as the stream: a message waiting behind the
    /// one that just ended keeps the carrier up, because unkeying between the
    /// two would drop it in the middle of what the other station reads as one
    /// transmission.
    pub(crate) fn should_unkey(&self) -> bool {
        self.rig_keyed && !self.transmit.is_busy()
    }

    /// Reports why the rig could not be handed a transmission, if it could not.
    ///
    /// A rig that was asked to key and did not leaves a message nobody hears,
    /// so it holds the send button rather than being let through quietly.
    pub fn rig_problem(&self) -> Option<String> {
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

    fn poll_transmit(&mut self) {
        let ready = self.rig_ready_to_send();
        let rig_error = self.rig_snapshot.error.as_ref().filter(|_| self.rig_keyed);
        if let Some(on_air) = self.transmit.on_air_mut() {
            let advance = on_air.advance(ready, rig_error);
            if let Some(echo) = advance.echo {
                self.echo_sent(&echo);
            }
            if advance.underran {
                self.notice = Some(self.i18n.text("error-underrun"));
            }
            match advance.ending {
                None => {}
                // The unsent text goes back to the draft rather than being
                // dropped, as it does when the operator stops a transmission
                // themselves.
                Some(Ending::RigRefused(error)) => {
                    self.abort_transmission();
                    self.notice = Some(error.to_string());
                    return;
                }
                // What had not gone out goes back to the draft, the queue
                // behind it included, as it does when the rig fails: a keyer
                // that is not running fails every message the same way, and
                // the text is still what the operator wrote.
                Some(Ending::Failed(error)) => {
                    self.abort_transmission();
                    if let Some(error) = error {
                        self.report(&error);
                    }
                    return;
                }
                Some(Ending::Finished { late }) => {
                    // Logged rather than shown: it is what says whether a
                    // serial adapter is fit to key with, which is a question
                    // asked once while setting a station up rather than on
                    // every message.
                    if let Some(late) = late {
                        log::note(&format!("FSK keyer: worst transition {} µs late", late.as_micros()));
                    }
                    self.transmit.finish();
                }
            }
        }
        if let Some(text) = self.transmit.take_next() {
            self.start_transmission(text);
        }
    }

    /// Opens a stream for `text` and hands it to a worker.
    ///
    /// The schedule is built against the rate the device actually opened at
    /// rather than the one that was asked for, because that is the rate the
    /// audio is generated at and so the one the played position counts in.
    fn start_transmission(&mut self, text: String) {
        // Asked for before the audio is generated, so the lead-in the rig
        // needs runs alongside filling the queue rather than after it. The
        // transmitter's own mark lead-in is a separate half-second, and the
        // two add up: it is there for the receiving integrators rather than
        // for the rig, which is why neither replaces the other.
        if let Some(worker) = self.rig_worker.as_ref()
            && !self.rig_keyed
        {
            worker.transmit(self.keying());
            self.rig_keyed = true;
        }
        match self.open_transmission(&text) {
            Ok((schedule, output)) => self.transmit.begin(Sending::new(text, schedule), output),
            Err(error) => {
                self.report(&error);
                // The message had already left the queue, so it is given back
                // rather than dropped: what could not be sent is still what
                // the operator wrote, and nothing else would tell them it had
                // gone.
                let returned = self.transmit.abandon_queue(text);
                self.return_to_draft(returned);
            }
        }
    }

    /// Starts `text` on its output and returns where its characters land.
    ///
    /// The keyer counts in microseconds of its own timeline, so a message it
    /// keys is scheduled at that rate, as one played is scheduled at the
    /// device's.
    fn open_transmission(&self, text: &str) -> Result<(TxSchedule, Output), AppError> {
        let config = self.tx_config();
        let codes = encode_text(text, &config)?;
        if self.settings.keyer.enabled {
            let schedule = TxSchedule::new(text, KEYER_TICKS_PER_SECOND, &config)?;
            let worker = KeyerWorker::spawn(self.settings.keyer.address.clone(), codes, config);
            return Ok((schedule, Output::keyer(worker)));
        }
        let (playback, writer) = self.audio.open_playback(PLAYBACK_CAPACITY_SAMPLES)?;
        let schedule = TxSchedule::new(text, playback.sample_rate_hz(), &config)?;
        Ok((
            schedule,
            Output::audio(playback, TxWorker::spawn(writer, codes, config)),
        ))
    }

    fn echo_sent(&mut self, text: &str) {
        for column in &mut self.columns {
            column.push_sent(text);
        }
    }

    /// Throws away everything printed so far.
    pub fn clear(&mut self) {
        for column in &mut self.columns {
            column.clear();
        }
    }

    /// Starts the decode paths over, for a receiver that has lost its place.
    ///
    /// A missed start bit corrupts everything until the line idles long
    /// enough to resynchronize, and this is the operator's way of not waiting
    /// for that. What has been printed is kept: it is what was received.
    pub fn resynchronize(&self) {
        self.audio.reset_reception();
    }

    /// Decodes a recording in place of the capture device.
    pub fn open_wav(&mut self, path: &std::path::Path) {
        self.clear();
        match self.audio.play_file(path) {
            Ok(()) => {
                let message = self.i18n.text("status-reading");
                self.notice = Some(format!("{message}: {}", path.display()));
            }
            Err(error) => {
                let message = self.i18n.text("error-wav");
                self.notice = Some(format!("{message}: {error}"));
            }
        }
    }

    pub fn reveal(&mut self, folder: Folder) {
        let path = self.paths.folder(folder).to_path_buf();
        if let Err(error) = self.platform.open_path(&path) {
            let message = self.i18n.text("error-open-folder");
            self.notice = Some(format!("{message}: {error}"));
        }
    }

    /// Opens the operator's manual, which is published on the web rather than
    /// carried beside the executable.
    pub fn open_manual(&mut self) {
        let url = grayline_shell::manual_url(&crate::identity::IDENTITY);
        if let Err(error) = self.platform.open_url(&url) {
            let message = self.i18n.text("error-open-manual");
            self.notice = Some(format!("{message}: {error}"));
        }
    }

    fn report(&mut self, error: &AppError) {
        let text = error.to_string();
        if self.notice.as_deref() != Some(text.as_str()) {
            log::note(&text);
            self.notice = Some(text);
        }
    }
}

fn trim_in_place(text: &mut String) -> bool {
    let trimmed = text.trim();
    if trimmed.len() == text.len() {
        return false;
    }
    let trimmed = trimmed.to_owned();
    *text = trimmed;
    true
}

/// The amplitude a fader position stands for.
///
/// Squared for the reason recorded beside the SSTV application's own fader: a
/// control that scaled amplitude directly would spend its upper half on levels
/// that all sound about the same. Never exactly zero, because a transmitter
/// cannot be built from a silent one, and a level set to nothing is a fader at
/// the bottom rather than a fault.
fn tx_amplitude(travel: f64) -> f64 {
    let travel = travel.clamp(0.0, 1.0);
    (travel * travel).max(f64::EPSILON)
}

fn worker_settings(settings: &Settings) -> WorkerSettings {
    WorkerSettings {
        tones: ToneSet {
            mark_hz: settings.mark_hz,
            space_hz: settings.mark_hz + settings.shift_hz,
        },
        baud: settings.baud,
        reverse: settings.reverse,
        afc: settings.afc,
        // Zero is the squelch being off: nothing is quieter than a signal
        // that reads nothing, and the core takes the absence of a threshold as
        // the squelch being clamped open.
        squelch: (settings.squelch_threshold > 0.0).then_some(settings.squelch_threshold),
        unshift_on_space: settings.unshift_on_space,
        atc: settings.atc,
    }
}

impl core::fmt::Debug for App {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("App")
            .field("locale", &self.i18n.locale())
            .field("tones", &self.tones())
            .field("baud", &self.settings.baud)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
