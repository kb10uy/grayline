use grayline_logger_link::LoggerSettings;
use grayline_rig_control::{DEFAULT_PORT_NAME, RigControlError, RigError, script::ScriptError};

use std::{
    env, fs,
    io::{BufRead, BufReader, BufWriter, Write},
    net::{TcpListener, UdpSocket},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
    thread,
    time::Duration,
};

/// A directory shared by every headless application in the suite.
///
/// A headless application writes nothing on its own, but it still needs
/// somewhere to say it would have written: pointing it at the source tree
/// would leave files behind.
pub fn scratch_dir() -> PathBuf {
    let path = env::temp_dir().join(format!("grayline-rtty-scratch-{}", std::process::id()));
    fs::create_dir_all(&path).expect("the temporary directory is writable");
    path
}

/// A rig failure in the shape rig control publishes one.
///
/// A refused connection reaches the snapshot as the port the script was
/// loading when the transport gave up, which is what the interface has to
/// print legibly.
pub(crate) fn refused_port() -> RigControlError {
    RigControlError::Script(ScriptError::Port {
        name: DEFAULT_PORT_NAME.to_owned(),
        error: RigError::Connect {
            address: "127.0.0.1:4532".to_owned(),
            detail: "connection refused".to_owned(),
        },
    })
}

/// A stand-in for `gl-keyer` that reports a scripted run of states.
///
/// Each `\status` takes the next of the `(state, elapsed microseconds)`
/// pairs, and the last one repeats, so a test says how a stream goes without a
/// serial port. It serves one connection, which is one message.
pub(crate) struct FakeKeyer {
    pub address: String,
    received: Arc<Mutex<Vec<String>>>,
}

impl FakeKeyer {
    pub fn spawn(states: &[(&'static str, u64)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&received);
        let states = states.to_vec();
        thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut writer = BufWriter::new(stream.try_clone().unwrap());
            let reader = BufReader::new(stream);
            let mut next = 0;
            for request in reader.lines().map_while(Result::ok) {
                let answer = if request.contains("chk_vfo") {
                    "chk_vfo:\nChkVFO: 0\nRPRT 0\n".to_owned()
                } else if request.contains("\\status") {
                    let (state, elapsed) = states[next.min(states.len() - 1)];
                    next += 1;
                    format!("status:\nState: {state}\nSent: 0\nQueued: 0\nElapsed: {elapsed}\nLate: 40\nRPRT 0\n")
                } else {
                    "RPRT 0\n".to_owned()
                };
                recorder.lock().unwrap().push(request);
                if writer.write_all(answer.as_bytes()).is_err() || writer.flush().is_err() {
                    return;
                }
            }
        });
        Self { address, received }
    }

    pub fn received(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
    }
}

/// Where a headless application sends the contact to the logger.
///
/// A socket of the suite's own, held open for the whole run so its port is
/// never handed to anything else, and never read. The default address is the
/// logger's own, and a test that switches the sending on — applying every menu
/// action does — would otherwise fill in the entry form of a logger the
/// operator has running.
pub(crate) fn logger_sink() -> LoggerSettings {
    static SINK: LazyLock<UdpSocket> = LazyLock::new(|| UdpSocket::bind("127.0.0.1:0").unwrap());
    LoggerSettings {
        enabled: false,
        address: SINK.local_addr().unwrap().to_string(),
    }
}

/// A stand-in for the logger, receiving what an application sends it.
pub(crate) struct FakeLogger {
    socket: UdpSocket,
}

impl FakeLogger {
    pub(crate) fn listen() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        Self { socket }
    }

    /// The settings that send to this logger, switched off.
    pub(crate) fn settings(&self) -> LoggerSettings {
        LoggerSettings {
            enabled: false,
            address: self.socket.local_addr().unwrap().to_string(),
        }
    }

    /// The next datagram filling the form in, or `None` once nothing has
    /// arrived for a while.
    ///
    /// A subscription is passed over, so a test reads what the application
    /// filled in without minding when it last renewed.
    pub(crate) fn received(&self) -> Option<String> {
        let mut buffer = [0; 4096];
        loop {
            let (length, _) = self.socket.recv_from(&mut buffer).ok()?;
            let text = String::from_utf8(buffer[..length].to_vec()).unwrap();
            if text.contains(r#""type":"subscribe""#) || text.contains(r#""type":"unsubscribe""#) {
                continue;
            }
            return Some(text);
        }
    }
}
