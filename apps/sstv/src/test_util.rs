use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

/// Where an interface built without directories of its own keeps them.
///
/// [`crate::app::App::headless`] used to hold empty paths, which are relative
/// and therefore resolve against the working directory: the suite wrote the
/// default band plan and rig script into the package directory every time it
/// ran, and those files were committed more than once by mistake. One
/// directory is shared by every headless interface because none of them is
/// meant to see anything in it; a test that cares about what is on disk builds
/// a [`TempDir`] of its own instead.
///
/// Left behind rather than removed at exit, which no reliable hook covers. It
/// sits under the system's temporary directory, which is where a file nobody
/// removes belongs.
pub(crate) fn scratch_dir() -> &'static Path {
    static SCRATCH: LazyLock<PathBuf> = LazyLock::new(|| {
        let path =
            std::env::temp_dir().join(format!("grayline-sstv-headless-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    });
    &SCRATCH
}

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        let index = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("grayline-sstv-test-{}-{index}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
