//! The window is drawn from what the application hands it, so what it is
//! handed is what these check: a picture nothing reaches is the failure this
//! whole path has, and it looks exactly like a quiet band.

use egui_kittest::{Harness, kittest::Queryable};
use rstest::rstest;

use super::*;
use crate::locales::CATALOG;

fn levels(mark: f64, space: f64) -> ChannelLevels {
    ChannelLevels { mark, space }
}

fn frame(points: Vec<ChannelLevels>) -> ScopeFrame {
    ScopeFrame {
        points,
        spectrum: Vec::new(),
        bin_hz: 0.0,
        sequence: 1,
    }
}

/// An open window, named in `locale`.
fn opened(locale: Locale) -> Scope {
    let mut scope = Scope::default();
    scope.set_open(true);
    scope.describe(&I18n::new(locale, &CATALOG));
    scope
}

#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn the_window_draws_in_every_locale(#[case] locale: Locale) {
    let i18n = I18n::new(locale, &CATALOG);
    let scope = opened(locale);
    scope.push(ScopeFrame {
        points: (0..64).map(|index| levels(f64::from(index) / 64.0, 0.2)).collect(),
        spectrum: (0..128).map(|bin| bin as f32).collect(),
        bin_hz: 23.4,
        sequence: 1,
    });
    scope.retune(ToneSet::AFSK_170);

    let view = scope.view();
    let mut harness = Harness::new_ui(|ui| contents(ui, &view));
    harness.run();
    harness.get_by_label(&i18n.text("label-spectrum"));
    harness.get_by_label(&i18n.text("label-channels"));
}

/// A window opened on a band nobody is transmitting on draws nothing, and
/// nothing is what it has to survive drawing.
#[test]
fn a_window_with_nothing_to_draw_still_draws() {
    let scope = opened(Locale::En);
    let view = scope.view();
    let mut harness = Harness::new_ui(|ui| contents(ui, &view));
    harness.run();
    harness.get_by_label(&I18n::new(Locale::En, &CATALOG).text("label-spectrum"));
}

/// The window is named after the application, so that a taskbar full of
/// windows says which one this belongs to.
#[test]
fn the_window_is_named_in_the_operators_language() {
    let scope = opened(Locale::Ja);
    let title = scope.view().labels.title.clone();
    assert!(title.starts_with(crate::identity::DISPLAY_NAME), "{title}");
    assert!(
        title.ends_with(&I18n::new(Locale::Ja, &CATALOG).text("window-scope")),
        "{title}"
    );
}

#[test]
fn a_trace_keeps_the_pairs_it_draws_and_drops_the_rest() {
    let scope = opened(Locale::En);
    let pairs = TRACE_POINTS + 200;
    scope.push(frame((0..pairs).map(|index| levels(index as f64, 0.0)).collect()));

    let view = scope.view();
    assert_eq!(view.points.len(), TRACE_POINTS);
    // The newest are the ones kept: what a scope shows is what is arriving.
    assert_eq!(view.points.back().map(|pair| pair.mark), Some((pairs - 1) as f64));
}

/// The transform fills a fifth of a second behind the tap, so the first
/// frames of a reception carry pairs and no band. Blanking the picture for
/// them would flicker the display every time the receiver was rebuilt.
#[test]
fn a_frame_without_a_band_keeps_the_one_before_it() {
    let scope = opened(Locale::En);
    scope.push(ScopeFrame {
        points: vec![levels(0.5, 0.1)],
        spectrum: vec![1.0, 2.0, 3.0],
        bin_hz: 23.4,
        sequence: 1,
    });
    scope.push(frame(vec![levels(0.4, 0.2)]));

    let view = scope.view();
    assert_eq!(view.spectrum, [1.0, 2.0, 3.0]);
    assert_eq!(view.bin_hz, 23.4);
}

#[test]
fn closing_the_window_throws_away_what_was_on_it() {
    let mut scope = opened(Locale::En);
    scope.push(ScopeFrame {
        points: vec![levels(0.5, 0.1)],
        spectrum: vec![1.0, 2.0],
        bin_hz: 23.4,
        sequence: 1,
    });

    scope.set_open(false);
    let view = scope.view();
    assert!(view.points.is_empty());
    assert!(view.spectrum.is_empty());
}

/// The operator closes the window from its own frame, which the application
/// only hears about through this: a request read twice would close a window
/// they opened again in between.
#[test]
fn a_close_request_is_taken_once() {
    let scope = opened(Locale::En);
    scope.shared.close_requested.store(true, Ordering::Relaxed);
    assert!(scope.take_close_request());
    assert!(!scope.take_close_request());
}

/// Opening the window again after the operator closed it must not close it
/// on the frame it opened.
#[test]
fn opening_the_window_forgets_the_close_that_shut_it() {
    let mut scope = opened(Locale::En);
    scope.shared.close_requested.store(true, Ordering::Relaxed);
    scope.set_open(false);
    scope.set_open(true);
    assert!(!scope.take_close_request());
}

/// Everything the window is fed sets this, because the main window sleeps
/// between frames and the scope is asked for its own.
#[test]
fn anything_worth_drawing_asks_for_a_frame() {
    let scope = opened(Locale::En);
    assert!(scope.shared.moved.swap(false, Ordering::Relaxed));

    scope.push(frame(vec![levels(0.5, 0.1)]));
    assert!(scope.shared.moved.swap(false, Ordering::Relaxed));

    let moved_to = ToneSet::from_center_and_shift(1_700.0, 850.0);
    scope.retune(moved_to);
    assert!(scope.shared.moved.swap(false, Ordering::Relaxed));

    // The pair the frequency control is on has not moved, and a picture that
    // did not change is not worth waking the window for.
    scope.retune(moved_to);
    assert!(!scope.shared.moved.swap(false, Ordering::Relaxed));
}

#[test]
fn a_full_scale_tone_reaches_the_top_of_the_band() {
    assert_eq!(level(FULL_SCALE), 1.0);
    assert_eq!(level(0.0), 0.0);
    // Ten decibels below the floor is still the floor rather than a negative
    // height, which would draw downwards out of the box.
    let below = FULL_SCALE * 10.0_f32.powf(-(FLOOR_DB + 10.0) / 20.0);
    assert_eq!(level(below), 0.0);

    let half = level(FULL_SCALE * 10.0_f32.powf(-FLOOR_DB / 40.0));
    assert!((half - 0.5).abs() < 0.01, "half the scale should be halfway up: {half}");
}

/// Mark to the right and space upwards, which is the picture MMTTY's own XY
/// scope draws and what the two axes are labelled with.
#[test]
fn a_pair_is_plotted_towards_the_channel_that_is_louder() {
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
    assert_eq!(point(rect, levels(1.0, 0.0), 1.0), egui::pos2(100.0, 100.0));
    assert_eq!(point(rect, levels(0.0, 1.0), 1.0), egui::pos2(0.0, 0.0));
    assert_eq!(point(rect, levels(0.5, 0.5), 1.0), egui::pos2(50.0, 50.0));
    // A reading past the scale is drawn at the edge rather than outside it.
    assert_eq!(point(rect, levels(4.0, 0.0), 1.0), egui::pos2(100.0, 100.0));
}

/// The channels carry an envelope whose height depends on the band-pass, so
/// the trace is drawn against the strongest pair on it — with a floor, or
/// silence would be magnified into a picture.
#[test]
fn a_trace_is_scaled_by_the_strongest_pair_on_it() {
    let quiet: VecDeque<_> = [levels(0.01, 0.005)].into_iter().collect();
    assert_eq!(trace_scale(&quiet), MINIMUM_TRACE_SCALE);

    let loud: VecDeque<_> = [levels(0.2, 0.9), levels(0.4, 0.1)].into_iter().collect();
    assert_eq!(trace_scale(&loud), 0.9);
}

/// A column narrower than a bin repeats it rather than reading past the end
/// of the spectrum, which is what a wide window on a low capture rate does.
#[rstest]
#[case(8, 4)]
#[case(4, 8)]
#[case(1, 1)]
fn every_column_names_a_bin_that_exists(#[case] columns: usize, #[case] bins: usize) {
    for column in 0..=columns {
        assert!(bin_at(column, columns, bins) < bins);
    }
}

#[test]
fn a_frequency_sits_where_it_belongs_across_the_band() {
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 20.0));
    assert_eq!(x_of(rect, 4_000.0, 0.0), 0.0);
    assert_eq!(x_of(rect, 4_000.0, 2_000.0), 50.0);
    assert_eq!(x_of(rect, 4_000.0, 4_000.0), 100.0);
    // A pair the panel allows but the capture rate cannot show stays inside
    // the box rather than being drawn beside it.
    assert_eq!(x_of(rect, 4_000.0, 6_000.0), 100.0);
}
