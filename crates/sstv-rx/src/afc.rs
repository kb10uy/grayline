use alloc::collections::VecDeque;

use grayline_sstv::signal::SYNC_HZ;

const SYNC_THRESHOLD: f64 = 0.58;

/// How far from the expected sync frequency a measurement may sit.
///
/// A discriminator reading further out than this belongs to something other
/// than the sync pulse being measured, so it is not averaged into the offset.
const MEASUREMENT_WINDOW_HZ: f64 = 100.0;

/// How long a run above the threshold has to last to count as a sync pulse.
const RUN_SECONDS: core::ops::RangeInclusive<f64> = 0.003..=0.050;

const DETECTOR_LAG_SECONDS: f64 = 0.012;

const PULSE_GAP_SECONDS: f64 = 0.0015;

const PULSE_EDGE_SECONDS: f64 = 0.0015;

const PULSE_MEASUREMENTS: usize = 2;

const MAX_OFFSET_HZ: f64 = 150.0;

/// How many runs the offset is smoothed over.
///
/// The sum is divided by this whether or not that many runs have arrived, so
/// the correction ramps in over the first full history rather than jumping to
/// the first measurement.
const HISTORY: usize = 15;

const INHIBIT_SECONDS: f64 = 0.1;

#[derive(Clone, Copy)]
struct Measurement {
    sample: u64,
    frequency_hz: f64,
}

pub(crate) struct Afc {
    enabled: bool,
    sample_rate_hz: f64,
    active: bool,
    position: u64,
    run_samples: u64,
    recent: VecDeque<Measurement>,
    offsets: VecDeque<f64>,
    offset_hz: f64,
    inhibit_samples: usize,
}

impl Afc {
    pub(crate) fn new(sample_rate_hz: f64) -> Self {
        Self {
            enabled: false,
            sample_rate_hz,
            active: false,
            position: 0,
            run_samples: 0,
            recent: VecDeque::new(),
            offsets: VecDeque::with_capacity(HISTORY),
            offset_hz: 0.0,
            inhibit_samples: 0,
        }
    }

    pub(crate) fn process(&mut self, sync_strength: f64, measurement: Option<f64>) -> bool {
        if !self.enabled {
            return false;
        }
        self.position += 1;
        self.inhibit_samples = self.inhibit_samples.saturating_sub(1);
        if let Some(frequency_hz) = measurement {
            let oldest = self
                .position
                .saturating_sub(self.samples(*RUN_SECONDS.end() + DETECTOR_LAG_SECONDS));
            while self.recent.front().is_some_and(|kept| kept.sample < oldest) {
                self.recent.pop_front();
            }
            self.recent.push_back(Measurement {
                sample: self.position,
                frequency_hz,
            });
        }
        if sync_strength >= SYNC_THRESHOLD {
            self.active = true;
            self.run_samples += 1;
            false
        } else if self.active {
            self.finish_run()
        } else {
            false
        }
    }

    pub(crate) fn finish_run(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        let run_samples = core::mem::take(&mut self.run_samples);
        let duration = run_samples as f64 / self.sample_rate_hz;
        if self.inhibit_samples > 0 || !RUN_SECONDS.contains(&duration) {
            return false;
        }
        let Some(measured) = self.pulse_frequency(run_samples) else {
            return false;
        };
        let offset = measured - f64::from(SYNC_HZ);
        if offset.abs() > MAX_OFFSET_HZ {
            return false;
        }
        if self.offsets.len() == HISTORY {
            self.offsets.pop_front();
        }
        self.offsets.push_back(offset);
        self.offset_hz = self.offsets.iter().sum::<f64>() / HISTORY as f64;
        self.inhibit_samples = (self.sample_rate_hz * INHIBIT_SECONDS) as usize;
        true
    }

    pub(crate) const fn enable(&mut self) {
        self.enabled = true;
    }

    pub(crate) const fn offset_hz(&self) -> f64 {
        self.offset_hz
    }

    fn samples(&self, seconds: f64) -> u64 {
        (seconds * self.sample_rate_hz) as u64
    }

    fn pulse_frequency(&self, run_samples: u64) -> Option<f64> {
        let earliest = self
            .position
            .saturating_sub(run_samples + self.samples(DETECTOR_LAG_SECONDS));
        let expected = f64::from(SYNC_HZ) + self.offset_hz;
        let gap = self.samples(PULSE_GAP_SECONDS);
        let in_pulse = self
            .recent
            .iter()
            .filter(|kept| kept.sample >= earliest && (kept.frequency_hz - expected).abs() <= MEASUREMENT_WINDOW_HZ);

        let mut longest: Option<(u64, u64)> = None;
        let mut current: Option<(u64, u64)> = None;
        for kept in in_pulse.clone() {
            let (first, _) = current
                .filter(|(_, last)| kept.sample - last <= gap)
                .unwrap_or((kept.sample, kept.sample));
            current = Some((first, kept.sample));
            if longest.is_none_or(|(start, end)| kept.sample - first >= end - start) {
                longest = current;
            }
        }

        let (first, last) = longest?;
        let edge = self.samples(PULSE_EDGE_SECONDS);
        let interior = first + edge..=last.checked_sub(edge)?;
        let (sum, count) = in_pulse
            .filter(|kept| interior.contains(&kept.sample))
            .fold((0.0, 0_usize), |(sum, count), kept| {
                (sum + kept.frequency_hz, count + 1)
            });
        (count >= PULSE_MEASUREMENTS).then(|| sum / count as f64)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const RATE: f64 = 48_000.0;
    const DETECTOR_LAG: f64 = 0.008;
    const LINE_SECONDS: f64 = 0.15;

    fn feed_line(afc: &mut Afc, pulse_seconds: f64, reading: impl Fn(f64) -> f64) {
        let half_cycle = (RATE / 2_400.0) as usize;
        for sample in 0..(RATE * LINE_SECONDS) as usize {
            let seconds = sample as f64 / RATE;
            let gate = (DETECTOR_LAG..DETECTOR_LAG + pulse_seconds).contains(&seconds);
            let measurement = (sample % half_cycle == 0).then(|| {
                if seconds < pulse_seconds {
                    reading(seconds)
                } else {
                    1_500.0
                }
            });
            afc.process(if gate { 0.9 } else { 0.1 }, measurement);
        }
    }

    #[test]
    fn a_persistent_tone_does_not_grow_the_measurement_buffer() {
        let rate = 8_000.0;
        let mut afc = Afc::new(rate);
        afc.enable();
        for _ in 0..(rate * 10.0) as usize {
            afc.process(0.9, Some(1_210.0));
        }
        let bound = (rate * (RUN_SECONDS.end() + DETECTOR_LAG_SECONDS)) as usize + 1;
        assert!(
            afc.recent.len() <= bound,
            "the buffer grew to {} measurements",
            afc.recent.len()
        );
        assert!(!afc.finish_run(), "an overlong run must not update the offset");
        assert_eq!(afc.offset_hz(), 0.0);
    }

    #[rstest]
    #[case(0.004_862, 40.0)]
    #[case(0.009, 40.0)]
    #[case(0.009, -60.0)]
    #[case(0.020, 40.0)]
    fn tracks_repeated_offset_sync_pulses(#[case] pulse_seconds: f64, #[case] offset_hz: f64) {
        let mut afc = Afc::new(RATE);
        afc.enable();
        for _ in 0..24 {
            feed_line(&mut afc, pulse_seconds, |_| 1_200.0 + offset_hz);
        }
        assert!(
            (afc.offset_hz() - offset_hz).abs() < 1.0,
            "offset was {} Hz",
            afc.offset_hz()
        );
    }

    #[rstest]
    #[case(0.004_862)]
    #[case(0.009)]
    #[case(0.020)]
    fn the_step_out_of_a_pulse_is_not_measured(#[case] pulse_seconds: f64) {
        let mut afc = Afc::new(RATE);
        afc.enable();
        for _ in 0..24 {
            feed_line(&mut afc, pulse_seconds, |seconds| {
                let remaining = pulse_seconds - seconds;
                if remaining < 0.0005 {
                    1_300.0
                } else if remaining < 0.001_3 {
                    1_230.0
                } else {
                    1_200.0
                }
            });
        }
        assert!(afc.offset_hz().abs() < 1.0, "offset was {} Hz", afc.offset_hz());
    }

    #[test]
    fn a_run_without_a_pulse_before_it_is_not_measured() {
        let mut afc = Afc::new(RATE);
        afc.enable();
        for _ in 0..24 {
            feed_line(&mut afc, 0.009, |_| 1_900.0);
        }
        assert_eq!(afc.offset_hz(), 0.0);
    }
}
