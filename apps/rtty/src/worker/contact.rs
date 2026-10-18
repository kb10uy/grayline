pub use grayline_qso::{ContactPaths, ContactSnapshot, ContactState, ContactWorker};

use grayline_qso::ContactSettings;
use grayline_shell::worker::Waker;

pub fn spawn(settings: &ContactSettings, paths: &ContactPaths, waker: Waker) -> ContactWorker {
    ContactWorker::spawn(
        paths.open(settings.lookup, &settings.wavelog_url),
        "grayline-rtty-contact".to_owned(),
        move || waker.wake(),
    )
}
