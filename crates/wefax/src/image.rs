use alloc::{vec, vec::Vec};
use core::ops::Range;

use crate::error::WefaxError;

/// An owned, row-major eight-bit grayscale raster whose height grows as lines
/// arrive.
///
/// A WEFAX transmission does not declare its length, so the height is not
/// known until the stop tone. Rows are reserved a block at a time rather than
/// by doubling: doubling leaves up to half the allocation unused and copies
/// everything retained so far each time it grows, and a chart running for
/// twenty minutes would do that a dozen times over.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrayRaster {
    width: usize,
    height: usize,
    max_height: usize,
    growth: usize,
    pixels: Vec<u8>,
}

impl GrayRaster {
    /// Creates an empty raster that will grow `growth` rows at a time up to
    /// `max_height`.
    pub fn new(width: usize, max_height: usize, growth: usize) -> Result<Self, WefaxError> {
        if width == 0 || max_height == 0 || growth == 0 {
            return Err(WefaxError::InvalidRasterSize);
        }
        let reserved = growth.min(max_height).saturating_mul(width);
        Ok(Self {
            width,
            height: 0,
            max_height,
            growth,
            pixels: Vec::with_capacity(reserved),
        })
    }

    /// Returns the pixels in one row.
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Returns how many rows have been appended.
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Returns the row limit this raster was created with.
    pub const fn max_height(&self) -> usize {
        self.max_height
    }

    /// Appends one black row and returns its index.
    pub fn push_row(&mut self) -> Result<usize, WefaxError> {
        if self.height >= self.max_height {
            return Err(WefaxError::LineLimitReached {
                max_lines: self.max_height,
            });
        }
        if self.pixels.len() == self.pixels.capacity() {
            let remaining = self.max_height - self.height;
            self.pixels.reserve_exact(self.growth.min(remaining) * self.width);
        }
        self.pixels.resize(self.pixels.len() + self.width, 0);
        let row = self.height;
        self.height += 1;
        Ok(row)
    }

    /// Returns one row, or `None` past the last one appended.
    pub fn row(&self, y: usize) -> Option<&[u8]> {
        self.rows(y..y.checked_add(1)?)
    }

    /// Returns one row for writing, or `None` past the last one appended.
    pub fn row_mut(&mut self, y: usize) -> Option<&mut [u8]> {
        if y >= self.height {
            return None;
        }
        let start = y * self.width;
        Some(&mut self.pixels[start..start + self.width])
    }

    /// Returns a contiguous run of rows, for a caller publishing only what is
    /// new since it last looked.
    pub fn rows(&self, range: Range<usize>) -> Option<&[u8]> {
        if range.start > range.end || range.end > self.height {
            return None;
        }
        Some(&self.pixels[range.start * self.width..range.end * self.width])
    }

    /// Returns every pixel appended so far, row-major.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Rotates every row left by `pixels`, negative for right.
    pub fn roll(&mut self, pixels: i64) {
        let shift = self.wrap(pixels);
        if shift == 0 {
            return;
        }
        for y in 0..self.height {
            let start = y * self.width;
            self.pixels[start..start + self.width].rotate_left(shift);
        }
    }

    /// Rolls row `y` left by `(y - pivot) * pixels_per_line`, rounded.
    ///
    /// A line clock running fast or slow displaces each row a little further
    /// than the one before it, so undoing that error on rows already decoded
    /// is a shear about whichever row the corrected clock was pivoted on.
    pub fn shear(&mut self, pivot: usize, pixels_per_line: f64) {
        if !pixels_per_line.is_finite() || pixels_per_line == 0.0 {
            return;
        }
        for y in 0..self.height {
            let offset = libm::round((y as f64 - pivot as f64) * pixels_per_line);
            let shift = self.wrap(offset as i64);
            if shift == 0 {
                continue;
            }
            let start = y * self.width;
            self.pixels[start..start + self.width].rotate_left(shift);
        }
    }

    /// Reduces a signed displacement to the left rotation that produces it.
    fn wrap(&self, pixels: i64) -> usize {
        pixels.rem_euclid(self.width as i64) as usize
    }
}

/// Builds a raster from rows, for a test or an offline caller that already
/// holds a complete picture.
impl GrayRaster {
    /// Creates a raster holding `rows` copies of `row`, for tests and offline
    /// callers that already hold a complete picture.
    pub fn from_rows(width: usize, rows: &[Vec<u8>]) -> Result<Self, WefaxError> {
        if width == 0 || rows.is_empty() || rows.iter().any(|row| row.len() != width) {
            return Err(WefaxError::InvalidRasterSize);
        }
        let mut pixels = vec![0; width * rows.len()];
        for (target, source) in pixels.chunks_exact_mut(width).zip(rows) {
            target.copy_from_slice(source);
        }
        Ok(Self {
            width,
            height: rows.len(),
            max_height: rows.len(),
            growth: rows.len(),
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(width: usize, rows: usize) -> GrayRaster {
        let mut raster = GrayRaster::new(width, rows, rows).unwrap();
        for y in 0..rows {
            raster.push_row().unwrap();
            for (x, pixel) in raster.row_mut(y).unwrap().iter_mut().enumerate() {
                *pixel = (x % 251) as u8;
            }
        }
        raster
    }

    #[test]
    fn invalid_dimensions_are_rejected() {
        assert_eq!(GrayRaster::new(0, 10, 4).unwrap_err(), WefaxError::InvalidRasterSize);
        assert_eq!(GrayRaster::new(10, 0, 4).unwrap_err(), WefaxError::InvalidRasterSize);
        assert_eq!(GrayRaster::new(10, 10, 0).unwrap_err(), WefaxError::InvalidRasterSize);
    }

    #[test]
    fn rows_are_reserved_a_block_at_a_time() {
        let mut raster = GrayRaster::new(8, 100, 4).unwrap();
        assert_eq!(raster.pixels.capacity(), 32);
        for _ in 0..4 {
            raster.push_row().unwrap();
        }
        assert_eq!(raster.pixels.capacity(), 32);
        raster.push_row().unwrap();
        assert_eq!(raster.pixels.capacity(), 64);
        assert_eq!(raster.height(), 5);
    }

    #[test]
    fn the_last_block_is_bounded_by_the_line_limit() {
        let mut raster = GrayRaster::new(8, 6, 4).unwrap();
        for _ in 0..6 {
            raster.push_row().unwrap();
        }
        assert_eq!(raster.pixels.capacity(), 48);
        assert_eq!(
            raster.push_row().unwrap_err(),
            WefaxError::LineLimitReached { max_lines: 6 }
        );
    }

    #[test]
    fn rows_outside_the_decoded_height_are_absent() {
        let raster = filled(8, 3);
        assert!(raster.row(2).is_some());
        assert!(raster.row(3).is_none());
        assert!(raster.rows(1..3).is_some());
        assert!(raster.rows(1..4).is_none());
        assert_eq!(raster.rows(2..2).unwrap().len(), 0);
    }

    #[test]
    fn rolling_wraps_in_both_directions() {
        let original = filled(16, 4);
        let mut raster = original.clone();
        raster.roll(5);
        assert_ne!(raster, original);
        raster.roll(-5);
        assert_eq!(raster, original);

        let mut whole = original.clone();
        whole.roll(16);
        assert_eq!(whole, original);
    }

    #[test]
    fn an_integer_shear_matches_rolling_each_row_by_hand() {
        let original = filled(32, 9);
        let mut sheared = original.clone();
        sheared.shear(4, 3.0);

        let mut expected = original.clone();
        for y in 0..expected.height() {
            let shift = ((y as i64 - 4) * 3).rem_euclid(32) as usize;
            expected.row_mut(y).unwrap().rotate_left(shift);
        }
        assert_eq!(sheared, expected);
    }

    #[test]
    fn a_shear_leaves_its_pivot_row_alone() {
        let original = filled(32, 9);
        let mut sheared = original.clone();
        sheared.shear(4, 0.25);
        assert_eq!(sheared.row(4), original.row(4));
        assert_ne!(sheared.row(8), original.row(8));
    }

    #[test]
    fn a_zero_or_non_finite_shear_changes_nothing() {
        let original = filled(32, 9);
        let mut raster = original.clone();
        raster.shear(4, 0.0);
        raster.shear(4, f64::NAN);
        assert_eq!(raster, original);
    }

    #[test]
    fn from_rows_requires_a_rectangular_picture() {
        let rows = vec![vec![1_u8; 4], vec![2_u8; 4]];
        let raster = GrayRaster::from_rows(4, &rows).unwrap();
        assert_eq!(raster.height(), 2);
        assert_eq!(raster.row(1).unwrap(), &[2, 2, 2, 2]);

        let ragged = vec![vec![1_u8; 4], vec![2_u8; 3]];
        assert_eq!(
            GrayRaster::from_rows(4, &ragged).unwrap_err(),
            WefaxError::InvalidRasterSize
        );
    }
}
