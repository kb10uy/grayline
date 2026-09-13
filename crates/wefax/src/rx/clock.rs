use crate::error::WefaxError;

/// Where each line of the raster begins, in absolute samples.
///
/// Every position is computed by multiplying from the epoch rather than by
/// advancing a running total. A WEFAX reception is thousands of lines long, so
/// an error added once per line would accumulate into a visible slant on its
/// own, quite apart from the clock error the slant tracker exists to correct.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineClock {
    epoch_samples: f64,
    samples_per_line: f64,
}

impl LineClock {
    /// Creates a clock whose line zero begins at `epoch_samples`.
    pub fn new(epoch_samples: f64, samples_per_line: f64) -> Result<Self, WefaxError> {
        if !epoch_samples.is_finite() || !samples_per_line.is_finite() || samples_per_line <= 0.0 {
            return Err(WefaxError::InvalidLineClock);
        }
        Ok(Self {
            epoch_samples,
            samples_per_line,
        })
    }

    /// Returns the absolute sample position a fraction of the way into a line.
    pub fn position_at(&self, line: usize, fraction: f64) -> f64 {
        self.epoch_samples + (line as f64 + fraction) * self.samples_per_line
    }

    /// Returns the fractional line number a sample position falls on.
    pub fn line_at(&self, sample: f64) -> f64 {
        (sample - self.epoch_samples) / self.samples_per_line
    }

    /// Returns where line zero begins, in absolute samples.
    pub const fn epoch_samples(&self) -> f64 {
        self.epoch_samples
    }

    /// Returns the current line length in samples.
    pub const fn samples_per_line(&self) -> f64 {
        self.samples_per_line
    }

    /// Changes the line length while leaving `pivot_line` where it is.
    ///
    /// Correcting the rate would otherwise move every line already decoded,
    /// including the ones behind the correction that were decoded correctly
    /// under the old figure.
    pub fn set_samples_per_line(&mut self, samples_per_line: f64, pivot_line: usize) -> Result<(), WefaxError> {
        if !samples_per_line.is_finite() || samples_per_line <= 0.0 {
            return Err(WefaxError::InvalidLineClock);
        }
        let pivot = self.position_at(pivot_line, 0.0);
        self.samples_per_line = samples_per_line;
        self.epoch_samples = pivot - pivot_line as f64 * samples_per_line;
        Ok(())
    }

    /// Moves every line by `samples`, positive for later.
    pub fn shift(&mut self, samples: f64) -> Result<(), WefaxError> {
        if !samples.is_finite() {
            return Err(WefaxError::InvalidLineClock);
        }
        self.epoch_samples += samples;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_advance_by_exactly_one_line() {
        let clock = LineClock::new(100.0, 24_000.0).unwrap();
        assert_eq!(clock.position_at(0, 0.0), 100.0);
        assert_eq!(clock.position_at(1, 0.0), 24_100.0);
        assert_eq!(clock.position_at(0, 0.5), 12_100.0);
    }

    #[test]
    fn a_fractional_line_length_does_not_accumulate() {
        let clock = LineClock::new(0.0, 5_512.5).unwrap();
        assert_eq!(clock.position_at(4_000, 0.0), 22_050_000.0);
    }

    #[test]
    fn line_at_inverts_position_at() {
        let clock = LineClock::new(731.25, 5_512.5).unwrap();
        for line in [0, 1, 97, 4_000] {
            assert_eq!(clock.line_at(clock.position_at(line, 0.0)), line as f64);
        }
    }

    #[test]
    fn a_rate_change_leaves_its_pivot_where_it_was() {
        let mut clock = LineClock::new(100.0, 24_000.0).unwrap();
        let pivot = clock.position_at(50, 0.0);
        clock.set_samples_per_line(24_012.0, 50).unwrap();
        assert_eq!(clock.position_at(50, 0.0), pivot);
        assert_eq!(clock.position_at(51, 0.0), pivot + 24_012.0);
        assert_eq!(clock.position_at(49, 0.0), pivot - 24_012.0);
    }

    #[test]
    fn shifting_moves_every_line_together() {
        let mut clock = LineClock::new(100.0, 24_000.0).unwrap();
        clock.shift(-40.0).unwrap();
        assert_eq!(clock.position_at(0, 0.0), 60.0);
        assert_eq!(clock.position_at(9, 0.0), 216_060.0);
    }

    #[test]
    fn an_unusable_clock_is_rejected() {
        assert_eq!(LineClock::new(0.0, 0.0).unwrap_err(), WefaxError::InvalidLineClock);
        assert_eq!(
            LineClock::new(f64::NAN, 100.0).unwrap_err(),
            WefaxError::InvalidLineClock
        );
        let mut clock = LineClock::new(0.0, 100.0).unwrap();
        assert_eq!(
            clock.set_samples_per_line(-1.0, 0).unwrap_err(),
            WefaxError::InvalidLineClock
        );
        assert_eq!(clock.shift(f64::INFINITY).unwrap_err(), WefaxError::InvalidLineClock);
    }
}
