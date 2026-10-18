use core::ops::Range;

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct Raster {
    pub(super) mode: Mode,
    pub(super) profile: RasterProfile,
    pub(super) width: usize,
}

impl Raster {
    pub(super) fn units(&self) -> usize {
        let spec = self.mode.spec();
        spec.active_rows() as usize / spec.rows_per_raster_unit() as usize
    }

    pub(super) fn rows_per_unit(&self) -> usize {
        self.mode.spec().rows_per_raster_unit() as usize
    }

    pub(super) fn segment(&self, channel: ScanChannel, row_offset: u8) -> Result<PixelSegment, SstvError> {
        self.profile
            .pixels()
            .get(channel, row_offset)
            .ok_or(SstvError::UnsupportedRxMode(self.mode))
    }

    /// Returns the half-open sample range averaged for one transmitted pixel.
    ///
    /// A narrow edge guard excludes samples that can belong to an adjacent
    /// component after sub-sample rounding. Pixels too short to contain a
    /// guarded sample fall back to the reading nearest their centre.
    pub(super) fn pixel_window(
        &self,
        clock: RasterClock,
        unit: usize,
        segment: PixelSegment,
        x: usize,
    ) -> Result<(u64, u64), SstvError> {
        let width = self.width as u128;
        let base = u128::from(self.profile.period_ps) * unit as u128 + u128::from(segment.start_ps);
        let edge = |index: u128| -> Result<f64, SstvError> {
            let protocol = base + u128::from(segment.duration_ps) * index / width;
            let protocol = u64::try_from(protocol).map_err(|_| SstvError::TimeOverflow)?;
            clock.position_at(protocol)
        };
        let left = edge(x as u128)?;
        let right = edge(x as u128 + 1)?;
        let guard = (right - left) * PIXEL_GUARD;
        let first = ceil_sample(left + guard);
        let end = ceil_sample(right - guard);
        if end > first {
            Ok((first, end))
        } else {
            let center = ceil_sample((left + right) * 0.5 - 0.5);
            Ok((center, center + 1))
        }
    }

    pub(super) fn selector_window(&self, clock: RasterClock, unit: usize) -> Result<(u64, u64), SstvError> {
        let (start_ps, end_ps) = self
            .profile
            .selector_window_ps()
            .ok_or(SstvError::UnsupportedRxMode(self.mode))?;
        let unit_start = self
            .profile
            .period_ps
            .checked_mul(unit as u64)
            .ok_or(SstvError::TimeOverflow)?;
        let edge = |offset_ps: u64| -> Result<u64, SstvError> {
            clock.sample_from(unit_start.checked_add(offset_ps).ok_or(SstvError::TimeOverflow)?)
        };
        let first = edge(start_ps)?;
        let last = edge(end_ps)?;
        Ok((first, last.max(first.saturating_add(1))))
    }

    pub(super) fn frequency_to_level(&self, frequency_hz: f64) -> u8 {
        let hz = if frequency_hz > 0.0 {
            (frequency_hz + 0.5) as u32
        } else {
            0
        };
        self.mode
            .spec()
            .signal_band()
            .frequency_to_level(Frequency::from_hz(hz))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Source {
    Live,
    Staged,
}

#[derive(Clone, Copy)]
pub(super) struct Scan<'a> {
    pub(super) raster: &'a Raster,
    pub(super) input: &'a SampleBuffer,
    pub(super) clock: RasterClock,
    pub(super) source: Source,
}

impl Scan<'_> {
    pub(super) fn draw(&self, pass: &mut DecodeState) -> Result<Range<usize>, SstvError> {
        let raster = self.raster;
        let width = raster.width;
        let unit = pass.raster_unit;
        let row = unit * raster.rows_per_unit();
        let rows = match raster.profile.organization {
            RasterOrganization::DirectGbr => {
                let green = raster.segment(ScanChannel::Green, 0)?;
                let blue = raster.segment(ScanChannel::Blue, 0)?;
                let red = raster.segment(ScanChannel::Red, 0)?;
                for x in 0..width {
                    let g = self.level_at(unit, green, x)?;
                    let b = self.level_at(unit, blue, x)?;
                    let r = self.level_at(unit, red, x)?;
                    set_pixel(&mut pass.image, x, unit, Rgb8::new(r, g, b));
                }
                row..row + 1
            }
            RasterOrganization::YCrCb => {
                let luminance = raster.segment(ScanChannel::Luminance, 0)?;
                let red = raster.segment(ScanChannel::RedDifference, 0)?;
                let blue = raster.segment(ScanChannel::BlueDifference, 0)?;
                for x in 0..width {
                    let y = self.level_at(unit, luminance, x)?;
                    let cr = self.level_at(unit, red, x)?;
                    let cb = self.level_at(unit, blue, x)?;
                    set_pixel(&mut pass.image, x, unit, y_cr_cb_to_rgb(YCrCb8 { y, cr, cb }));
                }
                row..row + 1
            }
            RasterOrganization::AlternatingYCrCb => {
                // Only one chrominance plane arrives per raster unit, so every
                // row is drawn from the received plane and the retained one.
                let luminance = raster.segment(ScanChannel::Luminance, 0)?;
                let chroma = raster.segment(ScanChannel::RedDifference, 0)?;
                let selector = self.robot_selector_at(unit)?.unwrap_or_else(|| {
                    pass.robot_selector
                        .map(RobotSelector::opposite)
                        .unwrap_or(if unit & 1 == 0 {
                            RobotSelector::Cb
                        } else {
                            RobotSelector::Cr
                        })
                });
                for x in 0..width {
                    let y = self.level_at(unit, luminance, x)?;
                    let received = self.level_at(unit, chroma, x)?;
                    pass.robot_chroma[selector.plane()][x] = received;
                    let cr = pass.robot_chroma[0][x];
                    let cb = pass.robot_chroma[1][x];
                    set_pixel(&mut pass.image, x, unit, y_cr_cb_to_rgb(YCrCb8 { y, cr, cb }));
                }
                pass.robot_selector = Some(selector);
                row..row + 1
            }
            RasterOrganization::PairedYCrCb => {
                let first = raster.segment(ScanChannel::Luminance, 0)?;
                let red = raster.segment(ScanChannel::RedDifference, 0)?;
                let blue = raster.segment(ScanChannel::BlueDifference, 0)?;
                let second = raster.segment(ScanChannel::Luminance, 1)?;
                let row = unit * 2;
                for x in 0..width {
                    let y0 = self.level_at(unit, first, x)?;
                    let cr = self.level_at(unit, red, x)?;
                    let cb = self.level_at(unit, blue, x)?;
                    let y1 = self.level_at(unit, second, x)?;
                    set_pixel(&mut pass.image, x, row, y_cr_cb_to_rgb(YCrCb8 { y: y0, cr, cb }));
                    set_pixel(&mut pass.image, x, row + 1, y_cr_cb_to_rgb(YCrCb8 { y: y1, cr, cb }));
                }
                row..row + 2
            }
            RasterOrganization::DirectRgb | RasterOrganization::PairedLuminance => {
                return Err(SstvError::UnsupportedRxMode(raster.mode));
            }
        };
        pass.raster_unit += 1;
        Ok(rows)
    }

    fn mean_frequency(&self, range: (u64, u64)) -> Result<f64, SstvError> {
        let (first, end) = range;
        match self.input.frequency_sum(first, end) {
            Some(sum) => Ok(sum / (end - first) as f64),
            None => {
                let sample = if first < self.input.first() {
                    first
                } else {
                    first.max(self.input.end())
                };
                Err(match self.source {
                    Source::Staged => SstvError::InsufficientStagedData {
                        required_sample: sample,
                    },
                    Source::Live => SstvError::SampleDiscarded { sample },
                })
            }
        }
    }

    fn level_at(&self, unit: usize, segment: PixelSegment, x: usize) -> Result<u8, SstvError> {
        let window = self.raster.pixel_window(self.clock, unit, segment, x)?;
        // Averaging most of the transmitted pixel interval suppresses random
        // frequency noise without reducing the demodulated stream's sample rate.
        Ok(self.raster.frequency_to_level(self.mean_frequency(window)?))
    }

    pub(super) fn robot_selector_at(&self, unit: usize) -> Result<Option<RobotSelector>, SstvError> {
        let average = self.mean_frequency(self.raster.selector_window(self.clock, unit)?)?;
        Ok(if average < 1700.0 {
            Some(RobotSelector::Cr)
        } else if average >= 2100.0 {
            Some(RobotSelector::Cb)
        } else {
            None
        })
    }
}

fn set_pixel(image: &mut RgbImage, x: usize, y: usize, pixel: Rgb8) {
    if let Some(value) = image.row_mut(y).and_then(|row| row.get_mut(x)) {
        *value = pixel;
    }
}
