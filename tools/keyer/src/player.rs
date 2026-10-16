use std::{
    sync::{
        Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::{
    Logger,
    error::KeyerError,
    line::ControlLine,
    method::{Method, Progress},
    session::{ConnectionId, Session, StopReason},
    stream::{CodeSource, Next, key_stream},
};

/// How long an idle keyer sleeps before looking at the time limit again.
const IDLE_WAKE: Duration = Duration::from_millis(100);

/// What the connections and the keying thread share.
#[derive(Debug)]
pub struct Shared {
    session: Mutex<Session>,
    wake: Condvar,
    cancel: AtomicBool,
    shutdown: AtomicBool,
    has_ptt: bool,
}

impl Shared {
    /// A fresh session, with or without a PTT line to offer.
    pub fn new(has_ptt: bool) -> Self {
        Self {
            session: Mutex::new(Session::default()),
            wake: Condvar::new(),
            cancel: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            has_ptt,
        }
    }

    /// Whether there is a PTT line to key.
    pub const fn has_ptt(&self) -> bool {
        self.has_ptt
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

    /// Asks the keying thread to stop and leave the lines at rest.
    pub fn shut_down(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.cancel();
    }

    fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The PTT line, if there is one, and what it was last set to.
pub struct PttLine<P> {
    line: Option<P>,
    applied: bool,
}

impl<P: ControlLine> PttLine<P> {
    /// Takes over `line` and releases it.
    pub fn new(line: Option<P>) -> Result<Self, KeyerError> {
        let mut ptt = Self { line, applied: true };
        ptt.apply(false)?;
        Ok(ptt)
    }

    /// Sets the line to `keyed`, if it is not already.
    ///
    /// A failed write still counts as applied: retrying it on every pass
    /// would spin, and the failure is reported instead.
    pub fn apply(&mut self, keyed: bool) -> Result<(), KeyerError> {
        if keyed == self.applied {
            return Ok(());
        }
        self.applied = keyed;
        match self.line.as_mut() {
            Some(line) => line.set(keyed),
            None => Ok(()),
        }
    }

    const fn applied(&self) -> bool {
        self.applied
    }
}

/// Keys whatever the session asks for until the keyer is shut down.
///
/// `limit` is the longest the rig may stay keyed; nothing is cut off without
/// one.
pub fn run<M: Method, P: ControlLine>(
    method: &mut M,
    ptt: &mut PttLine<P>,
    shared: &Shared,
    limit: Option<Duration>,
    log: Logger,
) {
    let mut session = shared.lock();
    while !shared.shutdown.load(Ordering::Acquire) {
        if session.expire(method.now(), limit) {
            shared.cancel.store(true, Ordering::Release);
        }
        let keyed = session.ptt();
        let started = session.take_started();
        if started.is_some() {
            shared.cancel.store(false, Ordering::Release);
        }
        if started.is_none() && keyed == ptt.applied() {
            session = shared
                .wake
                .wait_timeout(session, IDLE_WAKE)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(session, _)| session);
            continue;
        }
        drop(session);
        if let Err(error) = ptt.apply(keyed) {
            report(shared, log, &error);
        }
        if let Some(config) = started {
            let mut source = SessionSource {
                shared,
                ptt: &mut *ptt,
                limit,
                log,
            };
            let outcome = key_stream(method, &config, &mut source, &shared.cancel);
            if let Err(error) = &outcome {
                report(shared, log, error);
            }
            shared.update(|session| session.finish(outcome.ok()));
        }
        session = shared.lock();
    }
    session.stop(StopReason::ShutDown);
    session.set_ptt(ConnectionId(0), false);
    drop(session);
    if let Err(error) = ptt.apply(false) {
        log(&error.to_string());
    }
    if let Err(error) = method.rest() {
        log(&error.to_string());
    }
}

fn report(shared: &Shared, log: Logger, error: &KeyerError) {
    let message = error.to_string();
    log(&message);
    shared.update(|session| session.fail(message));
}

struct SessionSource<'a, P> {
    shared: &'a Shared,
    ptt: &'a mut PttLine<P>,
    limit: Option<Duration>,
    log: Logger,
}

impl<P: ControlLine> CodeSource for SessionSource<'_, P> {
    fn next(&mut self, now: Duration) -> Next {
        let (next, keyed) = self.shared.update(|session| {
            if session.expire(now, self.limit) {
                self.shared.cancel.store(true, Ordering::Release);
            }
            (session.take_next(), session.ptt())
        });
        if let Err(error) = self.ptt.apply(keyed) {
            report(self.shared, self.log, &error);
        }
        next
    }

    fn progress(&mut self, sent: usize, progress: Progress) {
        self.shared.update(|session| session.record(sent, progress));
    }
}
