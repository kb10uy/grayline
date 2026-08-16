//! The received text, and the pane it is printed in.

use egui::{Align, FontId, Layout, RichText, ScrollArea, TextStyle, Ui};

/// How many lines one decode path keeps.
///
/// A watch runs for hours and nothing here is written to disk yet, so the
/// oldest lines go. Five hundred is about two hours of a station calling CQ,
/// and it is what the pane can lay out in a frame without the whole of it
/// being re-measured while text arrives.
pub const LINE_LIMIT: usize = 500;

/// The text one decode path has printed.
///
/// Kept as one string rather than a list of lines because that is what the
/// pane draws: egui lays a paragraph out once and caches it, so a scrollback
/// assembled from its lines on every frame would be re-measured six times a
/// second at 45.45 baud.
#[derive(Clone, Debug, Default)]
pub struct Scrollback {
    text: String,
    /// Whether the characters seen since the last printable one asked for a
    /// new line.
    ///
    /// A teleprinter line ends with CR CR LF, and a pane that honoured each
    /// of those would double-space everything, so a run of them is one break.
    /// The break is held until something is printed after it, which is what
    /// keeps a blank line off the bottom of the pane between transmissions.
    pending_break: bool,
}

impl Scrollback {
    /// Prints what a decode path decoded.
    pub fn push_str(&mut self, text: &str) {
        for character in text.chars() {
            self.push(character);
        }
    }

    fn push(&mut self, character: char) {
        match character {
            '\r' | '\n' => {
                self.pending_break = true;
                return;
            }
            // The bell is a sound rather than a character; nothing here makes
            // one, and printing U+0007 would put a box in the middle of a
            // callsign.
            '\u{7}' => return,
            _ => {}
        }
        if self.pending_break {
            self.pending_break = false;
            self.text.push('\n');
            self.trim();
        }
        self.text.push(character);
    }

    /// Drops the oldest lines once the pane is holding more than it keeps.
    ///
    /// Trimmed a tenth of the limit at a time rather than a line at a time,
    /// so a long watch does not move the whole buffer on every line.
    fn trim(&mut self) {
        let lines = self.text.bytes().filter(|byte| *byte == b'\n').count() + 1;
        if lines <= LINE_LIMIT {
            return;
        }
        let dropping = lines - LINE_LIMIT + LINE_LIMIT / 10;
        let Some(cut) = self
            .text
            .char_indices()
            .filter(|(_, character)| *character == '\n')
            .nth(dropping - 1)
            .map(|(index, _)| index + 1)
        else {
            return;
        };
        self.text.drain(..cut);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.pending_break = false;
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Draws one decode path's text, following the newest line.
///
/// Monospaced, because RTTY is a teleprinter: RYRY tuning patterns and the
/// callsign columns of a contest exchange are read as columns.
pub fn pane(ui: &mut Ui, scrollback: &Scrollback, hint: &str) {
    let size = ui.text_style_height(&TextStyle::Body);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                if scrollback.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(hint).weak());
                    return;
                }
                // Selectable, because the whole point of a received callsign
                // is that it gets copied somewhere else.
                ui.style_mut().interaction.selectable_labels = true;
                ui.add(egui::Label::new(RichText::new(scrollback.text()).font(FontId::monospace(size))).wrap());
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_teleprinter_line_ending_is_one_break() {
        let mut scrollback = Scrollback::default();
        scrollback.push_str("CQ\r\r\nDE\r\r\nJL1HIS");
        assert_eq!(scrollback.text(), "CQ\nDE\nJL1HIS");
    }

    /// A break arriving at the end of a transmission must not leave the pane
    /// scrolled past the last line it printed.
    #[test]
    fn a_break_with_nothing_after_it_is_not_printed_yet() {
        let mut scrollback = Scrollback::default();
        scrollback.push_str("RY\r\n");
        assert_eq!(scrollback.text(), "RY");
        scrollback.push_str("RY");
        assert_eq!(scrollback.text(), "RY\nRY");
    }

    /// The bell is a sound, and a pane that printed U+0007 would put a box in
    /// the middle of a callsign.
    #[test]
    fn the_bell_is_not_printed() {
        let mut scrollback = Scrollback::default();
        scrollback.push_str("A\u{7}B");
        assert_eq!(scrollback.text(), "AB");
    }

    #[test]
    fn clearing_leaves_nothing_behind() {
        let mut scrollback = Scrollback::default();
        scrollback.push_str("TEST\r\n");
        scrollback.clear();
        assert!(scrollback.is_empty());

        // The break the clear threw away must not print on the next line.
        scrollback.push_str("AGAIN");
        assert_eq!(scrollback.text(), "AGAIN");
    }

    /// A watch left running overnight must not grow without bound.
    #[test]
    fn the_oldest_lines_are_dropped_once_the_pane_is_full() {
        let mut scrollback = Scrollback::default();
        for line in 0..LINE_LIMIT * 2 {
            scrollback.push_str(&format!("{line}\r\n"));
        }
        scrollback.push_str("last");

        let lines: Vec<&str> = scrollback.text().lines().collect();
        assert!(lines.len() <= LINE_LIMIT, "kept {} lines", lines.len());
        assert_eq!(lines[lines.len() - 1], "last");
        // Whole lines are dropped, so what is left starts at the beginning of
        // one rather than in the middle of a callsign.
        assert!(
            lines[0].parse::<usize>().is_ok(),
            "the first line was cut: {:?}",
            lines[0]
        );
    }
}
