//! The parts of a Grayline application that do not depend on its mode.
//!
//! Platform integration, localization machinery, and the rolling log are the
//! same work whichever signal an application carries, so they live here and
//! each application supplies what makes it itself through [`Identity`] and an
//! [`i18n::Catalog`].
//!
//! What is deliberately absent is as informative as what is here. Settings,
//! the menu model, and the views still belong to the application: with one
//! application written there is no second reading to generalize from, and a
//! shared shape guessed from one caller is harder to correct later than one
//! extracted from two.

#![deny(missing_docs)]
// Not `forbid(unsafe_code)`, unlike the rest of this workspace: the Windows
// integration calls the Win32 API directly, which is the whole reason this
// module is separated from everything that can be written safely.

/// Fluent-backed message lookup, and the languages an application offers.
pub mod i18n;
/// The rolling log an application writes under its state directory.
pub mod log;
/// Everything that only makes sense on one operating system.
pub mod platform;

/// What one application calls itself.
///
/// Passed to the shared parts rather than read from a constant, because a
/// second application in this family answers every one of these differently
/// while running the same code. Passed explicitly rather than installed once
/// into a global, so a test can build one without disturbing another.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    /// The application's own directory, under
    /// [`platform::FAMILY_DIRECTORY`].
    pub app_directory: &'static str,
    /// What the operator is shown, in the window title and the log's first
    /// line.
    pub display_name: &'static str,
    /// The executable's name, which the desktop entry, the Wayland `app_id`,
    /// and the single-instance claim all have to agree on.
    pub process_name: &'static str,
    /// Where the operator's own pictures are kept, under their pictures
    /// directory.
    pub pictures_directory: &'static str,
    /// How the Windows shell groups taskbar buttons, pinned shortcuts, and
    /// notifications.
    ///
    /// Carried on every platform though only one reads it: an identity that
    /// changed shape by target would have to be assembled behind `#[cfg]` at
    /// each application, which is the arrangement this module exists to spare
    /// them.
    pub app_user_model_id: &'static str,
    /// The application icon, for the platforms with nowhere else to read one
    /// from.
    pub icon_png: &'static [u8],
}
