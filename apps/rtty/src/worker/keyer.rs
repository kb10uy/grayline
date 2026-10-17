//! The thread that streams one message to `gl-keyer`, in place of the audio.
//!
//! The keyer times every bit itself, so what crosses the socket is the whole
//! message ahead of time and then a status poll. What it reports back is the
//! stream's own timeline in microseconds, which is what a playback position
//! is for the audio path: the same schedule reads either.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use grayline_keyer_protocol::{KeyerClient, StreamState};
use grayline_rig_control::RigError;
use grayline_rtty::{TxCode, TxConfig};

use crate::{
    error::AppError,
    worker::{transmit::TxPhase, update},
};

/// The rate a keyer's timeline counts in: one tick is a microsecond.
pub const KEYER_TICKS_PER_SECOND: u32 = 1_000_000;

/// How long a command to the keyer may take before it is called broken.
const TIMEOUT: Duration = Duration::from_millis(1_500);

/// How often the keyer is asked how far it has got.
///
/// Short beside a character, which is about 160 ms at 45.45 baud, so the echo
/// follows the line about as closely as it follows the audio.
const STATUS_INTERVAL: Duration = Duration::from_millis(20);

/// How often a stream waiting for the rig looks again.
const START_POLL: Duration = Duration::from_millis(5);

#[derive(Clone, Debug, Default)]
pub struct KeyerSnapshot {
    pub phase: TxPhase,
    /// How far the stream has gone out on the keyer's own timeline.
    pub elapsed: Duration,
    /// The latest any transition took effect behind its deadline.
    pub late: Duration,
    /// Whether the keyer reported the stream as having gone out whole.
    pub completed: bool,
    pub error: Option<AppError>,
}

/// One message being keyed by `gl-keyer`.
///
/// The connection is made at once, so a keyer that is not running is reported
/// while the rig is still switching over; the stream is opened only once the
/// interface says the rig is keyed, because the keyer starts keying the moment
/// a stream opens.
pub struct KeyerWorker {
    snapshot: Arc<Mutex<KeyerSnapshot>>,
    start: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl KeyerWorker {
    pub fn spawn(address: String, codes: Vec<TxCode>, config: TxConfig) -> Self {
        let snapshot = Arc::new(Mutex::new(KeyerSnapshot {
            phase: TxPhase::Priming,
            ..KeyerSnapshot::default()
        }));
        let start = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let signals = Signals {
            snapshot: Arc::clone(&snapshot),
            start: Arc::clone(&start),
            cancel: Arc::clone(&cancel),
        };
        let thread = thread::Builder::new()
            .name("grayline-rtty-keyer".to_owned())
            .spawn(move || stream_loop(&address, &codes, &config, &signals))
            .ok();
        if thread.is_none() {
            update(&snapshot, |state| {
                state.phase = TxPhase::Failed;
                state.error = Some(AppError::WorkerUnavailable("keyer"));
            });
        }
        Self {
            snapshot,
            start,
            cancel,
            thread,
        }
    }

    pub fn latest(&self) -> KeyerSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| KeyerSnapshot {
                phase: TxPhase::Failed,
                error: Some(AppError::WorkerUnavailable("keyer")),
                ..KeyerSnapshot::default()
            })
    }

    /// Opens the stream, now that the rig is keyed.
    pub fn start(&self) {
        self.start.store(true, Ordering::Release);
    }

    /// Stops the stream, leaving the line at mark.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Drop for KeyerWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for KeyerWorker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("KeyerWorker")
            .field("snapshot", &self.latest())
            .finish_non_exhaustive()
    }
}

struct Signals {
    snapshot: Arc<Mutex<KeyerSnapshot>>,
    start: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Signals {
    fn cancelled(&self) -> bool {
        if self.cancel.load(Ordering::Acquire) {
            update(&self.snapshot, |state| state.phase = TxPhase::Cancelled);
            return true;
        }
        false
    }

    fn fail(&self, error: AppError) {
        update(&self.snapshot, |state| {
            state.phase = TxPhase::Failed;
            state.error = Some(error);
        });
    }
}

fn stream_loop(address: &str, codes: &[TxCode], config: &TxConfig, signals: &Signals) {
    let mut client = match KeyerClient::connect(address, TIMEOUT) {
        Ok(client) => client,
        Err(error) => return signals.fail(AppError::Keyer(error)),
    };
    update(&signals.snapshot, |state| state.phase = TxPhase::Producing);
    while !signals.start.load(Ordering::Acquire) {
        if signals.cancelled() {
            return;
        }
        thread::sleep(START_POLL);
    }
    // The whole message goes before the end is marked, so the stream never
    // runs dry: a keyer with nothing queued idles at mark, which would put the
    // line behind the schedule the echo is read from.
    let sent: Result<(), RigError> = client
        .open(config)
        .and_then(|()| client.send(codes))
        .and_then(|()| client.end());
    if let Err(error) = sent {
        let _ = client.stop();
        return signals.fail(AppError::Keyer(error));
    }
    update(&signals.snapshot, |state| state.phase = TxPhase::Draining);
    loop {
        if signals.cancelled() {
            let _ = client.stop();
            return;
        }
        let status = match client.status() {
            Ok(status) => status,
            Err(error) => return signals.fail(AppError::Keyer(error)),
        };
        update(&signals.snapshot, |state| {
            state.elapsed = status.elapsed;
            state.late = status.late;
        });
        match status.state {
            StreamState::Completed => {
                update(&signals.snapshot, |state| state.completed = true);
                return;
            }
            state if state.is_cut_short() => {
                let detail = status.error.unwrap_or_else(|| state.name().to_owned());
                return signals.fail(AppError::KeyerStopped(detail));
            }
            _ => {}
        }
        thread::sleep(STATUS_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use std::{net::TcpListener, time::Instant};

    use grayline_rtty::encode_text;

    use super::*;
    use crate::test_util::FakeKeyer;

    const DEADLINE: Duration = Duration::from_secs(10);

    fn settle(worker: &KeyerWorker, predicate: impl Fn(&KeyerSnapshot) -> bool) -> KeyerSnapshot {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let snapshot = worker.latest();
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "the keyer settled at {snapshot:?}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn spawn(fake: &FakeKeyer, text: &str) -> KeyerWorker {
        let config = TxConfig::default();
        KeyerWorker::spawn(fake.address.clone(), encode_text(text, &config).unwrap(), config)
    }

    #[test]
    fn nothing_is_opened_until_the_rig_is_keyed() {
        let fake = FakeKeyer::spawn(&[("completed", 0)]);
        let worker = spawn(&fake, "RY");

        settle(&worker, |snapshot| snapshot.phase == TxPhase::Producing);
        thread::sleep(Duration::from_millis(30));

        assert_eq!(fake.received(), ["+\\chk_vfo"]);
    }

    #[test]
    fn a_started_stream_is_sent_whole_and_followed_to_its_end() {
        let fake = FakeKeyer::spawn(&[("sending", 400_000), ("sending", 900_000), ("completed", 1_500_000)]);
        let worker = spawn(&fake, "RY");

        settle(&worker, |snapshot| snapshot.phase == TxPhase::Producing);
        worker.start();
        let finished = settle(&worker, |snapshot| snapshot.completed);

        assert_eq!(finished.phase, TxPhase::Draining);
        assert_eq!(finished.elapsed, Duration::from_millis(1_500));
        assert_eq!(finished.late, Duration::from_micros(40));
        let received = fake.received();
        assert!(received[1].starts_with("+\\rtty_open baud=45.45 "), "{received:?}");
        assert!(received[2].starts_with("+\\rtty_send "), "{received:?}");
        assert_eq!(received[3], "+\\rtty_end");
        assert!(received[4..].iter().all(|line| line == "+\\status"), "{received:?}");
    }

    #[test]
    fn a_stream_the_keyer_cut_short_is_a_failure() {
        let fake = FakeKeyer::spawn(&[("sending", 0), ("timeout", 600_000)]);
        let worker = spawn(&fake, "RY");
        worker.start();

        let failed = settle(&worker, |snapshot| snapshot.phase == TxPhase::Failed);

        assert!(
            matches!(&failed.error, Some(AppError::KeyerStopped(detail)) if detail == "timeout"),
            "{:?}",
            failed.error
        );
    }

    #[test]
    fn a_cancelled_stream_is_stopped_at_the_keyer() {
        let fake = FakeKeyer::spawn(&[("sending", 0)]);
        let worker = spawn(&fake, "RY");
        worker.start();
        settle(&worker, |snapshot| snapshot.phase == TxPhase::Draining);

        worker.cancel();
        settle(&worker, |snapshot| snapshot.phase == TxPhase::Cancelled);
        drop(worker);

        assert_eq!(fake.received().last().map(String::as_str), Some("+\\stop"));
    }

    #[test]
    fn a_keyer_that_is_not_running_is_reported_before_anything_is_keyed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        let config = TxConfig::default();
        let worker = KeyerWorker::spawn(address, encode_text("RY", &config).unwrap(), config);

        let failed = settle(&worker, |snapshot| snapshot.phase == TxPhase::Failed);

        assert!(matches!(failed.error, Some(AppError::Keyer(_))), "{:?}", failed.error);
    }
}
