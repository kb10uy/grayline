use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use grayline_audio::CaptureReader;
use grayline_rtty::{ReceivePipeline, RxEvent, code::Case};

use crate::{
    error::AppError,
    worker::receive::{ColumnSnapshot, Controls, DecodePath, Mailbox, RxSnapshot, WorkerSettings},
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

/// One decode path, and what it has decoded since the last snapshot.
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
        Ok(())
    }

    /// Acts on whatever the interface asked for since the last block.
    fn obey(&mut self, controls: &Controls) -> Option<AppError> {
        let wanted = controls.settings();
        let reset = controls.reset.swap(false, Ordering::Relaxed);
        if wanted == self.settings && !reset {
            return None;
        }
        self.settings = wanted;
        self.restart().err()
    }

    fn process(&mut self, samples: &[f32]) -> Option<AppError> {
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

    fn snapshot(&mut self, dropped_samples: u64, error: Option<AppError>) -> RxSnapshot {
        RxSnapshot {
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

    /// A transmission of `text`, produced by the crate's own transmitter.
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
        // Taken rather than copied: the interface appends what it is given,
        // so a character handed over twice would be printed twice.
        assert!(session.snapshot(0, None).columns[0].text.is_empty());
    }

    /// The header says which tone is being heard, and it has to be right or
    /// the operator tunes away from a signal that was arriving.
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

    /// A reset is how the operator says the receiver has lost its place, so
    /// it has to act even when nothing about the settings changed.
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
    fn a_capture_rate_the_decoder_cannot_use_is_reported() {
        assert!(Session::new(4_000, settings()).is_err());
    }
}
