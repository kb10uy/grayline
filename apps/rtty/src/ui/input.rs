//! What may be typed into a field whose contents go on the air.
//!
//! Baudot has no lower case and 32 codes per shift, so a field that accepted
//! anything would be a field whose contents cannot be sent. The two ways text
//! arrives are treated differently on purpose. A keystroke is one character
//! and the operator is watching the caret, so a character with no code is
//! simply not added — the field answers the way a teleprinter with no such key
//! does. Pasted and expanded text arrives in a block, and silently dropping
//! part of it would hide what was lost, so it is kept and drawn in red, and
//! the send button waits until it has gone.

use std::sync::Arc;

use egui::{Color32, Context, Event, FontId, Galley, Id, ImeEvent, TextFormat, Ui, text::LayoutJob};

use crate::app::text::{is_sendable, normalize, typed};

const REFUSED_COLOR: Color32 = Color32::from_rgb(0xD0, 0x40, 0x40);

/// Rewrites this frame's text input for the field holding focus.
///
/// Called before the field is added, because the field consumes the events as
/// it is drawn. Focus is what limits it to that one field: everything else in
/// the window — a macro editor, the search of a menu — keeps the keyboard it
/// would otherwise lose.
pub fn sanitize(context: &Context, id: Id) {
    if !context.memory(|memory| memory.has_focus(id)) {
        return;
    }
    context.input_mut(|input| {
        input.events.retain_mut(|event| match event {
            Event::Text(text) => {
                *text = typed(text);
                !text.is_empty()
            }
            // Kept even once it is empty: an IME commit is also what ends the
            // composition, and a field that never saw one would hold a
            // preedit that has already been decided.
            Event::Ime(ImeEvent::Commit(text)) => {
                *text = typed(text);
                true
            }
            Event::Paste(text) => {
                *text = normalize(text);
                true
            }
            _ => true,
        });
    });
}

/// Lays text out with the characters that cannot be sent marked.
///
/// Built as one job of two formats rather than per character, so a message
/// with nothing wrong in it is laid out as a single run.
///
/// The font is the caller's rather than the style's, because a field whose
/// contents go on the air is monospaced: what is written is read back against
/// the text the same message printed in the pane above it.
pub fn layout(ui: &Ui, text: &str, font: &FontId, wrap_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    let plain = TextFormat {
        font_id: font.clone(),
        color: ui.visuals().text_color(),
        ..TextFormat::default()
    };
    let refused = TextFormat {
        font_id: font.clone(),
        color: REFUSED_COLOR,
        underline: egui::Stroke::new(1.0, REFUSED_COLOR),
        ..TextFormat::default()
    };
    for (sendable, run) in runs(text) {
        job.append(run, 0.0, if sendable { plain.clone() } else { refused.clone() });
    }
    job.wrap.max_width = wrap_width;
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn runs(text: &str) -> Vec<(bool, &str)> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut current = None;
    for (index, character) in text.char_indices() {
        let sendable = is_sendable(character);
        match current {
            Some(previous) if previous == sendable => {}
            Some(previous) => {
                runs.push((previous, &text[start..index]));
                start = index;
                current = Some(sendable);
            }
            None => current = Some(sendable),
        }
    }
    if let Some(sendable) = current {
        runs.push((sendable, &text[start..]));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_can_all_be_sent_is_one_run() {
        assert_eq!(runs("CQ DE"), [(true, "CQ DE")]);
        assert_eq!(runs(""), []);
    }

    #[test]
    fn refused_characters_are_split_into_runs_of_their_own() {
        assert_eq!(runs("OK %% GO"), [(true, "OK "), (false, "%%"), (true, " GO")]);
        assert_eq!(runs("%A"), [(false, "%"), (true, "A")]);
        assert_eq!(runs("A%"), [(true, "A"), (false, "%")]);
    }

    #[test]
    fn runs_are_split_on_character_boundaries() {
        assert_eq!(runs("あA"), [(false, "あ"), (true, "A")]);
        assert_eq!(runs("ああAA"), [(false, "ああ"), (true, "AA")]);
    }
}
