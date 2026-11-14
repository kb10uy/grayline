use std::{
    collections::VecDeque,
    fmt::Write as _,
    io::{ErrorKind, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use grayline_rtty::{
    Parity, TxCode, TxConfig,
    tx::{Framer, Keying},
};
use serialport::SerialPort;

use crate::{
    clock::Clock,
    error::KeyerError,
    method::{Flow, Method, Progress, TICKS_PER_SECOND, ticks_to_duration},
};

/// The byte that aborts whatever the unit is doing, in any of its modes.
const ABORT: u8 = 0x03;

/// How long one read of the port waits for bytes, and how long the port has
/// to stay silent to be taken as having nothing more to say.
const READ_TIMEOUT: Duration = Duration::from_millis(100);

/// How long the unit may take to answer a command.
const REPLY_TIMEOUT: Duration = Duration::from_secs(1);

/// How often the unit is asked how far it has got.
const STATUS_INTERVAL: Duration = Duration::from_millis(20);

/// The most codes one `SENDM` carries, and the most held back for a unit
/// whose queue is full.
const CODES_PER_SEND: usize = 256;

/// The longest `TXDELAY` and `TXTAIL` the unit holds, in milliseconds.
const MAXIMUM_HOLD_MILLISECONDS: f64 = 65_535.0;

/// The unit's `BAUD` range, in thousandths of a baud.
const MILLIBAUD_RANGE: (f64, f64) = (10_000.0, 1_200_000.0);

/// The reason the unit gives for codes that do not fit its queue.
const QUEUE_FULL: &str = "queue full";

/// Opens the control port of a terminal unit at `path`.
///
/// DTR is asserted for as long as the port is open. The unit aborts when it
/// drops, so a keyer that dies, however it dies, does not leave the unit
/// keyed.
pub fn open_port(path: &str) -> Result<Box<dyn SerialPort>, KeyerError> {
    let failed = |error: serialport::Error| KeyerError::Port {
        port: path.to_owned(),
        detail: error.to_string(),
    };
    let mut port = serialport::new(path, 9_600)
        .timeout(READ_TIMEOUT)
        .dtr_on_open(true)
        .open()
        .map_err(failed)?;
    port.write_data_terminal_ready(true).map_err(failed)?;
    Ok(port)
}

/// Keying handed to a JL1HIS terminal unit, which times the line itself.
///
/// The unit takes code words over its control port and keys them from a
/// state machine of its own, so nothing here is timed but the polling. Codes
/// are handed over as the unit has room for them, PTT on the unit is keyed
/// once the first of them are there, and the stream's timeline is followed
/// through the unit's `STATUS`: the count of codes it has started says where
/// the stream is, and the host's clock only fills in the code being keyed.
pub struct UnitMethod<P, C> {
    link: Link<P>,
    clock: C,
    version: String,
    stream: Option<Stream>,
    progress: Progress,
}

impl<P: Read + Write, C: Clock> UnitMethod<P, C> {
    /// Takes over the unit on `port`, aborting whatever it was doing.
    pub fn new(port: P, clock: C) -> Result<Self, KeyerError> {
        let mut link = Link {
            port,
            received: Vec::new(),
            unsure: true,
        };
        let version = link.resync(&clock)?;
        Ok(Self {
            link,
            clock,
            version,
            stream: None,
            progress: Progress::default(),
        })
    }

    /// The firmware version the unit reported.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Hands over as many held codes as the unit has room for, and keys PTT
    /// once a stream has been given its first chance to hand some over.
    fn flush(&mut self) -> Result<(), KeyerError> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(());
        };
        while !stream.batch.is_empty() && stream.free > 0 {
            let count = stream.batch.len().min(stream.free).min(CODES_PER_SEND);
            let command = send_command(stream.batch.iter().take(count));
            match self.link.request(&command, &self.clock)? {
                Ok(_) => {
                    stream.batch.drain(..count);
                    stream.handed += count;
                    stream.free -= count;
                }
                Err(reason) if reason == QUEUE_FULL => stream.free = 0,
                Err(reason) => return Err(refused("SENDM", &reason)),
            }
        }
        if stream.since.is_none() {
            self.link.command("PTT ON", &self.clock)?;
            stream.since = Some(self.clock.now());
        }
        Ok(())
    }

    fn poll(&mut self, cancel: &AtomicBool) -> Result<Option<UnitStatus>, KeyerError> {
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.clock.sleep_until(self.clock.now() + STATUS_INTERVAL);
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let status = self.link.status(&self.clock)?;
        if let Some(stream) = self.stream.as_mut() {
            stream.observe(status, self.clock.now());
        }
        self.advance();
        Ok(Some(status))
    }

    fn advance(&mut self) {
        if let Some(stream) = &self.stream {
            self.progress.elapsed = self.progress.elapsed.max(stream.elapsed(self.clock.now()));
        }
    }
}

impl<P: Read + Write, C: Clock> Method for UnitMethod<P, C> {
    fn now(&self) -> Duration {
        self.clock.now()
    }

    fn ready(&self) -> bool {
        self.stream
            .as_ref()
            .is_none_or(|stream| stream.batch.len() < CODES_PER_SEND)
    }

    fn keys_ptt(&self) -> bool {
        true
    }

    fn begin(&mut self, config: &TxConfig, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        let commands = settings(config)?;
        let mut framer =
            Framer::new(TICKS_PER_SECOND, config).map_err(|error| KeyerError::Framing(error.to_string()))?;
        let mut character = Length::default();
        framer.frame(TxCode::Character(0), &mut character);
        self.progress = Progress::default();
        if self.link.unsure {
            self.link.resync(&self.clock)?;
        }
        for command in &commands {
            if cancel.load(Ordering::Acquire) {
                return Ok(Flow::Cancelled);
            }
            self.link.command(command, &self.clock)?;
        }
        let status = self.link.status(&self.clock)?;
        self.stream = Some(Stream {
            lead_in: framer.lead_in_ticks(),
            character: character.0,
            tail: framer.tail_ticks(),
            batch: VecDeque::new(),
            handed: 0,
            free: status.free,
            started: 0,
            since: None,
            released_at: None,
        });
        Ok(Flow::Continue)
    }

    fn send(&mut self, code: TxCode, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        if cancel.load(Ordering::Acquire) {
            return Ok(Flow::Cancelled);
        }
        let Some(stream) = self.stream.as_mut() else {
            return Ok(Flow::Cancelled);
        };
        match code {
            TxCode::Character(word) => stream.batch.push_back(word),
            TxCode::DisableDiddle | TxCode::EnableDiddle => return Ok(Flow::Continue),
            TxCode::HoldMark | TxCode::CarrierOff => {
                return Err(KeyerError::Framing(
                    "the terminal unit cannot hold the line between characters".to_owned(),
                ));
            }
        }
        if stream.batch.len() >= CODES_PER_SEND {
            self.flush()?;
            self.advance();
        }
        Ok(Flow::Continue)
    }

    fn idle(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        if cancel.load(Ordering::Acquire) {
            return Ok(Flow::Cancelled);
        }
        self.flush()?;
        let Some(status) = self.poll(cancel)? else {
            return Ok(Flow::Cancelled);
        };
        if !status.ptt {
            return Err(cut_short(status));
        }
        Ok(Flow::Continue)
    }

    fn finish(&mut self, cancel: &AtomicBool) -> Result<Flow, KeyerError> {
        if cancel.load(Ordering::Acquire) {
            return Ok(Flow::Cancelled);
        }
        self.flush()?;
        if let Some(stream) = self.stream.as_mut()
            && stream.batch.is_empty()
            && stream.released_at.is_none()
        {
            self.link.command("PTT OFF", &self.clock)?;
            stream.released_at = Some(self.clock.now());
        }
        let Some(status) = self.poll(cancel)? else {
            return Ok(Flow::Cancelled);
        };
        if status.ptt {
            return Ok(Flow::Pending);
        }
        let Some(stream) = &self.stream else {
            return Ok(Flow::Cancelled);
        };
        if stream.released_at.is_none() || status.last != Last::Done {
            return Err(cut_short(status));
        }
        self.progress.elapsed = ticks_to_duration(stream.end());
        Ok(Flow::Continue)
    }

    fn rest(&mut self) -> Result<(), KeyerError> {
        self.stream = None;
        self.link.abort(&self.clock)
    }

    fn progress(&self) -> Progress {
        self.progress
    }
}

/// One stream as far as the unit has been handed it.
///
/// Every character takes as long as every other, since the unit keys neither
/// a gap nor a parity bit, so the timeline is a matter of counting: the
/// lead-in, then one character's length for each code the unit has started.
struct Stream {
    lead_in: f64,
    character: f64,
    tail: f64,
    batch: VecDeque<u8>,
    handed: usize,
    free: usize,
    started: usize,
    since: Option<Duration>,
    released_at: Option<Duration>,
}

impl Stream {
    fn observe(&mut self, status: UnitStatus, now: Duration) {
        self.free = status.free;
        let started = status.sent.min(self.handed);
        if status.ptt && started > self.started {
            self.started = started;
            self.since = Some(now);
        }
    }

    /// How far the stream has gone out at `now`, never ahead of the unit.
    ///
    /// What is being keyed, the lead-in or the latest character, is counted
    /// from when the keyer learned that it had begun, which is no earlier
    /// than when it did. The tail follows the last character once PTT has
    /// been asked off.
    fn elapsed(&self, now: Duration) -> Duration {
        let Some(since) = self.since else {
            return Duration::ZERO;
        };
        let (position, length) = match self.started {
            0 => (0.0, self.lead_in),
            started => (self.lead_in + (started - 1) as f64 * self.character, self.character),
        };
        let length = ticks_to_duration(length);
        let mut elapsed = ticks_to_duration(position) + now.saturating_sub(since).min(length);
        if let Some(released_at) = self.released_at
            && self.batch.is_empty()
            && self.started == self.handed
        {
            let tail_since = released_at.max(since + length);
            elapsed += now.saturating_sub(tail_since).min(ticks_to_duration(self.tail));
        }
        elapsed
    }

    fn end(&self) -> f64 {
        self.lead_in + (self.handed + self.batch.len()) as f64 * self.character + self.tail
    }
}

#[derive(Default)]
struct Length(f64);

impl Extend<(Keying, f64)> for Length {
    fn extend<I: IntoIterator<Item = (Keying, f64)>>(&mut self, elements: I) {
        self.0 += elements.into_iter().map(|(_, ticks)| ticks).sum::<f64>();
    }
}

/// How the unit's latest transmission ended, as `STATUS` reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Last {
    None,
    Done,
    Abort,
    Tot,
}

/// What the unit's `STATUS` reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UnitStatus {
    ptt: bool,
    free: usize,
    sent: usize,
    last: Last,
}

impl UnitStatus {
    fn parse(text: &str) -> Option<Self> {
        let field = |name: &str| {
            text.split_whitespace()
                .find_map(|field| field.strip_prefix(name)?.strip_prefix('='))
        };
        Some(Self {
            ptt: match field("PTT")? {
                "ON" => true,
                "OFF" => false,
                _ => return None,
            },
            free: field("FREE")?.parse().ok()?,
            sent: field("SENT")?.parse().ok()?,
            last: match field("LAST")? {
                "NONE" => Last::None,
                "DONE" => Last::Done,
                "ABORT" => Last::Abort,
                "TOT" => Last::Tot,
                _ => return None,
            },
        })
    }
}

/// The control port, read a line at a time.
///
/// An answer is matched to its command by order alone, so a link that has
/// missed one is `unsure` of every answer after it until it has been
/// resynchronized.
struct Link<P> {
    port: P,
    received: Vec<u8>,
    unsure: bool,
}

impl<P: Read + Write> Link<P> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), KeyerError> {
        self.port
            .write_all(bytes)
            .and_then(|()| self.port.flush())
            .map_err(|error| KeyerError::Unit(format!("cannot write to it: {error}")))
    }

    fn request(&mut self, command: &str, clock: &impl Clock) -> Result<Result<String, String>, KeyerError> {
        self.write(format!("{command}\n").as_bytes())?;
        self.answer(command, clock)
    }

    fn command(&mut self, command: &str, clock: &impl Clock) -> Result<String, KeyerError> {
        self.request(command, clock)?
            .map_err(|reason| refused(command, &reason))
    }

    fn status(&mut self, clock: &impl Clock) -> Result<UnitStatus, KeyerError> {
        let answer = self.command("STATUS", clock)?;
        UnitStatus::parse(&answer).ok_or_else(|| {
            self.unsure = true;
            KeyerError::Unit(format!("unreadable STATUS: {answer}"))
        })
    }

    fn abort(&mut self, clock: &impl Clock) -> Result<(), KeyerError> {
        if self.unsure {
            return self.resync(clock).map(drop);
        }
        self.write(&[ABORT])?;
        self.answer("Ctrl-C", clock)?
            .map(drop)
            .map_err(|reason| refused("Ctrl-C", &reason))
    }

    /// Aborts, throws away everything the unit says until it falls silent,
    /// and asks its version, which only a unit answering this very question
    /// gives.
    fn resync(&mut self, clock: &impl Clock) -> Result<String, KeyerError> {
        self.write(&[ABORT])?;
        let deadline = clock.now() + REPLY_TIMEOUT;
        let mut buffer = [0; 256];
        while clock.now() < deadline {
            match self.port.read(&mut buffer) {
                Ok(0) => return Err(KeyerError::Unit("the port was closed".to_owned())),
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if is_silence(&error) => break,
                Err(error) => return Err(KeyerError::Unit(format!("cannot read from it: {error}"))),
            }
        }
        self.received.clear();
        self.unsure = false;
        let version = self.command("VER", clock)?;
        if version.is_empty() {
            self.unsure = true;
            return Err(KeyerError::Unit("VER was answered without a version".to_owned()));
        }
        Ok(version)
    }

    fn answer(&mut self, command: &str, clock: &impl Clock) -> Result<Result<String, String>, KeyerError> {
        let deadline = clock.now() + REPLY_TIMEOUT;
        loop {
            while let Some(end) = self.received.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.received.drain(..=end).collect();
                if let Some(answer) = parse_answer(&String::from_utf8_lossy(&line)) {
                    return Ok(answer);
                }
            }
            if clock.now() >= deadline {
                self.unsure = true;
                return Err(KeyerError::Unit(format!("no answer to {command}")));
            }
            let mut buffer = [0; 256];
            match self.port.read(&mut buffer) {
                Ok(0) => return Err(KeyerError::Unit("the port was closed".to_owned())),
                Ok(read) => self.received.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == ErrorKind::Interrupted || is_silence(&error) => {}
                Err(error) => return Err(KeyerError::Unit(format!("cannot read from it: {error}"))),
            }
        }
    }
}

fn is_silence(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

/// Reads a line from the unit as an answer, `OK` or `ERR` and what follows.
///
/// Every other line is one of the unit's tagged lines, a log or an echo, and
/// answers nothing.
fn parse_answer(line: &str) -> Option<Result<String, String>> {
    let line = line.trim_end_matches(['\r', '\n']);
    let answer = |word: &str| {
        let rest = line.strip_prefix(word)?;
        if rest.is_empty() {
            Some(String::new())
        } else {
            rest.strip_prefix(' ').map(str::to_owned)
        }
    };
    answer("OK").map(Ok).or_else(|| answer("ERR").map(Err))
}

/// The `SET` commands that frame a stream on the unit as `config` does, or
/// why the unit cannot.
///
/// Diddle is turned off: a stream is keyed from codes that already carry
/// their shifts, and a LTRS the unit sent on its own while its queue was
/// empty would move the receiver out of figures.
fn settings(config: &TxConfig) -> Result<Vec<String>, KeyerError> {
    let refuse = |what: &str| Err(KeyerError::Framing(format!("the terminal unit {what}")));
    let framing = &config.framing;
    if framing.bits.bits() != 5 {
        return refuse("keys five-bit codes only");
    }
    if framing.parity != Parity::None {
        return refuse("sends no parity bit");
    }
    if config.char_gap_bits != 0.0 {
        return refuse("leaves no gap between characters");
    }
    let millibaud = (framing.baud.bits_per_second() * 1_000.0).round();
    if !(MILLIBAUD_RANGE.0..=MILLIBAUD_RANGE.1).contains(&millibaud) {
        return refuse("keys from 10 to 1200 baud");
    }
    let millibaud = millibaud as u32;
    let milliseconds = |seconds: f64| {
        let milliseconds = (seconds * 1_000.0).round();
        (0.0..=MAXIMUM_HOLD_MILLISECONDS)
            .contains(&milliseconds)
            .then_some(milliseconds as u32)
    };
    let (Some(delay), Some(tail)) = (milliseconds(config.lead_in_seconds), milliseconds(config.tail_seconds)) else {
        return refuse("holds mark for at most 65.535 s around a stream");
    };
    Ok(vec![
        "SET BITS 5".to_owned(),
        format!("SET BAUD {}.{:03}", millibaud / 1_000, millibaud % 1_000),
        format!("SET STOP {}", framing.stop.name()),
        format!("SET TXDELAY {delay}"),
        format!("SET TXTAIL {tail}"),
        "SET DIDDLE OFF".to_owned(),
    ])
}

/// Writes codes as `SENDM` reads them: a word's uppermost bit is the first
/// on the line, which is how a [`TxCode`] holds a character.
fn send_command<'a>(words: impl IntoIterator<Item = &'a u8>) -> String {
    let mut command = String::from("SENDM ");
    for word in words {
        let _ = write!(command, "{word:02X}");
    }
    command
}

fn refused(command: &str, reason: &str) -> KeyerError {
    KeyerError::Unit(format!("{command} was refused: {reason}"))
}

fn cut_short(status: UnitStatus) -> KeyerError {
    let how = match status.last {
        Last::Tot => "its TOT ran out",
        Last::Abort => "it was aborted",
        Last::Done | Last::None => "it released PTT",
    };
    KeyerError::Unit(format!("the stream was cut short: {how}"))
}

#[cfg(test)]
mod tests {
    use std::{
        io, mem,
        sync::{Arc, Mutex},
        thread,
        time::Instant,
    };

    use grayline_rtty::{BaudRate, BitLength, TxFraming, encode_text};
    use rstest::rstest;

    use super::*;
    use crate::{
        player::{self, Shared},
        session::{ConnectionId, Phase, StopReason},
        stream::{CodeSource, FixedSource, Next, Outcome, key_stream},
        test_util::FakeClock,
    };

    const MICROSECOND: Duration = Duration::from_micros(1);

    #[derive(Debug)]
    struct Model {
        line: Vec<u8>,
        output: VecDeque<u8>,
        commands: Vec<String>,
        mute: bool,
        withhold: Option<&'static str>,
        withheld: Vec<u8>,
        capacity: usize,
        refusals: usize,
        queue: VecDeque<u8>,
        sent: Vec<(Duration, u8)>,
        counted: usize,
        last: &'static str,
        requested: bool,
        keyed_at: Option<Duration>,
        keyed_from: Option<Duration>,
        tail_from: Option<Duration>,
        next_start: Duration,
        character: Duration,
        delay: Duration,
        tail: Duration,
        tot: Option<Duration>,
        baud: f64,
        stop_bits: f64,
    }

    impl Default for Model {
        fn default() -> Self {
            Self {
                line: Vec::new(),
                output: VecDeque::new(),
                commands: Vec::new(),
                mute: false,
                withhold: None,
                withheld: Vec::new(),
                capacity: 8_192,
                refusals: 0,
                queue: VecDeque::new(),
                sent: Vec::new(),
                counted: 0,
                last: "NONE",
                requested: false,
                keyed_at: None,
                keyed_from: None,
                tail_from: None,
                next_start: Duration::ZERO,
                character: Duration::from_secs_f64(7.5 / 45.45),
                delay: Duration::from_millis(100),
                tail: Duration::from_millis(50),
                tot: None,
                baud: 45.45,
                stop_bits: 1.5,
            }
        }
    }

    impl Model {
        fn advance(&mut self, now: Duration) {
            let Some(keyed_at) = self.keyed_at else {
                return;
            };
            if self.tot.is_some_and(|tot| now >= keyed_at + tot) {
                self.abort("TOT");
                return;
            }
            while self.next_start <= now {
                let Some(word) = self.queue.pop_front() else {
                    break;
                };
                self.sent.push((self.next_start, word));
                self.counted += 1;
                self.next_start += self.character;
            }
            if !self.queue.is_empty() {
                return;
            }
            if self.requested {
                self.next_start = self.next_start.max(now);
            } else {
                self.tail_from = Some(self.next_start);
                if now >= self.next_start + self.tail {
                    self.keyed_at = None;
                    self.last = "DONE";
                }
            }
        }

        fn abort(&mut self, how: &'static str) {
            self.queue.clear();
            self.requested = false;
            if self.keyed_at.take().is_some() {
                self.last = how;
            }
        }

        /// How much of the stream's timeline has really gone out at `now`.
        fn position(&self, now: Duration) -> Duration {
            let Some(keyed_from) = self.keyed_from else {
                return Duration::ZERO;
            };
            let characters: Duration = self
                .sent
                .iter()
                .filter(|(start, _)| *start >= keyed_from)
                .map(|(start, _)| now.saturating_sub(*start).min(self.character))
                .sum();
            let tail = self
                .tail_from
                .map_or(Duration::ZERO, |from| now.saturating_sub(from).min(self.tail));
            now.saturating_sub(keyed_from).min(self.delay) + characters + tail
        }

        fn set(&mut self, name: &str, value: &str) {
            match name {
                "BAUD" => self.baud = value.parse().unwrap(),
                "STOP" => self.stop_bits = value.parse().unwrap(),
                "TXDELAY" => self.delay = Duration::from_millis(value.parse().unwrap()),
                "TXTAIL" => self.tail = Duration::from_millis(value.parse().unwrap()),
                _ => {}
            }
            self.character = Duration::from_secs_f64((6.0 + self.stop_bits) / self.baud);
        }

        fn run(&mut self, line: &str, now: Duration) -> String {
            self.advance(now);
            self.commands.push(line.to_owned());
            let words: Vec<&str> = line.split_whitespace().collect();
            match words[..] {
                ["SET", name, value] => {
                    self.set(name, value);
                    "OK".to_owned()
                }
                ["SENDM", hex] => {
                    let words: Vec<u8> = hex
                        .as_bytes()
                        .chunks(2)
                        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                        .collect();
                    if self.queue.len() + words.len() > self.capacity {
                        self.refusals += 1;
                        return "ERR queue full".to_owned();
                    }
                    self.queue.extend(words);
                    "OK".to_owned()
                }
                ["PTT", "ON"] => {
                    self.requested = true;
                    if self.keyed_at.is_none() {
                        self.keyed_at = Some(now);
                        self.keyed_from = Some(now);
                        self.tail_from = None;
                        self.counted = 0;
                        self.next_start = now + self.delay;
                    }
                    self.output.extend(b"LOG PTT on\r\n");
                    "OK".to_owned()
                }
                ["PTT", "OFF"] => {
                    self.requested = false;
                    "OK".to_owned()
                }
                ["STATUS"] => format!(
                    "OK PTT={} MODE=COMMAND QUEUED={} FREE={} SENT={} LAST={}",
                    if self.keyed_at.is_some() { "ON" } else { "OFF" },
                    self.queue.len(),
                    self.capacity - self.queue.len(),
                    self.counted,
                    self.last,
                ),
                ["VER"] => "OK 0.1.0".to_owned(),
                _ => "ERR unknown command".to_owned(),
            }
        }

        fn answer(&mut self, answer: &str) {
            if !self.mute {
                self.output.extend(answer.as_bytes());
                self.output.extend(b"\r\n");
            }
        }
    }

    #[derive(Clone, Debug)]
    struct FakeUnit {
        clock: FakeClock,
        model: Arc<Mutex<Model>>,
    }

    impl FakeUnit {
        fn new(clock: FakeClock, model: Model) -> Self {
            Self {
                clock,
                model: Arc::new(Mutex::new(model)),
            }
        }

        fn with<R>(&self, f: impl FnOnce(&mut Model) -> R) -> R {
            let mut model = self.model.lock().unwrap();
            model.advance(self.clock.now());
            f(&mut model)
        }

        fn words(&self) -> Vec<u8> {
            self.with(|model| model.sent.iter().map(|(_, word)| *word).collect())
        }
    }

    impl Write for FakeUnit {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let now = self.clock.now();
            let mut model = self.model.lock().unwrap();
            let late = mem::take(&mut model.withheld);
            model.output.extend(late);
            for &byte in bytes {
                match byte {
                    ABORT => {
                        model.advance(now);
                        model.line.clear();
                        model.abort("ABORT");
                        model.commands.push("^C".to_owned());
                        model.answer("OK");
                    }
                    b'\n' => {
                        let line = String::from_utf8(mem::take(&mut model.line)).unwrap();
                        let answer = model.run(&line, now);
                        if model.withhold.is_some_and(|command| line.starts_with(command)) {
                            model.withhold = None;
                            model.withheld = format!("{answer}\r\n").into_bytes();
                        } else {
                            model.answer(&answer);
                        }
                    }
                    _ => model.line.push(byte),
                }
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for FakeUnit {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let mut model = self.model.lock().unwrap();
            if model.output.is_empty() {
                drop(model);
                self.clock.advance(READ_TIMEOUT);
                return Err(io::Error::from(ErrorKind::TimedOut));
            }
            let read = buffer.len().min(model.output.len());
            for (slot, byte) in buffer.iter_mut().zip(model.output.drain(..read)) {
                *slot = byte;
            }
            Ok(read)
        }
    }

    /// A fixed stream that checks every report of progress against what the
    /// simulated unit has really keyed.
    struct Watched<'a> {
        inner: FixedSource,
        fake: &'a FakeUnit,
        reports: Vec<(Duration, Duration)>,
    }

    impl<'a> Watched<'a> {
        fn new(codes: Vec<TxCode>, fake: &'a FakeUnit) -> Self {
            Self {
                inner: FixedSource::new(codes),
                fake,
                reports: Vec::new(),
            }
        }
    }

    impl CodeSource for Watched<'_> {
        fn next(&mut self, now: Duration, ready: bool) -> Next {
            self.inner.next(now, ready)
        }

        fn progress(&mut self, sent: usize, progress: Progress) {
            let truth = self.fake.with(|model| model.position(self.fake.clock.now()));
            self.reports.push((progress.elapsed, truth));
            self.inner.progress(sent, progress);
        }
    }

    fn unit(model: Model) -> (UnitMethod<FakeUnit, FakeClock>, FakeUnit, FakeClock) {
        let clock = FakeClock::default();
        let fake = FakeUnit::new(clock.clone(), model);
        let method = UnitMethod::new(fake.clone(), clock.clone()).unwrap();
        (method, fake, clock)
    }

    fn characters(codes: &[TxCode]) -> Vec<u8> {
        codes
            .iter()
            .filter_map(|code| match code {
                TxCode::Character(word) => Some(*word),
                _ => None,
            })
            .collect()
    }

    fn timeline(config: &TxConfig, characters: usize) -> Duration {
        let bits = 7.0 + config.framing.stop.extra_bits();
        Duration::from_secs_f64(
            config.lead_in_seconds
                + characters as f64 * bits / config.framing.baud.bits_per_second()
                + config.tail_seconds,
        )
    }

    fn close(left: Duration, right: Duration) -> bool {
        left.abs_diff(right) <= MICROSECOND
    }

    #[test]
    fn a_stream_is_set_up_handed_over_and_followed_to_its_tail() {
        let (mut method, fake, _) = unit(Model::default());
        let config = TxConfig::default();
        let codes = encode_text("RYRY DE JL1HIS", &config).unwrap();
        let mut source = FixedSource::new(codes.clone());
        let cancel = AtomicBool::new(false);
        let outcome = key_stream(&mut method, &config, &mut source, &cancel).unwrap();
        assert_eq!(outcome, Outcome::Completed);
        assert_eq!(method.version(), "0.1.0");

        let commands = fake.with(|model| model.commands.clone());
        assert_eq!(
            commands[..11],
            [
                "^C",
                "VER",
                "SET BITS 5",
                "SET BAUD 45.450",
                "SET STOP 1.5",
                "SET TXDELAY 500",
                "SET TXTAIL 500",
                "SET DIDDLE OFF",
                "STATUS",
                &send_command(&characters(&codes)),
                "PTT ON",
            ]
        );
        assert_eq!(commands.iter().filter(|command| *command == "PTT OFF").count(), 1);
        assert_eq!(commands.last().map(String::as_str), Some("^C"));

        let (sent, keyed, keyed_from) = fake.with(|model| (model.sent.clone(), model.keyed_at, model.keyed_from));
        assert_eq!(keyed, None);
        assert_eq!(fake.words(), characters(&codes));
        assert_eq!(sent[0].0, keyed_from.unwrap() + Duration::from_millis(500));
        assert!(close(source.last_progress().elapsed, timeline(&config, sent.len())));
        assert_eq!(source.last_progress().late, Duration::ZERO);
    }

    #[test]
    fn a_code_reaches_the_unit_with_its_first_bit_uppermost() {
        let (mut method, fake, _) = unit(Model::default());
        let cancel = AtomicBool::new(false);
        method.begin(&TxConfig::default(), &cancel).unwrap();
        let e = encode_text("E", &TxConfig::default()).unwrap();
        assert_eq!(e.last(), Some(&TxCode::Character(0b10000)));
        method.send(TxCode::Character(0b10000), &cancel).unwrap();
        method.send(TxCode::Character(0b00001), &cancel).unwrap();
        method.idle(&cancel).unwrap();
        assert!(fake.with(|model| model.commands.contains(&"SENDM 1001".to_owned())));
    }

    #[test]
    fn progress_follows_the_unit_while_it_keys_and_never_leads_it() {
        let (mut method, fake, _) = unit(Model::default());
        let config = TxConfig::default();
        let codes = encode_text("THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG", &config).unwrap();
        let mut source = Watched::new(codes.clone(), &fake);
        let cancel = AtomicBool::new(false);
        let outcome = key_stream(&mut method, &config, &mut source, &cancel).unwrap();
        assert_eq!(outcome, Outcome::Completed);
        let whole = timeline(&config, codes.len());
        assert!(source.reports.len() as u128 > whole.as_millis() / 25);
        for (reported, truth) in &source.reports {
            assert!(*reported <= *truth + MICROSECOND, "{reported:?} leads {truth:?}");
            assert!(
                *truth <= *reported + STATUS_INTERVAL + MICROSECOND,
                "{reported:?} trails {truth:?}"
            );
        }
        assert!(close(source.reports.last().unwrap().0, whole));
    }

    #[test]
    fn a_stream_that_runs_dry_is_not_believed_ahead_of_the_unit() {
        let (mut method, fake, clock) = unit(Model::default());
        let config = TxConfig::default();
        let cancel = AtomicBool::new(false);
        method.begin(&config, &cancel).unwrap();
        method.send(TxCode::Character(0b11111), &cancel).unwrap();
        let started = clock.now();
        while clock.now() < started + Duration::from_secs(3) {
            assert_eq!(method.idle(&cancel).unwrap(), Flow::Continue);
            let truth = fake.with(|model| model.position(clock.now()));
            assert!(method.progress().elapsed <= truth + MICROSECOND);
        }
        let keyed = timeline(&config, 1) - Duration::from_secs_f64(config.tail_seconds);
        assert!(close(method.progress().elapsed, keyed));

        method.send(TxCode::Character(0b11111), &cancel).unwrap();
        while method.finish(&cancel).unwrap() == Flow::Pending {
            let truth = fake.with(|model| model.position(clock.now()));
            assert!(method.progress().elapsed <= truth + MICROSECOND);
        }
        assert!(close(method.progress().elapsed, timeline(&config, 2)));
    }

    #[test]
    fn a_stream_longer_than_the_queue_waits_for_room() {
        let (mut method, fake, _) = unit(Model {
            capacity: 8,
            ..Model::default()
        });
        let config = TxConfig {
            lead_in_seconds: 0.0,
            tail_seconds: 0.0,
            ..TxConfig::default()
        };
        let codes: Vec<TxCode> = (0..600_u16)
            .map(|index| TxCode::Character((index % 32) as u8))
            .collect();
        let mut source = Watched::new(codes.clone(), &fake);
        let cancel = AtomicBool::new(false);
        let outcome = key_stream(&mut method, &config, &mut source, &cancel).unwrap();
        assert_eq!(outcome, Outcome::Completed);
        assert_eq!(fake.words(), characters(&codes));
        assert_eq!(fake.with(|model| model.refusals), 0);
        for (reported, truth) in &source.reports {
            assert!(*reported <= *truth + MICROSECOND, "{reported:?} leads {truth:?}");
        }
    }

    #[test]
    fn a_unit_whose_tot_runs_out_fails_the_stream() {
        let (mut method, fake, _) = unit(Model {
            tot: Some(Duration::from_secs(1)),
            ..Model::default()
        });
        let config = TxConfig::default();
        let codes = encode_text("THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG", &config).unwrap();
        let mut source = FixedSource::new(codes);
        let cancel = AtomicBool::new(false);
        let result = key_stream(&mut method, &config, &mut source, &cancel);
        assert!(
            matches!(&result, Err(KeyerError::Unit(detail)) if detail.contains("TOT")),
            "{result:?}"
        );
        assert_eq!(fake.with(|model| model.keyed_at), None);
    }

    #[test]
    fn a_tot_that_runs_out_in_the_tail_is_not_a_stream_gone_out_whole() {
        let (mut method, fake, _) = unit(Model {
            tot: Some(Duration::from_secs(1)),
            ..Model::default()
        });
        let config = TxConfig::default();
        let mut source = FixedSource::new([TxCode::Character(0b01010), TxCode::Character(0b10101)]);
        let cancel = AtomicBool::new(false);
        let result = key_stream(&mut method, &config, &mut source, &cancel);
        assert!(
            matches!(&result, Err(KeyerError::Unit(detail)) if detail.contains("TOT")),
            "{result:?}"
        );
        assert_eq!(fake.with(|model| model.sent.len()), 2);
    }

    #[test]
    fn the_time_limit_stops_a_stream_the_unit_is_still_keying() {
        const OWNER: ConnectionId = ConnectionId(1);
        let (mut method, fake, _) = unit(Model::default());
        let config = TxConfig::default();
        let codes = encode_text("THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG", &config).unwrap();
        let shared = Arc::new(Shared::new(method.keys_ptt()));
        assert!(shared.keys_ptt());
        shared.update(|session| {
            session.open(OWNER, config).unwrap();
            session.append(OWNER, &codes).unwrap();
            session.end(OWNER).unwrap();
        });
        let playing = Arc::clone(&shared);
        let player = thread::spawn(move || {
            player::run(&mut method, &playing, Some(Duration::from_secs(2)), |_| {});
        });
        let started = Instant::now();
        while shared.update(|session| session.status().phase) != Phase::Stopped(StopReason::TimedOut) {
            assert!(started.elapsed() < Duration::from_secs(5), "never timed out");
            thread::sleep(Duration::from_millis(1));
        }
        shared.shut_down();
        player.join().unwrap();
        let (keyed, last, sent) = fake.with(|model| (model.keyed_at, model.last, model.sent.len()));
        assert_eq!((keyed, last), (None, "ABORT"));
        assert!(sent < codes.len());
    }

    #[test]
    fn a_cancelled_stream_is_aborted_at_the_unit() {
        let (mut method, fake, _) = unit(Model::default());
        let cancel = AtomicBool::new(false);
        method.begin(&TxConfig::default(), &cancel).unwrap();
        for word in 0..10 {
            method.send(TxCode::Character(word), &cancel).unwrap();
        }
        method.idle(&cancel).unwrap();
        cancel.store(true, Ordering::Release);
        assert_eq!(method.idle(&cancel).unwrap(), Flow::Cancelled);
        method.rest().unwrap();
        let (queued, keyed, last) =
            fake.with(|model| (model.queue.len(), model.keyed_at, model.commands.last().cloned()));
        assert_eq!((queued, keyed), (0, None));
        assert_eq!(last.as_deref(), Some("^C"));
    }

    #[test]
    fn a_late_answer_fails_its_stream_and_not_the_one_after() {
        let (mut method, fake, _) = unit(Model::default());
        fake.with(|model| model.withhold = Some("STATUS"));
        let config = TxConfig::default();
        let codes = encode_text("RYRY", &config).unwrap();
        let cancel = AtomicBool::new(false);
        let mut first = FixedSource::new(codes.clone());
        let result = key_stream(&mut method, &config, &mut first, &cancel);
        assert!(matches!(result, Err(KeyerError::Unit(_))), "{result:?}");
        assert_eq!(fake.words(), Vec::<u8>::new());

        let mut second = FixedSource::new(codes.clone());
        let outcome = key_stream(&mut method, &config, &mut second, &cancel).unwrap();
        assert_eq!(outcome, Outcome::Completed);
        assert_eq!(fake.words(), characters(&codes));
    }

    #[rstest]
    #[case::parity(TxConfig { framing: TxFraming { parity: Parity::Odd, ..TxConfig::default().framing }, ..TxConfig::default() })]
    #[case::eight_bits(TxConfig { framing: TxFraming { bits: BitLength::new(8).unwrap(), ..TxConfig::default().framing }, ..TxConfig::default() })]
    #[case::gap(TxConfig { char_gap_bits: 1.0, ..TxConfig::default() })]
    #[case::fast(TxConfig { framing: TxFraming { baud: BaudRate::new(1_500.0).unwrap(), ..TxConfig::default().framing }, ..TxConfig::default() })]
    #[case::long_lead_in(TxConfig { lead_in_seconds: 70.0, ..TxConfig::default() })]
    fn framing_the_unit_cannot_key_is_refused_before_anything_is_set(#[case] config: TxConfig) {
        let (mut method, fake, _) = unit(Model::default());
        let result = method.begin(&config, &AtomicBool::new(false));
        assert!(matches!(result, Err(KeyerError::Framing(_))), "{result:?}");
        assert!(fake.with(|model| model.commands.iter().all(|command| !command.starts_with("SET"))));
    }

    #[test]
    fn a_held_mark_cannot_be_handed_to_the_unit() {
        let (mut method, _, _) = unit(Model::default());
        let cancel = AtomicBool::new(false);
        method.begin(&TxConfig::default(), &cancel).unwrap();
        assert_eq!(method.send(TxCode::DisableDiddle, &cancel), Ok(Flow::Continue));
        assert!(matches!(
            method.send(TxCode::HoldMark, &cancel),
            Err(KeyerError::Framing(_))
        ));
    }

    #[test]
    fn a_port_that_does_not_answer_is_not_taken_for_a_unit() {
        let clock = FakeClock::default();
        let fake = FakeUnit::new(
            clock.clone(),
            Model {
                mute: true,
                ..Model::default()
            },
        );
        let result = UnitMethod::new(fake, clock);
        assert!(matches!(result, Err(KeyerError::Unit(_))));
    }

    #[rstest]
    #[case("OK\r\n", Some(Ok(String::new())))]
    #[case("OK 0.1.0\r\n", Some(Ok("0.1.0".to_owned())))]
    #[case("ERR queue full\r\n", Some(Err("queue full".to_owned())))]
    #[case("LOG PTT on\r\n", None)]
    #[case("OKAY\n", None)]
    fn only_ok_and_err_lines_answer(#[case] line: &str, #[case] answer: Option<Result<String, String>>) {
        assert_eq!(parse_answer(line), answer);
    }

    #[rstest]
    #[case("PTT=ON MODE=COMMAND QUEUED=12 FREE=8180 SENT=34 LAST=NONE", Some(UnitStatus { ptt: true, free: 8_180, sent: 34, last: Last::None }))]
    #[case("PTT=OFF MODE=KEYBOARD QUEUED=0 FREE=8192 SENT=46 LAST=TOT", Some(UnitStatus { ptt: false, free: 8_192, sent: 46, last: Last::Tot }))]
    #[case("PTT=OFF MODE=COMMAND QUEUED=0 FREE=8192 SENT=0 LAST=LATER", None)]
    #[case("PTT=MAYBE QUEUED=0 FREE=8192 SENT=0 LAST=DONE", None)]
    #[case("PTT=ON MODE=COMMAND QUEUED=12 FREE=8180", None)]
    fn status_is_read_by_its_fields(#[case] text: &str, #[case] status: Option<UnitStatus>) {
        assert_eq!(UnitStatus::parse(text), status);
    }
}
