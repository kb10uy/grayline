//! What the operator is writing, what is queued behind it, and what is on air.
//!
//! Kept apart from [`App`](crate::app::App) because it is a small machine of
//! its own: a draft becomes a queued message, a queued message becomes a
//! transmission, and a transmission gives its unsent remainder back. The
//! application drives it once per frame and reads it to draw.

use std::collections::VecDeque;

use grayline_rtty::TxSchedule;

/// How much of a message has left for the rig.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SentProgress {
    /// Characters of the message that have finished going out.
    pub sent: usize,
    /// Characters in the message altogether.
    pub total: usize,
}

/// The message on the air, and where its characters land in the audio.
#[derive(Debug)]
pub struct Sending {
    text: String,
    schedule: TxSchedule,
    /// How many characters have been handed to the received text.
    ///
    /// Echoing follows what has been played rather than what has been
    /// generated, so the printed copy and the signal agree; this is how far
    /// that has got, so a frame prints only what is new.
    echoed: usize,
}

impl Sending {
    pub fn new(text: String, schedule: TxSchedule) -> Self {
        Self {
            text,
            schedule,
            echoed: 0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn progress(&self, played_samples: u64) -> SentProgress {
        SentProgress {
            sent: self.schedule.characters_sent_by(played_samples),
            total: self.schedule.character_count(),
        }
    }

    /// The characters played since this was last asked, for the echo.
    pub fn take_echo(&mut self, played_samples: u64) -> Option<String> {
        let sent = self.schedule.characters_sent_by(played_samples);
        if sent <= self.echoed {
            return None;
        }
        let text: String = self.text.chars().take(sent).skip(self.echoed).collect();
        self.echoed = sent;
        Some(text)
    }

    /// Everything that has not been played, for a transmission cut short.
    pub fn unsent(&self, played_samples: u64) -> String {
        let sent = self.schedule.characters_sent_by(played_samples);
        self.text.chars().skip(sent).collect()
    }

    /// How much of the whole transmission is left, in samples.
    pub fn remaining_samples(&self, played_samples: u64) -> u64 {
        self.schedule.total_samples().saturating_sub(played_samples)
    }
}

/// The draft, the queue, and the message being keyed.
#[derive(Debug, Default)]
pub struct Transmit {
    /// What the operator is writing.
    pub draft: String,
    queue: VecDeque<String>,
    sending: Option<Sending>,
}

impl Transmit {
    /// Moves the draft into the queue, if there is anything in it.
    ///
    /// Returns whether anything was queued, so the caller can leave the
    /// keyboard focus and the notice alone when there was not.
    pub fn queue_draft(&mut self) -> bool {
        let text = self.draft.trim_end_matches(['\r', '\n']).to_owned();
        if text.is_empty() {
            return false;
        }
        self.draft.clear();
        self.queue(text);
        true
    }

    /// Queues `text` without touching the draft, for a macro that sends.
    pub fn queue(&mut self, text: String) {
        if !text.is_empty() {
            self.queue.push_back(text);
        }
    }

    /// Takes the next message to key, if nothing is on the air.
    pub fn take_next(&mut self) -> Option<String> {
        if self.sending.is_some() {
            return None;
        }
        self.queue.pop_front()
    }

    pub fn begin(&mut self, sending: Sending) {
        self.sending = Some(sending);
    }

    pub fn sending(&self) -> Option<&Sending> {
        self.sending.as_ref()
    }

    pub fn sending_mut(&mut self) -> Option<&mut Sending> {
        self.sending.as_mut()
    }

    /// What is still waiting, in the order it will go out.
    pub fn queued(&self) -> impl ExactSizeIterator<Item = &str> {
        self.queue.iter().map(String::as_str)
    }

    pub fn is_busy(&self) -> bool {
        self.sending.is_some() || !self.queue.is_empty()
    }

    /// Forgets the message on the air, for one that ended by itself.
    pub fn finish(&mut self) {
        self.sending = None;
    }

    /// Drops a queued message that has not started.
    pub fn cancel_queued(&mut self, index: usize) {
        self.queue.remove(index);
    }

    /// Gives up everything and hands back what was never sent.
    ///
    /// The remainder of the message on the air comes first, then whatever was
    /// queued behind it, separated the way the draft separates lines: an
    /// aborted transmission is resumed by editing and pressing send again, and
    /// that wants the text back rather than a record of how it was divided.
    pub fn abandon(&mut self, played_samples: u64) -> String {
        let unsent = self
            .sending
            .take()
            .map(|sending| sending.unsent(played_samples))
            .unwrap_or_default();
        self.abandon_queue(unsent)
    }

    /// Hands back `leading` and everything still queued behind it.
    ///
    /// Used where a message was taken off the queue and then could not be
    /// started: it has already left the queue, and dropping it there would
    /// lose text the operator wrote without anything saying so.
    pub fn abandon_queue(&mut self, leading: String) -> String {
        let mut parts = Vec::new();
        if !leading.is_empty() {
            parts.push(leading);
        }
        parts.extend(self.queue.drain(..));
        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use grayline_rtty::TxConfig;

    use super::*;

    const RATE: u32 = 8_000;

    fn schedule(text: &str) -> TxSchedule {
        TxSchedule::new(text, RATE, &TxConfig::default()).unwrap()
    }

    fn sending(text: &str) -> Sending {
        Sending::new(text.to_owned(), schedule(text))
    }

    #[test]
    fn a_draft_becomes_a_queued_message_and_leaves_the_field_empty() {
        let mut transmit = Transmit {
            draft: "CQ DE JL1HIS".to_owned(),
            ..Transmit::default()
        };

        assert!(transmit.queue_draft());

        assert!(transmit.draft.is_empty());
        assert_eq!(transmit.queued().collect::<Vec<_>>(), ["CQ DE JL1HIS"]);
    }

    /// Pressing send on nothing must not key the rig, and must not clear a
    /// field that only held the newline that got there by accident.
    #[test]
    fn an_empty_draft_queues_nothing() {
        let mut transmit = Transmit::default();
        assert!(!transmit.queue_draft());
        transmit.draft = "\n\n".to_owned();
        assert!(!transmit.queue_draft());
        assert_eq!(transmit.queued().len(), 0);
    }

    /// The trailing newline is what the Enter key leaves behind, and sending
    /// it would put a blank line on the air at the end of every message.
    #[test]
    fn a_trailing_line_ending_is_not_part_of_the_message() {
        let mut transmit = Transmit {
            draft: "RY RY\r\n".to_owned(),
            ..Transmit::default()
        };
        transmit.queue_draft();
        assert_eq!(transmit.queued().collect::<Vec<_>>(), ["RY RY"]);
    }

    #[test]
    fn messages_are_keyed_one_at_a_time_in_the_order_they_were_queued() {
        let mut transmit = Transmit::default();
        transmit.queue("first".to_owned());
        transmit.queue("second".to_owned());

        assert_eq!(transmit.take_next().as_deref(), Some("first"));
        transmit.begin(sending("first"));
        assert_eq!(transmit.take_next(), None);

        transmit.finish();
        assert_eq!(transmit.take_next().as_deref(), Some("second"));
    }

    #[test]
    fn a_queued_message_can_be_dropped_before_it_starts() {
        let mut transmit = Transmit::default();
        transmit.queue("first".to_owned());
        transmit.queue("second".to_owned());

        transmit.cancel_queued(0);

        assert_eq!(transmit.queued().collect::<Vec<_>>(), ["second"]);
    }

    /// What the operator gets back is what never left, so it can be edited and
    /// sent again.
    #[test]
    fn abandoning_returns_the_part_that_was_never_played() {
        let text = "RYRYRY";
        let mut transmit = Transmit::default();
        let sending = sending(text);
        let ends = sending.progress(u64::MAX);
        assert_eq!(ends.total, 6);
        let third = schedule(text).character_ends()[2];
        transmit.begin(sending);
        transmit.queue("QUEUED".to_owned());

        let returned = transmit.abandon(third);

        // Three characters had finished, so what comes back starts at the
        // fourth and carries the queue behind it.
        assert_eq!(returned, "YRY\nQUEUED");
        assert!(!transmit.is_busy());
    }

    #[test]
    fn abandoning_before_anything_played_returns_the_whole_message() {
        let mut transmit = Transmit::default();
        transmit.begin(sending("CQ"));
        assert_eq!(transmit.abandon(0), "CQ");
    }

    /// The echo prints what has been played, and prints each character once.
    #[test]
    fn the_echo_hands_over_each_character_exactly_once() {
        let text = "RYRY";
        let ends = schedule(text).character_ends().to_vec();
        let mut sending = sending(text);

        assert_eq!(sending.take_echo(0), None);
        assert_eq!(sending.take_echo(ends[1]).as_deref(), Some("RY"));
        assert_eq!(sending.take_echo(ends[1]), None);
        assert_eq!(sending.take_echo(ends[3]).as_deref(), Some("RY"));
        assert_eq!(sending.take_echo(u64::MAX), None);
    }

    #[test]
    fn progress_counts_the_characters_that_have_left() {
        let text = "CQ CQ";
        let ends = schedule(text).character_ends().to_vec();
        let sending = sending(text);

        assert_eq!(sending.progress(0), SentProgress { sent: 0, total: 5 });
        assert_eq!(sending.progress(ends[2]), SentProgress { sent: 3, total: 5 });
        assert_eq!(sending.progress(u64::MAX), SentProgress { sent: 5, total: 5 });
    }

    #[test]
    fn the_remaining_length_runs_out_rather_than_going_negative() {
        let sending = sending("RY");
        assert!(sending.remaining_samples(0) > 0);
        assert_eq!(sending.remaining_samples(u64::MAX), 0);
    }

    /// A message is only busy while there is something to send, which is what
    /// the stop button and the transmit indicator are drawn from.
    #[test]
    fn nothing_queued_and_nothing_sending_is_not_busy() {
        let mut transmit = Transmit::default();
        assert!(!transmit.is_busy());
        transmit.queue("RY".to_owned());
        assert!(transmit.is_busy());
        transmit.take_next();
        assert!(!transmit.is_busy());
    }
}
