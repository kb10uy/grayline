use std::collections::BTreeMap;

use encoding_rs::Encoding;
use jiff::Timestamp;

use crate::{
    error::QsoError,
    record::Record,
    store::{Origin, Store},
};

/// Which ADIF field each well-known key is read out of.
///
/// Only what a station *is*: the band, the mode, the reports and the
/// confirmations describe a contact, and this is a directory rather than a log.
/// The operator's own side of the exchange is left out for the same reason —
/// `OPERATOR`, `STATION_CALLSIGN` and everything under `MY_` describes the
/// station reading the file rather than the one it is filed under.
const FIELDS: [(&str, &str); 14] = [
    ("NAME", "name"),
    ("QTH", "qth"),
    ("GRIDSQUARE", "grid"),
    ("COUNTRY", "dxcc"),
    ("DXCC", "dxcc_id"),
    ("CQZ", "cq_zone"),
    ("ITUZ", "itu_zone"),
    ("CONT", "continent"),
    ("STATE", "state"),
    ("CNTY", "county"),
    ("IOTA", "iota"),
    ("QSL_VIA", "qsl_manager"),
    ("EMAIL", "email"),
    ("COMMENT", "note"),
];

/// What `note` falls back to when the QSO carried no `COMMENT`.
const NOTE_FALLBACK: &str = "NOTES";

/// One QSO as an ADIF document spells it, reduced to what a directory keeps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdifRecord {
    /// The station worked, and what the QSO said about it.
    pub record: Record,
    /// When the contact took place, when the document said.
    ///
    /// Kept only to order one QSO against another, never filed: which contact
    /// a value came from decides whether it is still true, but that a contact
    /// happened is not something this store records.
    pub worked_at: Option<Timestamp>,
}

/// What one ADIF document held.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Adif {
    /// Every QSO read, in document order.
    pub records: Vec<AdifRecord>,
    /// What was passed over, and why.
    pub skipped: Vec<String>,
}

/// What an import wrote.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportReport {
    /// QSOs read out of the document.
    pub read: usize,
    /// Stations the document had something to say about.
    pub stations: usize,
    /// Fields actually written, which is fewer when the operator had already
    /// filed one by hand.
    pub fields: usize,
    /// What was passed over, and why.
    pub skipped: Vec<String>,
}

/// Reads every QSO an ADIF document holds.
///
/// The document arrives as bytes and is named an encoding rather than being
/// assumed to be UTF-8: Turbo HAMLOG and the loggers of its generation write
/// Shift_JIS, and reading one of those as UTF-8 fails on exactly the field —
/// the name, the QTH — that the import was wanted for.
pub fn read_adif(source: &[u8], encoding: &str) -> Result<Adif, QsoError> {
    let encoding = Encoding::for_label(encoding.as_bytes()).ok_or_else(|| QsoError::Encoding(encoding.to_owned()))?;
    Ok(read_bytes(source, encoding))
}

/// Reads an ADIF document into `store`, at [`Origin::Adif`].
///
/// A station worked many times is filed once, and each of its fields comes from
/// the most recent QSO that carried one. A later contact that left the QTH
/// blank is not a retraction of the QTH an earlier one recorded: it says
/// nothing about the QTH at all, and taking the whole of the last QSO would let
/// one sparse contest entry erase everything a rag-chew years earlier
/// established.
pub fn import(store: &mut Store, source: &[u8], encoding: &str) -> Result<ImportReport, QsoError> {
    let adif = read_adif(source, encoding)?;
    let read = adif.records.len();
    let records = fold(adif.records);
    let stations = records.len();
    let fields = store.merge_all(&records, Origin::Adif)?;
    Ok(ImportReport {
        read,
        stations,
        fields,
        skipped: adif.skipped,
    })
}

/// Reduces many QSOs with one station to the one record they add up to.
fn fold(records: Vec<AdifRecord>) -> Vec<Record> {
    let mut latest: BTreeMap<String, BTreeMap<String, (String, Option<Timestamp>)>> = BTreeMap::new();
    for entry in records {
        let station = latest.entry(entry.record.callsign().to_owned()).or_default();
        for (key, value) in entry.record.iter() {
            // `None` sorts before every instant, so a QSO with no date it can
            // be placed by never outranks one that has one, and two undated
            // QSOs fall back to the order the document listed them in.
            let outranked = station.get(key).is_none_or(|(_, at)| entry.worked_at >= *at);
            if outranked {
                station.insert(key.to_owned(), (value.to_owned(), entry.worked_at));
            }
        }
    }

    latest
        .into_iter()
        .filter_map(|(callsign, fields)| {
            let mut record = Record::new(&callsign)?;
            for (key, (value, _)) in fields {
                record.set(&key, &value);
            }
            Some(record)
        })
        .collect()
}

/// Walks the document as bytes, decoding one value at a time.
///
/// A tag's length counts bytes of the document as it was written, so the
/// document cannot be decoded first and then walked: a Shift_JIS `<QTH:6>東京都`
/// says six, and six characters of the decoded text would be the QTH and the
/// three characters after it. The tags themselves are ASCII in every encoding
/// this reads, so only the values need the decoder.
fn read_bytes(source: &[u8], encoding: &'static Encoding) -> Adif {
    let mut adif = Adif::default();
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut rest = source;

    while let Some(open) = rest.iter().position(|byte| *byte == b'<') {
        rest = &rest[open + 1..];
        let Some(close) = rest.iter().position(|byte| *byte == b'>') else {
            adif.skipped
                .push("a tag was left unclosed at the end of the document".to_owned());
            break;
        };
        let specifier = String::from_utf8_lossy(&rest[..close]);
        rest = &rest[close + 1..];

        let mut parts = specifier.splitn(3, ':');
        let name = parts.next().unwrap_or_default().trim().to_ascii_uppercase();
        let length = parts.next().and_then(|length| length.trim().parse::<usize>().ok());

        let Some(length) = length else {
            match name.as_str() {
                // Everything before the header's end describes the document
                // rather than a contact, so whatever was collected is dropped.
                "EOH" => fields.clear(),
                "EOR" => take_record(&mut adif, &mut fields),
                _ => {}
            }
            continue;
        };

        // The length is honoured rather than the value being scanned to the
        // next `<`, because a COMMENT may legally hold one.
        if rest.len() < length {
            adif.skipped
                .push(format!("`{name}` claimed {length} bytes the document does not hold"));
            break;
        }
        let (value, _, _) = encoding.decode(&rest[..length]);
        rest = &rest[length..];
        fields.insert(name, value.trim().to_owned());
    }

    // A document whose last record was never closed still described a contact,
    // and dropping it would silently lose the newest QSO in the log.
    take_record(&mut adif, &mut fields);
    adif
}

fn take_record(adif: &mut Adif, fields: &mut BTreeMap<String, String>) {
    let collected = std::mem::take(fields);
    if collected.is_empty() {
        return;
    }
    let Some(call) = collected.get("CALL") else {
        adif.skipped
            .push("a QSO named no station and was passed over".to_owned());
        return;
    };
    let Some(mut record) = Record::new(call) else {
        adif.skipped
            .push(format!("`{call}` is not a callsign and was passed over"));
        return;
    };

    for (from, to) in FIELDS {
        if let Some(value) = collected.get(from) {
            record.set(to, value);
        }
    }
    if record.get("note").is_none()
        && let Some(value) = collected.get(NOTE_FALLBACK)
    {
        record.set("note", value);
    }

    adif.records.push(AdifRecord {
        worked_at: worked_at(&collected),
        record,
    });
}

/// When the QSO happened, in UTC, which is the only zone ADIF writes.
fn worked_at(fields: &BTreeMap<String, String>) -> Option<Timestamp> {
    let date = fields.get("QSO_DATE")?;
    if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let time = fields.get("TIME_ON").map_or("000000", String::as_str);
    let time = match time.len() {
        4 if time.bytes().all(|byte| byte.is_ascii_digit()) => format!("{time}00"),
        6 if time.bytes().all(|byte| byte.is_ascii_digit()) => time.to_owned(),
        _ => "000000".to_owned(),
    };
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &date[0..4],
        &date[4..6],
        &date[6..8],
        &time[0..2],
        &time[2..4],
        &time[4..6]
    )
    .parse()
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn read(document: &str) -> Adif {
        read_adif(document.as_bytes(), "utf-8").expect("utf-8 is an encoding")
    }

    fn only(document: &str) -> Record {
        let adif = read(document);
        assert_eq!(adif.records.len(), 1, "{:?}", adif.skipped);
        adif.records.into_iter().next().expect("one record").record
    }

    #[test]
    fn a_header_is_dropped_rather_than_read_as_a_contact() {
        let adif = read("Exported by something\n<PROGRAMID:7>Testing<EOH>\n<CALL:6>JA1ABC<NAME:4>Taro<EOR>\n");

        assert_eq!(adif.records.len(), 1);
        assert_eq!(adif.records[0].record.callsign(), "JA1ABC");
        assert_eq!(adif.records[0].record.get("name"), Some("Taro"));
    }

    #[test]
    fn a_value_holding_a_tag_opener_is_taken_whole() {
        let record = only("<CALL:6>JA1ABC<COMMENT:11>a < in here<EOR>");

        assert_eq!(record.get("note"), Some("a < in here"));
    }

    #[test]
    fn a_tag_name_is_read_whatever_its_case() {
        let record = only("<call:6>JA1ABC<Name:4>Taro<eor>");

        assert_eq!(record.callsign(), "JA1ABC");
        assert_eq!(record.get("name"), Some("Taro"));
    }

    #[test]
    fn a_tag_carrying_a_type_is_read_like_any_other() {
        let record = only("<CALL:6:S>JA1ABC<NAME:4:S>Taro<EOR>");

        assert_eq!(record.get("name"), Some("Taro"));
    }

    /// A log written by a program that was still running has no final `<EOR>`,
    /// and the QSO it was in the middle of is the most recent one there is.
    #[test]
    fn a_record_the_document_never_closed_is_still_read() {
        let record = only("<CALL:6>JA1ABC<NAME:4>Taro");

        assert_eq!(record.get("name"), Some("Taro"));
    }

    #[test]
    fn a_length_the_document_cannot_honour_is_reported_rather_than_guessed_at() {
        let adif = read("<CALL:6>JA1ABC<EOR><CALL:40>JH1XYZ");

        assert_eq!(adif.records.len(), 1);
        assert_eq!(adif.skipped.len(), 1, "{:?}", adif.skipped);
    }

    #[test]
    fn a_qso_naming_no_station_is_passed_over() {
        let adif = read("<NAME:4>Taro<EOR><CALL:6>JA1ABC<EOR>");

        assert_eq!(adif.records.len(), 1);
        assert_eq!(adif.skipped.len(), 1, "{:?}", adif.skipped);
    }

    #[test]
    fn notes_stand_in_for_a_comment_the_qso_did_not_carry() {
        assert_eq!(only("<CALL:6>JA1ABC<NOTES:5>first<EOR>").get("note"), Some("first"));
        assert_eq!(
            only("<CALL:6>JA1ABC<COMMENT:6>chosen<NOTES:5>first<EOR>").get("note"),
            Some("chosen")
        );
    }

    /// The whole point of naming an encoding: a Shift_JIS length counts the
    /// bytes the logger wrote, not the characters they decode to.
    #[test]
    fn a_shift_jis_log_is_read_in_its_own_encoding() {
        let mut document = b"<CALL:6>JA1ABC<QTH:6>".to_vec();
        document.extend_from_slice(&[0x93, 0x8C, 0x8B, 0x9E, 0x93, 0x73]);
        document.extend_from_slice(b"<NAME:4>Taro<EOR>");

        let adif = read_adif(&document, "shift_jis").expect("shift_jis is an encoding");

        assert_eq!(adif.records.len(), 1, "{:?}", adif.skipped);
        assert_eq!(adif.records[0].record.get("qth"), Some("東京都"));
        assert_eq!(adif.records[0].record.get("name"), Some("Taro"));
    }

    #[test]
    fn an_encoding_this_build_does_not_know_is_refused() {
        let error = read_adif(b"", "klingon").expect_err("a refusal");
        assert!(matches!(error, QsoError::Encoding(_)), "{error}");
    }

    #[rstest]
    #[case("20240115", Some("0000"))]
    #[case("20240115", Some("1230"))]
    #[case("20240115", Some("123045"))]
    #[case("20240115", None)]
    fn a_qso_the_document_dated_can_be_placed(#[case] date: &str, #[case] time: Option<&str>) {
        let time = time.map_or(String::new(), |time| format!("<TIME_ON:{}>{time}", time.len()));
        let document = format!("<CALL:6>JA1ABC<QSO_DATE:8>{date}{time}<EOR>");
        let adif = read(&document);

        assert!(adif.records[0].worked_at.is_some());
    }

    #[rstest]
    #[case("<QSO_DATE:6>202401")]
    #[case("<QSO_DATE:8>notadate")]
    #[case("")]
    fn a_qso_the_document_did_not_date_is_left_unplaced(#[case] dating: &str) {
        let adif = read(&format!("<CALL:6>JA1ABC{dating}<EOR>"));

        assert_eq!(adif.records[0].worked_at, None);
    }

    /// Each field comes from the newest QSO that carried one, so a later
    /// contact that left the QTH blank does not erase what an earlier one said.
    #[test]
    fn a_field_is_taken_from_the_newest_qso_that_carried_it() {
        let mut store = Store::in_memory().expect("a store");
        let document = concat!(
            "<CALL:6>JA1ABC<QSO_DATE:8>20200101<NAME:4>Taro<QTH:5>Chiba<EOR>",
            "<CALL:6>JA1ABC<QSO_DATE:8>20220101<QTH:5>Tokyo<EOR>",
            "<CALL:6>JA1ABC<QSO_DATE:8>20240101<GRIDSQUARE:6>PM95TQ<EOR>",
        );

        let report = import(&mut store, document.as_bytes(), "utf-8").expect("an import");
        assert_eq!(report.read, 3);
        assert_eq!(report.stations, 1);

        let record = store.get("JA1ABC").expect("a read").expect("it was filed");
        assert_eq!(record.get("name"), Some("Taro"));
        assert_eq!(record.get("qth"), Some("Tokyo"));
        assert_eq!(record.get("grid"), Some("PM95TQ"));
    }

    #[test]
    fn a_dated_qso_outranks_an_undated_one_whatever_their_order() {
        let mut store = Store::in_memory().expect("a store");
        let document = concat!(
            "<CALL:6>JA1ABC<QSO_DATE:8>20200101<QTH:5>Tokyo<EOR>",
            "<CALL:6>JA1ABC<QTH:5>Chiba<EOR>",
        );

        import(&mut store, document.as_bytes(), "utf-8").expect("an import");

        let record = store.get("JA1ABC").expect("a read").expect("it was filed");
        assert_eq!(record.get("qth"), Some("Tokyo"));
    }

    #[test]
    fn an_import_does_not_displace_what_the_operator_typed() {
        let mut store = Store::in_memory().expect("a store");
        let mut typed = Record::new("JA1ABC").expect("that is a callsign");
        typed.set("name", "Taro");
        store.merge(&typed, Origin::Manual).expect("a write");

        let report = import(&mut store, b"<CALL:6>JA1ABC<NAME:6>TARO Y<QTH:5>Tokyo<EOR>", "utf-8").expect("an import");

        assert_eq!(report.fields, 1);
        let record = store.get("JA1ABC").expect("a read").expect("it was filed");
        assert_eq!(record.get("name"), Some("Taro"));
        assert_eq!(record.get("qth"), Some("Tokyo"));
    }

    /// Nothing about the contact itself is filed, whatever the log carried.
    #[test]
    fn what_a_contact_was_is_read_past_rather_than_kept() {
        let record = only(concat!(
            "<CALL:6>JA1ABC<BAND:3>20m<MODE:4>SSTV<FREQ:6>14.230",
            "<RST_SENT:3>595<RST_RCVD:3>595<QSL_RCVD:1>Y",
            "<STATION_CALLSIGN:6>JH1XYZ<MY_GRIDSQUARE:6>PM95TQ<EOR>",
        ));

        assert!(record.is_empty(), "{record:?}");
    }
}
