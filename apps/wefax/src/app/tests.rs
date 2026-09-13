use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use grayline_shell::{common::DEFAULT_UI_SCALE, i18n::Locale};
use grayline_wefax::{Format, Ioc, LinesPerMinute};
use rstest::rstest;

use super::*;
use crate::worker::receive::StripUpdate;

/// A platform that records what the interface asked it for.
#[derive(Default)]
struct RecordingPlatform {
    activities: Arc<Mutex<Vec<Activity>>>,
    opened: Arc<Mutex<Vec<PathBuf>>>,
    visited: Arc<Mutex<Vec<String>>>,
}

impl Platform for RecordingPlatform {
    fn set_activity(&mut self, activity: Activity) {
        self.activities.lock().unwrap().push(activity);
    }

    fn open_path(&mut self, path: &Path) -> io::Result<()> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }

    fn open_url(&mut self, url: &str) -> io::Result<()> {
        self.visited.lock().unwrap().push(url.to_owned());
        Ok(())
    }
}

fn columns(width: usize, first_line: usize, lines: usize, replaces_all: bool) -> StripUpdate {
    StripUpdate {
        width,
        first_line,
        gray: vec![128; lines * width],
        replaces_all,
    }
}

#[test]
fn a_headless_application_starts_idle_and_empty() {
    let app = App::headless();
    assert_eq!(app.progress(), RxProgress::Idle);
    assert!(app.strip.is_empty());
    assert!(app.notice.is_none());
    assert_eq!(app.title(), format!("Grayline WEFAX {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn the_title_says_what_the_receiver_is_doing() {
    let mut app = App::headless();
    app.i18n = I18n::new(Locale::En, &CATALOG);
    assert!(!app.title().contains('\u{2014}'));
}

#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn switching_the_language_relabels_the_interface(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    assert_eq!(app.i18n.locale(), locale);
    assert_ne!(app.i18n.text("action-start"), "action-start");
}

#[test]
fn the_zoom_stays_inside_what_the_settings_allow() {
    let mut app = App::headless();
    app.zoom_by(10.0);
    assert_eq!(app.ui_scale, *UI_SCALE_RANGE.end());
    app.zoom_by(-10.0);
    assert_eq!(app.ui_scale, *UI_SCALE_RANGE.start());
    app.set_ui_scale(DEFAULT_UI_SCALE);
    assert_eq!(app.ui_scale, DEFAULT_UI_SCALE);
}

#[test]
fn the_geometry_reaches_the_worker_settings() {
    let mut app = App::headless();
    app.ioc = Ioc::Ioc288;
    app.lines_per_minute = LinesPerMinute::L240;
    app.inverted = true;
    app.push_settings();

    let settings = app.audio.settings();
    assert_eq!(settings.format.ioc, Ioc::Ioc288);
    assert_eq!(settings.format.lines_per_minute, LinesPerMinute::L240);
    assert!(settings.inverted);
}

#[test]
fn clearing_forgets_the_picture_and_the_geometry() {
    let ctx = egui::Context::default();
    let mut app = App::headless();
    app.strip.apply(&ctx, &columns(8, 0, 3, false));
    app.format = Some(Format::MARINE);

    app.clear();

    assert!(app.strip.is_empty());
    assert!(app.format.is_none());
}

#[test]
fn a_new_capture_session_starts_the_strip_over() {
    let ctx = egui::Context::default();
    let mut app = App::headless();
    app.strip.apply(&ctx, &columns(8, 0, 3, false));
    assert!(!app.strip.is_empty());

    // A device change bumps the session, and the worker behind it begins its
    // lines again at zero.
    app.session = app.session.wrapping_sub(1);
    app.poll_workers(&ctx);

    assert!(app.strip.is_empty());
}

#[test]
fn saving_with_nothing_received_says_so_rather_than_writing_a_file() {
    let mut app = App::headless();
    app.save_chart();
    assert_eq!(
        app.notice.as_deref(),
        Some(app.i18n.text("error-nothing-to-save").as_str())
    );
}

#[test]
fn a_received_chart_is_written_where_the_operator_browses() {
    let ctx = egui::Context::default();
    let mut app = App::headless();
    std::fs::create_dir_all(app.paths.received_dir()).unwrap();
    app.strip.apply(&ctx, &columns(4, 0, 2, false));
    app.format = Some(Format::MARINE);

    app.save_chart();

    let notice = app.notice.clone().expect("the save was reported");
    assert!(notice.starts_with(&app.i18n.text("status-saved")), "{notice}");
    let saved: Vec<_> = std::fs::read_dir(app.paths.received_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "png"))
        .collect();
    assert!(!saved.is_empty());
    for entry in saved {
        std::fs::remove_file(entry.path()).ok();
    }
}

#[test]
fn opening_a_folder_reaches_the_platform() {
    let platform = RecordingPlatform::default();
    let opened = Arc::clone(&platform.opened);
    let mut app = App::headless_on(Box::new(platform));

    app.reveal(Folder::Received);
    app.reveal(Folder::Config);

    assert_eq!(opened.lock().unwrap().len(), 2);
}

/// The manual this application opens has to be its own pages: the site
/// carries one set per application, and the SSTV pages describe equipment
/// this one has nothing to do with.
#[test]
fn the_manual_opens_at_this_application_s_own_address() {
    let platform = RecordingPlatform::default();
    let visited = Arc::clone(&platform.visited);
    let mut app = App::headless_on(Box::new(platform));

    app.open_manual();

    assert_eq!(visited.lock().unwrap().as_slice(), [crate::identity::MANUAL_URL]);
    assert_eq!(app.notice, None);
}

/// Nothing changed means nothing written: the settings file is the operator's,
/// and rewriting it on every frame would touch its timestamp forever.
#[test]
fn persisting_an_unchanged_application_writes_nothing() {
    let mut app = App::headless();
    let before = app.saved.clone();
    app.persist();
    assert_eq!(app.saved, before);

    app.inverted = !app.inverted;
    app.persist();
    assert_ne!(app.saved, before);
}

#[test]
fn an_idle_receiver_asks_for_no_frames() {
    let app = App::headless();
    assert!(
        app.repaint_after().is_none(),
        "a disconnected device has nothing to draw"
    );
}

#[test]
fn selecting_a_device_that_is_not_there_changes_nothing() {
    let mut app = App::headless();
    app.select_device_named("no such device");
    assert!(app.audio.device.is_none());
}

#[test]
fn a_recording_that_cannot_be_read_is_reported() {
    let mut app = App::headless();
    app.open_wav(Path::new("no-such-recording.wav"));
    let notice = app.notice.clone().expect("the failure was reported");
    assert!(notice.starts_with(&app.i18n.text("error-wav")), "{notice}");
    assert!(!app.audio.is_capturing());
}

/// The whole path a dropped file takes: read, demodulated, decoded, and drawn
/// as columns of the strip.
#[test]
fn a_recording_is_decoded_into_the_strip() {
    let root = crate::test_util::TempDir::new();
    let path = root.path().join("chart.wav");
    let format = Format::MARINE;
    write_transmission(&path, 11_025, format);

    let ctx = egui::Context::default();
    let mut app = App::headless();
    // Saving is what an operator wants and what a test does not.
    app.auto_save = false;
    app.open_wav(&path);
    assert!(app.audio.is_capturing(), "{:?}", app.notice);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline && app.strip.lines() < 4 {
        app.poll_workers(&ctx);
        std::thread::sleep(Duration::from_millis(4));
    }

    assert!(
        app.strip.lines() >= 4,
        "only {} lines decoded: progress={:?} level={} apt={:?} notice={:?}",
        app.strip.lines(),
        app.progress(),
        app.audio.snapshot().signal_level,
        app.audio.snapshot().apt,
        app.notice
    );
    assert_eq!(app.format, Some(format));
    assert_eq!(app.strip.width(), format.pixels_per_line());

    // The recording ends, and the picture is still the operator's to correct.
    // Nothing is arriving by then, so a correction has to reach the strip on
    // its own account rather than riding the next block of audio.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline && app.progress().is_active() {
        app.poll_workers(&ctx);
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(!app.progress().is_active(), "the recording should have ended");

    for correction in [Correction::Phase(64), Correction::Slant(200.0)] {
        let before = app.strip.gray().to_vec();
        match correction {
            Correction::Phase(pixels) => app.shift_phase(pixels),
            Correction::Slant(ppm) => app.adjust_slant_ppm(ppm),
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline && app.strip.gray() == before.as_slice() {
            app.poll_workers(&ctx);
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_ne!(
            app.strip.gray(),
            before.as_slice(),
            "{correction:?} never reached the strip"
        );
    }
}

/// What the operator can still change once a chart has arrived.
#[derive(Clone, Copy, Debug)]
enum Correction {
    Phase(i64),
    Slant(f64),
}

/// Writes a start tone, a phasing signal, and a few lines of bars.
fn write_transmission(path: &Path, rate: u32, format: Format) {
    use grayline_wefax::WefaxBand;
    use grayline_wefax::format::{APT_TONE_SECONDS, PHASING_WHITE_FRACTION};
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::f64::consts::TAU;

    let band = WefaxBand::WIDE;
    let line = format.samples_per_line(rate);
    let width = format.pixels_per_line();
    let mut writer = WavWriter::create(
        path,
        WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .unwrap();

    let mut phase = 0.0_f64;
    let mut emit = |frequency_hz: f64, writer: &mut WavWriter<_>| {
        writer.write_sample((phase.sin() * 24_000.0) as i16).unwrap();
        phase = (phase + TAU * frequency_hz / f64::from(rate)).rem_euclid(TAU);
    };

    let mut keying = 0.0_f64;
    for _ in 0..(f64::from(rate) * APT_TONE_SECONDS) as usize {
        let level = if keying < 0.5 { 0 } else { u8::MAX };
        emit(band.frequency_hz(level), &mut writer);
        keying = (keying + format.ioc.apt_start_hz() / f64::from(rate)).fract();
    }
    for sample in 0..(f64::from(rate) * 8.0) as usize {
        let white = (sample as f64 / line).fract() < PHASING_WHITE_FRACTION;
        emit(band.frequency_hz(if white { u8::MAX } else { 0 }), &mut writer);
    }
    for sample in 0..(line * 12.0) as usize {
        let pixel = (((sample as f64 / line).fract() * width as f64) as usize).min(width - 1);
        let level = if (pixel * 8 / width).is_multiple_of(2) {
            0
        } else {
            u8::MAX
        };
        emit(band.frequency_hz(level), &mut writer);
    }
    writer.finalize().unwrap();
}
