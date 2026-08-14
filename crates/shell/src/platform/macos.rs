//! macOS integration.

use egui::IconData;

use crate::Identity;

pub const UI_FONTS: [&str; 2] = ["Hiragino Sans", "Helvetica Neue"];

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
