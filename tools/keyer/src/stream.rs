use std::{collections::VecDeque, sync::atomic::AtomicBool, time::Duration};

use grayline_rtty::{TxCode, TxConfig};

use crate::{
    error::KeyerError,
    method::{Flow, Method, Progress},
};

/// What a stream asks for next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Next {
    /// Key this code.
    Code(TxCode),
    /// Nothing yet, and the stream is not over.
    Wait,
    /// Everything has been keyed; send the tail.
    End,
    /// Give up now.
    Stop,
}

/// How a stream came to an end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Every code and the tail went out.
    Completed,
    /// It was cut short.
    Stopped,
}

/// Where a stream's codes come from, and where its progress goes.
pub trait CodeSource {
    /// The next thing to key, decided at `now` on the method's clock.
    fn next(&mut self, now: Duration) -> Next;
    /// Records that `sent` codes and `progress` have gone out.
    fn progress(&mut self, sent: usize, progress: Progress);
}

/// Keys one stream from `source` through `method`, leaving the output at mark.
pub fn key_stream<M: Method, S: CodeSource>(
    method: &mut M,
    config: &TxConfig,
    source: &mut S,
    cancel: &AtomicBool,
) -> Result<Outcome, KeyerError> {
    let outcome = run(method, config, source, cancel);
    let rested = method.rest();
    let outcome = outcome?;
    rested?;
    Ok(outcome)
}

fn run<M: Method, S: CodeSource>(
    method: &mut M,
    config: &TxConfig,
    source: &mut S,
    cancel: &AtomicBool,
) -> Result<Outcome, KeyerError> {
    if method.begin(config, cancel)? == Flow::Cancelled {
        return Ok(Outcome::Stopped);
    }
    let mut sent = 0;
    loop {
        let flow = match source.next(method.now()) {
            Next::Code(code) => {
                let flow = method.send(code, cancel)?;
                if flow == Flow::Continue {
                    sent += 1;
                }
                flow
            }
            Next::Wait => method.idle(cancel)?,
            Next::End => {
                let flow = method.finish(cancel)?;
                source.progress(sent, method.progress());
                return Ok(match flow {
                    Flow::Continue => Outcome::Completed,
                    Flow::Cancelled => Outcome::Stopped,
                });
            }
            Next::Stop => return Ok(Outcome::Stopped),
        };
        source.progress(sent, method.progress());
        if flow == Flow::Cancelled {
            return Ok(Outcome::Stopped);
        }
    }
}

/// A stream whose codes are all known before it starts.
#[derive(Debug, Default)]
pub struct FixedSource {
    codes: VecDeque<TxCode>,
    sent: usize,
    progress: Progress,
}

impl FixedSource {
    /// Holds `codes` to be keyed in order.
    pub fn new(codes: impl IntoIterator<Item = TxCode>) -> Self {
        Self {
            codes: codes.into_iter().collect(),
            ..Self::default()
        }
    }

    /// How many codes have gone out.
    pub const fn sent(&self) -> usize {
        self.sent
    }

    /// How far the stream has gone out.
    pub const fn last_progress(&self) -> Progress {
        self.progress
    }
}

impl CodeSource for FixedSource {
    fn next(&mut self, _now: Duration) -> Next {
        self.codes.pop_front().map_or(Next::End, Next::Code)
    }

    fn progress(&mut self, sent: usize, progress: Progress) {
        self.sent = sent;
        self.progress = progress;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::{
        method::LineMethod,
        test_util::{FakeClock, RecordingLine},
    };

    struct CancellingSource<'a> {
        inner: FixedSource,
        cancel: &'a AtomicBool,
        after: usize,
    }

    impl CodeSource for CancellingSource<'_> {
        fn next(&mut self, now: Duration) -> Next {
            if self.inner.sent() >= self.after {
                self.cancel.store(true, Ordering::Release);
            }
            self.inner.next(now)
        }

        fn progress(&mut self, sent: usize, progress: Progress) {
            self.inner.progress(sent, progress);
        }
    }

    fn method() -> (LineMethod<RecordingLine, FakeClock>, RecordingLine) {
        let clock = FakeClock::default();
        let line = RecordingLine::new(clock.clone(), Duration::ZERO);
        (LineMethod::new(line.clone(), clock, false).unwrap(), line)
    }

    #[test]
    fn a_fixed_stream_completes_and_counts_every_code() {
        let (mut method, line) = method();
        let mut source = FixedSource::new([TxCode::Character(0b11111), TxCode::Character(0)]);
        let cancel = AtomicBool::new(false);
        let outcome = key_stream(&mut method, &TxConfig::default(), &mut source, &cancel).unwrap();
        assert_eq!(outcome, Outcome::Completed);
        assert_eq!(source.sent(), 2);
        assert_eq!(line.levels(), vec![false, true, false, true, false]);
        let expected = 0.5 + 0.5 + 2.0 * 7.5 / 45.45;
        assert!((source.last_progress().elapsed.as_secs_f64() - expected).abs() < 1.0e-6);
    }

    #[test]
    fn a_cancelled_stream_stops_and_leaves_the_line_at_mark() {
        let (mut method, line) = method();
        let cancel = AtomicBool::new(false);
        let mut source = CancellingSource {
            inner: FixedSource::new([TxCode::Character(0); 4]),
            cancel: &cancel,
            after: 1,
        };
        let outcome = key_stream(&mut method, &TxConfig::default(), &mut source, &cancel).unwrap();
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(source.inner.sent(), 1);
        assert_eq!(line.changes().last().map(|(_, level)| *level), Some(false));
    }
}
