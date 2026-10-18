pub use grayline_qso::{ContactPaths, ContactSnapshot, ContactState, ContactWorker};

use grayline_shell::worker::Waker;

use crate::storage::config::ContactSettings;

pub fn spawn(settings: &ContactSettings, paths: &ContactPaths, waker: Waker) -> ContactWorker {
    ContactWorker::spawn(
        paths.open(settings.lookup, &settings.wavelog_url),
        "grayline-sstv-contact".to_owned(),
        move || waker.wake(),
    )
}
