//! macOS integration.

use egui::IconData;

use crate::Identity;

pub const UI_FONTS: [&str; 2] = ["Hiragino Sans", "Helvetica Neue"];

/// The coding fonts to look for, in the order they are preferred.
///
/// Monaspace leads the list on every platform. It is the one family here that
/// an operator installs on purpose rather than finds already present, and the
/// one that covers the arrows and box drawing a teleprinter transcript is
/// annotated with: neither Consolas, Courier New, nor Segoe UI carries U+21B5,
/// the mark a transmit message shows a line ending with, so without it that
/// mark is drawn by a bundled fallback at a scale of its own.
pub const MONOSPACE_FONTS: [&str; 4] = ["Monaspace Neon", "SF Mono", "Menlo", "Monaco"];

pub const FILE_MANAGER: Option<&str> = Some("open");

pub const FAMILY_DIRECTORY: &str = "Grayline";

/// The window is themed by AppKit from the system appearance, and the menu
/// bar is drawn inside the window, so nothing has to be arranged in advance.
pub fn prepare_process(_identity: &Identity) {}

pub fn prepare_window(_cc: &eframe::CreationContext<'_>) {}

/// Keeping the machine awake needs an `IOPMAssertion`, which is not wired up
/// yet, so activity is accepted and discarded.
pub type Host = super::InertPlatform;

pub type Claim = super::FileLock;

pub fn claim_single_instance(identity: &Identity) -> Option<Claim> {
    super::lock_file_claim(identity)
}

pub fn window_icon(identity: &Identity) -> Option<IconData> {
    super::embedded_icon(identity)
}
