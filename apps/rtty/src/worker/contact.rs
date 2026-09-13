pub use grayline_qso::{ContactPaths, ContactSnapshot, ContactState, ContactWorker};

use crate::{storage::config::ContactSettings, worker::Waker};

pub fn spawn(settings: &ContactSettings, paths: &ContactPaths, waker: Waker) -> ContactWorker {
    ContactWorker::spawn(
        paths.open(settings.lookup, &settings.wavelog_url),
        "grayline-rtty-contact".to_owned(),
        move || waker.wake(),
    )
}
