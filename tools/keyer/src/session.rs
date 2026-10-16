use std::{collections::VecDeque, time::Duration};

use grayline_rtty::{TxCode, TxConfig};

use crate::{
    method::Progress,
    stream::{Next, Outcome},
};

/// The most codes a stream may hold waiting to be keyed.
pub const MAXIMUM_QUEUED_CODES: usize = 65_536;

/// One client connection, which owns whatever it keyed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConnectionId(pub u64);

/// Why a stream was cut short.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    /// A client asked for it.
    Requested,
    /// The connection that owned it went away.
    Disconnected,
    /// It ran past the transmit time limit.
    TimedOut,
    /// The keyer is shutting down.
    ShutDown,
}

/// Where the last stream stands.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Phase {
    /// Nothing has been sent since the keyer started.
    #[default]
    Idle,
    /// A stream is open.
    Sending,
    /// The last stream went out whole.
    Completed,
    /// The last stream was cut short.
    Stopped(StopReason),
    /// The last stream ended on a hardware failure.
    Failed,
}

impl Phase {
    /// The name a status reply gives this phase.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Sending => "sending",
            Self::Completed => "completed",
            Self::Stopped(StopReason::Requested) => "stopped",
            Self::Stopped(StopReason::Disconnected) => "disconnected",
            Self::Stopped(StopReason::TimedOut) => "timeout",
            Self::Stopped(StopReason::ShutDown) => "shutdown",
            Self::Failed => "failed",
        }
    }
}

/// Why a request about a stream was turned down.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// Another stream is still open.
    Busy,
    /// No stream is open, or the open one is being stopped.
    NotOpen,
    /// The open stream belongs to another connection.
    NotOwner,
    /// The stream was already told it has ended.
    Ended,
    /// The codes would overflow the queue.
    Full,
}

/// What a status request reports.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Status {
    /// Where the last stream stands.
    pub phase: Phase,
    /// Whether the PTT line is asked to be keyed.
    pub ptt: bool,
    /// Codes of the current or last stream that have gone out.
    pub sent: usize,
    /// Codes still waiting.
    pub queued: usize,
    /// How far the current or last stream has gone out.
    pub progress: Progress,
    /// The last hardware failure since a stream was opened.
    pub fault: Option<String>,
}

#[derive(Debug)]
struct Stream {
    owner: ConnectionId,
    config: TxConfig,
    queue: VecDeque<TxCode>,
    ended: bool,
    started: bool,
    stop: Option<StopReason>,
}

/// What clients have asked of the keyer, kept apart from the hardware.
#[derive(Debug, Default)]
pub struct Session {
    stream: Option<Stream>,
    phase: Phase,
    sent: usize,
    progress: Progress,
    ptt: bool,
    ptt_owner: Option<ConnectionId>,
    keyed_since: Option<Duration>,
    fault: Option<String>,
}

impl Session {
    /// Opens a stream owned by `owner`.
    pub fn open(&mut self, owner: ConnectionId, config: TxConfig) -> Result<(), Refusal> {
        if self.stream.is_some() {
            return Err(Refusal::Busy);
        }
        self.stream = Some(Stream {
            owner,
            config,
            queue: VecDeque::new(),
            ended: false,
            started: false,
            stop: None,
        });
        self.phase = Phase::Sending;
        self.sent = 0;
        self.progress = Progress::default();
        self.fault = None;
        Ok(())
    }

    /// Queues `codes` on the stream `owner` opened.
    pub fn append(&mut self, owner: ConnectionId, codes: &[TxCode]) -> Result<(), Refusal> {
        let stream = self.owned(owner)?;
        if stream.queue.len() + codes.len() > MAXIMUM_QUEUED_CODES {
            return Err(Refusal::Full);
        }
        stream.queue.extend(codes);
        Ok(())
    }

    /// Marks the stream `owner` opened as having no more codes to come.
    pub fn end(&mut self, owner: ConnectionId) -> Result<(), Refusal> {
        self.owned(owner)?.ended = true;
        Ok(())
    }

    fn owned(&mut self, owner: ConnectionId) -> Result<&mut Stream, Refusal> {
        let stream = self
            .stream
            .as_mut()
            .filter(|stream| stream.stop.is_none())
            .ok_or(Refusal::NotOpen)?;
        if stream.owner != owner {
            return Err(Refusal::NotOwner);
        }
        if stream.ended {
            return Err(Refusal::Ended);
        }
        Ok(stream)
    }

    /// Stops the open stream, returning whether one being keyed has to be
    /// cancelled.
    pub fn stop(&mut self, reason: StopReason) -> bool {
        match self.stream.as_mut() {
            None => false,
            Some(stream) if !stream.started => {
                self.stream = None;
                self.phase = Phase::Stopped(reason);
                false
            }
            Some(stream) => {
                stream.stop.get_or_insert(reason);
                stream.queue.clear();
                true
            }
        }
    }

    /// Asks for the PTT line to be keyed or released on behalf of `owner`.
    pub fn set_ptt(&mut self, owner: ConnectionId, keyed: bool) {
        self.ptt = keyed;
        self.ptt_owner = keyed.then_some(owner);
    }

    /// Whether the PTT line is asked to be keyed.
    pub const fn ptt(&self) -> bool {
        self.ptt
    }

    /// Lets go of everything `connection` keyed, returning whether a stream
    /// being keyed has to be cancelled.
    pub fn release(&mut self, connection: ConnectionId) -> bool {
        if self.ptt_owner == Some(connection) {
            self.ptt = false;
            self.ptt_owner = None;
        }
        match &self.stream {
            Some(stream) if stream.owner == connection => self.stop(StopReason::Disconnected),
            _ => false,
        }
    }

    /// Enforces the transmit time limit at `now`, returning whether a stream
    /// being keyed has to be cancelled.
    pub fn expire(&mut self, now: Duration, limit: Option<Duration>) -> bool {
        if !self.ptt && self.stream.is_none() {
            self.keyed_since = None;
            return false;
        }
        let since = *self.keyed_since.get_or_insert(now);
        match limit {
            Some(limit) if now.saturating_sub(since) >= limit => {
                self.keyed_since = None;
                self.ptt = false;
                self.ptt_owner = None;
                self.stop(StopReason::TimedOut)
            }
            _ => false,
        }
    }

    /// Hands the keyer a stream that is open and not yet being keyed.
    pub fn take_started(&mut self) -> Option<TxConfig> {
        let stream = self.stream.as_mut().filter(|stream| !stream.started)?;
        stream.started = true;
        Some(stream.config)
    }

    /// What the stream being keyed asks for next.
    pub fn take_next(&mut self) -> Next {
        let Some(stream) = self.stream.as_mut() else {
            return Next::Stop;
        };
        if stream.stop.is_some() {
            return Next::Stop;
        }
        match stream.queue.pop_front() {
            Some(code) => Next::Code(code),
            None if stream.ended => Next::End,
            None => Next::Wait,
        }
    }

    /// Records how far the stream being keyed has gone out.
    pub fn record(&mut self, sent: usize, progress: Progress) {
        self.sent = sent;
        self.progress = progress;
    }

    /// Closes the stream that was being keyed.
    pub fn finish(&mut self, outcome: Option<Outcome>) {
        let stop = self.stream.take().and_then(|stream| stream.stop);
        self.phase = match outcome {
            None => Phase::Failed,
            Some(Outcome::Completed) => Phase::Completed,
            Some(Outcome::Stopped) => Phase::Stopped(stop.unwrap_or(StopReason::Requested)),
        };
    }

    /// Notes a hardware failure for the next status request.
    pub fn fail(&mut self, fault: String) {
        self.fault = Some(fault);
    }

    /// What a status request reports.
    pub fn status(&self) -> Status {
        Status {
            phase: self.phase,
            ptt: self.ptt,
            sent: self.sent,
            queued: self.stream.as_ref().map_or(0, |stream| stream.queue.len()),
            progress: self.progress,
            fault: self.fault.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const OWNER: ConnectionId = ConnectionId(1);
    const OTHER: ConnectionId = ConnectionId(2);
    const SECOND: Duration = Duration::from_secs(1);

    fn opened() -> Session {
        let mut session = Session::default();
        session.open(OWNER, TxConfig::default()).unwrap();
        session
    }

    #[test]
    fn codes_come_out_in_order_then_wait_then_end() {
        let mut session = opened();
        session
            .append(OWNER, &[TxCode::Character(1), TxCode::HoldMark])
            .unwrap();
        assert!(session.take_started().is_some());
        assert_eq!(session.take_next(), Next::Code(TxCode::Character(1)));
        assert_eq!(session.take_next(), Next::Code(TxCode::HoldMark));
        assert_eq!(session.take_next(), Next::Wait);
        session.end(OWNER).unwrap();
        assert_eq!(session.take_next(), Next::End);
        session.finish(Some(Outcome::Completed));
        assert_eq!(session.status().phase, Phase::Completed);
    }

    #[test]
    fn a_second_stream_waits_for_the_first_to_finish() {
        let mut session = opened();
        assert_eq!(session.open(OTHER, TxConfig::default()), Err(Refusal::Busy));
        session.take_started();
        session.finish(Some(Outcome::Completed));
        assert_eq!(session.open(OTHER, TxConfig::default()), Ok(()));
    }

    #[rstest]
    #[case(OTHER, Err(Refusal::NotOwner))]
    #[case(OWNER, Ok(()))]
    fn only_the_owner_feeds_a_stream(#[case] connection: ConnectionId, #[case] expected: Result<(), Refusal>) {
        let mut session = opened();
        assert_eq!(session.append(connection, &[TxCode::HoldMark]), expected);
    }

    #[test]
    fn an_ended_stream_takes_no_more_codes() {
        let mut session = opened();
        session.end(OWNER).unwrap();
        assert_eq!(session.append(OWNER, &[TxCode::HoldMark]), Err(Refusal::Ended));
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut session = opened();
        let codes = vec![TxCode::HoldMark; MAXIMUM_QUEUED_CODES];
        session.append(OWNER, &codes).unwrap();
        assert_eq!(session.append(OWNER, &[TxCode::HoldMark]), Err(Refusal::Full));
    }

    #[test]
    fn stopping_a_stream_not_yet_keyed_closes_it_without_a_cancel() {
        let mut session = opened();
        assert!(!session.stop(StopReason::Requested));
        assert_eq!(session.status().phase, Phase::Stopped(StopReason::Requested));
        assert!(session.take_started().is_none());
    }

    #[test]
    fn stopping_a_stream_being_keyed_cancels_it_and_keeps_the_reason() {
        let mut session = opened();
        session.append(OWNER, &[TxCode::HoldMark]).unwrap();
        session.take_started();
        assert!(session.stop(StopReason::Requested));
        assert_eq!(session.take_next(), Next::Stop);
        assert_eq!(session.append(OWNER, &[TxCode::HoldMark]), Err(Refusal::NotOpen));
        session.finish(Some(Outcome::Stopped));
        assert_eq!(session.status().phase, Phase::Stopped(StopReason::Requested));
    }

    #[test]
    fn a_disconnect_releases_only_what_that_connection_keyed() {
        let mut session = opened();
        session.take_started();
        session.set_ptt(OTHER, true);
        assert!(!session.release(ConnectionId(3)));
        assert!(session.ptt());
        assert!(session.release(OWNER));
        assert!(session.ptt());
        session.finish(Some(Outcome::Stopped));
        assert_eq!(session.status().phase, Phase::Stopped(StopReason::Disconnected));
        assert!(!session.release(OTHER));
        assert!(!session.ptt());
    }

    #[test]
    fn the_time_limit_unkeys_and_stops_once_it_runs_out() {
        let mut session = opened();
        session.take_started();
        session.set_ptt(OWNER, true);
        let limit = Some(10 * SECOND);
        assert!(!session.expire(SECOND, limit));
        assert!(!session.expire(10 * SECOND, limit));
        assert!(session.expire(11 * SECOND, limit));
        assert!(!session.ptt());
        session.finish(Some(Outcome::Stopped));
        assert_eq!(session.status().phase, Phase::Stopped(StopReason::TimedOut));
    }

    #[test]
    fn the_time_limit_counts_from_when_keying_began() {
        let mut session = Session::default();
        let limit = Some(10 * SECOND);
        assert!(!session.expire(100 * SECOND, limit));
        session.set_ptt(OWNER, true);
        assert!(!session.expire(101 * SECOND, limit));
        assert!(!session.expire(110 * SECOND, limit));
        assert!(!session.expire(111 * SECOND, limit));
        assert!(!session.ptt());
    }

    #[test]
    fn no_limit_never_expires() {
        let mut session = Session::default();
        session.set_ptt(OWNER, true);
        assert!(!session.expire(Duration::ZERO, None));
        assert!(!session.expire(Duration::from_secs(1_000_000), None));
        assert!(session.ptt());
    }

    #[test]
    fn a_failure_is_reported_until_the_next_stream_opens() {
        let mut session = opened();
        session.take_started();
        session.fail("unplugged".to_owned());
        session.finish(None);
        let status = session.status();
        assert_eq!(status.phase, Phase::Failed);
        assert_eq!(status.fault.as_deref(), Some("unplugged"));
        session.open(OWNER, TxConfig::default()).unwrap();
        assert_eq!(session.status().fault, None);
    }
}
