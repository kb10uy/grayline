//! Integration for every platform without a dedicated implementation.
//!
//! macOS and Linux are the platforms this is written for; anything else the
//! project has not been tried on lands here too and gets the neutral answers
//! Linux does.

use std::{fs::OpenOptions, path::Path};

use egui::IconData;

use super::{Activity, Platform};
use crate::Identity;

#[cfg(target_os = "macos")]
pub const UI_FONTS: &[&str] = &["Hiragino Sans", "Helvetica Neue"];

#[cfg(not(target_os = "macos"))]
pub const UI_FONTS: &[&str] = &["Noto Sans CJK JP", "Noto Sans", "DejaVu Sans"];

#[cfg(target_os = "macos")]
pub const MONOSPACE_FONTS: &[&str] = &["Monaspace Neon", "SF Mono", "Menlo", "Monaco"];

#[cfg(not(target_os = "macos"))]
pub const MONOSPACE_FONTS: &[&str] = &[
    "Monaspace Neon",
    "Noto Sans Mono CJK JP",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
];

/// macOS and Linux have a shell opener this can name. Another platform
/// reaching here reports that revealing a directory is unsupported rather
/// than spawning a command that does not exist.
pub const FILE_MANAGER: Option<&str> = if cfg!(target_os = "macos") {
    Some("open")
} else if cfg!(target_os = "linux") {
    Some("xdg-open")
} else {
    None
};

/// Linux keeps its per-application directories lowercase, under a base
/// directory that is already hidden. The platforms that show these to the
/// operator name them the way the family is named.
pub const FAMILY_DIRECTORY: &str = if cfg!(target_os = "linux") {
    "grayline"
} else {
    "Grayline"
};

/// macOS themes the window from the system appearance and the menu bar is
/// drawn inside the window, and Linux needs nothing either, so nothing has to
/// be arranged in advance.
pub fn prepare_process(_identity: &Identity) {}

pub fn prepare_window(_cc: &eframe::CreationContext<'_>) {}

/// Keeping the machine awake needs an `IOPMAssertion` on macOS and the
/// desktop's inhibit service on Linux, neither of which is wired up yet, so
/// activity is accepted and discarded. Everything else, such as opening a
/// directory, still reaches the shared implementation.
#[derive(Default)]
pub struct Host;

impl Platform for Host {
    fn set_activity(&mut self, _activity: Activity) {}
}

/// A claim backed by a locked file.
///
/// The lock is released by the operating system when the process ends, so a
/// copy that crashed does not keep the next launch out.
pub struct Claim {
    #[expect(dead_code, reason = "held open to hold the lock")]
    file: std::fs::File,
}

impl Claim {
    /// Nothing is published: raising another process's window needs a
    /// compositor-specific route that this application does not have yet, so
    /// a second launch reports the situation and exits instead.
    pub fn publish_window(&self, _cc: &eframe::CreationContext<'_>) {}
}

pub fn claim_single_instance(identity: &Identity) -> Option<Claim> {
    let directory = directories::BaseDirs::new()
        .and_then(|directories| directories.runtime_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{}.lock", identity.process_name)))
        .ok()?;
    file.try_lock().ok()?;
    Some(Claim { file })
}

pub fn window_icon(identity: &Identity) -> Option<IconData> {
    super::embedded_icon(identity)
}
