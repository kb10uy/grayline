use std::sync::mpsc;

use super::{ContactPaths, ContactState, ContactWorker, Request, collapse_lookups};
use crate::{
    Directory, QsoError, Record, Store,
    test_util::{FakeWavelog, TEST_TIMEOUT, TempDir},
};

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
        Request::LookUp {
            callsign: "JA1AAA".to_owned(),
            refresh: false,
        },
        Request::Save {
            record: record("JA1BBB", "Second"),
            dropped: vec![],
        },
        Request::LookUp {
            callsign: "JA1BBB".to_owned(),
            refresh: true,
        },
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
    assert!(matches!(&requests[2], Request::LookUp { callsign, refresh: true } if callsign == "JA1BBB"));
    requests.remove(2);
    collapse_lookups(&mut requests);
    assert_eq!(requests.len(), 3);
}

#[test]
fn saves_publish_changes_and_drop_drains_writes_to_disk() {
    let root = TempDir::new();
    let paths = ContactPaths {
        store: Some(root.store()),
        credentials: None,
    };
    let (sent, received) = mpsc::channel();
    let worker = ContactWorker::spawn(paths.open(false, ""), "contact-test".to_owned(), move || {
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
    let store = Store::open(&root.store()).unwrap();
    assert!(store.get("JA1AAA").unwrap().unwrap().is_empty());
    assert_eq!(store.get("JA1BBB").unwrap().unwrap().get("name"), Some("Second"));
    assert_eq!(store.get("JA1CCC").unwrap().unwrap().get("name"), Some("Third"));
}

#[test]
fn a_remote_failure_is_published_beside_the_local_answer() {
    let server = FakeWavelog::spawn(&[(503, "")]);
    let mut directory = Directory::new(Store::in_memory().unwrap(), Some(server.client()));
    directory.save(&record("JA1AAA", "Filed")).unwrap();
    let (sent, received) = mpsc::channel();
    let worker = ContactWorker::spawn(Ok(directory), "contact-test".to_owned(), move || {
        sent.send(()).unwrap();
    });
    worker.refresh("JA1AAA");
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    let snapshot = worker.latest();
    assert_eq!(snapshot.state, ContactState::Known);
    assert_eq!(snapshot.fields["name"], "Filed");
    assert!(matches!(snapshot.error.as_deref(), Some(QsoError::Refused(503))));
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

#[test]
fn absent_paths_keep_a_lookup_offline_even_with_a_url() {
    let mut directory = ContactPaths::default().open(true, "http://127.0.0.1:1").unwrap();
    assert!(directory.look_up("JA1AAA", true).unwrap().remote_error.is_none());
}
