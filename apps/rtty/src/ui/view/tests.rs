//! Duplicate widget ids and impossible layouts only surface at runtime, so
//! every locale is rendered here rather than only in front of an operator.

use egui_kittest::{Harness, kittest::Queryable};
use grayline_rtty::ToneSet;
use grayline_shell::i18n::{I18n, Locale};
use rstest::rstest;

use super::*;
use crate::{
    ui::menu,
    worker::receive::{ColumnSnapshot, RxSnapshot},
};

fn render(app: &mut App) -> Harness<'_> {
    let mut harness = Harness::new_ui(|ui| {
        let model = menu::model(app);
        view(ui, app, &model, menu::is_in_window());
    });
    harness.run();
    harness
}

/// Draws the window at a given size, so what a narrow one does can be checked.
fn render_sized(app: &mut App, size: egui::Vec2) -> Harness<'_> {
    let mut harness = Harness::builder().with_size(size).build_ui(|ui| {
        let model = menu::model(app);
        view(ui, app, &model, menu::is_in_window());
    });
    harness.run();
    harness
}

#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn the_window_draws_in_every_locale(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    let i18n = I18n::new(locale, &crate::locales::CATALOG);
    let harness = render(&mut app);
    harness.get_by_label(&i18n.text("action-clear"));
    harness.get_by_label(&i18n.text("action-resync"));
    harness.get_by_label(&i18n.text("label-mark"));
    harness.get_by_label(&i18n.text("action-send"));
    harness.get_by_label(&i18n.text("action-stop"));
    harness.get_by_label(&i18n.text("label-his-call"));
}

/// The pane says what it is waiting for while it is empty, because an empty
/// black rectangle says nothing about whether the application is working.
#[test]
fn an_empty_pane_says_what_it_is_waiting_for() {
    let mut app = App::headless();
    let hint = app.i18n.text("hint-listening");
    let harness = render(&mut app);
    harness.get_by_label(&hint);
}

#[test]
fn printed_text_is_drawn() {
    let mut app = App::headless();
    app.columns[0].push_str("CQ CQ DE JL1HIS\r\n");
    let harness = render(&mut app);
    harness.get_by_label("CQ CQ DE JL1HIS");
}

/// The header is what the operator tunes on, so every reading on it has to be
/// drawn from the snapshot rather than from what was asked for.
#[test]
fn the_header_reads_what_the_decode_path_is_hearing() {
    let mut app = App::headless();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let column = ColumnSnapshot {
        signal_strength: 0.5,
        case: Case::Figures,
        tones: ToneSet {
            mark_hz: 2_130.0,
            space_hz: 2_300.0,
        },
        squelch_open: true,
        ..ColumnSnapshot::default()
    };
    assert_eq!(tones_reading(&column), "M:2130 / S:2300");
    assert_eq!(case_text(&app, &column).text(), i18n.text("case-figures"));

    app.audio.seed_snapshot(RxSnapshot {
        columns: vec![column],
        ..RxSnapshot::default()
    });
    let harness = render(&mut app);
    harness.get_by_label(&i18n.text("path-resonator"));
    harness.get_by_label(&i18n.text("case-figures"));
    harness.get_by_label("M:2130 / S:2300");
}

/// Everything on the header line is read together, so nothing on it may sit
/// on a baseline of its own: two font families laid out at the same size do
/// not share one.
#[test]
fn the_header_readings_share_one_baseline() {
    let mut app = App::headless();
    app.audio.seed_snapshot(RxSnapshot {
        columns: vec![ColumnSnapshot {
            tones: ToneSet {
                mark_hz: 2_125.0,
                space_hz: 2_295.0,
            },
            ..ColumnSnapshot::default()
        }],
        ..RxSnapshot::default()
    });
    let letters = I18n::new(Locale::En, &crate::locales::CATALOG).text("case-letters");
    let harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));
    let reading = harness.get_by_label("M:2125 / S:2295").rect();
    let case = harness.get_by_label(&letters).rect();
    assert_eq!(reading.top(), case.top());
    assert_eq!(reading.height(), case.height());
}

/// The window is opened from the panel, which is where the operator is while
/// they are tuning.
#[test]
fn the_panel_opens_the_scope() {
    let mut app = App::headless();
    let label = app.i18n.text("action-scope");
    {
        let mut harness = render(&mut app);
        harness.get_by_label(&label).click();
        harness.run();
    }
    assert!(app.scope.is_open());
    assert!(app.audio.scope());
}

/// The panel is laid out from the width it is given, so a window too narrow
/// for it still has to draw every control.
#[test]
fn a_narrow_window_keeps_every_control() {
    let mut app = App::headless();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let harness = render_sized(&mut app, egui::vec2(420.0, 500.0));
    for key in [
        "label-mark",
        "label-shift",
        "label-speed",
        "action-afc",
        "action-clear",
        "action-send",
        "label-his-call",
    ] {
        harness.get_by_label(&i18n.text(key));
    }
}

/// The interface has to settle. A frame is drawn whenever the pointer moves,
/// so a layout still being measured steps about under the pointer.
#[test]
fn the_window_settles_and_stops_asking_for_frames() {
    let mut app = App::headless();
    let mut harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));
    // `run` gives up after a few steps if frames keep being asked for, which
    // is the assertion: by now nothing should be.
    harness.run();
}

/// The macro buttons are drawn from the configuration, so a station that has
/// edited it gets its own buttons rather than the ones that shipped.
#[test]
fn the_macro_buttons_are_the_ones_the_configuration_names() {
    let mut app = App::headless();
    app.macros = vec![
        crate::app::macros::Macro {
            label: "CALL".to_owned(),
            text: "CQ".to_owned(),
            send: false,
        },
        crate::app::macros::Macro {
            label: "SIGN".to_owned(),
            text: "SK".to_owned(),
            send: false,
        },
    ];
    let harness = render(&mut app);
    harness.get_by_label("CALL");
    harness.get_by_label("SIGN");
}

/// Putting the station on the air is done from the panel beside the text
/// rather than from under the message, so that a press cannot land among the
/// macros the operator is typing between.
#[test]
fn sending_is_done_from_the_panel_beside_the_text() {
    let mut app = App::headless();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);

    let harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));

    let panel = egui::PanelState::load(&harness.ctx, Id::new("side-panel"))
        .expect("the side panel is drawn")
        .outer_rect;
    for control in ["action-send", "action-stop", "label-level"] {
        let rect = harness.get_by_label(&i18n.text(control)).rect();
        assert!(panel.contains_rect(rect), "{control} is not in the side panel");
    }
}

/// The set messages are listed beside the buttons, in every locale, because
/// the list is drawn from a file the operator writes in their own language.
#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn the_set_messages_are_listed_beside_the_buttons(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    let i18n = I18n::new(locale, &crate::locales::CATALOG);

    let harness = render(&mut app);

    // A list reports what it is showing as its value rather than its label,
    // the way every other one in the window does.
    assert!(
        harness
            .query_all_by_value(&i18n.text("label-templates"))
            .next()
            .is_some(),
        "the set messages are not listed"
    );
}

/// The row is drawn for whichever of the two lists has something in it, so a
/// station that has emptied one still reaches the other.
#[test]
fn the_list_is_drawn_even_where_there_are_no_buttons() {
    let mut app = App::headless();
    app.macros.clear();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);

    let harness = render(&mut app);

    assert!(
        harness
            .query_all_by_value(&i18n.text("label-templates"))
            .next()
            .is_some(),
        "the set messages are not listed"
    );
}

/// A station that has emptied both files gets no row at all rather than an
/// empty one.
#[test]
fn a_station_with_neither_draws_no_row() {
    let mut app = App::headless();
    app.macros.clear();
    app.templates.clear();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);

    let harness = render(&mut app);

    assert!(
        harness
            .query_all_by_value(&i18n.text("label-templates"))
            .next()
            .is_none()
    );
    harness.get_by_label(&i18n.text("action-send"));
}

/// A station with no macros at all draws no button row rather than an empty
/// one, and everything else still lays out.
#[test]
fn a_station_with_no_macros_still_draws_the_panel() {
    let mut app = App::headless();
    app.macros.clear();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let harness = render(&mut app);
    harness.get_by_label(&i18n.text("action-send"));
}

/// The message being keyed is drawn above the field, with the queue behind it,
/// so a long exchange still lays out rather than pushing the field off.
#[test]
fn the_message_on_the_air_and_the_queue_are_both_drawn() {
    let mut app = App::headless();
    sending(&mut app);
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);

    let harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));

    harness.get_by_label(&i18n.text("label-on-air"));
    harness.get_by_label(&i18n.text("label-queued"));
    harness.get_by_label(&i18n.text("action-stop"));
}

/// The field is a fixed four rows: a long message scrolls inside it rather
/// than growing the panel, so the buttons under it never move while an
/// operator is typing between them.
#[test]
fn the_field_keeps_its_height_however_long_the_message_is() {
    let height = |draft: &str| {
        let mut app = App::headless();
        app.transmit.draft = draft.to_owned();
        let harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));
        let state = egui::PanelState::load(&harness.ctx, Id::new("transmit-panel"));
        state.expect("the transmit panel is drawn").size().y
    };
    assert_eq!(
        height("CQ"),
        height(
            &"RY RY DE JL1HIS
"
            .repeat(40)
        )
    );
}

/// What is on the air is a pane of its own above the field, so a message
/// arriving in the queue takes its height from the text rather than pushing
/// the field and its buttons down the window.
#[test]
fn the_stack_is_a_pane_of_its_own_and_leaves_the_field_where_it_was() {
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let mut idle = App::headless();
    let harness = render_sized(&mut idle, egui::vec2(1_100.0, 720.0));
    assert!(harness.query_by_label(&i18n.text("label-on-air")).is_none());
    assert!(egui::PanelState::load(&harness.ctx, Id::new("pending-panel")).is_none());
    let quiet = egui::PanelState::load(&harness.ctx, Id::new("transmit-panel"))
        .expect("the transmit panel is drawn")
        .size()
        .y;

    let mut busy = App::headless();
    sending(&mut busy);
    let harness = render_sized(&mut busy, egui::vec2(1_100.0, 720.0));
    assert!(egui::PanelState::load(&harness.ctx, Id::new("pending-panel")).is_some());
    let keyed = egui::PanelState::load(&harness.ctx, Id::new("transmit-panel"))
        .expect("the transmit panel is drawn")
        .size()
        .y;

    assert_eq!(quiet, keyed);
}

/// egui hands out widget identifiers by position, so a pane that comes and
/// goes renumbers everything claimed after it: the text pane would be built
/// as a different widget every time a message was queued, losing what it had
/// scrolled to and selected, and a debug build would draw a red frame around
/// each widget it caught changing identity.
#[test]
fn the_stack_coming_and_going_leaves_the_identifiers_under_it_alone() {
    use egui_kittest::kittest::NodeT as _;

    let printed = "CQ DE JA1ZZZ K";
    let identifier = |busy: bool| {
        let mut app = App::headless();
        app.columns[0].push_str(printed);
        if busy {
            sending(&mut app);
        }
        let harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));
        format!("{:?}", harness.get_by_label(printed).accesskit_node().id())
    };
    assert_eq!(identifier(false), identifier(true));
}

/// The same on the status bar, where a fault turning up on the left must
/// leave the reading on the right — which has not moved — as the widget it
/// already was.
#[test]
fn a_fault_leaves_the_reading_beside_it_alone() {
    use egui_kittest::kittest::NodeT as _;

    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let reading = i18n.text("status-no-audio");
    let identifier = |noticed: bool| {
        let mut app = App::headless();
        if noticed {
            app.notice = Some("A NOTICE".to_owned());
        }
        let harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));
        format!("{:?}", harness.get_by_label(&reading).accesskit_node().id())
    };
    assert_eq!(identifier(false), identifier(true));
}

/// The stack is read down its left edge, so what is waiting starts where what
/// is on the air starts however each row is introduced.
///
/// Both locales, because the column the labels sit in is a fixed width and
/// the labels are not: a translation that outgrew it would push its own
/// messages out of line.
#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn the_stack_lines_its_messages_up_on_one_left_edge(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    sending(&mut app);
    let harness = render_sized(&mut app, egui::vec2(1_100.0, 720.0));

    let air = harness.get_by_label("CQ CQ DE JL1HIS").rect().left();
    let waiting = harness.get_by_label("SECOND MESSAGE").rect().left();
    assert!((air - waiting).abs() < 1.0, "{air} against {waiting}");
}

/// Puts a message on the air with another waiting behind it.
fn sending(app: &mut App) {
    let text = "CQ CQ DE JL1HIS";
    let schedule = grayline_rtty::TxSchedule::new(text, 48_000, &app.tx_config()).unwrap();
    app.transmit
        .begin(crate::app::transmit::Sending::new(text.to_owned(), schedule));
    app.transmit.queue("SECOND MESSAGE".to_owned());
}

/// What a station sent is printed with what it received, so an exchange reads
/// back as one transcript.
#[test]
fn sent_text_is_printed_with_the_received_text() {
    let mut app = App::headless();
    app.columns[0].push_str("CQ DE JA1ZZZ K\r\n");
    app.columns[0].push_sent("JA1ZZZ DE JL1HIS");
    let harness = render(&mut app);
    harness.get_by_label("CQ DE JA1ZZZ K\nJA1ZZZ DE JL1HIS");
}

/// This station's own details are set once and then left alone, so they are
/// behind the Settings menu rather than beside the text that is worked.
#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn the_station_window_opens_from_the_menu(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    let i18n = I18n::new(locale, &crate::locales::CATALOG);

    // Closed, the fields are nowhere in the window.
    {
        let harness = render(&mut app);
        assert!(harness.query_by_label(&i18n.text("label-my-call")).is_none());
    }

    menu::apply(&mut app, menu::Action::ShowStation);
    let harness = render(&mut app);

    harness.get_by_label(&i18n.text("station-title"));
    harness.get_by_label(&i18n.text("label-my-call"));
    harness.get_by_label(&i18n.text("label-my-qth"));
    harness.get_by_label(&i18n.text("station-close"));
}

/// The Settings menu has to carry it, or the window has no way of being
/// opened at all.
#[test]
fn the_settings_menu_names_the_station_window() {
    let app = App::headless();
    let model = menu::model(&app);
    let items = menu::flatten(&model);
    assert!(
        items.iter().any(|item| matches!(
            item,
            menu::Item::Command {
                action: menu::Action::ShowStation,
                ..
            }
        )),
        "the station window is not on any menu"
    );
}

/// The operator's own fields are edited in the same window their station is,
/// which is what the Settings menu opens.
#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn the_extra_fields_are_edited_in_the_station_window(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    app.custom_variables = std::collections::BTreeMap::from([("grid".to_owned(), "PM95UQ".to_owned())]);
    let i18n = I18n::new(locale, &crate::locales::CATALOG);

    menu::apply(&mut app, menu::Action::ShowStation);
    let harness = render(&mut app);

    harness.get_by_label(&i18n.text("custom-title"));
    harness.get_by_label(&i18n.text("custom-add"));
    // Both halves of the stored row are there to be edited. A text field
    // reports its value on more than one node, so the query is by count.
    for value in ["grid", "PM95UQ"] {
        assert!(
            harness.query_all_by_value(value).next().is_some(),
            "{value} is not in the window"
        );
    }
}

/// The window on what is filed opens from the panel rather than from a menu,
/// because it is about the station on the air right now.
#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn the_contact_window_shows_the_fields_the_settings_ask_for(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    let i18n = I18n::new(locale, &crate::locales::CATALOG);

    // Closed, none of it is in the window. Probed by the note rather than by
    // the title, which is the word the panel's own section heading uses.
    {
        let harness = render(&mut app);
        assert!(harness.query_by_label(&i18n.text("contact-note-keys")).is_none());
    }

    app.contact.callsign = "JA1ABC".to_owned();
    app.open_contact();
    let harness = render(&mut app);

    harness.get_by_label(&i18n.text("contact-name"));
    harness.get_by_label(&i18n.text("contact-name-latin"));
    harness.get_by_label(&i18n.text("contact-qth"));
    harness.get_by_label(&i18n.text("contact-add"));
    harness.get_by_label(&i18n.text("station-close"));
}

/// A key the settings named that this build has no label for stands under its
/// own name: the field list is the operator's to write, and a key they
/// invented is one no catalogue was ever going to know.
#[test]
fn a_field_no_catalogue_knows_stands_under_its_own_name() {
    let mut app = App::headless();
    app.contact_settings.fields = vec!["club".to_owned()];
    app.contact.callsign = "JA1ABC".to_owned();
    app.open_contact();

    let harness = render(&mut app);

    harness.get_by_label("club");
}

/// The Settings menu carries the directory's own two settings, or an operator
/// has no way to turn the lookup off or to learn that the key file exists.
#[test]
fn the_settings_menu_carries_the_directory() {
    let app = App::headless();
    let model = menu::model(&app);
    let items = menu::flatten(&model);
    assert!(
        items.iter().any(|item| matches!(
            item,
            menu::Item::Check {
                action: menu::Action::ToggleContactLookup,
                ..
            }
        )),
        "the lookup switch is not on the menu"
    );
    assert!(
        items.iter().any(|item| matches!(
            item,
            menu::Item::Command {
                action: menu::Action::WriteContactCredentials,
                ..
            }
        )),
        "writing the credentials file is not on the menu"
    );
}
