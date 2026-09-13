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
