use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crate::{
    Logger,
    error::KeyerError,
    player::Shared,
    protocol::{self, Command, Reply, code},
    session::{ConnectionId, Refusal, StopReason},
};

/// The longest line a client may send before the connection is dropped.
const MAXIMUM_LINE_BYTES: u64 = 16_384;

/// Binds the address clients reach the keyer on.
pub fn bind(address: &str) -> Result<TcpListener, KeyerError> {
    TcpListener::bind(address).map_err(|error| KeyerError::Listen {
        address: address.to_owned(),
        detail: error.to_string(),
    })
}

/// Serves every client that connects, each on a thread of its own.
pub fn serve(listener: TcpListener, shared: Arc<Shared>, log: Logger) {
    let next_id = AtomicU64::new(1);
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                log(&format!("accept failed: {error}"));
                continue;
            }
        };
        let id = ConnectionId(next_id.fetch_add(1, Ordering::Relaxed));
        let shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name(format!("gl-keyer-client-{}", id.0))
            .spawn(move || connection(stream, id, &shared, log));
        if let Err(error) = spawned {
            log(&format!("cannot serve a client: {error}"));
        }
    }
}

fn connection(stream: TcpStream, id: ConnectionId, shared: &Shared, log: Logger) {
    let peer = stream
        .peer_addr()
        .map_or_else(|_| "an unknown peer".to_owned(), |address| address.to_string());
    log(&format!("{peer} connected"));
    let _ = stream.set_nodelay(true);
    if let Ok(reader) = stream.try_clone() {
        converse(reader, stream, id, shared);
    }
    if shared.update(|session| session.release(id)) {
        shared.cancel();
    }
    log(&format!("{peer} disconnected"));
}

fn converse(reader: TcpStream, mut writer: TcpStream, id: ConnectionId, shared: &Shared) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = String::new();
        let read = match (&mut reader).take(MAXIMUM_LINE_BYTES).read_line(&mut line) {
            Ok(read) => read,
            Err(_) => return,
        };
        if read == 0 {
            return;
        }
        if read as u64 == MAXIMUM_LINE_BYTES && !line.ends_with('\n') {
            let _ = writer.write_all(Reply::refuse("", code::INVALID).render(true).as_bytes());
            return;
        }
        if line.trim().is_empty() {
            continue;
        }
        let request = protocol::parse(&line);
        let reply = match request.command {
            Ok(Command::Quit) => return,
            Ok(command) => execute(&command, id, shared),
            Err(reply) => reply,
        };
        if writer.write_all(reply.render(request.extended).as_bytes()).is_err() {
            return;
        }
    }
}

/// Carries out one command on behalf of connection `id`.
pub fn execute(command: &Command, id: ConnectionId, shared: &Shared) -> Reply {
    let name = command.name();
    let refused = |refusal: Refusal| {
        let code = match refusal {
            Refusal::Full => code::TRUNCATED,
            Refusal::Busy | Refusal::NotOpen | Refusal::NotOwner | Refusal::Ended => code::REJECTED,
        };
        Reply::refuse(name, code)
    };
    let done = |result: Result<(), Refusal>| result.map_or_else(refused, |()| Reply::new(name, Vec::new()));
    match command {
        Command::ChkVfo => Reply::new(name, vec!["ChkVFO: 0".to_owned()]),
        Command::SetPtt(_) | Command::GetPtt if !shared.has_ptt() => Reply::refuse(name, code::NOT_AVAILABLE),
        Command::SetPtt(keyed) => {
            shared.update(|session| session.set_ptt(id, *keyed));
            Reply::new(name, Vec::new())
        }
        Command::GetPtt => {
            let keyed = shared.update(|session| session.ptt());
            Reply::new(name, vec![format!("PTT: {}", u8::from(keyed))])
        }
        Command::RttyOpen(config) => done(shared.update(|session| session.open(id, *config))),
        Command::RttySend(codes) => done(shared.update(|session| session.append(id, codes))),
        Command::RttyEnd => done(shared.update(|session| session.end(id))),
        Command::Status => {
            let status = shared.update(|session| session.status());
            let mut lines = vec![
                format!("State: {}", status.phase.name()),
                format!("PTT: {}", u8::from(status.ptt)),
                format!("Sent: {}", status.sent),
                format!("Queued: {}", status.queued),
                format!("Elapsed: {}", status.progress.elapsed.as_micros()),
                format!("Late: {}", status.progress.late.as_micros()),
            ];
            if let Some(fault) = status.fault {
                lines.push(format!("Error: {fault}"));
            }
            Reply::new(name, lines)
        }
        Command::Stop => {
            if shared.update(|session| session.stop(StopReason::Requested)) {
                shared.cancel();
            }
            Reply::new(name, Vec::new())
        }
        Command::Quit => Reply::new(name, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    use grayline_rig::{Response, RigError, Rigctld};
    use grayline_rtty::{
        TxConfig, encode_text,
        tx::{Framer, Keying},
    };

    use super::*;
    use crate::{
        method::LineMethod,
        player::{self, PttLine},
        protocol::{encode_codes, encode_open},
        test_util::{FakeClock, RecordingLine},
    };

    const TIMEOUT: Duration = Duration::from_secs(5);

    struct Keyer {
        address: String,
        fsk: RecordingLine,
        ptt: RecordingLine,
        shared: Arc<Shared>,
        player: Option<JoinHandle<()>>,
    }

    impl Keyer {
        fn start(has_ptt: bool) -> Self {
            let clock = FakeClock::default();
            let fsk = RecordingLine::new(clock.clone(), Duration::ZERO);
            let ptt = RecordingLine::new(clock.clone(), Duration::ZERO);
            let shared = Arc::new(Shared::new(has_ptt));
            let listener = bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let serving = Arc::clone(&shared);
            thread::spawn(move || serve(listener, serving, |_| {}));
            let playing = Arc::clone(&shared);
            let (fsk_line, ptt_line) = (fsk.clone(), ptt.clone());
            let player = thread::spawn(move || {
                let mut method = LineMethod::new(fsk_line, clock, false).unwrap();
                let mut ptt = PttLine::new(has_ptt.then_some(ptt_line)).unwrap();
                player::run(&mut method, &mut ptt, &playing, None, |_| {});
            });
            Self {
                address,
                fsk,
                ptt,
                shared,
                player: Some(player),
            }
        }

        fn connect(&self) -> Rigctld {
            Rigctld::connect(&self.address, TIMEOUT).unwrap()
        }
    }

    impl Drop for Keyer {
        fn drop(&mut self) {
            self.shared.shut_down();
            if let Some(player) = self.player.take() {
                let _ = player.join();
            }
        }
    }

    fn field(response: &Response, label: &str) -> String {
        response
            .lines()
            .iter()
            .find_map(|line| line.strip_prefix(label)?.strip_prefix(": "))
            .unwrap_or_default()
            .to_owned()
    }

    fn wait_for_state(rig: &mut Rigctld, state: &str) -> Response {
        let started = Instant::now();
        loop {
            let status = rig.run("\\status").unwrap();
            if field(&status, "State") == state {
                return status;
            }
            assert!(started.elapsed() < TIMEOUT, "still {status:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn refusal(result: Result<Response, RigError>) -> i32 {
        match result {
            Err(RigError::Refused { code, .. }) => code,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_stream_sent_through_a_rigctld_client_keys_the_line_in_order() {
        let keyer = Keyer::start(true);
        let mut rig = keyer.connect();
        assert!(!rig.requires_vfo());
        let config = TxConfig::default();
        let codes = encode_text("RYRY DE JL1HIS", &config).unwrap();
        rig.run("T 1").unwrap();
        rig.run(&format!("\\rtty_open {}", encode_open(&config))).unwrap();
        let (first, second) = codes.split_at(5);
        rig.run(&format!("\\rtty_send {}", encode_codes(first))).unwrap();
        rig.run(&format!("\\rtty_send {}", encode_codes(second))).unwrap();
        rig.run("\\rtty_end").unwrap();
        let status = wait_for_state(&mut rig, "completed");
        assert_eq!(field(&status, "Sent"), codes.len().to_string());
        assert_eq!(field(&status, "Queued"), "0");
        rig.run("T 0").unwrap();
        assert_eq!(field(&rig.run("t").unwrap(), "PTT"), "0");

        let mut framer = Framer::new(1_000_000, &config).unwrap();
        let mut elements = Vec::new();
        for code in &codes {
            framer.frame(*code, &mut elements);
        }
        let mut expected = vec![false];
        expected.extend(elements.iter().map(|(keying, _)| *keying == Keying::Space));
        expected.push(false);
        expected.dedup();
        assert_eq!(keyer.fsk.levels(), expected);

        let started = Instant::now();
        while keyer.ptt.levels() != [false, true, false] {
            assert!(started.elapsed() < TIMEOUT, "PTT went {:?}", keyer.ptt.levels());
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_dropped_connection_stops_its_stream_and_unkeys() {
        let keyer = Keyer::start(true);
        let mut owner = keyer.connect();
        owner.run("T 1").unwrap();
        owner.run("\\rtty_open lead_in=0 tail=0").unwrap();
        owner.run("\\rtty_send 0123").unwrap();
        drop(owner);
        let mut watcher = keyer.connect();
        let status = wait_for_state(&mut watcher, "disconnected");
        assert_eq!(field(&status, "PTT"), "0");
        assert_eq!(keyer.fsk.changes().last().map(|(_, level)| *level), Some(false));
    }

    #[test]
    fn a_stream_belongs_to_the_connection_that_opened_it() {
        let keyer = Keyer::start(true);
        let mut owner = keyer.connect();
        let mut other = keyer.connect();
        owner.run("\\rtty_open").unwrap();
        assert_eq!(refusal(other.run("\\rtty_open")), code::REJECTED);
        assert_eq!(refusal(other.run("\\rtty_send 0")), code::REJECTED);
        other.run("\\stop").unwrap();
        wait_for_state(&mut owner, "stopped");
        other.run("\\rtty_open").unwrap();
    }

    #[test]
    fn ptt_without_a_line_is_not_available() {
        let keyer = Keyer::start(false);
        let mut rig = keyer.connect();
        assert_eq!(refusal(rig.run("T 1")), code::NOT_AVAILABLE);
        assert_eq!(refusal(rig.run("\\get_ptt")), code::NOT_AVAILABLE);
        assert_eq!(refusal(rig.run("\\get_freq")), code::NOT_IMPLEMENTED);
    }
}
