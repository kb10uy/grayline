use toml_edit::{Item, Table, value};

/// Where jl1his-logger's desktop client takes other programs' entries.
///
/// The second of its two default listeners. The first, `127.0.0.1:2237`, is
/// WSJT-X's port, and a WSJT-X running beside this one would find it taken.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:2333";

/// Whether, and where, the contact being worked is sent to the logger.
///
/// Written under `[logger]` by every application that sends one, in the same
/// shape, so a station set up for one is set up for the others by copying the
/// section across.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggerSettings {
    /// Whether the contact is sent at all.
    pub enabled: bool,
    /// The address the logger listens on, as an IP address and a port.
    ///
    /// An address rather than a host name, because the name would be looked
    /// up on the thread that draws the interface, and the logger is on the
    /// same machine or one whose address the operator knows.
    pub address: String,
}

impl Default for LoggerSettings {
    /// Off, at the logger's own address.
    ///
    /// Off because the logger's listener is off until it is turned on there,
    /// and a datagram sent to a port nobody listens on is one more thing the
    /// operator did not ask for.
    fn default() -> Self {
        Self {
            enabled: false,
            address: DEFAULT_ADDRESS.to_owned(),
        }
    }
}

impl LoggerSettings {
    /// Reads a `[logger]` section over `defaults`.
    ///
    /// An address left blank reads as the default one rather than as nowhere:
    /// the switch says whether to send, and an empty address would only be a
    /// second way of saying no that the switch does not show.
    pub fn read(section: Option<&Table>, defaults: Self) -> Self {
        let Some(section) = section else {
            return defaults;
        };
        Self {
            enabled: section
                .get("enabled")
                .and_then(Item::as_bool)
                .unwrap_or(defaults.enabled),
            address: section
                .get("address")
                .and_then(Item::as_str)
                .map(str::trim)
                .filter(|address| !address.is_empty())
                .map_or(defaults.address, str::to_owned),
        }
    }

    /// Writes these into a `[logger]` section, the address included.
    ///
    /// The address is written while the switch is off, because the file is
    /// where it is changed, and an operator whose logger listens elsewhere has
    /// no way to learn the key exists otherwise.
    pub fn write(&self, section: &mut Table) {
        section["enabled"] = value(self.enabled);
        section["address"] = value(self.address.as_str());
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use toml_edit::DocumentMut;

    use super::*;

    fn read(text: &str) -> LoggerSettings {
        let document: DocumentMut = text.parse().unwrap();
        LoggerSettings::read(
            document.get("logger").and_then(Item::as_table),
            LoggerSettings::default(),
        )
    }

    #[test]
    fn a_missing_section_reads_as_the_defaults() {
        assert_eq!(read(""), LoggerSettings::default());
    }

    #[test]
    fn a_written_section_reads_back_beside_keys_it_does_not_own() {
        let wanted = LoggerSettings {
            enabled: true,
            address: "192.0.2.10:2333".to_owned(),
        };
        let mut document: DocumentMut = "[logger]\nnote = \"kept\"\n".parse().unwrap();
        wanted.write(document["logger"].as_table_mut().unwrap());

        let written = document.to_string();
        assert!(written.contains("note = \"kept\""), "{written}");
        assert_eq!(read(&written), wanted);
    }

    #[rstest]
    #[case::blank("[logger]\naddress = \"  \"\n")]
    #[case::not_text("[logger]\naddress = 2333\n")]
    fn an_unusable_address_reads_as_the_default(#[case] text: &str) {
        assert_eq!(read(text).address, DEFAULT_ADDRESS);
    }
}
