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

/// Shows and corrects what the directory has filed under the contact.
///
/// Opened from the panel rather than from the Settings menu, unlike the window
/// above: this is about the station on the air right now, which is what that
/// panel is for, while Settings holds what is set once and then left alone.
pub(super) fn contact_dialog(ui: &mut Ui, app: &mut App) {
    if !app.contact_dialog_open {
        return;
    }

    let title = app.i18n.text("contact-title");
    let note = app.i18n.text("contact-note-keys");
    let invalid = app.i18n.text("custom-invalid");
    let other = app.i18n.text("contact-other");
    let add = app.i18n.text("contact-add");
    let refresh = app.i18n.text("contact-refresh");
    let close = app.i18n.text("station-close");
    let callsign = app.contact_snapshot.callsign.clone();
    let state = contact_state(app);
    // Resolved before the rows are borrowed for editing, because a label comes
    // from the catalogue and the catalogue lives on the same interface. A key
    // the settings named that this build has no label for stands under its own
    // name: the field list is the operator's to write, and a key they invented
    // is one no catalogue was ever going to know.
    let labels: Vec<String> = app
        .contact_draft
        .iter()
        .map(|row| {
            app.i18n
                .message(&contact_label_key(&row.key))
                .unwrap_or_else(|| row.key.clone())
        })
        .collect();
    let refreshable = app.can_refresh_contact();

    let mut changed = false;
    let mut removed = None;
    let mut adding = false;
    let mut refreshing = false;
    let mut done = false;
    let response = egui::Modal::new(Id::new("contact")).show(ui.ctx(), |ui| {
        ui.set_max_width(420.0);
        // The callsign is what a record is filed under rather than something
        // filed in it, so it is shown rather than offered for editing; the
        // panel behind this window is where it is typed.
        ui.heading(format!("{title} — {callsign}"));
        ui.add_space(4.0);
        if let Some(state) = &state {
            ui.label(RichText::new(state).size(LABEL).weak());
        }
        ui.add_space(8.0);

        let remove_width = ui.spacing().interact_size.y;
        let gaps = ui.spacing().item_spacing.x * 2.0;
        let labelled_width = ui.available_width() - FIELD_LABEL_WIDTH - ui.spacing().item_spacing.x;
        let name_width = (ui.available_width() - remove_width - gaps) * 0.4;
        let value_width = ui.available_width() - remove_width - gaps - name_width;

        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            let mut headed = false;
            for (index, row) in app.contact_draft.iter_mut().enumerate() {
                // No keyboard filter on any of these, unlike every other field
                // whose contents can reach the air. The store is the family's
                // rather than this application's: a name in kanji is a good
                // entry that the SSTV application prints as an image, and
                // refusing to type one here would file a worse record for the
                // sake of a macro that reads the Latin spelling instead.
                if row.offered {
                    let label = labels.get(index).map_or(row.key.as_str(), String::as_str);
                    ui.horizontal(|ui| {
                        field_label(ui, label);
                        changed |= ui
                            .add(egui::TextEdit::singleline(&mut row.value).desired_width(labelled_width))
                            .changed();
                    });
                    continue;
                }
                // What the settings asked for leads, in the order it was
                // written. Anything else the directory happens to hold follows
                // with its name laid open, because nothing here chose it.
                if !headed {
                    headed = true;
                    ui.add_space(8.0);
                    ui.label(RichText::new(other.clone()).size(LABEL).weak());
                }
                let (key, value) = (&mut row.key, &mut row.value);
                ui.horizontal(|ui| {
                    let usable = grayline_qso::valid_key(key);
                    let field = egui::TextEdit::singleline(key)
                        .desired_width(name_width)
                        .text_color_opt((!usable).then_some(ERROR_COLOR));
                    let mut response = ui.add(field);
                    if !usable {
                        response = response.on_hover_text(invalid.clone());
                    }
                    // A name is taken up once the field is left rather than on
                    // every keystroke: half a name is a different key.
                    changed |= response.lost_focus();
                    changed |= ui
                        .add(egui::TextEdit::singleline(value).desired_width(value_width))
                        .changed();
                    if ui.button("✖").clicked() {
                        removed = Some(index);
                    }
                });
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            adding = ui.button(add).clicked();
            // Absent rather than disabled when there is nobody to ask: a
            // station with no logger configured has no "again" to press.
            if refreshable {
                refreshing = ui.button(refresh).clicked();
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new(note).size(LABEL).weak());
        ui.add_space(16.0);
        done = ui.button(close).clicked();
    });

    if adding {
        app.add_contact_field();
    }
    if let Some(index) = removed {
        app.contact_draft.remove(index);
        changed = true;
    }
    let closing = done || response.should_close();
    if changed || closing {
        app.commit_contact();
    }
    // Looking the station up again answers with what the logger says, so the
    // rows being edited here stop being what is filed; the window closes rather
    // than showing a draft the answer has moved past.
    if refreshing {
        app.refresh_contact();
    }
    if closing || refreshing {
        app.contact_dialog_open = false;
    }
}

/// What the directory is doing, in words, when it is worth saying.
fn contact_state(app: &App) -> Option<String> {
    if let Some(error) = &app.contact_snapshot.error {
        return Some(app.i18n.text_with(
            "contact-state-failed",
            &[("detail", grayline_shell::i18n::owned(error.to_string()))],
        ));
    }
    match app.contact_snapshot.state {
        ContactState::Looking => Some(app.i18n.text("contact-state-looking")),
        ContactState::Unknown => Some(app.i18n.text_with(
            "contact-state-unknown",
            &[(
                "callsign",
                grayline_shell::i18n::owned(app.contact_snapshot.callsign.clone()),
            )],
        )),
        ContactState::Idle | ContactState::Known | ContactState::Failed => None,
    }
}
