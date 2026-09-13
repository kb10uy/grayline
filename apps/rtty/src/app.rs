//! The application's state, and every command the interface can issue.
//!
//! The interface reads these fields and calls these methods; nothing here
//! draws. Keeping the two apart is what lets the whole of the state be driven
//! from a test without a window.

use std::{collections::BTreeMap, time::Duration};

use grayline_audio::Playback;
use grayline_rtty::{BaudRate, ToneSet, TxConfig, TxFraming, TxSchedule, encode_text};
use grayline_shell::{
    common::{CommonConfig, CommonSettings, UI_SCALE_RANGE},
    i18n::{I18n, Locale},
    log,
    platform::{Activity, Platform},
};

use crate::{
    app::{
        macros::{Contact, Macro, MacroContext, Station, Template, expand, valid_variable_name},
        transmit::{Sending, Transmit},
    },
    error::AppError,
    locales::CATALOG,
    storage::{
        config::{Config, MAXIMUM_MARK_HZ, MINIMUM_MARK_HZ, Settings},
        library,
        paths::{AppPaths, Folder},
    },
    ui::scrollback::Scrollback,
    worker::{
        Waker,
        audio::{AudioState, TxState},
        receive::{ColumnSnapshot, DecodePath, WorkerSettings},
        transmit::{TxPhase, TxWorker},
    },
};

pub mod macros;
pub mod transmit;

/// How far one point of drag moves the mark tone, in hertz.
///
/// Five, because that is about the width of the band-pass's tolerance for
/// being wrong: a pair set to within five hertz decodes, and a field that
/// moved by one would take a hundred drags to cross a mistuning.
pub const MARK_STEP_HZ: f64 = 5.0;

/// One second of playback queue at the preferred rate.
const PLAYBACK_CAPACITY_SAMPLES: usize = 48_000;

/// The two lists of messages, as their files were read.
///
/// Passed in together rather than read here, because the application is also
/// built for tests that touch no disk at all.
#[derive(Default)]
struct Library {
    macros: Vec<Macro>,
    templates: Vec<Template>,
}

/// Everything the application is, other than the pixels on screen.
pub struct App {
    pub i18n: I18n,
    pub audio: AudioState,
    /// What each decode path has printed, in the order the paths are listed.
    pub columns: Vec<Scrollback>,
    pub ui_scale: f32,

    /// The tone a mark is expected at; space sits `shift_hz` above it.
    pub mark_hz: f64,
    pub shift_hz: f64,
    pub baud: f64,
    pub reverse: bool,
    pub afc: bool,
    /// The reading a signal has to beat to be printed; zero prints
    /// everything, which is the squelch being off.
    pub squelch_threshold: f64,
    pub unshift_on_space: bool,
    pub atc: bool,
    /// Whether a transmission re-announces figures after a space.
    ///
    /// Separate from the receive setting above, and on by default, because it
    /// is about the station being sent to rather than this one: a receiver
    /// with unshift-on-space would otherwise print a contest exchange after a
    /// space as letters.
    pub tx_unshift_on_space: bool,
    /// How hard the transmission is driven, as fader travel in `0..=1`.
    pub tx_level: f64,

    /// The draft, the queue behind it, and the message on the air.
    pub transmit: Transmit,
    /// Who this station is, for the macros that say so.
    pub station: Station,
    /// The station being worked, as entered beside the received text.
    pub contact: Contact,
    /// The buttons under the message field.
    pub macros: Vec<Macro>,
    /// The set messages listed beside the buttons, in the order they are
    /// offered.
    pub templates: Vec<Template>,
    /// The operator's own fields, reached from a macro as `${custom.<name>}`.
    pub custom_variables: BTreeMap<String, String>,
    /// Whether the window naming this station is open.
    pub station_dialog_open: bool,
    /// The rows the window is editing.
    ///
    /// Edited apart from `custom_variables` because a name is half typed for
    /// as long as it takes to type it, and a half-typed name is a different
    /// field: the rows are taken up once they are usable, and an unusable one
    /// stays on screen to be corrected rather than disappearing.
    pub variables_draft: Vec<(String, String)>,

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
    /// The playback stream and worker the message on the air is running on.
    tx: TxState,
    /// The session an underrun was last reported for, so one gap in the
    /// audio is one notice rather than one per frame.
    underran: Option<u64>,
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
            waker,
        );
        let mut app = Self::from_parts(
            audio,
            paths,
            config,
            &settings,
            Library { macros, templates },
            common,
            grayline_shell::platform::host(),
        );
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
        audio: AudioState,
        paths: AppPaths,
        config: Config,
        settings: &Settings,
        library: Library,
        common: CommonConfig,
        platform: Box<dyn Platform>,
    ) -> Self {
        let session = audio.session();
        let shared = common.settings();
        Self {
            i18n: I18n::new(shared.locale, &CATALOG),
            audio,
            columns: DecodePath::ALL.map(|_| Scrollback::default()).into_iter().collect(),
            ui_scale: shared.ui_scale,
            mark_hz: settings.mark_hz,
            shift_hz: settings.shift_hz,
            baud: settings.baud,
            reverse: settings.reverse,
            afc: settings.afc,
            squelch_threshold: settings.squelch_threshold,
            unshift_on_space: settings.unshift_on_space,
            atc: settings.atc,
            tx_unshift_on_space: settings.tx_unshift_on_space,
            tx_level: settings.tx_level,
            transmit: Transmit::default(),
            station: settings.station.clone(),
            contact: Contact::default(),
            macros: library.macros,
            templates: library.templates,
            custom_variables: settings.custom_variables.clone(),
            station_dialog_open: false,
            variables_draft: Vec::new(),
            notice: None,
            session,
            paths,
            config,
            saved: settings.clone(),
            common,
            platform,
            activity: Activity::Idle,
            tx: TxState::default(),
            underran: None,
        }
    }

    /// Builds an application that touches neither the host nor the disk.
    #[cfg(test)]
    pub fn headless() -> Self {
        Self::headless_on(Box::new(grayline_shell::platform::QuietPlatform))
    }

    #[cfg(test)]
    pub fn headless_on(platform: Box<dyn Platform>) -> Self {
        let settings = Settings::default();
        let scratch = crate::test_util::scratch_dir();
        Self::from_parts(
            AudioState::disconnected(worker_settings(&settings)),
            AppPaths::from_roots(scratch.join("config"), scratch.join("state")),
            Config::detached(),
            &settings,
            Library {
                macros: crate::app::macros::default_macros(),
                templates: crate::app::macros::default_templates(),
            },
            CommonConfig::detached(),
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
        format!("{} {}", crate::identity::DISPLAY_NAME, env!("CARGO_PKG_VERSION"))
    }

    /// The pair the receiver is listening for, before any AFC movement.
    pub fn tones(&self) -> ToneSet {
        ToneSet {
            mark_hz: self.mark_hz,
            space_hz: self.mark_hz + self.shift_hz,
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
                self.notice = Some(format!("{message}: {fault:?}"));
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
        if let Some(error) = self.audio.snapshot().error.clone() {
            self.report(&error);
        }

        self.poll_transmit();

        // A watch that is printing keeps the machine awake, and so does one
        // that is sending; a watch listening to an empty band does not.
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

    /// Returns how long the interface may sleep for.
    ///
    /// The worker asks for a frame itself when it has something worth showing,
    /// so this only covers the readouts nothing else would redraw. A
    /// transmission is the exception: nothing wakes the interface as the
    /// underline advances, so it is drawn at a rate the eye reads as motion.
    pub fn repaint_after(&self) -> Option<Duration> {
        if self.tx.is_running() {
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
        let settings = self.settings();
        if settings == self.saved {
            return;
        }
        self.config.save(&settings);
        self.saved = settings;
    }

    fn settings(&self) -> Settings {
        Settings {
            device: self.audio.device.as_ref().map(|device| device.name().to_owned()),
            output_device: self.audio.output_device.as_ref().map(|device| device.name().to_owned()),
            mark_hz: self.mark_hz,
            shift_hz: self.shift_hz,
            baud: self.baud,
            reverse: self.reverse,
            afc: self.afc,
            squelch_threshold: self.squelch_threshold,
            unshift_on_space: self.unshift_on_space,
            atc: self.atc,
            tx_unshift_on_space: self.tx_unshift_on_space,
            tx_level: self.tx_level,
            station: self.station.clone(),
            custom_variables: self.custom_variables.clone(),
        }
        .clamped()
    }

    /// Hands the receive settings to the worker.
    ///
    /// Called after anything on the panel changes, rather than on every frame,
    /// so a setting the operator did not touch is not rewritten — and, because
    /// a receiver cannot be retuned in place, so a frame that changed nothing
    /// does not restart the decoders.
    pub fn push_settings(&mut self) {
        self.audio.set_settings(worker_settings(&self.settings()));
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
        let (mark, space) = if self.reverse {
            (tones.space_hz, tones.mark_hz)
        } else {
            (tones.mark_hz, tones.space_hz)
        };
        self.mark_hz = mark.clamp(MINIMUM_MARK_HZ, MAXIMUM_MARK_HZ);
        self.shift_hz = (space - mark).abs();
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
                baud: BaudRate::new(self.baud).unwrap_or_default(),
                ..default.framing
            },
            reverse: self.reverse,
            tx_unshift_on_space: self.tx_unshift_on_space,
            amplitude: tx_amplitude(self.tx_level),
            ..default
        }
    }

    /// The first character of the draft the transmitter has no code for.
    ///
    /// Typing cannot produce one, because the field refuses the keystroke;
    /// pasted text and an expanded macro can, and this is what holds the send
    /// button until the operator has dealt with it.
    pub fn unsendable_character(&self) -> Option<char> {
        crate::ui::input::first_unsendable(&self.transmit.draft)
    }

    /// Opens the window naming this station, with its fields loaded.
    pub fn open_station(&mut self) {
        self.variables_draft = self
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
        self.custom_variables = self
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
                station: &self.station,
                contact: &self.contact,
                custom: &self.custom_variables,
                now: &now,
            },
        )
        .map(|text| crate::ui::input::normalize(&text))
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
            // A macro that names something this application cannot fill in is
            // reported rather than written half finished: what it would put in
            // the field is a message with a gap where a callsign belongs.
            Err(error) => {
                self.report(&error);
                return None;
            }
        };
        if self.macros.get(index).is_some_and(|template| template.send) {
            if self.audio.output_device.is_none() {
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
    pub fn set_contact_callsign(&mut self, callsign: &str) {
        self.contact.callsign = crate::ui::input::normalize(callsign);
    }

    /// Whether there is a message to send and a way to send it.
    pub fn can_send(&self) -> bool {
        !self.transmit.draft.trim().is_empty()
            && self.unsendable_character().is_none()
            && self.audio.output_device.is_some()
    }

    /// Queues the draft.
    ///
    /// It goes out when whatever is ahead of it has finished, which is what
    /// lets the operator keep writing while the rig is keying.
    pub fn send_draft(&mut self) {
        if !self.can_send() {
            if self.audio.output_device.is_none() {
                let message = self.i18n.text("error-no-output");
                self.notice = Some(message);
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
        let played = self.tx.played_samples();
        self.tx.stop();
        let unsent = self.transmit.abandon(played);
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
        self.tx.latest().is_some_and(|snapshot| snapshot.phase.is_active())
    }

    /// How long the message on the air has left, if one is on the air.
    pub fn transmission_remaining(&self) -> Option<Duration> {
        let sending = self.transmit.sending()?;
        let rate = self.tx.sample_rate_hz()?;
        let remaining = sending.remaining_samples(self.tx.played_samples());
        Some(Duration::from_secs_f64(remaining as f64 / f64::from(rate)))
    }

    /// How much of the message on the air has left for the rig.
    pub fn sent_progress(&self) -> Option<transmit::SentProgress> {
        Some(self.transmit.sending()?.progress(self.tx.played_samples()))
    }

    /// Drives the message on the air, and starts the next one.
    fn poll_transmit(&mut self) {
        if self.tx.is_running() {
            let played = self.tx.played_samples();
            if let Some(sending) = self.transmit.sending_mut()
                && let Some(echo) = sending.take_echo(played)
            {
                self.echo_sent(&echo);
            }
            let snapshot = self.tx.latest().unwrap_or_default();
            match snapshot.phase {
                TxPhase::Failed => {
                    self.tx.stop();
                    self.transmit.finish();
                    if let Some(error) = snapshot.error {
                        self.report(&error);
                    }
                }
                // The device is told to start once the queue has something in
                // it, so the lead-in is not the thing that underruns.
                _ if !self.tx.is_started() && snapshot.phase != TxPhase::Priming => {
                    if let Err(error) = self.tx.start_playback() {
                        self.tx.stop();
                        self.transmit.finish();
                        self.report(&error);
                    }
                }
                _ => {}
            }
            // A queue that ran dry put a gap in the middle of a character,
            // which the station being sent to reads as noise. Nothing can be
            // done about it now, but the operator should know the message
            // that went out was not the message on screen.
            if self.tx.has_underrun() && self.underran != Some(self.session) {
                self.underran = Some(self.session);
                self.notice = Some(self.i18n.text("error-underrun"));
            }
            if self.tx.is_started() && self.tx.is_drained() {
                // Whatever is left is text the device never played, which for
                // a transmission that ran to its end is nothing.
                let played = self.tx.played_samples();
                if let Some(sending) = self.transmit.sending_mut()
                    && let Some(echo) = sending.take_echo(played)
                {
                    self.echo_sent(&echo);
                }
                self.tx.stop();
                self.transmit.finish();
            }
        }
        if !self.tx.is_running()
            && let Some(text) = self.transmit.take_next()
        {
            self.start_transmission(text);
        }
    }

    /// Opens a stream for `text` and hands it to a worker.
    ///
    /// The schedule is built against the rate the device actually opened at
    /// rather than the one that was asked for, because that is the rate the
    /// audio is generated at and so the one the played position counts in.
    fn start_transmission(&mut self, text: String) {
        match self.open_transmission(&text) {
            Ok((playback, worker, schedule)) => {
                self.tx.begin(playback, worker);
                self.transmit.begin(Sending::new(text, schedule));
            }
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

    fn open_transmission(&self, text: &str) -> Result<(Playback, TxWorker, TxSchedule), AppError> {
        let config = self.tx_config();
        let (playback, writer) = self.audio.open_playback(PLAYBACK_CAPACITY_SAMPLES)?;
        let rate = playback.sample_rate_hz();
        let codes = encode_text(text, &config)?;
        let schedule = TxSchedule::new(text, rate, &config)?;
        Ok((playback, TxWorker::spawn(writer, codes, config), schedule))
    }

    /// Prints what has been sent alongside what was received.
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
            .field("baud", &self.baud)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
