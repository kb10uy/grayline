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

/// One request the stand-in served.
#[derive(Clone, Debug)]
pub struct Asked {
    /// The request target, query string and all.
    pub path: String,
    /// The request body, which is empty for a GET.
    pub body: String,
    headers: Vec<(String, String)>,
}

impl Asked {
    /// One header, found however the client happened to capitalize it.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(sent, _)| sent.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
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
    requested: Arc<Mutex<Vec<Asked>>>,
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
                let Some(asked) = read_request(&stream) else {
                    return;
                };
                recorder.lock().expect("the recorder").push(asked);

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

    /// A client holding a v1 key, which is what most installations answer.
    pub fn client(&self) -> Wavelog {
        self.client_with_key("secret")
    }

    /// A client holding whatever key the test wants, which is how it chooses
    /// between the two APIs.
    pub fn client_with_key(&self, key: &str) -> Wavelog {
        Wavelog::new(&self.base, key, TEST_TIMEOUT).expect("a client")
    }

    /// Every request served so far.
    pub fn requested(&self) -> Vec<Asked> {
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

fn read_request(stream: &std::net::TcpStream) -> Option<Asked> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let path = start.split(' ').nth(1).unwrap_or_default().to_owned();

    let mut headers = Vec::new();
    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim().to_owned(), value.trim().to_owned());
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().unwrap_or(0);
        }
        headers.push((name, value));
    }

    let mut body = vec![0; length];
    let body = match reader.read_exact(&mut body) {
        Ok(()) => String::from_utf8_lossy(&body).into_owned(),
        Err(_) => String::new(),
    };
    Some(Asked { path, body, headers })
}
