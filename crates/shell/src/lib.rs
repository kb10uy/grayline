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
    /// Where the operator's manual is published, which is the address the
    /// Help menu opens.
    ///
    /// Each application answers with its own pages: they describe different
    /// equipment and are written on their own schedule, so one address for
    /// the family would land the operator on a page about the other mode.
    pub manual_url: &'static str,
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

/// The variable that points an application at a different manual.
///
/// The published address is compiled in, because an operator should not have
/// to configure where the help is. This exists for the person writing the
/// pages: a build run with it set opens the copy they are rendering locally
/// rather than the site, which is the only way to read a page before it is
/// published.
pub const MANUAL_URL_VARIABLE: &str = "GRAYLINE_MANUAL_URL";

/// The address the Help menu should open.
///
/// [`MANUAL_URL_VARIABLE`] replaces the identity's own address when it is set
/// to something; set to nothing it is ignored, because a variable exported
/// empty by a shell script reads as one that was never set rather than as a
/// request to open nowhere.
pub fn manual_url(identity: &Identity) -> String {
    chosen_manual_url(std::env::var(MANUAL_URL_VARIABLE).ok(), identity)
}

/// The address `configured` names, or the identity's own.
///
/// Split from the lookup so it can be checked without an environment: a test
/// that set the variable would be setting it for every other test in the
/// process at the same time.
fn chosen_manual_url(configured: Option<String>, identity: &Identity) -> String {
    configured
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| identity.manual_url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Identity, chosen_manual_url};

    const TEST_IDENTITY: Identity = Identity {
        app_directory: "test",
        display_name: "Grayline Test",
        process_name: "grayline-test",
        pictures_directory: "Grayline Test",
        manual_url: "https://grayline.jl1his.radio/test/",
        app_user_model_id: "kb10uy.GraylineTest",
        icon_png: &[],
    };

    #[test]
    fn the_published_address_is_opened_when_nothing_overrides_it() {
        assert_eq!(chosen_manual_url(None, &TEST_IDENTITY), TEST_IDENTITY.manual_url);
        assert_eq!(
            chosen_manual_url(Some("   ".to_owned()), &TEST_IDENTITY),
            TEST_IDENTITY.manual_url
        );
    }

    #[test]
    fn an_override_is_opened_instead() {
        assert_eq!(
            chosen_manual_url(Some("http://localhost:8080/sstv/".to_owned()), &TEST_IDENTITY),
            "http://localhost:8080/sstv/"
        );
    }
}
