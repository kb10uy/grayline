use std::{sync::Arc, time::Instant};

use grayline_audio::Playback;
use grayline_rig_control::{Keying, RigSnapshot, RigState, RigWorker};

use crate::{
    error::AppError,
    worker::transmit::{TxGain, TxPhase, TxProgress, TxSnapshot, TxWorker},
};

/// What a transmission is sending.
///
/// A tone and a picture use the same worker and the same stream, and
/// everything else about them differs only in what the operator is told.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Picture,
    /// A tune tone, which stops itself at `until`.
    Tune {
        until: Instant,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ending {
    Complete,
    Cancelled,
    Failed(String),
}

/// The transmission going out, if one is, and how the last one ended.
#[derive(Debug)]
pub struct Outgoing {
    /// The amplitude a running transmission reads, shared with its worker.
    gain: Arc<TxGain>,
    running: Option<Running>,
    completed: bool,
}

#[derive(Debug)]
struct Running {
    kind: Kind,
    playback: Option<Playback>,
    worker: Option<TxWorker>,
    snapshot: TxSnapshot,
    /// Whether the device was told to start consuming the queue.
    started: bool,
    /// Set while the transmission has asked for the rig to be keyed.
    ///
    /// A transmission that keyed the rig owes it an unkeying however it ends,
    /// so ending one is what gives the keying back.
    keyed: bool,
}

impl Outgoing {
    pub fn new(travel: f32) -> Self {
        Self {
            gain: Arc::new(TxGain::from_travel(travel)),
            running: None,
            completed: false,
        }
    }

    pub fn gain(&self) -> Arc<TxGain> {
        Arc::clone(&self.gain)
    }

    /// Moves the fader, which a running transmission picks up per block.
    pub fn set_level(&self, travel: f32) {
        self.gain.set_travel(travel);
    }

    /// Takes over a stream and the worker filling it, and keys the rig.
    ///
    /// The rig is asked before any audio is played, so the lead-in it needs
    /// runs alongside filling the queue rather than after it.
    pub fn begin(&mut self, kind: Kind, playback: Playback, worker: TxWorker, rig: Option<&RigWorker>) {
        if let Some(rig) = rig {
            rig.transmit(Keying::Audio);
        }
        self.completed = false;
        self.running = Some(Running {
            kind,
            playback: Some(playback),
            worker: Some(worker),
            snapshot: TxSnapshot {
                phase: TxPhase::Priming,
                ..TxSnapshot::default()
            },
            started: false,
            keyed: rig.is_some(),
        });
    }

    /// Reads the worker and the stream, and says why the transmission has to
    /// end if it does.
    ///
    /// The tune deadline is checked before the worker is read, because the
    /// tone it produces never runs out on its own and the deadline is the only
    /// thing that ends it.
    pub fn poll(&mut self, rig: &RigSnapshot, now: Instant) -> Option<Ending> {
        let running = self.running.as_mut()?;
        if let Kind::Tune { until } = running.kind
            && now >= until
        {
            return Some(Ending::Complete);
        }
        running.snapshot = running.worker.as_ref()?.latest();
        running.advance(rig)
    }

    /// Stops the audio and then gives the rig its keying back.
    pub fn end(&mut self, ending: &Ending, rig: Option<&RigWorker>) {
        let keyed = self.running.take().is_some_and(|running| running.keyed);
        if keyed && let Some(rig) = rig {
            rig.receive();
        }
        self.completed = *ending == Ending::Complete;
    }

    /// Unkeys the rig without stopping the audio, for rig control that is
    /// going away while a transmission runs.
    ///
    /// The request has to go before the connection closes for the worker to
    /// see it at all.
    pub fn release_rig(&mut self, rig: Option<&RigWorker>) {
        let Some(running) = self.running.as_mut().filter(|running| running.keyed) else {
            return;
        };
        running.keyed = false;
        if let Some(rig) = rig {
            rig.receive();
        }
    }

    pub const fn is_active(&self) -> bool {
        self.running.is_some()
    }

    pub fn is_tuning(&self) -> bool {
        self.kind().is_some_and(|kind| matches!(kind, Kind::Tune { .. }))
    }

    pub fn is_sending_picture(&self) -> bool {
        self.kind() == Some(Kind::Picture)
    }

    fn kind(&self) -> Option<Kind> {
        self.running.as_ref().map(|running| running.kind)
    }

    pub fn phase(&self) -> TxPhase {
        self.running
            .as_ref()
            .map_or(TxPhase::Idle, |running| running.snapshot.phase)
    }

    /// Whether the last transmission ran to its end, rather than being
    /// stopped or failing.
    pub const fn is_complete(&self) -> bool {
        !self.is_active() && self.completed
    }

    /// Reports which image row the transmission is currently on.
    ///
    /// The clock is the sample count the device callback has consumed, not the
    /// worker's generated count: the worker runs ahead to keep the queue full,
    /// so only the callback knows what is actually being heard. That position
    /// maps onto a row because the raster scans at a uniform rate.
    pub fn progress(&self) -> TxProgress {
        match self.running.as_ref() {
            Some(running) => running.snapshot.raster.progress_at(running.played_samples()),
            None if self.completed => TxProgress::Complete,
            None => TxProgress::Idle,
        }
    }

    /// Returns the physical playback rate, if a stream is open.
    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.running.as_ref()?.playback.as_ref().map(Playback::sample_rate_hz)
    }

    /// Puts a transmission in `phase` with no device or worker behind it.
    ///
    /// Tests have no device to send on, and what is asserted about a
    /// transmission is what the operator can do while it runs rather than its
    /// audio.
    #[cfg(test)]
    pub(crate) fn begin_for_test(&mut self, kind: Kind, phase: TxPhase) {
        self.completed = false;
        self.running = Some(Running {
            kind,
            playback: None,
            worker: None,
            snapshot: TxSnapshot {
                phase,
                ..TxSnapshot::default()
            },
            started: false,
            keyed: false,
        });
    }

    #[cfg(test)]
    pub(crate) fn key_for_test(&mut self) {
        if let Some(running) = self.running.as_mut() {
            running.keyed = true;
        }
    }

    #[cfg(test)]
    pub(crate) fn is_keyed(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.keyed)
    }
}

impl Running {
    fn advance(&mut self, rig: &RigSnapshot) -> Option<Ending> {
        match self.snapshot.phase {
            TxPhase::Failed => {
                let error = self.snapshot.error.as_ref().map_or_else(
                    || AppError::WorkerUnavailable("transmit").to_string(),
                    AppError::to_string,
                );
                return Some(Ending::Failed(error));
            }
            TxPhase::Cancelled => return Some(Ending::Cancelled),
            _ => {}
        }
        // A rig that refused one of the operator's commands has not been keyed,
        // so the transmission it was keyed for cannot go out.
        if self.keyed
            && let Some(error) = rig.error.as_ref()
        {
            return Some(Ending::Failed(error.to_string()));
        }
        let should_start = !self.started
            && self.ready_to_send(rig.state)
            && matches!(self.snapshot.phase, TxPhase::Producing | TxPhase::Draining);
        if should_start && let Err(error) = self.start_playback() {
            return Some(Ending::Failed(error.to_string()));
        }
        if self.has_underrun() {
            return Some(Ending::Failed(AppError::PlaybackUnderrun.to_string()));
        }
        if self.snapshot.phase == TxPhase::Draining && self.is_drained() {
            return Some(Ending::Complete);
        }
        None
    }

    /// Whether the audio may start.
    ///
    /// A transmission that keyed the rig waits for the rig to say it is
    /// transmitting, because the lead-in the rig needs to switch over is time
    /// anything sent into it is simply lost.
    const fn ready_to_send(&self, state: RigState) -> bool {
        !self.keyed || matches!(state, RigState::Transmitting)
    }

    fn start_playback(&mut self) -> Result<(), AppError> {
        self.playback.as_ref().ok_or(AppError::PlaybackClosed)?.play()?;
        self.started = true;
        Ok(())
    }

    /// Whether the device ran the queue dry while a picture was going out.
    fn has_underrun(&self) -> bool {
        self.started
            && self
                .playback
                .as_ref()
                .is_some_and(|playback| playback.underrun_samples() > 0)
    }

    /// Whether the queue was closed by the worker and played to its end.
    fn is_drained(&self) -> bool {
        self.playback.as_ref().is_some_and(Playback::is_complete)
    }

    fn played_samples(&self) -> u64 {
        self.playback.as_ref().map_or(0, Playback::played_samples)
    }
}

#[cfg(test)]
mod tests;
