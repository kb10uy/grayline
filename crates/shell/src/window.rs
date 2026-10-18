use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    Identity, fonts, log,
    menu::{self, Menu, MenuHost},
    platform::{self, SingleInstance},
};

/// How much of the monitor the window may take up when it first opens.
///
/// The rest is left to the panels and window decorations the desktop puts
/// around it, whose sizes the application cannot ask for.
const MONITOR_FRACTION: f32 = 0.92;

/// What one application answers the window it runs in with.
///
/// The window owns the order a frame runs in, the menu bar, the opening size,
/// and the claim on being the only copy; the application owns everything it
/// draws and everything its menus do.
pub trait Application {
    /// What the application's menus activate.
    type Action: Clone + PartialEq + 'static;

    /// Opens a file the operator handed the window, by dropping it on the
    /// window or naming it on the command line.
    ///
    /// An application with nothing to open ignores it.
    fn open_file(&mut self, _path: &Path) {}

    /// Collects whatever the workers published since the last frame.
    fn poll(&mut self, ctx: &egui::Context);

    /// The menus as they read this frame.
    fn menus(&self) -> Vec<Menu<Self::Action>>;

    /// Carries out `action`, returning whether it asked to quit.
    fn apply(&mut self, action: Self::Action) -> bool;

    /// Draws the window, with the menu bar inside it when `in_window_menu`,
    /// and returns whatever the operator activated.
    fn view(&mut self, ui: &mut egui::Ui, menus: &[Menu<Self::Action>], in_window_menu: bool) -> Option<Self::Action>;

    /// The window title as it reads this frame.
    fn title(&self) -> String;

    /// The interface scale the operator chose.
    fn ui_scale(&self) -> f32;

    /// Records the interface scale the platform reports.
    fn set_ui_scale(&mut self, scale: f32);

    /// Saves whatever changed since the last frame.
    fn persist(&mut self);

    /// How soon a frame is wanted when nothing else would ask for one.
    fn repaint_after(&self) -> Option<Duration>;
}

/// Opens the window and runs `build`'s application in it until it closes.
///
/// `window_size` is the size the interface is laid out for, in points. A
/// file named on the command line is handed to the application once it is
/// built.
pub fn run<A: Application + 'static>(
    identity: &Identity,
    window_size: [f32; 2],
    instance: SingleInstance,
    build: impl FnOnce(&eframe::CreationContext<'_>) -> A + 'static,
) -> eframe::Result<()> {
    // The app_id is set explicitly because eframe otherwise derives it from
    // the window title, which carries the version, so the identity would
    // change with every release. On Wayland it becomes the surface's `app_id`,
    // which is the only thing a compositor can match a window against a
    // desktop entry with, and therefore the only route to a window icon there:
    // winit's Wayland backend discards the pixels the icon provides. It has to
    // equal the base name of the installed desktop entry, which is why it is
    // the process name rather than the package name.
    //
    // eframe's own clamp converts the monitor to points with the scale factor
    // winit reports before the window exists. A Wayland compositor only sends
    // the fractional scale once the surface is mapped, so a fractionally
    // scaled output reads as the next whole number until then and the window
    // opens at a fraction of the size asked for. `fit_to_monitor` does the
    // same job once the real scale has arrived.
    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id(identity.process_name)
        .with_clamp_size_to_monitor_size(false)
        .with_inner_size(window_size);
    match platform::window_icon(identity) {
        Some(icon) => viewport = viewport.with_icon(icon),
        None => log::note("could not load the application icon; using the platform default"),
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let file = std::env::args_os().nth(1).map(PathBuf::from);
    eframe::run_native(
        &identity.title(),
        options,
        Box::new(move |cc| Ok(Box::new(Host::new(cc, instance, window_size, file, build)))),
    )
}

struct Host<A: Application> {
    app: A,
    menu: Option<MenuHost<A::Action>>,
    /// The title the window is currently showing.
    ///
    /// Held so the title is only pushed to the platform when it actually
    /// changes, rather than on every frame.
    title: String,
    window_size: egui::Vec2,
    /// Set when the opening size has been measured against the monitor.
    ///
    /// The monitor is not known on the first frame, so this stays clear until
    /// a frame reports one.
    fitted: bool,
    /// Set when the operator chose to quit, so the frame can finish drawing
    /// and persist before the window closes.
    quitting: bool,
    /// Held so the claim lasts as long as the window does, and is released
    /// with it so the next launch is let through.
    _instance: SingleInstance,
}

impl<A: Application> Host<A> {
    fn new(
        cc: &eframe::CreationContext<'_>,
        instance: SingleInstance,
        window_size: [f32; 2],
        file: Option<PathBuf>,
        build: impl FnOnce(&eframe::CreationContext<'_>) -> A,
    ) -> Self {
        fonts::install(&cc.egui_ctx);
        platform::prepare_window(cc);
        instance.publish_window(cc);

        let mut app = build(cc);
        if let Some(path) = file {
            app.open_file(&path);
        }
        cc.egui_ctx.set_zoom_factor(app.ui_scale());
        let menu = match MenuHost::install(cc, &app.menus()) {
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
            window_size: window_size.into(),
            fitted: false,
            quitting: false,
            _instance: instance,
        }
    }

    /// Shrinks the window to fit the monitor it opened on.
    ///
    /// Returns whether the monitor was known yet, not whether anything was
    /// resized: a window that already fits is left alone.
    fn fit_to_monitor(&self, ctx: &egui::Context) -> bool {
        let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else {
            return false;
        };
        let fitted = self.window_size.min(monitor * MONITOR_FRACTION);
        if fitted != self.window_size {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(fitted));
        }
        true
    }
}

impl<A: Application> Drop for Host<A> {
    fn drop(&mut self) {
        if let Some(menu) = &self.menu {
            menu.prepare_for_close();
        }
    }
}

impl<A: Application> eframe::App for Host<A> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.fitted {
            self.fitted = self.fit_to_monitor(&ctx);
        }
        if let Some(path) = ctx.input(|input| input.raw.dropped_files.iter().find_map(|file| file.path.clone())) {
            self.app.open_file(&path);
        }
        self.app.poll(&ctx);

        self.app.set_ui_scale(ctx.zoom_factor());

        let menus = self.app.menus();
        if let Some(native) = self.menu.as_mut() {
            native.sync(&menus);
            for action in native.poll() {
                self.quitting |= self.app.apply(action);
            }
        }
        let in_window_menu = menu::is_in_window() || self.menu.is_none();
        if let Some(action) = self.app.view(ui, &menus, in_window_menu) {
            self.quitting |= self.app.apply(action);
        }

        if ctx.zoom_factor() != self.app.ui_scale() {
            ctx.set_zoom_factor(self.app.ui_scale());
        }

        let title = self.app.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        self.app.persist();
        // The workers ask for a frame themselves when they have produced
        // something worth showing, so the window only schedules the frames
        // nothing else would.
        if let Some(after) = self.app.repaint_after() {
            ctx.request_repaint_after(after);
        }
        if self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
