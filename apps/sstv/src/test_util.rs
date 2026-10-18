use grayline_rig_control::{DEFAULT_PORT_NAME, RigControlError, RigError, script::ScriptError};

use std::{
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
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
