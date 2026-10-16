use std::{
    sync::{
        Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::{
    Logger,
    method::{Method, Progress},
    session::{Session, StopReason},
    stream::{CodeSource, Next, key_stream},
};

/// How long an idle keyer sleeps before looking at the time limit again.
const IDLE_WAKE: Duration = Duration::from_millis(100);

/// What the connections and the keying thread share.
#[derive(Debug, Default)]
pub struct Shared {
    session: Mutex<Session>,
    wake: Condvar,
    cancel: AtomicBool,
    shutdown: AtomicBool,
}

impl Shared {
    /// A fresh session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `change` on the session and wakes the keying thread.
    pub fn update<R>(&self, change: impl FnOnce(&mut Session) -> R) -> R {
        let result = change(&mut self.lock());
        self.wake.notify_all();
        result
    }

    /// Stops the stream being keyed at its next element.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    /// Asks the keying thread to stop and leave the line at rest.
    pub fn shut_down(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.cancel();
    }

    fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Keys whatever the session asks for until the keyer is shut down.
///
/// `limit` is the longest one stream may run; nothing is cut off without one.
pub fn run<M: Method>(method: &mut M, shared: &Shared, limit: Option<Duration>, log: Logger) {
    let mut session = shared.lock();
    while !shared.shutdown.load(Ordering::Acquire) {
        if session.expire(method.now(), limit) {
            shared.cancel.store(true, Ordering::Release);
        }
        let Some(config) = session.take_started() else {
            session = shared
                .wake
                .wait_timeout(session, IDLE_WAKE)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(session, _)| session);
            continue;
        };
        shared.cancel.store(false, Ordering::Release);
        drop(session);
        let mut source = SessionSource { shared, limit };
        let outcome = key_stream(method, &config, &mut source, &shared.cancel);
        if let Err(error) = &outcome {
            let message = error.to_string();
            log(&message);
            shared.update(|session| session.fail(message));
        }
        shared.update(|session| session.finish(outcome.ok()));
        session = shared.lock();
    }
    session.stop(StopReason::ShutDown);
    drop(session);
    if let Err(error) = method.rest() {
        log(&error.to_string());
    }
}

struct SessionSource<'a> {
    shared: &'a Shared,
    limit: Option<Duration>,
}

impl CodeSource for SessionSource<'_> {
    fn next(&mut self, now: Duration) -> Next {
        self.shared.update(|session| {
            if session.expire(now, self.limit) {
                self.shared.cancel.store(true, Ordering::Release);
            }
            session.take_next()
        })
    }

    fn progress(&mut self, sent: usize, progress: Progress) {
        self.shared.update(|session| session.record(sent, progress));
    }
}
