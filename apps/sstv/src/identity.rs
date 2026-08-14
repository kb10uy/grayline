//! What this application calls itself.
//!
//! Gathered in one module because the names are read by parts of the program
//! that have nothing else in common: the window, the picture library, the
//! single-instance claim, and the metadata written into saved images. What
//! `grayline-shell` needs of them is handed over as [`IDENTITY`]; the rest
//! stays here, because it answers to SSTV rather than to the platform.

/// The application's own directory, under the family's.
///
/// Lowercase on every platform: the family directory above it already carries
/// the platform's capitalization, and a name repeated in two cases inside one
/// path reads as two different applications.
pub const APP_DIRECTORY: &str = "sstv";

/// What the operator is shown, in the window title and the desktop entry.
pub const DISPLAY_NAME: &str = "Grayline SSTV";

/// The executable's name.
///
/// Not taken from `CARGO_PKG_NAME`: the package carries an `-app` suffix so it
/// does not collide with the protocol library, while this name reaches the
/// desktop entry, the Wayland `app_id`, and the single-instance claim, which
/// have to agree with the installed binary rather than with Cargo.
pub const PROCESS_NAME: &str = "grayline-sstv";

/// Where received and sent pictures are kept, under the operator's own
/// pictures directory.
///
/// Spelled for a person rather than for the machine, and not nested under the
/// family directory: this is somewhere the operator browses themselves, and a
/// picture one level further down is a picture they have to go looking for.
pub const PICTURES_DIRECTORY: &str = "Grayline SSTV";

/// Where the operator's manual is published.
///
/// The manual is a site rather than pages carried beside the executable, so
/// what the Help menu opens is the current text instead of the one the
/// operator's archive was cut with, and a correction reaches every copy
/// without a release.
pub const MANUAL_URL: &str = "https://grayline.jl1his.radio/sstv/";

/// The namespace the receive metadata is written in.
///
/// An XMP namespace is an identifier and not an address, so it is not required
/// to resolve. It is spelled as this repository's URL anyway, because that is
/// where a reader who finds one of these files would look next.
pub const XMP_NAMESPACE: &str = "https://github.com/kb10uy/grayline/ns/1.0/";

pub const IDENTITY: grayline_shell::Identity = grayline_shell::Identity {
    app_directory: APP_DIRECTORY,
    display_name: DISPLAY_NAME,
    process_name: PROCESS_NAME,
    pictures_directory: PICTURES_DIRECTORY,
    manual_url: MANUAL_URL,
    app_user_model_id: "kb10uy.GraylineSSTV",
    icon_png: include_bytes!("../assets/icon.png"),
};
