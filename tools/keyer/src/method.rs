use std::{sync::atomic::AtomicBool, time::Duration};

use grayline_rtty::{TxCode, TxConfig};

use crate::error::KeyerError;

/// Keying handed to a JL1HIS terminal unit, which times the line itself.
pub mod jl1his_terminal_unit;
/// Keying by switching a serial port's control line, timed by the keyer.
pub mod line_bitbang;

pub use jl1his_terminal_unit::Jl1hisTerminalUnit;
pub use line_bitbang::LineBitbang;

/// The ticks per second a line is framed in: one tick is a microsecond.
pub(crate) const TICKS_PER_SECOND: u32 = 1_000_000;

/// Whether keying went on to the end of what it was asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Flow {
    /// It did.
    Continue,
    /// It is still under way, and the same call has to be made again. Only
    /// [`Method::finish`] answers this.
    Pending,
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
/// once `cancel` is raised. A method whose hardware times the line itself
/// blocks no longer than it takes to hand the work over and to ask how far
/// it has got, so that the stream's progress and its time limit are looked at
/// while the hardware keys.
pub trait Method {
    /// Where the method's own clock stands.
    fn now(&self) -> Duration;
    /// Whether [`send`](Self::send) can take another code now. A stream idles
    /// while it cannot.
    fn ready(&self) -> bool {
        true
    }
    /// Whether the method keys PTT itself around each stream, so that a
    /// client has no need to key the rig any other way.
    fn keys_ptt(&self) -> bool {
        false
    }
    /// Starts a stream framed by `config`, through its lead-in.
    fn begin(&mut self, config: &TxConfig, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Keys one code.
    fn send(&mut self, code: TxCode, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Idles at mark for one bit while the stream waits for more codes.
    fn idle(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Ends the stream through its tail, or answers [`Flow::Pending`] while
    /// the tail is still to come.
    fn finish(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError>;
    /// Returns the output to mark at once and forgets the stream.
    fn rest(&mut self) -> Result<(), KeyerError>;
    /// How far the current or last stream has gone out.
    fn progress(&self) -> Progress;
}

pub(crate) fn ticks_to_duration(ticks: f64) -> Duration {
    Duration::from_nanos((ticks * 1_000.0).round() as u64)
}
