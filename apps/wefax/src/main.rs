#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{error::Error, path::PathBuf, sync::Arc};

use egui::{FontData, FontDefinitions, FontFamily};

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
use grayline_shell::log;
use grayline_shell::platform::{self, UI_FONTS};
use storage::paths;
use ui::{menu, view};

/// Draws the interface with the platform's UI font.
///
/// The system families are installed ahead of the bundled fonts, so Latin and
/// Japanese come from one face, while a machine with none of them installed
/// still renders text.
///
/// The font database is queried directly because the face index matters:
/// Windows ships `Yu Gothic UI` as the second face of a collection whose first
/// face is `Yu Gothic`, and that first face carries a half-em line gap.
fn install_fonts(ctx: &egui::Context) {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    ctx.set_fonts(font_definitions(&database));
}

fn font_definitions(database: &fontdb::Database) -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    let mut installed = Vec::new();
    for family in UI_FONTS {
        let Some((data, index)) = load_face(database, family) else {
            continue;
        };
        definitions.font_data.insert(
            (*family).to_owned(),
            Arc::new(FontData {
                font: data.into(),
                index,
                tweak: egui::FontTweak::default(),
            }),
        );
        installed.push((*family).to_owned());
    }

    if installed.is_empty() {
        log::note("no system UI font matched; using the bundled fonts");
    }
    for family in installed.iter().rev() {
        for target in [FontFamily::Proportional, FontFamily::Monospace] {
            definitions
                .families
                .entry(target)
                .or_default()
                .insert(0, family.clone());
        }
    }
    definitions
}

fn load_face(database: &fontdb::Database, family: &str) -> Option<(Vec<u8>, u32)> {
    let id = database.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    })?;
    let (source, index) = database.face_source(id)?;
    let data = match source {
        fontdb::Source::File(path) => std::fs::read(path).ok()?,
        fontdb::Source::Binary(data) => data.as_ref().as_ref().to_vec(),
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    Some((data, index))
}

/// The name the desktop environment knows this application by.
///
/// Set explicitly because eframe otherwise derives it from the window title,
/// which carries the version, so the identity would change with every release.
const APP_ID: &str = identity::PROCESS_NAME;

/// The window size the interface is laid out for, in points.
///
/// Wider than it is tall, because the picture is: a WEFAX line is turned a
/// quarter turn so that time runs across the window.
const DEFAULT_WINDOW_SIZE: [f32; 2] = [1280.0, 720.0];

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
    if let Err(error) = log::open(paths.log_file(), identity::DISPLAY_NAME) {
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
    // A recording named on the command line is decoded instead of the device,
    // which is how a file reaches the application from a file manager's "open
    // with" as well as from a shell.
    let recording = std::env::args_os().nth(1).map(PathBuf::from);
    eframe::run_native(
        &format!("{} {}", identity::DISPLAY_NAME, env!("CARGO_PKG_VERSION")),
        options,
        Box::new(move |cc| Ok(Box::new(Interface::new(cc, paths, instance, recording)))),
    )?;
    Ok(())
}

/// The eframe entry point, holding the application and its menu bar.
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
        install_fonts(&cc.egui_ctx);
        platform::prepare_window(cc);
        instance.publish_window(cc);

        let mut app = App::new(paths, worker::Waker::new(cc.egui_ctx.clone()));
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
        self.app.poll_workers(ui.ctx());

        // The zoom shortcuts and the menu both change the scale; whichever
        // route the operator took, the result is one value that gets persisted.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A system with none of the wanted families still has to render text.
    #[test]
    fn an_empty_database_leaves_the_bundled_fonts_usable() {
        let definitions = font_definitions(&fontdb::Database::new());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            assert!(!definitions.families[&family].is_empty());
        }
    }

    #[test]
    fn a_matched_family_is_installed_ahead_of_the_bundled_fonts() {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let Some(wanted) = UI_FONTS.iter().find(|family| load_face(&database, family).is_some()) else {
            return; // No system font to check against on this machine.
        };

        let definitions = font_definitions(&database);
        let bundled = FontDefinitions::default();
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let installed = &definitions.families[&family];
            assert_eq!(installed.first().map(String::as_str), Some(*wanted));
            assert!(
                installed.len() > bundled.families[&family].len(),
                "the bundled fonts should still be behind the system ones"
            );
        }
    }
}
