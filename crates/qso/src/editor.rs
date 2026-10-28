use crate::{
    record::{Record, expand_fields, normalize_callsign, valid_key},
    worker::{ContactPaths, ContactSnapshot, ContactWorker},
};

/// One row of a contact dialog.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContactRow {
    /// The field's name, as it is filed.
    pub key: String,
    /// What is filed under it; empty to take it back.
    pub value: String,
    /// Whether the settings asked for this key, and so whether it is shown
    /// with a label rather than with its name laid open for editing.
    ///
    /// A row is offered whether or not the station has a value for it, which
    /// is what makes the dialog say what this operator files rather than only
    /// what this contact happens to have.
    pub offered: bool,
}

/// The contact directory as an interface works it: the worker, what it last
/// answered, and the dialog editing what is filed.
///
/// Holds no callsign of its own. The field the operator types one into is the
/// application's, and each operation is handed what that field says now.
pub struct ContactEditor {
    worker: ContactWorker,
    /// What the worker last published, read once per frame.
    pub snapshot: ContactSnapshot,
    /// Whether the dialog is open.
    pub dialog_open: bool,
    /// The rows the dialog is editing.
    ///
    /// The fields the settings ask for lead, then anything else the directory
    /// happens to hold.
    pub draft: Vec<ContactRow>,
    /// The files the worker is opened over, named rather than discovered so
    /// that a test never reaches the operator's own store or key.
    paths: ContactPaths,
    /// The callsign last asked about.
    ///
    /// Leaving a field is what commits it, and the operator leaves the field
    /// whether or not they changed anything; without this the same station
    /// would be asked about again on every pass through the panel.
    requested: String,
}

impl ContactEditor {
    /// Opens the directory over `paths`, on a thread called `thread_name`
    /// that calls `wake` when it has something new.
    pub fn new(paths: ContactPaths, thread_name: String, wake: impl Fn() + Send + 'static) -> Self {
        Self {
            worker: ContactWorker::spawn(paths.open(), thread_name, wake),
            snapshot: ContactSnapshot::default(),
            dialog_open: false,
            draft: Vec::new(),
            paths,
            requested: String::new(),
        }
    }

    /// The worker, for a test to wait on.
    pub const fn worker(&self) -> &ContactWorker {
        &self.worker
    }

    /// The files the worker is opened over.
    pub const fn paths(&self) -> &ContactPaths {
        &self.paths
    }

    /// Asks the directory about `typed`, unless it was the last thing asked.
    ///
    /// Text that is not a callsign asks nothing: a half-typed field and a
    /// garbled identifier both arrive here, and neither is a station worth
    /// looking up.
    pub fn look_up(&mut self, typed: &str) {
        let Some(callsign) = normalize_callsign(typed) else {
            self.requested.clear();
            return;
        };
        if callsign == self.requested {
            return;
        }
        self.worker.look_up(&callsign);
        self.requested = callsign;
    }

    /// Forgets the callsign last asked about, so the same one is asked about
    /// again the next time it is typed.
    pub fn forget_request(&mut self) {
        self.requested.clear();
    }

    /// Opens the dialog on what is filed under the station last answered for.
    ///
    /// The fields `fields` ask for lead, in the order they were written,
    /// whether or not this station has any of them: a station in Japan wants
    /// somewhere to put a JCC code and a station anywhere else does not, and
    /// which of those an operator is was never something to decide here.
    /// Anything else already filed follows, under its own name.
    pub fn open(&mut self, fields: &[String]) {
        let known = &self.snapshot.fields;
        let offered = expand_fields(fields.iter().map(String::as_str));
        let mut draft: Vec<ContactRow> = offered
            .iter()
            .map(|key| ContactRow {
                value: known.get(key).cloned().unwrap_or_default(),
                key: key.clone(),
                offered: true,
            })
            .collect();
        draft.extend(
            known
                .iter()
                .filter(|(key, _)| !offered.contains(key))
                .map(|(key, value)| ContactRow {
                    key: key.clone(),
                    value: value.clone(),
                    offered: false,
                }),
        );
        self.draft = draft;
        self.dialog_open = true;
    }

    /// Adds an empty row for a field of the operator's own.
    pub fn add_field(&mut self) {
        self.draft.push(ContactRow::default());
    }

    /// Files the edited rows under `typed`.
    ///
    /// A row the operator emptied is dropped rather than left alone: clearing
    /// a value is how a wrong one is taken back, and a write that only ever
    /// added would leave it filed. A row whose name
    /// no `${...}` expression could hold is kept in the dialog to be corrected
    /// and left out of the store. Nothing is written when nothing changed.
    pub fn commit(&mut self, typed: &str) {
        let Some(callsign) = normalize_callsign(typed) else {
            return;
        };
        let Some(mut record) = Record::new(&callsign) else {
            return;
        };
        let mut dropped = Vec::new();
        for row in &self.draft {
            if !valid_key(&row.key) {
                continue;
            }
            if record.set(&row.key, &row.value) {
                continue;
            }
            if self.snapshot.fields.contains_key(&row.key) {
                dropped.push(row.key.clone());
            }
        }
        let unchanged = record.iter().eq(self
            .snapshot
            .fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())));
        if unchanged && dropped.is_empty() {
            return;
        }
        self.worker.save(record, dropped);
    }

    /// Reads what the worker published, returning whether the fields changed.
    pub fn poll(&mut self) -> bool {
        let latest = self.worker.latest();
        let changed = latest.fields != self.snapshot.fields;
        self.snapshot = latest;
        changed
    }
}

impl core::fmt::Debug for ContactEditor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ContactEditor")
            .field("snapshot", &self.snapshot)
            .field("dialog_open", &self.dialog_open)
            .finish_non_exhaustive()
    }
}
