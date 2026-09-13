//! The application's state, and every command the interface can issue.
//!
//! The interface reads these fields and calls these methods; nothing here
//! draws. Keeping the two apart is what lets the whole of the state be driven
//! from a test without a window.

use std::time::Duration;

use grayline_rtty::{BaudRate, ToneSet, TxConfig, TxFraming, TxSchedule, encode_text};
use grayline_shell::{
    common::{CommonConfig, CommonSettings, UI_SCALE_RANGE},
    i18n::{I18n, Locale},
    log,
    platform::{Activity, Platform},
};

use crate::{
    app::transmit::{Sending, Transmit},
    error::AppError,
    locales::CATALOG,
    storage::{
        config::{Config, MAXIMUM_MARK_HZ, MINIMUM_MARK_HZ, Settings},
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

pub mod transmit;

/// How far one point of drag moves the mark tone, in hertz.
///
/// Five, because that is about the width of the band-pass's tolerance for
/// being wrong: a pair set to within five hertz decodes, and a field that
/// moved by one would take a hundred drags to cross a mistuning.
pub const MARK_STEP_HZ: f64 = 5.0;

/// One second of playback queue at the preferred rate.
const PLAYBACK_CAPACITY_SAMPLES: usize = 48_000;

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
    pub squelch: bool,
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
            squelch: settings.squelch,
            squelch_threshold: settings.squelch_threshold,
            unshift_on_space: settings.unshift_on_space,
            atc: settings.atc,
            tx_unshift_on_space: settings.tx_unshift_on_space,
            tx_level: settings.tx_level,
            transmit: Transmit::default(),
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
            squelch: self.squelch,
            squelch_threshold: self.squelch_threshold,
            unshift_on_space: self.unshift_on_space,
            atc: self.atc,
            tx_unshift_on_space: self.tx_unshift_on_space,
            tx_level: self.tx_level,
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
        let config = self.tx_config();
        let (playback, writer) = match self.audio.open_playback(PLAYBACK_CAPACITY_SAMPLES) {
            Ok(opened) => opened,
            Err(error) => return self.report(&error),
        };
        let rate = playback.sample_rate_hz();
        let codes = match encode_text(&text, &config) {
            Ok(codes) => codes,
            Err(error) => return self.report(&AppError::Rtty(error)),
        };
        let schedule = match TxSchedule::new(&text, rate, &config) {
            Ok(schedule) => schedule,
            Err(error) => return self.report(&AppError::Rtty(error)),
        };
        self.tx.begin(playback, TxWorker::spawn(writer, codes, config));
        self.transmit.begin(Sending::new(text, schedule));
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
        squelch: settings.squelch.then_some(settings.squelch_threshold),
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
