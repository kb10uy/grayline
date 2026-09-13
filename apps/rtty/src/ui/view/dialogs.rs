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
    let labels = ["label-my-call", "label-my-name", "label-my-qth", "label-my-grid"].map(|key| app.i18n.text(key));

    let mut done = false;
    let mut changed = false;
    let mut removed = None;
    let response = egui::Modal::new(Id::new("station")).show(ui.ctx(), |ui| {
        ui.set_max_width(360.0);
        ui.heading(title);
        ui.add_space(8.0);
        let width = ui.available_width() - FIELD_LABEL_WIDTH - ui.spacing().item_spacing.x;
        for (index, label) in labels.iter().enumerate() {
            let id = Id::new(["station-callsign", "station-name", "station-qth", "station-grid"][index]);
            // The same filter every field whose contents reach the air runs:
            // what is typed here is typed to be sent, through the macros that
            // read it.
            crate::ui::input::sanitize(ui.ctx(), id);
            let target = match index {
                0 => &mut app.station.callsign,
                1 => &mut app.station.name,
                2 => &mut app.station.qth,
                _ => &mut app.station.grid,
            };
            ui.horizontal(|ui| {
                field_label(ui, label);
                ui.add(egui::TextEdit::singleline(target).id(id).desired_width(width));
            });
        }
        ui.add_space(4.0);
        ui.label(RichText::new(note).size(LABEL).weak());

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(4.0);
        (changed, removed) = custom_fields(ui, app);

        ui.add_space(16.0);
        done = ui.button(close).clicked();
    });

    if let Some(index) = removed {
        app.variables_draft.remove(index);
        changed = true;
    }
    let closing = done || response.should_close();
    if changed || closing {
        app.commit_custom_variables();
    }
    if closing {
        app.station_dialog_open = false;
    }
}

/// The fields the operator invented, which only their own macros name.
///
/// Everything above this window is something the application already knows to
/// ask for; these are the ones only the operator does, which is why both the
/// name and what it stands for are typed here.
///
/// Returns whether a row changed and which row was struck out, rather than
/// acting on either: the rows are borrowed while they are being drawn.
fn custom_fields(ui: &mut Ui, app: &mut App) -> (bool, Option<usize>) {
    let heading_text = app.i18n.text("custom-title");
    let note = app.i18n.text("custom-note");
    let invalid = app.i18n.text("custom-invalid");
    let add = app.i18n.text("custom-add");
    let name_hint = app.i18n.text("custom-name");
    let value_hint = app.i18n.text("custom-value");

    let mut changed = false;
    let mut removed = None;
    heading(ui, &heading_text);
    ui.add_space(4.0);

    let remove_width = ui.spacing().interact_size.y;
    let gaps = ui.spacing().item_spacing.x * 2.0;
    let name_width = (ui.available_width() - remove_width - gaps) * 0.4;
    let value_width = ui.available_width() - remove_width - gaps - name_width;
    for (index, (name, value)) in app.variables_draft.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let usable = valid_variable_name(name);
            let field = egui::TextEdit::singleline(name)
                .id(Id::new(("custom-name", index)))
                .desired_width(name_width)
                .hint_text(&name_hint)
                .text_color_opt((!usable).then_some(ERROR_COLOR));
            let mut response = ui.add(field);
            if !usable {
                response = response.on_hover_text(&invalid);
            }
            // Taken up once the field is left rather than on every keystroke:
            // half a name is a different field, and a macro would read it.
            changed |= response.lost_focus();

            // The value goes on the air through whichever macro names it, so
            // it runs the filter every other sendable field runs; the name
            // never leaves the configuration, so it does not.
            let value_id = Id::new(("custom-value", index));
            crate::ui::input::sanitize(ui.ctx(), value_id);
            changed |= ui
                .add(
                    egui::TextEdit::singleline(value)
                        .id(value_id)
                        .desired_width(value_width)
                        .hint_text(&value_hint),
                )
                .changed();
            if ui.button("✖").clicked() {
                removed = Some(index);
            }
        });
    }

    ui.add_space(4.0);
    if ui.button(add).clicked() {
        app.add_custom_variable();
    }
    ui.add_space(4.0);
    ui.label(RichText::new(note).size(LABEL).weak());
    (changed, removed)
}
