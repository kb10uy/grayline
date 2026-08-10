//! The whole window: a control bar along the bottom, and the chart above it.
//!
//! WEFAX gives an operator very little to do — the transmission is somebody
//! else's — so everything worked during a reception fits on one row, and what
//! is left of the window belongs to the picture.

use egui::{Align, Color32, ComboBox, Id, Layout, Panel, RichText, Ui, Vec2};
use grayline_shell::i18n::{I18n, number, owned};
use grayline_wefax::{Ioc, LinesPerMinute};

use crate::{
    app::{App, PHASE_COARSE_PIXELS, PHASE_STEP_PIXELS, SLANT_COARSE_PPM, SLANT_STEP_PPM},
    ui::menu::{self, Action, Menu},
    worker::receive::RxProgress,
};

/// Width of the signal meter, in points.
const METER_WIDTH: f32 = 72.0;
/// Width the line-rate reading is given, so the buttons either side of it do
/// not move as the figure changes.
const PPM_READING_WIDTH: f32 = 68.0;

/// Draws the window and returns whatever the menu activated.
pub fn view(ui: &mut Ui, app: &mut App, model: &[Menu], in_window_menu: bool) -> Option<Action> {
    let mut activated = None;
    if in_window_menu {
        Panel::top(Id::new("menu-bar")).show(ui, |ui| {
            activated = menu::bar(ui, model);
        });
    }
    Panel::bottom(Id::new("control-bar")).show(ui, |ui| {
        ui.add_space(2.0);
        controls(ui, app);
        ui.add_space(2.0);
    });
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
        let hint = app.i18n.text("hint-drop-wav");
        app.strip.paint(ui, &hint);
    });
    activated
}

/// The controls on top, and a status line under them.
fn controls(ui: &mut Ui, app: &mut App) {
    settings_row(ui, app);
    ui.add_space(2.0);
    status_row(ui, app);
}

/// Everything a reception is worked with, in groups that stay together.
fn settings_row(ui: &mut Ui, app: &mut App) {
    ui.horizontal_wrapped(|ui| {
        block(ui, "geometry", |ui| geometry(ui, app));
        block(ui, "flags", |ui| flags(ui, app));
        block(ui, "actions", |ui| actions(ui, app));
        block(ui, "phase", |ui| phase(ui, app));
        block(ui, "slant", |ui| slant(ui, app));
        block(ui, "save", |ui| save(ui, app));
    });
}

/// Draws one group of controls as a single item of the wrapping row.
///
/// A narrow window has to put some of the bar on a second line, and a group
/// that broke in the middle of itself — the two halves of the phase control on
/// different rows — reads as two controls rather than one. A wrapping layout
/// breaks between the things it is given, so each group is given to it whole,
/// as one allocation the width of what that group needed last time.
///
/// A group whose width has changed since — a different language, a longer
/// reading — draws at the old width for one frame and is measured again. The
/// frame that follows is asked for straight away, so the wrong one is never on
/// screen long enough to be seen.
fn block<R>(ui: &mut Ui, salt: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let id = Id::new("wefax-control-block").with(salt);
    let remembered = ui
        .ctx()
        .memory(|memory| memory.data.get_temp::<f32>(id))
        .unwrap_or_default();
    let desired = Vec2::new(remembered, ui.spacing().interact_size.y);
    let inner = ui.allocate_ui_with_layout(desired, Layout::left_to_right(Align::Center), add);

    let measured = inner.response.rect.width();
    if measured != remembered {
        ui.ctx().memory_mut(|memory| memory.data.insert_temp(id, measured));
        ui.ctx().request_repaint();
    }
    inner.inner
}

/// What a reception is told about itself before it starts.
fn flags(ui: &mut Ui, app: &mut App) {
    let mut changed = false;
    changed |= ui
        .checkbox(&mut app.auto_start, app.i18n.text("action-auto-start"))
        .changed();
    changed |= ui
        .checkbox(&mut app.auto_stop, app.i18n.text("action-auto-stop"))
        .changed();
    changed |= ui
        .checkbox(&mut app.slant_tracking, app.i18n.text("action-slant"))
        .changed();
    changed |= ui.checkbox(&mut app.inverted, app.i18n.text("action-invert")).changed();
    if changed {
        app.push_settings();
    }
    ui.separator();
}

/// Starting, ending, and throwing away a reception.
fn actions(ui: &mut Ui, app: &mut App) {
    if ui.button(app.i18n.text("action-start")).clicked() {
        app.start();
    }
    let receiving = app.progress().is_active();
    if ui
        .add_enabled(receiving, egui::Button::new(app.i18n.text("action-stop")))
        .clicked()
    {
        app.stop();
    }
    if ui.button(app.i18n.text("action-clear")).clicked() {
        app.clear();
    }
    ui.separator();
}

fn save(ui: &mut Ui, app: &mut App) {
    if ui
        .add_enabled(!app.strip.is_empty(), egui::Button::new(app.i18n.text("action-save")))
        .clicked()
    {
        app.save_chart();
    }
}

/// Whatever the application last had to say, and what the receiver is doing.
///
/// The two ends of one line: a message is read from the left, and the state is
/// looked up rather than read, so it sits where the eye returns to.
fn status_row(ui: &mut Ui, app: &App) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(state_text(app));
            ui.separator();
            meter(ui, app);
            ui.separator();

            // Left to right again for the message, so a long one is truncated
            // at its end rather than at its beginning.
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let notice = app.notice.clone().unwrap_or_default();
                ui.label(RichText::new(notice).color(Color32::from_rgb(0xE0, 0xA0, 0x30)).small());
            });
        });
    });
}

/// The index of cooperation and the line rate, which nothing on the air fully
/// announces.
fn geometry(ui: &mut Ui, app: &mut App) {
    let mut changed = false;
    ComboBox::from_id_salt("ioc")
        .selected_text(format!("IOC {}", app.ioc.index()))
        .width(84.0)
        .show_ui(ui, |ui| {
            for ioc in Ioc::ALL {
                changed |= ui
                    .selectable_value(&mut app.ioc, ioc, format!("IOC {}", ioc.index()))
                    .changed();
            }
        });
    ComboBox::from_id_salt("lpm")
        .selected_text(format!("{} LPM", app.lines_per_minute.as_lpm()))
        .width(92.0)
        .show_ui(ui, |ui| {
            for rate in LinesPerMinute::ALL {
                changed |= ui
                    .selectable_value(&mut app.lines_per_minute, rate, format!("{} LPM", rate.as_lpm()))
                    .changed();
            }
        });
    if changed {
        app.push_settings();
    }
    ui.separator();
}

/// Nudges the picture sideways, for a chart the phasing signal placed wrongly.
fn phase(ui: &mut Ui, app: &App) {
    ui.label(app.i18n.text("label-phase"));
    for (label, pixels) in [
        ("\u{25c0}\u{25c0}", -PHASE_COARSE_PIXELS),
        ("\u{25c0}", -PHASE_STEP_PIXELS),
        ("\u{25b6}", PHASE_STEP_PIXELS),
        ("\u{25b6}\u{25b6}", PHASE_COARSE_PIXELS),
    ] {
        if ui.small_button(label).clicked() {
            app.shift_phase(pixels);
        }
    }
    // Each group carries the rule that ends it, so a row the bar wrapped onto
    // never begins with one hanging in front of nothing.
    ui.separator();
}

/// Corrects the line rate by hand, with the fitted one between the controls.
///
/// The reading is what the clock is actually running at rather than what has
/// been asked for: a correction the tracker then refits away should be seen to
/// have been refitted away.
fn slant(ui: &mut Ui, app: &App) {
    ui.label(app.i18n.text("label-slant"));
    for (label, ppm) in [("\u{25c0}\u{25c0}", -SLANT_COARSE_PPM), ("\u{25c0}", -SLANT_STEP_PPM)] {
        if ui.small_button(label).clicked() {
            app.adjust_slant_ppm(ppm);
        }
    }
    let reading = match app.line_rate_error_ppm() {
        Some(ppm) => format!("{ppm:+.0} ppm"),
        None => "— ppm".to_owned(),
    };
    ui.add_sized(
        [PPM_READING_WIDTH, ui.spacing().interact_size.y],
        egui::Label::new(RichText::new(reading).monospace()),
    );
    for (label, ppm) in [("\u{25b6}", SLANT_STEP_PPM), ("\u{25b6}\u{25b6}", SLANT_COARSE_PPM)] {
        if ui.small_button(label).clicked() {
            app.adjust_slant_ppm(ppm);
        }
    }
    ui.separator();
}

/// How much signal is arriving, and how much of it looks like a framing tone.
fn meter(ui: &mut Ui, app: &App) {
    let snapshot = app.audio.snapshot();
    let apt = snapshot.apt.iter().fold(0.0_f32, |high, value| high.max(*value));
    ui.add(egui::ProgressBar::new(apt.clamp(0.0, 1.0)).desired_width(METER_WIDTH));
    ui.label(app.i18n.text("label-apt"));
    ui.add(
        egui::ProgressBar::new(snapshot.signal_level.clamp(0.0, 1.0))
            .desired_width(METER_WIDTH)
            .show_percentage(),
    );
    ui.label(app.i18n.text("label-signal"));
}

/// What the receiver is doing, and how far it has got.
fn state_text(app: &App) -> RichText {
    let progress = app.progress();
    let text = describe(&app.i18n, progress, app);
    RichText::new(text).color(state_color(progress)).strong()
}

fn describe(i18n: &I18n, progress: RxProgress, app: &App) -> String {
    let state = i18n.text(progress.label_key());
    let Some(format) = app.format else {
        return state;
    };
    let geometry = format!("IOC {} / {} LPM", format.ioc.index(), format.lines_per_minute.as_lpm());
    match progress.lines() {
        0 => i18n.text_with(
            "state-with-format",
            &[("state", owned(state)), ("format", owned(geometry))],
        ),
        lines => i18n.text_with(
            "state-with-lines",
            &[
                ("state", owned(state)),
                ("format", owned(geometry)),
                ("lines", number(lines as f64)),
            ],
        ),
    }
}

const fn state_color(progress: RxProgress) -> Color32 {
    match progress {
        RxProgress::Idle => Color32::GRAY,
        RxProgress::Starting | RxProgress::Phasing => Color32::from_rgb(0xE0, 0xC0, 0x40),
        RxProgress::Imaging { .. } => Color32::from_rgb(0x60, 0xD0, 0x70),
        RxProgress::Complete { .. } => Color32::from_rgb(0x70, 0xB0, 0xF0),
        RxProgress::Stopped { .. } => Color32::from_rgb(0xE0, 0x80, 0x70),
    }
}

#[cfg(test)]
mod tests;
