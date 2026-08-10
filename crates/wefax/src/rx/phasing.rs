use alloc::{vec, vec::Vec};

use crate::{
    error::WefaxError,
    format::{Format, Ioc, LinesPerMinute, PHASING_WHITE_FRACTION},
};

/// Contrast a folded pulse has to reach, in normalized deviation units.
const MIN_CONTRAST: f64 = 0.5;
/// Lines folded into one segment of the rate estimate.
const SEGMENT_LINES: usize = 8;
/// Segments needed before a rate is estimated from their drift.
const MIN_SEGMENTS: usize = 3;
/// Normalized deviation below which a sample counts as black.
const DARK_LEVEL: f64 = -0.5;
/// Length of one window of the picture-started test, in seconds.
const DARK_WINDOW_SECONDS: f64 = 1.0;
/// Share of a window that has to be black for it to still be phasing.
const DARK_FRACTION: f64 = 0.80;
/// Consecutive windows below that share before the picture is declared begun.
const DARK_WINDOWS: usize = 2;

/// Where the phasing signal put the start of a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhasingResult {
    /// The geometry the fold settled on.
    pub format: Format,
    /// Absolute sample position a line begins at.
    pub epoch_samples: f64,
    /// Line length in samples, corrected by the drift across the interval.
    pub samples_per_line: f64,
    /// Where the pulse sat within the assumed line, in pixels.
    pub offset_pixels: f64,
    /// How far the pulse stood above the rest of the line.
    pub contrast: f32,
    /// How many lines the fold covered.
    pub lines: usize,
}

/// A circular fold of the phasing signal at one candidate line rate.
///
/// The line period is known before the signal arrives, so the pulse is found
/// by folding rather than by hunting: every line lands its white pulse on the
/// same bins and everything else averages away. The fold's origin is
/// arbitrary because it is circular, which is why the start tone's own
/// detection latency costs nothing here.
#[derive(Clone, Debug)]
struct Candidate {
    lines_per_minute: LinesPerMinute,
    samples_per_line: f64,
    sum: Vec<f64>,
    count: Vec<u32>,
    segment_sum: Vec<f64>,
    segment_count: Vec<u32>,
    segment_index: usize,
    observations: Vec<(f64, f64)>,
}

impl Candidate {
    fn new(lines_per_minute: LinesPerMinute, sample_rate_hz: u32, bins: usize) -> Self {
        Self {
            lines_per_minute,
            samples_per_line: lines_per_minute.samples_per_line(sample_rate_hz),
            sum: vec![0.0; bins],
            count: vec![0; bins],
            segment_sum: vec![0.0; bins],
            segment_count: vec![0; bins],
            segment_index: 0,
            observations: Vec::new(),
        }
    }

    fn process(&mut self, normalized: f64, elapsed: f64) {
        let bins = self.sum.len();
        let position = elapsed / self.samples_per_line;
        let line = position as usize;
        let bin = (((position - line as f64) * bins as f64) as usize).min(bins - 1);
        self.sum[bin] += normalized;
        self.count[bin] += 1;
        self.segment_sum[bin] += normalized;
        self.segment_count[bin] += 1;

        let segment = line / SEGMENT_LINES;
        if segment > self.segment_index {
            self.close_segment();
            self.segment_index = segment;
        }
    }

    /// Records where the pulse sat in the segment just completed.
    fn close_segment(&mut self) {
        if let Some(found) = locate(&self.segment_sum, &self.segment_count)
            && found.contrast >= MIN_CONTRAST
        {
            let center = (self.segment_index * SEGMENT_LINES) as f64 + SEGMENT_LINES as f64 * 0.5;
            self.observations.push((center, found.start as f64));
        }
        self.segment_sum.fill(0.0);
        self.segment_count.fill(0);
    }

    fn lines(&self, elapsed: f64) -> usize {
        (elapsed / self.samples_per_line) as usize
    }

    /// Returns the drift of the pulse in bins per line, if enough segments
    /// agreed on where it was.
    fn drift(&self) -> Option<f64> {
        if self.observations.len() < MIN_SEGMENTS {
            return None;
        }
        let bins = self.sum.len() as f64;
        let mut unwrapped = Vec::with_capacity(self.observations.len());
        let mut offset = 0.0;
        let mut previous = self.observations[0].1;
        for (line, bin) in &self.observations {
            // A pulse that walked off one end of the line reappears at the
            // other; the shorter of the two readings is the real one.
            let step = bin - previous;
            if step > bins * 0.5 {
                offset -= bins;
            } else if step < -bins * 0.5 {
                offset += bins;
            }
            previous = *bin;
            unwrapped.push((*line, bin + offset));
        }
        let count = unwrapped.len() as f64;
        let mean_line = unwrapped.iter().map(|(line, _)| line).sum::<f64>() / count;
        let mean_bin = unwrapped.iter().map(|(_, bin)| bin).sum::<f64>() / count;
        let mut covariance = 0.0;
        let mut variance = 0.0;
        for (line, bin) in &unwrapped {
            covariance += (line - mean_line) * (bin - mean_bin);
            variance += (line - mean_line) * (line - mean_line);
        }
        (variance > 0.0).then(|| covariance / variance)
    }
}

/// A pulse located in a folded line.
#[derive(Clone, Copy, Debug)]
struct Located {
    start: usize,
    contrast: f64,
}

/// Finds the window of pulse length whose mean stands highest.
///
/// The window's length is known, so sliding it either way trades pulse bins
/// for background bins and its extremum therefore sits on the pulse whatever
/// surrounds it. Reading an edge off a threshold instead would need a
/// threshold, and would move with the background.
fn locate(sum: &[f64], count: &[u32]) -> Option<Located> {
    let bins = sum.len();
    let width = ((bins as f64 * PHASING_WHITE_FRACTION) as usize).max(1);
    if bins <= width || count.iter().all(|value| *value == 0) {
        return None;
    }
    let mean = |bin: usize| {
        let observed = count[bin];
        if observed == 0 {
            0.0
        } else {
            sum[bin] / f64::from(observed)
        }
    };
    let total = (0..bins).map(mean).sum::<f64>();
    let mut window = (0..width).map(mean).sum::<f64>();
    let (mut best_start, mut best_window) = (0, window);
    for start in 1..bins {
        window -= mean(start - 1);
        window += mean((start + width - 1) % bins);
        if window > best_window {
            best_window = window;
            best_start = start;
        }
    }
    let pulse = best_window / width as f64;
    let background = (total - best_window) / (bins - width) as f64;
    Some(Located {
        start: best_start,
        contrast: (pulse - background) * 0.5,
    })
}

/// Counts how much of the recent signal was black.
///
/// The phasing signal is black but for its pulse, so the picture beginning is
/// what ends it. Measuring over a fixed interval rather than over lines keeps
/// this independent of a line rate that may not be known yet.
#[derive(Clone, Debug)]
struct DarkWindow {
    window_samples: u64,
    seen: u64,
    dark: u64,
    below: usize,
}

impl DarkWindow {
    fn new(sample_rate_hz: u32) -> Self {
        Self {
            window_samples: (f64::from(sample_rate_hz) * DARK_WINDOW_SECONDS) as u64,
            seen: 0,
            dark: 0,
            below: 0,
        }
    }

    fn process(&mut self, normalized: f64) {
        self.seen += 1;
        if normalized <= DARK_LEVEL {
            self.dark += 1;
        }
        if self.seen >= self.window_samples {
            let fraction = self.dark as f64 / self.seen as f64;
            self.below = if fraction < DARK_FRACTION { self.below + 1 } else { 0 };
            self.seen = 0;
            self.dark = 0;
        }
    }

    fn picture_started(&self) -> bool {
        self.below >= DARK_WINDOWS
    }

    /// Returns whether any window has already stopped looking like phasing.
    fn left_the_phasing_signal(&self) -> bool {
        self.below > 0
    }
}

/// Folds the phasing signal to find where a line begins, and at what rate.
#[derive(Clone, Debug)]
pub(crate) struct PhasingDetector {
    ioc: Ioc,
    candidates: Vec<Candidate>,
    dark: DarkWindow,
    first_sample: u64,
    elapsed: f64,
}

impl PhasingDetector {
    pub(crate) fn new(
        sample_rate_hz: u32,
        ioc: Ioc,
        candidates: &[LinesPerMinute],
        first_sample: u64,
    ) -> Result<Self, WefaxError> {
        if candidates.is_empty() {
            return Err(WefaxError::NoLineRateCandidates);
        }
        let bins = ioc.pixels_per_line();
        Ok(Self {
            ioc,
            candidates: candidates
                .iter()
                .map(|rate| Candidate::new(*rate, sample_rate_hz, bins))
                .collect(),
            dark: DarkWindow::new(sample_rate_hz),
            first_sample,
            elapsed: 0.0,
        })
    }

    /// Folds one normalized deviation sample into every candidate.
    pub(crate) fn process(&mut self, normalized: f64, sample: u64) {
        self.elapsed = sample.saturating_sub(self.first_sample) as f64;
        self.dark.process(normalized);
        // Folding the picture in would wash the pulse out: a chart carries
        // white wherever it likes, and enough of it outweighs a pulse that
        // occupies a twentieth of the line. One window's worth gets in before
        // the picture is noticed, which is the price of noticing it at all.
        if self.dark.left_the_phasing_signal() {
            return;
        }
        for candidate in &mut self.candidates {
            candidate.process(normalized, self.elapsed);
        }
    }

    /// Returns whether the picture appears to have started.
    pub(crate) fn picture_started(&self) -> bool {
        self.dark.picture_started()
    }

    /// Returns how long the fold has been running, in samples.
    pub(crate) const fn elapsed(&self) -> f64 {
        self.elapsed
    }

    /// Returns the best reading the fold can offer, if one is confident.
    ///
    /// A candidate at the wrong rate smears the pulse across the whole line,
    /// so contrast both selects the rate and decides whether the pulse was
    /// found at all.
    pub(crate) fn resolve(&self) -> Option<PhasingResult> {
        let bins = self.ioc.pixels_per_line();
        let mut best: Option<(f64, PhasingResult)> = None;
        for candidate in &self.candidates {
            let Some(found) = locate(&candidate.sum, &candidate.count) else {
                continue;
            };
            if found.contrast < MIN_CONTRAST {
                continue;
            }
            let drift = candidate.drift().unwrap_or(0.0);
            let samples_per_line = candidate.samples_per_line + drift * candidate.samples_per_line / bins as f64;
            let offset_pixels = found.start as f64;
            let result = PhasingResult {
                format: Format {
                    ioc: self.ioc,
                    lines_per_minute: candidate.lines_per_minute,
                },
                // The pulse sits `offset_pixels` into every folded line, so a
                // line begins that far after the fold's own origin.
                epoch_samples: self.first_sample as f64 + offset_pixels / bins as f64 * candidate.samples_per_line,
                samples_per_line,
                offset_pixels,
                contrast: found.contrast as f32,
                lines: candidate.lines(self.elapsed),
            };
            if best.as_ref().is_none_or(|(score, _)| found.contrast > *score) {
                best = Some((found.contrast, result));
            }
        }
        best.map(|(_, result)| result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::WefaxBand;
    use rstest::rstest;

    /// Folds a synthetic phasing signal and returns what the detector made of it.
    fn phase(
        rate: u32,
        format: Format,
        candidates: &[LinesPerMinute],
        seconds: f64,
        offset_fraction: f64,
        rate_error_ppm: f64,
    ) -> Option<PhasingResult> {
        let mut detector = PhasingDetector::new(rate, format.ioc, candidates, 0).unwrap();
        let line = format.samples_per_line(rate) * (1.0 + rate_error_ppm * 1.0e-6);
        let count = (f64::from(rate) * seconds) as u64;
        for sample in 0..count {
            let position = (sample as f64 / line).fract();
            let start = offset_fraction.fract();
            let end = start + PHASING_WHITE_FRACTION;
            let white = (position >= start && position < end) || (end > 1.0 && position < end - 1.0);
            detector.process(if white { 1.0 } else { -1.0 }, sample);
        }
        detector.resolve()
    }

    #[rstest]
    fn the_pulse_is_found_where_it_was_placed(
        #[values(11_025, 48_000)] rate: u32,
        #[values(Ioc::Ioc576, Ioc::Ioc288)] ioc: Ioc,
        #[values(0.0, 0.25, 0.5, 0.9)] offset_fraction: f64,
    ) {
        let format = Format {
            ioc,
            lines_per_minute: LinesPerMinute::L120,
        };
        let result =
            phase(rate, format, &[LinesPerMinute::L120], 12.0, offset_fraction, 0.0).expect("a pulse was found");
        let expected = offset_fraction * ioc.pixels_per_line() as f64;
        assert!(
            (result.offset_pixels - expected).abs() <= 1.0,
            "expected {expected} px, found {} px",
            result.offset_pixels
        );
        assert!(result.contrast > 0.5, "contrast was {}", result.contrast);
    }

    #[rstest]
    fn the_line_rate_is_inferred_from_the_sharpest_fold(
        #[values(
            LinesPerMinute::L60,
            LinesPerMinute::L90,
            LinesPerMinute::L100,
            LinesPerMinute::L120,
            LinesPerMinute::L180,
            LinesPerMinute::L240
        )]
        lines_per_minute: LinesPerMinute,
    ) {
        let format = Format {
            ioc: Ioc::Ioc576,
            lines_per_minute,
        };
        let result = phase(11_025, format, &LinesPerMinute::ALL, 20.0, 0.1, 0.0).expect("a pulse was found");
        assert_eq!(result.format.lines_per_minute, lines_per_minute);
    }

    #[rstest]
    #[case(0.0)]
    #[case(500.0)]
    #[case(-500.0)]
    fn the_drift_of_the_pulse_estimates_the_rate(#[case] rate_error_ppm: f64) {
        let rate = 11_025;
        let format = Format::MARINE;
        let result =
            phase(rate, format, &[LinesPerMinute::L120], 30.0, 0.1, rate_error_ppm).expect("a pulse was found");
        let truth = format.samples_per_line(rate) * (1.0 + rate_error_ppm * 1.0e-6);
        let error_ppm = (result.samples_per_line - truth) / truth * 1.0e6;
        assert!(
            error_ppm.abs() < 50.0,
            "{rate_error_ppm} ppm was estimated as {} ppm off",
            error_ppm
        );
    }

    #[test]
    fn a_picture_is_not_mistaken_for_a_phasing_signal() {
        let rate = 11_025;
        let mut detector = PhasingDetector::new(rate, Ioc::Ioc576, &LinesPerMinute::ALL, 0).unwrap();
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        for sample in 0..rate as u64 * 10 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let value = f64::from((state >> 40) as u32) / f64::from(1_u32 << 24) * 2.0 - 1.0;
            detector.process(value, sample);
        }
        assert!(detector.resolve().is_none());
        assert!(detector.picture_started());
    }

    #[test]
    fn the_picture_beginning_ends_the_phasing_signal() {
        let rate = 11_025;
        let format = Format::MARINE;
        let mut detector = PhasingDetector::new(rate, format.ioc, &[format.lines_per_minute], 0).unwrap();
        let line = format.samples_per_line(rate);
        let phasing = rate as u64 * 6;
        for sample in 0..phasing {
            let position = (sample as f64 / line).fract();
            detector.process(if position < PHASING_WHITE_FRACTION { 1.0 } else { -1.0 }, sample);
        }
        assert!(!detector.picture_started());
        assert!(detector.resolve().is_some());

        // A mid-gray picture is nowhere near black.
        for sample in phasing..phasing + rate as u64 * 3 {
            detector.process(0.0, sample);
        }
        assert!(detector.picture_started());
    }

    #[test]
    fn the_epoch_places_the_pulse_at_the_start_of_a_line() {
        let rate = 11_025;
        let format = Format::MARINE;
        let result = phase(rate, format, &[format.lines_per_minute], 12.0, 0.25, 0.0).unwrap();
        let line = format.samples_per_line(rate);
        // The pulse began a quarter of a line into the fold, so line zero
        // begins a quarter of a line after the fold's own origin.
        assert!((result.epoch_samples - line * 0.25).abs() < line / 500.0);
    }

    #[test]
    fn a_shift_outside_the_band_still_folds() {
        let rate = 11_025;
        let format = Format::MARINE;
        let band = WefaxBand::NARROW;
        let mut detector = PhasingDetector::new(rate, format.ioc, &[format.lines_per_minute], 0).unwrap();
        let line = format.samples_per_line(rate);
        for sample in 0..rate as u64 * 10 {
            let position = (sample as f64 / line).fract();
            let frequency = if position < PHASING_WHITE_FRACTION {
                band.white_hz()
            } else {
                band.black_hz()
            };
            detector.process(band.normalized(frequency), sample);
        }
        assert!(detector.resolve().is_some());
    }

    #[test]
    fn an_empty_candidate_list_is_rejected() {
        assert_eq!(
            PhasingDetector::new(11_025, Ioc::Ioc576, &[], 0).unwrap_err(),
            WefaxError::NoLineRateCandidates
        );
    }
}
