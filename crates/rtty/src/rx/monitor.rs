use alloc::collections::{VecDeque, vec_deque::Drain};

/// The mark and space channel outputs one input sample produced.
///
/// These are the two values the comparator compares — rectified, integrated,
/// and corrected — which is the pair MMTTY's XY scope draws
/// (`docs/memo/mmtty/dsp.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChannelLevels {
    /// The mark channel's output.
    pub mark: f64,
    /// The space channel's output.
    pub space: f64,
}

impl ChannelLevels {
    /// Returns `mark − space`, the signed reading a tuning display shows.
    pub const fn difference(self) -> f64 {
        self.mark - self.space
    }
}

/// How the monitor tap samples the channel outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorConfig {
    /// Keeps one pair in this many; one keeps every sample.
    ///
    /// A display needs far fewer points than a capture rate produces, and the
    /// decimation is the caller's because only the caller knows how many it
    /// is going to draw. It is a display setting: nothing downstream of the
    /// comparator sees it.
    pub decimation: usize,
    /// How many pairs are held before the oldest is dropped.
    pub capacity: usize,
}

/// The bounded ring the tap collects into.
///
/// The oldest pair is dropped rather than the newest: a caller that stopped
/// draining is one whose display stopped drawing, and what it wants when it
/// returns is the signal now rather than the signal it missed.
pub(crate) struct Monitor {
    config: MonitorConfig,
    pairs: VecDeque<ChannelLevels>,
    since_kept: usize,
}

impl Monitor {
    pub(crate) fn new(config: MonitorConfig) -> Self {
        let config = MonitorConfig {
            decimation: config.decimation.max(1),
            capacity: config.capacity.max(1),
        };
        Self {
            config,
            pairs: VecDeque::with_capacity(config.capacity),
            since_kept: 0,
        }
    }

    pub(crate) const fn config(&self) -> MonitorConfig {
        self.config
    }

    pub(crate) fn observe(&mut self, levels: ChannelLevels) {
        self.since_kept += 1;
        if self.since_kept < self.config.decimation {
            return;
        }
        self.since_kept = 0;
        if self.pairs.len() == self.config.capacity {
            self.pairs.pop_front();
        }
        self.pairs.push_back(levels);
    }

    pub(crate) fn drain(&mut self) -> Drain<'_, ChannelLevels> {
        self.pairs.drain(..)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn levels(value: f64) -> ChannelLevels {
        ChannelLevels {
            mark: value,
            space: -value,
        }
    }

    #[test]
    fn a_difference_is_signed_towards_mark() {
        assert_eq!(levels(1.0).difference(), 2.0);
        assert_eq!(ChannelLevels::default().difference(), 0.0);
    }

    #[test]
    fn decimation_keeps_one_pair_in_every_window() {
        let mut monitor = Monitor::new(MonitorConfig {
            decimation: 3,
            capacity: 16,
        });
        for index in 0..9 {
            monitor.observe(levels(f64::from(index)));
        }
        let kept: Vec<f64> = monitor.drain().map(|pair| pair.mark).collect();
        assert_eq!(kept, [2.0, 5.0, 8.0]);
    }

    #[test]
    fn a_full_ring_drops_the_oldest_pair() {
        let mut monitor = Monitor::new(MonitorConfig {
            decimation: 1,
            capacity: 2,
        });
        for index in 0..4 {
            monitor.observe(levels(f64::from(index)));
        }
        let kept: Vec<f64> = monitor.drain().map(|pair| pair.mark).collect();
        assert_eq!(kept, [2.0, 3.0]);
    }

    #[test]
    fn draining_empties_the_ring() {
        let mut monitor = Monitor::new(MonitorConfig {
            decimation: 1,
            capacity: 4,
        });
        monitor.observe(levels(1.0));
        assert_eq!(monitor.drain().count(), 1);
        assert_eq!(monitor.drain().count(), 0);
    }

    /// A zero would keep nothing at all, which reads as a tap that is on and
    /// silent rather than as one that was configured wrongly.
    #[test]
    fn an_impossible_configuration_is_brought_back_into_range() {
        let monitor = Monitor::new(MonitorConfig {
            decimation: 0,
            capacity: 0,
        });
        assert_eq!(monitor.config.decimation, 1);
        assert_eq!(monitor.config.capacity, 1);
    }
}
