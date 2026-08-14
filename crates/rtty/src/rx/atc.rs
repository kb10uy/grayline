use grayline_dsp::filter::{Iir, IirLowPassDesign, IirResponse};

use crate::{RttyError, rx::config::AtcDesign};

/// The target level the corrector re-centers a channel on.
///
/// MMTTY's `ATCC` is 8192 on its limiter's ±16384 integer scale; mapping
/// 16384 to 1.0 puts it at 0.5 here, and `ATCW` (1024) at 0.0625.
const CENTER: f64 = 0.5;
/// Half the minimum extremum spread the corrector assumes.
const MINIMUM_SPREAD: f64 = 0.062_5;
/// Samples per extremum block, MMTTY's own count.
const BLOCK_SAMPLES: u32 = 64;
/// The most remembered blocks the original's fixed lists allow.
const MAXIMUM_BLOCKS: usize = 16;
/// How much the corrector expands a value away from the threshold.
const EXPANSION: f64 = 1.1;
/// Cutoff of the threshold smoothing filter in hertz (`CATC::CATC`).
const THRESHOLD_CUTOFF_HZ: f64 = 100.0;

/// The automatic threshold corrector, MMTTY's `CATC` (`Rtty.cpp:1860`).
///
/// Tracks the extremes of one channel over a short list of 64-sample blocks,
/// smooths the midpoint of that range, and re-centers and expands the signal
/// around it. Signals with echo are the case it was added for.
#[derive(Clone, Debug)]
pub(crate) struct Atc {
    lows: [f64; MAXIMUM_BLOCKS + 1],
    highs: [f64; MAXIMUM_BLOCKS + 1],
    blocks: usize,
    current_low: f64,
    current_high: f64,
    low: f64,
    high: f64,
    counter: u32,
    smoother: Iir,
}

impl Atc {
    pub(crate) fn new(sample_rate_hz: f64, design: AtcDesign) -> Result<Self, RttyError> {
        if design.blocks == 0 || design.blocks > MAXIMUM_BLOCKS {
            return Err(RttyError::Dsp(grayline_dsp::DspError::InvalidOrder));
        }
        Ok(Self {
            lows: [0.0; MAXIMUM_BLOCKS + 1],
            highs: [1.0; MAXIMUM_BLOCKS + 1],
            blocks: design.blocks,
            current_low: f64::MAX,
            current_high: f64::MIN,
            low: 0.0,
            high: 1.0,
            counter: BLOCK_SAMPLES,
            smoother: Iir::from_low_pass(IirLowPassDesign {
                order: 3,
                sample_rate_hz,
                cutoff_hz: THRESHOLD_CUTOFF_HZ,
                response: IirResponse::Butterworth,
            })?,
        })
    }

    pub(crate) fn process_sample(&mut self, sample: f64) -> f64 {
        self.current_low = self.current_low.min(sample);
        self.current_high = self.current_high.max(sample);
        if self.counter == 0 {
            self.counter = BLOCK_SAMPLES;
            // The floor on the spread keeps a quiet channel from being
            // expanded into noise.
            let low = self.current_low.min(CENTER - MINIMUM_SPREAD);
            let high = self.current_high.max(CENTER + MINIMUM_SPREAD);
            self.lows.copy_within(1..=self.blocks, 0);
            self.highs.copy_within(1..=self.blocks, 0);
            self.lows[self.blocks] = low;
            self.highs[self.blocks] = high;
            self.low = self.lows[..=self.blocks].iter().fold(f64::MAX, |a, &b| a.min(b));
            self.high = self.highs[..=self.blocks].iter().fold(f64::MIN, |a, &b| a.max(b));
            self.current_low = f64::MAX;
            self.current_high = f64::MIN;
        }
        self.counter -= 1;
        let threshold = self.smoother.process_sample((self.high + self.low) * 0.5);
        let expanded = if self.high > self.low {
            (sample - threshold) * EXPANSION + threshold
        } else {
            sample
        };
        expanded + (CENTER - threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atc() -> Atc {
        Atc::new(11_025.0, AtcDesign::default()).unwrap()
    }

    #[test]
    fn a_quiet_channel_stays_below_the_decision_center() {
        // The original's arithmetic lifts a near-zero input toward 0.19 on
        // this scale; the property that matters is that the spread floor
        // keeps it well under the center, so it can never win a comparison
        // against a real tone.
        let mut atc = atc();
        let mut output = 0.0;
        for _ in 0..22_050 {
            output = atc.process_sample(0.001);
        }
        assert!(output < CENTER - MINIMUM_SPREAD, "quiet input became {output}");
        assert!(output > 0.0, "quiet input inverted to {output}");
    }

    #[test]
    fn the_threshold_tracks_a_shifted_midpoint() {
        // A channel riding on a pedestal, as an echo produces: the corrector
        // re-centers it so the high half sits above the nominal center and
        // the low half below.
        let mut atc = atc();
        let mut high_output = 0.0;
        let mut low_output = 0.0;
        for index in 0..44_100 {
            let sample = if (index / 242) % 2 == 0 { 0.9 } else { 0.5 };
            let output = atc.process_sample(sample);
            if (index / 242) % 2 == 0 {
                high_output = output;
            } else {
                low_output = output;
            }
        }
        assert!(high_output > CENTER, "high half at {high_output}");
        assert!(low_output < CENTER, "low half at {low_output}");
    }

    #[test]
    fn an_invalid_block_count_is_rejected() {
        assert!(Atc::new(11_025.0, AtcDesign { blocks: 0 }).is_err());
        assert!(Atc::new(11_025.0, AtcDesign { blocks: 17 }).is_err());
    }
}
