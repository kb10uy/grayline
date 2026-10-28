use std::{sync::mpsc, time::Duration};

use tempfile::TempDir;

use super::{ContactPaths, ContactState, ContactWorker, Request, collapse_lookups};
use crate::{QsoError, Record, Store};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn record(callsign: &str, name: &str) -> Record {
    let mut record = Record::new(callsign).unwrap();
    record.set("name", name);
    record
}

#[test]
fn collapsing_keeps_the_last_lookup_and_every_save_in_order() {
    let mut requests = [
        Request::Save {
            record: record("JA1AAA", "First"),
            dropped: vec!["qth".to_owned()],
        },
        Request::LookUp("JA1AAA".to_owned()),
        Request::Save {
            record: record("JA1BBB", "Second"),
            dropped: vec![],
        },
        Request::LookUp("JA1BBB".to_owned()),
        Request::Save {
            record: record("JA1CCC", "Third"),
            dropped: vec![],
        },
    ]
    .into();
    collapse_lookups(&mut requests);
    assert_eq!(requests.len(), 4);
    let saved: Vec<_> = requests
        .iter()
        .filter_map(|request| match request {
            Request::Save { record, .. } => Some(record.callsign()),
            _ => None,
        })
        .collect();
    assert_eq!(saved, ["JA1AAA", "JA1BBB", "JA1CCC"]);
    assert!(matches!(&requests[0], Request::Save { dropped, .. } if dropped == &["qth"]));
    assert!(matches!(&requests[2], Request::LookUp(callsign) if callsign == "JA1BBB"));
    requests.remove(2);
    collapse_lookups(&mut requests);
    assert_eq!(requests.len(), 3);
}

#[test]
fn saves_publish_changes_and_drop_drains_writes_to_disk() {
    let root = TempDir::new().unwrap();
    let paths = ContactPaths {
        store: Some(root.path().join(crate::STORE_FILE)),
    };
    let (sent, received) = mpsc::channel();
    let worker = ContactWorker::spawn(paths.open(), "contact-test".to_owned(), move || {
        sent.send(()).unwrap();
    });
    worker.look_up("JA1AAA");
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(worker.latest().state, ContactState::Unknown);
    worker.save(record("JA1AAA", "First"), vec![]);
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(worker.latest().fields["name"], "First");
    worker.save(Record::new("JA1AAA").unwrap(), vec!["name".to_owned()]);
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(worker.latest().state, ContactState::Unknown);
    assert!(worker.latest().fields.is_empty());
    worker.save(record("JA1BBB", "Second"), vec![]);
    worker.save(record("JA1CCC", "Third"), vec![]);
    drop(worker);
    let store = Store::open(&root.path().join(crate::STORE_FILE)).unwrap();
    assert!(store.get("JA1AAA").unwrap().unwrap().is_empty());
    assert_eq!(store.get("JA1BBB").unwrap().unwrap().get("name"), Some("Second"));
    assert_eq!(store.get("JA1CCC").unwrap().unwrap().get("name"), Some("Third"));
}

#[test]
fn an_open_failure_is_kept_in_the_snapshot() {
    let mut worker = ContactWorker::spawn(
        Err(QsoError::Store("cannot open".to_owned())),
        "contact-test".to_owned(),
        || {},
    );
    worker.thread.take().unwrap().join().unwrap();
    let snapshot = worker.latest();
    assert_eq!(snapshot.state, ContactState::Failed);
    assert!(matches!(snapshot.error.as_deref(), Some(QsoError::Store(message)) if message == "cannot open"));
}
