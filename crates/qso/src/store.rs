use std::{
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    error::QsoError,
    record::{Record, normalize_callsign},
};

/// The shape of the store this build reads and writes.
///
/// Held in SQLite's own `user_version` rather than in a table of its own,
/// because the counter already exists and a table would be a second one.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS station (
    callsign     TEXT PRIMARY KEY NOT NULL,
    looked_up_at TEXT,
    found        INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE IF NOT EXISTS station_field (
    callsign   TEXT NOT NULL REFERENCES station(callsign) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    origin     TEXT NOT NULL,
    written_at TEXT NOT NULL,
    PRIMARY KEY (callsign, key)
) STRICT, WITHOUT ROWID;
";

/// Where a stored field came from, and so what may replace it.
///
/// The order is the rule: a field is overwritten only by a source at least as
/// strong as the one that wrote it, so nothing an import or a lookup does can
/// displace what the operator typed.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Origin {
    /// Read out of an ADIF log the operator already had.
    Adif,
    /// Answered by the operator's own logger.
    Wavelog,
    /// Entered by the operator.
    Manual,
}

impl Origin {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Adif => "adif",
            Self::Wavelog => "wavelog",
            Self::Manual => "manual",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Origin {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "adif" => Ok(Self::Adif),
            "wavelog" => Ok(Self::Wavelog),
            "manual" => Ok(Self::Manual),
            _ => Err(()),
        }
    }
}

/// The stations the operator has filed something about.
#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens the store at `path`, creating it and its directory if neither is
    /// there yet.
    pub fn open(path: &Path) -> Result<Self, QsoError> {
        if let Some(directory) = path.parent()
            && !directory.as_os_str().is_empty()
        {
            fs::create_dir_all(directory).map_err(|error| QsoError::Open {
                path: path.to_owned(),
                detail: error.to_string(),
            })?;
        }
        let connection = Connection::open(path).map_err(|error| QsoError::Open {
            path: path.to_owned(),
            detail: error.to_string(),
        })?;
        Self::prepare(connection, path)
    }

    /// Opens a store that lives only as long as it is held, for tests and for
    /// an application asked not to keep one.
    pub fn in_memory() -> Result<Self, QsoError> {
        let connection = Connection::open_in_memory().map_err(|error| QsoError::Open {
            path: PathBuf::from(":memory:"),
            detail: error.to_string(),
        })?;
        Self::prepare(connection, Path::new(":memory:"))
    }

    fn prepare(connection: Connection, path: &Path) -> Result<Self, QsoError> {
        // Write-ahead logging rather than the default rollback journal, because
        // two applications in this family may be running at once and under the
        // journal a reader blocks a writer.
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 3000; PRAGMA synchronous = NORMAL;")
            .map_err(|error| QsoError::Open {
                path: path.to_owned(),
                detail: error.to_string(),
            })?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(store_error)?;

        let found: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(store_error)?;
        if found > SCHEMA_VERSION {
            return Err(QsoError::Version {
                found,
                known: SCHEMA_VERSION,
            });
        }
        connection.execute_batch(SCHEMA).map_err(store_error)?;
        connection
            .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
            .map_err(store_error)?;
        Ok(Self { connection })
    }

    /// What is filed under `callsign`, or nothing when it is not filed at all.
    pub fn get(&self, callsign: &str) -> Result<Option<Record>, QsoError> {
        let callsign = normalized(callsign)?;
        let filed: Option<i64> = self
            .connection
            .query_row("SELECT 1 FROM station WHERE callsign = ?1", params![callsign], |row| {
                row.get(0)
            })
            .optional()
            .map_err(store_error)?;
        if filed.is_none() {
            return Ok(None);
        }

        let mut record = Record::new(&callsign).ok_or_else(|| QsoError::Callsign(callsign.clone()))?;
        let mut statement = self
            .connection
            .prepare("SELECT key, value FROM station_field WHERE callsign = ?1 ORDER BY key")
            .map_err(store_error)?;
        let rows = statement
            .query_map(params![callsign], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(store_error)?;
        for row in rows {
            let (key, value) = row.map_err(store_error)?;
            record.set(&key, &value);
        }
        Ok(Some(record))
    }

    /// Writes every field of `record` at `origin`, leaving alone any field a
    /// stronger origin already wrote. Answers how many were written.
    pub fn merge(&mut self, record: &Record, origin: Origin) -> Result<usize, QsoError> {
        self.merge_all(std::slice::from_ref(record), origin)
    }

    /// Writes many records at once, under one transaction.
    ///
    /// One transaction rather than one per station, so that an import
    /// interrupted halfway leaves the store as it was rather than half filled
    /// with a log the operator would then have to import again to complete.
    pub fn merge_all(&mut self, records: &[Record], origin: Origin) -> Result<usize, QsoError> {
        let transaction = self.connection.transaction().map_err(store_error)?;
        let now = Timestamp::now().to_string();
        let mut written = 0;

        for record in records {
            transaction
                .execute(
                    "INSERT INTO station (callsign) VALUES (?1) ON CONFLICT (callsign) DO NOTHING",
                    params![record.callsign()],
                )
                .map_err(store_error)?;
            written += merge_fields(&transaction, record, origin, &now)?;
        }

        transaction.commit().map_err(store_error)?;
        Ok(written)
    }

    /// Forgets a station outright, fields and lookup history alike.
    pub fn remove(&mut self, callsign: &str) -> Result<bool, QsoError> {
        let callsign = normalized(callsign)?;
        let removed = self
            .connection
            .execute("DELETE FROM station WHERE callsign = ?1", params![callsign])
            .map_err(store_error)?;
        Ok(removed > 0)
    }

    /// The filed callsigns starting with `prefix`, in order, at most `limit` of
    /// them. An empty prefix lists everything.
    pub fn list(&self, prefix: &str, limit: usize) -> Result<Vec<String>, QsoError> {
        let prefix = prefix.trim().to_ascii_uppercase();
        let mut statement = self
            .connection
            .prepare("SELECT callsign FROM station WHERE callsign LIKE ?1 ESCAPE '\\' ORDER BY callsign LIMIT ?2")
            .map_err(store_error)?;
        let pattern = format!("{}%", escape_like(&prefix));
        let rows = statement
            .query_map(params![pattern, limit as i64], |row| row.get::<_, String>(0))
            .map_err(store_error)?;
        rows.collect::<Result<_, _>>().map_err(store_error)
    }

    /// When a remote lookup for `callsign` last completed, and whether it found
    /// anything. Nothing when one never has.
    pub fn last_lookup(&self, callsign: &str) -> Result<Option<(Timestamp, bool)>, QsoError> {
        let callsign = normalized(callsign)?;
        let row: Option<(Option<String>, i64)> = self
            .connection
            .query_row(
                "SELECT looked_up_at, found FROM station WHERE callsign = ?1",
                params![callsign],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(store_error)?;
        let Some((Some(at), found)) = row else {
            return Ok(None);
        };
        // A timestamp the file holds that will not parse is read as no lookup
        // at all, which asks again rather than refusing to answer.
        Ok(at.parse::<Timestamp>().ok().map(|at| (at, found != 0)))
    }

    /// Notes that a remote lookup for `callsign` has just completed.
    pub fn record_lookup(&mut self, callsign: &str, found: bool) -> Result<(), QsoError> {
        let callsign = normalized(callsign)?;
        let now = Timestamp::now().to_string();
        self.connection
            .execute(
                "INSERT INTO station (callsign, looked_up_at, found) VALUES (?1, ?2, ?3)
                 ON CONFLICT (callsign) DO UPDATE SET looked_up_at = excluded.looked_up_at, found = excluded.found",
                params![callsign, now, i64::from(found)],
            )
            .map_err(store_error)?;
        Ok(())
    }

    /// Folds the write-ahead log back into the file.
    ///
    /// Called when the store is being put down for the last time, so that what
    /// a roaming profile copies is one self-contained file rather than a file
    /// and the log that completes it.
    pub fn checkpoint(&self) -> Result<(), QsoError> {
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(store_error)
    }
}

fn merge_fields(
    transaction: &rusqlite::Transaction<'_>,
    record: &Record,
    origin: Origin,
    now: &str,
) -> Result<usize, QsoError> {
    let mut written = 0;
    for (key, value) in record.iter() {
        let existing: Option<String> = transaction
            .query_row(
                "SELECT origin FROM station_field WHERE callsign = ?1 AND key = ?2",
                params![record.callsign(), key],
                |row| row.get(0),
            )
            .optional()
            .map_err(store_error)?;
        // An origin the file holds that this build does not know is read as the
        // strongest there is: a newer build wrote it, and guessing it weaker
        // would be a way to lose it.
        let outranked = match existing.as_deref().map(Origin::from_str) {
            None => true,
            Some(Ok(existing)) => origin >= existing,
            Some(Err(())) => false,
        };
        if !outranked {
            continue;
        }
        transaction
            .execute(
                "INSERT INTO station_field (callsign, key, value, origin, written_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (callsign, key) DO UPDATE
                 SET value = excluded.value, origin = excluded.origin, written_at = excluded.written_at",
                params![record.callsign(), key, value, origin.as_str(), now],
            )
            .map_err(store_error)?;
        written += 1;
    }
    Ok(written)
}

fn normalized(callsign: &str) -> Result<String, QsoError> {
    normalize_callsign(callsign).ok_or_else(|| QsoError::Callsign(callsign.to_owned()))
}

fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn store_error(error: rusqlite::Error) -> QsoError {
    QsoError::Store(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU32, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("after 1970")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("grayline-qso-{stamp}-{unique}"));
            fs::create_dir_all(&path).expect("a temporary directory");
            Self(path)
        }

        fn store(&self) -> PathBuf {
            self.0.join("contacts.sqlite3")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record(callsign: &str, fields: &[(&str, &str)]) -> Record {
        let mut record = Record::new(callsign).expect("that is a callsign");
        for (key, value) in fields {
            assert!(record.set(key, value), "{key}");
        }
        record
    }

    #[test]
    fn a_stored_record_is_read_back_as_it_was_written() {
        let mut store = Store::in_memory().expect("a store");
        store
            .merge(&record("JA1ABC", &[("name", "Taro"), ("qth", "Tokyo")]), Origin::Manual)
            .expect("a write");

        let read = store.get("ja1abc").expect("a read").expect("it was filed");
        assert_eq!(read.callsign(), "JA1ABC");
        assert_eq!(read.get("name"), Some("Taro"));
        assert_eq!(read.get("qth"), Some("Tokyo"));
    }

    #[test]
    fn a_callsign_nothing_is_filed_under_reads_as_nothing() {
        let store = Store::in_memory().expect("a store");
        assert_eq!(store.get("JA1ABC").expect("a read"), None);
    }

    #[test]
    fn a_store_survives_being_put_down_and_opened_again() {
        let directory = TempDir::new();
        {
            let mut store = Store::open(&directory.store()).expect("a store");
            store
                .merge(&record("JA1ABC", &[("name", "Taro")]), Origin::Manual)
                .expect("a write");
            store.checkpoint().expect("a checkpoint");
        }

        let store = Store::open(&directory.store()).expect("the same store");
        let read = store.get("JA1ABC").expect("a read").expect("it was filed");
        assert_eq!(read.get("name"), Some("Taro"));
    }

    #[test]
    fn a_lookup_does_not_displace_what_the_operator_typed() {
        let mut store = Store::in_memory().expect("a store");
        store
            .merge(&record("JA1ABC", &[("name", "Taro")]), Origin::Manual)
            .expect("a write");
        let written = store
            .merge(
                &record("JA1ABC", &[("name", "TARO Y"), ("qth", "Tokyo")]),
                Origin::Wavelog,
            )
            .expect("a write");

        assert_eq!(written, 1, "only the field nothing stronger had written");
        let read = store.get("JA1ABC").expect("a read").expect("it was filed");
        assert_eq!(read.get("name"), Some("Taro"));
        assert_eq!(read.get("qth"), Some("Tokyo"));
    }

    #[test]
    fn the_operator_displaces_what_a_lookup_wrote() {
        let mut store = Store::in_memory().expect("a store");
        store
            .merge(&record("JA1ABC", &[("name", "TARO Y")]), Origin::Wavelog)
            .expect("a write");
        store
            .merge(&record("JA1ABC", &[("name", "Taro")]), Origin::Manual)
            .expect("a write");

        let read = store.get("JA1ABC").expect("a read").expect("it was filed");
        assert_eq!(read.get("name"), Some("Taro"));
    }

    #[test]
    fn a_later_import_displaces_an_earlier_one() {
        let mut store = Store::in_memory().expect("a store");
        store
            .merge(&record("JA1ABC", &[("qth", "Chiba")]), Origin::Adif)
            .expect("a write");
        store
            .merge(&record("JA1ABC", &[("qth", "Tokyo")]), Origin::Adif)
            .expect("a write");

        let read = store.get("JA1ABC").expect("a read").expect("it was filed");
        assert_eq!(read.get("qth"), Some("Tokyo"));
    }

    #[test]
    fn forgetting_a_station_takes_its_fields_with_it() {
        let mut store = Store::in_memory().expect("a store");
        store
            .merge(&record("JA1ABC", &[("name", "Taro")]), Origin::Manual)
            .expect("a write");

        assert!(store.remove("JA1ABC").expect("a removal"));
        assert_eq!(store.get("JA1ABC").expect("a read"), None);
        assert!(!store.remove("JA1ABC").expect("a second removal"));

        let orphans: i64 = store
            .connection
            .query_row("SELECT count(*) FROM station_field", [], |row| row.get(0))
            .expect("a count");
        assert_eq!(orphans, 0);
    }

    #[test]
    fn a_listing_answers_the_callsigns_a_prefix_names() {
        let mut store = Store::in_memory().expect("a store");
        for callsign in ["JA1ABC", "JA1XYZ", "JH1ABC"] {
            store
                .merge(&record(callsign, &[("name", "x")]), Origin::Manual)
                .expect("a write");
        }

        assert_eq!(store.list("ja1", 10).expect("a listing"), ["JA1ABC", "JA1XYZ"]);
        assert_eq!(store.list("", 2).expect("a listing"), ["JA1ABC", "JA1XYZ"]);
        assert!(store.list("W1", 10).expect("a listing").is_empty());
    }

    #[test]
    fn a_wildcard_in_a_prefix_is_matched_literally() {
        let mut store = Store::in_memory().expect("a store");
        store
            .merge(&record("JA1ABC", &[("name", "x")]), Origin::Manual)
            .expect("a write");

        assert!(store.list("%", 10).expect("a listing").is_empty());
    }

    #[test]
    fn a_lookup_is_noted_against_the_station_it_was_for() {
        let mut store = Store::in_memory().expect("a store");
        assert_eq!(store.last_lookup("JA1ABC").expect("a read"), None);

        store.record_lookup("JA1ABC", false).expect("a note");
        let (_, found) = store.last_lookup("JA1ABC").expect("a read").expect("it was noted");
        assert!(!found);

        store.record_lookup("JA1ABC", true).expect("a second note");
        let (_, found) = store.last_lookup("JA1ABC").expect("a read").expect("it was noted");
        assert!(found);
    }

    /// A station a lookup found nothing for is one the store has to remember
    /// asking about, so that it does not ask again on the next contact.
    #[test]
    fn a_fruitless_lookup_files_a_station_with_no_fields() {
        let mut store = Store::in_memory().expect("a store");
        store.record_lookup("JA1ABC", false).expect("a note");

        let read = store.get("JA1ABC").expect("a read").expect("it was noted");
        assert!(read.is_empty());
    }

    #[test]
    fn two_processes_may_hold_the_same_store() {
        let directory = TempDir::new();
        let mut one = Store::open(&directory.store()).expect("a store");
        let mut two = Store::open(&directory.store()).expect("the same store");

        one.merge(&record("JA1ABC", &[("name", "Taro")]), Origin::Manual)
            .expect("a write");
        two.merge(&record("JH1XYZ", &[("name", "Jiro")]), Origin::Manual)
            .expect("a write");

        assert_eq!(one.list("", 10).expect("a listing"), ["JA1ABC", "JH1XYZ"]);
        assert_eq!(two.list("", 10).expect("a listing"), ["JA1ABC", "JH1XYZ"]);
    }

    #[test]
    fn a_store_a_newer_build_wrote_is_refused_rather_than_opened() {
        let directory = TempDir::new();
        {
            let store = Store::open(&directory.store()).expect("a store");
            store
                .connection
                .execute_batch("PRAGMA user_version = 2;")
                .expect("a version bump");
        }

        let error = Store::open(&directory.store()).expect_err("a refusal");
        assert!(matches!(error, QsoError::Version { found: 2, known: 1 }), "{error}");
    }

    #[test]
    fn a_callsign_that_is_not_one_is_refused_rather_than_filed() {
        let store = Store::in_memory().expect("a store");
        let error = store.get("??").expect_err("a refusal");
        assert!(matches!(error, QsoError::Callsign(_)), "{error}");
    }
}
