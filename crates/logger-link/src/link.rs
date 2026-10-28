use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    sync::Arc,
};

use serde_json::{Map, Value, json};

use crate::{
    LoggerSettings,
    subscription::{FormUpdate, Subscription, Timing},
};

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

/// A link to the logger: the contact being worked goes out through it, and
/// the logger's entry form comes back.
///
/// Remembers what it last sent of each field, or last heard the logger hold
/// in it, and sends only what has changed since. A field the operator
/// corrected in the logger is theirs until the application's own value for it
/// changes again, rather than being written back over on every pass through
/// the panel.
pub struct LoggerLink {
    thread_name: String,
    wake: Arc<dyn Fn() + Send + Sync>,
    timing: Timing,
    connection: Option<Connection>,
    sent: BTreeMap<String, String>,
}

struct Connection {
    socket: UdpSocket,
    target: SocketAddr,
    subscription: Subscription,
}

impl LoggerLink {
    /// A link that sends nothing until it is configured, listening for the
    /// logger's form on a thread called `thread_name` that calls `wake` when
    /// the form changes.
    pub fn new(thread_name: impl Into<String>, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            thread_name: thread_name.into(),
            wake: Arc::new(wake),
            timing: Timing::LOGGER,
            connection: None,
            sent: BTreeMap::new(),
        }
    }

    /// Opens the link to the logger `settings` name, or closes it if they say
    /// not to send, and subscribes to its form.
    ///
    /// The socket is bound to the loopback address when the logger is on it,
    /// so a link to the same machine never listens where another one can
    /// reach it. A link that could not be opened is left closed.
    pub fn reconfigure(&mut self, settings: &LoggerSettings) -> Result<(), LinkError> {
        self.connection = None;
        self.sent.clear();
        if !settings.enabled {
            return Ok(());
        }
        let address = settings.address.trim();
        let target: SocketAddr = address.parse().map_err(|_| LinkError::Address(address.to_owned()))?;
        let local = match target.ip() {
            ip if ip.is_loopback() => ip,
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };
        let socket = UdpSocket::bind(SocketAddr::new(local, 0))?;
        let subscription =
            Subscription::start(&socket, target, &self.thread_name, Arc::clone(&self.wake), self.timing)?;
        self.connection = Some(Connection {
            socket,
            target,
            subscription,
        });
        Ok(())
    }

    /// Whether this link sends anything.
    pub const fn is_open(&self) -> bool {
        self.connection.is_some()
    }

    /// Whether the logger is sharing its form: it answered the subscription
    /// within the last 15 seconds and has not said goodbye since.
    pub fn is_synced(&self) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|connection| connection.subscription.is_live())
    }

    /// The logger's form, if it changed since it was last taken.
    ///
    /// Its fields count as sent from then on, so what the application takes
    /// from it is not sent back, and only what changes afterwards is.
    pub fn take_form(&mut self) -> Option<FormUpdate> {
        let update = self.connection.as_mut()?.subscription.take()?;
        self.sent.extend(
            update
                .form
                .fields()
                .map(|(name, text)| (name.to_owned(), text.to_owned())),
        );
        Some(update)
    }

    /// Sends whichever of `fields` changed since they were last sent.
    ///
    /// Fields are named as ADIF names them. One that was never sent counts as
    /// having been sent empty, so a field the application has nothing for
    /// does not clear what the operator typed into the logger themselves. A
    /// datagram that could not be sent is not remembered as sent, and goes
    /// out again with the next change.
    pub fn reflect<'a>(&mut self, fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<(), LinkError> {
        let Some(Connection { socket, target, .. }) = &self.connection else {
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
    use std::{
        sync::mpsc::{self, Receiver},
        time::Duration,
    };

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

        fn next(&self) -> Option<(Value, SocketAddr)> {
            let mut buffer = [0; 2048];
            let (length, from) = self.socket.recv_from(&mut buffer).ok()?;
            Some((serde_json::from_slice(&buffer[..length]).unwrap(), from))
        }

        fn next_of(&self, kind: &str) -> Option<(Value, SocketAddr)> {
            loop {
                let (message, from) = self.next()?;
                if message["type"] == kind {
                    return Some((message, from));
                }
            }
        }

        fn received(&self) -> Option<Value> {
            self.next_of("set").map(|(message, _)| message)
        }

        fn subscriber(&self) -> SocketAddr {
            self.next_of("subscribe").unwrap().1
        }

        fn send(&self, to: SocketAddr, message: &Value) {
            self.socket.send_to(message.to_string().as_bytes(), to).unwrap();
        }
    }

    fn form(fields: &Value) -> Value {
        json!({"v": 1, "type": "form", "fields": fields})
    }

    fn linked(logger: &Logger, timing: Timing) -> (LoggerLink, Receiver<()>) {
        let (woken, wakes) = mpsc::channel();
        let mut link = LoggerLink::new("logger-link-test", move || {
            let _ = woken.send(());
        });
        link.timing = timing;
        link.reconfigure(&logger.settings()).unwrap();
        (link, wakes)
    }

    fn open(logger: &Logger) -> LoggerLink {
        linked(logger, Timing::LOGGER).0
    }

    fn woken(wakes: &Receiver<()>) {
        wakes.recv_timeout(Duration::from_secs(2)).unwrap();
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
        let mut link = open(&logger);

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
        let mut link = open(&logger);

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
        let mut link = open(&logger);
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
        let mut link = LoggerLink::new("logger-link-test", || {});
        link.reconfigure(&LoggerSettings {
            enabled: false,
            ..logger.settings()
        })
        .unwrap();
        assert!(!link.is_open());

        link.reflect([("CALL", "JA1ABC")]).unwrap();
        assert_eq!(logger.next(), None);
    }

    #[rstest]
    #[case::host_name("localhost:2333")]
    #[case::no_port("127.0.0.1")]
    #[case::blank("")]
    fn what_is_not_an_address_is_refused(#[case] address: &str) {
        let mut link = LoggerLink::new("logger-link-test", || {});
        let refused = link.reconfigure(&LoggerSettings {
            enabled: true,
            address: address.to_owned(),
        });
        assert!(matches!(refused, Err(LinkError::Address(_))), "{refused:?}");
        assert!(!link.is_open());
    }

    #[test]
    fn the_form_the_subscription_answers_is_taken() {
        let logger = Logger::listen();
        let (mut link, wakes) = linked(&logger, Timing::LOGGER);
        assert!(!link.is_synced());

        logger.send(logger.subscriber(), &form(&json!({"CALL": "JA1ABC", "NAME": ""})));
        woken(&wakes);

        assert!(link.is_synced());
        let update = link.take_form().unwrap();
        assert!(update.resumed);
        assert_eq!(update.form.get("CALL"), "JA1ABC");
        assert_eq!(update.form.get("QTH"), "");
        assert_eq!(link.take_form(), None);
    }

    #[test]
    fn the_same_form_again_is_not_taken_again() {
        let logger = Logger::listen();
        let (mut link, wakes) = linked(&logger, Timing::LOGGER);
        let subscriber = logger.subscriber();
        logger.send(subscriber, &form(&json!({"CALL": "JA1ABC"})));
        woken(&wakes);
        link.take_form();

        logger.send(subscriber, &form(&json!({"CALL": "JA1ABC"})));
        logger.send(subscriber, &form(&json!({"CALL": "JA1XYZ"})));
        woken(&wakes);

        let update = link.take_form().unwrap();
        assert!(!update.resumed);
        assert_eq!(update.form.get("CALL"), "JA1XYZ");
    }

    #[test]
    fn what_the_form_holds_is_not_sent_back() {
        let logger = Logger::listen();
        let (mut link, wakes) = linked(&logger, Timing::LOGGER);
        logger.send(
            logger.subscriber(),
            &form(&json!({"CALL": "JA1ABC", "RST_RCVD": "599"})),
        );
        woken(&wakes);
        link.take_form();

        link.reflect([("CALL", "JA1ABC"), ("RST_RCVD", "599")]).unwrap();
        assert_eq!(logger.received(), None);

        link.reflect([("CALL", "JA1XYZ"), ("RST_RCVD", "599")]).unwrap();
        assert_eq!(
            logger.received(),
            Some(json!({"v": 1, "type": "set", "fields": {"CALL": "JA1XYZ"}}))
        );
    }

    #[test]
    fn a_logger_saying_goodbye_is_lost_until_it_answers_again() {
        let logger = Logger::listen();
        let (mut link, wakes) = linked(&logger, Timing::LOGGER);
        let subscriber = logger.subscriber();
        logger.send(subscriber, &form(&json!({"CALL": "JA1ABC"})));
        woken(&wakes);
        link.take_form();

        logger.send(subscriber, &json!({"v": 1, "type": "bye"}));
        woken(&wakes);
        assert!(!link.is_synced());
        assert_eq!(link.take_form(), None);

        logger.send(subscriber, &form(&json!({"CALL": "JA1ABC"})));
        woken(&wakes);
        assert!(link.take_form().unwrap().resumed);
    }

    #[test]
    fn a_form_from_anywhere_but_the_logger_is_ignored() {
        let logger = Logger::listen();
        let (link, wakes) = linked(&logger, Timing::LOGGER);
        let stranger = Logger::listen();

        stranger.send(logger.subscriber(), &form(&json!({"CALL": "JA1ABC"})));

        assert!(wakes.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(!link.is_synced());
    }

    #[test]
    fn a_silent_logger_is_lost_while_the_subscription_is_renewed() {
        let logger = Logger::listen();
        let timing = Timing {
            renew: Duration::from_millis(50),
            expiry: Duration::from_millis(300),
            tick: Duration::from_millis(10),
        };
        let (link, wakes) = linked(&logger, timing);
        let subscriber = logger.subscriber();
        logger.send(subscriber, &form(&json!({"CALL": "JA1ABC"})));
        woken(&wakes);

        assert_eq!(logger.subscriber(), subscriber);
        woken(&wakes);
        assert!(!link.is_synced());
    }

    #[test]
    fn closing_the_link_unsubscribes() {
        let logger = Logger::listen();
        let mut link = open(&logger);
        let subscriber = logger.subscriber();

        link.reconfigure(&LoggerSettings {
            enabled: false,
            ..logger.settings()
        })
        .unwrap();

        assert_eq!(logger.next_of("unsubscribe").map(|(_, from)| from), Some(subscriber));
        assert!(!link.is_open());
    }
}
