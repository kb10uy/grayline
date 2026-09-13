use std::time::Duration;

use jiff::Timestamp;

use crate::{
    error::QsoError,
    record::{Record, normalize_callsign},
    store::{Origin, Store},
    wavelog::Wavelog,
};

/// How long an answer stands before the same callsign is asked about again.
///
/// A month, because what this keeps is what a station *is*: a name does not
/// change, and a QTH changes on the scale of house moves rather than contacts.
pub const HIT_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 30);

/// How long an answer of "nothing" stands, which is shorter.
///
/// A station absent from the callbook last week may be in it today, and a new
/// licensee is exactly the operator whose details are worth having.
pub const MISS_TTL: Duration = Duration::from_secs(60 * 60 * 24);

/// What the directory answered about a callsign.
#[derive(Debug)]
pub struct Lookup {
    /// Everything filed under the callsign, after any fetch was folded in.
    pub record: Record,
    /// Whether anything at all is filed under it.
    pub known: bool,
    /// Set when the remote was asked and refused.
    ///
    /// The local answer is still returned beside it: a logger that is down is
    /// no reason to forget what the store already holds, and the operator with
    /// no network is the one who most needs what was cached.
    pub remote_error: Option<QsoError>,
}

/// The stations the operator knows about, and where to ask about the rest.
#[derive(Debug)]
pub struct Directory {
    store: Store,
    remote: Option<Wavelog>,
}

impl Directory {
    /// Composes a directory over `store`, asking `remote` about what it does
    /// not hold. With no remote it answers from the store alone, which is what
    /// an operator who has not configured a logger — or has switched the
    /// lookup off — is left with.
    pub fn new(store: Store, remote: Option<Wavelog>) -> Self {
        Self { store, remote }
    }

    /// Answers what is known about `callsign`.
    ///
    /// The store first, and the remote only when the store has nothing recent
    /// enough to say. `refresh` asks the remote whatever the store holds, which
    /// is what the operator means by looking a station up again.
    pub fn look_up(&mut self, callsign: &str, refresh: bool) -> Result<Lookup, QsoError> {
        let callsign = normalize_callsign(callsign).ok_or_else(|| QsoError::Callsign(callsign.to_owned()))?;
        let filed = self.store.get(&callsign)?;

        let mut remote_error = None;
        if self.should_ask(&callsign, filed.as_ref(), refresh)? {
            match self
                .remote
                .as_ref()
                .expect("asking requires a remote")
                .look_up(&callsign)
            {
                Ok(answer) => {
                    if let Some(answer) = &answer {
                        self.store.merge(answer, Origin::Wavelog)?;
                    }
                    self.store.record_lookup(&callsign, answer.is_some())?;
                }
                // A failure is reported beside the local answer rather than
                // instead of it, and is not noted as a lookup: nothing was
                // learned, so the next attempt should not be held off.
                Err(error) => remote_error = Some(error),
            }
        }

        // Read again rather than merging in hand: the store is what decides
        // whether a fetched field displaced a typed one, and asking it is what
        // keeps that rule in one place.
        let record = self.store.get(&callsign)?;
        Ok(Lookup {
            known: record.as_ref().is_some_and(|record| !record.is_empty()),
            record: record.unwrap_or_else(|| Record::new(&callsign).expect("it normalized")),
            remote_error,
        })
    }

    /// Files what the operator typed, which no lookup will displace.
    pub fn save(&mut self, record: &Record) -> Result<(), QsoError> {
        self.store.merge(record, Origin::Manual)?;
        Ok(())
    }

    /// Drops one field the operator emptied, whatever origin wrote it.
    ///
    /// Emptying a field is not the same as leaving it alone: a save writes what
    /// is filled in, and a value the operator cleared would otherwise come back
    /// the next time the record was read.
    pub fn unset(&mut self, callsign: &str, key: &str) -> Result<bool, QsoError> {
        self.store.unset(callsign, key)
    }

    /// Forgets a station outright.
    pub fn forget(&mut self, callsign: &str) -> Result<bool, QsoError> {
        self.store.remove(callsign)
    }

    /// The store underneath, for a caller that browses rather than looks up.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The store underneath, for a caller that writes at an origin of its own.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Whether the remote is worth asking about `callsign` now.
    fn should_ask(&self, callsign: &str, filed: Option<&Record>, refresh: bool) -> Result<bool, QsoError> {
        if self.remote.is_none() {
            return Ok(false);
        }
        if refresh {
            return Ok(true);
        }
        let Some((at, found)) = self.store.last_lookup(callsign)? else {
            // Never asked. A station filed by hand or by an import is left
            // alone: the operator put those there, and asking a logger to
            // confirm them is not what filing them meant.
            return Ok(filed.is_none_or(Record::is_empty));
        };
        let ttl = if found { HIT_TTL } else { MISS_TTL };
        // A file whose timestamp sits in the future — a clock that was wrong,
        // or a store carried from another machine — would otherwise hold the
        // lookup off for as long as the difference.
        let elapsed = Timestamp::now().duration_since(at);
        Ok(elapsed.is_negative() || elapsed.unsigned_abs() > ttl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::FakeWavelog;

    const ANSWER: &str = r#"{"name":"Taro","location":"Tokyo","gridsquare":"PM95TQ"}"#;
    const NOTHING: &str = r#"{"callsign":"JA1ABC","name":"","gridsquare":""}"#;

    fn filed(store: &mut Store, callsign: &str, key: &str, value: &str, origin: Origin) {
        let mut record = Record::new(callsign).expect("that is a callsign");
        assert!(record.set(key, value));
        store.merge(&record, origin).expect("a write");
    }

    #[test]
    fn a_station_the_store_holds_is_answered_without_asking_anybody() {
        let server = FakeWavelog::spawn(&[(200, ANSWER)]);
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "Taro", Origin::Manual);
        let mut directory = Directory::new(store, Some(server.client()));

        let lookup = directory.look_up("ja1abc", false).expect("an answer");

        assert!(lookup.known);
        assert_eq!(lookup.record.get("name"), Some("Taro"));
        assert!(server.requested().is_empty(), "nothing should have been asked");
    }

    #[test]
    fn a_station_the_store_does_not_hold_is_asked_about_and_then_kept() {
        let server = FakeWavelog::spawn(&[(200, ANSWER)]);
        let mut directory = Directory::new(Store::in_memory().expect("a store"), Some(server.client()));

        let lookup = directory.look_up("JA1ABC", false).expect("an answer");
        assert!(lookup.known);
        assert_eq!(lookup.record.get("qth"), Some("Tokyo"));

        let again = directory.look_up("JA1ABC", false).expect("an answer");
        assert_eq!(again.record.get("qth"), Some("Tokyo"));
        assert_eq!(server.requested().len(), 1);
    }

    #[test]
    fn a_station_nobody_knows_is_not_asked_about_twice_in_a_row() {
        let server = FakeWavelog::spawn(&[(200, NOTHING)]);
        let mut directory = Directory::new(Store::in_memory().expect("a store"), Some(server.client()));

        assert!(!directory.look_up("JA1ABC", false).expect("an answer").known);
        assert!(!directory.look_up("JA1ABC", false).expect("an answer").known);

        assert_eq!(server.requested().len(), 1);
    }

    #[test]
    fn looking_a_station_up_again_asks_whatever_the_store_holds() {
        let server = FakeWavelog::spawn(&[(200, ANSWER)]);
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "Taro", Origin::Manual);
        let mut directory = Directory::new(store, Some(server.client()));

        let lookup = directory.look_up("JA1ABC", true).expect("an answer");

        assert_eq!(server.requested().len(), 1);
        assert_eq!(lookup.record.get("qth"), Some("Tokyo"));
        assert_eq!(lookup.record.get("name"), Some("Taro"), "typed by hand, so it stands");
    }

    #[test]
    fn a_logger_that_is_down_does_not_take_the_store_with_it() {
        let server = FakeWavelog::spawn(&[(503, "")]);
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "Taro", Origin::Adif);
        let mut directory = Directory::new(store, Some(server.client()));

        let lookup = directory.look_up("JA1ABC", true).expect("an answer");

        assert!(lookup.known);
        assert_eq!(lookup.record.get("name"), Some("Taro"));
        assert!(matches!(lookup.remote_error, Some(QsoError::Refused(503))));
    }

    #[test]
    fn a_failed_lookup_is_not_counted_as_having_asked() {
        let server = FakeWavelog::spawn(&[(503, ""), (200, ANSWER)]);
        let mut directory = Directory::new(Store::in_memory().expect("a store"), Some(server.client()));

        assert!(
            directory
                .look_up("JA1ABC", false)
                .expect("an answer")
                .remote_error
                .is_some()
        );
        let lookup = directory.look_up("JA1ABC", false).expect("an answer");

        assert!(lookup.known);
        assert_eq!(server.requested().len(), 2);
    }

    #[test]
    fn a_directory_with_no_remote_answers_from_the_store_alone() {
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "Taro", Origin::Manual);
        let mut directory = Directory::new(store, None);

        assert!(directory.look_up("JA1ABC", false).expect("an answer").known);
        assert!(!directory.look_up("JH1XYZ", true).expect("an answer").known);
    }

    #[test]
    fn a_station_nothing_is_known_about_answers_an_empty_record_rather_than_nothing() {
        let mut directory = Directory::new(Store::in_memory().expect("a store"), None);

        let lookup = directory.look_up("ja1abc", false).expect("an answer");

        assert!(!lookup.known);
        assert_eq!(lookup.record.callsign(), "JA1ABC");
        assert!(lookup.record.is_empty());
    }

    #[test]
    fn what_the_operator_saves_is_theirs_and_stays_theirs() {
        let server = FakeWavelog::spawn(&[(200, ANSWER)]);
        let mut directory = Directory::new(Store::in_memory().expect("a store"), Some(server.client()));
        let mut typed = Record::new("JA1ABC").expect("that is a callsign");
        typed.set("name", "Taro-san");
        directory.save(&typed).expect("a save");

        let lookup = directory.look_up("JA1ABC", true).expect("an answer");

        assert_eq!(lookup.record.get("name"), Some("Taro-san"));
        assert_eq!(lookup.record.get("qth"), Some("Tokyo"));
    }

    #[test]
    fn forgetting_a_station_leaves_the_directory_with_nothing_to_say() {
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "Taro", Origin::Manual);
        let mut directory = Directory::new(store, None);

        assert!(directory.forget("JA1ABC").expect("a removal"));
        assert!(!directory.look_up("JA1ABC", false).expect("an answer").known);
    }

    #[test]
    fn a_callsign_that_is_not_one_never_reaches_the_logger() {
        let server = FakeWavelog::spawn(&[(200, ANSWER)]);
        let mut directory = Directory::new(Store::in_memory().expect("a store"), Some(server.client()));

        let error = directory.look_up("??", false).expect_err("a refusal");

        assert!(matches!(error, QsoError::Callsign(_)), "{error}");
        assert!(server.requested().is_empty());
    }
}
