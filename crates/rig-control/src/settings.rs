use std::{collections::BTreeMap, ops::RangeInclusive};

use grayline_rig::DEFAULT_ADDRESS;
use toml_edit::{Item, Table, value};

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
/// The longest either side of a transmission may be padded by, in seconds.
pub const KEYING_SECONDS_RANGE: RangeInclusive<f32> = 0.0..=5.0;
/// How far apart frequency reads may be asked to be, in seconds.
///
/// Zero is a value rather than a floor: it turns reading off, which is what an
/// operator wants who keys through rig control but reads the frequency off the
/// front panel.
pub const POLL_SECONDS_RANGE: RangeInclusive<f32> = 0.0..=60.0;

/// How the station's rig is reached.
///
/// Only which `rigctld` instances to reach and the keying timing. What is sent
/// over them is the operator's own script, because what a rig wants around a
/// transmission is a property of that rig and of the station around it, and
/// none of that belongs hard-coded in an application.
///
/// An application finds the `[rig]` section in its own configuration file and
/// hands it to [`RigSettings::read`]: the file is the application's, while what
/// the section means is rig control's.
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

impl RigSettings {
    /// Reads a `[rig]` section over `defaults`.
    ///
    /// A missing section, a missing key, and a value that cannot be used all
    /// read as the default; a timing outside its range is clamped into it,
    /// as every other hand-edited figure is. The ports are the exception: a
    /// `ports` section that is present is taken as written, an entry that is
    /// not a section is dropped, and one that leaves nothing behind leaves the
    /// script with no port to reach rather than being given the default back.
    pub fn read(section: Option<&Table>, defaults: Self) -> Self {
        let Some(section) = section else {
            return defaults;
        };
        let seconds = |key: &str, range: &RangeInclusive<f32>, default: f32| {
            section
                .get(key)
                .and_then(Item::as_value)
                .and_then(|item| {
                    item.as_float()
                        .or_else(|| item.as_integer().map(|integer| integer as f64))
                })
                .map(|seconds| seconds as f32)
                .filter(|seconds| seconds.is_finite())
                .map_or(default, |seconds| seconds.clamp(*range.start(), *range.end()))
        };
        Self {
            enabled: section
                .get("enabled")
                .and_then(Item::as_bool)
                .unwrap_or(defaults.enabled),
            ports: section
                .get("ports")
                .and_then(Item::as_table)
                .map_or(defaults.ports, read_ports),
            poll_seconds: seconds("poll-interval", &POLL_SECONDS_RANGE, defaults.poll_seconds),
            lead_in_seconds: seconds("lead-in", &KEYING_SECONDS_RANGE, defaults.lead_in_seconds),
            tail_seconds: seconds("tail", &KEYING_SECONDS_RANGE, defaults.tail_seconds),
        }
    }

    /// Writes these into a `[rig]` section, ports and all.
    ///
    /// Written even at the defaults, because the configuration file is where
    /// they are edited: an operator who had to add the keys before changing
    /// them would have no way to learn the keys exist. Keys are assigned
    /// rather than the section being rebuilt, so a comment beside one survives.
    pub fn write(&self, section: &mut Table) {
        section["enabled"] = value(self.enabled);
        for (key, seconds) in [
            ("poll-interval", self.poll_seconds),
            ("lead-in", self.lead_in_seconds),
            ("tail", self.tail_seconds),
        ] {
            // Rounded to the millisecond on the way out, so a widened f32 does
            // not write 0.20000000298023224 into a file meant to be read.
            section[key] = value((f64::from(seconds) * 1_000.0).round() / 1_000.0);
        }
        let ports = child_table(section, "ports");
        ports.retain(|name, _| self.ports.contains_key(name));
        for (name, port) in &self.ports {
            child_table(ports, name)["address"] = value(port.address.as_str());
        }
    }
}

fn read_ports(ports: &Table) -> BTreeMap<String, PortSettings> {
    ports
        .iter()
        .filter_map(|(name, item)| {
            let address = item
                .as_table()?
                .get("address")
                .and_then(Item::as_str)
                .map(str::trim)
                .filter(|address| !address.is_empty())
                .unwrap_or(DEFAULT_ADDRESS)
                .to_owned();
            Some((name.to_owned(), PortSettings { address }))
        })
        .collect()
}

fn child_table<'a>(parent: &'a mut Table, name: &str) -> &'a mut Table {
    let entry = parent.entry(name).or_insert(Item::Table(Table::new()));
    if !entry.is_table() {
        *entry = Item::Table(Table::new());
    }
    entry.as_table_mut().expect("the entry holds a table")
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use toml_edit::DocumentMut;

    use super::*;

    fn read(text: &str) -> RigSettings {
        let document: DocumentMut = text.parse().unwrap();
        RigSettings::read(document.get("rig").and_then(Item::as_table), RigSettings::default())
    }

    #[test]
    fn a_missing_section_reads_as_the_defaults() {
        assert_eq!(read(""), RigSettings::default());
    }

    #[test]
    fn a_written_section_reads_back() {
        let wanted = RigSettings {
            enabled: true,
            ports: BTreeMap::from([(
                "radio".to_owned(),
                PortSettings {
                    address: "192.168.0.8:4532".to_owned(),
                },
            )]),
            poll_seconds: 2.5,
            lead_in_seconds: 0.3,
            tail_seconds: 0.08,
        };
        let mut document = DocumentMut::new();
        document["rig"] = Item::Table(Table::new());
        wanted.write(document["rig"].as_table_mut().unwrap());

        assert_eq!(read(&document.to_string()), wanted);
    }

    #[rstest]
    #[case(
        "poll-interval = 2.5
",
        2.5,
        DEFAULT_LEAD_IN_SECONDS
    )]
    #[case(
        "poll-interval = -4
",
        0.0,
        DEFAULT_LEAD_IN_SECONDS
    )]
    #[case(
        "poll-interval = 600
",
        60.0,
        DEFAULT_LEAD_IN_SECONDS
    )]
    #[case(
        "lead-in = 99
",
        DEFAULT_POLL_SECONDS,
        5.0
    )]
    #[case(
        "lead-in = \"soon\"
",
        DEFAULT_POLL_SECONDS,
        DEFAULT_LEAD_IN_SECONDS
    )]
    #[case(
        "lead-in = nan
",
        DEFAULT_POLL_SECONDS,
        DEFAULT_LEAD_IN_SECONDS
    )]
    fn timings_are_read_within_range(#[case] written: &str, #[case] poll: f32, #[case] lead_in: f32) {
        let rig = read(&format!(
            "[rig]
{written}"
        ));
        assert_eq!(rig.poll_seconds, poll);
        assert_eq!(rig.lead_in_seconds, lead_in);
    }

    #[test]
    fn a_ports_key_that_is_not_a_section_leaves_the_default_port() {
        assert_eq!(
            read(
                "[rig]
ports = 3
"
            )
            .ports,
            RigSettings::default().ports
        );
    }

    #[test]
    fn a_port_with_no_usable_address_reaches_the_default_one() {
        let rig = read(
            "[rig.ports.radio]
address = \"  \"
",
        );
        assert_eq!(rig.ports["radio"].address, DEFAULT_ADDRESS);
    }
}
