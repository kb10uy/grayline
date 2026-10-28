use crate::{
    error::QsoError,
    record::{Record, normalize_callsign},
    store::{Origin, Store},
};

/// What the directory answered about a callsign.
#[derive(Debug)]
pub struct Lookup {
    /// Everything filed under the callsign.
    pub record: Record,
    /// Whether anything at all is filed under it.
    pub known: bool,
}

/// The stations the operator knows about.
#[derive(Debug)]
pub struct Directory {
    store: Store,
}

impl Directory {
    /// Composes a directory over `store`.
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Answers what is filed under `callsign`.
    pub fn look_up(&self, callsign: &str) -> Result<Lookup, QsoError> {
        let callsign = normalize_callsign(callsign).ok_or_else(|| QsoError::Callsign(callsign.to_owned()))?;
        let record = self.store.get(&callsign)?;
        Ok(Lookup {
            known: record.as_ref().is_some_and(|record| !record.is_empty()),
            record: record.unwrap_or_else(|| Record::new(&callsign).expect("it normalized")),
        })
    }

    /// Files what the operator typed, which no import will displace.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filed(store: &mut Store, callsign: &str, key: &str, value: &str, origin: Origin) {
        let mut record = Record::new(callsign).expect("that is a callsign");
        assert!(record.set(key, value));
        store.merge(&record, origin).expect("a write");
    }

    #[test]
    fn a_station_the_store_holds_is_answered_from_it() {
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "Taro", Origin::Manual);
        let directory = Directory::new(store);

        let lookup = directory.look_up("ja1abc").expect("an answer");

        assert!(lookup.known);
        assert_eq!(lookup.record.get("name"), Some("Taro"));
    }

    #[test]
    fn a_station_nothing_is_known_about_answers_an_empty_record_rather_than_nothing() {
        let directory = Directory::new(Store::in_memory().expect("a store"));

        let lookup = directory.look_up("ja1abc").expect("an answer");

        assert!(!lookup.known);
        assert_eq!(lookup.record.callsign(), "JA1ABC");
        assert!(lookup.record.is_empty());
    }

    #[test]
    fn what_the_operator_saves_displaces_an_import() {
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "TARO Y", Origin::Adif);
        filed(&mut store, "JA1ABC", "qth", "Tokyo", Origin::Adif);
        let mut directory = Directory::new(store);
        let mut typed = Record::new("JA1ABC").expect("that is a callsign");
        typed.set("name", "Taro-san");
        directory.save(&typed).expect("a save");

        let lookup = directory.look_up("JA1ABC").expect("an answer");

        assert_eq!(lookup.record.get("name"), Some("Taro-san"));
        assert_eq!(lookup.record.get("qth"), Some("Tokyo"));
    }

    #[test]
    fn forgetting_a_station_leaves_the_directory_with_nothing_to_say() {
        let mut store = Store::in_memory().expect("a store");
        filed(&mut store, "JA1ABC", "name", "Taro", Origin::Manual);
        let mut directory = Directory::new(store);

        assert!(directory.forget("JA1ABC").expect("a removal"));
        assert!(!directory.look_up("JA1ABC").expect("an answer").known);
    }

    #[test]
    fn a_callsign_that_is_not_one_is_refused() {
        let directory = Directory::new(Store::in_memory().expect("a store"));

        let error = directory.look_up("??").expect_err("a refusal");

        assert!(matches!(error, QsoError::Callsign(_)), "{error}");
    }
}
