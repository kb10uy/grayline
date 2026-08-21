//! The sections of the side panel, and the controls each one holds.

use super::*;

use crate::{
    app::MARK_STEP_HZ,
    storage::config::{BAUD_RATES, MAXIMUM_MARK_HZ, MAXIMUM_SQUELCH, MINIMUM_MARK_HZ, MINIMUM_SQUELCH, SHIFTS_HZ},
};

pub(super) fn side_panel(ui: &mut Ui, app: &mut App) {
    // The sections below can outgrow the panel's height on a small window or
    // a large font scale; scroll rather than silently clipping the bottom.
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        let tuning_title = app.i18n.text("section-tuning");
        section(ui, &tuning_title, |ui| tuning_panel(ui, app));
        ui.add_space(12.0);
        let squelch_title = app.i18n.text("section-squelch");
        section(ui, &squelch_title, |ui| squelch_panel(ui, app));
        ui.add_space(12.0);
        actions(ui, app);
    });
}

fn section(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui)) {
    heading(ui, title);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        contents(ui);
    });
}

/// The pair, the speed, and what the receiver is allowed to do about them.
///
/// The mark tone is a figure rather than a list: a station is met wherever the
/// operator's receiver puts it, and the shift beside it is what is standard.
fn tuning_panel(ui: &mut Ui, app: &mut App) {
    let gap = ui.spacing().item_spacing.x;
    let fields = ui.available_width() - FIELD_LABEL_WIDTH - gap;
    let mut changed = false;

    let mark_label = app.i18n.text("label-mark");
    ui.horizontal(|ui| {
        field_label(ui, &mark_label);
        let mark = egui::DragValue::new(&mut app.mark_hz)
            .speed(MARK_STEP_HZ)
            .range(MINIMUM_MARK_HZ..=MAXIMUM_MARK_HZ)
            .fixed_decimals(0)
            .suffix(" Hz");
        changed |= ui.add_sized([fields, ui.spacing().interact_size.y], mark).changed();
    });

    let shift_label = app.i18n.text("label-shift");
    ui.horizontal(|ui| {
        field_label(ui, &shift_label);
        ComboBox::from_id_salt("shift")
            .selected_text(format!("{:.0} Hz", app.shift_hz))
            .width(fields)
            .show_ui(ui, |ui| {
                for shift in SHIFTS_HZ {
                    changed |= ui
                        .selectable_value(&mut app.shift_hz, shift, format!("{shift:.0} Hz"))
                        .changed();
                }
            });
    });

    let speed_label = app.i18n.text("label-speed");
    ui.horizontal(|ui| {
        field_label(ui, &speed_label);
        ComboBox::from_id_salt("baud")
            .selected_text(baud_text(app.baud))
            .width(fields)
            .show_ui(ui, |ui| {
                for baud in BAUD_RATES {
                    changed |= ui.selectable_value(&mut app.baud, baud, baud_text(baud)).changed();
                }
            });
    });

    ui.add_space(4.0);
    let reverse = app.i18n.text("action-reverse");
    let afc = app.i18n.text("action-afc");
    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut app.reverse, RichText::new(reverse).size(SMALL))
            .changed();
        changed |= ui.checkbox(&mut app.afc, RichText::new(afc).size(SMALL)).changed();
    });

    // What the frequency control found is lost the next time the receiver is
    // built, so the operator is given a way to keep it.
    let adopt = app.i18n.text("action-adopt-tones");
    let detected = app.column(0).map(|column| column.tones);
    let moved = detected.is_some_and(|tones| {
        let wanted = app.tones();
        (tones.mark_hz - wanted.mark_hz).abs() >= 1.0 || (tones.space_hz - wanted.space_hz).abs() >= 1.0
    });
    let width = ui.available_width();
    let height = ui.spacing().interact_size.y;
    let pressed = ui
        .add_enabled_ui(moved, |ui| {
            ui.add_sized([width, height], egui::Button::new(RichText::new(adopt).size(SMALL)))
        })
        .inner
        .clicked();

    if changed {
        app.push_settings();
    }
    if pressed {
        app.adopt_detected_tones();
    }
}

/// Rounded to the hundredth, because 45.45 baud is named by those digits.
fn baud_text(baud: f64) -> String {
    let text = format!("{baud:.2}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed} baud")
}

/// The threshold, and what the signal it is being set against reads.
///
/// The meter is under the slider rather than in the header alone because this
/// is where the threshold is chosen: a setting made against a number that is
/// somewhere else is made blind.
fn squelch_panel(ui: &mut Ui, app: &mut App) {
    let label = app.i18n.text("action-squelch");
    let mut changed = ui
        .checkbox(&mut app.squelch, RichText::new(label).size(SMALL))
        .changed();
    let slider = egui::Slider::new(&mut app.squelch_threshold, MINIMUM_SQUELCH..=MAXIMUM_SQUELCH)
        .fixed_decimals(2)
        .show_value(true);
    changed |= ui.add_enabled_ui(app.squelch, |ui| ui.add(slider)).inner.changed();

    let strength = app.column(0).map_or(0.0, |column| column.signal_strength);
    ui.add(egui::ProgressBar::new(strength.clamp(0.0, 1.0)).desired_width(ui.available_width()));

    if changed {
        app.push_settings();
    }
}

/// Opening the display, throwing away what was printed, and telling the
/// framing to start again.
fn actions(ui: &mut Ui, app: &mut App) {
    let scope = app.i18n.text("action-scope");
    let open = app.scope.is_open();
    let full = ui.available_width();
    let line = ui.spacing().interact_size.y;
    let toggled = ui
        .add_sized(
            [full, line],
            egui::Button::new(RichText::new(scope).size(SMALL)).selected(open),
        )
        .clicked();
    if toggled {
        app.set_scope_open(!open);
    }
    ui.add_space(4.0);

    let clear = app.i18n.text("action-clear");
    let resync = app.i18n.text("action-resync");
    let hint = app.i18n.text("hint-resync");
    let gap = ui.spacing().item_spacing.x;
    let width = (ui.available_width() - gap) / 2.0;
    let height = ui.spacing().interact_size.y;
    let mut cleared = false;
    let mut resynchronized = false;
    ui.horizontal(|ui| {
        cleared = ui
            .add_sized([width, height], egui::Button::new(RichText::new(clear).size(SMALL)))
            .clicked();
        resynchronized = ui
            .add_sized([width, height], egui::Button::new(RichText::new(resync).size(SMALL)))
            .on_hover_text(hint)
            .clicked();
    });
    if cleared {
        app.clear();
    }
    if resynchronized {
        app.resynchronize();
    }
}
