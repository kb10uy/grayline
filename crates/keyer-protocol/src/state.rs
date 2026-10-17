/// Where a keyer's last stream stands, as `\status` reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamState {
    /// Nothing has been sent since the keyer started.
    Idle,
    /// A stream is open.
    Sending,
    /// The last stream went out whole.
    Completed,
    /// The last stream was cut short by a client.
    Stopped,
    /// The connection that owned the last stream went away.
    Disconnected,
    /// The last stream ran past the keyer's transmit time limit.
    TimedOut,
    /// The keyer shut down under the last stream.
    ShutDown,
    /// The last stream ended on a hardware failure.
    Failed,
}

impl StreamState {
    /// Every state, in no particular order.
    pub const ALL: [Self; 8] = [
        Self::Idle,
        Self::Sending,
        Self::Completed,
        Self::Stopped,
        Self::Disconnected,
        Self::TimedOut,
        Self::ShutDown,
        Self::Failed,
    ];

    /// The word a status reply spells this state with.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Sending => "sending",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Disconnected => "disconnected",
            Self::TimedOut => "timeout",
            Self::ShutDown => "shutdown",
            Self::Failed => "failed",
        }
    }

    /// Reads the word [`name`](Self::name) writes.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.name() == name)
    }

    /// Whether the stream ended without going out whole.
    pub const fn is_cut_short(self) -> bool {
        matches!(
            self,
            Self::Stopped | Self::Disconnected | Self::TimedOut | Self::ShutDown | Self::Failed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_reads_back_from_its_name() {
        for state in StreamState::ALL {
            assert_eq!(StreamState::from_name(state.name()), Some(state));
        }
        assert_eq!(StreamState::from_name("Completed"), None);
    }
}
