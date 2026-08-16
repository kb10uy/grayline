//! Duplicate widget ids and impossible layouts only surface at runtime, so
//! every locale is rendered here rather than only in front of an operator.

use egui_kittest::{Harness, kittest::Queryable};
use grayline_rtty::ToneSet;
use grayline_shell::i18n::{I18n, Locale};
use rstest::rstest;

use super::*;
use crate::{ui::menu, worker::receive::ColumnSnapshot};

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
        difference: -0.4,
        case: Case::Figures,
        tones: ToneSet {
            mark_hz: 2_130.0,
            space_hz: 2_300.0,
        },
        squelch_open: true,
        ..ColumnSnapshot::default()
    };
    assert_eq!(tones_reading(&column), "2130/2300");
    assert_eq!(tuning_text(&app, &column).text(), i18n.text("tone-space"));
    assert_eq!(case_text(&app, &column).text(), i18n.text("case-figures"));

    let harness = render(&mut app);
    harness.get_by_label(&i18n.text("path-resonator"));
}

/// A closed squelch is the receiver saying it is hearing nothing, and the
/// comparator's answer under noise is not a reading worth printing.
#[test]
fn a_closed_squelch_reads_as_neither_tone() {
    let app = App::headless();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let column = ColumnSnapshot {
        difference: 0.9,
        squelch_open: false,
        ..ColumnSnapshot::default()
    };
    assert_eq!(tuning_text(&app, &column).text(), i18n.text("tone-quiet"));
}

/// The panel is laid out from the width it is given, so a window too narrow
/// for it still has to draw every control.
#[test]
fn a_narrow_window_keeps_every_control() {
    let mut app = App::headless();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let harness = render_sized(&mut app, egui::vec2(420.0, 500.0));
    for key in ["label-mark", "label-shift", "label-speed", "action-afc", "action-clear"] {
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

/// Mark and space are read at a glance rather than by reading the word, so
/// they cannot share a colour.
#[test]
fn the_two_tones_are_drawn_apart() {
    let app = App::headless();
    let mark = ColumnSnapshot {
        difference: 0.5,
        squelch_open: true,
        ..ColumnSnapshot::default()
    };
    let space = ColumnSnapshot {
        difference: -0.5,
        squelch_open: true,
        ..ColumnSnapshot::default()
    };
    assert_ne!(tuning_text(&app, &mark).text(), tuning_text(&app, &space).text());
    assert_ne!(MARK_COLOR, SPACE_COLOR);
}
