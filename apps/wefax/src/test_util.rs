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
            "grayline-wefax-test-{}-{}",
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
/// somewhere to say it would have written: pointing it at the source tree once
/// left files behind that were committed by mistake.
pub fn scratch_dir() -> PathBuf {
    let path = env::temp_dir().join(format!("grayline-wefax-scratch-{}", std::process::id()));
    fs::create_dir_all(&path).expect("the temporary directory is writable");
    path
}
