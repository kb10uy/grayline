use std::{env, fs, path::PathBuf};

/// A directory shared by every headless application in the suite.
///
/// A headless application writes nothing on its own, but it still needs
/// somewhere to say it would have written: pointing it at the source tree
/// would leave files behind.
pub fn scratch_dir() -> PathBuf {
    let path = env::temp_dir().join(format!("grayline-wefax-scratch-{}", std::process::id()));
    fs::create_dir_all(&path).expect("the temporary directory is writable");
    path
}
