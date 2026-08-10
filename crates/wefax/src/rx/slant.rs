use alloc::vec::Vec;

use crate::image::GrayRaster;

/// Lines between one look at the picture and the next.
const INTERVAL_LINES: usize = 16;
/// Lines the estimate is drawn from.
const WINDOW_LINES: usize = 64;
/// Lines between the two rows of a pair.
///
/// A wider baseline multiplies the displacement without multiplying the noise
/// that measuring it costs, which is the whole reason for pairing rows that
/// are not adjacent.
const BASELINE: usize = 8;
/// Lines between one pair and the next within the window.
const PAIR_STRIDE: usize = 4;
/// Largest displacement considered, in pixels.
const MAX_SHIFT: i64 = 24;
/// Number of shifts evaluated, which is the span either side plus zero.
const SHIFTS: usize = (MAX_SHIFT as usize) * 2 + 1;
/// Pairs needed before a median means anything.
const MIN_PAIRS: usize = 5;
/// Drift below which a correction is not worth the disturbance.
///
/// At IOC 576 and 120 lines per minute this is 27 parts per million, well
/// under the point where a reception visibly leans.
const MIN_DRIFT: f64 = 0.05;
/// Median absolute deviation above which the pairs did not agree.
const MAX_DISPERSION: f64 = 1.0;
/// Lines a correction suppresses the next one for.
const HOLDOFF_LINES: usize = 32;
/// Range of levels a row needs before it is worth correlating.
const MIN_ROW_CONTRAST: u8 = 8;
/// Share of the typical score the best one has to beat.
///
/// Every shift scores about the same when the rows do not line up anywhere in
/// range — because they match somewhere outside it, or because there is
/// nothing to match. The winner of a flat field is arbitrary, and an arbitrary
/// answer that several pairs happen to agree on would survive the dispersion
/// test and move the picture for no reason.
const MATCH_MARGIN: f64 = 0.9;

/// Refits the line clock from the picture, on the picture.
///
/// A line-length error displaces each row a little further than the one before
/// it, which is a shear and nothing else. Measuring it on the raster costs one
/// pass over a few megabytes; measuring it on the demodulated stream the way
/// the SSTV decoder does would mean retaining that stream, and twenty minutes
/// of it is over a hundred megabytes. So this reads the rows already drawn,
/// and the correction redraws them by rolling each one.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SlantTracker {
    next_check: usize,
    holdoff_until: usize,
}

impl SlantTracker {
    /// Returns the drift to apply in pixels per line, if the picture agreed.
    pub(crate) fn observe(&mut self, raster: &GrayRaster, line: usize) -> Option<f64> {
        if line < self.next_check.max(WINDOW_LINES) || line < self.holdoff_until {
            return None;
        }
        self.next_check = line + INTERVAL_LINES;

        let first = line - WINDOW_LINES;
        let mut displacements = Vec::with_capacity(WINDOW_LINES / PAIR_STRIDE);
        let mut row = first;
        while row + BASELINE < line {
            if let (Some(early), Some(late)) = (raster.row(row), raster.row(row + BASELINE))
                && has_contrast(early)
                && let Some(displacement) = displacement(early, late)
            {
                displacements.push(displacement);
            }
            row += PAIR_STRIDE;
        }
        if displacements.len() < MIN_PAIRS {
            return None;
        }

        let center = median(&mut displacements);
        // The median rather than the mean: a coastline or a front that really
        // does move sideways is one pair's answer, not the picture's.
        let mut deviations: Vec<f64> = displacements.iter().map(|value| (value - center).abs()).collect();
        if median(&mut deviations) > MAX_DISPERSION {
            return None;
        }
        let drift = center / BASELINE as f64;
        if drift.abs() < MIN_DRIFT {
            return None;
        }
        self.holdoff_until = line + HOLDOFF_LINES;
        Some(drift)
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Returns whether a row carries enough variation to be correlated.
fn has_contrast(row: &[u8]) -> bool {
    let (low, high) = row.iter().fold((u8::MAX, u8::MIN), |(low, high), level| {
        (low.min(*level), high.max(*level))
    });
    high.saturating_sub(low) >= MIN_ROW_CONTRAST
}

/// Returns how far `late` sits to the right of `early`, in pixels.
///
/// The rows wrap, because the raster's own phase does: a picture displaced far
/// enough leaves one edge and returns at the other.
fn displacement(early: &[u8], late: &[u8]) -> Option<f64> {
    let width = early.len();
    if width == 0 || late.len() != width || width as i64 <= MAX_SHIFT * 2 {
        return None;
    }
    let mut scores = [0.0_f64; SHIFTS];
    let mut best = 0;
    for index in 0..SHIFTS {
        let shift = index as i64 - MAX_SHIFT;
        let mut sum = 0_u64;
        for (offset, level) in early.iter().enumerate() {
            let other = (offset as i64 + shift).rem_euclid(width as i64) as usize;
            sum += u64::from(level.abs_diff(late[other]));
        }
        scores[index] = sum as f64;
        if scores[index] < scores[best] {
            best = index;
        }
    }
    // A minimum against the end of the range means the real one is outside it.
    if best == 0 || best == SHIFTS - 1 {
        return None;
    }
    let mut ranked = scores;
    ranked.sort_unstable_by(f64::total_cmp);
    if scores[best] > ranked[SHIFTS / 2] * MATCH_MARGIN {
        return None;
    }
    let (low, center, high) = (scores[best - 1], scores[best], scores[best + 1]);
    let curvature = low - 2.0 * center + high;
    let refinement = if curvature > 0.0 {
        (0.5 * (low - high) / curvature).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    Some((best as i64 - MAX_SHIFT) as f64 + refinement)
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A raster whose content shifts right by `drift` pixels per line.
    fn sheared(width: usize, height: usize, drift: f64) -> GrayRaster {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let base: Vec<u8> = (0..width)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 56) as u8
            })
            .collect();
        let rows: Vec<Vec<u8>> = (0..height)
            .map(|row| {
                let mut line = base.clone();
                let shift = libm::round(row as f64 * drift) as i64;
                line.rotate_right(shift.rem_euclid(width as i64) as usize);
                line
            })
            .collect();
        GrayRaster::from_rows(width, &rows).unwrap()
    }

    #[test]
    fn a_pair_of_rows_reports_how_far_apart_they_sit() {
        let raster = sheared(256, 20, 1.0);
        let found = displacement(raster.row(0).unwrap(), raster.row(8).unwrap()).unwrap();
        assert!((found - 8.0).abs() < 0.2, "found {found}");
    }

    #[test]
    fn a_displacement_outside_the_search_range_is_declined() {
        let raster = sheared(256, 40, 8.0);
        assert!(displacement(raster.row(0).unwrap(), raster.row(8).unwrap()).is_none());
    }

    #[test]
    fn the_drift_of_a_sheared_picture_is_measured() {
        for drift in [0.25_f64, 1.0, -0.5] {
            let raster = sheared(512, 80, drift);
            let mut tracker = SlantTracker::default();
            let found = tracker.observe(&raster, 79).expect("the picture agreed");
            assert!((found - drift).abs() < 0.05, "{drift} was measured as {found}");
        }
    }

    #[test]
    fn a_picture_with_nothing_to_correlate_is_left_alone() {
        let rows = vec![vec![200_u8; 512]; 80];
        let raster = GrayRaster::from_rows(512, &rows).unwrap();
        let mut tracker = SlantTracker::default();
        assert!(tracker.observe(&raster, 79).is_none());
    }

    #[test]
    fn a_picture_that_is_not_leaning_is_left_alone() {
        let raster = sheared(512, 80, 0.0);
        let mut tracker = SlantTracker::default();
        assert!(tracker.observe(&raster, 79).is_none());
    }

    #[test]
    fn too_few_lines_is_not_enough_to_look_at() {
        let raster = sheared(512, 40, 1.0);
        let mut tracker = SlantTracker::default();
        assert!(tracker.observe(&raster, 39).is_none());
    }

    #[test]
    fn a_correction_suppresses_the_next_one() {
        let raster = sheared(512, 200, 1.0);
        let mut tracker = SlantTracker::default();
        assert!(tracker.observe(&raster, 79).is_some());
        for line in 80..79 + HOLDOFF_LINES {
            assert!(tracker.observe(&raster, line).is_none(), "line {line} was not held off");
        }
        assert!(tracker.observe(&raster, 79 + HOLDOFF_LINES).is_some());
    }

    #[test]
    fn the_median_of_an_even_count_is_the_middle_of_the_two() {
        assert_eq!(median(&mut [3.0, 1.0, 4.0, 2.0]), 2.5);
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
    }
}
