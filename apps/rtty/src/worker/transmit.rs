//! The thread that keys one message, and the states it passes through.
//!
//! Transmission is buffered rather than live: the operator writes a message
//! and presses send, so the transmitter's own contract — the end of the code
//! stream is the end of the transmission — is the one this runs on.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use grayline_audio::PlaybackWriter;
use grayline_rtty::{Transmitter, TxCode, TxConfig};

use crate::{error::AppError, worker::update};

const PCM_BLOCK_SIZE: usize = 1_024;

const BACK_OFF: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxPhase {
    #[default]
    Idle,
    /// Filling the queue before the device is told to start.
    Priming,
    Producing,
    /// Everything has been generated; the device is playing out what is left.
    Draining,
    Cancelled,
    Failed,
}

impl TxPhase {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Priming | Self::Producing | Self::Draining)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TxSnapshot {
    pub phase: TxPhase,
    pub generated_samples: u64,
    pub error: Option<AppError>,
}

/// One message being keyed.
pub struct TxWorker {
    snapshot: Arc<Mutex<TxSnapshot>>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TxWorker {
    /// Keys `codes` into `writer` and closes the queue when they run out.
    pub fn spawn(writer: PlaybackWriter, codes: Vec<TxCode>, config: TxConfig) -> Self {
        Self::start("grayline-rtty-transmit", move |snapshot, cancel| {
            transmit_loop(writer, codes, config, &snapshot, &cancel);
        })
    }

    fn start(name: &str, body: impl FnOnce(Arc<Mutex<TxSnapshot>>, Arc<AtomicBool>) + Send + 'static) -> Self {
        let snapshot = Arc::new(Mutex::new(TxSnapshot {
            phase: TxPhase::Priming,
            ..TxSnapshot::default()
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_cancel = Arc::clone(&cancel);
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || body(worker_snapshot, worker_cancel))
            .ok();
        if thread.is_none()
            && let Ok(mut current) = snapshot.lock()
        {
            *current = TxSnapshot {
                phase: TxPhase::Failed,
                error: Some(AppError::WorkerUnavailable("transmit")),
                ..TxSnapshot::default()
            };
        }
        Self {
            snapshot,
            cancel,
            thread,
        }
    }

    pub fn latest(&self) -> TxSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| TxSnapshot {
                phase: TxPhase::Failed,
                error: Some(AppError::WorkerUnavailable("transmit")),
                ..TxSnapshot::default()
            })
    }

    /// Stops keying without waiting for the thread to notice.
    ///
    /// What is already in the playback queue still plays; cutting that off is
    /// the caller's business, because it owns the device.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Drop for TxWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for TxWorker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TxWorker")
            .field("snapshot", &self.latest())
            .finish_non_exhaustive()
    }
}

fn transmit_loop(
    mut writer: PlaybackWriter,
    codes: Vec<TxCode>,
    config: TxConfig,
    snapshot: &Mutex<TxSnapshot>,
    cancel: &AtomicBool,
) {
    let sample_rate_hz = writer.sample_rate_hz();
    let mut transmitter = match Transmitter::new(codes.into_iter(), sample_rate_hz, config) {
        Ok(transmitter) => transmitter,
        Err(error) => return fail(snapshot, error),
    };
    // A quarter of a second in the queue before the device starts, so the
    // first mark of the lead-in is not the one that underruns.
    let prefill = writer.vacant().min((sample_rate_hz as usize / 4).max(1));
    let mut block = [0.0_f32; PCM_BLOCK_SIZE];
    let mut count = 0;
    let mut offset = 0;
    let mut primed = false;
    loop {
        if cancel.load(Ordering::Acquire) || writer.is_cancelled() {
            update(snapshot, |state| state.phase = TxPhase::Cancelled);
            return;
        }
        if offset == count {
            match transmitter.process(&mut block) {
                Ok(0) => {
                    writer.finish();
                    update(snapshot, |state| state.phase = TxPhase::Draining);
                    return;
                }
                Ok(next) => {
                    count = next;
                    offset = 0;
                }
                Err(error) => return fail(snapshot, error),
            }
        }
        let written = writer.write(&block[offset..count]);
        if written == 0 {
            thread::sleep(BACK_OFF);
            continue;
        }
        offset += written;
        update(snapshot, |state| {
            state.generated_samples += written as u64;
            if !primed && state.generated_samples >= prefill as u64 {
                state.phase = TxPhase::Producing;
                primed = true;
            }
        });
    }
}

fn fail(snapshot: &Mutex<TxSnapshot>, error: impl Into<AppError>) {
    update(snapshot, |state| {
        state.phase = TxPhase::Failed;
        state.error = Some(error.into());
    });
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use grayline_audio::synthetic_playback;
    use grayline_rtty::{ReceivePipeline, RxConfig, RxEvent, encode_text};

    use super::*;

    const RATE: u32 = 8_000;
    const DEADLINE: Duration = Duration::from_secs(10);

    fn config() -> TxConfig {
        TxConfig::default()
    }

    fn keyed(worker: &TxWorker, reader: &mut grayline_audio::PlaybackReader) -> Vec<f32> {
        let deadline = Instant::now() + DEADLINE;
        let mut samples = Vec::new();
        let mut block = [0.0_f32; 512];
        loop {
            let snapshot = worker.latest();
            assert!(snapshot.phase != TxPhase::Failed, "{:?}", snapshot.error);
            assert!(Instant::now() < deadline, "the transmission did not finish");
            let read = reader.read(&mut block);
            samples.extend_from_slice(&block[..read]);
            if snapshot.phase == TxPhase::Draining && reader.is_complete() {
                return samples;
            }
            thread::yield_now();
        }
    }

    #[test]
    fn a_transmitted_message_decodes_as_the_text_it_was_given() {
        let text = "CQ CQ DE JL1HIS";
        let (writer, mut reader) = synthetic_playback(RATE, 4_096).unwrap();
        let worker = TxWorker::spawn(writer, encode_text(text, &config()).unwrap(), config());

        let samples = keyed(&worker, &mut reader);

        let mut pipeline = ReceivePipeline::new(RATE, RxConfig::default()).unwrap();
        let mut printed = String::new();
        pipeline
            .process(&samples, |event| {
                if let RxEvent::Character { character, .. } = event {
                    printed.push(*character);
                }
            })
            .unwrap();
        assert!(printed.contains(text), "received {printed:?}");
    }

    #[test]
    fn a_finished_transmission_generated_the_whole_of_it() {
        let (writer, mut reader) = synthetic_playback(RATE, 4_096).unwrap();
        let worker = TxWorker::spawn(writer, encode_text("RY", &config()).unwrap(), config());

        let samples = keyed(&worker, &mut reader);

        let snapshot = worker.latest();
        assert_eq!(snapshot.phase, TxPhase::Draining);
        assert_eq!(snapshot.generated_samples, samples.len() as u64);
    }

    #[test]
    fn a_cancelled_transmission_stops_generating() {
        let (writer, mut reader) = synthetic_playback(RATE, 1_024).unwrap();
        let long = "RY".repeat(200);
        let worker = TxWorker::spawn(writer, encode_text(&long, &config()).unwrap(), config());

        let deadline = Instant::now() + DEADLINE;
        let mut block = [0.0_f32; 256];
        while worker.latest().phase != TxPhase::Producing {
            assert!(Instant::now() < deadline, "the transmission never started");
            reader.read(&mut block);
            thread::yield_now();
        }
        worker.cancel();

        while worker.latest().phase == TxPhase::Producing {
            assert!(Instant::now() < deadline, "the transmission did not stop");
            reader.read(&mut block);
            thread::yield_now();
        }
        let stopped = worker.latest();
        assert_eq!(stopped.phase, TxPhase::Cancelled);
        let after = worker.latest().generated_samples;
        assert_eq!(stopped.generated_samples, after);
    }

    #[test]
    fn a_transmitter_that_cannot_be_built_reports_why() {
        let (writer, _reader) = synthetic_playback(RATE, 1_024).unwrap();
        let refused = TxConfig {
            amplitude: 0.0,
            ..config()
        };
        let worker = TxWorker::spawn(writer, Vec::new(), refused);

        let deadline = Instant::now() + DEADLINE;
        while worker.latest().phase != TxPhase::Failed {
            assert!(Instant::now() < deadline, "the failure was never reported");
            thread::yield_now();
        }
        assert!(worker.latest().error.is_some());
    }

    #[test]
    fn only_a_running_transmission_is_active() {
        assert!(TxPhase::Priming.is_active());
        assert!(TxPhase::Producing.is_active());
        assert!(TxPhase::Draining.is_active());
        for phase in [TxPhase::Idle, TxPhase::Cancelled, TxPhase::Failed] {
            assert!(!phase.is_active());
        }
    }
}
