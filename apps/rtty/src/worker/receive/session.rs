use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use grayline_audio::CaptureReader;
use grayline_rtty::{ReceivePipeline, RxEvent, code::Case, rx::MonitorConfig};

use crate::{
    error::AppError,
    worker::receive::{
        ColumnSnapshot, Controls, DecodePath, Mailbox, RxSnapshot, ScopeFrame, WorkerSettings, spectrum::Spectrum,
    },
};

/// Samples taken from the capture queue in one read.
const READ_SAMPLES: usize = 4_096;
/// How long to wait when the queue is empty.
const IDLE_POLL: Duration = Duration::from_millis(2);
/// The shortest interval between two published snapshots.
///
/// A character takes about 160 ms at 45.45 baud, so this only bounds the
/// readouts; it exists so a burst of queued audio cannot publish faster than
/// the interface can draw.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

/// Channel pairs the monitor tap keeps per second, for the scope.
///
/// About ninety to a bit at 45.45 baud, which is more than the width of the
/// picture they are drawn into. MMTTY interpolates its own channels up before
/// drawing them (`docs/memo/mmtty/dsp.md`); these run at the capture rate, so
/// the same resolution is had by throwing fewer of them away.
const MONITOR_POINT_RATE_HZ: u32 = 4_000;

/// About a second of pairs.
///
/// The tap drops its oldest when it fills, so this is how long the interface
/// may look away for without the trace it comes back to having a gap in it.
const MONITOR_CAPACITY: usize = 4_096;

/// How many pairs are dropped for each one kept, at `sample_rate_hz`.
fn monitor_decimation(sample_rate_hz: u32) -> usize {
    sample_rate_hz.div_ceil(MONITOR_POINT_RATE_HZ).max(1) as usize
}

/// Decodes from `reader` until asked to stop.
pub(super) fn run(mut reader: CaptureReader, mailbox: &Mailbox, stop: &AtomicBool, controls: &Controls) {
    let sample_rate_hz = reader.sample_rate_hz();
    let mut session = match Session::new(sample_rate_hz, controls.settings()) {
        Ok(session) => session,
        Err(error) => {
            mailbox.publish(RxSnapshot {
                error: Some(error),
                ..RxSnapshot::default()
            });
            return;
        }
    };
    let mut pcm = vec![0.0_f32; READ_SAMPLES];
    let mut published = Instant::now() - PUBLISH_INTERVAL;
    // Whether anything has been decoded that the interface has not been told
    // about. Publishing is throttled, so the last block before the audio stops
    // would otherwise never be shown: a recording ends, and the characters on
    // its final samples would never be printed.
    let mut unpublished = false;

    while !stop.load(Ordering::Relaxed) {
        let mut error = session.obey(controls);

        let reading = reader.read(&mut pcm);
        if reading.count == 0 {
            if unpublished || error.is_some() {
                unpublished = false;
                published = Instant::now();
                mailbox.publish(session.snapshot(reader.dropped_samples(), error));
            }
            thread::sleep(IDLE_POLL);
            continue;
        }
        // A capture overrun leaves a hole in the timeline. A character being
        // framed across it would be decoded from the bits either side, so the
        // paths are started over rather than carried across the gap.
        if reading.is_discontinuous() && session.restart().is_err() {
            error = Some(AppError::CaptureRestartFailed);
        }
        if error.is_none() {
            error = session.process(&pcm[..reading.count]);
        }

        unpublished = true;
        let due = published.elapsed() >= PUBLISH_INTERVAL;
        if due || error.is_some() {
            unpublished = false;
            published = Instant::now();
            mailbox.publish(session.snapshot(reader.dropped_samples(), error));
        }
    }
}

struct Column {
    path: DecodePath,
    pipeline: ReceivePipeline,
    text: String,
    case: Case,
    squelch_open: bool,
    /// Samples handed to this pipeline, which is where a failure reported by
    /// it happened: the pipeline counts its own position but keeps it.
    processed: u64,
}

impl Column {
    fn new(path: DecodePath, sample_rate_hz: u32, settings: WorkerSettings) -> Result<Self, AppError> {
        let config = settings.rx_config();
        Ok(Self {
            path,
            pipeline: ReceivePipeline::new(sample_rate_hz, config)?,
            text: String::new(),
            case: Case::default(),
            // The core opens with the squelch clamped open when there is no
            // threshold, and the header reads whether bits are being framed.
            squelch_open: config.squelch_threshold.is_none(),
            processed: 0,
        })
    }
}

/// Every decode path running on one capture stream.
///
/// They are all fed the same PCM, so a column that decodes what another one
/// missed is a comparison the operator can make rather than a second session
/// they have to run.
struct Session {
    columns: Vec<Column>,
    sample_rate_hz: u32,
    settings: WorkerSettings,
    /// Whether the scope window is open, and what it is drawn from.
    ///
    /// Neither of these is part of the settings: the tap is a setting on a
    /// pipeline that is already running, and the transform is this worker's
    /// own rather than the receiver's.
    scope: bool,
    spectrum: Option<Spectrum>,
    sequence: u64,
}

impl Session {
    fn new(sample_rate_hz: u32, settings: WorkerSettings) -> Result<Self, AppError> {
        let columns = DecodePath::ALL
            .into_iter()
            .map(|path| Column::new(path, sample_rate_hz, settings))
            .collect::<Result<Vec<_>, AppError>>()?;
        Ok(Self {
            columns,
            sample_rate_hz,
            settings,
            scope: false,
            spectrum: None,
            sequence: 0,
        })
    }

    /// Builds every path again, keeping nothing they were part way through.
    ///
    /// A receiver is built from its configuration and cannot be retuned in
    /// place, so this is what a settings change costs: the front-end filters
    /// settle again in a few milliseconds, and the text already decoded lives
    /// in the interface rather than here.
    fn restart(&mut self) -> Result<(), AppError> {
        for column in &mut self.columns {
            let rebuilt = Column::new(column.path, self.sample_rate_hz, self.settings)?;
            column.pipeline = rebuilt.pipeline;
            column.case = rebuilt.case;
            column.squelch_open = rebuilt.squelch_open;
            // The count is not reset: it says where in the capture stream a
            // failure happened, and the stream did not start over.
        }
        // The rebuilt pipeline is a new one, and a tap is something a
        // pipeline carries rather than something it is built from.
        self.tap();
        Ok(())
    }

    /// Opens or closes the monitor tap, following the scope window.
    ///
    /// Only the first path is tapped: every column is fed the same audio, and
    /// the scope is a picture of what is arriving rather than of what one
    /// demodulator made of it.
    fn tap(&mut self) {
        let config = self.scope.then(|| MonitorConfig {
            decimation: monitor_decimation(self.sample_rate_hz),
            capacity: MONITOR_CAPACITY,
        });
        if let Some(column) = self.columns.first_mut() {
            column.pipeline.set_monitor(config);
        }
    }

    fn obey(&mut self, controls: &Controls) -> Option<AppError> {
        let scope = controls.scope.load(Ordering::Relaxed);
        if scope != self.scope {
            self.scope = scope;
            self.tap();
            // A window that closed and opened again is watching what is
            // arriving now, so the transform starts on what it hears from
            // here rather than on what was in the buffer when it left.
            self.spectrum = scope.then(|| Spectrum::new(self.sample_rate_hz)).flatten();
        }

        let wanted = controls.settings();
        let reset = controls.reset.swap(false, Ordering::Relaxed);
        if wanted == self.settings && !reset {
            return None;
        }
        self.settings = wanted;
        self.restart().err()
    }

    fn process(&mut self, samples: &[f32]) -> Option<AppError> {
        if let Some(spectrum) = self.spectrum.as_mut() {
            spectrum.observe(samples);
        }
        let mut failure = None;
        for column in &mut self.columns {
            let sample = column.processed;
            let text = &mut column.text;
            let case = &mut column.case;
            let squelch_open = &mut column.squelch_open;
            let outcome = column.pipeline.process(samples, |event| match event {
                RxEvent::Character { character, .. } => text.push(*character),
                RxEvent::CaseChanged { case: shifted, .. } => *case = *shifted,
                RxEvent::SquelchChanged { open, .. } => *squelch_open = *open,
                _ => {}
            });
            column.processed += samples.len() as u64;
            if let Err(source) = outcome {
                failure = Some(AppError::Decode { sample, source });
            }
        }
        failure
    }

    fn scope_frame(&mut self) -> Option<ScopeFrame> {
        if !self.scope {
            return None;
        }
        self.sequence += 1;
        Some(ScopeFrame {
            points: self
                .columns
                .first_mut()
                .map(|column| column.pipeline.drain_monitor().collect())
                .unwrap_or_default(),
            spectrum: self.spectrum.as_mut().map(Spectrum::magnitudes).unwrap_or_default(),
            bin_hz: self.spectrum.as_ref().map_or(0.0, Spectrum::bin_hz),
            sequence: self.sequence,
        })
    }

    fn snapshot(&mut self, dropped_samples: u64, error: Option<AppError>) -> RxSnapshot {
        RxSnapshot {
            scope: self.scope_frame(),
            columns: self
                .columns
                .iter_mut()
                .map(|column| ColumnSnapshot {
                    path: column.path,
                    text: core::mem::take(&mut column.text),
                    signal_strength: column.pipeline.signal_strength() as f32,
                    difference: column.pipeline.channels().difference() as f32,
                    case: column.case,
                    tones: column.pipeline.tones(),
                    squelch_open: column.squelch_open,
                })
                .collect(),
            dropped_samples,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::f64::consts::TAU;

    use grayline_rtty::{ToneSet, Transmitter, TxConfig, encode_text};

    use super::*;

    const RATE: u32 = 11_025;

    fn settings() -> WorkerSettings {
        WorkerSettings::default()
    }

    fn controls(settings: WorkerSettings) -> Controls {
        let controls = Controls::default();
        controls.apply(settings);
        controls
    }

    fn transmission(text: &str) -> Vec<f32> {
        let config = TxConfig::default();
        let codes = encode_text(text, &config).unwrap();
        let mut transmitter = Transmitter::new(codes.into_iter(), RATE, config).unwrap();
        let mut pcm = Vec::new();
        let mut block = [0.0_f32; 1_024];
        loop {
            let produced = transmitter.process(&mut block).unwrap();
            pcm.extend_from_slice(&block[..produced]);
            if produced == 0 {
                break;
            }
        }
        pcm
    }

    fn tone(frequency_hz: f64, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| (TAU * frequency_hz * index as f64 / f64::from(RATE)).sin() as f32)
            .collect()
    }

    #[test]
    fn a_session_starts_one_column_per_decode_path() {
        let session = Session::new(RATE, settings()).unwrap();
        assert_eq!(session.columns.len(), DecodePath::ALL.len());
    }

    #[test]
    fn a_transmission_reaches_the_snapshot_as_text() {
        let mut session = Session::new(RATE, settings()).unwrap();
        assert!(session.process(&transmission("CQ DE JL1HIS\r\n")).is_none());

        let snapshot = session.snapshot(0, None);
        assert_eq!(snapshot.columns[0].text, "CQ DE JL1HIS\r\n");
        assert!(session.snapshot(0, None).columns[0].text.is_empty());
    }

    #[test]
    fn the_reading_follows_the_tone_being_heard() {
        let mut session = Session::new(RATE, settings()).unwrap();
        session.process(&tone(ToneSet::AFSK_170.mark_hz, RATE as usize / 4));
        assert!(session.snapshot(0, None).columns[0].difference > 0.0);

        session.process(&tone(ToneSet::AFSK_170.space_hz, RATE as usize / 4));
        assert!(session.snapshot(0, None).columns[0].difference < 0.0);
    }

    #[test]
    fn a_settings_change_rebuilds_the_paths_on_the_new_pair() {
        let controls = controls(settings());
        let mut session = Session::new(RATE, settings()).unwrap();
        let wanted = WorkerSettings {
            tones: ToneSet::from_center_and_shift(1_700.0, 850.0),
            ..settings()
        };
        controls.apply(wanted);

        assert!(session.obey(&controls).is_none());
        assert_eq!(session.columns[0].pipeline.tones(), wanted.tones);
    }

    /// Rebuilding on every block would restart the front end 12 times a
    /// second and never let the filters settle.
    #[test]
    fn settings_that_did_not_change_leave_the_paths_alone() {
        let controls = controls(settings());
        let mut session = Session::new(RATE, settings()).unwrap();
        session.process(&transmission("RY"));
        session.obey(&controls);
        assert!(
            !session.columns[0].text.is_empty(),
            "an untouched setting should not have thrown the decoded text away"
        );
    }

    #[test]
    fn a_reset_starts_the_paths_over() {
        let controls = controls(settings());
        let mut session = Session::new(RATE, settings()).unwrap();
        session.columns[0].case = Case::Figures;
        controls.reset.store(true, Ordering::Relaxed);

        assert!(session.obey(&controls).is_none());
        assert_eq!(session.columns[0].case, Case::default());
    }

    #[test]
    fn a_closed_scope_is_published_nothing() {
        let mut session = Session::new(RATE, settings()).unwrap();
        session.process(&transmission("RY"));
        assert!(session.snapshot(0, None).scope.is_none());
    }

    #[test]
    fn an_open_scope_is_given_the_pairs_the_comparator_compared() {
        let controls = controls(settings());
        controls.scope.store(true, Ordering::Relaxed);
        let mut session = Session::new(RATE, settings()).unwrap();
        assert!(session.obey(&controls).is_none());
        session.process(&tone(ToneSet::AFSK_170.mark_hz, RATE as usize / 2));

        let frame = session.snapshot(0, None).scope.expect("an open scope is fed");
        assert!(!frame.points.is_empty());
        // The second half, because the front end's filters settle over the
        // first few milliseconds of any tone and read neither channel yet.
        let settled = &frame.points[frame.points.len() / 2..];
        assert!(
            settled.iter().all(|pair| pair.mark > pair.space),
            "the mark tone was the one playing"
        );
        assert!(!frame.spectrum.is_empty(), "half a second fills the transform");
        assert!(frame.bin_hz > 0.0);
    }

    #[test]
    fn every_frame_says_it_is_a_new_one() {
        let controls = controls(settings());
        controls.scope.store(true, Ordering::Relaxed);
        let mut session = Session::new(RATE, settings()).unwrap();
        session.obey(&controls);

        let first = session.snapshot(0, None).scope.unwrap().sequence;
        assert_eq!(session.snapshot(0, None).scope.unwrap().sequence, first + 1);
    }

    /// A settings change rebuilds every pipeline, and a pipeline is built
    /// without a tap: the window would go blank for the rest of the reception.
    #[test]
    fn a_rebuilt_path_is_tapped_again() {
        let controls = controls(settings());
        controls.scope.store(true, Ordering::Relaxed);
        let mut session = Session::new(RATE, settings()).unwrap();
        session.obey(&controls);

        controls.apply(WorkerSettings {
            baud: 50.0,
            ..settings()
        });
        assert!(session.obey(&controls).is_none());
        session.process(&tone(ToneSet::AFSK_170.mark_hz, RATE as usize / 4));
        assert!(!session.snapshot(0, None).scope.unwrap().points.is_empty());
    }

    #[test]
    fn closing_the_scope_stops_the_tap() {
        let controls = controls(settings());
        controls.scope.store(true, Ordering::Relaxed);
        let mut session = Session::new(RATE, settings()).unwrap();
        session.obey(&controls);
        session.process(&tone(ToneSet::AFSK_170.mark_hz, RATE as usize / 4));

        controls.scope.store(false, Ordering::Relaxed);
        assert!(session.obey(&controls).is_none());
        assert!(session.snapshot(0, None).scope.is_none());
        assert!(!session.columns[0].pipeline.tones().mark_hz.is_nan());
    }

    #[test]
    fn the_tap_keeps_about_the_same_rate_whatever_the_device_runs_at() {
        for rate in [8_000, 11_025, 22_050, 44_100, 48_000, 96_000] {
            let kept = f64::from(rate) / monitor_decimation(rate) as f64;
            assert!(
                kept <= f64::from(MONITOR_POINT_RATE_HZ),
                "{rate} Hz keeps {kept} pairs a second"
            );
            assert!(
                kept >= f64::from(MONITOR_POINT_RATE_HZ) / 2.0,
                "{rate} Hz keeps only {kept} pairs a second"
            );
        }
    }

    #[test]
    fn a_capture_rate_the_decoder_cannot_use_is_reported() {
        assert!(Session::new(4_000, settings()).is_err());
    }
}
