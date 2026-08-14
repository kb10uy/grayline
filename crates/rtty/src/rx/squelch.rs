use grayline_dsp::filter::MovingAverage;

/// How many 100 ms peak windows the strength average spans
/// (`SmoozSQ.SetCount(8)`), about 0.8 s of response.
const STRENGTH_WINDOWS: usize = 8;

/// Signal strength and squelch on the channel difference (`Rtty.cpp:851`).
///
/// Tracks the peak of `|mark − space|` over 100 ms windows and averages the
/// last eight peaks. The threshold lives on the normalized scale directly;
/// MMTTY's ten-fold threshold raise while its limiter runs is not ported,
/// because this receive path always normalizes.
#[derive(Clone, Debug)]
pub(crate) struct Squelch {
    threshold: Option<f64>,
    window_samples: u64,
    counter: u64,
    peak: f64,
    smoother: MovingAverage,
    average: f64,
}

impl Squelch {
    pub(crate) fn new(sample_rate_hz: f64, threshold: Option<f64>) -> Self {
        Self {
            threshold,
            window_samples: (sample_rate_hz / 10.0) as u64,
            counter: 0,
            peak: 0.0,
            smoother: MovingAverage::with_window(STRENGTH_WINDOWS).expect("window is a positive constant"),
            average: 0.0,
        }
    }

    /// Tracks one strength sample and returns whether the squelch is open.
    pub(crate) fn process_sample(&mut self, strength: f64) -> bool {
        self.peak = self.peak.max(strength);
        if self.counter == 0 {
            self.counter = self.window_samples;
            self.average = self.smoother.process_sample(self.peak);
            self.peak = 0.0;
        }
        self.counter -= 1;
        self.threshold.is_none_or(|threshold| self.average >= threshold)
    }

    /// Returns the smoothed strength reading.
    pub(crate) fn signal_strength(&self) -> f64 {
        self.average
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strong_difference_opens_and_silence_closes() {
        let mut squelch = Squelch::new(11_025.0, Some(0.05));
        let mut open = false;
        for _ in 0..22_050 {
            open = squelch.process_sample(0.5);
        }
        assert!(open);
        assert!(squelch.signal_strength() > 0.4);
        for _ in 0..22_050 {
            open = squelch.process_sample(0.0);
        }
        assert!(!open);
    }

    #[test]
    fn no_threshold_is_always_open() {
        let mut squelch = Squelch::new(11_025.0, None);
        assert!(squelch.process_sample(0.0));
    }
}
