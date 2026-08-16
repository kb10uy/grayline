//! What this application calls itself.

/// The application's own directory, under the family's.
pub const APP_DIRECTORY: &str = "rtty";

/// What the operator is shown, in the window title and the desktop entry.
pub const DISPLAY_NAME: &str = "Grayline RTTY";

/// The executable's name.
///
/// Not taken from `CARGO_PKG_NAME`: the package carries an `-app` suffix so it
/// does not collide with the protocol library, while this name reaches the
/// desktop entry, the Wayland `app_id`, and the single-instance claim, which
/// have to agree with the installed binary rather than with Cargo.
pub const PROCESS_NAME: &str = "grayline-rtty";

/// Where the operator's own files from this application are kept.
///
/// RTTY produces text rather than pictures and writes none of it yet; the
/// received-text log will answer here when it arrives. The identity carries
/// the field on every application, so it is named now rather than left to be
/// invented later under a different name.
pub const DOCUMENTS_DIRECTORY: &str = "Grayline RTTY";

/// Where the operator's manual is published, for the reason recorded beside
/// the SSTV application's own address.
pub const MANUAL_URL: &str = "https://grayline.jl1his.radio/rtty/";

pub const IDENTITY: grayline_shell::Identity = grayline_shell::Identity {
    app_directory: APP_DIRECTORY,
    display_name: DISPLAY_NAME,
    process_name: PROCESS_NAME,
    pictures_directory: DOCUMENTS_DIRECTORY,
    manual_url: MANUAL_URL,
    app_user_model_id: "kb10uy.GraylineRTTY",
    icon_png: include_bytes!("../assets/icon.png"),
};
