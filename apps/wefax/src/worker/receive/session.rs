use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use grayline_audio::CaptureReader;
use grayline_wefax::{Format, ReceivePipeline, RxConfig, rx::StopReason};

use crate::{
    error::AppError,
    worker::receive::{Chart, Controls, Mailbox, RxProgress, RxSnapshot, StripUpdate},
};

/// Samples taken from the capture queue in one read.
const READ_SAMPLES: usize = 4_096;
/// How long to wait when the queue is empty.
const IDLE_POLL: Duration = Duration::from_millis(2);
/// The shortest interval between two published strips.
///
/// A line takes at least a quarter of a second to arrive, so this only bounds
/// the readouts; it exists so a burst of queued audio cannot publish faster
/// than the interface can draw.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

/// Decodes from `reader` until asked to stop.
pub(super) fn run(mut reader: CaptureReader, mailbox: &Mailbox, stop: &AtomicBool, controls: &Controls) {
    let sample_rate_hz = reader.sample_rate_hz();
    let mut session = match Session::new(sample_rate_hz, controls) {
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
    // would otherwise never be shown: a recording ends, and a decoder that
    // finished a chart on its final samples would appear to have stopped part
    // way through it.
    let mut unpublished = false;

    while !stop.load(Ordering::Relaxed) {
        // A correction the operator asked for changes the picture without any
        // audio arriving, which is the whole of what happens once a reception
        // has ended. It has to be published on its own account, or the chart
        // on screen would keep the geometry the reception left it with.
        let (changed, mut error) = session.obey(controls);
        unpublished |= changed;

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
        // A capture overrun leaves a hole in the timeline, and a decoder that
        // was handed the samples either side of it would place every line
        // after it wrongly. The reception is restarted instead.
        if reading.is_discontinuous() && session.restart(controls).is_err() {
            error = Some(AppError::CaptureRestartFailed);
        }
        if error.is_none() {
            error = session.process(&pcm[..reading.count]);
        }

        unpublished = true;
        let due = published.elapsed() >= PUBLISH_INTERVAL;
        if due || error.is_some() || session.has_chart() {
            unpublished = false;
            published = Instant::now();
            mailbox.publish(session.snapshot(reader.dropped_samples(), error));
        }
    }
}

struct Session {
    pipeline: ReceivePipeline,
    sample_rate_hz: u32,
    config: RxConfig,
    /// Lines the interface has been given columns for.
    published_lines: usize,
    /// The raster revision those columns were taken at.
    ///
    /// The decoder advances this once per decoded line and once per
    /// correction, so a revision that ran ahead of the line count is how a
    /// correction is noticed without the decoder having to announce one.
    published_revision: u64,
    chart: Option<Chart>,
    saved: bool,
}

impl Session {
    fn new(sample_rate_hz: u32, controls: &Controls) -> Result<Self, AppError> {
        let config = configure(controls);
        Ok(Self {
            pipeline: ReceivePipeline::new(sample_rate_hz, config)?,
            sample_rate_hz,
            config,
            published_lines: 0,
            published_revision: 0,
            chart: None,
            saved: false,
        })
    }

    fn restart(&mut self, controls: &Controls) -> Result<(), AppError> {
        let config = configure(controls);
        self.pipeline = ReceivePipeline::new(self.sample_rate_hz, config)?;
        self.config = config;
        self.published_lines = 0;
        self.published_revision = 0;
        self.saved = false;
        Ok(())
    }

    /// Acts on whatever the interface asked for since the last block.
    ///
    /// Returns whether anything the interface draws changed, and what went
    /// wrong if something did.
    fn obey(&mut self, controls: &Controls) -> (bool, Option<AppError>) {
        let wanted = configure(controls);
        // The band is fixed when a demodulator is built, so changing the shift
        // is a new reception rather than a setting.
        if wanted.band != self.config.band
            && let Err(error) = self.restart(controls)
        {
            return (true, Some(error));
        }
        self.config = wanted;
        let decoder = self.pipeline.decoder_mut();
        decoder.set_auto_start(wanted.auto_start);
        decoder.set_auto_stop(wanted.auto_stop);
        decoder.set_inverted(wanted.inverted);

        let mut changed = false;
        if controls.reset.swap(false, Ordering::Relaxed) {
            // Ended before it is kept, so what a reset leaves behind is
            // refitted on the same terms as a reception that finished.
            self.pipeline.decoder_mut().stop(StopReason::Manual);
            self.finish_chart();
            if let Err(error) = self.restart(controls) {
                return (true, Some(error));
            }
            return (true, None);
        }
        if controls.manual_stop.swap(false, Ordering::Relaxed) {
            self.pipeline.decoder_mut().stop(StopReason::Manual);
            self.finish_chart();
            changed = true;
        }
        if controls.manual_start.swap(false, Ordering::Relaxed) {
            self.finish_chart();
            if let Err(error) = self.restart(controls) {
                return (true, Some(error));
            }
            let sample = self.pipeline.demodulator().next_sample();
            let format = controls.format();
            if let Err(error) = self.pipeline.decoder_mut().start_manually(format, sample) {
                return (true, Some(error.into()));
            }
            changed = true;
        }
        let shift = controls.take_phase_shift();
        if shift != 0 {
            changed |= self.pipeline.decoder_mut().shift_phase(shift).is_ok();
        }
        let ppm = controls.take_slant_ppm();
        if ppm != 0.0
            && let Some(width) = self.pipeline.decoder().format().map(Format::pixels_per_line)
        {
            // A line that is a millionth longer displaces the next one by a
            // millionth of its own width, so parts per million become pixels
            // per line by multiplying through the line's own length.
            let drift = ppm * 1.0e-6 * width as f64;
            changed |= self.pipeline.decoder_mut().adjust_slant(drift).is_ok();
        }
        (changed, None)
    }

    fn process(&mut self, samples: &[f32]) -> Option<AppError> {
        let sample = self.pipeline.demodulator().next_sample();
        match self.pipeline.process(samples, |_| {}) {
            Ok(()) => {
                if self.pipeline.decoder().state().is_terminal() {
                    self.finish_chart();
                }
                None
            }
            Err(source) => Some(AppError::Decode { sample, source }),
        }
    }

    fn has_chart(&self) -> bool {
        self.chart.is_some()
    }

    /// Keeps the picture a finished reception left, once.
    ///
    /// The picture is refitted first. A live correction can only see the lean
    /// its own short window shows, and the whole of a finished chart is a far
    /// longer baseline to measure against; the phase the pivots walked away
    /// from goes back at the same time. What is saved is therefore what the
    /// operator is left looking at.
    fn finish_chart(&mut self) {
        if self.saved {
            return;
        }
        if self.config.slant_tracking {
            let _ = self.pipeline.decoder_mut().refine();
        }
        let decoder = self.pipeline.decoder();
        let (Some(format), Some(raster)) = (decoder.format(), decoder.raster()) else {
            return;
        };
        if raster.height() == 0 {
            return;
        }
        self.saved = true;
        self.chart = Some(Chart {
            format,
            width: raster.width(),
            height: raster.height(),
            gray: raster.pixels().to_vec(),
        });
    }

    fn snapshot(&mut self, dropped_samples: u64, error: Option<AppError>) -> RxSnapshot {
        let strip = self.strip();

        let decoder = self.pipeline.decoder();
        let state = decoder.state();
        let format = decoder.format();
        let samples_per_line = decoder.samples_per_line();
        let strengths = self.pipeline.demodulator().apt_strengths();
        RxSnapshot {
            progress: RxProgress::of(state),
            format,
            strip,
            chart: self.chart.take(),
            signal_level: self.pipeline.demodulator().signal_level(),
            apt: [strengths.start_576, strengths.start_288, strengths.stop],
            samples_per_line,
            dropped_samples,
            error,
        }
    }

    /// Returns the columns the interface has not been given yet.
    fn strip(&mut self) -> Option<StripUpdate> {
        let decoder = self.pipeline.decoder();
        let raster = decoder.raster()?;
        let lines = raster.height();
        let revision = decoder.raster_revision();
        if revision == self.published_revision && lines == self.published_lines {
            return None;
        }
        // A revision that moved further than the line count did means a
        // correction rewrote the lines already drawn, so they go out again.
        let corrected = revision - self.published_revision != (lines - self.published_lines) as u64;
        let first_line = if corrected { 0 } else { self.published_lines };
        let gray = raster.rows(first_line..lines)?.to_vec();
        self.published_lines = lines;
        self.published_revision = revision;
        Some(StripUpdate {
            width: raster.width(),
            first_line,
            gray,
            replaces_all: corrected,
        })
    }
}

fn configure(controls: &Controls) -> RxConfig {
    let format = controls.format();
    RxConfig {
        band: controls.band(),
        format: Some(format),
        auto_start: controls.auto_start.load(Ordering::Relaxed),
        auto_stop: controls.auto_stop.load(Ordering::Relaxed),
        infer_lines_per_minute: controls.infer_lines_per_minute.load(Ordering::Relaxed),
        slant_tracking: controls.slant_tracking.load(Ordering::Relaxed),
        inverted: controls.inverted.load(Ordering::Relaxed),
        ..RxConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grayline_wefax::{Ioc, LinesPerMinute, rx::RxState};

    use crate::worker::receive::WorkerSettings;

    fn controls(settings: WorkerSettings) -> Controls {
        let controls = Controls::default();
        controls.apply(&settings);
        controls
    }

    fn settings() -> WorkerSettings {
        WorkerSettings {
            format: Format::MARINE,
            auto_start: true,
            auto_stop: true,
            infer_lines_per_minute: true,
            slant_tracking: true,
            inverted: false,
            narrow_shift: false,
        }
    }

    #[test]
    fn the_configuration_follows_the_controls() {
        let controls = controls(WorkerSettings {
            format: Format {
                ioc: Ioc::Ioc288,
                lines_per_minute: LinesPerMinute::L90,
            },
            auto_start: false,
            narrow_shift: true,
            inverted: true,
            ..settings()
        });
        let config = configure(&controls);
        assert_eq!(config.format.unwrap().ioc, Ioc::Ioc288);
        assert_eq!(config.format.unwrap().lines_per_minute, LinesPerMinute::L90);
        assert!(!config.auto_start);
        assert!(config.inverted);
        assert_eq!(config.band, grayline_wefax::WefaxBand::NARROW);
    }

    #[test]
    fn a_session_with_no_raster_has_no_columns_to_publish() {
        let controls = controls(settings());
        let mut session = Session::new(11_025, &controls).unwrap();
        assert!(session.strip().is_none());
        assert_eq!(session.snapshot(0, None).progress, RxProgress::of(RxState::Idle));
    }

    #[test]
    fn a_manual_start_puts_the_session_into_phasing() {
        let controls = controls(WorkerSettings {
            auto_start: false,
            ..settings()
        });
        let mut session = Session::new(11_025, &controls).unwrap();
        controls.manual_start.store(true, Ordering::Relaxed);

        assert!(session.obey(&controls).1.is_none());
        assert_eq!(session.pipeline.decoder().state(), RxState::Phasing);
    }

    #[test]
    fn changing_the_shift_starts_the_reception_over() {
        let controls = controls(settings());
        let mut session = Session::new(11_025, &controls).unwrap();
        assert_eq!(session.config.band, grayline_wefax::WefaxBand::WIDE);

        controls.narrow_shift.store(true, Ordering::Relaxed);
        assert!(session.obey(&controls).1.is_none());
        assert_eq!(session.config.band, grayline_wefax::WefaxBand::NARROW);
        assert_eq!(session.pipeline.demodulator().band(), grayline_wefax::WefaxBand::NARROW);
    }
}
