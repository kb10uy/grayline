use std::{
    mem,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use grayline_rtty::{
    TxCode, TxConfig,
    tx::{Framer, Keying},
};

use crate::{clock::Clock, error::KeyerError, line::ControlLine};

/// The ticks per second a line is framed in: one tick is a microsecond.
const TICKS_PER_SECOND: u32 = 1_000_000;

/// The longest a wait goes without looking at whether it was cancelled.
const CANCEL_POLL: Duration = Duration::from_millis(20);

/// Whether keying went on to the end of what it was asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Flow {
    /// It did.
    Continue,
    /// The cancel flag was raised first.
    Cancelled,
}

/// How far a stream has gone out.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    /// The stream's own timeline up to the end of what was last keyed.
    pub elapsed: Duration,
    /// The latest any transition took effect behind its deadline.
    pub late: Duration,
}

/// A way of putting a code stream on the air.
///
/// Every call blocks until what it keyed has gone out, and gives up early
/// once `cancel` is raised.
pub trait Method {
    /// Where the method's own clock stands.
    fn now(&self) -> Duration;
    /// Starts a stream framed by `config`, through its lead-in.
    fn begin(&mut self, config: &TxConfig, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Keys one code.
    fn send(&mut self, code: TxCode, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Idles at mark for one bit while the stream waits for more codes.
    fn idle(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Ends the stream through its tail.
    fn finish(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Returns the output to mark at once and forgets the stream.
    fn rest(&mut self) -> Result<(), KeyerError>;
    /// How far the current or last stream has gone out.
    fn progress(&self) -> Progress;
}

/// Keying by switching a control line at each element boundary.
///
/// The keyer owns the timing: every boundary is a deadline measured from the
/// start of the stream, so a late transition does not push the ones after it.
pub struct LineMethod<L, C> {
    line: L,
    clock: C,
    mark_asserted: bool,
    asserted: Option<bool>,
    framer: Option<Framer>,
    elements: Vec<(Keying, f64)>,
    origin: Duration,
    position: f64,
    late: Duration,
}

impl<L: ControlLine, C: Clock> LineMethod<L, C> {
    /// Takes over `line` and puts it at mark.
    pub fn new(line: L, clock: C, mark_asserted: bool) -> Result<Self, KeyerError> {
        let mut method = Self {
            line,
            clock,
            mark_asserted,
            asserted: None,
            framer: None,
            elements: Vec::new(),
            origin: Duration::ZERO,
            position: 0.0,
            late: Duration::ZERO,
        };
        method.rest()?;
        Ok(method)
    }

    const fn level(&self, keying: Keying) -> bool {
        match keying {
            Keying::Space => !self.mark_asserted,
            Keying::Mark | Keying::Muted => self.mark_asserted,
        }
    }

    fn drive(&mut self, asserted: bool) -> Result<(), KeyerError> {
        if self.asserted != Some(asserted) {
            self.asserted = None;
            self.line.set(asserted)?;
            self.asserted = Some(asserted);
        }
        Ok(())
    }

    fn hold(&mut self, keying: Keying, ticks: f64, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        if cancel.load(Ordering::Acquire) {
            return Ok(Flow::Cancelled);
        }
        let start = self.origin + ticks_to_duration(self.position);
        self.drive(self.level(keying))?;
        self.late = self.late.max(self.clock.now().saturating_sub(start));
        self.position += ticks;
        let end = self.origin + ticks_to_duration(self.position);
        loop {
            let now = self.clock.now();
            if now >= end {
                return Ok(Flow::Continue);
            }
            if cancel.load(Ordering::Acquire) {
                return Ok(Flow::Cancelled);
            }
            self.clock.sleep_until(end.min(now + CANCEL_POLL));
        }
    }

    fn play(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        let mut elements = mem::take(&mut self.elements);
        let mut flow = Ok(Flow::Continue);
        for (keying, ticks) in elements.drain(..) {
            flow = self.hold(keying, ticks, cancel);
            if flow != Ok(Flow::Continue) {
                break;
            }
        }
        elements.clear();
        self.elements = elements;
        flow
    }
}

impl<L: ControlLine, C: Clock> Method for LineMethod<L, C> {
    fn now(&self) -> Duration {
        self.clock.now()
    }

    fn begin(&mut self, config: &TxConfig, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        let framer = Framer::new(TICKS_PER_SECOND, config).map_err(|error| KeyerError::Framing(error.to_string()))?;
        let lead_in = framer.lead_in_ticks();
        self.framer = Some(framer);
        self.origin = self.clock.now();
        self.position = 0.0;
        self.late = Duration::ZERO;
        self.drive(self.mark_asserted)?;
        if lead_in > 0.0 {
            self.hold(Keying::Mark, lead_in, cancel)
        } else {
            Ok(Flow::Continue)
        }
    }

    fn send(&mut self, code: TxCode, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        let Some(framer) = self.framer.as_mut() else {
            return Ok(Flow::Cancelled);
        };
        framer.frame(code, &mut self.elements);
        self.play(cancel)
    }

    fn idle(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        let Some(bit) = self.framer.as_ref().map(Framer::ticks_per_bit) else {
            return Ok(Flow::Cancelled);
        };
        self.hold(Keying::Mark, bit, cancel)
    }

    fn finish(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        let Some(tail) = self.framer.as_ref().map(Framer::tail_ticks) else {
            return Ok(Flow::Cancelled);
        };
        if tail > 0.0 {
            self.hold(Keying::Mark, tail, cancel)
        } else {
            Ok(Flow::Continue)
        }
    }

    fn rest(&mut self) -> Result<(), KeyerError> {
        self.framer = None;
        self.elements.clear();
        self.asserted = None;
        self.drive(self.mark_asserted)
    }

    fn progress(&self) -> Progress {
        Progress {
            elapsed: ticks_to_duration(self.position),
            late: self.late,
        }
    }
}

fn ticks_to_duration(ticks: f64) -> Duration {
    Duration::from_nanos((ticks * 1_000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use grayline_rtty::BaudRate;
    use rstest::rstest;

    use super::*;
    use crate::test_util::{FakeClock, RecordingLine};

    const MS: Duration = Duration::from_millis(1);

    fn config() -> TxConfig {
        let mut config = TxConfig {
            lead_in_seconds: 0.1,
            tail_seconds: 0.1,
            ..TxConfig::default()
        };
        config.framing.baud = BaudRate::new(50.0).unwrap();
        config
    }

    fn key_e(latency: Duration, mark_asserted: bool) -> (Vec<(Duration, bool)>, Progress) {
        let clock = FakeClock::default();
        let line = RecordingLine::new(clock.clone(), latency);
        let mut method = LineMethod::new(line.clone(), clock, mark_asserted).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(method.begin(&config(), &cancel).unwrap(), Flow::Continue);
        assert_eq!(
            method.send(TxCode::Character(0b10000), &cancel).unwrap(),
            Flow::Continue
        );
        assert_eq!(method.finish(&cancel).unwrap(), Flow::Continue);
        let progress = method.progress();
        method.rest().unwrap();
        (line.changes(), progress)
    }

    #[test]
    fn each_boundary_is_switched_at_its_deadline() {
        let (changes, progress) = key_e(Duration::ZERO, false);
        assert_eq!(
            changes,
            vec![
                (Duration::ZERO, false),
                (100 * MS, true),
                (120 * MS, false),
                (140 * MS, true),
                (220 * MS, false),
                (350 * MS, false),
            ]
        );
        assert_eq!(progress.elapsed, 350 * MS);
        assert_eq!(progress.late, Duration::ZERO);
    }

    #[test]
    fn a_slow_line_is_reported_late_without_pushing_later_deadlines() {
        let (changes, progress) = key_e(MS, false);
        let times: Vec<Duration> = changes.iter().map(|(time, _)| *time).collect();
        assert_eq!(times[1..5], [102 * MS, 122 * MS, 142 * MS, 222 * MS]);
        assert_eq!(progress.late, MS);
    }

    #[rstest]
    #[case(false)]
    #[case(true)]
    fn mark_is_whichever_level_the_station_wires_it_as(#[case] mark_asserted: bool) {
        let (changes, _) = key_e(Duration::ZERO, mark_asserted);
        assert_eq!(changes[0].1, mark_asserted);
        assert_eq!(changes[1].1, !mark_asserted);
        assert_eq!(changes.last().unwrap().1, mark_asserted);
    }

    #[test]
    fn a_muted_element_holds_mark_on_a_line() {
        let clock = FakeClock::default();
        let line = RecordingLine::new(clock.clone(), Duration::ZERO);
        let mut method = LineMethod::new(line.clone(), clock, false).unwrap();
        let cancel = AtomicBool::new(false);
        method.begin(&config(), &cancel).unwrap();
        method.send(TxCode::CarrierOff, &cancel).unwrap();
        assert_eq!(line.levels(), vec![false]);
        assert_eq!(method.progress().elapsed, 160 * MS);
    }

    #[test]
    fn a_raised_cancel_stops_before_the_next_element() {
        let clock = FakeClock::default();
        let line = RecordingLine::new(clock.clone(), Duration::ZERO);
        let mut method = LineMethod::new(line.clone(), clock, false).unwrap();
        let cancel = AtomicBool::new(false);
        method.begin(&config(), &cancel).unwrap();
        cancel.store(true, Ordering::Release);
        assert_eq!(method.send(TxCode::Character(0), &cancel).unwrap(), Flow::Cancelled);
        assert_eq!(line.levels(), vec![false]);
    }

    #[test]
    fn idling_holds_mark_for_one_bit() {
        let clock = FakeClock::default();
        let line = RecordingLine::new(clock.clone(), Duration::ZERO);
        let mut method = LineMethod::new(line, clock, false).unwrap();
        let cancel = AtomicBool::new(false);
        method.begin(&config(), &cancel).unwrap();
        method.idle(&cancel).unwrap();
        assert_eq!(method.progress().elapsed, 120 * MS);
    }
}
