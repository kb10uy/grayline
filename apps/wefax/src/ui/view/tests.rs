//! egui reports duplicate widget ids and impossible layouts by panicking at
//! runtime, so every locale is rendered here rather than only in front of an
//! operator.

use egui_kittest::{Harness, kittest::Queryable};
use grayline_shell::i18n::Locale;
use grayline_wefax::Format;
use rstest::rstest;

use super::*;
use crate::{ui::menu, worker::receive::StripUpdate};

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
    // Twice: the first frame measures each group, the second lays them out
    // against what it measured.
    harness.run();
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
    harness.get_by_label(&i18n.text("action-start"));
    harness.get_by_label(&i18n.text("action-clear"));
}

#[test]
fn a_reception_in_progress_draws_its_geometry() {
    let mut app = App::headless();
    app.format = Some(Format::MARINE);
    render(&mut app);
    assert!(describe(&app.i18n, RxProgress::Imaging { lines: 12 }, &app).contains("IOC 576"));
}

/// The status line carries the message on one side and the state on the
/// other, so both have to be drawn on the same row.
#[test]
fn the_status_line_shows_a_message_beside_the_state() {
    let mut app = App::headless();
    app.notice = Some("something happened".to_owned());
    let state = app.i18n.text(app.progress().label_key());
    let harness = render(&mut app);
    harness.get_by_label("something happened");
    harness.get_by_label(&state);
}

#[test]
fn the_line_rate_reads_as_unknown_before_a_reception() {
    let mut app = App::headless();
    assert!(app.line_rate_error_ppm().is_none());
    let harness = render(&mut app);
    harness.get_by_label("\u{2014} ppm");
}

/// A window too narrow for one row has to put the bar on two, and every
/// control has to still be there when it does.
#[test]
fn a_narrow_window_keeps_every_control() {
    let mut app = App::headless();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let harness = render_sized(&mut app, egui::vec2(420.0, 500.0));
    for key in ["action-start", "action-clear", "action-auto-start", "action-invert"] {
        harness.get_by_label(&i18n.text(key));
    }
    harness.get_by_label("\u{2014} ppm");

    // The bar has to have wrapped for this to be worth asserting, and the
    // group has to have stayed on one row when it did.
    let top = |key: &str| harness.get_by_label(&i18n.text(key)).rect().top();
    assert!(
        (top("action-auto-start") - top("action-invert")).abs() < 1.0,
        "the flags broke across rows"
    );
    assert!(
        top("action-start") > top("action-auto-start"),
        "the bar should have wrapped at this width"
    );
}

/// The groups are what the bar breaks between, so each is laid out as one
/// item rather than as the widgets inside it.
///
/// Their widths are what makes that work, and a group is measured from what it
/// drew, so a width of nothing means the group never reached the layout.
#[test]
fn every_group_is_measured_as_one_item() {
    let mut app = App::headless();
    let harness = render_sized(&mut app, egui::vec2(1_280.0, 500.0));
    for salt in ["geometry", "flags", "actions", "phase", "slant", "save"] {
        let id = egui::Id::new("wefax-control-block").with(salt);
        let width = harness
            .ctx
            .memory(|memory| memory.data.get_temp::<f32>(id))
            .unwrap_or_default();
        assert!(width > 0.0, "the {salt} group was never measured");
    }
}

/// The picture takes whatever the control bar leaves, and drawing it must not
/// depend on a reception having started.
#[test]
fn an_empty_strip_still_draws() {
    let mut app = App::headless();
    render(&mut app);
    assert!(app.strip.is_empty());
}

#[test]
fn a_strip_with_columns_draws() {
    let mut app = App::headless();
    let update = StripUpdate {
        width: 64,
        first_line: 0,
        gray: vec![200; 64 * 8],
        replaces_all: false,
    };
    let ctx = egui::Context::default();
    app.strip.apply(&ctx, &update);
    render(&mut app);
    assert_eq!(app.strip.lines(), 8);
}

/// Acquiring is one colour whichever half of it the receiver is in — the
/// label already says which — but a reception that is drawing, one that
/// finished, and one that was cut short have to be told apart at a glance.
#[test]
fn the_states_worth_telling_apart_have_their_own_colours() {
    let groups = [
        RxProgress::Idle,
        RxProgress::Phasing,
        RxProgress::Imaging { lines: 1 },
        RxProgress::Complete { lines: 1 },
        RxProgress::Stopped { lines: 1 },
    ];
    for (index, state) in groups.iter().enumerate() {
        for other in &groups[index + 1..] {
            assert_ne!(state_color(*state), state_color(*other), "{state:?} and {other:?}");
        }
    }
    assert_eq!(state_color(RxProgress::Starting), state_color(RxProgress::Phasing));
}
