use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::{self, JoinHandle},
};

use grayline_audio::CaptureReader;
use grayline_wefax::{Format, Ioc, LinesPerMinute, WefaxBand, rx::RxState};

use crate::{error::AppError, worker::Waker};

mod session;

use session::run;

/// Columns of the strip, and where they belong in it.
///
/// The interface draws the raster rotated, so one decoded line is one column.
/// Only the columns that changed are published: a chart runs to megabytes, and
/// handing the whole of it over thirty times a second would cost more than
/// decoding it does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StripUpdate {
    /// Pixels in one line, which is the strip's height.
    pub width: usize,
    /// The line the first column belongs to.
    pub first_line: usize,
    /// Line-major gray levels, `width` of them per column.
    pub gray: Vec<u8>,
    /// Whether every column before `first_line` changed as well.
    ///
    /// A slant or phase correction moves lines already drawn, so the columns
    /// behind the newest ones are no longer what the interface was given.
    pub replaces_all: bool,
}

impl StripUpdate {
    pub fn lines(&self) -> usize {
        self.gray.len().checked_div(self.width).unwrap_or_default()
    }

    fn end(&self) -> usize {
        self.first_line + self.lines()
    }

    /// Appends `later` when it continues from where this one ends.
    ///
    /// Returns whether it did. Two updates that do not meet cannot be joined,
    /// which only happens across a correction, and a correction publishes
    /// everything anyway.
    fn extend(&mut self, later: &Self) -> bool {
        if later.replaces_all || later.width != self.width || later.first_line != self.end() {
            return false;
        }
        self.gray.extend_from_slice(&later.gray);
        true
    }
}

/// What the receiver is currently doing, as the interface says it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RxProgress {
    /// No signal has been identified, and no start was asked for.
    #[default]
    Idle,
    /// A start tone is being held.
    Starting,
    /// The phasing signal is being folded.
    Phasing,
    /// Lines are being drawn.
    Imaging { lines: usize },
    /// A stop tone ended the transmission.
    Complete { lines: usize },
    /// Reception ended for some other reason.
    Stopped { lines: usize },
}

impl RxProgress {
    fn of(state: RxState) -> Self {
        match state {
            RxState::Idle => Self::Idle,
            RxState::Starting { .. } => Self::Starting,
            RxState::Phasing => Self::Phasing,
            RxState::Imaging { lines } => Self::Imaging { lines },
            RxState::Complete { lines } => Self::Complete { lines },
            RxState::Stopped { lines, .. } => Self::Stopped { lines },
        }
    }

    pub const fn lines(self) -> usize {
        match self {
            Self::Idle | Self::Starting | Self::Phasing => 0,
            Self::Imaging { lines } | Self::Complete { lines } | Self::Stopped { lines } => lines,
        }
    }

    /// Whether a picture is being drawn right now.
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Phasing | Self::Imaging { .. })
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Idle => "state-idle",
            Self::Starting => "state-starting",
            Self::Phasing => "state-phasing",
            Self::Imaging { .. } => "state-imaging",
            Self::Complete { .. } => "state-complete",
            Self::Stopped { .. } => "state-stopped",
        }
    }
}

/// A chart the receiver finished, offered for saving.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chart {
    pub format: Format,
    pub width: usize,
    pub height: usize,
    pub gray: Vec<u8>,
}

/// One observation of the receive pipeline.
#[derive(Clone, Debug, Default)]
pub struct RxSnapshot {
    pub progress: RxProgress,
    pub format: Option<Format>,
    pub strip: Option<StripUpdate>,
    pub chart: Option<Chart>,
    pub signal_level: f32,
    /// How strongly each framing tone is present, for the tuning readout.
    pub apt: [f32; 3],
    pub samples_per_line: Option<f64>,
    pub dropped_samples: u64,
    pub error: Option<AppError>,
}

/// The parts of a snapshot the interface actually draws.
///
/// The worker publishes on every block of audio it reads, and most of those
/// say what the one before said. Comparing this instead of the whole snapshot
/// is what keeps the interface asleep between the observations worth showing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Visible {
    progress: RxProgress,
    format: Option<Format>,
    has_strip: bool,
    has_chart: bool,
    has_error: bool,
    dropped_samples: u64,
    signal_level: u8,
    apt: [u8; 3],
}

impl Visible {
    fn of(snapshot: &RxSnapshot) -> Self {
        Self {
            progress: snapshot.progress,
            format: snapshot.format,
            has_strip: snapshot.strip.is_some(),
            has_chart: snapshot.chart.is_some(),
            has_error: snapshot.error.is_some(),
            dropped_samples: snapshot.dropped_samples,
            signal_level: quantize(snapshot.signal_level),
            apt: snapshot.apt.map(quantize),
        }
    }
}

/// Rounds a reading to the steps a meter can be seen to move in.
fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 64.0) as u8
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
fn merge(snapshot: &mut RxSnapshot, previous: RxSnapshot) {
    if snapshot.error.is_none() {
        snapshot.error = previous.error;
    }
    if snapshot.chart.is_none() {
        snapshot.chart = previous.chart;
    }
    match (previous.strip, snapshot.strip.take()) {
        (Some(mut earlier), Some(later)) => {
            snapshot.strip = Some(if earlier.extend(&later) { earlier } else { later });
        }
        (earlier, later) => snapshot.strip = later.or(earlier),
    }
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
/// Everything here is read by the worker on its next block of audio, which is
/// arriving continuously while a device is open.
#[derive(Debug, Default)]
pub struct Controls {
    pub(super) auto_start: AtomicBool,
    pub(super) auto_stop: AtomicBool,
    pub(super) infer_lines_per_minute: AtomicBool,
    pub(super) slant_tracking: AtomicBool,
    pub(super) inverted: AtomicBool,
    pub(super) narrow_shift: AtomicBool,
    /// Requests that the reception in progress be abandoned.
    pub(super) reset: AtomicBool,
    /// Requests that a reception begin here, without a start tone.
    pub(super) manual_start: AtomicBool,
    /// Requests that the reception in progress be ended and kept.
    pub(super) manual_stop: AtomicBool,
    /// Sideways nudges the operator has asked for, in pixels.
    pub(super) phase_shift: Mutex<i64>,
    /// Line-rate corrections the operator has asked for, in parts per million.
    pub(super) slant_ppm: Mutex<f64>,
    /// The geometry a manual start uses, packed as `(ioc index, lines)`.
    pub(super) ioc: AtomicU32,
    pub(super) lines_per_minute: AtomicU32,
}

impl Controls {
    fn new(settings: &WorkerSettings) -> Self {
        let controls = Self::default();
        controls.apply(settings);
        controls
    }

    fn apply(&self, settings: &WorkerSettings) {
        self.auto_start.store(settings.auto_start, Ordering::Relaxed);
        self.auto_stop.store(settings.auto_stop, Ordering::Relaxed);
        self.infer_lines_per_minute
            .store(settings.infer_lines_per_minute, Ordering::Relaxed);
        self.slant_tracking.store(settings.slant_tracking, Ordering::Relaxed);
        self.inverted.store(settings.inverted, Ordering::Relaxed);
        self.narrow_shift.store(settings.narrow_shift, Ordering::Relaxed);
        self.ioc.store(settings.format.ioc.index(), Ordering::Relaxed);
        self.lines_per_minute
            .store(settings.format.lines_per_minute.as_lpm(), Ordering::Relaxed);
    }

    pub(super) fn format(&self) -> Format {
        let index = self.ioc.load(Ordering::Relaxed);
        let lines = self.lines_per_minute.load(Ordering::Relaxed);
        Format {
            ioc: Ioc::ALL
                .into_iter()
                .find(|ioc| ioc.index() == index)
                .unwrap_or(Ioc::Ioc576),
            lines_per_minute: LinesPerMinute::ALL
                .into_iter()
                .find(|rate| rate.as_lpm() == lines)
                .unwrap_or(LinesPerMinute::L120),
        }
    }

    pub(super) fn band(&self) -> WefaxBand {
        if self.narrow_shift.load(Ordering::Relaxed) {
            WefaxBand::NARROW
        } else {
            WefaxBand::WIDE
        }
    }

    pub(super) fn take_phase_shift(&self) -> i64 {
        let mut shift = self.phase_shift.lock().unwrap_or_else(PoisonError::into_inner);
        core::mem::replace(&mut shift, 0)
    }

    pub(super) fn take_slant_ppm(&self) -> f64 {
        let mut ppm = self.slant_ppm.lock().unwrap_or_else(PoisonError::into_inner);
        core::mem::replace(&mut ppm, 0.0)
    }
}

/// What a worker is started with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkerSettings {
    pub format: Format,
    pub auto_start: bool,
    pub auto_stop: bool,
    pub infer_lines_per_minute: bool,
    pub slant_tracking: bool,
    pub inverted: bool,
    pub narrow_shift: bool,
}

impl RxWorker {
    pub fn spawn(reader: CaptureReader, settings: WorkerSettings, waker: Waker) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let controls = Arc::new(Controls::new(&settings));
        let mailbox = Arc::new(Mailbox::new(waker));
        let join = {
            let stop = Arc::clone(&stop);
            let controls = Arc::clone(&controls);
            let mailbox = Arc::clone(&mailbox);
            thread::Builder::new()
                .name("grayline-wefax-receive".to_owned())
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

    pub fn settings(&self, settings: &WorkerSettings) {
        self.controls.apply(settings);
    }

    /// Abandons the reception in progress and searches again.
    pub fn request_reset(&self) {
        self.controls.reset.store(true, Ordering::Relaxed);
    }

    /// Begins a reception here, without waiting for a start tone.
    pub fn request_start(&self) {
        self.controls.manual_start.store(true, Ordering::Relaxed);
    }

    /// Ends the reception in progress, keeping what was drawn.
    pub fn request_stop(&self) {
        self.controls.manual_stop.store(true, Ordering::Relaxed);
    }

    /// Nudges the picture sideways by `pixels`, negative for left.
    pub fn request_phase_shift(&self, pixels: i64) {
        let mut shift = self.controls.phase_shift.lock().unwrap_or_else(PoisonError::into_inner);
        *shift += pixels;
    }

    /// Corrects the line rate by `ppm`, negative for a shorter line.
    pub fn request_slant_ppm(&self, ppm: f64) {
        let mut wanted = self.controls.slant_ppm.lock().unwrap_or_else(PoisonError::into_inner);
        *wanted += ppm;
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

    fn update(first_line: usize, lines: usize, replaces_all: bool) -> StripUpdate {
        StripUpdate {
            width: 4,
            first_line,
            gray: vec![first_line as u8; lines * 4],
            replaces_all,
        }
    }

    #[test]
    fn contiguous_columns_are_joined_rather_than_dropped() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(RxSnapshot {
            strip: Some(update(0, 2, true)),
            ..RxSnapshot::default()
        });
        mailbox.publish(RxSnapshot {
            strip: Some(update(2, 3, false)),
            ..RxSnapshot::default()
        });

        let strip = mailbox.take().unwrap().strip.unwrap();
        assert_eq!(strip.first_line, 0);
        assert_eq!(strip.lines(), 5);
        assert!(strip.replaces_all, "the earlier update's own claim has to survive");
    }

    /// A correction rewrites the columns already published, so what it carries
    /// replaces whatever was waiting rather than being appended to it.
    #[test]
    fn a_redraw_replaces_what_was_waiting() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(RxSnapshot {
            strip: Some(update(0, 2, false)),
            ..RxSnapshot::default()
        });
        mailbox.publish(RxSnapshot {
            strip: Some(update(0, 9, true)),
            ..RxSnapshot::default()
        });

        let strip = mailbox.take().unwrap().strip.unwrap();
        assert_eq!(strip.lines(), 9);
        assert!(strip.replaces_all);
    }

    #[test]
    fn a_snapshot_without_columns_keeps_the_ones_waiting() {
        let mailbox = Mailbox::new(Waker::default());
        mailbox.publish(RxSnapshot {
            strip: Some(update(0, 2, true)),
            ..RxSnapshot::default()
        });
        mailbox.publish(RxSnapshot::default());
        assert_eq!(mailbox.take().unwrap().strip.unwrap().lines(), 2);
    }

    #[test]
    fn an_uncollected_chart_survives_a_newer_snapshot() {
        let mailbox = Mailbox::new(Waker::default());
        let chart = Chart {
            format: Format::MARINE,
            width: 2,
            height: 1,
            gray: vec![1, 2],
        };
        mailbox.publish(RxSnapshot {
            chart: Some(chart.clone()),
            ..RxSnapshot::default()
        });
        mailbox.publish(RxSnapshot::default());
        assert_eq!(mailbox.take().unwrap().chart, Some(chart));
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
        let earlier = RxSnapshot {
            signal_level: 0.200,
            ..RxSnapshot::default()
        };
        let later = RxSnapshot {
            signal_level: 0.202,
            ..RxSnapshot::default()
        };
        assert_eq!(Visible::of(&earlier), Visible::of(&later));
    }

    #[rstest]
    #[case(RxSnapshot { progress: RxProgress::Phasing, ..RxSnapshot::default() })]
    #[case(RxSnapshot { format: Some(Format::MARINE), ..RxSnapshot::default() })]
    #[case(RxSnapshot { signal_level: 0.5, ..RxSnapshot::default() })]
    #[case(RxSnapshot { apt: [0.9, 0.0, 0.0], ..RxSnapshot::default() })]
    #[case(RxSnapshot { dropped_samples: 1, ..RxSnapshot::default() })]
    #[case(RxSnapshot { error: Some(AppError::CaptureRestartFailed), ..RxSnapshot::default() })]
    fn anything_the_interface_draws_looks_different(#[case] snapshot: RxSnapshot) {
        assert_ne!(Visible::of(&snapshot), Visible::of(&RxSnapshot::default()));
    }

    #[test]
    fn an_empty_snapshot_looks_like_what_the_interface_starts_with() {
        assert_eq!(Visible::of(&RxSnapshot::default()), Visible::default());
    }

    #[rstest]
    #[case(RxState::Idle, RxProgress::Idle)]
    #[case(RxState::Phasing, RxProgress::Phasing)]
    #[case(RxState::Imaging { lines: 7 }, RxProgress::Imaging { lines: 7 })]
    #[case(RxState::Complete { lines: 9 }, RxProgress::Complete { lines: 9 })]
    fn decoder_states_map_to_what_the_interface_says(#[case] state: RxState, #[case] expected: RxProgress) {
        assert_eq!(RxProgress::of(state), expected);
        assert_eq!(RxProgress::of(state).lines(), expected.lines());
    }

    #[test]
    fn only_a_live_reception_is_active() {
        assert!(RxProgress::Phasing.is_active());
        assert!(RxProgress::Imaging { lines: 1 }.is_active());
        assert!(!RxProgress::Idle.is_active());
        assert!(!RxProgress::Complete { lines: 1 }.is_active());
    }

    #[test]
    fn the_stored_geometry_survives_a_round_trip_through_the_controls() {
        let settings = WorkerSettings {
            format: Format {
                ioc: Ioc::Ioc288,
                lines_per_minute: LinesPerMinute::L240,
            },
            auto_start: false,
            auto_stop: false,
            infer_lines_per_minute: false,
            slant_tracking: false,
            inverted: true,
            narrow_shift: true,
        };
        let controls = Controls::new(&settings);
        assert_eq!(controls.format(), settings.format);
        assert_eq!(controls.band(), WefaxBand::NARROW);
    }

    #[test]
    fn a_phase_shift_accumulates_until_it_is_taken() {
        let controls = Controls::default();
        *controls.phase_shift.lock().unwrap() += 5;
        *controls.phase_shift.lock().unwrap() -= 2;
        assert_eq!(controls.take_phase_shift(), 3);
        assert_eq!(controls.take_phase_shift(), 0);
    }

    /// Two presses before the worker looks are one correction, not one press
    /// lost.
    #[test]
    fn a_line_rate_correction_accumulates_until_it_is_taken() {
        let controls = Controls::default();
        *controls.slant_ppm.lock().unwrap() += 50.0;
        *controls.slant_ppm.lock().unwrap() -= 5.0;
        assert_eq!(controls.take_slant_ppm(), 45.0);
        assert_eq!(controls.take_slant_ppm(), 0.0);
    }
}
