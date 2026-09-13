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

use grayline_rtty::code::Ita2Encoder;

/// What an unsendable character is drawn in.
const REFUSED_COLOR: Color32 = Color32::from_rgb(0xD0, 0x40, 0x40);

/// Whether the transmitter has a code for `character`.
///
/// The line endings are sendable although no single code carries them: the
/// encoder turns a bare line feed into the carriage return and line feed pair
/// the protocol's own line ending uses.
pub fn is_sendable(character: char) -> bool {
    matches!(character, '\n' | '\r') || Ita2Encoder::maps(character)
}

/// The first character of `text` that could not be sent.
pub fn first_unsendable(text: &str) -> Option<char> {
    text.chars().find(|character| !is_sendable(*character))
}

/// Brings text arriving in a block into the shape the field holds.
///
/// Upper cased because that is what leaves the transmitter whatever was
/// written, and a field that showed one and sent the other would be lying
/// about the message. Line endings are reduced to the single character the
/// field stores, so a paste from a document does not leave stray carriage
/// returns between its lines. What has no code at all is deliberately kept.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            characters.next_if_eq(&'\n');
            out.push('\n');
            continue;
        }
        out.push(character.to_ascii_uppercase());
    }
    out
}

/// Drops what a keystroke cannot put on the air, and upper cases the rest.
fn typed(text: &str) -> String {
    text.chars()
        .map(|character| character.to_ascii_uppercase())
        .filter(|character| is_sendable(*character))
        .collect()
}

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

/// Splits `text` into runs that are all sendable or all not.
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
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case('A', true)]
    #[case('a', true)]
    #[case('1', true)]
    #[case(' ', true)]
    #[case('-', true)]
    #[case('/', true)]
    #[case('\n', true)]
    #[case('\r', true)]
    #[case('%', false)]
    #[case('{', false)]
    #[case('}', false)]
    #[case('あ', false)]
    #[case('*', false)]
    fn the_sendable_characters_are_the_ones_the_encoder_has_a_code_for(
        #[case] character: char,
        #[case] expected: bool,
    ) {
        assert_eq!(is_sendable(character), expected);
    }

    /// A keystroke that cannot go on the air simply does not appear, the way
    /// a teleprinter with no such key answers.
    #[test]
    fn typing_drops_what_cannot_be_sent_and_upper_cases_the_rest() {
        assert_eq!(typed("cq"), "CQ");
        assert_eq!(typed("%"), "");
        assert_eq!(typed("de jl1his"), "DE JL1HIS");
    }

    /// Upper casing is the encoder's own, which folds ASCII and leaves the
    /// rest alone rather than turning one character into two.
    #[test]
    fn upper_casing_does_not_invent_characters() {
        assert_eq!(normalize("ß"), "ß");
        assert_eq!(first_unsendable(&normalize("ß")), Some('ß'));
    }

    /// Pasting keeps what it cannot send, because a block that lost part of
    /// itself silently would be worse than one that shows the problem.
    #[test]
    fn pasting_keeps_what_cannot_be_sent() {
        assert_eq!(normalize("100% copy"), "100% COPY");
        assert_eq!(first_unsendable(&normalize("100% copy")), Some('%'));
    }

    #[test]
    fn pasted_line_endings_become_the_one_the_field_holds() {
        assert_eq!(normalize("CQ\r\nDE\rTEST\n"), "CQ\nDE\nTEST\n");
    }

    #[test]
    fn a_message_with_nothing_wrong_in_it_has_no_refused_character() {
        assert_eq!(first_unsendable("CQ CQ DE JL1HIS K"), None);
        assert_eq!(first_unsendable(""), None);
    }

    #[test]
    fn the_first_refused_character_is_the_one_reported() {
        assert_eq!(first_unsendable("OK % AND *"), Some('%'));
    }

    /// The runs are what the field is drawn from: text with nothing wrong in
    /// it has to lay out as one, or every message pays for the check.
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

    /// A run is split on a byte index that has to be a character boundary, or
    /// laying out a message with anything but ASCII in it would panic.
    #[test]
    fn runs_are_split_on_character_boundaries() {
        assert_eq!(runs("あA"), [(false, "あ"), (true, "A")]);
        assert_eq!(runs("ああAA"), [(false, "ああ"), (true, "AA")]);
    }
}
