use alloc::{collections::VecDeque, vec, vec::Vec};
use core::ops::Range;

use crate::{
    RxProcessError, SstvError,
    color::{YCrCb8, y_cr_cb_to_rgb},
    image::{ImageSize, Rgb8, RgbImage},
    mode::{Mode, RasterOrganization, ScanChannel, Support},
    signal::Frequency,
    time::{SstvDuration, samples_spanned},
};

use super::{
    acquisition::{acquire_full, acquire_startup, startup_window_samples},
    clock::{RasterClock, ceil_sample},
    config::{RasterStart, RxConfig, Staging},
    event::{RxEvent, RxOutcome, RxProcessResult, RxState, StopReason},
    input::{DemodulatedBlock, SampleBuffer},
    raster::{PixelSegment, RasterProfile},
    slant::{SlantEstimate, SlantEstimator},
    sync::{MIN_CONFIDENCE, SyncObservation, observe, push_bounded},
};

const BAD_SYNC_SCORE_LIMIT: u8 = 8;
const BAD_SYNC_PENALTY: u8 = 1;
const GOOD_SYNC_REWARD: u8 = 2;
const AUTO_STOP_WARMUP: usize = 8;
/// Most recent rate estimates averaged for a live refit.
///
/// MMSSTV smooths its real-time rate estimate over the same count and averages
/// however many estimates it has collected, so the first refit does not wait
/// for the window to fill.
const LIVE_SLANT_SMOOTHING: usize = 16;

const LIVE_SLANT_MIN_UNITS: usize = 8;

const LIVE_SLANT_HOLDOFF_UNITS: usize = 8;

/// Numerator of the shrinking acceptance threshold, in parts per million.
const LIVE_SLANT_THRESHOLD_SCALE: f64 = 3_200.0;

const LIVE_SLANT_MIN_THRESHOLD_PPM: f64 = 8.0;

const PHASE_AGREEMENT: usize = 3;
const PHASE_HOLDOFF_UNITS: usize = 6;
const MIN_PHASE_DISPLACEMENT: u64 = 2;
const PIXEL_GUARD: f64 = 0.1875;

/// Result of rebuilding an image from staged immutable samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RefinementResult {
    /// New monotonically increasing image revision.
    pub revision: u64,
    /// Global timing estimate used for the rebuild.
    pub slant: SlantEstimate,
}

/// Concrete streaming decoder for the supported SSTV receive modes.
///
/// Input must extend at least one demodulated sample beyond the nominal raster
/// end so the final guarded pixel window can become available.
#[derive(Debug)]
pub struct RxDecoder {
    raster: Raster,
    sample_rate_hz: u32,
    config: RxConfig,
    decode: DecodeState,
    input: Option<SampleBuffer>,
    next_sample: Option<u64>,
    observations: VecDeque<SyncObservation>,
    staged_observations: Vec<SyncObservation>,
    phase_displacements: VecDeque<i64>,
    last_phase_adjustment: Option<usize>,
    bad_sync_score: u8,
    sync_checks: usize,
    rate_estimates: VecDeque<f64>,
    last_slant_unit: Option<usize>,
    staged: Option<SampleBuffer>,
    image_revision: u64,
}

#[derive(Debug)]
struct DecodeState {
    image: RgbImage,
    state: RxState,
    clock: Option<RasterClock>,
    raster_unit: usize,
    delivered_rows: usize,
    pending_events: VecDeque<RxEvent>,
    robot_chroma: [Vec<u8>; 2],
    robot_selector: Option<RobotSelector>,
}

impl DecodeState {
    fn new(size: ImageSize) -> Self {
        Self {
            image: RgbImage::new(size, Rgb8::default()),
            state: RxState::Acquiring,
            clock: None,
            raster_unit: 0,
            delivered_rows: 0,
            pending_events: VecDeque::with_capacity(3),
            robot_chroma: [vec![128; size.width()], vec![128; size.width()]],
            robot_selector: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RobotSelector {
    Cr,
    Cb,
}

impl RobotSelector {
    const fn opposite(self) -> Self {
        match self {
            Self::Cr => Self::Cb,
            Self::Cb => Self::Cr,
        }
    }

    const fn plane(self) -> usize {
        match self {
            Self::Cr => 0,
            Self::Cb => 1,
        }
    }
}

impl RxDecoder {
    /// Validates mode and sample rate and allocates the transport-sized image.
    pub fn new(mode: Mode, sample_rate_hz: u32) -> Result<Self, SstvError> {
        Self::with_config(mode, sample_rate_hz, RxConfig::default())
    }

    /// Validates inputs and constructs a decoder with explicit receive options.
    pub fn with_config(mode: Mode, sample_rate_hz: u32, config: RxConfig) -> Result<Self, SstvError> {
        if sample_rate_hz == 0 {
            return Err(SstvError::InvalidSampleRate);
        }
        let Some(profile) = RasterProfile::for_mode(mode) else {
            return Err(SstvError::UnsupportedRxMode(mode));
        };
        if mode.spec().decode_support() != Support::Supported {
            return Err(SstvError::UnsupportedRxMode(mode));
        }
        let size = ImageSize::new(mode.spec().width() as usize, mode.spec().height() as usize)
            .expect("mode dimensions are valid");
        Ok(Self {
            raster: Raster {
                mode,
                profile,
                width: size.width(),
            },
            sample_rate_hz,
            config,
            decode: DecodeState::new(size),
            input: None,
            next_sample: None,
            observations: VecDeque::with_capacity(16),
            staged_observations: Vec::new(),
            phase_displacements: VecDeque::with_capacity(PHASE_AGREEMENT),
            last_phase_adjustment: None,
            bad_sync_score: 0,
            sync_checks: 0,
            rate_estimates: VecDeque::with_capacity(LIVE_SLANT_SMOOTHING),
            last_slant_unit: None,
            staged: None,
            image_revision: 0,
        })
    }

    /// Returns the selected mode.
    pub const fn mode(&self) -> Mode {
        self.raster.mode
    }

    /// Returns the current decoder state.
    pub const fn state(&self) -> RxState {
        self.decode.state
    }

    /// Returns the decoded image.
    pub const fn image(&self) -> &RgbImage {
        &self.decode.image
    }

    /// Returns the acquired physical raster epoch, if available.
    pub fn source_epoch(&self) -> Option<u64> {
        self.decode.clock.map(|clock| clock.source_epoch())
    }

    /// Returns the acquired effective sample rate, if available.
    pub fn effective_sample_rate_hz(&self) -> Option<f64> {
        self.decode.clock.map(|clock| clock.effective_sample_rate_hz())
    }

    /// Returns the immutable physical sample rate supplied at construction.
    pub const fn physical_sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Returns the bounded sync observation history from oldest to newest.
    pub fn sync_observations(&self) -> impl ExactSizeIterator<Item = &SyncObservation> {
        self.observations.iter()
    }

    /// Returns the number of paired samples retained for staged refinement.
    pub fn staged_samples_len(&self) -> usize {
        self.staged.as_ref().map_or(0, SampleBuffer::len)
    }

    /// Returns the current image revision, initially zero.
    pub const fn image_revision(&self) -> u64 {
        self.image_revision
    }

    /// Terminates decoding, keeping the rows decoded so far.
    ///
    /// AutoStop reaches the same terminal state from the decoder's own
    /// synchronization history. This is the entry point for a caller that can
    /// see a reception is over for a reason the decoder cannot observe, such as
    /// a signal that stopped arriving at all: no further input advances the
    /// raster, so nothing here would ever score a bad line. An already
    /// finished reception is left as it is.
    pub fn stop(&mut self, reason: StopReason) {
        if matches!(self.decode.state, RxState::Complete | RxState::Stopped { .. }) {
            return;
        }
        self.decode.state = RxState::Stopped {
            completed_rows: self.decode.delivered_rows,
            reason,
        };
        self.queue_event(RxEvent::Stopped { reason });
    }

    /// Consumes a prefix and returns at most one event.
    ///
    /// For a non-empty valid block this either consumes at least one sample or
    /// returns an event. Once [`RxState::Complete`] is reached, further non-empty
    /// input is rejected. If processing fails after consuming a prefix, the
    /// returned [`RxProcessError`] reports that prefix so it is not resent.
    pub fn process(&mut self, block: DemodulatedBlock<'_>) -> Result<RxProcessResult, RxProcessError> {
        let expected = self.next_sample;
        self.process_inner(block).map_err(|error| {
            let consumed = if expected.is_none_or(|sample| sample == block.first_sample()) {
                self.next_sample
                    .and_then(|sample| sample.checked_sub(block.first_sample()))
                    .and_then(|count| usize::try_from(count).ok())
                    .unwrap_or(0)
                    .min(block.frequency_hz().len())
            } else {
                0
            };
            if consumed == 0 && expected.is_none() {
                self.input = None;
                self.staged = None;
                self.next_sample = None;
            }
            RxProcessError::new(consumed, error)
        })
    }

    fn process_inner(&mut self, block: DemodulatedBlock<'_>) -> Result<RxProcessResult, SstvError> {
        block.validate_continuity(self.next_sample)?;
        if let Some(event) = self.poll_event() {
            return Ok(RxProcessResult::new(0, Some(event)));
        }
        if matches!(self.decode.state, RxState::Complete | RxState::Stopped { .. }) {
            return if block.frequency_hz().is_empty() {
                Ok(RxProcessResult::new(0, None))
            } else {
                Err(SstvError::RxAlreadyComplete)
            };
        }
        if self.input.is_none() {
            self.input = Some(SampleBuffer::new(block.first_sample()));
            if let Staging::Memory { max_samples } = self.config.staging {
                self.staged = Some(SampleBuffer::with_capacity(
                    block.first_sample(),
                    self.staging_reservation(max_samples),
                ));
            }
            self.next_sample = Some(block.first_sample());
        }

        let mut consumed = 0;
        loop {
            if self.decode.clock.is_none() {
                if self.config.raster_start == RasterStart::AfterHeader {
                    let clock = self.clock_at_input_start()?;
                    self.decode.clock = Some(clock);
                    self.skip_units_before_input(clock)?;
                    self.decode.state = RxState::Decoding {
                        completed_rows: self.decode.delivered_rows,
                    };
                    return Ok(RxProcessResult::new(
                        consumed,
                        Some(RxEvent::RasterAcquired {
                            source_epoch: clock.source_epoch(),
                            effective_sample_rate_hz: clock.effective_sample_rate_hz(),
                        }),
                    ));
                }
                let input = self.input.as_ref().expect("input initialized");
                let target = startup_window_samples(self.raster.profile, self.sample_rate_hz);
                if input.len() as u64 >= target {
                    match acquire_startup(
                        input,
                        self.raster.profile,
                        self.sample_rate_hz,
                        self.config.sync_detector_delay,
                    ) {
                        Ok(clock) => {
                            self.decode.clock = Some(clock);
                            self.skip_units_before_input(clock)?;
                            self.decode.state = RxState::Decoding {
                                completed_rows: self.decode.delivered_rows,
                            };
                            return Ok(RxProcessResult::new(
                                consumed,
                                Some(RxEvent::RasterAcquired {
                                    source_epoch: clock.source_epoch(),
                                    effective_sample_rate_hz: clock.effective_sample_rate_hz(),
                                }),
                            ));
                        }
                        Err(SstvError::RasterNotAcquired) => {
                            let advance = (target / 3).max(1);
                            let keep_from = input.first().saturating_add(advance);
                            self.input
                                .as_mut()
                                .expect("input initialized")
                                .discard_before(keep_from);
                            // What acquisition passed over can never be decoded,
                            // so retaining it would only spend the staging
                            // capacity a long stretch of noise then exhausts.
                            if let Some(staged) = self.staged.as_mut() {
                                staged.discard_before(keep_from);
                            }
                            if consumed == block.frequency_hz().len() {
                                return Ok(RxProcessResult::new(consumed, None));
                            }
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
                let needed = usize::try_from(target - input.len() as u64).unwrap_or(usize::MAX);
                let take = needed.min(block.frequency_hz().len() - consumed);
                self.append(block, consumed, take)?;
                consumed += take;
                if take == 0 {
                    return Ok(RxProcessResult::new(consumed, None));
                }
                continue;
            }

            if self.can_decode_next()? {
                if !self.synchronize()? {
                    let event = self.poll_event().expect("stop event queued");
                    return Ok(RxProcessResult::new(consumed, Some(event)));
                }
                if !self.can_decode_next()? {
                    if let Some(event) = self.poll_event() {
                        return Ok(RxProcessResult::new(consumed, Some(event)));
                    }
                    continue;
                }
                for row in self.decode_next()? {
                    self.queue_event(RxEvent::RowDecoded { row });
                }
                self.track_slant()?;
                if let Some(event) = self.poll_event() {
                    return Ok(RxProcessResult::new(consumed, Some(event)));
                }
                continue;
            }
            let remaining = block.frequency_hz().len() - consumed;
            if remaining == 0 {
                return Ok(RxProcessResult::new(consumed, None));
            }
            let required_end = self.required_end()?;
            let available_end = self.input.as_ref().expect("input initialized").end();
            let needed = usize::try_from(required_end.saturating_sub(available_end)).unwrap_or(usize::MAX);
            let take = needed.max(1).min(remaining);
            self.append(block, consumed, take)?;
            consumed += take;
        }
    }

    /// Consumes the decoder and returns its complete or partial image.
    pub fn finish(self) -> RxOutcome {
        let DecodeState { image, state, .. } = self.decode;
        match state {
            RxState::Complete => RxOutcome::Complete(image),
            RxState::Stopped { reason, .. } => RxOutcome::Stopped { image, reason },
            state => RxOutcome::Incomplete { image, state },
        }
    }

    /// How much room the retained stream is given before it has to grow.
    ///
    /// The configured maximum is what a caller intends to retain, so it is
    /// asked for outright: growing into it instead would leave up to half the
    /// allocation unused and copy everything retained so far on the way. It is
    /// bounded against the mode's own raster because a caller can state a
    /// maximum far larger than any one picture — an offline decoder handed a
    /// long recording states the length of the file — and reserving that would
    /// be worse than growing. Twice the raster leaves room for the trailing
    /// audio a refinement is fitted against.
    fn staging_reservation(&self, max_samples: usize) -> usize {
        let raster = self.raster.profile.period_ps.saturating_mul(self.raster.units() as u64);
        let samples = SstvDuration::from_picos(raster).to_samples_ceil(self.sample_rate_hz) as usize;
        max_samples.min(samples.saturating_mul(2))
    }

    /// Places the raster where the header said it starts.
    ///
    /// The first input sample is where mode detection ended, which is the end
    /// of the header, and a mode that sends leading segments before its first
    /// raster unit starts that much later. Detection is timed on the
    /// synchronization envelope, whose lag makes this phase a few milliseconds
    /// late; the samples that lag covers were dropped with the header, so the
    /// raster cannot reach back for them and live synchronization corrects the
    /// phase instead.
    fn clock_at_input_start(&self) -> Result<RasterClock, SstvError> {
        let first = self.input.as_ref().expect("input initialized").first();
        let rate = f64::from(self.sample_rate_hz);
        let epoch = first as f64 + samples_spanned(rate, self.raster.profile.leading_ps);
        RasterClock::from_estimate(epoch, rate)
    }

    /// Starts the raster at the first unit whose picture the input still holds.
    ///
    /// Mode detection completes a few milliseconds after the raster epoch it
    /// implies, so for modes that begin their picture right after the leading
    /// sync pulse the first unit is already partly gone by the time decoding
    /// starts. Those rows are left blank and counted as delivered, which is
    /// what MMSSTV shows as well, rather than failing the whole reception on
    /// samples that were never received.
    fn skip_units_before_input(&mut self, clock: RasterClock) -> Result<(), SstvError> {
        let Some(segment) = self.raster.profile.pixels().iter().next() else {
            return Err(SstvError::UnsupportedRxMode(self.raster.mode));
        };
        let first = self.input.as_ref().expect("input initialized").first();
        let units = self.raster.units();
        let mut unit = 0;
        while unit < units && self.raster.pixel_window(clock, unit, segment, 0)?.0 < first {
            unit += 1;
        }
        if unit == units {
            return Err(SstvError::RasterNotAcquired);
        }
        self.decode.raster_unit = unit;
        self.decode.delivered_rows = unit * self.raster.mode.spec().rows_per_raster_unit() as usize;
        Ok(())
    }

    /// Takes `count` samples of `block` from `offset` into the working window,
    /// and into staging when a refit may need to read them again.
    fn append(&mut self, block: DemodulatedBlock<'_>, offset: usize, count: usize) -> Result<(), SstvError> {
        block.validate_range(offset, count)?;
        let first = block
            .first_sample()
            .checked_add(offset as u64)
            .ok_or(SstvError::SamplePositionOverflow)?;
        let part = DemodulatedBlock::new(
            first,
            &block.frequency_hz()[offset..offset + count],
            &block.sync_strength()[offset..offset + count],
        );
        if let Staging::Memory { max_samples } = self.config.staging {
            let staged_len = self.staged.as_ref().map_or(0, SampleBuffer::len);
            if staged_len.checked_add(count).is_none_or(|len| len > max_samples) {
                return Err(SstvError::StagingCapacityExceeded { max_samples });
            }
            self.staged.as_mut().expect("staging initialized").append(part, count);
        }
        self.input.as_mut().expect("input initialized").append(part, count);
        self.next_sample = Some(
            first
                .checked_add(count as u64)
                .ok_or(SstvError::SamplePositionOverflow)?,
        );
        Ok(())
    }

    /// Returns the sample the current raster unit's last pixel window ends at.
    fn required_end(&self) -> Result<u64, SstvError> {
        let clock = self.decode.clock.expect("clock acquired");
        let last = self
            .raster
            .profile
            .pixels()
            .last()
            .ok_or(SstvError::UnsupportedRxMode(self.raster.mode))?;
        let width = self.raster.width;
        let (_, end) = self
            .raster
            .pixel_window(clock, self.decode.raster_unit, last, width - 1)?;
        Ok(end)
    }

    fn can_decode_next(&self) -> Result<bool, SstvError> {
        if self.decode.raster_unit >= self.raster.units() {
            return Ok(false);
        }
        Ok(self.input.as_ref().expect("input initialized").end() >= self.required_end()?)
    }

    fn decode_next(&mut self) -> Result<Range<usize>, SstvError> {
        let clock = self.decode.clock.expect("clock acquired");
        let scan = Scan {
            raster: &self.raster,
            input: self.input.as_ref().expect("input initialized"),
            clock,
            source: Source::Live,
        };
        let rows = scan.draw(&mut self.decode)?;
        self.image_revision = self.image_revision.saturating_add(1);
        // One period of margin stays behind the current unit so that a live
        // phase correction, which may move the raster backwards, still reaches
        // the samples of the unit it corrects.
        let discard =
            clock.sample_at(self.raster.profile.period_ps * self.decode.raster_unit.saturating_sub(1) as u64)?;
        self.input.as_mut().expect("input initialized").discard_before(discard);
        Ok(rows)
    }

    fn deliver_row(&mut self, row: usize) -> RxEvent {
        self.decode.delivered_rows += 1;
        if self.decode.delivered_rows == self.raster.mode.spec().active_rows() as usize {
            self.decode.state = RxState::Complete;
        } else {
            self.decode.state = RxState::Decoding {
                completed_rows: self.decode.delivered_rows,
            };
        }
        RxEvent::RowDecoded { row }
    }

    fn queue_event(&mut self, event: RxEvent) {
        self.decode.pending_events.push_back(event);
    }

    /// Returns the next queued event without requiring an empty input block.
    pub fn poll_event(&mut self) -> Option<RxEvent> {
        self.decode.pending_events.pop_front().map(|event| self.emit(event))
    }

    fn emit(&mut self, event: RxEvent) -> RxEvent {
        match event {
            RxEvent::RowDecoded { row } => self.deliver_row(row),
            event => event,
        }
    }
}

/// Returns the rate error a live refit acts on after `units` raster units.
///
/// MMSSTV tightens this as lines accumulate, so early and noisy fits only
/// correct gross errors while later ones can refine the rate.
fn live_slant_threshold_ppm(units: usize) -> f64 {
    (LIVE_SLANT_THRESHOLD_SCALE / units.max(1) as f64).max(LIVE_SLANT_MIN_THRESHOLD_PPM)
}

mod rebuild;
mod scan;
mod sync_track;

use scan::{Raster, Scan, Source};

#[cfg(test)]
mod tests;
