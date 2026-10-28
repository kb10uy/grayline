use std::{
    collections::BTreeMap,
    io,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

const SUBSCRIBE: &[u8] = br#"{"v":1,"type":"subscribe"}"#;
const UNSUBSCRIBE: &[u8] = br#"{"v":1,"type":"unsubscribe"}"#;

/// The logger's entry form, as it last said it was.
///
/// Fields are named as ADIF names them. The logger sends the whole form every
/// time, an empty field included, so a field it does not name is one it does
/// not share rather than one it left as it was.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoggerForm(BTreeMap<String, String>);

impl LoggerForm {
    /// What the form holds in `name`, or nothing.
    pub fn get(&self, name: &str) -> &str {
        self.0.get(name).map_or("", String::as_str)
    }

    /// Every field the form holds.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(name, text)| (name.as_str(), text.as_str()))
    }
}

/// A form the logger sent that differs from the one last taken.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormUpdate {
    /// The form as it is now.
    pub form: LoggerForm,
    /// Whether this is the first form since the link was opened or since the
    /// logger was last lost.
    ///
    /// The application has been keeping its own fields in the meantime, and
    /// the ones the logger has nothing for are worth handing back to it.
    pub resumed: bool,
}

/// How often the subscription is renewed, and how long a silent logger is
/// still counted as there.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    pub renew: Duration,
    pub expiry: Duration,
    pub tick: Duration,
}

impl Timing {
    /// The logger's own: a subscriber renews every 5 seconds and is dropped
    /// after 15, so a logger that has not answered for 15 seconds has dropped
    /// it or gone.
    pub(crate) const LOGGER: Self = Self {
        renew: Duration::from_secs(5),
        expiry: Duration::from_secs(15),
        tick: Duration::from_secs(1),
    };
}

#[derive(Default)]
struct Mirror {
    form: Option<LoggerForm>,
    generation: u64,
    session: u64,
}

#[derive(Default)]
struct Shared {
    mirror: Mutex<Mirror>,
    closed: AtomicBool,
}

impl Shared {
    fn hold(&self, form: Option<LoggerForm>) -> bool {
        let mut mirror = self.mirror.lock().unwrap_or_else(PoisonError::into_inner);
        if mirror.form == form {
            return false;
        }
        if mirror.form.is_none() {
            mirror.session += 1;
        }
        mirror.form = form;
        mirror.generation += 1;
        true
    }
}

/// A subscription to the logger's form, renewed on a thread of its own.
pub(crate) struct Subscription {
    socket: UdpSocket,
    target: SocketAddr,
    shared: Arc<Shared>,
    generation: u64,
    session: u64,
}

impl Subscription {
    /// Subscribes through `socket` to the logger at `target`, on a thread
    /// called `thread_name` that calls `wake` whenever the form changes or the
    /// logger is lost.
    pub(crate) fn start(
        socket: &UdpSocket,
        target: SocketAddr,
        thread_name: &str,
        wake: Arc<dyn Fn() + Send + Sync>,
        timing: Timing,
    ) -> io::Result<Self> {
        let listening = socket.try_clone()?;
        listening.set_read_timeout(Some(timing.tick))?;
        listening.send_to(SUBSCRIBE, target)?;
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || listen(&listening, target, &thread_shared, &*wake, timing))?;
        Ok(Self {
            socket: socket.try_clone()?,
            target,
            shared,
            generation: 0,
            session: 0,
        })
    }

    pub(crate) fn is_live(&self) -> bool {
        self.shared
            .mirror
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .form
            .is_some()
    }

    pub(crate) fn take(&mut self) -> Option<FormUpdate> {
        let mirror = self.shared.mirror.lock().unwrap_or_else(PoisonError::into_inner);
        if mirror.generation == self.generation {
            return None;
        }
        self.generation = mirror.generation;
        let form = mirror.form.clone()?;
        let resumed = mirror.session != self.session;
        self.session = mirror.session;
        Some(FormUpdate { form, resumed })
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        let _ = self.socket.send_to(UNSUBSCRIBE, self.target);
    }
}

fn listen(socket: &UdpSocket, target: SocketAddr, shared: &Shared, wake: &dyn Fn(), timing: Timing) {
    let mut buffer = vec![0; 65536];
    let mut renewed = Instant::now();
    let mut heard: Option<Instant> = None;
    while !shared.closed.load(Ordering::Relaxed) {
        let message = match socket.recv_from(&mut buffer) {
            Ok((length, from)) if from == target => parse(&buffer[..length]),
            Ok(_) => None,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset
                ) =>
            {
                None
            }
            Err(_) => {
                thread::sleep(timing.tick);
                None
            }
        };
        let now = Instant::now();
        let changed = match message {
            Some(Message::Form(form)) => {
                heard = Some(now);
                shared.hold(Some(form))
            }
            Some(Message::Bye) => {
                heard = None;
                shared.hold(None)
            }
            None if heard.is_some_and(|at| now - at >= timing.expiry) => {
                heard = None;
                shared.hold(None)
            }
            None => false,
        };
        if changed && !shared.closed.load(Ordering::Relaxed) {
            wake();
        }
        if now - renewed >= timing.renew {
            let _ = socket.send_to(SUBSCRIBE, target);
            renewed = now;
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Message {
    Form(LoggerForm),
    Bye,
}

fn parse(datagram: &[u8]) -> Option<Message> {
    let message: Value = serde_json::from_slice(datagram).ok()?;
    if message.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    match message.get("type")?.as_str()? {
        "form" => message
            .get("fields")?
            .as_object()?
            .iter()
            .map(|(name, text)| Some((name.to_ascii_uppercase(), text.as_str()?.to_owned())))
            .collect::<Option<_>>()
            .map(|fields| Message::Form(LoggerForm(fields))),
        "bye" => Some(Message::Bye),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn form(fields: &[(&str, &str)]) -> LoggerForm {
        LoggerForm(
            fields
                .iter()
                .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn a_form_is_read_with_its_fields_named_in_capitals() {
        let parsed = parse(br#"{"v": 1, "type": "form", "fields": {"call": "JA1ABC", "NAME": ""}}"#);
        assert_eq!(parsed, Some(Message::Form(form(&[("CALL", "JA1ABC"), ("NAME", "")]))));
    }

    #[test]
    fn bye_is_read() {
        assert_eq!(parse(br#"{"v": 1, "type": "bye"}"#), Some(Message::Bye));
    }

    #[rstest]
    #[case::another_version(r#"{"v": 2, "type": "bye"}"#)]
    #[case::no_version(r#"{"type": "bye"}"#)]
    #[case::unknown_type(r#"{"v": 1, "type": "set", "fields": {}}"#)]
    #[case::a_field_not_text(r#"{"v": 1, "type": "form", "fields": {"CALL": 1}}"#)]
    #[case::no_fields(r#"{"v": 1, "type": "form"}"#)]
    #[case::not_json("<call:6>JA1ABC <eor>")]
    fn anything_else_is_ignored(#[case] datagram: &str) {
        assert_eq!(parse(datagram.as_bytes()), None);
    }
}
