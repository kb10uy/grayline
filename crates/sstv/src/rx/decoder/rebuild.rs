//! Refitting a finished reception from the samples it was staged from.
//!
//! The live clock is fitted from a handful of periods at startup, so a raster
//! decoded against it slants. These rebuild the image against a clock refitted
//! over the whole reception.

use alloc::vec::Vec;

use super::*;

impl RxDecoder {
    /// Retains contiguous samples after normal completion for staged refinement.
    ///
    /// Offline sources can provide their remaining tail after the active image
    /// has completed so a fitted raster clock still has enough guarded coverage.
    pub fn stage_for_refinement(&mut self, block: DemodulatedBlock<'_>) -> Result<(), SstvError> {
        if self.decode.state != RxState::Complete {
            return Err(SstvError::RxNotComplete);
        }
        block.validate_continuity(self.next_sample)?;
        block.validate_range(0, block.frequency_hz().len())?;
        let Some(staged) = self.staged.as_mut() else {
            return Err(SstvError::StagingDisabled);
        };
        let max_samples = match self.config.staging {
            Staging::Memory { max_samples } => max_samples,
            Staging::Disabled => return Err(SstvError::StagingDisabled),
        };
        if staged
            .len()
            .checked_add(block.frequency_hz().len())
            .is_none_or(|length| length > max_samples)
        {
            return Err(SstvError::StagingCapacityExceeded { max_samples });
        }
        staged.append(block, block.frequency_hz().len());
        self.next_sample = Some(
            block
                .first_sample()
                .checked_add(block.frequency_hz().len() as u64)
                .ok_or(SstvError::SamplePositionOverflow)?,
        );
        Ok(())
    }
    /// Re-estimates global rate and epoch and rebuilds from immutable staged samples.
    ///
    /// This remains available after normal completion. It does not apply local
    /// warping and never mutates the retained demodulated samples.
    /// A successful rebuild is terminal and sets the decoder state to
    /// [`RxState::Complete`], so callers must collect the complete transmission
    /// before invoking this method.
    pub fn refine_staged(&mut self) -> Result<RefinementResult, SstvError> {
        let Some(staged) = self.staged.as_ref() else {
            return Err(SstvError::StagingDisabled);
        };
        let estimator =
            SlantEstimator::for_mode(self.sample_rate_hz, self.raster.mode).expect("decoder mode has a raster profile");
        let acquisition_clock = acquire_full(
            staged,
            self.raster.profile,
            self.sample_rate_hz,
            self.config.sync_detector_delay,
        )?;
        let observations: Vec<_> = (0..self.raster.units())
            .filter_map(|unit| {
                observe(
                    staged,
                    self.raster.profile,
                    acquisition_clock,
                    unit,
                    self.sample_rate_hz,
                    self.config.sync_detector_delay,
                )
            })
            .collect();
        let slant = estimator
            .estimate(&observations)
            .ok_or(SstvError::InsufficientStagedSync)?;
        let clock = RasterClock::from_estimate(slant.source_epoch, slant.effective_sample_rate_hz)?;
        self.validate_staged_coverage(staged, clock, self.raster.units())?;
        let mut candidate = self.redraw(staged, clock, self.raster.units())?;
        candidate.delivered_rows = self.raster.mode.spec().active_rows() as usize;
        candidate.state = RxState::Complete;
        self.decode = candidate;
        self.image_revision = self.image_revision.saturating_add(1);
        Ok(RefinementResult {
            revision: self.image_revision,
            slant,
        })
    }
    /// Refits the raster rate from the sync collected so far.
    ///
    /// This is MMSSTV's real-time slant adjustment: rather than waiting for the
    /// transmission to end, the rate is re-estimated as lines accumulate and,
    /// when it has moved far enough to matter, the rows already decoded are
    /// redrawn from the retained samples so the whole image stays consistent.
    pub(super) fn track_slant(&mut self) -> Result<(), SstvError> {
        if !self.config.live_slant || self.staged.is_none() {
            return Ok(());
        }
        let units = self.decode.raster_unit;
        if units < LIVE_SLANT_MIN_UNITS {
            return Ok(());
        }
        let estimator =
            SlantEstimator::for_mode(self.sample_rate_hz, self.raster.mode).expect("decoder mode has a raster profile");
        let Some(estimate) = estimator.estimate(&self.staged_observations) else {
            return Ok(());
        };
        if self.rate_estimates.len() == LIVE_SLANT_SMOOTHING {
            self.rate_estimates.pop_front();
        }
        self.rate_estimates.push_back(estimate.effective_sample_rate_hz);
        if self
            .last_slant_unit
            .is_some_and(|last| units < last + LIVE_SLANT_HOLDOFF_UNITS)
        {
            return Ok(());
        }
        let smoothed = self.rate_estimates.iter().sum::<f64>() / self.rate_estimates.len() as f64;
        let current = self.decode.clock.expect("clock acquired").effective_sample_rate_hz();
        if (smoothed / current - 1.0).abs() * 1.0e6 < live_slant_threshold_ppm(units) {
            return Ok(());
        }
        let refit = RasterClock::from_estimate(estimate.source_epoch, smoothed)?;
        // A refit the retained samples cannot cover is simply left for a later
        // attempt, so tracking never destroys a usable image.
        if let Ok(redrawn) = self.rebuild_live(refit, units) {
            self.last_slant_unit = Some(redrawn);
            self.rate_estimates.clear();
            self.phase_displacements.clear();
            self.last_phase_adjustment = None;
            self.queue_event(RxEvent::SlantAdjusted {
                unit: redrawn,
                effective_sample_rate_hz: smoothed,
            });
        }
        Ok(())
    }
    /// Redraws the decoded raster under `clock` and resumes decoding.
    ///
    /// Returns the number of units redrawn, which may be fewer than `units`
    /// when the refit reaches past the samples received so far.
    pub(super) fn rebuild_live(&mut self, clock: RasterClock, units: usize) -> Result<usize, SstvError> {
        let staged = self.staged.as_ref().expect("staging checked by the caller");
        let units = self.covered_units(staged, clock, units);
        if units == 0 {
            return Err(SstvError::InsufficientStagedSync);
        }
        let mut candidate = self.redraw(staged, clock, units)?;
        // Decoding continues from the refitted clock, so the working window is
        // taken again from the retained samples rather than from the window
        // that was trimmed against the old one.
        let resume = clock.sample_at(
            self.raster
                .profile
                .period_ps
                .checked_mul(units as u64)
                .ok_or(SstvError::TimeOverflow)?,
        )?;
        let input = staged.tail_from(resume);
        // Rows are delivered by the live path, not by a rebuild, so the
        // delivery bookkeeping is restated for the units that were redrawn.
        // Row events still queued deliver later and count themselves then, so
        // the restatement leaves them out rather than counting their rows
        // twice and completing the reception early.
        let queued_rows = self
            .decode
            .pending_events
            .iter()
            .filter(|event| matches!(event, RxEvent::RowDecoded { .. }))
            .count();
        candidate.delivered_rows = (units * self.raster.rows_per_unit()).saturating_sub(queued_rows);
        candidate.state = RxState::Decoding {
            completed_rows: candidate.delivered_rows,
        };
        candidate.pending_events = core::mem::take(&mut self.decode.pending_events);
        self.decode = candidate;
        self.input = Some(input);
        self.image_revision = self.image_revision.saturating_add(1);
        Ok(units)
    }
    fn redraw(&self, staged: &SampleBuffer, clock: RasterClock, units: usize) -> Result<DecodeState, SstvError> {
        let mut candidate = DecodeState::new(self.decode.image.size());
        candidate.clock = Some(clock);
        let scan = Scan {
            raster: &self.raster,
            input: staged,
            clock,
            source: Source::Staged,
        };
        while candidate.raster_unit < units {
            scan.draw(&mut candidate)?;
        }
        Ok(candidate)
    }
    /// Returns how many leading raster units `staged` covers under `clock`.
    ///
    /// A refit that runs faster than the current estimate places already
    /// decoded units later in the stream, sometimes past the samples received
    /// so far. Those units are redrawn as the audio arrives, so the correction
    /// applies to what is covered instead of being rejected outright.
    pub(super) fn covered_units(&self, staged: &SampleBuffer, clock: RasterClock, units: usize) -> usize {
        (0..units)
            .take_while(|unit| self.unit_is_covered(staged, clock, *unit))
            .count()
    }
    pub(super) fn unit_is_covered(&self, staged: &SampleBuffer, clock: RasterClock, unit: usize) -> bool {
        let width = self.raster.width;
        let covered = |sample: u64| staged.frequency(sample).is_some();
        for segment in self.raster.profile.pixels().iter() {
            let Ok((first, _)) = self.raster.pixel_window(clock, unit, segment, 0) else {
                return false;
            };
            let Ok((_, end)) = self.raster.pixel_window(clock, unit, segment, width - 1) else {
                return false;
            };
            if !covered(first) || !covered(end - 1) {
                return false;
            }
        }
        if self.raster.profile.selector_window_ps().is_some() {
            let Ok((first, end)) = self.raster.selector_window(clock, unit) else {
                return false;
            };
            if !covered(first) || !covered(end - 1) {
                return false;
            }
        }
        true
    }
    pub(super) fn validate_staged_coverage(
        &self,
        staged: &SampleBuffer,
        clock: RasterClock,
        units: usize,
    ) -> Result<(), SstvError> {
        let width = self.raster.width;
        let require = |sample: u64| -> Result<(), SstvError> {
            if staged.frequency(sample).is_some() {
                Ok(())
            } else {
                Err(SstvError::InsufficientStagedData {
                    required_sample: sample,
                })
            }
        };
        for unit in 0..units {
            // Sampling positions increase monotonically, so covering the first
            // and last sample of every segment covers everything between them.
            for segment in self.raster.profile.pixels().iter() {
                let (first, _) = self.raster.pixel_window(clock, unit, segment, 0)?;
                let (_, end) = self.raster.pixel_window(clock, unit, segment, width - 1)?;
                require(first)?;
                require(end - 1)?;
            }
            if self.raster.profile.selector_window_ps().is_some() {
                let (first, end) = self.raster.selector_window(clock, unit)?;
                require(first)?;
                require(end - 1)?;
            }
        }
        Ok(())
    }
}
