//! The station's own details, and the messages written from them.
//!
//! MMTTY's `ConvString` is the reference for what a macro can say, but not for
//! how it says it: percent letters are replaced by the `${...}` names the
//! family's other application already interpolates its templates with, so an
//! operator who has written one has written both. The braces cost nothing on
//! the air, being outside ITA2 and so not taken away from what can be sent.
//! The dollar sign is not — it is FIGS-D in the Bell table — but a name is
//! never written without its braces, so nothing can be confused for one.

use std::collections::BTreeMap;

use jiff::Zoned;

/// What stands in for a name that was never asked for.
///
/// The original's own fallback: a message that greets somebody by name should
/// still read as a greeting when the name is not known yet.
pub const UNKNOWN_NAME: &str = "OM";

/// The report a contact starts out being given.
pub const DEFAULT_RST: &str = "599";

/// What a macro is written from: the station, the contact, and the clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Station {
    pub callsign: String,
    pub name: String,
    pub qth: String,
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
            text: "\n${contact.callsign} DE ${station.callsign} ${greeting} ${contact.name}, UR RST ${contact.rst.sent} ${contact.rst.sent} QTH ${station.qth} BTU ${contact.callsign} DE ${station.callsign} K\n".to_owned(),
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
/// A name that is not one of these is left exactly as it was written. That is
/// deliberate rather than an oversight: the expansion lands in a field that
/// refuses the braces, so a misspelled name arrives in red with the send
/// button held, which is where a mistake in a macro should stop. For the same
/// reason there is nothing to escape a literal `${` into: it could not have
/// been sent either way.
pub fn expand(template: &str, station: &Station, contact: &Contact, now: &Zoned) -> String {
    let values = values(station, contact, now);
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match values.get(name) {
            Some(value) => out.push_str(value),
            None => out.push_str(&rest[start..start + 3 + end]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Every name a macro may use, and what it stands for right now.
///
/// Ordered so that listing them for the operator lists them the same way
/// every time.
pub fn values(station: &Station, contact: &Contact, now: &Zoned) -> BTreeMap<&'static str, String> {
    let utc = now.with_time_zone(jiff::tz::TimeZone::UTC);
    BTreeMap::from([
        ("station.callsign", station.callsign.clone()),
        ("station.name", station.name.clone()),
        ("station.qth", station.qth.clone()),
        ("contact.callsign", contact.callsign.clone()),
        ("contact.name", or_else(&contact.name, UNKNOWN_NAME)),
        ("contact.qth", contact.qth.clone()),
        ("contact.rst.sent", or_else(&contact.rst_sent, DEFAULT_RST)),
        ("contact.rst.received", contact.rst_received.clone()),
        ("date.utc", utc.strftime("%Y-%m-%d").to_string()),
        ("time.utc", utc.strftime("%H%M").to_string()),
        ("greeting", greeting(now).to_owned()),
    ])
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

    #[test]
    fn a_macro_is_written_from_the_station_and_the_contact() {
        let expanded = expand(
            "${contact.callsign} DE ${station.callsign} UR ${contact.rst.sent}",
            &station(),
            &contact(),
            &at(9),
        );
        assert_eq!(expanded, "JA1ZZZ DE JL1HIS UR 579");
    }

    /// A contact whose name was never asked for is still greeted.
    #[test]
    fn an_unknown_name_falls_back_to_the_customary_one() {
        let mut contact = contact();
        contact.name.clear();
        assert_eq!(expand("HI ${contact.name}", &station(), &contact, &at(9)), "HI OM");
    }

    #[test]
    fn a_report_that_was_cleared_falls_back_to_the_usual_one() {
        let mut contact = contact();
        contact.rst_sent.clear();
        assert_eq!(
            expand("RST ${contact.rst.sent}", &station(), &contact, &at(9)),
            "RST 599"
        );
    }

    /// An empty callsign expands to nothing rather than to a stand-in: a
    /// message addressed to a station not yet identified should read as
    /// unfinished, which is what the operator has to fix.
    #[test]
    fn an_unset_callsign_expands_to_nothing() {
        let contact = Contact::default();
        assert_eq!(expand("[${contact.callsign}]", &station(), &contact, &at(9)), "[]");
    }

    /// A misspelled name survives into the field, where the braces it carries
    /// are refused and the send button is held: the mistake stops where the
    /// operator can see it.
    #[test]
    fn an_unknown_name_is_left_where_it_was_written() {
        let expanded = expand("DE ${station.kallsign}", &station(), &contact(), &at(9));
        assert_eq!(expanded, "DE ${station.kallsign}");
        assert_eq!(crate::ui::input::first_unsendable(&expanded), Some('{'));
    }

    /// An opening with no closing brace is text rather than a name, and
    /// throwing away the rest of the message over it would lose the message.
    #[test]
    fn an_unclosed_name_is_left_as_it_stands() {
        assert_eq!(
            expand("DE ${station.callsign and more", &station(), &contact(), &at(9)),
            "DE ${station.callsign and more"
        );
    }

    #[test]
    fn text_with_no_names_in_it_is_unchanged() {
        assert_eq!(expand("RYRYRY", &station(), &contact(), &at(9)), "RYRYRY");
        assert_eq!(expand("", &station(), &contact(), &at(9)), "");
    }

    #[test]
    fn names_are_filled_in_wherever_they_appear() {
        let expanded = expand(
            "${station.callsign} ${station.callsign} ${station.callsign}",
            &station(),
            &contact(),
            &at(9),
        );
        assert_eq!(expanded, "JL1HIS JL1HIS JL1HIS");
    }

    #[rstest]
    #[case(0, "GM")]
    #[case(11, "GM")]
    #[case(12, "GA")]
    #[case(17, "GA")]
    #[case(18, "GE")]
    #[case(23, "GE")]
    fn the_greeting_follows_the_time_of_day(#[case] hour: i8, #[case] expected: &str) {
        assert_eq!(expand("${greeting}", &station(), &contact(), &at(hour)), expected);
    }

    /// The date and the time are the ones a contact is logged by, so they are
    /// universal rather than the operator's own.
    #[test]
    fn the_date_and_the_time_are_universal() {
        let now = date(2026, 9, 13)
            .at(1, 5, 0, 0)
            .in_tz("Asia/Tokyo")
            .expect("a valid instant");
        assert_eq!(expand("${date.utc}", &station(), &contact(), &now), "2026-09-12");
        assert_eq!(expand("${time.utc}", &station(), &contact(), &now), "1605");
    }

    /// Every macro that ships has to be sendable once it is filled in, or the
    /// first button a new operator presses is one that cannot be used.
    #[test]
    fn the_macros_that_ship_are_sendable_once_they_are_written() {
        for shipped in default_macros() {
            let expanded = expand(&shipped.text, &station(), &contact(), &at(9));
            assert_eq!(
                crate::ui::input::first_unsendable(&expanded),
                None,
                "{} expands to {expanded:?}",
                shipped.label
            );
        }
    }

    /// A station that has filled nothing in still gets text it could send,
    /// rather than a message with a name left in it.
    #[test]
    fn the_macros_that_ship_are_sendable_before_anything_is_filled_in() {
        for shipped in default_macros() {
            let expanded = expand(&shipped.text, &Station::default(), &Contact::default(), &at(9));
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
    #[test]
    fn every_name_in_the_table_is_a_name_that_expands() {
        for name in values(&station(), &contact(), &at(9)).into_keys() {
            let written = format!("${{{name}}}");
            assert_ne!(
                expand(&written, &station(), &contact(), &at(9)),
                written,
                "{name} is in the table but not filled in"
            );
        }
    }

    #[test]
    fn clearing_a_contact_leaves_the_report_this_station_gives_out() {
        let mut contact = contact();
        contact.clear();
        assert!(contact.is_empty());
        assert_eq!(contact.rst_sent, DEFAULT_RST);
    }
}
