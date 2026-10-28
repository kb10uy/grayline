use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
};

use serde_json::{Map, Value, json};

use crate::LoggerSettings;

/// Why the logger could not be sent to.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The settings name something that is not an IP address and a port.
    #[error("not an address and a port: {0}")]
    Address(String),
    /// The socket could not be opened, or the datagram could not be sent.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// A socket the contact being worked is sent to the logger through.
///
/// Remembers what it last sent of each field and sends only what has changed
/// since. A field the operator corrected in the logger is theirs until the
/// application's own value for it changes again, rather than being written
/// back over on every pass through the panel.
#[derive(Debug, Default)]
pub struct LoggerLink {
    socket: Option<(UdpSocket, SocketAddr)>,
    sent: BTreeMap<String, String>,
}

impl LoggerLink {
    /// Opens a link to the logger `settings` name, or one that sends nothing
    /// if they say not to send.
    ///
    /// The socket is bound to the loopback address when the logger is on it,
    /// so a link to the same machine never listens where another one can
    /// reach it.
    pub fn open(settings: &LoggerSettings) -> Result<Self, LinkError> {
        if !settings.enabled {
            return Ok(Self::default());
        }
        let address = settings.address.trim();
        let target: SocketAddr = address.parse().map_err(|_| LinkError::Address(address.to_owned()))?;
        let local = match target.ip() {
            ip if ip.is_loopback() => ip,
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };
        let socket = UdpSocket::bind(SocketAddr::new(local, 0))?;
        Ok(Self {
            socket: Some((socket, target)),
            sent: BTreeMap::new(),
        })
    }

    /// Whether this link sends anything.
    pub const fn is_open(&self) -> bool {
        self.socket.is_some()
    }

    /// Sends whichever of `fields` changed since they were last sent.
    ///
    /// Fields are named as ADIF names them. One that was never sent counts as
    /// having been sent empty, so a field the application has nothing for
    /// does not clear what the operator typed into the logger themselves. A
    /// datagram that could not be sent is not remembered as sent, and goes
    /// out again with the next change.
    pub fn reflect<'a>(&mut self, fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<(), LinkError> {
        let Some((socket, target)) = &self.socket else {
            return Ok(());
        };
        let changed: Vec<(&str, &str)> = fields
            .into_iter()
            .filter(|(name, text)| self.sent.get(*name).map_or("", String::as_str) != *text)
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        socket.send_to(set_message(&changed).as_bytes(), target)?;
        self.sent.extend(
            changed
                .into_iter()
                .map(|(name, text)| (name.to_owned(), text.to_owned())),
        );
        Ok(())
    }
}

/// The datagram filling `fields` into the logger's entry form.
///
/// `{"v": 1, "type": "set", "fields": {...}}`, leaving every field it does
/// not name as it is. An empty value clears its field.
pub fn set_message(fields: &[(&str, &str)]) -> String {
    let fields: Map<String, Value> = fields
        .iter()
        .map(|(name, text)| ((*name).to_owned(), Value::String((*text).to_owned())))
        .collect();
    json!({"v": 1, "type": "set", "fields": fields}).to_string()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rstest::rstest;

    use super::*;

    struct Logger {
        socket: UdpSocket,
    }

    impl Logger {
        fn listen() -> Self {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            socket.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
            Self { socket }
        }

        fn settings(&self) -> LoggerSettings {
            LoggerSettings {
                enabled: true,
                address: self.socket.local_addr().unwrap().to_string(),
            }
        }

        fn received(&self) -> Option<Value> {
            let mut buffer = [0; 2048];
            let (length, _) = self.socket.recv_from(&mut buffer).ok()?;
            Some(serde_json::from_slice(&buffer[..length]).unwrap())
        }
    }

    #[test]
    fn a_message_is_the_loggers_set_message() {
        let message: Value = serde_json::from_str(&set_message(&[("CALL", "JA1ABC"), ("NAME", "")])).unwrap();
        assert_eq!(
            message,
            json!({"v": 1, "type": "set", "fields": {"CALL": "JA1ABC", "NAME": ""}})
        );
    }

    #[test]
    fn only_what_changed_is_sent_again() {
        let logger = Logger::listen();
        let mut link = LoggerLink::open(&logger.settings()).unwrap();

        link.reflect([("CALL", "JA1ABC"), ("RST_RCVD", "599")]).unwrap();
        assert_eq!(
            logger.received(),
            Some(json!({"v": 1, "type": "set", "fields": {"CALL": "JA1ABC", "RST_RCVD": "599"}}))
        );

        link.reflect([("CALL", "JA1ABC"), ("RST_RCVD", "579")]).unwrap();
        assert_eq!(
            logger.received(),
            Some(json!({"v": 1, "type": "set", "fields": {"RST_RCVD": "579"}}))
        );

        link.reflect([("CALL", "JA1ABC"), ("RST_RCVD", "579")]).unwrap();
        assert_eq!(logger.received(), None);
    }

    #[test]
    fn a_field_never_filled_is_not_sent_until_it_is() {
        let logger = Logger::listen();
        let mut link = LoggerLink::open(&logger.settings()).unwrap();

        link.reflect([("CALL", ""), ("NAME", "")]).unwrap();
        assert_eq!(logger.received(), None);

        link.reflect([("CALL", "JA1ABC"), ("NAME", "")]).unwrap();
        assert_eq!(
            logger.received(),
            Some(json!({"v": 1, "type": "set", "fields": {"CALL": "JA1ABC"}}))
        );
    }

    #[test]
    fn a_field_emptied_after_it_was_sent_clears_it() {
        let logger = Logger::listen();
        let mut link = LoggerLink::open(&logger.settings()).unwrap();
        link.reflect([("CALL", "JA1ABC")]).unwrap();
        logger.received();

        link.reflect([("CALL", "")]).unwrap();
        assert_eq!(
            logger.received(),
            Some(json!({"v": 1, "type": "set", "fields": {"CALL": ""}}))
        );
    }

    #[test]
    fn a_link_switched_off_sends_nothing() {
        let logger = Logger::listen();
        let mut link = LoggerLink::open(&LoggerSettings {
            enabled: false,
            ..logger.settings()
        })
        .unwrap();
        assert!(!link.is_open());

        link.reflect([("CALL", "JA1ABC")]).unwrap();
        assert_eq!(logger.received(), None);
    }

    #[rstest]
    #[case::host_name("localhost:2333")]
    #[case::no_port("127.0.0.1")]
    #[case::blank("")]
    fn what_is_not_an_address_is_refused(#[case] address: &str) {
        let refused = LoggerLink::open(&LoggerSettings {
            enabled: true,
            address: address.to_owned(),
        });
        assert!(matches!(refused, Err(LinkError::Address(_))), "{refused:?}");
    }
}
