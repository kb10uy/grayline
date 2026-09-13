//! The application's state, and every command the interface can issue.
//!
//! The interface reads these fields and calls these methods; nothing here
//! draws. Keeping the two apart is what lets the whole of the state be driven
//! from a test without a window.

use std::time::Duration;

use grayline_shell::{
    common::{CommonConfig, CommonSettings, UI_SCALE_RANGE},
    i18n::{I18n, Locale},
    log,
    platform::{Activity, Platform},
};
use grayline_wefax::{Format, Ioc, LinesPerMinute};

use crate::{
    error::AppError,
    locales::CATALOG,
    storage::{
        config::{Config, Settings},
        history,
        paths::{AppPaths, Folder},
    },
    ui::strip::Strip,
    worker::{
        Waker,
        audio::AudioState,
        receive::{Chart, RxProgress, WorkerSettings},
    },
};

/// How far one press of the phase controls moves the picture, in pixels.
pub const PHASE_STEP_PIXELS: i64 = 16;
/// How far the coarse phase controls move it.
pub const PHASE_COARSE_PIXELS: i64 = 128;

/// How far one press of the line-rate controls moves the clock, in parts per
/// million.
///
/// Ten parts per million leans an IOC 576 chart by about eighteen pixels over
/// a thousand lines, which is the point at which a lean starts to be visible.
pub const SLANT_STEP_PPM: f64 = 5.0;
/// How far the coarse line-rate controls move it.
pub const SLANT_COARSE_PPM: f64 = 50.0;

/// Everything the application is, other than the pixels on screen.
pub struct App {
    pub i18n: I18n,
    pub audio: AudioState,
    pub strip: Strip,
    pub ui_scale: f32,

    /// The geometry a reception starts on, and falls back to.
    pub ioc: Ioc,
    pub lines_per_minute: LinesPerMinute,
    pub auto_start: bool,
    pub auto_stop: bool,
    pub infer_lines_per_minute: bool,
    pub slant_tracking: bool,
    pub inverted: bool,
    pub narrow_shift: bool,
    pub auto_save: bool,

    /// The last thing worth telling the operator, shown on the control bar.
    pub notice: Option<String>,
    /// The geometry the receiver settled on, once it has.
    pub format: Option<Format>,

    paths: AppPaths,
    config: Config,
    /// The settings last written, so a frame that changed nothing writes
    /// nothing.
    saved: Settings,
    /// The language and the scale, which the whole family shares.
    common: CommonConfig,
    platform: Box<dyn Platform>,
    activity: Activity,
    /// The session the strip on screen belongs to.
    ///
    /// A device change starts a new worker whose lines begin again at zero, so
    /// the strip has to be cleared rather than appended to.
    session: u64,
}

impl App {
    pub fn new(paths: AppPaths, waker: Waker) -> Self {
        let (config, settings) = Config::load(paths.config_file().to_path_buf());
        let common = CommonConfig::discover();
        let audio = AudioState::new(settings.device.as_deref(), worker_settings(&settings), waker);
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
            strip: Strip::default(),
            ui_scale: shared.ui_scale,
            ioc: settings.ioc,
            lines_per_minute: settings.lines_per_minute,
            auto_start: settings.auto_start,
            auto_stop: settings.auto_stop,
            infer_lines_per_minute: settings.infer_lines_per_minute,
            slant_tracking: settings.slant_tracking,
            inverted: settings.inverted,
            narrow_shift: settings.narrow_shift,
            auto_save: settings.auto_save,
            notice: None,
            format: None,
            session,
            paths,
            config,
            saved: settings.clone(),
            common,
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
            AppPaths::from_roots(scratch.join("config"), scratch.join("pictures"), scratch.join("state")),
            Config::detached(),
            &settings,
            CommonConfig::detached(),
            platform,
        )
    }

    /// The window title, which carries what the receiver is doing.
    pub fn title(&self) -> String {
        let name = format!("{} {}", crate::identity::DISPLAY_NAME, env!("CARGO_PKG_VERSION"));
        match self.progress() {
            RxProgress::Idle => name,
            progress => format!("{} \u{2014} {}", name, self.i18n.text(progress.label_key())),
        }
    }

    pub fn progress(&self) -> RxProgress {
        self.audio.snapshot().progress
    }

    /// Returns how far the fitted line clock stands from the nominal one, in
    /// parts per million.
    ///
    /// This is what a chart leaning to one side reads as, and the operator has
    /// no other way to see the correction working.
    pub fn line_rate_error_ppm(&self) -> Option<f64> {
        let rate = self.audio.sample_rate_hz()?;
        let format = self.format?;
        let fitted = self.audio.snapshot().samples_per_line?;
        let nominal = format.samples_per_line(rate);
        (nominal > 0.0).then(|| (fitted - nominal) / nominal * 1.0e6)
    }

    /// Collects whatever the receive worker published since the last frame.
    pub fn poll_workers(&mut self, ctx: &egui::Context) {
        if let Some(fault) = self.audio.take_capture_fault() {
            self.audio.rescan();
            // A device that came back is opened again rather than left for the
            // operator to reselect: a chart takes ten minutes, and a stream
            // that stopped between two of them should not need a click.
            if !self.audio.reopen() {
                let message = self.i18n.text("error-device-lost");
                self.notice = Some(format!("{message}: {fault:?}"));
            }
        }
        let session = self.audio.session();
        if session != self.session {
            self.session = session;
            self.strip.clear();
        }

        if let Some(update) = self.audio.poll()
            && !self.strip.apply(ctx, &update)
        {
            log::note("dropped a run of columns that did not continue the strip");
        }
        self.format = self.audio.snapshot().format;
        if let Some(error) = self.audio.snapshot().error.clone() {
            self.report(&error);
        }
        if let Some(chart) = self.audio.take_chart()
            && self.auto_save
        {
            self.write_chart(&chart);
        }

        let activity = if self.progress().is_active() {
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
            ioc: self.ioc,
            lines_per_minute: self.lines_per_minute,
            auto_start: self.auto_start,
            auto_stop: self.auto_stop,
            infer_lines_per_minute: self.infer_lines_per_minute,
            slant_tracking: self.slant_tracking,
            inverted: self.inverted,
            narrow_shift: self.narrow_shift,
            auto_save: self.auto_save,
        }
    }

    /// Hands the receive settings to the worker.
    ///
    /// Called after anything on the control bar changes, rather than on every
    /// frame, so a setting the operator did not touch is not rewritten.
    pub fn push_settings(&mut self) {
        self.audio.set_settings(worker_settings(&self.settings()));
    }

    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end());
    }

    pub fn zoom_by(&mut self, delta: f32) {
        self.set_ui_scale(self.ui_scale + delta);
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

    /// Abandons the reception in progress and clears the picture.
    pub fn clear(&mut self) {
        self.audio.reset_reception();
        self.strip.clear();
        self.format = None;
    }

    /// Begins a reception here, without waiting for a start tone.
    pub fn start(&mut self) {
        self.push_settings();
        self.strip.clear();
        self.audio.start_reception();
    }

    /// Ends the reception in progress, keeping what was drawn.
    pub fn stop(&mut self) {
        self.audio.stop_reception();
    }

    pub fn shift_phase(&self, pixels: i64) {
        self.audio.shift_phase(pixels);
    }

    /// Corrects the line rate by hand, for a chart the tracker cannot fit.
    ///
    /// A chart with nothing to correlate — a mostly white one — leaves the
    /// automatic correction with no observation to work from, and this is the
    /// only way to straighten it.
    pub fn adjust_slant_ppm(&self, ppm: f64) {
        self.audio.adjust_slant_ppm(ppm);
    }

    /// Decodes a recording in place of the capture device.
    pub fn open_wav(&mut self, path: &std::path::Path) {
        self.strip.clear();
        self.format = None;
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

    /// Writes the picture on screen into the operator's pictures directory.
    pub fn save_chart(&mut self) {
        if self.strip.is_empty() {
            self.notice = Some(self.i18n.text("error-nothing-to-save"));
            return;
        }
        let chart = Chart {
            format: self.format.unwrap_or(Format::MARINE),
            width: self.strip.width(),
            height: self.strip.lines(),
            gray: self.strip.gray().to_vec(),
        };
        self.write_chart(&chart);
    }

    fn write_chart(&mut self, chart: &Chart) {
        // The picture is line-major, which is how it was decoded: the quarter
        // turn belongs to the display, not to the file.
        match history::save(
            self.paths.received_dir(),
            chart.format,
            chart.width,
            chart.height,
            &chart.gray,
        ) {
            Ok(path) => {
                let message = self.i18n.text("status-saved");
                self.notice = Some(format!("{message}: {}", path.display()));
            }
            Err(error) => {
                let message = self.i18n.text("error-save-failed");
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
        format: Format {
            ioc: settings.ioc,
            lines_per_minute: settings.lines_per_minute,
        },
        auto_start: settings.auto_start,
        auto_stop: settings.auto_stop,
        infer_lines_per_minute: settings.infer_lines_per_minute,
        slant_tracking: settings.slant_tracking,
        inverted: settings.inverted,
        narrow_shift: settings.narrow_shift,
    }
}

impl core::fmt::Debug for App {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("App")
            .field("locale", &self.i18n.locale())
            .field("format", &self.format)
            .field("strip", &self.strip)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
