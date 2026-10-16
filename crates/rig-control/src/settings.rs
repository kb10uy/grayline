use std::collections::BTreeMap;

use grayline_rig::DEFAULT_ADDRESS;

/// How often the rig is asked what it is tuned to, in seconds.
pub const DEFAULT_POLL_SECONDS: f32 = 1.0;
/// How long the rig is given to settle between keying and the first sample.
///
/// A rig switching to transmit takes a moment its own audio does not wait for,
/// and anything sent inside it is simply lost. A fifth of a second covers the
/// relays in a station that has them; a rig that switches faster only wastes
/// it.
pub const DEFAULT_LEAD_IN_SECONDS: f32 = 0.2;
/// How long the carrier is held after the last sample has been played.
pub const DEFAULT_TAIL_SECONDS: f32 = 0.05;
/// The name the default port is offered to the script under.
pub const DEFAULT_PORT_NAME: &str = "rig";

/// How the station's rig is reached.
///
/// Only which `rigctld` instances to reach and the keying timing. What is sent
/// over them is the operator's own script, because what a rig wants around a
/// transmission is a property of that rig and of the station around it, and
/// none of that belongs hard-coded in an application.
///
/// An application parses these out of its own configuration file: the file is
/// the application's, while what the fields mean is rig control's.
#[derive(Clone, Debug, PartialEq)]
pub struct RigSettings {
    /// Whether rig control is asked for at all.
    pub enabled: bool,
    /// The transports to open, under the names the script reaches them by.
    pub ports: BTreeMap<String, PortSettings>,
    /// How often the frequency is read, in seconds; zero never reads it.
    pub poll_seconds: f32,
    /// How long after keying the first sample is sent, in seconds.
    pub lead_in_seconds: f32,
    /// How long after the last sample the rig is unkeyed, in seconds.
    pub tail_seconds: f32,
}

/// One `rigctld` the application connects to.
///
/// There is nothing to choose between here: Hamlib already covers the CI-V and
/// DTR/RTS PTT that a second kind of transport would be for, so a port is an
/// address and no more.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortSettings {
    /// Where that `rigctld` listens.
    pub address: String,
}

impl Default for RigSettings {
    fn default() -> Self {
        Self {
            // A station with no rigctld running is the state every first run
            // is in, and trying to reach one would only produce an error the
            // operator did not ask for.
            enabled: false,
            ports: BTreeMap::from([(
                DEFAULT_PORT_NAME.to_owned(),
                PortSettings {
                    address: DEFAULT_ADDRESS.to_owned(),
                },
            )]),
            poll_seconds: DEFAULT_POLL_SECONDS,
            lead_in_seconds: DEFAULT_LEAD_IN_SECONDS,
            tail_seconds: DEFAULT_TAIL_SECONDS,
        }
    }
}
