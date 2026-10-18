use std::sync::{Mutex, PoisonError};

/// Applies a change to a published snapshot, leaving it alone if the worker
/// that owns it panicked while holding the lock.
pub fn update<T>(snapshot: &Mutex<T>, update: impl FnOnce(&mut T)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}

/// Asks the interface to draw a frame it has no other reason to draw.
///
/// A worker runs ahead of the interface, and nothing the operator does makes
/// what it produced appear, so the frame that shows it has to be asked for by
/// whoever produced it. Without this the interface would have to redraw at
/// the monitor's rate whether or not anything had happened.
///
/// A waker with no context behind it does nothing, which is what a worker
/// driven by a test holds.
#[derive(Clone, Default)]
pub struct Waker(Option<egui::Context>);

impl Waker {
    /// A waker that asks `context` for a frame.
    pub const fn new(context: egui::Context) -> Self {
        Self(Some(context))
    }

    /// Asks for a frame, if there is an interface to ask.
    pub fn wake(&self) {
        if let Some(context) = self.0.as_ref() {
            context.request_repaint();
        }
    }
}

impl core::fmt::Debug for Waker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_tuple("Waker").field(&self.0.is_some()).finish()
    }
}

/// What a worker publishes through a [`Mailbox`].
pub trait Snapshot {
    /// The part of a snapshot the interface draws.
    ///
    /// The interface is asked for a frame only when this changes, so an idle
    /// worker leaves it alone entirely. Readings are quantized into it, so a
    /// level that wobbles in its last digit does not cost a frame.
    type Visible: Default + PartialEq;

    /// What the interface would draw from this snapshot.
    fn visible(&self) -> Self::Visible;

    /// Folds `previous`, published and not yet collected, into this one.
    ///
    /// Whatever only one snapshot carries — a decoded row, an error, text that
    /// will never be decoded again — has to survive being replaced by the next
    /// before the interface came for it.
    fn absorb(&mut self, previous: Self);
}

/// A single-slot handoff holding the newest snapshot.
///
/// The interface only ever uses the newest snapshot, so the worker overwrites
/// the slot instead of queueing. This bounds the handoff by construction: an
/// interface that stops polling cannot make the worker accumulate frames.
pub struct Mailbox<S: Snapshot> {
    slot: Mutex<Slot<S>>,
    waker: Waker,
}

struct Slot<S: Snapshot> {
    pending: Option<S>,
    shown: S::Visible,
}

impl<S: Snapshot> Mailbox<S> {
    /// An empty mailbox that wakes the interface through `waker`.
    pub fn new(waker: Waker) -> Self {
        Self {
            slot: Mutex::new(Slot {
                pending: None,
                shown: S::Visible::default(),
            }),
            waker,
        }
    }

    /// Replaces the pending snapshot, keeping payloads not yet collected, and
    /// wakes the interface if it would now draw something different.
    pub fn publish(&self, mut snapshot: S) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = slot.pending.take() {
            snapshot.absorb(previous);
        }
        let visible = snapshot.visible();
        slot.pending = Some(snapshot);
        if visible != slot.shown {
            slot.shown = visible;
            drop(slot);
            self.waker.wake();
        }
    }

    /// Takes the pending snapshot, or nothing when none arrived since the last
    /// take.
    pub fn take(&self) -> Option<S> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).pending.take()
    }
}

impl<S: Snapshot> core::fmt::Debug for Mailbox<S> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Mailbox").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Text {
        reading: u8,
        text: String,
    }

    impl Snapshot for Text {
        type Visible = u8;

        fn visible(&self) -> u8 {
            self.reading
        }

        fn absorb(&mut self, previous: Self) {
            self.text.insert_str(0, &previous.text);
        }
    }

    #[test]
    fn an_uncollected_snapshot_is_folded_into_the_next() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(Text {
            reading: 1,
            text: "CQ ".to_owned(),
        });
        mailbox.publish(Text {
            reading: 2,
            text: "DE".to_owned(),
        });

        let taken = mailbox.take().unwrap();
        assert_eq!((taken.reading, taken.text.as_str()), (2, "CQ DE"));
        assert!(mailbox.take().is_none());
    }
}
