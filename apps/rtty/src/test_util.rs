use grayline_rig_control::{DEFAULT_PORT_NAME, RigControlError, RigError, script::ScriptError};

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

/// A directory that removes itself when the test that made it finishes.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "grayline-rtty-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("the temporary directory is writable");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _: io::Result<()> = fs::remove_dir_all(&self.0);
    }
}

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
