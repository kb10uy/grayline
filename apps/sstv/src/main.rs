#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{error::Error, time::Duration};

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
use grayline_shell::{
    log, platform,
    window::{self, Application},
    worker::Waker,
};
use storage::paths;
use ui::{menu, view};

/// The window size the interface is laid out for, in points.
const DEFAULT_WINDOW_SIZE: [f32; 2] = [1024.0, 768.0];

fn main() -> Result<(), Box<dyn Error>> {
    platform::prepare_process(&identity::IDENTITY);

    // A second copy would fail to open the audio devices the first one holds,
    // which is harder to understand than not opening at all. The running copy
    // has already been asked to come forward by the time this returns.
    let Some(instance) = platform::claim_single_instance(&identity::IDENTITY) else {
        return Ok(());
    };

    let paths = paths::AppPaths::discover()?;
    paths.initialize()?;
    if let Err(error) = log::open(paths.log_file(), &identity::IDENTITY) {
        eprintln!("could not open the log file: {error}");
    }

    window::run(&identity::IDENTITY, DEFAULT_WINDOW_SIZE, instance, |cc| {
        App::new(paths, Waker::new(cc.egui_ctx.clone()))
    })?;
    Ok(())
}

impl Application for App {
    type Action = menu::Action;

    fn poll(&mut self, _ctx: &egui::Context) {
        self.poll_workers();
    }

    fn menus(&self) -> Vec<menu::Menu> {
        menu::model(self)
    }

    fn apply(&mut self, action: menu::Action) -> bool {
        menu::apply(self, action)
    }

    fn view(&mut self, ui: &mut egui::Ui, menus: &[menu::Menu], in_window_menu: bool) -> Option<menu::Action> {
        view::view(ui, self, menus, in_window_menu)
    }

    fn title(&self) -> String {
        App::title(self)
    }

    fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    fn set_ui_scale(&mut self, scale: f32) {
        App::set_ui_scale(self, scale);
    }

    fn persist(&mut self) {
        App::persist(self);
    }

    fn repaint_after(&self) -> Option<Duration> {
        App::repaint_after(self)
    }
}
