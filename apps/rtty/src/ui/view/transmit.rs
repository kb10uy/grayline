//! The panel a message is written and sent from.
//!
//! It sits under the received text rather than in a window of its own, the
//! way MMTTY puts it, because writing and reading are one activity: the reply
//! is composed against the exchange still on screen above it.

use super::*;

use crate::{
    app::transmit::SentProgress,
    ui::{input, scrollback::sent_color},
};

/// Lines of the message field.
///
/// Four, and fixed: a call is three lines — the callsign, the exchange, and
/// the invitation to reply — and the fourth is the one being written past
/// them. A longer message scrolls inside the field rather than growing it,
/// because a field that grew would move the send button out from under the
/// hand that is about to press it.
const DRAFT_ROWS: usize = 4;

/// What the field keeps between its border and its text, in points.
///
/// The margin a field draws for itself, written out because the border is
/// this module's own: the scrollbar belongs inside it, and a field left to
/// draw its own border would have put the bar outside it.
const DRAFT_MARGIN: egui::Margin = egui::Margin::symmetric(4, 2);

/// The column the labels of the stack sit in, in points.
///
/// Wide enough for the longest of them and the button beside it, and the same
/// for every row: the messages start at one place, so the stack is read down
/// its left edge rather than down a ragged one.
const STACK_LABEL_WIDTH: f32 = 76.0;

/// How wide the button that drops a message is, in points.
///
/// Held to the mark it carries and little more. It is drawn on every row,
/// disabled on the one on the air, because a column of buttons that appeared
/// a row at a time would move the labels beside it.
const DROP_BUTTON_WIDTH: f32 = 15.0;

/// How tall the stack grows before it scrolls, in points.
///
/// Four rows, like the field under it. A queue longer than that is a contest
/// operator's, and they would rather have the field where they left it than
/// see the whole of what is waiting.
const STACK_HEIGHT: f32 = 96.0;

/// What stands in for a line ending where a message is drawn on one line.
///
/// U+21B5 rather than the return symbol at U+23CE: the arrow is the older
/// character and the one a text face is far likelier to carry, and a glyph
/// the font has no drawing for would put a box in the middle of a message.
const LINE_BREAK_MARK: &str = "\u{21b5}";

/// The identifier of the message field, which the keyboard filter needs before
/// the field is drawn.
pub fn draft_id() -> Id {
    Id::new("transmit-draft")
}

/// How wide the list of set messages is, in points.
///
/// Wide enough for a name and the arrow, and no wider: it stands in the row
/// of buttons, and a list that took the width of its longest entry would move
/// the buttons every time one was added to the file.
const TEMPLATE_LIST_WIDTH: f32 = 96.0;

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

/// Draws the whole transmit area: the field, and the buttons and list that
/// write into it.
///
/// Putting it on the air is not here but in the panel beside the text, where
/// SSTV keeps the same control.
pub(super) fn transmit_panel(ui: &mut Ui, app: &mut App) {
    ui.add_space(2.0);
    draft(ui, app);
    macro_buttons(ui, app);
    ui.add_space(2.0);
}

/// Whether anything is on the air or waiting behind it.
///
/// What decides whether the stack is drawn at all: an idle transmitter has
/// nothing to say about itself, and a pane that was there to say so would
/// only be taking height from the text.
pub(super) fn has_pending(app: &App) -> bool {
    app.transmit.sending().is_some() || app.transmit.queued().len() > 0
}

/// The message on the air and the ones waiting behind it.
///
/// A pane of its own above the field rather than the top of it, so the field
/// and its buttons stay where they were put: a message arriving in the queue
/// would otherwise push them down the window mid-sentence.
pub(super) fn pending_panel(ui: &mut Ui, app: &mut App) {
    ui.add_space(2.0);
    // Cut off at the pane's own edge rather than a few points past it, so a
    // long queue does not print its fifth row over the field below.
    ui.visuals_mut().clip_rect_margin = 0.0;
    egui::ScrollArea::vertical()
        .id_salt("pending")
        .max_height(STACK_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            on_air(ui, app);
            queued(ui, app);
        });
    ui.add_space(2.0);
}

/// A row's button and label, in the column every row shares.
///
/// The button is on the left of the column and the label against its right,
/// so the messages all begin at the same place and the labels all end at it.
/// The message on the air is given the button too, disabled: it is dropped by
/// stopping rather than by leaving the queue, and a button that was only on
/// some of the rows would set those rows' labels apart from the rest.
///
/// Returns whether the button was pressed.
fn stack_label(ui: &mut Ui, label: RichText, drop_hint: Option<&str>) -> bool {
    let height = ui.spacing().interact_size.y;
    let mut dropped = false;
    ui.allocate_ui_with_layout(
        egui::vec2(STACK_LABEL_WIDTH, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(STACK_LABEL_WIDTH, height));
            ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);
            let button = egui::Button::new(RichText::new("✖").size(LABEL));
            let response = ui
                .add_enabled_ui(drop_hint.is_some(), |ui| {
                    ui.add_sized([DROP_BUTTON_WIDTH, height], button)
                })
                .inner;
            if let Some(hint) = drop_hint {
                dropped = response.on_hover_text(hint).clicked();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(label));
        },
    );
    dropped
}

/// The row under the field: the macro buttons, the list of set messages
/// beside them, and the keys that reach both.
///
/// A press writes the message into the field at the caret rather than sending
/// it, so what is about to go out can be read and edited first; a macro marked
/// to send in `macros.toml` is the exception, and goes out as a message of its
/// own rather than joining whatever is half written.
fn macro_buttons(ui: &mut Ui, app: &mut App) {
    if app.macros.is_empty() && app.templates.is_empty() {
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
    let mut picked = None;
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
        picked = template_list(ui, app);
    });

    if let Some(index) = pressed {
        write_at_caret(ui, app, |app, caret| app.apply_macro(index, caret));
    }
    if let Some(index) = picked {
        // The caret it was handed is ignored: a set message replaces the
        // draft, and the caret lands at the end of what it wrote.
        write_at_caret(ui, app, |app, _| app.apply_template(index));
    }
}

/// The set messages, listed beside the buttons.
///
/// A list rather than more buttons, which is where MMTTY puts the same thing:
/// these are the messages said in the middle of a contact rather than the ones
/// that open and close it, and there are more of them than a row of buttons
/// could hold without becoming something to read instead of press.
///
/// Returns the one that was chosen.
fn template_list(ui: &mut Ui, app: &App) -> Option<usize> {
    if app.templates.is_empty() {
        return None;
    }
    let mut picked = None;
    // The first nine answer to the function keys the buttons use, held down
    // with the modifier, as the original's own list does.
    for (index, key) in FUNCTION_KEYS.iter().enumerate().take(app.templates.len().min(9)) {
        let struck = ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::COMMAND, *key) || input.consume_key(egui::Modifiers::CTRL, *key)
        });
        if struck {
            picked = Some(index);
        }
    }

    let label = app.i18n.text("label-templates");
    let hint = app.i18n.text("hint-templates");
    ComboBox::from_id_salt("templates")
        .selected_text(RichText::new(label).size(SMALL))
        .width(TEMPLATE_LIST_WIDTH)
        .show_ui(ui, |ui| {
            for (index, template) in app.templates.iter().enumerate() {
                let row = ui.selectable_label(false, RichText::new(&template.name).size(SMALL));
                // What it will write, because a name is what the operator
                // gave it rather than what it says.
                if row.on_hover_text(one_line(&template.text)).clicked() {
                    picked = Some(index);
                }
            }
        })
        .response
        .on_hover_text(hint);
    picked
}

/// Writes a message into the field at the caret, and leaves the caret after
/// it.
///
/// `apply` is handed where the caret was and answers with where it should end
/// up, or with nothing for a message that went out rather than being written.
fn write_at_caret(ui: &Ui, app: &mut App, apply: impl FnOnce(&mut App, usize) -> Option<usize>) {
    let id = draft_id();
    let mut state = egui::TextEdit::load_state(ui.ctx(), id);
    let caret = state.as_ref().and_then(|state| state.cursor.char_range()).map_or_else(
        || app.transmit.draft.chars().count(),
        |range| range.primary.index.max(range.secondary.index).into(),
    );
    let Some(after) = apply(app, caret) else {
        return;
    };
    // The caret lands after what was written, so a message put in mid-sentence
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
        let color = sent_color(ui.visuals());
        stack_label(ui, RichText::new(label).size(LABEL).color(color), None);
        // The whole message, scrolled sideways rather than wrapped: the row is
        // one line high so the ones behind it keep their places, and what is
        // read off it is where the underline has reached.
        egui::ScrollArea::horizontal()
            .id_salt("on-air")
            .max_height(ui.spacing().interact_size.y)
            .show(ui, |ui| {
                ui.add(egui::Label::new(sent_job(ui, sending.text(), progress)).extend());
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
    let sent = egui::TextFormat {
        font_id: font.clone(),
        color,
        underline: egui::Stroke::new(1.0, sent_color(ui.visuals())),
        ..egui::TextFormat::default()
    };
    let waiting = egui::TextFormat {
        font_id: font,
        color: ui.visuals().weak_text_color(),
        ..egui::TextFormat::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&text[..boundary].replace('\n', LINE_BREAK_MARK), 0.0, sent);
    job.append(&text[boundary..].replace('\n', LINE_BREAK_MARK), 0.0, waiting);
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
    // The same family as the message on the air above it, because these are
    // the same messages a moment earlier.
    let font = egui::FontId::monospace(ui.text_style_height(&TextStyle::Body));
    for (index, message) in pending.iter().enumerate() {
        ui.horizontal(|ui| {
            if stack_label(ui, RichText::new(&label).size(LABEL).weak(), Some(&drop_hint)) {
                dropping = Some(index);
            }
            let text = RichText::new(one_line(message)).font(font.clone()).weak();
            ui.add(egui::Label::new(text).truncate());
        });
    }
    if let Some(index) = dropping {
        app.transmit.cancel_queued(index);
    }
}

/// A message as one line, for a list that shows what is waiting rather than
/// what it says.
fn one_line(message: &str) -> String {
    let flattened = message.replace('\n', &format!(" {LINE_BREAK_MARK} "));
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
    // Monospaced, like the text the pane above prints: a message is written
    // against what it answers, and RYRY and a contest exchange are columns.
    let font = egui::FontId::monospace(ui.text_style_height(&TextStyle::Body));
    let row = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let mut layouter = {
        let font = font.clone();
        move |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| input::layout(ui, text.as_str(), &font, wrap_width)
    };
    // Frameless, because the border and the ground below are this module's:
    // the field is held to four rows by the area it scrolls in, and the bar
    // that scrolls it belongs inside the border rather than beside it.
    let field = egui::TextEdit::multiline(&mut app.transmit.draft)
        .id(id)
        .font(font)
        .frame(egui::Frame::NONE)
        .desired_rows(DRAFT_ROWS)
        .desired_width(f32::INFINITY)
        .hint_text(app.i18n.text("hint-draft"))
        .layouter(&mut layouter);
    // Held to its rows rather than growing with what is written: the field
    // scrolls, and everything under it stays where the operator left it.
    //
    // The bar is solid and always drawn rather than the floating one that
    // fades in over the text: it is the only thing that says the message runs
    // past the bottom of the field, and a message is read while it is being
    // written rather than while the pointer is over it. Always, because a bar
    // that appeared on the fifth line would reflow the four above it.
    let corner_radius = ui.visuals().widgets.inactive.corner_radius;
    let framed = egui::Frame::new()
        .inner_margin(DRAFT_MARGIN)
        .corner_radius(corner_radius)
        .fill(ui.visuals().text_edit_bg_color())
        .show(ui, |ui| {
            ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
            // egui lets a scrolling area paint a few points past its own
            // edge, which here would print the fifth line of a message
            // outside the field's border.
            ui.visuals_mut().clip_rect_margin = 0.0;
            egui::ScrollArea::vertical()
                .id_salt("draft")
                // Rounded down, so the row after the last one begins below
                // the area rather than showing the tops of its letters.
                .max_height((row * DRAFT_ROWS as f32).floor())
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show(ui, |ui| ui.add(field))
                .inner
        });
    let response = framed.inner;
    // The border a field draws for itself, drawn around the scrolling area
    // instead, and answering to the same three states: what is being typed
    // into has to look like it, and the field inside no longer says so.
    let stroke = if response.has_focus() {
        ui.visuals().selection.stroke
    } else if framed.response.hovered() {
        ui.visuals().widgets.hovered.bg_stroke
    } else {
        ui.visuals().widgets.inactive.bg_stroke
    };
    ui.painter()
        .rect_stroke(framed.response.rect, corner_radius, stroke, egui::StrokeKind::Inside);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A queued message is listed to say what is waiting rather than to be
    /// read, so its line endings become a mark and its length is capped.
    #[test]
    fn a_listed_message_is_one_line() {
        assert_eq!(one_line("CQ CQ\nDE JL1HIS"), "CQ CQ ↵ DE JL1HIS");
        assert_eq!(one_line("RY RY"), "RY RY");
    }

    #[test]
    fn a_long_message_is_cut_rather_than_pushing_the_row_wide() {
        let listed = one_line(&"RY".repeat(100));
        assert!(listed.ends_with('…'), "{listed}");
        assert_eq!(listed.chars().count(), 61);
    }

    /// The mark is the arrow at U+21B5, which is what a text face is likeliest
    /// to have a glyph for.
    #[test]
    fn the_line_ending_mark_is_the_arrow() {
        assert_eq!(LINE_BREAK_MARK, "\u{21b5}");
        assert_eq!(LINE_BREAK_MARK.chars().count(), 1);
    }
}
