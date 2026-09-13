//! Duplicate widget ids and impossible layouts only surface at runtime, so
//! every locale is rendered here rather than only in front of an operator.

use egui_kittest::{
    Harness,
    kittest::{Queryable, by},
};
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

/// Draws the window with the fonts the application installs for itself.
///
/// The bar's alignment answers to the installed font's metrics.
fn render_as_installed(app: &mut App, size: egui::Vec2) -> Harness<'_> {
    let mut harness = Harness::builder().with_size(size).build_ui(|ui| {
        let model = menu::model(app);
        view(ui, app, &model, menu::is_in_window());
    });
    crate::install_fonts(&harness.ctx);
    harness.run_steps(8);
    harness
}

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

/// Every control on the bar has to sit on the same line as its neighbours.
///
/// Drawn with the fonts the application actually installs, because that is
/// where the differences show: a combo box, a button and a check box come out
/// a fraction of a point apart under a system font, and a combo box lays
/// itself out from the top of the room it is given rather than centring in it,
/// so the two differences add up.
#[test]
fn every_control_on_a_row_shares_its_centre() {
    let mut app = App::headless();
    let i18n = I18n::new(Locale::En, &crate::locales::CATALOG);
    let harness = render_as_installed(&mut app, egui::vec2(1_280.0, 500.0));

    // A combo box carries its selection as a value rather than as a label.
    let button = |label: &str| harness.get_by_label(label).rect().center().y;
    let combo = |text: &str| harness.get(by().value(text)).rect().center().y;
    let reference = button(&i18n.text("action-start"));
    for (name, found) in [
        ("IOC 576", combo("IOC 576")),
        ("120 LPM", combo("120 LPM")),
        ("Auto start", button(&i18n.text("action-auto-start"))),
        ("Clear", button(&i18n.text("action-clear"))),
        ("Save", button(&i18n.text("action-save"))),
    ] {
        assert!(
            (found - reference).abs() < 0.01,
            "{name} sits {} points off the row",
            found - reference
        );
    }
}

#[test]
fn the_bar_settles_and_stops_asking_for_frames() {
    let mut app = App::headless();
    let mut harness = render_as_installed(&mut app, egui::vec2(1_280.0, 500.0));
    // `run` gives up after a few steps if frames keep being asked for, which
    // is the assertion: by now nothing should be.
    harness.run();
}

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

#[test]
fn labels_are_inert() {
    let mut app = App::headless();

    let harness = render(&mut app);

    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        assert!(
            !harness.ctx.style_of(theme).interaction.selectable_labels,
            "{theme:?} labels should be inert"
        );
    }
}
