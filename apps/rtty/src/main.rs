#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{error::Error, path::PathBuf};

mod app;
mod error;
mod identity;
mod locales;
mod storage;
mod ui;
mod worker;

#[cfg(test)]
mod test_util;

use app::App;
use grayline_shell::{log, platform};
use storage::paths;
use ui::{menu, view};

/// The name the desktop environment knows this application by.
///
/// Set explicitly because eframe otherwise derives it from the window title,
/// which carries the version, so the identity would change with every release.
const APP_ID: &str = identity::PROCESS_NAME;

/// The window size the interface is laid out for, in points.
///
/// Wide enough for a full teleprinter line beside the settings panel, and
/// tall enough that the scrollback holds more than the last few exchanges.
const DEFAULT_WINDOW_SIZE: [f32; 2] = [1100.0, 720.0];

/// How much of the monitor the window may take up when it first opens.
const MONITOR_FRACTION: f32 = 0.92;

fn main() -> Result<(), Box<dyn Error>> {
    platform::prepare_process(&identity::IDENTITY);

    // A second copy would fail to open the audio device the first one holds,
    // which is harder to understand than not opening at all.
    let Some(instance) = platform::claim_single_instance(&identity::IDENTITY) else {
        return Ok(());
    };

    let paths = paths::AppPaths::discover()?;
    paths.initialize()?;
    if let Err(error) = log::open(paths.log_file(), &identity::IDENTITY) {
        eprintln!("could not open the log file: {error}");
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id(APP_ID)
        .with_clamp_size_to_monitor_size(false)
        .with_inner_size(DEFAULT_WINDOW_SIZE);
    match platform::window_icon(&identity::IDENTITY) {
        Some(icon) => viewport = viewport.with_icon(icon),
        None => log::note("could not load the application icon; using the platform default"),
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let recording = std::env::args_os().nth(1).map(PathBuf::from);
    eframe::run_native(
        &identity::IDENTITY.title(),
        options,
        Box::new(move |cc| Ok(Box::new(Interface::new(cc, paths, instance, recording)))),
    )?;
    Ok(())
}

struct Interface {
    app: App,
    menu: Option<menu::MenuHost>,
    title: String,
    /// Set when the opening size has been measured against the monitor.
    fitted: bool,
    /// Set when the operator chose to quit, so the frame can finish drawing
    /// and persist before the window closes.
    quitting: bool,
    _instance: platform::SingleInstance,
}

/// Returns a recording dropped onto the window since the last frame.
///
/// Dropping a file is how a recording is opened: the application keeps no file
/// browser of its own, for the same reason it opens its directories in the
/// operator's file manager rather than listing them.
fn dropped_recording(ctx: &egui::Context) -> Option<PathBuf> {
    ctx.input(|input| input.raw.dropped_files.iter().find_map(|file| file.path.clone()))
}

/// Shrinks the window to fit the monitor it opened on.
///
/// Returns whether the monitor was known yet, not whether anything was
/// resized: a window that already fits is left alone.
fn fit_to_monitor(ctx: &egui::Context) -> bool {
    let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else {
        return false;
    };
    let wanted = egui::Vec2::from(DEFAULT_WINDOW_SIZE);
    let fitted = wanted.min(monitor * MONITOR_FRACTION);
    if fitted != wanted {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(fitted));
    }
    true
}

impl Interface {
    fn new(
        cc: &eframe::CreationContext<'_>,
        paths: paths::AppPaths,
        instance: platform::SingleInstance,
        recording: Option<PathBuf>,
    ) -> Self {
        grayline_shell::fonts::install(&cc.egui_ctx);
        platform::prepare_window(cc);
        instance.publish_window(cc);

        let mut app = App::new(paths, grayline_shell::worker::Waker::new(cc.egui_ctx.clone()));
        if let Some(path) = recording {
            app.open_wav(&path);
        }
        cc.egui_ctx.set_zoom_factor(app.ui_scale);
        let model = menu::model(&app);
        let menu = match menu::MenuHost::install(cc, &model) {
            Ok(menu) => Some(menu),
            Err(error) => {
                log::note(&format!("could not install the platform menu bar: {error}"));
                None
            }
        };
        Self {
            title: app.title(),
            app,
            menu,
            fitted: false,
            quitting: false,
            _instance: instance,
        }
    }
}

impl Drop for Interface {
    fn drop(&mut self) {
        if let Some(menu) = &self.menu {
            menu.prepare_for_close();
        }
    }
}

impl eframe::App for Interface {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.fitted {
            self.fitted = fit_to_monitor(ui.ctx());
        }
        if let Some(path) = dropped_recording(ui.ctx()) {
            self.app.open_wav(&path);
        }
        self.app.poll_workers();

        self.app.set_ui_scale(ui.ctx().zoom_factor());

        let model = menu::model(&self.app);
        if let Some(native) = self.menu.as_mut() {
            native.sync(&model);
            for action in native.poll() {
                self.quitting |= menu::apply(&mut self.app, action);
            }
        }
        let in_window_menu = menu::is_in_window() || self.menu.is_none();
        if let Some(action) = view::view(ui, &mut self.app, &model, in_window_menu) {
            self.quitting |= menu::apply(&mut self.app, action);
        }

        if ui.ctx().zoom_factor() != self.app.ui_scale {
            ui.ctx().set_zoom_factor(self.app.ui_scale);
        }

        let title = self.app.title();
        if title != self.title {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        self.app.persist();
        // The receive worker asks for a frame itself when it has decoded
        // something worth showing, so the interface only schedules the frames
        // nothing else would produce.
        if let Some(after) = self.app.repaint_after() {
            ui.ctx().request_repaint_after(after);
        }
        if self.quitting {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
