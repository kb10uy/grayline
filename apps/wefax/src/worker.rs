//! The thread that runs a reception, and the audio device it is attached to.
//!
//! The interface never blocks on any of this. The worker owns its own state
//! and publishes a snapshot the interface reads once per frame.

pub mod audio;
pub mod receive;

/// Asks the interface to draw a frame it has no other reason to draw.
///
/// A worker runs ahead of the interface, and nothing the operator does makes a
/// decoded line appear, so the frame that shows one has to be asked for by
/// whoever produced it.
///
/// A waker with no context behind it does nothing, which is what a worker
/// driven by a test holds.
#[derive(Clone, Default)]
pub struct Waker(Option<egui::Context>);

impl Waker {
    pub const fn new(context: egui::Context) -> Self {
        Self(Some(context))
    }

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
