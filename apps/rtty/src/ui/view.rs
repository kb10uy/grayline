//! The whole window: the printed text, the settings beside it, and a status
//! line under both.
//!
//! The panels are claimed in the order egui makes load-bearing: the status bar
//! first so it runs the full width, then the settings panel so it runs the
//! full height above it, then the transmit area along the bottom of what is
//! left, and the received text last, taking the rest.

use egui::{Align, Color32, ComboBox, Id, Layout, Panel, RichText, TextStyle, Ui};

use grayline_rtty::code::Case;

use crate::{
    app::App,
    ui::{
        menu::{self, Action, Menu},
        scrollback,
    },
    worker::receive::{ColumnSnapshot, DecodePath},
};

mod panels;
mod status_bar;
mod transmit;

use panels::side_panel;
use status_bar::status_bar;
use transmit::transmit_panel;

/// Fixed and exact, for the reason recorded beside the SSTV application's own
/// side panel: everything in it is laid out from the width it is given, and a
/// panel squeezed by a narrow window otherwise keeps the squeezed width.
const SIDE_PANEL_WIDTH: f32 = 232.0;
const FIELD_LABEL_WIDTH: f32 = 60.0;
/// Width of the signal meter, in points.
const METER_WIDTH: f32 = 64.0;

const SMALL: f32 = 12.0;
const LABEL: f32 = 11.0;

const ERROR_COLOR: Color32 = Color32::from_rgb(0xE0, 0xA0, 0x30);

/// Draws the window and returns whatever the menu activated.
pub fn view(ui: &mut Ui, app: &mut App, model: &[Menu], in_window_menu: bool) -> Option<Action> {
    // Labels are inert except in the text pane, which turns selection back on
    // for itself: a received callsign is there to be copied, while a heading
    // that took the text cursor would only swallow the press.
    ui.style_mut().interaction.selectable_labels = false;

    let mut activated = None;
    if in_window_menu {
        Panel::top(Id::new("menu-bar")).show(ui, |ui| {
            activated = menu::bar(ui, model);
        });
    }
    Panel::bottom(Id::new("status-bar")).show(ui, |ui| status_bar(ui, app));
    Panel::right(Id::new("side-panel"))
        .resizable(false)
        .exact_size(SIDE_PANEL_WIDTH)
        .show(ui, |ui| side_panel(ui, app));
    // Claimed after the side panel so the settings run the full height beside
    // it, and before the central panel so the text takes what is left.
    Panel::bottom(Id::new("transmit-panel"))
        .resizable(true)
        .default_size(transmit::DEFAULT_HEIGHT)
        .min_size(transmit::MINIMUM_HEIGHT)
        .show(ui, |ui| transmit_panel(ui, app));
    egui::CentralPanel::default().show(ui, |ui| columns(ui, app));
    activated
}

/// The decode paths, side by side, each with its own header over its text.
///
/// One path for now, and the layout is the mechanism for the rest: every
/// column is fed the same audio, so what they print can be read against each
/// other the moment there is a second demodulator to run.
fn columns(ui: &mut Ui, app: &App) {
    let hint = app.i18n.text("hint-listening");
    let count = app.columns.len();
    let gaps = ui.spacing().item_spacing.x * count.saturating_sub(1) as f32;
    let width = ((ui.available_width() - gaps) / count.max(1) as f32).max(0.0);
    let height = ui.available_height();

    ui.horizontal_top(|ui| {
        for (index, scrollback) in app.columns.iter().enumerate() {
            ui.allocate_ui(egui::vec2(width, height), |ui| {
                ui.vertical(|ui| {
                    column_header(ui, app, index);
                    ui.separator();
                    scrollback::pane(ui, scrollback, &hint);
                });
            });
        }
    });
}

/// What one decode path is hearing, over the text it printed from it.
fn column_header(ui: &mut Ui, app: &App, index: usize) {
    let snapshot = app.column(index);
    let path = snapshot
        .map(|column| column.path)
        .unwrap_or(DecodePath::ALL[index.min(DecodePath::ALL.len() - 1)]);
    let name = app.i18n.text(path.label_key());

    ui.horizontal(|ui| {
        ui.label(RichText::new(name).size(LABEL).weak());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let strength = snapshot.map_or(0.0, |column| column.signal_strength);
            ui.add(egui::ProgressBar::new(strength.clamp(0.0, 1.0)).desired_width(METER_WIDTH));
            ui.label(RichText::new(app.i18n.text("label-signal")).size(LABEL).weak());
            if let Some(column) = snapshot {
                ui.label(case_text(app, column));
                ui.label(RichText::new(tones_reading(column)).size(LABEL).weak());
            }
        });
    });
}

/// The case the decoder is reading in.
///
/// Shown because it is the one piece of decoder state an operator can act on:
/// a line printing digits as letters has missed a shift, and the way out is to
/// resynchronize.
fn case_text(app: &App, column: &ColumnSnapshot) -> RichText {
    let key = match column.case {
        Case::Letters => "case-letters",
        Case::Figures => "case-figures",
    };
    RichText::new(app.i18n.text(key)).size(LABEL).weak()
}

/// The pair actually being detected, which automatic frequency control moves
/// away from the pair that was asked for.
///
/// Drawn in the same family as the labels beside it rather than in the
/// monospaced one. The two families are laid out from their own metrics, so a
/// monospaced reading sits a pixel off the baseline of everything around it,
/// and the proportional family's figures are tabular anyway: the reading keeps
/// its width as the frequency control moves it.
fn tones_reading(column: &ColumnSnapshot) -> String {
    format!("M:{:.0} / S:{:.0}", column.tones.mark_hz, column.tones.space_hz)
}

fn heading(ui: &mut Ui, label: &str) {
    ui.label(RichText::new(label).size(LABEL).weak());
}

/// A field label, aligned with the middle of the field beside it.
fn field_label(ui: &mut Ui, label: &str) {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(FIELD_LABEL_WIDTH, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(FIELD_LABEL_WIDTH, height));
            ui.label(RichText::new(label).size(SMALL));
        },
    );
}

#[cfg(test)]
mod tests;
