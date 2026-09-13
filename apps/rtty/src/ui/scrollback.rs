//! The text of a contact, and the pane it is printed in.
//!
//! What was sent is printed here beside what was received, in a colour of its
//! own. A contact is one exchange rather than two, and reading it back from a
//! pane that held only half of it would mean reading it against a transmit
//! window that had already been cleared for the next message.

use core::ops::Range;

use egui::{Align, Color32, FontId, Layout, ScrollArea, TextFormat, TextStyle, Ui, Visuals, text::LayoutJob};

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
    /// Where the text this station sent sits, as byte ranges into `text`.
    ///
    /// Ranges rather than a second buffer, because the pane draws one
    /// paragraph: a transcript assembled from pieces on every frame would be
    /// laid out again every frame.
    sent: Vec<Range<usize>>,
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
            self.push(character, false);
        }
    }

    /// Prints what this station has actually put on the air.
    ///
    /// Fed from the played position rather than from the message, so the
    /// transcript reads in the order the band heard it.
    pub fn push_sent(&mut self, text: &str) {
        for character in text.chars() {
            self.push(character, true);
        }
    }

    fn push(&mut self, character: char, sent: bool) {
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
        let start = self.text.len();
        self.text.push(character);
        if sent {
            self.mark_sent(start..self.text.len());
        }
    }

    /// Records a run as sent, extending the last one where it continues.
    fn mark_sent(&mut self, range: Range<usize>) {
        match self.sent.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => self.sent.push(range),
        }
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
        self.sent.retain_mut(|range| {
            if range.end <= cut {
                return false;
            }
            *range = range.start.saturating_sub(cut)..range.end - cut;
            true
        });
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.sent.clear();
        self.pending_break = false;
    }

    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The transcript split into runs, each marked with whether it was sent.
    fn runs(&self) -> Vec<(bool, &str)> {
        let mut runs = Vec::new();
        let mut position = 0;
        for range in &self.sent {
            if range.start > position {
                runs.push((false, &self.text[position..range.start]));
            }
            runs.push((true, &self.text[range.clone()]));
            position = range.end;
        }
        if position < self.text.len() {
            runs.push((false, &self.text[position..]));
        }
        runs
    }
}

/// What this station's own text is printed in.
///
/// Defined for each theme rather than taken from the palette, because what it
/// has to be is legible and unmistakably not the received text; the built-in
/// roles are either the same weight as body text or reserved for faults.
pub fn sent_color(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(0x7A, 0xC4, 0xF2)
    } else {
        Color32::from_rgb(0x17, 0x5C, 0x99)
    }
}

/// The word at `index`, taken as a callsign is: a run with no spaces in it.
///
/// Punctuation is kept rather than trimmed, because a callsign carries a
/// stroke in it — `JA1ZZZ/1` is one word and cutting it at the stroke would
/// answer the wrong station.
fn word_at(text: &str, index: usize) -> Option<&str> {
    if index > text.len() {
        return None;
    }
    let boundary = |character: char| character.is_whitespace();
    let start = text[..index].rfind(boundary).map_or(0, |at| at + 1);
    let end = text[index..].find(boundary).map_or(text.len(), |at| index + at);
    let word = text[start..end].trim();
    (!word.is_empty()).then_some(word)
}

/// Draws one decode path's text, following the newest line.
///
/// Monospaced, because RTTY is a teleprinter: RYRY tuning patterns and the
/// callsign columns of a contest exchange are read as columns.
///
/// Returns the word the operator double-clicked, which is how a callsign gets
/// from the line that printed it into the field the macros read it from.
pub fn pane(ui: &mut Ui, scrollback: &Scrollback, hint: &str) -> Option<String> {
    let size = ui.text_style_height(&TextStyle::Body);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                if scrollback.is_empty() {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(hint).weak());
                    return None;
                }
                // Selectable, because the whole point of a received callsign
                // is that it gets copied somewhere else.
                ui.style_mut().interaction.selectable_labels = true;
                let mut job = LayoutJob::default();
                let font = FontId::monospace(size);
                for (sent, run) in scrollback.runs() {
                    job.append(
                        run,
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color: if sent {
                                sent_color(ui.visuals())
                            } else {
                                ui.visuals().text_color()
                            },
                            ..TextFormat::default()
                        },
                    );
                }
                job.wrap.max_width = ui.available_width();
                // Laid out here rather than by the label, because the galley
                // is what turns a pointer position back into a place in the
                // text.
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                let response = ui.add(egui::Label::new(galley.clone()).wrap());
                if !response.double_clicked() {
                    return None;
                }
                let pointer = response.interact_pointer_pos()?;
                let cursor = galley.cursor_from_pos(pointer - response.rect.min);
                let index = galley.text()[..]
                    .char_indices()
                    .nth(usize::from(cursor.index))
                    .map_or(galley.text().len(), |(index, _)| index);
                word_at(galley.text(), index).map(str::to_owned)
            })
            .inner
        })
        .inner
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

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

    /// The transcript is one exchange, and which half each part of it came
    /// from is what the colours say.
    #[test]
    fn sent_and_received_text_are_kept_apart_in_one_transcript() {
        let mut scrollback = Scrollback::default();
        scrollback.push_str(
            "CQ DE JA1ZZZ K
",
        );
        scrollback.push_sent(
            "JA1ZZZ DE JL1HIS
",
        );
        scrollback.push_str("R R");

        assert_eq!(
            scrollback.text(),
            "CQ DE JA1ZZZ K
JA1ZZZ DE JL1HIS
R R"
        );
        assert_eq!(
            scrollback.runs(),
            [
                (
                    false,
                    "CQ DE JA1ZZZ K
"
                ),
                (true, "JA1ZZZ DE JL1HIS"),
                (
                    false, "
R R"
                ),
            ]
        );
    }

    /// The echo arrives a character or two at a time as the audio plays, and
    /// a run per arrival would cost a format for every character sent.
    #[test]
    fn text_sent_in_pieces_is_one_run() {
        let mut scrollback = Scrollback::default();
        for piece in ["CQ", " ", "DE"] {
            scrollback.push_sent(piece);
        }
        assert_eq!(scrollback.runs(), [(true, "CQ DE")]);
    }

    #[test]
    fn a_transcript_with_nothing_sent_in_it_is_one_run() {
        let mut scrollback = Scrollback::default();
        scrollback.push_str("RYRY");
        assert_eq!(scrollback.runs(), [(false, "RYRY")]);
    }

    /// The ranges index the text, so dropping the oldest lines has to move
    /// them with it or the colours would land on the wrong characters.
    #[test]
    fn sent_runs_follow_the_text_when_the_oldest_lines_are_dropped() {
        let mut scrollback = Scrollback::default();
        for line in 0..LINE_LIMIT * 2 {
            scrollback.push_str(&format!(
                "{line}
"
            ));
        }
        scrollback.push_sent("DE JL1HIS");

        let runs = scrollback.runs();
        assert_eq!(runs.last(), Some(&(true, "DE JL1HIS")));
        let marked: String = runs.iter().filter(|(sent, _)| *sent).map(|(_, run)| *run).collect();
        assert_eq!(marked, "DE JL1HIS");
    }

    /// A long transmission outlives the trim, and every printed character of
    /// it stays marked as this station's own.
    ///
    /// The breaks between the lines are not marked: one is inserted when the
    /// character after it arrives, and it carries no glyph to colour.
    #[test]
    fn sent_text_stays_marked_through_a_trim() {
        let mut scrollback = Scrollback::default();
        for line in 0..LINE_LIMIT * 2 {
            scrollback.push_sent(&format!(
                "{line}
"
            ));
        }
        let printed = scrollback.text().replace('\n', "");
        let marked: String = scrollback
            .runs()
            .iter()
            .filter(|(sent, _)| *sent)
            .map(|(_, run)| *run)
            .collect();
        assert_eq!(marked, printed);
    }

    /// A callsign is one word of a received line, and the stroke a portable
    /// station signs with is part of it rather than a break in it.
    #[rstest]
    #[case(0, Some("CQ"))]
    #[case(1, Some("CQ"))]
    #[case(2, Some("CQ"))]
    #[case(3, Some("DE"))]
    #[case(6, Some("JA1ZZZ/1"))]
    #[case(10, Some("JA1ZZZ/1"))]
    #[case(15, Some("K"))]
    fn a_word_is_picked_out_of_the_line_that_printed_it(#[case] index: usize, #[case] expected: Option<&str>) {
        assert_eq!(word_at("CQ DE JA1ZZZ/1 K", index), expected);
    }

    #[test]
    fn a_position_in_empty_space_picks_no_word() {
        assert_eq!(word_at("CQ  DE", 3), None);
        assert_eq!(word_at("", 0), None);
        assert_eq!(word_at("CQ", 99), None);
    }

    /// A line break ends a word: the callsign at the end of one line and the
    /// word at the start of the next are two words.
    #[test]
    fn a_line_break_ends_a_word() {
        assert_eq!(
            word_at(
                "JA1ZZZ
DE",
                0
            ),
            Some("JA1ZZZ")
        );
        assert_eq!(
            word_at(
                "JA1ZZZ
DE",
                7
            ),
            Some("DE")
        );
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
