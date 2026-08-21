//! What more than one module's tests need, and nothing ships.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::wavelog::Wavelog;

/// How long a test gives a stand-in that is answering from the same machine.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A directory that removes itself, so a test that writes a real store leaves
/// nothing behind it.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after 1970")
            .as_nanos();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("grayline-qso-{stamp}-{unique}"));
        fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn store(&self) -> PathBuf {
        self.0.join("contacts.sqlite3")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A stand-in for a Wavelog installation, over plain HTTP.
///
/// Plain HTTP because what is under test is the request, the paths and the
/// mapping; TLS is the transport's business and a certificate would only make
/// the test harder to run than the thing it proves is worth.
pub struct FakeWavelog {
    address: SocketAddr,
    base: String,
    requested: Arc<Mutex<Vec<(String, String)>>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeWavelog {
    /// Serves one canned answer per request, in the order given.
    ///
    /// A request past the end of the list is never accepted, which is what a
    /// test asserting that no further request was made relies on.
    pub fn spawn(answers: &[(u16, &str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("an address");
        let requested = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&requested);
        let stopping = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stopping);
        let answers: Vec<(u16, String)> = answers
            .iter()
            .map(|(status, body)| (*status, (*body).to_owned()))
            .collect();

        let thread = thread::spawn(move || {
            for (status, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                // The connection that ends the test is the one `Drop` makes to
                // get this thread off `accept`, and it carries no request.
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                let Some((path, sent)) = read_request(&stream) else {
                    return;
                };
                recorder.lock().expect("the recorder").push((path, sent));

                let answer = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(answer.as_bytes());
                let _ = stream.flush();
            }
        });

        Self {
            address,
            base: format!("http://{address}"),
            requested,
            stopping,
            thread: Some(thread),
        }
    }

    pub fn client(&self) -> Wavelog {
        Wavelog::new(&self.base, "secret", TEST_TIMEOUT).expect("a client")
    }

    /// Every request served so far, as its path and its body.
    pub fn requested(&self) -> Vec<(String, String)> {
        self.requested.lock().expect("the recorder").clone()
    }
}

impl Drop for FakeWavelog {
    fn drop(&mut self) {
        // A test that asserts nothing was asked leaves the thread waiting on a
        // connection that will never come, so one is made to release it.
        self.stopping.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &std::net::TcpStream) -> Option<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split(' ').nth(1).unwrap_or_default().to_owned();

    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0; length];
    let body = match reader.read_exact(&mut body) {
        Ok(()) => String::from_utf8_lossy(&body).into_owned(),
        Err(_) => String::new(),
    };
    Some((path, body))
}
