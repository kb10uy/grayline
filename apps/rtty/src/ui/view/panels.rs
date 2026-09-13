//! The sections of the side panel, and the controls each one holds.

use super::*;

use grayline_shell::i18n::owned;

use crate::{
    app::MARK_STEP_HZ,
    storage::config::{
        BAUD_RATES, MAXIMUM_MARK_HZ, MAXIMUM_SQUELCH, MAXIMUM_TX_LEVEL, MINIMUM_MARK_HZ, MINIMUM_SQUELCH,
        MINIMUM_TX_LEVEL, SHIFTS_HZ,
    },
};

/// What a slider's own reading is written in, and the least a slider is drawn
/// at when the panel is squeezed, in points.
const SLIDER_VALUE_WIDTH: f32 = 52.0;
const MINIMUM_SLIDER_WIDTH: f32 = 32.0;

pub(super) fn side_panel(ui: &mut Ui, app: &mut App) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        let tuning_title = app.i18n.text("section-tuning");
        section(ui, &tuning_title, |ui| tuning_panel(ui, app));
        ui.add_space(12.0);
        let transmit_title = app.i18n.text("section-transmit");
        section(ui, &transmit_title, |ui| transmit_controls(ui, app));
        ui.add_space(12.0);
        let contact_title = app.i18n.text("section-contact");
        section(ui, &contact_title, |ui| contact_panel(ui, app));
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

/// The pair, the speed, what the receiver is allowed to do about them, and
/// the threshold under which it prints nothing.
///
/// The mark tone is a figure rather than a list: a station is met wherever the
/// operator's receiver puts it, and the shift beside it is what is standard.
///
/// The squelch is here rather than in a section of its own because it is read
/// against the same signal: the meter under it is what the threshold is set
/// by, and the frequency control above it is what put the receiver on the
/// signal being measured.
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

    ui.add_space(4.0);
    squelch_row(ui, app, &mut changed);

    if changed {
        app.push_settings();
    }
    if pressed {
        app.adopt_detected_tones();
    }
}

/// The threshold, and what the signal it is being set against reads.
///
/// The meter is under the slider rather than in the header alone because this
/// is where the threshold is chosen: a setting made against a number that is
/// somewhere else is made blind.
///
/// There is no switch beside it. Zero is the whole of one — nothing is quieter
/// than a signal that reads nothing — and a switch that only stood for the
/// left end of the slider would be a second place to look for the same state.
fn squelch_row(ui: &mut Ui, app: &mut App, changed: &mut bool) {
    let label = app.i18n.text("action-squelch");
    let hint = app.i18n.text("hint-squelch");
    ui.horizontal(|ui| {
        field_label(ui, &label);
        ui.spacing_mut().slider_width = (ui.available_width() - SLIDER_VALUE_WIDTH).max(MINIMUM_SLIDER_WIDTH);
        let slider = egui::Slider::new(&mut app.squelch_threshold, MINIMUM_SQUELCH..=MAXIMUM_SQUELCH)
            .fixed_decimals(2)
            .show_value(true);
        *changed |= ui.add(slider).on_hover_text(hint).changed();
    });

    let strength = app.column(0).map_or(0.0, |column| column.signal_strength);
    ui.add(egui::ProgressBar::new(strength.clamp(0.0, 1.0)).desired_width(ui.available_width()));
}

/// Rounded to the hundredth, because 45.45 baud is named by those digits.
fn baud_text(baud: f64) -> String {
    let text = format!("{baud:.2}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed} baud")
}

/// Putting the message on the air, stopping it, and how hard it is driven.
///
/// Beside the message rather than under it, where SSTV keeps the same two
/// controls: the button that puts the station on the air is one press away
/// from the macros if it stands among them, and the level belongs with the
/// other settings that decide what leaves the sound card.
fn transmit_controls(ui: &mut Ui, app: &mut App) {
    let send_label = app.i18n.text("action-send");
    let stop_label = app.i18n.text("action-stop");
    let can_send = app.can_send();
    let refused = app.unsendable_character();
    let busy = app.transmit.is_busy();
    let gap = ui.spacing().item_spacing.x;
    let height = ui.spacing().interact_size.y;
    let width = (ui.available_width() - gap) / 2.0;

    let mut sending = false;
    let mut stopping = false;
    ui.horizontal(|ui| {
        let button = egui::Button::new(RichText::new(send_label).size(SMALL));
        let response = ui
            .add_enabled_ui(can_send, |ui| ui.add_sized([width, height], button))
            .inner;
        sending = response.clicked();
        if let Some(character) = refused {
            response.on_hover_text(
                app.i18n
                    .text_with("hint-unsendable", &[("character", owned(character.to_string()))]),
            );
        } else if app.audio.output_device.is_none() {
            response.on_hover_text(app.i18n.text("error-no-output"));
        }

        let stop = egui::Button::new(RichText::new(stop_label).size(SMALL));
        stopping = ui
            .add_enabled_ui(busy, |ui| ui.add_sized([width, height], stop))
            .inner
            .on_hover_text(app.i18n.text("hint-stop"))
            .clicked();
    });

    ui.add_space(4.0);
    let level_label = app.i18n.text("label-level");
    ui.horizontal(|ui| {
        field_label(ui, &level_label);
        ui.spacing_mut().slider_width = ui.available_width();
        ui.add(
            egui::Slider::new(&mut app.tx_level, MINIMUM_TX_LEVEL..=MAXIMUM_TX_LEVEL)
                .show_value(false)
                .max_decimals(2),
        );
    });

    if sending {
        app.send_draft();
    }
    if stopping {
        app.abort_transmission();
    }
}

/// Who is being worked, which is what the macros are written from.
///
/// Beside the text rather than under it, because these are filled in from what
/// the other station just sent: the callsign is read off the line above and
/// the report is judged from the same signal the meter is reading.
fn contact_panel(ui: &mut Ui, app: &mut App) {
    let gap = ui.spacing().item_spacing.x;
    let fields = ui.available_width() - FIELD_LABEL_WIDTH - gap;
    let details_hint = app.i18n.text("contact-open");
    let labels = [
        ("label-his-call", "his-call"),
        ("label-his-name", "his-name"),
        ("label-his-qth", "his-qth"),
        ("label-rst-sent", "rst-sent"),
        ("label-rst-received", "rst-received"),
    ];
    let mut finished = false;
    let mut opening = false;
    for (index, (key, salt)) in labels.into_iter().enumerate() {
        let label = app.i18n.text(key);
        ui.horizontal(|ui| {
            field_label(ui, &label);
            let id = Id::new(salt);
            crate::ui::input::sanitize(ui.ctx(), id);
            let height = ui.spacing().interact_size.y;
            let width = if index == 0 { fields - height - gap } else { fields };
            let target = match index {
                0 => &mut app.contact.callsign,
                1 => &mut app.contact.name,
                2 => &mut app.contact.qth,
                3 => &mut app.contact.rst_sent,
                _ => &mut app.contact.rst_received,
            };
            let response = ui.add_sized([width, height], egui::TextEdit::singleline(target).id(id));
            finished |= response.lost_focus();
            if index == 0 {
                let known = grayline_qso::normalize_callsign(&app.contact.callsign).is_some();
                opening = ui
                    .add_enabled(known, egui::Button::new("\u{2026}"))
                    .on_hover_text(&details_hint)
                    .clicked();
            }
        });
    }

    ui.add_space(4.0);
    let clear = app.i18n.text("action-clear-contact");
    let hint = app.i18n.text("hint-clear-contact");
    let width = ui.available_width();
    let height = ui.spacing().interact_size.y;
    let pressed = ui
        .add_enabled_ui(!app.contact.is_empty(), |ui| {
            ui.add_sized([width, height], egui::Button::new(RichText::new(clear).size(SMALL)))
        })
        .inner
        .on_hover_text(hint)
        .clicked();
    if pressed {
        app.clear_contact();
    }
    if finished {
        app.finish_contact_edit();
    }
    if opening {
        app.open_contact();
    }
}

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
