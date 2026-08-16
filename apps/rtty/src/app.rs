//! The application's state, and every command the interface can issue.
//!
//! The interface reads these fields and calls these methods; nothing here
//! draws. Keeping the two apart is what lets the whole of the state be driven
//! from a test without a window.

use std::time::Duration;

use grayline_rtty::ToneSet;
use grayline_shell::{
    i18n::{I18n, Locale},
    log,
    platform::{Activity, Platform},
};

use crate::{
    error::AppError,
    locales::CATALOG,
    storage::{
        config::{Config, MAXIMUM_MARK_HZ, MAXIMUM_UI_SCALE, MINIMUM_MARK_HZ, MINIMUM_UI_SCALE, Settings},
        paths::{AppPaths, Folder},
    },
    ui::scrollback::Scrollback,
    worker::{
        Waker,
        audio::AudioState,
        receive::{ColumnSnapshot, DecodePath, WorkerSettings},
    },
};

/// How far one press of the tuning controls moves the mark tone, in hertz.
pub const MARK_STEP_HZ: f64 = 5.0;

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

    /// The last thing worth telling the operator, shown on the status bar.
    pub notice: Option<String>,

    paths: AppPaths,
    config: Config,
    /// The settings last written, so a frame that changed nothing writes
    /// nothing.
    saved: Settings,
    platform: Box<dyn Platform>,
    activity: Activity,
    /// The session the text on screen belongs to.
    ///
    /// A device change starts a new worker whose decoders begin again, so the
    /// scrollback is cleared rather than appended to.
    session: u64,
}

impl App {
    pub fn new(paths: AppPaths, waker: Waker) -> Self {
        let (config, settings) = Config::load(paths.config_file().to_path_buf());
        let audio = AudioState::new(settings.device.as_deref(), worker_settings(&settings), waker);
        let mut app = Self::from_parts(audio, paths, config, &settings, grayline_shell::platform::host());
        if let Some(error) = app.config.error() {
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
        platform: Box<dyn Platform>,
    ) -> Self {
        let session = audio.session();
        Self {
            i18n: I18n::new(settings.locale, &CATALOG),
            audio,
            columns: DecodePath::ALL.map(|_| Scrollback::default()).into_iter().collect(),
            ui_scale: settings.ui_scale,
            mark_hz: settings.mark_hz,
            shift_hz: settings.shift_hz,
            baud: settings.baud,
            reverse: settings.reverse,
            afc: settings.afc,
            squelch: settings.squelch,
            squelch_threshold: settings.squelch_threshold,
            unshift_on_space: settings.unshift_on_space,
            atc: settings.atc,
            notice: None,
            session,
            paths,
            config,
            saved: settings.clone(),
            platform,
            activity: Activity::Idle,
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

        // A watch that is printing keeps the machine awake; one that is
        // listening to an empty band does not.
        let activity = if self.is_printing() {
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
    /// so this only covers the readouts nothing else would redraw.
    pub fn repaint_after(&self) -> Option<Duration> {
        self.audio.is_capturing().then(|| Duration::from_secs(1))
    }

    /// Writes the settings when a frame changed one.
    pub fn persist(&mut self) {
        let settings = self.settings();
        if settings == self.saved {
            return;
        }
        self.config.save(&settings);
        self.saved = settings;
    }

    fn settings(&self) -> Settings {
        Settings {
            locale: self.i18n.locale(),
            ui_scale: self.ui_scale,
            device: self.audio.device.as_ref().map(|device| device.name().to_owned()),
            mark_hz: self.mark_hz,
            shift_hz: self.shift_hz,
            baud: self.baud,
            reverse: self.reverse,
            afc: self.afc,
            squelch: self.squelch,
            squelch_threshold: self.squelch_threshold,
            unshift_on_space: self.unshift_on_space,
            atc: self.atc,
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
        self.ui_scale = scale.clamp(MINIMUM_UI_SCALE, MAXIMUM_UI_SCALE);
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
