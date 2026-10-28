use grayline_logger_link::LoggerSettings;
use grayline_rig_control::{DEFAULT_PORT_NAME, RigControlError, RigError, script::ScriptError};

use std::{
    fs,
    net::UdpSocket,
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

    /// The next datagram, or `None` once nothing has arrived for a while.
    pub(crate) fn received(&self) -> Option<String> {
        let mut buffer = [0; 4096];
        let (length, _) = self.socket.recv_from(&mut buffer).ok()?;
        Some(String::from_utf8(buffer[..length].to_vec()).unwrap())
    }
}
