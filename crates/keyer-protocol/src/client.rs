use std::time::Duration;

use grayline_rig::{RigError, Rigctld};
use grayline_rtty::{TxCode, TxConfig};

use crate::{
    codes::{MAXIMUM_CODES_PER_LINE, encode_codes},
    framing::encode_open,
    state::StreamState,
};

/// Where `gl-keyer` listens unless it was told otherwise: the port after
/// `rigctld`'s 4532 and `rotctld`'s 4533.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:4534";

/// What a status request reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Status {
    /// Where the current or last stream stands.
    pub state: StreamState,
    /// Codes of that stream that have gone out.
    pub sent: usize,
    /// Codes still waiting.
    pub queued: usize,
    /// The stream's own timeline up to the end of what was last keyed: what
    /// a transmitter would have played of it.
    pub elapsed: Duration,
    /// The latest any transition took effect behind its deadline.
    pub late: Duration,
    /// The hardware failure the stream ended on, if it ended on one.
    pub error: Option<String>,
}

impl Status {
    /// Reads the lines of an extended `\status` answer.
    pub fn parse(lines: &[String]) -> Option<Self> {
        let field = |label: &str| {
            lines
                .iter()
                .find_map(|line| line.strip_prefix(label)?.strip_prefix(": "))
        };
        let micros = |label: &str| field(label)?.parse().ok().map(Duration::from_micros);
        Some(Self {
            state: StreamState::from_name(field("State")?)?,
            sent: field("Sent")?.parse().ok()?,
            queued: field("Queued")?.parse().ok()?,
            elapsed: micros("Elapsed")?,
            late: micros("Late")?,
            error: field("Error").map(str::to_owned),
        })
    }
}

/// A connection to `gl-keyer`.
///
/// A stream belongs to the connection that opened it, and stops when that
/// connection goes: dropping the client is a stop the keyer cannot miss.
#[derive(Debug)]
pub struct KeyerClient {
    rig: Rigctld,
}

impl KeyerClient {
    /// Connects to the keyer at `address`.
    pub fn connect(address: &str, timeout: Duration) -> Result<Self, RigError> {
        Rigctld::connect(address, timeout).map(|rig| Self { rig })
    }

    /// The address this connection was opened to.
    pub fn address(&self) -> &str {
        self.rig.address()
    }

    /// Opens a stream framed by `config`, which starts keying its lead-in.
    pub fn open(&mut self, config: &TxConfig) -> Result<(), RigError> {
        self.rig.run(&format!("\\rtty_open {}", encode_open(config))).map(drop)
    }

    /// Queues `codes` on the stream this connection opened.
    ///
    /// Split into as many commands as the keyer's line limit asks for.
    pub fn send(&mut self, codes: &[TxCode]) -> Result<(), RigError> {
        for chunk in codes.chunks(MAXIMUM_CODES_PER_LINE) {
            self.rig.run(&format!("\\rtty_send {}", encode_codes(chunk)))?;
        }
        Ok(())
    }

    /// Says no more codes are coming, so the tail follows the last of them.
    pub fn end(&mut self) -> Result<(), RigError> {
        self.rig.run("\\rtty_end").map(drop)
    }

    /// Cuts the open stream short, leaving the line at mark.
    pub fn stop(&mut self) -> Result<(), RigError> {
        self.rig.run("\\stop").map(drop)
    }

    /// Asks how the current or last stream stands.
    pub fn status(&mut self) -> Result<Status, RigError> {
        let response = self.rig.run("\\status")?;
        Status::parse(response.lines()).ok_or_else(|| RigError::Unreadable {
            command: "\\status".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_status_answer_is_read_by_its_labels() {
        let status = Status::parse(&lines(
            "status:\nState: sending\nSent: 12\nQueued: 3\nElapsed: 1500000\nLate: 250",
        ));
        assert_eq!(
            status,
            Some(Status {
                state: StreamState::Sending,
                sent: 12,
                queued: 3,
                elapsed: Duration::from_millis(1_500),
                late: Duration::from_micros(250),
                error: None,
            })
        );
    }

    #[test]
    fn a_failure_carries_what_the_keyer_said() {
        let status = Status::parse(&lines(
            "State: failed\nSent: 0\nQueued: 0\nElapsed: 0\nLate: 0\nError: the port went away",
        ))
        .unwrap();
        assert_eq!(status.state, StreamState::Failed);
        assert_eq!(status.error.as_deref(), Some("the port went away"));
    }

    #[test]
    fn an_answer_missing_a_field_is_unreadable() {
        assert_eq!(Status::parse(&lines("State: idle\nSent: 0\nQueued: 0\nLate: 0")), None);
        assert_eq!(
            Status::parse(&lines("State: keying\nSent: 0\nQueued: 0\nElapsed: 0\nLate: 0")),
            None
        );
    }
}
