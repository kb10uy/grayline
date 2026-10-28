use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use crate::{Directory, QsoError, Record, Store};

/// What the directory is doing about the callsign in the QSO panel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContactState {
    /// No callsign has been asked about yet.
    #[default]
    Idle,
    /// A lookup is pending.
    Looking,
    /// Something is filed under the callsign.
    Known,
    /// The directory was asked and has nothing filed under it.
    Unknown,
    /// The last operation failed.
    Failed,
}

/// What the worker last found, read once per frame.
#[derive(Clone, Debug, Default)]
pub struct ContactSnapshot {
    /// The callsign this is about, normalized.
    pub callsign: String,
    /// The latest operation state.
    pub state: ContactState,
    /// The fields a template reads as `${contact.<key>}`.
    ///
    /// A plain map rather than a `Record`, so the interface reads it without
    /// reaching through the directory's own type and the snapshot stays as
    /// cloneable as every other worker's.
    pub fields: BTreeMap<String, String>,
    /// A directory or worker failure, shared across snapshot copies.
    pub error: Option<Arc<QsoError>>,
}

enum Request {
    LookUp(String),
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
/// its store into the operator's own directory. `None` means there is none,
/// and the worker keeps one in memory.
#[derive(Clone, Debug, Default)]
pub struct ContactPaths {
    /// The shared store, or nothing for one that lives only in memory.
    pub store: Option<PathBuf>,
}

/// The thread that reads and writes the contact directory.
///
/// Spawned once and kept for the whole session rather than per lookup: it
/// holds the store, and it is where a record the operator corrected is
/// written, off the thread that draws the interface.
pub struct ContactWorker {
    snapshot: Arc<Mutex<ContactSnapshot>>,
    /// Held as an option so that dropping the worker closes the channel, which
    /// is what tells the thread to checkpoint the store and stop.
    requests: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl ContactWorker {
    /// Starts a worker over an opened directory, publishing an opening failure as a snapshot.
    /// The callback runs after each processed batch of requests.
    pub fn spawn(
        directory: Result<Directory, QsoError>,
        thread_name: String,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let snapshot = Arc::new(Mutex::new(ContactSnapshot::default()));
        let (requests, incoming) = mpsc::channel();
        let worker_snapshot = Arc::clone(&snapshot);

        let thread = thread::Builder::new()
            .name(thread_name)
            .spawn(move || match directory {
                Ok(directory) => contact_loop(directory, &incoming, &worker_snapshot, &wake),
                Err(error) => update(&worker_snapshot, |state| {
                    state.state = ContactState::Failed;
                    state.error = Some(Arc::new(error));
                }),
            })
            .ok();
        if thread.is_none() {
            update(&snapshot, |state| {
                state.state = ContactState::Failed;
                state.error = Some(Arc::new(QsoError::WorkerUnavailable));
            });
        }

        Self {
            snapshot,
            requests: Some(requests),
            thread,
        }
    }

    /// Returns the latest published snapshot.
    pub fn latest(&self) -> ContactSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| ContactSnapshot {
                state: ContactState::Failed,
                error: Some(Arc::new(QsoError::WorkerUnavailable)),
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
        self.request(Request::LookUp(callsign.to_owned()));
    }

    /// Files what the operator corrected, which no import will displace.
    pub fn save(&self, record: Record, dropped: Vec<String>) {
        self.request(Request::Save { record, dropped });
    }

    /// Whether the thread is still there to take requests.
    ///
    /// One whose directory could not be opened published why and stopped.
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|thread| !thread.is_finished())
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

impl ContactPaths {
    /// Opens the named store, or one in memory when none is named.
    pub fn open(&self) -> Result<Directory, QsoError> {
        let store = match &self.store {
            Some(path) => Store::open(path)?,
            None => Store::in_memory()?,
        };
        Ok(Directory::new(store))
    }
}

fn contact_loop(
    mut directory: Directory,
    incoming: &Receiver<Request>,
    snapshot: &Arc<Mutex<ContactSnapshot>>,
    wake: &impl Fn(),
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
                Request::LookUp(callsign) => look_up(&directory, &callsign, snapshot),
                Request::Save { record, dropped } => save(&mut directory, &record, &dropped, snapshot),
            }
        }
        wake();
    }
    // Folds the write-ahead log back in, so what a roaming profile copies is
    // one self-contained file rather than a file and the log completing it.
    let _ = directory.store().checkpoint();
}

/// Drops every queued lookup but the last.
///
/// An operator tabbing through the panel can leave several queued behind one
/// slow write, and every answer but the last would be about a station they
/// have already moved on from. A save is never dropped: it is what the operator
/// asked to have written.
fn collapse_lookups(pending: &mut VecDeque<Request>) {
    let Some(last) = pending
        .iter()
        .rposition(|request| matches!(request, Request::LookUp(_)))
    else {
        return;
    };
    *pending = pending
        .drain(..)
        .enumerate()
        .filter(|(index, request)| *index == last || !matches!(request, Request::LookUp(_)))
        .map(|(_, request)| request)
        .collect();
}

fn look_up(directory: &Directory, callsign: &str, snapshot: &Arc<Mutex<ContactSnapshot>>) {
    match directory.look_up(callsign) {
        Ok(answer) => update(snapshot, |state| {
            state.callsign = answer.record.callsign().to_owned();
            state.state = if answer.known {
                ContactState::Known
            } else {
                ContactState::Unknown
            };
            state.fields = fields(&answer.record);
            state.error = None;
        }),
        Err(error) => update(snapshot, |state| {
            state.state = ContactState::Failed;
            state.fields.clear();
            state.error = Some(Arc::new(error));
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
        Err(error) => update(snapshot, |state| state.error = Some(Arc::new(error))),
    }
}

fn fields(record: &Record) -> BTreeMap<String, String> {
    record
        .iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn update<T>(snapshot: &Mutex<T>, update: impl FnOnce(&mut T)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}

#[cfg(test)]
mod tests;
