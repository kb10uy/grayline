use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{clock::Clock, error::KeyerError, line::ControlLine};

#[derive(Clone, Debug, Default)]
pub struct FakeClock(Arc<Mutex<Duration>>);

impl FakeClock {
    pub fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }

    fn sleep_until(&self, deadline: Duration) {
        let mut now = self.0.lock().unwrap();
        *now = (*now).max(deadline);
    }
}

#[derive(Clone, Debug, Default)]
pub struct RecordingLine {
    clock: FakeClock,
    latency: Duration,
    changes: Arc<Mutex<Vec<(Duration, bool)>>>,
}

impl RecordingLine {
    pub fn new(clock: FakeClock, latency: Duration) -> Self {
        Self {
            clock,
            latency,
            changes: Arc::default(),
        }
    }

    pub fn changes(&self) -> Vec<(Duration, bool)> {
        self.changes.lock().unwrap().clone()
    }

    pub fn levels(&self) -> Vec<bool> {
        let mut levels: Vec<bool> = self.changes().into_iter().map(|(_, level)| level).collect();
        levels.dedup();
        levels
    }
}

impl ControlLine for RecordingLine {
    fn set(&mut self, asserted: bool) -> Result<(), KeyerError> {
        self.clock.advance(self.latency);
        self.changes.lock().unwrap().push((self.clock.now(), asserted));
        Ok(())
    }
}
