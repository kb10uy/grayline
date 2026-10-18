//! Platform integration, gathered behind one module.
//!
//! Everything that only makes sense on one operating system lives here, so the
//! rest of the application can be read without stepping over `#[cfg]`. Each
//! platform provides the same set of items and the compiler enforces that: a
//! new operation has to be answered on every platform before the build passes,
//! even if the answer is to do nothing.
//!
//! The menu bar is the one deliberate exception. It stays with the
//! sibling [`crate::menu`] module, which renders application-defined models.

use std::{
    ffi::OsStr,
    io,
    path::Path,
    process::{Command, Stdio},
};

use egui::IconData;
use image::ImageFormat;

use crate::Identity;

// The facade below documents every item these modules provide. Each is one
// answer to that description rather than a description of its own, so they are
// not made to restate it three times over.
#[allow(missing_docs)]
#[cfg_attr(target_os = "windows", path = "platform/windows.rs")]
#[cfg_attr(not(target_os = "windows"), path = "platform/other.rs")]
mod imp;

/// Font families the interface is drawn with, in priority order.
///
/// The platform's own UI face is named rather than left to the font crate's
/// list, which puts `Noto Sans JP` first. On Windows that resolves to
/// `NotoSansJP-VF.ttf`, a variable font whose weight axis defaults to Thin;
/// egui does not apply variable axes, so the whole interface would be drawn
/// hairline.
pub use imp::UI_FONTS;

/// Font families monospaced text is drawn with, in priority order.
///
/// Each platform's own coding font leads the list: the face the operator
/// already reads program text and terminal output in is the one their eye is
/// trained on, and it is the face on the machine most likely to have been
/// drawn for column alignment at small sizes. What follows it is the face that
/// was there before it shipped, so an older release of the same system still
/// finds one.
///
/// Monaspace leads the list on every platform. It is the one family here that
/// an operator installs on purpose rather than finds already present, and the
/// one that covers the arrows and box drawing a teleprinter transcript is
/// annotated with: neither Consolas, Courier New, nor Segoe UI carries U+21B5,
/// the mark a transmit message shows a line ending with, so without it that
/// mark is drawn by a bundled fallback at a scale of its own.
pub use imp::MONOSPACE_FONTS;

/// The directory every application in this family keeps its files under.
///
/// Each application owns a directory beneath this one, and the settings they
/// share sit alongside those. The name is chosen by the platform because the
/// conventions differ: the base directory is already hidden on Linux, which
/// spells these lowercase, while the other two show them to the operator and
/// nest vendor before product.
pub use imp::FAMILY_DIRECTORY;

/// Prepares the process before any window exists.
///
/// Called once at startup, before the event loop is built, for work that has
/// to be in place before the first window is created.
pub use imp::prepare_process;

/// Prepares the main window once the platform has created it.
///
/// Called from the eframe creation hook, which is the earliest point a native
/// window handle exists.
pub use imp::prepare_window;

/// Returns the icon the window and task switcher should show.
///
/// Windows reads it back out of the executable's own resources, which the
/// build script embeds from `assets/icon.ico`: the shell already shows that
/// icon on the file, so the window shows the same artwork rather than a second
/// copy that could drift from it. Every other platform has no such resource
/// section and decodes [`embedded_icon`] instead, as does Windows when the
/// resource is missing because the build had no resource compiler.
pub use imp::window_icon;

/// Takes a window off the screen without destroying it.
///
/// Only Windows has anything to take off the screen this way: it is the one
/// platform where the menu bar owns a window handle of its own.
#[cfg(target_os = "windows")]
pub(crate) use imp::hide_window;

fn embedded_icon(identity: &Identity) -> Option<IconData> {
    let image = image::load_from_memory_with_format(identity.icon_png, ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Some(IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// What the application is doing, as far as the platform cares.
///
/// Reported so the machine does not go to sleep in the middle of a picture.
/// Only the states worth keeping the machine awake for are distinguished; an
/// open device that nothing is arriving on is [`Activity::Idle`], so leaving
/// the application running does not hold sleep off indefinitely.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Activity {
    /// Nothing worth keeping the machine awake for.
    #[default]
    Idle,
    /// A picture is being decoded.
    Receiving,
    /// A picture is being sent.
    Transmitting,
}

/// Platform side effects that follow application state.
///
/// Unlike the rest of this module these are not one-shot, so they are reached
/// through a trait: the application holds one of these, and a test can hold a
/// recording one instead and assert on what the interface asked for.
pub trait Platform {
    /// Reports what the application is doing.
    ///
    /// Called only when the activity changes, so an implementation may treat
    /// each call as a transition rather than a repeat.
    fn set_activity(&mut self, activity: Activity);

    /// Opens `path` with whatever the platform opens it with.
    ///
    /// Reached through the platform rather than called directly so a test can
    /// hold one that answers without launching anything: the interface offers
    /// this on every directory it keeps, and a suite that took each of them at
    /// its word would bury the machine in file manager windows.
    fn open_path(&mut self, path: &Path) -> io::Result<()> {
        open_path(path)
    }

    /// Opens `url` in the operator's browser.
    ///
    /// Reached through the platform for the same reason as [`open_path`]: the
    /// Help menu is exercised by the suite, and a test that took this at its
    /// word would open a browser window every time it ran.
    fn open_url(&mut self, url: &str) -> io::Result<()> {
        open_url(url)
    }
}

/// Returns the platform the application is running on.
pub fn host() -> Box<dyn Platform> {
    Box::<imp::Host>::default()
}

/// A platform that answers without doing anything at all.
///
/// The real platform on the targets without their own integration still
/// opens directories for the operator. Tests want the opposite, and take this
/// instead.
///
/// Compiled unconditionally rather than under `#[cfg(test)]`, which would
/// cover this crate's own tests and not the applications', which are where it
/// is wanted.
#[derive(Default)]
pub struct QuietPlatform;

impl Platform for QuietPlatform {
    fn set_activity(&mut self, _activity: Activity) {}

    fn open_path(&mut self, _path: &Path) -> io::Result<()> {
        Ok(())
    }

    fn open_url(&mut self, _url: &str) -> io::Result<()> {
        Ok(())
    }
}

/// A claim on being the only running copy of the application.
///
/// Held for the lifetime of the process; releasing it lets the next launch
/// through. The application takes a claim because it holds audio devices
/// open, and a second copy silently failing to acquire them is harder to
/// understand than not opening at all.
pub struct SingleInstance(imp::Claim);

/// Claims the right to be the only running copy.
///
/// Returns `None` when another copy already holds the claim, having first
/// asked that copy to come to the front, so the launch the operator just made
/// still puts the application in front of them.
pub fn claim_single_instance(identity: &Identity) -> Option<SingleInstance> {
    imp::claim_single_instance(identity).map(SingleInstance)
}

impl SingleInstance {
    /// Records where this copy's window is, so a later launch can raise it.
    ///
    /// Called once the platform has created the window, which is the earliest
    /// point there is anything to record.
    pub fn publish_window(&self, cc: &eframe::CreationContext<'_>) {
        self.0.publish_window(cc);
    }
}

/// Opens `path` with whatever the platform opens it with.
pub fn open_path(path: &Path) -> io::Result<()> {
    hand_to_shell(path.as_os_str())
}

/// Opens `url` with whatever the platform opens it with.
///
/// The manual is a page on the web, and the operator's browser is the program
/// that reads one. Which browser that is belongs to them, so the address is
/// handed to the same opener a directory goes through rather than to a
/// program named here.
pub fn open_url(url: &str) -> io::Result<()> {
    hand_to_shell(url.as_ref())
}

/// Hands `argument` to the platform's shell opener.
///
/// A directory reaches the file manager and the manual reaches the browser
/// through one command, because each platform's file manager is also its shell
/// opener. There is nothing to choose between here, so nothing does.
///
/// The child is waited on by a detached thread rather than left unclaimed, so
/// a long-lived session does not accumulate zombies on the platforms that
/// create them.
fn hand_to_shell(argument: &OsStr) -> io::Result<()> {
    let Some(program) = imp::FILE_MANAGER else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no program to open things with",
        ));
    };

    let mut child = Command::new(program)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
