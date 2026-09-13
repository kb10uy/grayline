use alloc::collections::VecDeque;

use crate::{
    error::WefaxError,
    format::{Format, Ioc, WefaxBand},
    image::GrayRaster,
    rx::{
        apt::AptEvent,
        clock::LineClock,
        config::{PhasingFallback, RxConfig},
        event::{Refinement, RxEvent, RxOutcome, RxProcessError, RxProcessResult, RxState, StopReason},
        input::DemodulatedBlock,
        phasing::{PhasingDetector, PhasingResult},
        slant::{self, SlantTracker},
    },
};

/// Share of a pixel's interval left untouched at each end.
///
/// Averaging the central five-eighths guards against the neighbouring pixels
/// bleeding in through the discriminator's own settling time, and is what the
/// SSTV decoder does for the same reason.
const PIXEL_GUARD: f64 = 0.187_5;
/// Lines kept behind the one being decoded, for a correction that moves back.
const RETAIN_LINES: usize = 2;
/// Shortest span a completed picture is fitted over, in lines.
const REFINE_FIRST_BASELINE: usize = 8;
/// Lines a completed picture needs before fitting one is worth anything.
const REFINE_MINIMUM_LINES: usize = REFINE_FIRST_BASELINE * 4;

/// A stateful, streaming WEFAX line decoder.
///
/// Input is the demodulated frequency stream with absolute positions, and
/// output is a raster that grows a line at a time. Nothing here creates
/// threads or holds a clock: a caller drives it with whatever packets its
/// source produces, and the result does not depend on how they were divided.
#[derive(Clone, Debug)]
pub struct WefaxDecoder {
    sample_rate_hz: u32,
    config: RxConfig,
    state: RxState,
    format: Option<Format>,
    ioc: Option<Ioc>,
    phasing: Option<PhasingDetector>,
    phasing_result: Option<PhasingResult>,
    clock: Option<LineClock>,
    raster: Option<GrayRaster>,
    revision: u64,
    next_line: usize,
    next_sample: Option<u64>,
    /// How far a slant correction has carried line zero from where the
    /// phasing signal, and then the operator, put it — in pixels, positive
    /// for leftwards.
    ///
    /// A correction pivots on the line being decoded so the rows around it
    /// stay where they are, which moves the top of the picture instead. The
    /// phasing signal is the only thing that ever knew where a line begins, so
    /// what it established is put back when the reception ends.
    origin_shift: f64,
    events: VecDeque<RxEvent>,
    window: SampleWindow,
    slant: SlantTracker,
}

impl WefaxDecoder {
    /// Creates a decoder with the default configuration.
    pub fn new(sample_rate_hz: u32) -> Result<Self, WefaxError> {
        Self::with_config(sample_rate_hz, RxConfig::default())
    }

    /// Creates a decoder for a capture rate and a configuration.
    pub fn with_config(sample_rate_hz: u32, config: RxConfig) -> Result<Self, WefaxError> {
        if sample_rate_hz < crate::rx::MINIMUM_SAMPLE_RATE_HZ {
            return Err(WefaxError::SampleRateTooLow(sample_rate_hz));
        }
        Ok(Self {
            sample_rate_hz,
            ioc: config.format.map(|format| format.ioc),
            format: config.format,
            config,
            state: RxState::Idle,
            phasing: None,
            phasing_result: None,
            clock: None,
            raster: None,
            revision: 0,
            next_line: 0,
            next_sample: None,
            origin_shift: 0.0,
            events: VecDeque::new(),
            window: SampleWindow::default(),
            slant: SlantTracker::default(),
        })
    }

    /// Returns the state the decoder is in.
    pub const fn state(&self) -> RxState {
        self.state
    }

    /// Returns the configuration decoding is running under.
    pub const fn config(&self) -> &RxConfig {
        &self.config
    }

    /// Returns the geometry decoding settled on, once both halves are known.
    pub const fn format(&self) -> Option<Format> {
        self.format
    }

    /// Returns the picture decoded so far, once there is one.
    pub const fn raster(&self) -> Option<&GrayRaster> {
        self.raster.as_ref()
    }

    /// Returns a counter that advances whenever the raster changes.
    ///
    /// A caller publishing rows to a display compares this against what it
    /// last drew, rather than cloning a raster that runs to megabytes.
    pub const fn raster_revision(&self) -> u64 {
        self.revision
    }

    /// Returns how many lines have been decoded.
    pub const fn line_count(&self) -> usize {
        self.next_line
    }

    /// Returns the line length decoding is currently using.
    pub fn samples_per_line(&self) -> Option<f64> {
        self.clock.map(|clock| clock.samples_per_line())
    }

    /// Returns what the phasing signal reported, once it has.
    pub const fn phasing(&self) -> Option<PhasingResult> {
        self.phasing_result
    }

    /// Takes the next decision the decoder made, oldest first.
    pub fn poll_event(&mut self) -> Option<RxEvent> {
        self.events.pop_front()
    }

    /// Acts on a framing tone the demodulator decided.
    ///
    /// The samples on either side of the tone belong to different states, so
    /// a caller hands over the block up to the tone, then this, then the rest
    /// of the block through [`DemodulatedBlock::from`].
    pub fn accept_apt(&mut self, event: AptEvent) -> Result<(), WefaxError> {
        match event {
            AptEvent::StartAccepted { ioc, sample } => {
                if self.state != RxState::Idle || !self.config.auto_start {
                    return Ok(());
                }
                self.ioc = Some(ioc);
                self.state = RxState::Starting { ioc };
                self.events.push_back(RxEvent::AptStartDetected { ioc, sample });
            }
            AptEvent::StartEnded { sample } => {
                if !matches!(self.state, RxState::Starting { .. }) {
                    return Ok(());
                }
                // The event names the tone's last sample, so the fold begins
                // on the one after it.
                self.begin_phasing(sample + 1)?;
            }
            AptEvent::StopAccepted { sample } => {
                if !matches!(self.state, RxState::Imaging { .. }) || !self.config.auto_stop {
                    return Ok(());
                }
                self.state = RxState::Complete { lines: self.next_line };
                self.events.push_back(RxEvent::AptStopDetected { sample });
            }
        }
        Ok(())
    }

    /// Begins a reception here, without waiting for a start tone.
    pub fn start_manually(&mut self, format: Format, sample: u64) -> Result<(), WefaxError> {
        if self.state.is_terminal() {
            return Err(WefaxError::AlreadyComplete);
        }
        self.format = Some(format);
        self.ioc = Some(format.ioc);
        self.config.format = Some(format);
        // Starting by hand re-anchors the stream, so the next block may begin
        // wherever the caller says the reception does.
        self.next_sample = None;
        self.begin_phasing(sample)
    }

    /// Ends the reception, keeping whatever was decoded.
    pub fn stop(&mut self, reason: StopReason) {
        if self.state.is_terminal() {
            return;
        }
        self.state = RxState::Stopped {
            lines: self.next_line,
            reason,
        };
        self.events.push_back(RxEvent::Stopped { reason });
    }

    /// Changes the shift the gray scale is read against.
    pub const fn set_band(&mut self, band: WefaxBand) {
        self.config.band = band;
    }

    /// Sets whether black and white are the other way round.
    pub const fn set_inverted(&mut self, inverted: bool) {
        self.config.inverted = inverted;
    }

    /// Sets whether a start tone may begin a reception.
    pub const fn set_auto_start(&mut self, enabled: bool) {
        self.config.auto_start = enabled;
    }

    /// Sets whether a stop tone may end one.
    pub const fn set_auto_stop(&mut self, enabled: bool) {
        self.config.auto_stop = enabled;
    }

    /// Moves the picture sideways, taking the rows already drawn with it.
    pub fn shift_phase(&mut self, pixels: i64) -> Result<(), WefaxError> {
        let (Some(clock), Some(raster), Some(format)) = (self.clock.as_mut(), self.raster.as_mut(), self.format) else {
            return Err(WefaxError::NotImaging);
        };
        raster.roll(pixels);
        clock.shift(pixels as f64 * format.samples_per_pixel(self.sample_rate_hz))?;
        self.revision += 1;
        self.events.push_back(RxEvent::PhaseAdjusted {
            line: self.next_line,
            displacement_pixels: pixels as f64,
        });
        Ok(())
    }

    /// Changes the line length by a slant in pixels per line, correcting the
    /// rows already drawn to match.
    ///
    /// While a reception is running the correction pivots on the line being
    /// decoded, so the rows around it keep the positions they were read at and
    /// the next line is read where the picture says it is. Once it has ended
    /// there is no next line, and the operator is working from the top of the
    /// chart: the pivot moves to the first line, so straightening the bottom
    /// leaves the end they aligned by hand where they put it.
    pub fn adjust_slant(&mut self, pixels_per_line: f64) -> Result<(), WefaxError> {
        let pivot = if self.state.is_terminal() {
            0
        } else {
            self.next_line.saturating_sub(1)
        };
        self.apply_slant(pixels_per_line, pivot)
    }

    /// Refits the whole of a finished picture, and puts its phase back.
    ///
    /// Two things a reception can only be told once it is over. The line rate
    /// is fitted across the whole height rather than across the window a live
    /// correction is bound to, which is what removes the lean the live
    /// tracker's own threshold leaves behind — twenty-seven parts per million
    /// is a hundred pixels of shear over twenty minutes. And the phase is
    /// returned to what the phasing signal established, undoing the sideways
    /// walk that pivoting each live correction on a different line caused.
    ///
    /// Returns what changed, or `None` when the picture said nothing usable.
    pub fn refine(&mut self) -> Result<Option<Refinement>, WefaxError> {
        if !self.state.is_terminal() {
            return Err(WefaxError::NotComplete);
        }
        let Some(before) = self.samples_per_line() else {
            return Ok(None);
        };
        let height = self.raster.as_ref().map_or(0, GrayRaster::height);
        if height == 0 {
            return Ok(None);
        }

        // Putting the phase back needs no fit, but fitting the rate needs
        // enough picture for a long span to mean anything.
        //
        // Each span is twice the last. A fit removes what it measured, so the
        // next span starts from a residual small enough to stay inside the
        // displacement the correlation searches over, however far the picture
        // leaned to begin with.
        let mut baseline = REFINE_FIRST_BASELINE;
        while height >= REFINE_MINIMUM_LINES && baseline * 2 <= height {
            let drift = self
                .raster
                .as_ref()
                .and_then(|raster| slant::whole_picture_drift(raster, baseline));
            if let Some(drift) = drift {
                self.apply_slant(drift, 0)?;
            }
            baseline *= 2;
        }

        // Only the walk the pivots caused is put back. A shift the operator
        // asked for is theirs, and undoing it would be the application
        // arguing with them.
        let displacement = -crate::image::rounded(self.origin_shift);
        self.origin_shift = 0.0;
        if displacement != 0.0 {
            self.shift_phase(displacement as i64)?;
        }
        let after = self.samples_per_line().unwrap_or(before);
        Ok(Some(Refinement {
            samples_per_line: after,
            error_ppm: (after - before) / before * 1.0e6,
            displacement_pixels: displacement,
        }))
    }

    fn apply_slant(&mut self, pixels_per_line: f64, pivot: usize) -> Result<(), WefaxError> {
        if !pixels_per_line.is_finite() {
            return Err(WefaxError::InvalidLineClock);
        }
        let (Some(clock), Some(raster), Some(format)) = (self.clock.as_mut(), self.raster.as_mut(), self.format) else {
            return Err(WefaxError::NotImaging);
        };
        let samples_per_pixel = format.samples_per_pixel(self.sample_rate_hz);
        let previous = clock.samples_per_line();
        clock.set_samples_per_line(previous + pixels_per_line * samples_per_pixel, pivot)?;
        raster.shear(pivot, pixels_per_line);
        // `shear` rolls row y left by `(y - pivot) * pixels_per_line`, so line
        // zero moves by whatever the pivot was not.
        self.origin_shift += -(pivot as f64) * pixels_per_line;
        self.revision += 1;
        self.events.push_back(RxEvent::SlantAdjusted {
            line: pivot,
            samples_per_line: clock.samples_per_line(),
            error_ppm: (clock.samples_per_line() - previous) / previous * 1.0e6,
        });
        Ok(())
    }

    /// Consumes a block, returning how much of it was taken.
    pub fn process(&mut self, block: DemodulatedBlock<'_>) -> Result<RxProcessResult, RxProcessError> {
        if self.state.is_terminal() {
            return Err(RxProcessError::new(0, WefaxError::AlreadyComplete));
        }
        if let Some(expected) = self.expected_sample()
            && block.first_sample() != expected
        {
            return Err(RxProcessError::new(
                0,
                WefaxError::DemodulatedGap {
                    expected,
                    actual: block.first_sample(),
                },
            ));
        }
        for (offset, frequency) in block.frequency_hz().iter().enumerate() {
            let sample = block.first_sample() + offset as u64;
            self.next_sample = Some(sample + 1);
            if let Err(error) = self.consume(f64::from(*frequency), sample) {
                return Err(RxProcessError::new(offset, error));
            }
            if self.state.is_terminal() {
                return Ok(RxProcessResult {
                    consumed: offset + 1,
                    state: self.state,
                });
            }
        }
        Ok(RxProcessResult {
            consumed: block.len(),
            state: self.state,
        })
    }

    /// Ends the reception and hands back everything it produced.
    pub fn finish(mut self) -> RxOutcome {
        if !self.state.is_terminal() {
            self.stop(StopReason::Manual);
        }
        RxOutcome {
            state: self.state,
            format: self.format,
            samples_per_line: self.clock.map(|clock| clock.samples_per_line()),
            phasing: self.phasing_result,
            raster: self.raster,
        }
    }

    /// Returns the position the next block has to start at, once one has been
    /// taken.
    ///
    /// This follows the stream rather than the retained samples: the raster's
    /// first line can begin after the phasing signal ended, and the samples
    /// between the two are read and dropped rather than skipped.
    const fn expected_sample(&self) -> Option<u64> {
        self.next_sample
    }

    fn begin_phasing(&mut self, sample: u64) -> Result<(), WefaxError> {
        let ioc = self.ioc.ok_or(WefaxError::FormatNotSelected)?;
        self.phasing = Some(PhasingDetector::new(
            self.sample_rate_hz,
            ioc,
            self.config.candidates(),
            sample,
        )?);
        self.state = RxState::Phasing;
        Ok(())
    }

    fn consume(&mut self, frequency_hz: f64, sample: u64) -> Result<(), WefaxError> {
        match self.state {
            RxState::Idle | RxState::Starting { .. } => Ok(()),
            RxState::Phasing => {
                self.fold(frequency_hz, sample);
                self.settle_phasing(sample)
            }
            RxState::Imaging { .. } => {
                self.window.push(frequency_hz as f32, sample);
                self.decode_ready_lines()
            }
            RxState::Complete { .. } | RxState::Stopped { .. } => Ok(()),
        }
    }

    fn fold(&mut self, frequency_hz: f64, sample: u64) {
        let normalized = self.config.band.normalized(frequency_hz);
        if let Some(phasing) = self.phasing.as_mut() {
            phasing.process(normalized, sample);
        }
    }

    /// Decides whether the phasing signal has said all it is going to.
    fn settle_phasing(&mut self, sample: u64) -> Result<(), WefaxError> {
        let Some(phasing) = self.phasing.as_ref() else {
            return Ok(());
        };
        let rate = f64::from(self.sample_rate_hz);
        let elapsed = phasing.elapsed() / rate;
        if elapsed < self.config.phasing_min_seconds {
            return Ok(());
        }
        let timed_out = elapsed >= self.config.phasing_max_seconds;
        // The picture starting is what ends the phasing signal; the thirty
        // seconds it nominally runs for is a nominal figure, and a service
        // that sends more or less of it still has to be received.
        if !timed_out && !phasing.picture_started() {
            return Ok(());
        }
        match phasing.resolve() {
            Some(result) => self.begin_imaging(result, sample),
            None if timed_out => match self.config.phasing_fallback {
                PhasingFallback::StartImaging => {
                    let format = self
                        .format
                        .or(self.config.format)
                        .ok_or(WefaxError::FormatNotSelected)?;
                    let fallback = PhasingResult {
                        format,
                        epoch_samples: sample as f64,
                        samples_per_line: format.samples_per_line(self.sample_rate_hz),
                        offset_pixels: 0.0,
                        contrast: 0.0,
                        lines: 0,
                    };
                    self.begin_imaging(fallback, sample)
                }
                PhasingFallback::Stop => {
                    self.stop(StopReason::PhasingNotAcquired);
                    Ok(())
                }
            },
            None => Ok(()),
        }
    }

    fn begin_imaging(&mut self, result: PhasingResult, sample: u64) -> Result<(), WefaxError> {
        let format = result.format;
        let offset = self.config.phase_offset_fraction * result.samples_per_line;
        let mut clock = LineClock::new(result.epoch_samples - offset, result.samples_per_line)?;
        // Decoding starts on the first line that begins at or after the
        // sample in hand; the fold consumed everything before it.
        let line = clock.line_at(sample as f64);
        let first = if line <= 0.0 { 0.0 } else { libm::ceil(line) };
        clock.shift(first * clock.samples_per_line())?;

        let limit = self.config.line_limit(format);
        self.raster = Some(GrayRaster::new(
            format.pixels_per_line(),
            limit,
            format.lines_per_minute.as_lpm() as usize,
        )?);
        self.window.restart(clock.epoch_samples());
        self.slant.reset();
        self.origin_shift = 0.0;
        self.clock = Some(clock);
        self.format = Some(format);
        self.phasing = None;
        self.phasing_result = Some(result);
        self.next_line = 0;
        self.state = RxState::Imaging { lines: 0 };
        self.events.push_back(RxEvent::FormatSelected { format });
        self.events.push_back(RxEvent::PhasingAcquired {
            epoch_samples: clock.epoch_samples(),
            offset_pixels: result.offset_pixels,
            contrast: result.contrast,
            lines: result.lines,
        });
        Ok(())
    }

    fn decode_ready_lines(&mut self) -> Result<(), WefaxError> {
        let (Some(clock), Some(format)) = (self.clock, self.format) else {
            return Ok(());
        };
        let mut clock = clock;
        while self.window.end() as f64 >= clock.position_at(self.next_line + 1, 0.0) {
            self.decode_line(clock, format)?;
            if self.state.is_terminal() {
                return Ok(());
            }
            if self.track_slant()? {
                clock = self.clock.ok_or(WefaxError::NotImaging)?;
            }
        }
        let retained = clock.position_at(self.next_line.saturating_sub(RETAIN_LINES), 0.0);
        self.window.discard_before(retained.max(0.0) as u64);
        Ok(())
    }

    /// Refits the clock from the picture, and says whether it changed.
    fn track_slant(&mut self) -> Result<bool, WefaxError> {
        if !self.config.slant_tracking {
            return Ok(false);
        }
        let Some(raster) = self.raster.as_ref() else {
            return Ok(false);
        };
        let Some(drift) = self.slant.observe(raster, self.next_line) else {
            return Ok(false);
        };
        self.adjust_slant(drift)?;
        Ok(true)
    }

    fn decode_line(&mut self, clock: LineClock, format: Format) -> Result<(), WefaxError> {
        let line = self.next_line;
        let width = format.pixels_per_line();
        let band = self.config.band;
        let inverted = self.config.inverted;

        let raster = self.raster.as_mut().ok_or(WefaxError::NotImaging)?;
        match raster.push_row() {
            Ok(row) => debug_assert_eq!(row, line),
            Err(WefaxError::LineLimitReached { .. }) => {
                self.stop(StopReason::LineLimit);
                return Ok(());
            }
            Err(error) => return Err(error),
        }

        let Self { window, raster, .. } = self;
        let row = raster
            .as_mut()
            .and_then(|raster| raster.row_mut(line))
            .ok_or(WefaxError::NotImaging)?;
        // The right edge of one pixel is the left edge of the next, so each
        // boundary is computed once and carried over.
        let mut left = clock.position_at(line, 0.0);
        for (pixel, target) in row.iter_mut().enumerate() {
            let right = clock.position_at(line, (pixel + 1) as f64 / width as f64);
            let level = band.level(window.pixel_mean(left, right)?);
            *target = if inverted { u8::MAX - level } else { level };
            left = right;
        }

        self.next_line += 1;
        self.revision += 1;
        self.state = RxState::Imaging { lines: self.next_line };
        self.events.push_back(RxEvent::LineDecoded { line });
        Ok(())
    }
}

/// The demodulated samples still needed to draw lines.
///
/// Only a couple of line periods are kept: a phase correction may move the
/// raster backwards, and nothing older than that is ever read again. Twenty
/// minutes of demodulated stream would be over a hundred megabytes, which is
/// why the slant correction happens on the raster instead.
#[derive(Clone, Debug, Default)]
struct SampleWindow {
    first_sample: u64,
    frequency_hz: VecDeque<f32>,
}

impl SampleWindow {
    fn restart(&mut self, first_sample: f64) {
        self.first_sample = first_sample.max(0.0) as u64;
        self.frequency_hz.clear();
    }

    fn push(&mut self, value: f32, sample: u64) {
        if self.frequency_hz.is_empty() && sample >= self.first_sample {
            self.first_sample = sample;
        }
        if sample >= self.first_sample {
            self.frequency_hz.push_back(value);
        }
    }

    fn end(&self) -> u64 {
        self.first_sample + self.frequency_hz.len() as u64
    }

    fn discard_before(&mut self, sample: u64) {
        let drop = sample
            .saturating_sub(self.first_sample)
            .min(self.frequency_hz.len() as u64);
        self.frequency_hz.drain(..drop as usize);
        self.first_sample += drop;
    }

    /// Averages the central part of a pixel's interval.
    ///
    /// When the interval is narrower than a sample, which is the normal case
    /// at the lowest capture rates, the nearest sample stands in for it. That
    /// is a soft picture, not a failure.
    fn pixel_mean(&self, left: f64, right: f64) -> Result<f64, WefaxError> {
        let guard = (right - left) * PIXEL_GUARD;
        let first = libm::ceil(left + guard).max(0.0) as u64;
        let end = libm::ceil(right - guard).max(0.0) as u64;
        if end > first {
            let mut sum = 0.0;
            for run in self.runs(first, end)? {
                for &value in run {
                    sum += f64::from(value);
                }
            }
            return Ok(sum / (end - first) as f64);
        }
        let center = libm::ceil((left + right) * 0.5 - 0.5).max(0.0) as u64;
        Ok(f64::from(self.get(center)?))
    }

    /// Returns the retained samples covering `[first, end)` as the at most two
    /// runs the ring holds them in, translating and bounds-checking the range
    /// once rather than every sample.
    fn runs(&self, first: u64, end: u64) -> Result<[&[f32]; 2], WefaxError> {
        if first < self.first_sample {
            return Err(WefaxError::SampleDiscarded { sample: first });
        }
        if end > self.end() {
            return Err(WefaxError::SampleDiscarded { sample: self.end() });
        }
        let start = (first - self.first_sample) as usize;
        let stop = (end - self.first_sample) as usize;
        let (head, tail) = self.frequency_hz.as_slices();
        Ok(if stop <= head.len() {
            [&head[start..stop], &[]]
        } else if start >= head.len() {
            [&tail[start - head.len()..stop - head.len()], &[]]
        } else {
            [&head[start..], &tail[..stop - head.len()]]
        })
    }

    fn get(&self, sample: u64) -> Result<f32, WefaxError> {
        sample
            .checked_sub(self.first_sample)
            .and_then(|offset| self.frequency_hz.get(offset as usize).copied())
            .ok_or(WefaxError::SampleDiscarded { sample })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{LinesPerMinute, PHASING_WHITE_FRACTION};
    use alloc::{vec, vec::Vec};
    use rstest::rstest;

    /// Builds a demodulated stream: phasing, then a picture of `columns`
    /// repeated on every line.
    fn stream(
        rate: u32,
        format: Format,
        band: WefaxBand,
        phasing_seconds: f64,
        columns: &[u8],
        lines: usize,
    ) -> Vec<f32> {
        stream_at(rate, format, band, phasing_seconds, columns, lines, 0.0)
    }

    /// The same, sent at a line rate that is `rate_error_ppm` off nominal.
    fn stream_at(
        rate: u32,
        format: Format,
        band: WefaxBand,
        phasing_seconds: f64,
        columns: &[u8],
        lines: usize,
        rate_error_ppm: f64,
    ) -> Vec<f32> {
        let line_samples = format.samples_per_line(rate) * (1.0 + rate_error_ppm * 1.0e-6);
        let width = format.pixels_per_line();
        let phasing = (f64::from(rate) * phasing_seconds) as u64;
        let picture = (line_samples * lines as f64) as u64;
        let mut samples = Vec::with_capacity((phasing + picture) as usize);
        for sample in 0..phasing {
            let position = (sample as f64 / line_samples).fract();
            let white = position < PHASING_WHITE_FRACTION;
            samples.push(band.frequency_hz(if white { u8::MAX } else { 0 }) as f32);
        }
        for sample in 0..picture {
            let position = ((phasing + sample) as f64 / line_samples).fract();
            let pixel = ((position * width as f64) as usize).min(width - 1);
            samples.push(band.frequency_hz(columns[pixel * columns.len() / width]) as f32);
        }
        samples
    }

    fn decode(rate: u32, format: Format, samples: &[f32], config: RxConfig) -> WefaxDecoder {
        let mut decoder = WefaxDecoder::with_config(rate, config).unwrap();
        decoder.start_manually(format, 0).unwrap();
        let block = DemodulatedBlock::new(0, samples).unwrap();
        decoder.process(block).unwrap();
        decoder
    }

    fn config() -> RxConfig {
        RxConfig {
            phasing_min_seconds: 1.0,
            slant_tracking: false,
            ..RxConfig::default()
        }
    }

    #[rstest]
    fn a_transmitted_edge_decodes_where_it_was_sent(
        #[values(8_000, 11_025, 48_000)] rate: u32,
        #[values(Ioc::Ioc576, Ioc::Ioc288)] ioc: Ioc,
        #[values(LinesPerMinute::L120, LinesPerMinute::L240)] lines_per_minute: LinesPerMinute,
    ) {
        let format = Format { ioc, lines_per_minute };
        let band = WefaxBand::WIDE;
        let columns: Vec<u8> = (0..64).map(|index| if index < 32 { 0 } else { u8::MAX }).collect();
        let samples = stream(rate, format, band, 3.0, &columns, 30);
        let decoder = decode(rate, format, &samples, config());

        let raster = decoder.raster().expect("a raster was drawn");
        assert!(raster.height() > 4, "only {} lines decoded", raster.height());
        let width = format.pixels_per_line();
        let row = raster.row(raster.height() / 2).unwrap();
        let edge = row
            .iter()
            .position(|level| *level > 128)
            .expect("the edge is in the row");
        assert!(
            (edge as i64 - (width / 2) as i64).abs() <= 2,
            "the edge decoded at {edge} of {width}"
        );
    }

    #[rstest]
    fn a_gray_ramp_reads_back_as_itself(#[values(11_025, 48_000)] rate: u32) {
        let format = Format::MARINE;
        let band = WefaxBand::WIDE;
        let columns: Vec<u8> = (0..32).map(|index| (index * 255 / 31) as u8).collect();
        let samples = stream(rate, format, band, 3.0, &columns, 20);
        let decoder = decode(rate, format, &samples, config());

        let raster = decoder.raster().unwrap();
        let width = format.pixels_per_line();
        let row = raster.row(raster.height() / 2).unwrap();
        for (index, expected) in columns.iter().enumerate() {
            // Read the middle of each band, away from the transitions.
            let pixel = (index * width / columns.len()) + width / columns.len() / 2;
            let found = row[pixel];
            assert!(
                (i16::from(found) - i16::from(*expected)).abs() <= 4,
                "band {index} was sent as {expected} and read as {found}"
            );
        }
    }

    #[test]
    fn block_division_does_not_change_the_result() {
        let rate = 11_025;
        let format = Format::MARINE;
        let columns: Vec<u8> = (0..16).map(|index| (index * 17) as u8).collect();
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &columns, 12);
        let whole = decode(rate, format, &samples, config());

        for size in [73, 1_024, 4_093] {
            let mut split = WefaxDecoder::with_config(rate, config()).unwrap();
            split.start_manually(format, 0).unwrap();
            let mut position = 0_u64;
            for packet in samples.chunks(size) {
                let block = DemodulatedBlock::new(position, packet).unwrap();
                split.process(block).unwrap();
                position += packet.len() as u64;
            }
            assert_eq!(
                split.raster().unwrap(),
                whole.raster().unwrap(),
                "packets of {size} decoded differently"
            );
        }
    }

    #[test]
    fn a_gap_between_blocks_is_rejected() {
        let rate = 11_025;
        let format = Format::MARINE;
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &[0, 255], 10);
        let mut decoder = WefaxDecoder::with_config(rate, config()).unwrap();
        decoder.start_manually(format, 0).unwrap();
        decoder
            .process(DemodulatedBlock::new(0, &samples[..1_000]).unwrap())
            .unwrap();

        let error = decoder
            .process(DemodulatedBlock::new(2_000, &samples[2_000..3_000]).unwrap())
            .unwrap_err();
        assert_eq!(error.consumed(), 0);
        assert_eq!(
            error.error(),
            WefaxError::DemodulatedGap {
                expected: 1_000,
                actual: 2_000
            }
        );
    }

    #[test]
    fn input_after_the_end_is_rejected() {
        let rate = 11_025;
        let format = Format::MARINE;
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &[0, 255], 10);
        let mut decoder = decode(rate, format, &samples, config());
        decoder.stop(StopReason::Manual);
        let error = decoder
            .process(DemodulatedBlock::new(0, &samples[..10]).unwrap())
            .unwrap_err();
        assert_eq!(error.error(), WefaxError::AlreadyComplete);
    }

    #[test]
    fn the_line_limit_ends_the_reception() {
        let rate = 11_025;
        let format = Format::MARINE;
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &[0, 255], 20);
        let decoder = decode(
            rate,
            format,
            &samples,
            RxConfig {
                max_lines: Some(3),
                ..config()
            },
        );
        assert_eq!(
            decoder.state(),
            RxState::Stopped {
                lines: 3,
                reason: StopReason::LineLimit
            }
        );
        assert_eq!(decoder.raster().unwrap().height(), 3);
    }

    #[test]
    fn inversion_flips_the_gray_scale() {
        let rate = 11_025;
        let format = Format::MARINE;
        let columns = [0_u8, 255];
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &columns, 12);
        let upright = decode(rate, format, &samples, config());
        let inverted = decode(
            rate,
            format,
            &samples,
            RxConfig {
                inverted: true,
                ..config()
            },
        );
        let row = upright.raster().unwrap().row(0).unwrap();
        let flipped = inverted.raster().unwrap().row(0).unwrap();
        for (upright, inverted) in row.iter().zip(flipped) {
            assert_eq!(u8::MAX - upright, *inverted);
        }
    }

    #[test]
    fn a_stop_tone_completes_the_reception() {
        let rate = 11_025;
        let format = Format::MARINE;
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &[0, 255], 12);
        let mut decoder = decode(rate, format, &samples, config());
        let lines = decoder.line_count();
        decoder.accept_apt(AptEvent::StopAccepted { sample: 99 }).unwrap();
        assert_eq!(decoder.state(), RxState::Complete { lines });
        let outcome = decoder.finish();
        assert_eq!(outcome.format, Some(format));
        assert!(outcome.raster.is_some());
        assert!(outcome.samples_per_line.is_some());
    }

    #[test]
    fn shifting_the_phase_moves_the_picture_and_the_clock() {
        let rate = 11_025;
        let format = Format::MARINE;
        let columns: Vec<u8> = (0..16).map(|index| (index * 17) as u8).collect();
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &columns, 12);
        let mut decoder = decode(rate, format, &samples, config());
        let before = decoder.raster().unwrap().clone();
        let epoch = decoder.phasing().unwrap().epoch_samples;
        let revision = decoder.raster_revision();

        decoder.shift_phase(40).unwrap();
        let after = decoder.raster().unwrap();
        assert_ne!(&before, after);
        assert!(decoder.raster_revision() > revision);
        for row in 0..after.height() {
            let mut expected = before.row(row).unwrap().to_vec();
            expected.rotate_left(40);
            assert_eq!(after.row(row).unwrap(), expected.as_slice());
        }
        assert_eq!(decoder.phasing().unwrap().epoch_samples, epoch);
    }

    #[test]
    fn a_slant_adjustment_reports_what_it_changed() {
        let rate = 11_025;
        let format = Format::MARINE;
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &[0, 255], 12);
        let mut decoder = decode(rate, format, &samples, config());
        let before = decoder.samples_per_line().unwrap();
        while decoder.poll_event().is_some() {}

        decoder.adjust_slant(0.5).unwrap();
        let after = decoder.samples_per_line().unwrap();
        assert!(after > before);
        let event = decoder.poll_event().expect("an adjustment was reported");
        assert!(matches!(
            event,
            RxEvent::SlantAdjusted { samples_per_line, .. } if samples_per_line == after
        ));
    }

    fn texture(width: usize) -> Vec<u8> {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        (0..width)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 56) as u8
            })
            .collect()
    }

    #[rstest]
    #[case(300.0)]
    #[case(-300.0)]
    fn slant_tracking_refits_a_clock_that_is_running_wrong(#[case] rate_error_ppm: f64) {
        let rate = 11_025;
        let format = Format {
            ioc: Ioc::Ioc288,
            lines_per_minute: LinesPerMinute::L240,
        };
        let columns = texture(format.pixels_per_line());
        let samples = stream_at(rate, format, WefaxBand::WIDE, 3.0, &columns, 200, rate_error_ppm);
        let mut decoder = decode(
            rate,
            format,
            &samples,
            RxConfig {
                phasing_min_seconds: 1.0,
                ..RxConfig::default()
            },
        );

        let adjusted =
            core::iter::from_fn(|| decoder.poll_event()).any(|event| matches!(event, RxEvent::SlantAdjusted { .. }));
        assert!(adjusted, "the clock was never refitted");

        let truth = format.samples_per_line(rate) * (1.0 + rate_error_ppm * 1.0e-6);
        let found = decoder.samples_per_line().unwrap();
        let error_ppm = (found - truth) / truth * 1.0e6;
        assert!(
            error_ppm.abs() < 30.0,
            "{rate_error_ppm} ppm was left {error_ppm} ppm out"
        );
    }

    #[test]
    fn a_picture_with_nothing_to_correlate_is_left_running_as_it_was() {
        let rate = 11_025;
        let format = Format {
            ioc: Ioc::Ioc288,
            lines_per_minute: LinesPerMinute::L240,
        };
        let columns = [200_u8; 8];
        let samples = stream_at(rate, format, WefaxBand::WIDE, 3.0, &columns, 200, 300.0);
        let mut decoder = decode(
            rate,
            format,
            &samples,
            RxConfig {
                phasing_min_seconds: 1.0,
                ..RxConfig::default()
            },
        );
        let adjusted =
            core::iter::from_fn(|| decoder.poll_event()).any(|event| matches!(event, RxEvent::SlantAdjusted { .. }));
        assert!(!adjusted, "a featureless picture moved the clock");
        let raster = decoder.raster().unwrap();
        for row in 0..raster.height() {
            assert!(raster.row(row).unwrap().iter().all(|level| level.abs_diff(200) <= 4));
        }
    }

    /// The live tracker gives up below its own threshold, which is a lean of
    /// a hundred pixels over a long chart. Fitting the whole picture at the
    /// end is what takes the rest of it out.
    #[rstest]
    #[case(300.0)]
    #[case(-300.0)]
    fn refining_a_finished_picture_takes_out_the_lean_the_tracker_left(#[case] rate_error_ppm: f64) {
        let rate = 11_025;
        let format = Format {
            ioc: Ioc::Ioc288,
            lines_per_minute: LinesPerMinute::L240,
        };
        let columns = texture(format.pixels_per_line());
        let samples = stream_at(rate, format, WefaxBand::WIDE, 3.0, &columns, 200, rate_error_ppm);
        let mut decoder = decode(
            rate,
            format,
            &samples,
            RxConfig {
                phasing_min_seconds: 1.0,
                slant_tracking: false,
                ..RxConfig::default()
            },
        );
        decoder.stop(StopReason::Manual);

        let truth = format.samples_per_line(rate) * (1.0 + rate_error_ppm * 1.0e-6);
        let before = decoder.samples_per_line().unwrap();
        assert!(
            ((before - truth) / truth * 1.0e6).abs() > 100.0,
            "the reception should still be leaning before it is refined"
        );

        let refinement = decoder.refine().unwrap().expect("the picture was fitted");
        let after = decoder.samples_per_line().unwrap();
        let error_ppm = (after - truth) / truth * 1.0e6;
        assert!(
            error_ppm.abs() < 20.0,
            "{rate_error_ppm} ppm was left {error_ppm} ppm out"
        );
        assert_eq!(refinement.samples_per_line, after);
    }

    /// A correction pivots on the line being decoded, which moves the top of
    /// the picture. The phasing signal is the only thing that ever knew where
    /// a line begins, so what it established is put back at the end.
    #[test]
    fn refining_puts_the_phase_the_phasing_signal_found_back() {
        let rate = 11_025;
        let format = Format::MARINE;
        let columns: Vec<u8> = (0..16).map(|index| (index * 17) as u8).collect();
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &columns, 12);
        let mut decoder = decode(rate, format, &samples, config());

        let original = decoder.raster().unwrap().clone();
        // A correction pivoted well inside the picture carries line zero with
        // it, which is the walk the refinement undoes.
        decoder.adjust_slant(0.5).unwrap();
        assert_ne!(decoder.raster().unwrap().row(0), original.row(0));

        decoder.stop(StopReason::Manual);
        let refinement = decoder.refine().unwrap().expect("something was put back");
        assert_ne!(refinement.displacement_pixels, 0.0);
        assert_eq!(decoder.raster().unwrap().row(0), original.row(0));
    }

    #[test]
    fn refining_keeps_a_shift_the_operator_asked_for() {
        let rate = 11_025;
        let format = Format::MARINE;
        let columns: Vec<u8> = (0..16).map(|index| (index * 17) as u8).collect();
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &columns, 12);
        let mut decoder = decode(rate, format, &samples, config());

        decoder.shift_phase(40).unwrap();
        let shifted = decoder.raster().unwrap().clone();
        decoder.stop(StopReason::Manual);
        decoder.refine().unwrap();

        assert_eq!(decoder.raster().unwrap().row(0), shifted.row(0));
    }

    #[test]
    fn refining_a_reception_that_has_not_ended_is_refused() {
        let rate = 11_025;
        let format = Format::MARINE;
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &[0, 255], 12);
        let mut decoder = decode(rate, format, &samples, config());
        assert_eq!(decoder.refine().unwrap_err(), WefaxError::NotComplete);
    }

    #[test]
    fn refining_a_picture_too_short_to_fit_says_nothing() {
        let mut decoder = WefaxDecoder::new(11_025).unwrap();
        decoder.stop(StopReason::Manual);
        assert_eq!(decoder.refine().unwrap(), None);
    }

    #[test]
    fn a_correction_after_the_reception_pivots_on_the_first_line() {
        let rate = 11_025;
        let format = Format::MARINE;
        let columns: Vec<u8> = (0..16).map(|index| (index * 17) as u8).collect();
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &columns, 12);
        let mut decoder = decode(rate, format, &samples, config());
        decoder.stop(StopReason::Manual);

        let before = decoder.raster().unwrap().clone();
        decoder.adjust_slant(1.0).unwrap();
        let after = decoder.raster().unwrap();

        assert_eq!(after.row(0), before.row(0), "the first row is the pivot");
        let last = after.height() - 1;
        assert_ne!(after.row(last), before.row(last));
    }

    #[test]
    fn the_phase_can_still_be_moved_after_the_reception() {
        let rate = 11_025;
        let format = Format::MARINE;
        let columns: Vec<u8> = (0..16).map(|index| (index * 17) as u8).collect();
        let samples = stream(rate, format, WefaxBand::WIDE, 3.0, &columns, 12);
        let mut decoder = decode(rate, format, &samples, config());
        decoder.stop(StopReason::Manual);

        let before = decoder.raster().unwrap().clone();
        let revision = decoder.raster_revision();
        decoder.shift_phase(40).unwrap();

        let mut expected = before.row(0).unwrap().to_vec();
        expected.rotate_left(40);
        assert_eq!(decoder.raster().unwrap().row(0).unwrap(), expected.as_slice());
        assert!(decoder.raster_revision() > revision);
    }

    #[test]
    fn manual_controls_need_a_raster() {
        let mut decoder = WefaxDecoder::new(11_025).unwrap();
        assert_eq!(decoder.shift_phase(1).unwrap_err(), WefaxError::NotImaging);
        assert_eq!(decoder.adjust_slant(0.5).unwrap_err(), WefaxError::NotImaging);
    }

    #[test]
    fn a_start_tone_names_the_index_and_its_end_begins_the_phasing() {
        let mut decoder = WefaxDecoder::new(11_025).unwrap();
        decoder
            .accept_apt(AptEvent::StartAccepted {
                ioc: Ioc::Ioc288,
                sample: 10,
            })
            .unwrap();
        assert_eq!(decoder.state(), RxState::Starting { ioc: Ioc::Ioc288 });
        assert_eq!(
            decoder.poll_event(),
            Some(RxEvent::AptStartDetected {
                ioc: Ioc::Ioc288,
                sample: 10
            })
        );
        decoder.accept_apt(AptEvent::StartEnded { sample: 4_096 }).unwrap();
        assert_eq!(decoder.state(), RxState::Phasing);
    }

    #[test]
    fn a_start_tone_is_ignored_once_automatic_starting_is_off() {
        let mut decoder = WefaxDecoder::with_config(
            11_025,
            RxConfig {
                auto_start: false,
                ..RxConfig::default()
            },
        )
        .unwrap();
        decoder
            .accept_apt(AptEvent::StartAccepted {
                ioc: Ioc::Ioc576,
                sample: 0,
            })
            .unwrap();
        assert_eq!(decoder.state(), RxState::Idle);
    }

    #[test]
    fn phasing_that_never_resolves_can_stop_the_reception() {
        let rate = 11_025;
        let format = Format::MARINE;
        let config = RxConfig {
            phasing_min_seconds: 1.0,
            phasing_max_seconds: 2.0,
            phasing_fallback: PhasingFallback::Stop,
            ..RxConfig::default()
        };
        let samples = vec![WefaxBand::WIDE.center_hz as f32; rate as usize * 3];
        let decoder = decode(rate, format, &samples, config);
        assert_eq!(
            decoder.state(),
            RxState::Stopped {
                lines: 0,
                reason: StopReason::PhasingNotAcquired
            }
        );
    }

    #[test]
    fn phasing_that_never_resolves_can_start_drawing_anyway() {
        let rate = 11_025;
        let format = Format::MARINE;
        let config = RxConfig {
            phasing_min_seconds: 1.0,
            phasing_max_seconds: 2.0,
            phasing_fallback: PhasingFallback::StartImaging,
            format: Some(format),
            ..RxConfig::default()
        };
        let samples = vec![WefaxBand::WIDE.center_hz as f32; rate as usize * 6];
        let decoder = decode(rate, format, &samples, config);
        assert!(matches!(decoder.state(), RxState::Imaging { lines } if lines > 0));
        assert_eq!(decoder.phasing().unwrap().contrast, 0.0);
    }
}
