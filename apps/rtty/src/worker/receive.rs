use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use grayline_audio::CaptureReader;
use grayline_rtty::{
    BaudRate, RxConfig, RxFraming, ToneSet,
    code::Case,
    rx::{AfcConfig, AtcDesign, ChannelLevels},
};

use crate::{error::AppError, worker::Waker};

mod session;
mod spectrum;

use session::run;

pub use spectrum::FULL_SCALE;

/// The most channel pairs one frame carries.
///
/// Reached only where the interface stopped drawing for a third of a second
/// while the tap kept collecting; what does not fit is the oldest, for the
/// reason the core's own ring gives.
const SCOPE_POINTS_LIMIT: usize = 8_192;

/// One decode path a receive column runs.
///
/// The interface is built for several — every column is fed the same PCM and
/// each carries a pipeline of its own — but the core has one demodulator, so
/// there is one variant. The discriminators MMTTY carries and this project
/// deferred (`docs/memo/mmtty/porting.md`) become the others, and the column
/// count in the interface follows this list.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DecodePath {
    /// The resonator pair and the majority-vote framing machine, which is
    /// what `grayline-rtty` demodulates with.
    #[default]
    Resonator,
}

impl DecodePath {
    pub const ALL: [Self; 1] = [Self::Resonator];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Resonator => "path-resonator",
        }
    }
}

/// What one decode path has to say for itself.
///
/// The text is what was decoded since the last snapshot rather than
/// everything decoded so far: a session left running for an evening would
/// otherwise hand the whole of its scrollback across on every block of audio.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnSnapshot {
    pub path: DecodePath,
    pub text: String,
    /// The smoothed `|mark − space|` reading the squelch works on.
    pub signal_strength: f32,
    /// The signed `mark − space` reading, which says which tone is being
    /// heard rather than how much of it there is.
    pub difference: f32,
    pub case: Case,
    /// The pair being detected, which automatic frequency control moves.
    pub tones: ToneSet,
    pub squelch_open: bool,
}

/// What the scope window draws, published only while it is open.
///
/// The two halves come from different places on purpose: the pairs are the
/// ones the comparator compared, tapped from the first decode path, and the
/// band is a transform of the raw input that nothing in the receiver reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScopeFrame {
    /// The channel pairs the monitor tap kept since the last snapshot.
    pub points: Vec<ChannelLevels>,
    /// Magnitudes from bin zero upwards, one every [`ScopeFrame::bin_hz`].
    pub spectrum: Vec<f32>,
    pub bin_hz: f32,
    /// Counts the frames the tap has produced.
    ///
    /// Nothing on a scope is compared against a threshold — a trace that
    /// looks like the last one is still a new trace, and a band that has not
    /// moved is still being watched — so this is what tells the interface
    /// that there is a frame worth drawing.
    pub sequence: u64,
}

/// One observation of every decode path.
#[derive(Clone, Debug, Default)]
pub struct RxSnapshot {
    pub columns: Vec<ColumnSnapshot>,
    /// What the scope window draws, while one is open.
    pub scope: Option<ScopeFrame>,
    pub dropped_samples: u64,
    pub error: Option<AppError>,
}

/// The parts of a snapshot the interface actually draws.
///
/// The worker publishes on every block of audio it reads, and most of those
/// say what the one before said. Comparing this instead of the whole snapshot
/// is what keeps the interface asleep between the observations worth showing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Visible {
    columns: Vec<VisibleColumn>,
    /// The frame the scope window would draw, which is a new picture every
    /// time whatever it shows.
    scope: Option<u64>,
    has_error: bool,
    dropped_samples: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct VisibleColumn {
    has_text: bool,
    signal_strength: u8,
    difference: i8,
    case: Case,
    squelch_open: bool,
    /// The pair in whole hertz: automatic frequency control moves it by
    /// fractions, and a readout in hertz cannot show them.
    tones: (i32, i32),
}

impl Visible {
    fn of(snapshot: &RxSnapshot) -> Self {
        Self {
            columns: snapshot
                .columns
                .iter()
                .map(|column| VisibleColumn {
                    has_text: !column.text.is_empty(),
                    signal_strength: quantize(column.signal_strength),
                    difference: quantize_signed(column.difference),
                    case: column.case,
                    squelch_open: column.squelch_open,
                    tones: (column.tones.mark_hz as i32, column.tones.space_hz as i32),
                })
                .collect(),
            scope: snapshot.scope.as_ref().map(|frame| frame.sequence),
            has_error: snapshot.error.is_some(),
            dropped_samples: snapshot.dropped_samples,
        }
    }
}

/// Rounds a reading to the steps a meter can be seen to move in.
fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 64.0) as u8
}

fn quantize_signed(value: f32) -> i8 {
    (value.clamp(-1.0, 1.0) * 64.0) as i8
}

/// Single-slot handoff holding the newest observation.
#[derive(Debug, Default)]
pub(super) struct Mailbox {
    slot: Mutex<Slot>,
    waker: Waker,
}

#[derive(Debug, Default)]
struct Slot {
    pending: Option<RxSnapshot>,
    shown: Visible,
}

impl Mailbox {
    pub(super) fn new(waker: Waker) -> Self {
        Self {
            slot: Mutex::new(Slot::default()),
            waker,
        }
    }

    /// Replaces the pending snapshot, keeping payloads not yet collected.
    pub(super) fn publish(&self, mut snapshot: RxSnapshot) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = slot.pending.take() {
            merge(&mut snapshot, previous);
        }
        let visible = Visible::of(&snapshot);
        slot.pending = Some(snapshot);
        if visible != slot.shown {
            slot.shown = visible;
            drop(slot);
            self.waker.wake();
        }
    }

    fn take(&self) -> Option<RxSnapshot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).pending.take()
    }
}

/// Folds an uncollected snapshot into the one replacing it.
///
/// Text is the one thing a snapshot carries that cannot be replaced: it is
/// what was decoded during that block and nothing else will ever say it
/// again, so an uncollected snapshot's characters go in front of the newer
/// ones rather than being dropped for them.
fn merge(snapshot: &mut RxSnapshot, previous: RxSnapshot) {
    if snapshot.error.is_none() {
        snapshot.error = previous.error;
    }
    if let (Some(frame), Some(earlier)) = (snapshot.scope.as_mut(), previous.scope) {
        merge_scope(frame, earlier);
    }
    // Two snapshots with different column counts cannot be lined up against
    // each other, and nothing changes the count while a worker runs, so this
    // is the shape of the check rather than a case that happens.
    if snapshot.columns.len() != previous.columns.len() {
        return;
    }
    for (column, earlier) in snapshot.columns.iter_mut().zip(previous.columns) {
        if !earlier.text.is_empty() {
            column.text.insert_str(0, &earlier.text);
        }
    }
}

/// Puts the pairs an uncollected frame carried in front of the newer ones.
///
/// A trace is read as one line, so two frames that arrived between draws
/// belong to it in the order they were tapped. The band is not carried
/// across: it is a picture of the moment rather than a run of events, and the
/// newer one is that moment.
fn merge_scope(frame: &mut ScopeFrame, earlier: ScopeFrame) {
    if earlier.points.is_empty() {
        return;
    }
    let mut points = earlier.points;
    points.append(&mut frame.points);
    let excess = points.len().saturating_sub(SCOPE_POINTS_LIMIT);
    points.drain(..excess);
    frame.points = points;
}

/// Handle to a running receive worker.
///
/// Dropping the handle stops the worker and waits for it to finish, so the
/// capture queue is never left with a live consumer.
pub struct RxWorker {
    stop: Arc<AtomicBool>,
    controls: Arc<Controls>,
    mailbox: Arc<Mailbox>,
    join: Option<JoinHandle<()>>,
}

/// The knobs the interface turns while a reception runs.
///
/// One lock rather than an atomic per field: every RTTY setting is a
/// frequency, a speed, or a threshold, and the worker reads the whole set
/// once per block of audio to decide whether its pipelines still match it.
#[derive(Debug, Default)]
pub struct Controls {
    settings: Mutex<WorkerSettings>,
    /// Requests that the decode paths be started over.
    pub(super) reset: AtomicBool,
    /// Whether the scope window is open.
    ///
    /// Not a setting: what a display is doing is no business of the receiver,
    /// and putting it with the rest would rebuild every pipeline — throwing
    /// away the reception being watched — every time the window was opened.
    pub(super) scope: AtomicBool,
}

impl Controls {
    fn new(settings: WorkerSettings) -> Self {
        Self {
            settings: Mutex::new(settings),
            reset: AtomicBool::new(false),
            scope: AtomicBool::new(false),
        }
    }

    fn apply(&self, settings: WorkerSettings) {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = settings;
    }

    pub(super) fn settings(&self) -> WorkerSettings {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What a worker is started with, and everything the interface can change
/// about a reception in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkerSettings {
    pub tones: ToneSet,
    pub baud: f64,
    pub reverse: bool,
    pub afc: bool,
    /// The squelch threshold, or `None` for no squelch.
    pub squelch: Option<f64>,
    pub unshift_on_space: bool,
    pub atc: bool,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            tones: ToneSet::default(),
            baud: BaudRate::default().bits_per_second(),
            reverse: false,
            afc: false,
            squelch: Some(0.25),
            unshift_on_space: true,
            atc: false,
        }
    }
}

impl WorkerSettings {
    /// Returns the receiver these settings describe.
    ///
    /// A speed the core would refuse falls back to the amateur rate rather
    /// than failing to build a receiver: the panel cannot produce one, and a
    /// hand-edited configuration that could should still decode something.
    pub fn rx_config(self) -> RxConfig {
        RxConfig {
            tones: self.tones,
            framing: RxFraming {
                baud: BaudRate::new(self.baud).unwrap_or_default(),
                ..RxFraming::default()
            },
            reverse: self.reverse,
            unshift_on_space: self.unshift_on_space,
            atc: self.atc.then(AtcDesign::default),
            squelch_threshold: self.squelch,
            afc: self.afc.then(AfcConfig::default),
            ..RxConfig::default()
        }
    }
}

impl RxWorker {
    pub fn spawn(reader: CaptureReader, settings: WorkerSettings, waker: Waker) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let controls = Arc::new(Controls::new(settings));
        let mailbox = Arc::new(Mailbox::new(waker));
        let join = {
            let stop = Arc::clone(&stop);
            let controls = Arc::clone(&controls);
            let mailbox = Arc::clone(&mailbox);
            thread::Builder::new()
                .name("grayline-rtty-receive".to_owned())
                .spawn(move || run(reader, &mailbox, &stop, &controls))
                .ok()
        };
        // A worker that could not start would otherwise look like a live
        // receiver that never hears anything, so the failure is published
        // where every other receive failure is shown.
        if join.is_none() {
            mailbox.publish(RxSnapshot {
                error: Some(AppError::WorkerUnavailable("reception")),
                ..RxSnapshot::default()
            });
        }
        Self {
            stop,
            controls,
            mailbox,
            join,
        }
    }

    pub fn settings(&self, settings: WorkerSettings) {
        self.controls.apply(settings);
    }

    /// Starts the decode paths over, discarding whatever they were part way
    /// through.
    pub fn request_reset(&self) {
        self.controls.reset.store(true, Ordering::Relaxed);
    }

    /// Opens or closes the tap the scope window is drawn from.
    pub fn set_scope(&self, open: bool) {
        self.controls.scope.store(open, Ordering::Relaxed);
    }

    /// Returns the newest state, or `None` when nothing changed since the last
    /// call.
    pub fn latest(&self) -> Option<RxSnapshot> {
        self.mailbox.take()
    }
}

impl Drop for RxWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl core::fmt::Debug for RxWorker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("RxWorker").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn column(text: &str) -> ColumnSnapshot {
        ColumnSnapshot {
            text: text.to_owned(),
            tones: ToneSet::AFSK_170,
            ..ColumnSnapshot::default()
        }
    }

    fn snapshot(columns: Vec<ColumnSnapshot>) -> RxSnapshot {
        RxSnapshot {
            columns,
            ..RxSnapshot::default()
        }
    }

    /// Characters are the one payload nothing will say again, so two blocks
    /// that arrive between frames have to reach the interface as one run of
    /// text in the order they were decoded.
    #[test]
    fn uncollected_text_stays_in_front_of_what_follows_it() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(snapshot(vec![column("CQ ")]));
        mailbox.publish(snapshot(vec![column("DE JL1HIS")]));

        let taken = mailbox.take().unwrap();
        assert_eq!(taken.columns[0].text, "CQ DE JL1HIS");
    }

    #[test]
    fn a_snapshot_without_text_keeps_what_was_waiting() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(snapshot(vec![column("RY")]));
        mailbox.publish(snapshot(vec![column("")]));
        assert_eq!(mailbox.take().unwrap().columns[0].text, "RY");
    }

    #[test]
    fn an_uncollected_error_survives_a_newer_snapshot() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(RxSnapshot {
            error: Some(AppError::CaptureRestartFailed),
            ..RxSnapshot::default()
        });
        mailbox.publish(RxSnapshot::default());
        assert!(mailbox.take().unwrap().error.is_some());
    }

    #[test]
    fn a_collected_mailbox_reports_nothing_until_it_is_written_again() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(RxSnapshot::default());
        assert!(mailbox.take().is_some());
        assert!(mailbox.take().is_none());
    }

    /// A block of audio that moved nothing the interface draws says what the
    /// one before it said, so it must not cost a frame.
    #[test]
    fn a_negligible_change_looks_the_same() {
        let earlier = snapshot(vec![ColumnSnapshot {
            signal_strength: 0.200,
            ..ColumnSnapshot::default()
        }]);
        let later = snapshot(vec![ColumnSnapshot {
            signal_strength: 0.202,
            ..ColumnSnapshot::default()
        }]);
        assert_eq!(Visible::of(&earlier), Visible::of(&later));
    }

    #[rstest]
    #[case(ColumnSnapshot { text: "R".to_owned(), ..ColumnSnapshot::default() })]
    #[case(ColumnSnapshot { signal_strength: 0.5, ..ColumnSnapshot::default() })]
    #[case(ColumnSnapshot { difference: -0.5, ..ColumnSnapshot::default() })]
    #[case(ColumnSnapshot { case: Case::Figures, ..ColumnSnapshot::default() })]
    #[case(ColumnSnapshot { squelch_open: true, ..ColumnSnapshot::default() })]
    #[case(ColumnSnapshot { tones: ToneSet::from_center_and_shift(1_700.0, 850.0), ..ColumnSnapshot::default() })]
    fn anything_the_interface_draws_looks_different(#[case] column: ColumnSnapshot) {
        assert_ne!(
            Visible::of(&snapshot(vec![column])),
            Visible::of(&snapshot(vec![ColumnSnapshot::default()]))
        );
    }

    /// Which tone is being heard reads on the header, and mark and space are
    /// the same magnitude apart from the sign.
    #[test]
    fn the_two_tones_do_not_look_alike() {
        let mark = snapshot(vec![ColumnSnapshot {
            difference: 0.5,
            ..ColumnSnapshot::default()
        }]);
        let space = snapshot(vec![ColumnSnapshot {
            difference: -0.5,
            ..ColumnSnapshot::default()
        }]);
        assert_ne!(Visible::of(&mark), Visible::of(&space));
    }

    /// A scope frame is a picture rather than a reading: one that looks like
    /// the last one is still a new picture, and the window has to be woken
    /// for it.
    #[test]
    fn every_scope_frame_is_worth_a_frame_of_the_interface() {
        let frame = |sequence| RxSnapshot {
            scope: Some(ScopeFrame {
                sequence,
                ..ScopeFrame::default()
            }),
            ..RxSnapshot::default()
        };
        assert_ne!(Visible::of(&frame(1)), Visible::of(&frame(2)));
        assert_ne!(Visible::of(&frame(1)), Visible::of(&RxSnapshot::default()));
    }

    /// The trace is read as one line, so pairs that arrived between two draws
    /// belong to it in the order they were tapped.
    #[test]
    fn uncollected_pairs_stay_in_front_of_the_ones_that_follow_them() {
        let frame = |mark: f64| RxSnapshot {
            scope: Some(ScopeFrame {
                points: vec![ChannelLevels { mark, space: 0.0 }],
                sequence: 1,
                ..ScopeFrame::default()
            }),
            ..RxSnapshot::default()
        };
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(frame(1.0));
        mailbox.publish(frame(2.0));

        let points = mailbox.take().unwrap().scope.unwrap().points;
        assert_eq!(points.iter().map(|pair| pair.mark).collect::<Vec<_>>(), [1.0, 2.0]);
    }

    /// An interface that stopped drawing is one whose window stopped being
    /// looked at, and what it wants when it comes back is the signal now.
    #[test]
    fn a_trace_nobody_collected_does_not_grow_without_end() {
        let mailbox = Mailbox::new(Waker::default());
        for _ in 0..4 {
            mailbox.publish(RxSnapshot {
                scope: Some(ScopeFrame {
                    points: vec![ChannelLevels::default(); SCOPE_POINTS_LIMIT],
                    sequence: 1,
                    ..ScopeFrame::default()
                }),
                ..RxSnapshot::default()
            });
        }
        assert_eq!(mailbox.take().unwrap().scope.unwrap().points.len(), SCOPE_POINTS_LIMIT);
    }

    #[test]
    fn an_empty_snapshot_looks_like_what_the_interface_starts_with() {
        assert_eq!(Visible::of(&RxSnapshot::default()), Visible::default());
    }

    #[test]
    fn the_settings_the_panel_produces_reach_the_receiver() {
        let settings = WorkerSettings {
            tones: ToneSet::from_center_and_shift(1_700.0, 850.0),
            baud: 75.0,
            reverse: true,
            afc: false,
            squelch: None,
            unshift_on_space: false,
            atc: true,
        };
        let config = settings.rx_config();
        assert_eq!(config.tones, settings.tones);
        assert_eq!(config.framing.baud.bits_per_second(), 75.0);
        assert!(config.reverse);
        assert!(config.afc.is_none());
        assert!(config.squelch_threshold.is_none());
        assert!(!config.unshift_on_space);
        assert!(config.atc.is_some());
    }

    /// The controls are what a settings change travels through, and a worker
    /// that read a half-written set would build a receiver from neither.
    #[test]
    fn the_settings_round_trip_through_the_controls() {
        let controls = Controls::new(WorkerSettings::default());
        let wanted = WorkerSettings {
            baud: 50.0,
            afc: false,
            ..WorkerSettings::default()
        };
        controls.apply(wanted);
        assert_eq!(controls.settings(), wanted);
    }
}
