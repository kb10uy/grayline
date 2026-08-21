use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use grayline_qso::{Credentials, DEFAULT_TIMEOUT, Directory, Record, Store, Wavelog};

use crate::{
    error::AppError,
    storage::config::ContactSettings,
    worker::{Waker, update},
};

/// What the directory is doing about the callsign in the QSO panel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContactState {
    /// No callsign has been asked about yet.
    #[default]
    Idle,
    Looking,
    /// Something is filed under the callsign, locally or from the logger.
    Known,
    /// The directory was asked and has nothing filed under it.
    Unknown,
    Failed,
}

/// What the worker last found, read once per frame.
#[derive(Clone, Debug, Default)]
pub struct ContactSnapshot {
    /// The callsign this is about, normalized.
    pub callsign: String,
    pub state: ContactState,
    /// The fields a template reads as `${contact.<key>}`.
    ///
    /// A plain map rather than a `Record`, so the interface reads it without
    /// reaching through the directory's own type and the snapshot stays as
    /// cloneable as every other worker's.
    pub fields: BTreeMap<String, String>,
    pub error: Option<AppError>,
}

enum Request {
    LookUp {
        callsign: String,
        refresh: bool,
    },
    /// What the operator corrected: the fields they left filled in, and the
    /// ones they emptied. Emptying is not the same as leaving alone, so the
    /// two travel together rather than a write standing for both.
    Save {
        record: Record,
        dropped: Vec<String>,
    },
}

/// The files the directory is opened over.
///
/// Named rather than discovered, because a discovered path is one the test
/// suite cannot escape: an application built for a test would otherwise write
/// its store into the operator's own directory and read the operator's own API
/// key, which is a live logger a test has no business reaching. `None` means
/// there is none, and the worker does without.
#[derive(Clone, Debug, Default)]
pub struct ContactPaths {
    /// The shared store, or nothing for one that lives only in memory.
    pub store: Option<PathBuf>,
    /// The credentials file naming the logger, or nothing for no logger.
    pub credentials: Option<PathBuf>,
}

/// The thread that reads and writes the contact directory.
///
/// Unlike rig control this is spawned once and kept for the whole session: it
/// holds a store rather than a socket, it works with the network switched off,
/// and it is where a record the operator corrected is written. Turning the
/// lookup off takes the logger away from it, not the directory.
pub struct ContactWorker {
    snapshot: Arc<Mutex<ContactSnapshot>>,
    /// Held as an option so that dropping the worker closes the channel, which
    /// is what tells the thread to checkpoint the store and stop.
    requests: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl ContactWorker {
    /// Opens the store named by `settings` and starts the thread over it.
    ///
    /// A store that will not open leaves the worker running with none: the
    /// failure is reported once in the snapshot, and the application carries on
    /// with the contact variables empty rather than refusing to start.
    pub fn spawn(settings: &ContactSettings, paths: &ContactPaths, waker: Waker) -> Self {
        let snapshot = Arc::new(Mutex::new(ContactSnapshot::default()));
        let (requests, incoming) = mpsc::channel();
        let worker_snapshot = Arc::clone(&snapshot);
        let opened = open(settings, paths);

        let thread = thread::Builder::new()
            .name("grayline-sstv-contact".to_owned())
            .spawn(move || match opened {
                Ok(directory) => contact_loop(directory, &incoming, &worker_snapshot, &waker),
                Err(error) => update(&worker_snapshot, |state| {
                    state.state = ContactState::Failed;
                    state.error = Some(error);
                }),
            })
            .ok();
        if thread.is_none() {
            update(&snapshot, |state| {
                state.state = ContactState::Failed;
                state.error = Some(AppError::WorkerUnavailable("the contact directory"));
            });
        }

        Self {
            snapshot,
            requests: Some(requests),
            thread,
        }
    }

    pub fn latest(&self) -> ContactSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| ContactSnapshot {
                state: ContactState::Failed,
                error: Some(AppError::WorkerUnavailable("the contact directory")),
                ..ContactSnapshot::default()
            })
    }

    /// Asks what is known about `callsign`.
    ///
    /// The snapshot is moved to [`ContactState::Looking`] here rather than on
    /// the worker's thread, so that the interface shows the new callsign from
    /// the frame the request was made in rather than still showing the last
    /// station's details under it.
    pub fn look_up(&self, callsign: &str) {
        self.begin(callsign);
        self.request(Request::LookUp {
            callsign: callsign.to_owned(),
            refresh: false,
        });
    }

    /// Asks the logger again, whatever the store holds.
    pub fn refresh(&self, callsign: &str) {
        self.begin(callsign);
        self.request(Request::LookUp {
            callsign: callsign.to_owned(),
            refresh: true,
        });
    }

    /// Files what the operator corrected, which no lookup will displace.
    pub fn save(&self, record: Record, dropped: Vec<String>) {
        self.request(Request::Save { record, dropped });
    }

    fn begin(&self, callsign: &str) {
        update(&self.snapshot, |state| {
            state.callsign = callsign.to_ascii_uppercase();
            state.state = ContactState::Looking;
            state.fields.clear();
            state.error = None;
        });
    }

    fn request(&self, request: Request) {
        if let Some(requests) = self.requests.as_ref() {
            // A worker that has already stopped is one whose failure is
            // already in the snapshot, so there is nothing here to report.
            let _ = requests.send(request);
        }
    }
}

impl Drop for ContactWorker {
    fn drop(&mut self) {
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for ContactWorker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ContactWorker")
            .field("running", &self.thread.is_some())
            .finish()
    }
}

/// Opens the directory the worker will hold.
///
/// A store path of `None` means one that lives only as long as the session,
/// which is what the test harness runs on: opening the shared one would have
/// the suite write into the operator's own directory.
fn open(settings: &ContactSettings, paths: &ContactPaths) -> Result<Directory, AppError> {
    let store = match &paths.store {
        Some(path) => Store::open(path)?,
        None => Store::in_memory()?,
    };
    Ok(Directory::new(store, remote(settings, paths.credentials.as_deref())))
}

/// The logger to ask, when there is one configured to ask.
///
/// Anything missing — the switch, the address, the key — means no logger
/// rather than an error: an operator who has not set one up has not failed at
/// anything, and the store answers on its own.
fn remote(settings: &ContactSettings, credentials: Option<&Path>) -> Option<Wavelog> {
    if !settings.lookup {
        return None;
    }
    let credentials = credentials?;
    let credentials = Credentials::read(credentials).unwrap_or_default().with_environment();
    let url = if settings.wavelog_url.trim().is_empty() {
        credentials.url?
    } else {
        settings.wavelog_url.clone()
    };
    Wavelog::new(&url, &credentials.key?, DEFAULT_TIMEOUT).ok()
}

fn contact_loop(
    mut directory: Directory,
    incoming: &Receiver<Request>,
    snapshot: &Arc<Mutex<ContactSnapshot>>,
    waker: &Waker,
) {
    let mut pending = VecDeque::new();
    while let Ok(request) = incoming.recv() {
        pending.push_back(request);
        while let Ok(queued) = incoming.try_recv() {
            pending.push_back(queued);
        }
        collapse_lookups(&mut pending);

        for request in pending.drain(..) {
            match request {
                Request::LookUp { callsign, refresh } => look_up(&mut directory, &callsign, refresh, snapshot),
                Request::Save { record, dropped } => save(&mut directory, &record, &dropped, snapshot),
            }
        }
        waker.wake();
    }
    // Folds the write-ahead log back in, so what a roaming profile copies is
    // one self-contained file rather than a file and the log completing it.
    let _ = directory.store().checkpoint();
}

/// Drops every queued lookup but the last.
///
/// An operator tabbing through the panel can leave several queued behind one
/// slow round trip, and every answer but the last would be about a station they
/// have already moved on from. A save is never dropped: it is what the operator
/// asked to have written.
fn collapse_lookups(pending: &mut VecDeque<Request>) {
    let Some(last) = pending
        .iter()
        .rposition(|request| matches!(request, Request::LookUp { .. }))
    else {
        return;
    };
    *pending = pending
        .drain(..)
        .enumerate()
        .filter(|(index, request)| *index == last || !matches!(request, Request::LookUp { .. }))
        .map(|(_, request)| request)
        .collect();
}

fn look_up(directory: &mut Directory, callsign: &str, refresh: bool, snapshot: &Arc<Mutex<ContactSnapshot>>) {
    match directory.look_up(callsign, refresh) {
        Ok(answer) => update(snapshot, |state| {
            state.callsign = answer.record.callsign().to_owned();
            state.state = if answer.known {
                ContactState::Known
            } else {
                ContactState::Unknown
            };
            state.fields = fields(&answer.record);
            // A logger that refused is reported beside what the store knew
            // rather than instead of it: the answer below is still true.
            state.error = answer.remote_error.map(Into::into);
        }),
        Err(error) => update(snapshot, |state| {
            state.state = ContactState::Failed;
            state.fields.clear();
            state.error = Some(error.into());
        }),
    }
}

/// Files a corrected record, and publishes it when it is the one on screen.
///
/// A save for any other station changes nothing there: the operator is looking
/// at one contact, and a write about another is not news about it.
fn save(directory: &mut Directory, record: &Record, dropped: &[String], snapshot: &Arc<Mutex<ContactSnapshot>>) {
    let written = dropped
        .iter()
        .try_for_each(|key| directory.unset(record.callsign(), key).map(|_| ()))
        .and_then(|()| directory.save(record));
    match written {
        Ok(()) => update(snapshot, |state| {
            state.error = None;
            if record.callsign() == state.callsign {
                state.state = if record.is_empty() {
                    ContactState::Unknown
                } else {
                    ContactState::Known
                };
                state.fields = fields(record);
            }
        }),
        Err(error) => update(snapshot, |state| state.error = Some(error.into())),
    }
}

fn fields(record: &Record) -> BTreeMap<String, String> {
    record
        .iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}
