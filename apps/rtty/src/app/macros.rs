//! The station's own details, and the messages written from them.
//!
//! MMTTY's `ConvString` is the reference for what a macro can say, but not for
//! how it says it: percent letters are replaced by the `${...}` expressions
//! `grayline-variables` reads for the whole family, so an operator who has
//! written an SSTV template has written a macro. The braces cost nothing on
//! the air, being outside ITA2 and so not taken away from what can be sent.
//! The dollar sign is not — it is FIGS-D in the Bell table — but a name is
//! never written without its braces, so nothing can be confused for one.
//!
//! What this module holds is the half that is RTTY's: which names exist, and
//! what each one stands for right now.

use std::collections::BTreeMap;

use grayline_variables::{VariableError, VariableValue, Variables, interpolate};
use jiff::{Zoned, tz::TimeZone};

pub use grayline_variables::valid_variable_name;

/// The prefix the operator's own names are reached through.
///
/// Kept apart from the built-in names so that adding one here later cannot
/// take a name an operator was already using out from under their macros.
pub const CUSTOM_PREFIX: &str = "custom";

/// What stands in for a name that was never asked for.
///
/// The original's own fallback, and not the sort of placeholder the SSTV
/// templates refuse: `OM` is what an operator actually says to a station whose
/// name they have not been told, so it is a real value rather than the word
/// `Name` standing in for one.
pub const UNKNOWN_NAME: &str = "OM";

/// The report a contact starts out being given.
pub const DEFAULT_RST: &str = "599";

/// Who this station is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Station {
    pub callsign: String,
    pub name: String,
    pub qth: String,
    pub grid: String,
}

/// The station being worked, as entered beside the received text.
#[derive(Clone, Debug, PartialEq)]
pub struct Contact {
    pub callsign: String,
    pub name: String,
    pub qth: String,
    /// The report being sent to them.
    pub rst_sent: String,
    /// The report they sent.
    pub rst_received: String,
}

impl Default for Contact {
    fn default() -> Self {
        Self {
            callsign: String::new(),
            name: String::new(),
            qth: String::new(),
            rst_sent: DEFAULT_RST.to_owned(),
            rst_received: String::new(),
        }
    }
}

impl Contact {
    /// Empties everything the next station would answer differently.
    ///
    /// The report being sent goes back to its default rather than to nothing,
    /// because it is what this station gives out rather than something the
    /// contact told it.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_empty(&self) -> bool {
        self.callsign.is_empty() && self.name.is_empty() && self.qth.is_empty() && self.rst_received.is_empty()
    }
}

/// Everything a macro is written from, at the moment it is written.
#[derive(Clone, Copy, Debug)]
pub struct MacroContext<'a> {
    pub station: &'a Station,
    pub contact: &'a Contact,
    /// The operator's own fields, reached as `${custom.<name>}`.
    pub custom: &'a BTreeMap<String, String>,
    pub now: &'a Zoned,
}

/// One button, and the message behind it.
#[derive(Clone, Debug, PartialEq)]
pub struct Macro {
    /// What the button says.
    pub label: String,
    /// The message, before its names are filled in.
    pub text: String,
    /// Whether pressing it sends rather than writing into the field.
    pub send: bool,
}

/// The macros an operator starts with.
///
/// A first-time station can call CQ, answer, and sign off without writing a
/// file first; everything after that is edited in the configuration, which the
/// File menu opens.
pub fn default_macros() -> Vec<Macro> {
    vec![
        Macro {
            label: "CQ".to_owned(),
            text: "\nCQ CQ CQ DE ${station.callsign} ${station.callsign} ${station.callsign} PSE K\n".to_owned(),
            send: true,
        },
        Macro {
            label: "ANS".to_owned(),
            text: "\n${contact.callsign} DE ${station.callsign} ${station.callsign} K\n".to_owned(),
            send: false,
        },
        Macro {
            label: "RST".to_owned(),
            text: "\n${contact.callsign} DE ${station.callsign} ${greeting} ${contact.name}, UR RST ${report.sent} ${report.sent} QTH ${station.qth} BTU ${contact.callsign} DE ${station.callsign} K\n".to_owned(),
            send: false,
        },
        Macro {
            label: "73".to_owned(),
            text: "\n${contact.callsign} DE ${station.callsign} TNX FER QSO ${contact.name}, 73 ES GL. ${contact.callsign} DE ${station.callsign} SK\n".to_owned(),
            send: false,
        },
        Macro {
            label: "RY".to_owned(),
            text: "\nRYRYRYRYRYRYRYRYRYRYRYRYRYRY DE ${station.callsign}\n".to_owned(),
            send: true,
        },
    ]
}

/// Fills a macro's names in from the station, the contact, and the clock.
///
/// A name nothing was provided for is a failure rather than a gap, which is
/// what the SSTV templates do with one and for the same reason: a message that
/// silently lost the callsign it was addressed to is worse than one that never
/// got written.
pub fn expand(template: &str, context: &MacroContext<'_>) -> Result<String, VariableError> {
    interpolate(template, &values(context))
}

/// Every name a macro may use, and what it stands for right now.
///
/// The names are the SSTV templates', so a macro and a template say the same
/// thing the same way. Missing from them are the ones this application has
/// nothing to answer with: `radio.*` wants the rig control that is not written
/// yet, `rx.timestamp.*` wants a reception with a beginning and an end, and
/// `report.number` wants the contest serial the plan deferred. Added to them
/// is `greeting`, which is MMTTY's and which no template ever wanted.
///
/// The operator's own fields are folded in under their prefix, so one of them
/// can never quietly shadow a built-in name.
pub fn values(context: &MacroContext<'_>) -> Variables {
    let MacroContext {
        station,
        contact,
        custom,
        now,
    } = context;
    let mut values = Variables::new();
    for (name, value) in [
        ("station.callsign", &station.callsign),
        ("station.name", &station.name),
        ("station.qth", &station.qth),
        ("station.grid", &station.grid),
        ("contact.callsign", &contact.callsign),
        ("contact.qth", &contact.qth),
        ("report.received", &contact.rst_received),
    ] {
        values.insert(name, VariableValue::Text((*value).clone()));
    }
    values.insert(
        "contact.name",
        VariableValue::Text(or_else(&contact.name, UNKNOWN_NAME)),
    );
    values.insert(
        "report.sent",
        VariableValue::Text(or_else(&contact.rst_sent, DEFAULT_RST)),
    );
    values.insert(
        "application.version",
        VariableValue::Text(env!("CARGO_PKG_VERSION").to_owned()),
    );
    values.insert("greeting", VariableValue::Text(greeting(now).to_owned()));
    // Offered in both the zone the operator reads and the one the log is kept
    // in, so a macro chooses rather than converts. They are timestamps rather
    // than preformatted text, so `${tx.timestamp.utc:%H%M}` says the time and
    // `${tx.timestamp.utc:%Y-%m-%d}` the date out of the one name.
    values.insert("tx.timestamp.local", VariableValue::Timestamp((*now).clone()));
    values.insert(
        "tx.timestamp.utc",
        VariableValue::Timestamp(now.with_time_zone(TimeZone::UTC)),
    );
    for (name, value) in custom.iter() {
        values.insert(format!("{CUSTOM_PREFIX}.{name}"), VariableValue::Text(value.clone()));
    }
    values
}

fn or_else(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

/// The greeting for the time of day, as the original abbreviates them.
///
/// Taken from this station's own clock rather than from anything about the
/// contact: which part of their day a station is in is not something a
/// transmission carries, and the original reads its own clock too.
fn greeting(now: &Zoned) -> &'static str {
    match now.hour() {
        0..=11 => "GM",
        12..=17 => "GA",
        _ => "GE",
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use rstest::rstest;

    use super::*;

    fn station() -> Station {
        Station {
            callsign: "JL1HIS".to_owned(),
            name: "YU".to_owned(),
            qth: "TOKYO".to_owned(),
            grid: "PM95UQ".to_owned(),
        }
    }

    fn contact() -> Contact {
        Contact {
            callsign: "JA1ZZZ".to_owned(),
            name: "TARO".to_owned(),
            qth: "OSAKA".to_owned(),
            rst_sent: "579".to_owned(),
            rst_received: "599".to_owned(),
        }
    }

    fn at(hour: i8) -> Zoned {
        date(2026, 9, 13)
            .at(hour, 34, 0, 0)
            .in_tz("UTC")
            .expect("a valid instant")
    }

    fn no_custom() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn context<'a>(
        station: &'a Station,
        contact: &'a Contact,
        custom: &'a BTreeMap<String, String>,
        now: &'a Zoned,
    ) -> MacroContext<'a> {
        MacroContext {
            station,
            contact,
            custom,
            now,
        }
    }

    /// Writes `template` against the usual station and contact.
    fn written(template: &str) -> String {
        expand(template, &context(&station(), &contact(), &no_custom(), &at(9))).expect("the names are all provided")
    }

    #[test]
    fn a_macro_is_written_from_the_station_and_the_contact() {
        assert_eq!(
            written("${contact.callsign} DE ${station.callsign} UR ${report.sent}"),
            "JA1ZZZ DE JL1HIS UR 579"
        );
    }

    /// The names are the SSTV templates', so one message says the same thing
    /// in both places.
    #[rstest]
    #[case("${station.callsign}", "JL1HIS")]
    #[case("${station.qth}", "TOKYO")]
    #[case("${station.grid}", "PM95UQ")]
    #[case("${contact.callsign}", "JA1ZZZ")]
    #[case("${contact.name}", "TARO")]
    #[case("${contact.qth}", "OSAKA")]
    #[case("${report.sent}", "579")]
    #[case("${report.received}", "599")]
    fn the_names_are_the_ones_a_template_uses(#[case] written_as: &str, #[case] expected: &str) {
        assert_eq!(written(written_as), expected);
    }

    /// A contact whose name was never asked for is still greeted.
    #[test]
    fn an_unknown_name_falls_back_to_the_customary_one() {
        let mut contact = contact();
        contact.name.clear();
        let expanded = expand(
            "HI ${contact.name}",
            &context(&station(), &contact, &no_custom(), &at(9)),
        )
        .unwrap();
        assert_eq!(expanded, "HI OM");
    }

    #[test]
    fn a_report_that_was_cleared_falls_back_to_the_usual_one() {
        let mut contact = contact();
        contact.rst_sent.clear();
        let expanded = expand(
            "RST ${report.sent}",
            &context(&station(), &contact, &no_custom(), &at(9)),
        )
        .unwrap();
        assert_eq!(expanded, "RST 599");
    }

    /// An empty callsign expands to nothing rather than to a stand-in: a
    /// message addressed to a station not yet identified should read as
    /// unfinished, which is what the operator has to fix.
    #[test]
    fn an_unset_callsign_expands_to_nothing() {
        let expanded = expand(
            "[${contact.callsign}]",
            &context(&station(), &Contact::default(), &no_custom(), &at(9)),
        )
        .unwrap();
        assert_eq!(expanded, "[]");
    }

    /// A misspelled name is refused rather than left in the message, which is
    /// what a template does with one.
    #[test]
    fn an_unknown_name_is_refused() {
        let error = expand(
            "DE ${station.kallsign}",
            &context(&station(), &contact(), &no_custom(), &at(9)),
        )
        .unwrap_err();
        assert_eq!(error, VariableError::Missing("station.kallsign".to_owned()));
    }

    #[test]
    fn an_unclosed_name_is_refused() {
        let error = expand(
            "DE ${station.callsign and more",
            &context(&station(), &contact(), &no_custom(), &at(9)),
        )
        .unwrap_err();
        assert_eq!(error, VariableError::Unterminated);
    }

    /// A dollar sign is doubled to say one, exactly as a template says one.
    #[test]
    fn a_doubled_dollar_is_one_dollar() {
        assert_eq!(written("$$5"), "$5");
        assert_eq!(written("$$${station.callsign}"), "$JL1HIS");
    }

    /// A dollar that opens nothing is a dollar, so a price in a message needs
    /// no thought from the operator.
    #[test]
    fn a_dollar_that_opens_nothing_is_left_alone() {
        assert_eq!(written("5 $ EACH"), "5 $ EACH");
    }

    #[test]
    fn text_with_no_names_in_it_is_unchanged() {
        assert_eq!(written("RYRYRY"), "RYRYRY");
        assert_eq!(written(""), "");
    }

    #[test]
    fn names_are_filled_in_wherever_they_appear() {
        assert_eq!(
            written("${station.callsign} ${station.callsign} ${station.callsign}"),
            "JL1HIS JL1HIS JL1HIS"
        );
    }

    #[rstest]
    #[case(0, "GM")]
    #[case(11, "GM")]
    #[case(12, "GA")]
    #[case(17, "GA")]
    #[case(18, "GE")]
    #[case(23, "GE")]
    fn the_greeting_follows_the_time_of_day(#[case] hour: i8, #[case] expected: &str) {
        let expanded = expand("${greeting}", &context(&station(), &contact(), &no_custom(), &at(hour))).unwrap();
        assert_eq!(expanded, expected);
    }

    /// The clock is one name written whichever way the macro asks for, rather
    /// than a name per format.
    #[test]
    fn the_clock_takes_the_format_the_macro_asks_for() {
        let now = date(2026, 9, 13)
            .at(1, 5, 0, 0)
            .in_tz("Asia/Tokyo")
            .expect("a valid instant");
        let station = station();
        let contact = contact();
        let custom = no_custom();
        let context = context(&station, &contact, &custom, &now);
        assert_eq!(expand("${tx.timestamp.utc:%Y-%m-%d}", &context).unwrap(), "2026-09-12");
        assert_eq!(expand("${tx.timestamp.utc:%H%M}", &context).unwrap(), "1605");
        assert_eq!(expand("${tx.timestamp.local:%H%M}", &context).unwrap(), "0105");
    }

    /// Written without a format, a timestamp says what a template says.
    #[test]
    fn a_clock_with_no_format_reads_as_it_does_in_a_template() {
        assert_eq!(written("${tx.timestamp.utc}"), "2026-09-13 09:34");
    }

    /// Only a timestamp takes a format, which is the template renderer's own
    /// rule and worth the same refusal here.
    #[test]
    fn text_refuses_a_format() {
        let error = expand(
            "${station.callsign:%H}",
            &context(&station(), &contact(), &no_custom(), &at(9)),
        )
        .unwrap_err();
        assert!(matches!(error, VariableError::Format { .. }), "{error:?}");
    }

    /// A field the operator invented is reached under the prefix, so their
    /// own names and the built-in ones cannot collide.
    #[test]
    fn an_operator_field_is_reached_through_the_prefix() {
        let custom = BTreeMap::from([("club".to_owned(), "JARL".to_owned())]);
        let expanded = expand(
            "GRID ${station.grid} CLUB ${custom.club}",
            &context(&station(), &contact(), &custom, &at(9)),
        )
        .unwrap();
        assert_eq!(expanded, "GRID PM95UQ CLUB JARL");
    }

    /// A field named after a built-in one is still reached under the prefix,
    /// so it cannot take that name out from under a macro already using it.
    #[test]
    fn an_operator_field_cannot_shadow_a_built_in_name() {
        let custom = BTreeMap::from([("station.callsign".to_owned(), "WRONG".to_owned())]);
        let expanded = expand(
            "${station.callsign} ${custom.station.callsign}",
            &context(&station(), &contact(), &custom, &at(9)),
        )
        .unwrap();
        assert_eq!(expanded, "JL1HIS WRONG");
    }

    #[test]
    fn a_field_that_was_never_named_is_refused() {
        let error = expand(
            "CLUB ${custom.club}",
            &context(&station(), &contact(), &no_custom(), &at(9)),
        )
        .unwrap_err();
        assert_eq!(error, VariableError::Missing("custom.club".to_owned()));
    }

    /// Every macro that ships has to be sendable once it is filled in, or the
    /// first button a new operator presses is one that cannot be used.
    #[test]
    fn the_macros_that_ship_are_sendable_once_they_are_written() {
        for shipped in default_macros() {
            let expanded = written(&shipped.text);
            assert_eq!(
                crate::ui::input::first_unsendable(&expanded),
                None,
                "{} expands to {expanded:?}",
                shipped.label
            );
        }
    }

    /// A station that has filled nothing in still gets text it could send,
    /// rather than a message that will not expand at all.
    #[test]
    fn the_macros_that_ship_are_sendable_before_anything_is_filled_in() {
        let empty = Station::default();
        let no_contact = Contact::default();
        for shipped in default_macros() {
            let expanded = expand(&shipped.text, &context(&empty, &no_contact, &no_custom(), &at(9)))
                .unwrap_or_else(|error| panic!("{} does not expand: {error}", shipped.label));
            assert_eq!(
                crate::ui::input::first_unsendable(&expanded),
                None,
                "{} expands to {expanded:?}",
                shipped.label
            );
        }
    }

    /// Every name the table carries has to be one a macro can actually use,
    /// or the documentation written from it would name one that does nothing.
    #[rstest]
    #[case("station.callsign")]
    #[case("station.name")]
    #[case("station.qth")]
    #[case("station.grid")]
    #[case("contact.callsign")]
    #[case("contact.name")]
    #[case("contact.qth")]
    #[case("report.sent")]
    #[case("report.received")]
    #[case("application.version")]
    #[case("greeting")]
    #[case("tx.timestamp.utc")]
    #[case("tx.timestamp.local")]
    fn every_offered_name_expands(#[case] name: &str) {
        let written_as = format!("${{{name}}}");
        let expanded = expand(&written_as, &context(&station(), &contact(), &no_custom(), &at(9)));
        assert!(expanded.is_ok(), "{name} is offered but does not expand");
    }

    #[rstest]
    #[case("grid", true)]
    #[case("my_grid", true)]
    #[case("_grid", true)]
    #[case("grid2", true)]
    #[case("club.name", true)]
    #[case("", false)]
    #[case("2grid", false)]
    #[case("my grid", false)]
    #[case("my-grid", false)]
    #[case("grid.", false)]
    #[case(".grid", false)]
    #[case("グリッド", false)]
    fn a_field_name_is_one_an_expression_could_hold(#[case] name: &str, #[case] expected: bool) {
        assert_eq!(valid_variable_name(name), expected);
    }

    #[test]
    fn clearing_a_contact_leaves_the_report_this_station_gives_out() {
        let mut contact = contact();
        contact.clear();
        assert!(contact.is_empty());
        assert_eq!(contact.rst_sent, DEFAULT_RST);
    }
}
