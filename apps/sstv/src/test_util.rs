use grayline_logger_link::{LoggerSettings, set_message};
use grayline_rig_control::{DEFAULT_PORT_NAME, RigControlError, RigError, script::ScriptError};

use std::{
    cell::Cell,
    fs,
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

/// Where an interface built without directories of its own keeps them.
///
/// [`crate::app::App::headless`] must not hold empty paths: they are relative
/// and therefore resolve against the working directory, which would have the
/// suite write the default band plan and rig script into the package
/// directory every time it ran. One directory is shared by every headless
/// interface because none of them is meant to see anything in it; a test that
/// cares about what is on disk builds a [`tempfile::TempDir`] of its own instead.
///
/// Left behind rather than removed at exit, which no reliable hook covers. It
/// sits under the system's temporary directory, which is where a file nobody
/// removes belongs.
pub(crate) fn scratch_dir() -> &'static Path {
    static SCRATCH: LazyLock<PathBuf> = LazyLock::new(|| {
        let path = std::env::temp_dir().join(format!("grayline-sstv-headless-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    });
    &SCRATCH
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

/// A stand-in for the logger, receiving what an application sends it and
/// sharing a form of its own when asked to.
pub(crate) struct FakeLogger {
    socket: UdpSocket,
    subscriber: Cell<Option<SocketAddr>>,
}

impl FakeLogger {
    pub(crate) fn listen() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        Self {
            socket,
            subscriber: Cell::new(None),
        }
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
    /// A subscription is noted rather than returned, so a test reads what the
    /// application filled in without minding when it last renewed.
    pub(crate) fn received(&self) -> Option<String> {
        let mut buffer = [0; 4096];
        loop {
            let (length, from) = self.socket.recv_from(&mut buffer).ok()?;
            let text = String::from_utf8(buffer[..length].to_vec()).unwrap();
            if text.contains(r#""type":"subscribe""#) || text.contains(r#""type":"unsubscribe""#) {
                self.subscriber.set(Some(from));
                continue;
            }
            return Some(text);
        }
    }

    /// Sends the application its form, as a logger sharing it does.
    pub(crate) fn share(&self, fields: &[(&str, &str)]) {
        let form = set_message(fields).replacen(r#""type":"set""#, r#""type":"form""#, 1);
        self.send(&form);
    }

    /// Tells the application the logger is going.
    pub(crate) fn bye(&self) {
        self.send(r#"{"v":1,"type":"bye"}"#);
    }

    fn send(&self, message: &str) {
        self.socket.send_to(message.as_bytes(), self.subscriber()).unwrap();
    }

    fn subscriber(&self) -> SocketAddr {
        if let Some(subscriber) = self.subscriber.get() {
            return subscriber;
        }
        let mut buffer = [0; 4096];
        let (length, from) = self.socket.recv_from(&mut buffer).expect("never subscribed");
        let text = String::from_utf8(buffer[..length].to_vec()).unwrap();
        assert!(
            text.contains(r#""type":"subscribe""#),
            "filled in before subscribing: {text}"
        );
        self.subscriber.set(Some(from));
        from
    }
}
