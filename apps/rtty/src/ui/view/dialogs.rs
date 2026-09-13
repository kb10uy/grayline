//! The windows that open over the interface.
//!
//! Each is modal and each edits something set once and then left alone, which
//! is why none of them sits in a panel.

use super::*;

/// Edits what this station says about itself.
///
/// Behind the Settings menu rather than beside the text, where the SSTV
/// application keeps its own: none of it belongs to the contact being worked.
/// A callsign is entered when the application is first set up and then left
/// for years, while the panel beside the text is worked at every exchange.
pub(super) fn station_dialog(ui: &mut Ui, app: &mut App) {
    if !app.station_dialog_open {
        return;
    }

    let title = app.i18n.text("station-title");
    let note = app.i18n.text("station-callsign-required");
    let close = app.i18n.text("station-close");
    let labels = ["label-my-call", "label-my-name", "label-my-qth"].map(|key| app.i18n.text(key));

    let mut done = false;
    let response = egui::Modal::new(Id::new("station")).show(ui.ctx(), |ui| {
        ui.set_max_width(360.0);
        ui.heading(title);
        ui.add_space(8.0);
        let width = ui.available_width() - FIELD_LABEL_WIDTH - ui.spacing().item_spacing.x;
        for (index, label) in labels.iter().enumerate() {
            let id = Id::new(["station-callsign", "station-name", "station-qth"][index]);
            // The same filter every field whose contents reach the air runs:
            // what is typed here is typed to be sent, through the macros that
            // read it.
            crate::ui::input::sanitize(ui.ctx(), id);
            let target = match index {
                0 => &mut app.station.callsign,
                1 => &mut app.station.name,
                _ => &mut app.station.qth,
            };
            ui.horizontal(|ui| {
                field_label(ui, label);
                ui.add(egui::TextEdit::singleline(target).id(id).desired_width(width));
            });
        }
        ui.add_space(4.0);
        ui.label(RichText::new(note).size(LABEL).weak());
        ui.add_space(16.0);
        done = ui.button(close).clicked();
    });

    if done || response.should_close() {
        app.station_dialog_open = false;
    }
}
