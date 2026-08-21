use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use grayline_rtty::ToneSet;
use grayline_shell::{
    common::{COMMON_FILE, DEFAULT_UI_SCALE},
    i18n::Locale,
};
use rstest::rstest;

use super::*;

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

#[test]
fn a_headless_application_starts_empty_and_quiet() {
    let app = App::headless();
    assert_eq!(app.columns.len(), DecodePath::ALL.len());
    assert!(app.columns.iter().all(Scrollback::is_empty));
    assert!(app.notice.is_none());
    assert!(!app.is_printing());
    assert_eq!(app.title(), format!("Grayline RTTY {}", env!("CARGO_PKG_VERSION")));
}

#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn switching_the_language_relabels_the_interface(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    assert_eq!(app.i18n.locale(), locale);
    assert_ne!(app.i18n.text("action-clear"), "action-clear");
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

/// The pair is two figures on the panel and one pair in the receiver, and the
/// space tone is the one the panel never names.
#[test]
fn the_shift_is_added_to_the_mark_tone() {
    let mut app = App::headless();
    app.mark_hz = 1_275.0;
    app.shift_hz = 850.0;
    assert_eq!(
        app.tones(),
        ToneSet {
            mark_hz: 1_275.0,
            space_hz: 2_125.0
        }
    );
}

#[test]
fn the_tuning_settings_reach_the_worker() {
    let mut app = App::headless();
    app.mark_hz = 1_275.0;
    app.shift_hz = 425.0;
    app.baud = 75.0;
    app.reverse = true;
    app.afc = true;
    app.push_settings();

    let settings = app.audio.settings();
    assert_eq!(settings.tones.mark_hz, 1_275.0);
    assert_eq!(settings.tones.space_hz, 1_700.0);
    assert_eq!(settings.baud, 75.0);
    assert!(settings.reverse);
    assert!(settings.afc);
}

/// The threshold is only a threshold while the squelch is on; the core takes
/// its absence as the squelch being off.
#[test]
fn switching_the_squelch_off_takes_the_threshold_with_it() {
    let mut app = App::headless();
    app.squelch = false;
    app.push_settings();
    assert_eq!(app.audio.settings().squelch, None);

    app.squelch = true;
    app.squelch_threshold = 0.4;
    app.push_settings();
    assert_eq!(app.audio.settings().squelch, Some(0.4));
}

/// A figure the receiver could not be built from must not reach it, whichever
/// way it was typed.
#[test]
fn what_is_persisted_is_brought_back_into_range() {
    let mut app = App::headless();
    app.mark_hz = 40_000.0;
    app.squelch_threshold = 9.0;
    app.push_settings();

    assert_eq!(app.audio.settings().tones.mark_hz, MAXIMUM_MARK_HZ);
    assert_eq!(app.audio.settings().squelch, Some(1.0));
}

#[test]
fn clearing_throws_away_what_was_printed() {
    let mut app = App::headless();
    app.columns[0].push_str("CQ CQ");
    app.clear();
    assert!(app.columns[0].is_empty());
}

/// A device change starts a worker whose decoders begin again, so the text on
/// screen no longer belongs to what is being decoded.
#[test]
fn a_new_capture_session_starts_the_text_over() {
    let mut app = App::headless();
    app.columns[0].push_str("RYRYRY");
    app.session = app.session.wrapping_sub(1);

    app.poll_workers();

    assert!(app.columns[0].is_empty());
}

/// What the frequency control found is lost when the receiver is next built,
/// so taking it has to move the panel's own figures.
#[test]
fn the_detected_pair_can_be_taken_up() {
    let mut app = App::headless();
    app.adopt_detected_tones();
    // Nothing has been received, so there is nothing to take and the figures
    // stay where the operator left them.
    assert_eq!(app.mark_hz, Settings::default().mark_hz);
}

#[test]
fn a_recording_that_is_not_there_is_reported() {
    let mut app = App::headless();
    app.open_wav(Path::new("no-such-recording.wav"));
    assert!(app.notice.is_some());
}

#[test]
fn the_folders_are_opened_through_the_platform() {
    let platform = RecordingPlatform::default();
    let opened = Arc::clone(&platform.opened);
    let mut app = App::headless_on(Box::new(platform));

    app.reveal(Folder::Config);

    assert_eq!(opened.lock().unwrap().len(), 1);
    assert!(app.notice.is_none());
}

#[test]
fn the_manual_is_opened_at_this_applications_own_address() {
    let platform = RecordingPlatform::default();
    let visited = Arc::clone(&platform.visited);
    let mut app = App::headless_on(Box::new(platform));

    app.open_manual();

    assert_eq!(visited.lock().unwrap().as_slice(), [crate::identity::MANUAL_URL]);
}

/// An idle watch must not hold the machine awake, and one that is printing
/// must.
#[test]
fn only_a_printing_watch_keeps_the_machine_awake() {
    let platform = RecordingPlatform::default();
    let activities = Arc::clone(&platform.activities);
    let mut app = App::headless_on(Box::new(platform));

    app.poll_workers();

    assert!(
        activities
            .lock()
            .unwrap()
            .iter()
            .all(|activity| *activity == Activity::Idle),
        "an idle watch asked the platform for something"
    );
}

/// The language and the zoom are read from the file the family shares and
/// written back to it, which is what makes a language chosen in one
/// application the language the next one opens in.
#[test]
fn the_shared_settings_are_read_and_written_where_the_family_keeps_them() {
    let root = crate::test_util::TempDir::new();
    let shared = root.path().join(COMMON_FILE);
    fs::write(&shared, "language = \"ja\"\n").unwrap();

    let settings = Settings::default();
    let mut app = App::from_parts(
        AudioState::disconnected(worker_settings(&settings)),
        AppPaths::from_roots(root.path().join("config"), root.path().join("state")),
        Config::detached(),
        &settings,
        CommonConfig::load(shared.clone()),
        Box::new(grayline_shell::platform::QuietPlatform),
    );
    assert_eq!(app.i18n.locale(), Locale::Ja);

    app.set_ui_scale(1.5);
    app.persist();

    let written = fs::read_to_string(&shared).unwrap();
    assert!(written.contains("ui-scale = 1.5"), "{written}");
    assert!(written.contains("language = \"ja\""), "{written}");
}

#[test]
fn a_settings_change_is_persisted_once() {
    let mut app = App::headless();
    app.baud = 50.0;
    app.persist();
    assert_eq!(app.saved.baud, 50.0);
    // The second call has nothing to write, which is what keeps the file from
    // being rewritten on every frame.
    app.persist();
    assert_eq!(app.saved.baud, 50.0);
}
