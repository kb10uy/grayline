//! The panel a message is written and sent from.
//!
//! It sits under the received text rather than in a window of its own, the
//! way MMTTY puts it, because writing and reading are one activity: the reply
//! is composed against the exchange still on screen above it.

use super::*;

use grayline_shell::i18n::{number, owned};

use crate::{
    app::transmit::SentProgress,
    storage::config::{MAXIMUM_TX_LEVEL, MINIMUM_TX_LEVEL},
    ui::{input, scrollback::sent_color},
};

/// Lines of the message field, before the operator drags it.
///
/// Three, because a call is three lines: the callsign, the exchange, and the
/// invitation to reply.
const DRAFT_ROWS: usize = 3;

/// How tall the panel opens, in points.
pub(super) const DEFAULT_HEIGHT: f32 = 132.0;
pub(super) const MINIMUM_HEIGHT: f32 = 96.0;

/// The identifier of the message field, which the keyboard filter needs before
/// the field is drawn.
pub fn draft_id() -> Id {
    Id::new("transmit-draft")
}

/// The function keys the macro buttons answer to, in order.
const FUNCTION_KEYS: [egui::Key; 12] = [
    egui::Key::F1,
    egui::Key::F2,
    egui::Key::F3,
    egui::Key::F4,
    egui::Key::F5,
    egui::Key::F6,
    egui::Key::F7,
    egui::Key::F8,
    egui::Key::F9,
    egui::Key::F10,
    egui::Key::F11,
    egui::Key::F12,
];

/// Draws the whole transmit area.
pub(super) fn transmit_panel(ui: &mut Ui, app: &mut App) {
    ui.add_space(2.0);
    on_air(ui, app);
    queued(ui, app);
    draft(ui, app);
    macro_buttons(ui, app);
    ui.add_space(2.0);
    controls(ui, app);
}

/// The macro buttons, and the function keys that press them.
///
/// A press writes the message into the field at the caret rather than sending
/// it, so what is about to go out can be read and edited first; a macro marked
/// to send in the configuration is the exception, and goes out as a message of
/// its own rather than joining whatever is half written.
fn macro_buttons(ui: &mut Ui, app: &mut App) {
    if app.macros.is_empty() {
        return;
    }
    let mut pressed = None;
    for (index, key) in FUNCTION_KEYS.iter().enumerate().take(app.macros.len()) {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, *key)) {
            pressed = Some(index);
        }
    }

    ui.add_space(2.0);
    let height = ui.spacing().interact_size.y;
    let labels: Vec<(String, String)> = app
        .macros
        .iter()
        .enumerate()
        .map(|(index, template)| {
            let shortcut = FUNCTION_KEYS
                .get(index)
                .map_or_else(String::new, |key| key.name().to_owned());
            (template.label.clone(), shortcut)
        })
        .collect();
    ui.horizontal_wrapped(|ui| {
        for (index, (label, shortcut)) in labels.iter().enumerate() {
            let button = egui::Button::new(RichText::new(label).size(SMALL));
            let mut response = ui.add_sized([64.0, height], button);
            if !shortcut.is_empty() {
                response = response.on_hover_text(shortcut.trim());
            }
            if response.clicked() {
                pressed = Some(index);
            }
        }
    });

    if let Some(index) = pressed {
        press_macro(ui, app, index);
    }
}

/// Writes a macro into the field at the caret, and leaves the caret after it.
fn press_macro(ui: &Ui, app: &mut App, index: usize) {
    let id = draft_id();
    let mut state = egui::TextEdit::load_state(ui.ctx(), id);
    let caret = state.as_ref().and_then(|state| state.cursor.char_range()).map_or_else(
        || app.transmit.draft.chars().count(),
        |range| range.primary.index.max(range.secondary.index).into(),
    );
    let Some(after) = app.apply_macro(index, caret) else {
        return;
    };
    // The caret lands after what was written, so a macro pressed mid-message
    // leaves the operator where they would have typed next.
    if let Some(state) = state.as_mut() {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(after))));
        state.clone().store(ui.ctx(), id);
    }
    ui.ctx().memory_mut(|memory| memory.request_focus(id));
}

/// The message being keyed, with what has left underlined.
///
/// The underline follows the playback position rather than what the worker has
/// generated: what is in the sound card's queue has not been transmitted yet,
/// and an operator who stops sending wants the line to say what actually went
/// out.
fn on_air(ui: &mut Ui, app: &App) {
    let Some(sending) = app.transmit.sending() else {
        return;
    };
    let progress = app.sent_progress().unwrap_or_default();
    let label = app.i18n.text("label-on-air");
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(LABEL).color(sent_color(ui.visuals())));
        egui::ScrollArea::horizontal()
            .id_salt("on-air")
            .max_height(ui.spacing().interact_size.y)
            .show(ui, |ui| {
                ui.add(egui::Label::new(sent_job(ui, sending.text(), progress)).wrap());
            });
    });
}

/// The message laid out with its sent prefix underlined.
fn sent_job(ui: &Ui, text: &str, progress: SentProgress) -> egui::text::LayoutJob {
    let size = ui.text_style_height(&TextStyle::Body);
    let font = egui::FontId::monospace(size);
    let color = ui.visuals().text_color();
    let boundary = text
        .char_indices()
        .nth(progress.sent)
        .map_or(text.len(), |(index, _)| index);
    let mut job = egui::text::LayoutJob::default();
    let mut format = egui::TextFormat {
        font_id: font,
        color,
        ..egui::TextFormat::default()
    };
    format.underline = egui::Stroke::new(1.0, sent_color(ui.visuals()));
    job.append(&text[..boundary].replace('\n', "⏎"), 0.0, format.clone());
    format.underline = egui::Stroke::NONE;
    format.color = ui.visuals().weak_text_color();
    job.append(&text[boundary..].replace('\n', "⏎"), 0.0, format);
    job
}

/// What is waiting behind the message on the air.
fn queued(ui: &mut Ui, app: &mut App) {
    if app.transmit.queued().len() == 0 {
        return;
    }
    let label = app.i18n.text("label-queued");
    let drop_hint = app.i18n.text("hint-drop-queued");
    let mut dropping = None;
    let pending: Vec<String> = app.transmit.queued().map(str::to_owned).collect();
    for (index, message) in pending.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&label).size(LABEL).weak());
            if ui.small_button("✖").on_hover_text(&drop_hint).clicked() {
                dropping = Some(index);
            }
            ui.label(RichText::new(one_line(message)).size(SMALL).weak());
        });
    }
    if let Some(index) = dropping {
        app.transmit.cancel_queued(index);
    }
}

/// A message as one line, for a list that shows what is waiting rather than
/// what it says.
fn one_line(message: &str) -> String {
    let flattened = message.replace('\n', " ⏎ ");
    let mut characters = flattened.chars();
    let head: String = characters.by_ref().take(60).collect();
    if characters.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// The field the message is written in.
///
/// The keyboard filter runs before the field is added, because the field is
/// what consumes the key presses as it is drawn.
fn draft(ui: &mut Ui, app: &mut App) {
    let id = draft_id();
    input::sanitize(ui.ctx(), id);
    let size = ui.text_style_height(&TextStyle::Body);
    let mut layouter =
        |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| input::layout(ui, text.as_str(), wrap_width);
    let field = egui::TextEdit::multiline(&mut app.transmit.draft)
        .id(id)
        .font(egui::FontId::monospace(size))
        .desired_rows(DRAFT_ROWS)
        .desired_width(f32::INFINITY)
        .hint_text(app.i18n.text("hint-draft"))
        .layouter(&mut layouter);
    let response = ui.add(field);

    // Enter writes a line, as it does on a teleprinter; the modifier sends.
    // A field that sent on Enter would put half a message on the air every
    // time the operator reached for a new line.
    let send = response.has_focus()
        && ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)
                || input.consume_key(egui::Modifiers::CTRL, egui::Key::Enter)
        });
    if send {
        app.send_draft();
        // The field keeps the keyboard, because the next message is written
        // straight after the one just queued.
        ui.ctx().memory_mut(|memory| memory.request_focus(id));
    }

    // Escape stops a transmission from wherever the operator is, rather than
    // only from the button: it is the key reached for when something is going
    // out that should not be.
    if app.transmit.is_busy() && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        app.abort_transmission();
    }
}

/// Send, stop, the level, and what is left to go.
fn controls(ui: &mut Ui, app: &mut App) {
    let send_label = app.i18n.text("action-send");
    let stop_label = app.i18n.text("action-stop");
    let level_label = app.i18n.text("label-level");
    let can_send = app.can_send();
    let refused = app.unsendable_character();
    let busy = app.transmit.is_busy();
    let height = ui.spacing().interact_size.y;

    let mut sending = false;
    let mut stopping = false;
    ui.horizontal(|ui| {
        let button = egui::Button::new(RichText::new(send_label).size(SMALL));
        let response = ui
            .add_enabled_ui(can_send, |ui| ui.add_sized([72.0, height], button))
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
            .add_enabled_ui(busy, |ui| ui.add_sized([72.0, height], stop))
            .inner
            .on_hover_text(app.i18n.text("hint-stop"))
            .clicked();

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(remaining) = app.transmission_remaining() {
                let left = app
                    .i18n
                    .text_with("status-remaining", &[("seconds", number(remaining.as_secs() as u32))]);
                ui.label(RichText::new(left).size(LABEL).color(sent_color(ui.visuals())));
            }
            ui.add(
                egui::Slider::new(&mut app.tx_level, MINIMUM_TX_LEVEL..=MAXIMUM_TX_LEVEL)
                    .show_value(false)
                    .max_decimals(2),
            );
            ui.label(RichText::new(level_label).size(LABEL).weak());
        });
    });

    if sending {
        app.send_draft();
    }
    if stopping {
        app.abort_transmission();
    }
}
