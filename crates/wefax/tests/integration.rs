//! End-to-end reception of synthesized WEFAX audio.
//!
//! There is no transmit encoder in this crate and no recorded audio in this
//! repository, so a transmission is built here out of a phase-continuous
//! oscillator, exactly as the SSTV front end's own integration test does.

use std::f64::consts::TAU;

use grayline_wefax::{
    Format, GrayRaster, Ioc, LinesPerMinute, ReceivePipeline, RxConfig, WefaxBand, WefaxError,
    format::{APT_STOP_HZ, APT_TONE_SECONDS, PHASING_WHITE_FRACTION},
    rx::{RxEvent, RxState, StopReason},
};

/// Builds one transmission's audio, keeping oscillator phase across it.
struct Transmitter {
    sample_rate_hz: u32,
    band: WefaxBand,
    phase: f64,
    samples: Vec<f32>,
}

impl Transmitter {
    fn new(sample_rate_hz: u32, band: WefaxBand) -> Self {
        Self {
            sample_rate_hz,
            band,
            phase: 0.0,
            samples: Vec::new(),
        }
    }

    fn emit(&mut self, frequency_hz: f64) {
        self.samples.push(self.phase.sin() as f32);
        self.phase = (self.phase + TAU * frequency_hz / f64::from(self.sample_rate_hz)).rem_euclid(TAU);
    }

    fn count(&self, seconds: f64) -> usize {
        (f64::from(self.sample_rate_hz) * seconds) as usize
    }

    /// The picture keyed black and white at `alternation_hz`: an APT tone.
    fn apt(&mut self, alternation_hz: f64, seconds: f64) {
        let mut keying = 0.0_f64;
        for _ in 0..self.count(seconds) {
            let level = if keying < 0.5 { 0 } else { u8::MAX };
            self.emit(self.band.frequency_hz(level));
            keying += alternation_hz / f64::from(self.sample_rate_hz);
            if keying >= 1.0 {
                keying -= 1.0;
            }
        }
    }

    /// Black lines carrying a white pulse: the phasing signal.
    fn phasing(&mut self, format: Format, seconds: f64, offset_fraction: f64, rate_error_ppm: f64) {
        let line = self.line_samples(format, rate_error_ppm);
        for sample in 0..self.count(seconds) {
            let position = (sample as f64 / line).fract();
            let from = position - offset_fraction;
            let within = from.rem_euclid(1.0) < PHASING_WHITE_FRACTION;
            self.emit(self.band.frequency_hz(if within { u8::MAX } else { 0 }));
        }
    }

    /// A picture whose every line carries `columns`.
    fn picture(&mut self, format: Format, columns: &[u8], lines: usize, rate_error_ppm: f64) {
        let line = self.line_samples(format, rate_error_ppm);
        let width = format.pixels_per_line();
        for sample in 0..(line * lines as f64) as usize {
            let position = (sample as f64 / line).fract();
            let pixel = ((position * width as f64) as usize).min(width - 1);
            self.emit(self.band.frequency_hz(columns[pixel * columns.len() / width]));
        }
    }

    fn line_samples(&self, format: Format, rate_error_ppm: f64) -> f64 {
        format.samples_per_line(self.sample_rate_hz) * (1.0 + rate_error_ppm * 1.0e-6)
    }
}

/// A complete transmission: start tone, phasing, picture, stop tone.
fn transmission(rate: u32, format: Format, columns: &[u8], lines: usize) -> Vec<f32> {
    let mut transmitter = Transmitter::new(rate, WefaxBand::WIDE);
    transmitter.apt(format.ioc.apt_start_hz(), APT_TONE_SECONDS);
    transmitter.phasing(format, 6.0, 0.0, 0.0);
    transmitter.picture(format, columns, lines, 0.0);
    transmitter.apt(APT_STOP_HZ, APT_TONE_SECONDS);
    transmitter.samples
}

fn run(rate: u32, samples: &[f32], packet: usize, config: RxConfig) -> (ReceivePipeline, Vec<RxEvent>) {
    let mut pipeline = ReceivePipeline::new(rate, config).unwrap();
    let mut events = Vec::new();
    for block in samples.chunks(packet) {
        pipeline.process(block, |event| events.push(*event)).unwrap();
    }
    (pipeline, events)
}

fn config() -> RxConfig {
    RxConfig {
        phasing_min_seconds: 1.0,
        ..RxConfig::default()
    }
}

/// A bar pattern: alternating black and white bands across the line.
fn bars(count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| if index.is_multiple_of(2) { 0 } else { u8::MAX })
        .collect()
}

#[test]
fn a_complete_transmission_is_received_from_its_own_framing() {
    let rate = 11_025;
    let format = Format::MARINE;
    let columns = bars(8);
    let samples = transmission(rate, format, &columns, 12);
    let (pipeline, events) = run(rate, &samples, 1_024, config());

    assert!(
        events
            .iter()
            .any(|event| matches!(event, RxEvent::AptStartDetected { ioc: Ioc::Ioc576, .. })),
        "the start tone was not detected"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RxEvent::FormatSelected { format: found } if *found == format)),
        "the geometry was not selected: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RxEvent::PhasingAcquired { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RxEvent::AptStopDetected { .. }))
    );

    let outcome = pipeline.finish();
    assert!(matches!(outcome.state, RxState::Complete { lines } if lines > 4));
    assert_eq!(outcome.format, Some(format));

    let raster = outcome.raster.expect("a picture was drawn");
    assert_eq!(raster.width(), format.pixels_per_line());
    assert_gray_bars(&raster, &columns);
}

#[test]
fn the_line_rate_is_inferred_when_only_the_start_tone_announces_anything() {
    let rate = 11_025;
    let format = Format {
        ioc: Ioc::Ioc576,
        lines_per_minute: LinesPerMinute::L180,
    };
    let columns = bars(8);
    let samples = transmission(rate, format, &columns, 16);
    let (pipeline, _) = run(rate, &samples, 1_024, config());
    let outcome = pipeline.finish();
    assert_eq!(outcome.format, Some(format));
}

#[test]
fn a_narrow_index_is_received_from_its_own_start_tone() {
    let rate = 11_025;
    let format = Format {
        ioc: Ioc::Ioc288,
        lines_per_minute: LinesPerMinute::L120,
    };
    let columns = bars(8);
    let samples = transmission(rate, format, &columns, 12);
    let (pipeline, _) = run(rate, &samples, 1_024, config());
    let outcome = pipeline.finish();
    assert_eq!(outcome.format, Some(format));
    assert_eq!(outcome.raster.unwrap().width(), 905);
}

#[test]
fn packet_size_does_not_change_the_picture() {
    let rate = 11_025;
    let format = Format::MARINE;
    let columns = bars(8);
    let samples = transmission(rate, format, &columns, 10);

    let reference = run(rate, &samples, 1_024, config()).0.finish();
    for packet in [73, 4_093] {
        let outcome = run(rate, &samples, packet, config()).0.finish();
        assert_eq!(outcome.state, reference.state, "packets of {packet}");
        assert_eq!(outcome.format, reference.format, "packets of {packet}");
        assert_eq!(outcome.raster, reference.raster, "packets of {packet}");
    }
}

#[test]
fn a_reception_runs_at_the_lowest_supported_capture_rate() {
    let rate = 8_000;
    let format = Format::MARINE;
    let columns = bars(8);
    let samples = transmission(rate, format, &columns, 10);
    let (pipeline, _) = run(rate, &samples, 1_024, config());
    let outcome = pipeline.finish();
    assert_eq!(outcome.format, Some(format));
    assert_gray_bars(&outcome.raster.unwrap(), &columns);
}

#[test]
fn a_reception_runs_at_the_capture_rate_a_sound_card_prefers() {
    let rate = 48_000;
    let format = Format::MARINE;
    let columns = bars(8);
    let samples = transmission(rate, format, &columns, 8);
    let (pipeline, _) = run(rate, &samples, 4_093, config());
    let outcome = pipeline.finish();
    assert_eq!(outcome.format, Some(format));
    assert_gray_bars(&outcome.raster.unwrap(), &columns);
}

#[test]
fn a_configured_geometry_skips_the_start_tone_entirely() {
    let rate = 11_025;
    let format = Format::MARINE;
    let columns = bars(8);
    let mut transmitter = Transmitter::new(rate, WefaxBand::WIDE);
    // Joined after the start tone, which is what tuning in late looks like.
    transmitter.phasing(format, 6.0, 0.3, 0.0);
    transmitter.picture(format, &columns, 12, 0.0);

    let config = RxConfig {
        auto_start: false,
        format: Some(format),
        infer_lines_per_minute: false,
        phasing_min_seconds: 1.0,
        ..RxConfig::default()
    };
    let (pipeline, _) = run(rate, &transmitter.samples, 1_024, config);
    let outcome = pipeline.finish();
    assert_eq!(outcome.format, Some(format));
    assert_gray_bars(&outcome.raster.unwrap(), &columns);
}

#[test]
fn a_pipeline_without_a_geometry_or_a_start_tone_is_refused() {
    let config = RxConfig {
        auto_start: false,
        ..RxConfig::default()
    };
    assert_eq!(
        ReceivePipeline::new(11_025, config).unwrap_err(),
        WefaxError::FormatNotSelected
    );
}

#[test]
fn a_stop_tone_leaves_the_reception_complete_and_further_audio_changes_nothing() {
    let rate = 11_025;
    let format = Format::MARINE;
    let columns = bars(8);
    let mut samples = transmission(rate, format, &columns, 10);
    let mut extra = Transmitter::new(rate, WefaxBand::WIDE);
    extra.picture(format, &columns, 4, 0.0);
    samples.extend_from_slice(&extra.samples);

    let (pipeline, _) = run(rate, &samples, 1_024, config());
    let lines = pipeline.decoder().line_count();
    let outcome = pipeline.finish();
    assert_eq!(outcome.state, RxState::Complete { lines });
}

#[test]
fn the_line_limit_ends_a_transmission_that_will_not() {
    let rate = 11_025;
    let format = Format::MARINE;
    let columns = bars(8);
    let samples = transmission(rate, format, &columns, 20);
    let config = RxConfig {
        max_lines: Some(4),
        ..config()
    };
    let (pipeline, _) = run(rate, &samples, 1_024, config);
    let outcome = pipeline.finish();
    assert_eq!(
        outcome.state,
        RxState::Stopped {
            lines: 4,
            reason: StopReason::LineLimit
        }
    );
}

/// Checks that each band of the picture reads back at the level it was sent.
///
/// Read near the top of the raster. A stop tone has to be held before it is
/// trusted, so the last few lines of any reception are the tone itself rather
/// than the picture — which is what an operator sees in every other radiofax
/// program too.
fn assert_gray_bars(raster: &GrayRaster, columns: &[u8]) {
    assert!(raster.height() >= 3, "only {} lines decoded", raster.height());
    let width = raster.width();
    for line in 1..3 {
        let row = raster.row(line).unwrap();
        for (index, expected) in columns.iter().enumerate() {
            let pixel = index * width / columns.len() + width / columns.len() / 2;
            let found = row[pixel];
            assert!(
                found.abs_diff(*expected) <= 8,
                "on line {line}, band {index} was sent as {expected} and read as {found}"
            );
        }
    }
}
