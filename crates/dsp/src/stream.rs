use alloc::collections::VecDeque;

const PIXEL_GUARD: f64 = 0.1875;

/// Values at contiguous absolute sample positions, retained in a ring.
#[derive(Clone, Debug, Default)]
pub struct SampleRing<T> {
    first: u64,
    samples: VecDeque<T>,
}

impl<T: Copy> SampleRing<T> {
    /// Creates an empty ring whose next value lands at `first`.
    pub fn new(first: u64) -> Self {
        Self {
            first,
            samples: VecDeque::new(),
        }
    }

    /// Creates an empty ring with room for `capacity` values before it grows.
    pub fn with_capacity(first: u64, capacity: usize) -> Self {
        Self {
            first,
            samples: VecDeque::with_capacity(capacity),
        }
    }

    /// Returns the position of the oldest retained value.
    pub fn first(&self) -> u64 {
        self.first
    }

    /// Returns the position one past the newest retained value.
    pub fn end(&self) -> u64 {
        self.first + self.samples.len() as u64
    }

    /// Returns how many values are retained.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Returns whether no value is retained.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Appends one value at [`Self::end`].
    pub fn push(&mut self, value: T) {
        self.samples.push_back(value);
    }

    /// Appends values from [`Self::end`] onwards.
    pub fn extend(&mut self, values: impl IntoIterator<Item = T>) {
        self.samples.extend(values);
    }

    /// Returns the value at `sample`, if it is retained.
    pub fn get(&self, sample: u64) -> Option<T> {
        let index = usize::try_from(sample.checked_sub(self.first)?).ok()?;
        self.samples.get(index).copied()
    }

    /// Returns the values covering `[first, end)`, oldest first, as the at
    /// most two runs the ring holds them in, or `None` unless all of them are
    /// retained.
    pub fn runs(&self, first: u64, end: u64) -> Option<[&[T]; 2]> {
        if first < self.first || end < first || end > self.end() {
            return None;
        }
        let start = usize::try_from(first - self.first).ok()?;
        let stop = usize::try_from(end - self.first).ok()?;
        let (head, tail) = self.samples.as_slices();
        Some(if stop <= head.len() {
            [&head[start..stop], &[]]
        } else if start >= head.len() {
            [&tail[start - head.len()..stop - head.len()], &[]]
        } else {
            [&head[start..], &tail[..stop - head.len()]]
        })
    }

    /// Drops every value before `sample`.
    pub fn discard_before(&mut self, sample: u64) {
        let count = sample.saturating_sub(self.first).min(self.samples.len() as u64) as usize;
        self.samples.drain(..count);
        self.first += count as u64;
    }

    /// Drops every value and places the next one at `first`.
    pub fn restart(&mut self, first: u64) {
        self.samples.clear();
        self.first = first;
    }

    /// Copies the values at `sample` and later into a new ring.
    pub fn tail_from(&self, sample: u64) -> Self {
        let skip = usize::try_from(sample.saturating_sub(self.first))
            .unwrap_or(usize::MAX)
            .min(self.samples.len());
        let (head, tail) = self.samples.as_slices();
        let head_skip = skip.min(head.len());
        let mut samples = VecDeque::with_capacity(self.samples.len() - skip);
        samples.extend(head[head_skip..].iter().copied());
        samples.extend(tail[skip - head_skip..].iter().copied());
        Self {
            first: self.first + skip as u64,
            samples,
        }
    }
}

/// Returns the first whole sample at or after `position`, saturating at zero
/// and at `u64::MAX`.
pub fn ceil_sample(position: f64) -> u64 {
    let whole = position as u64;
    whole.saturating_add(u64::from((whole as f64) < position))
}

/// Returns the half-open range of whole samples averaged for a pixel spanning
/// the fractional positions `[left, right)`.
///
/// A guard of three sixteenths of the pixel is left out at each edge, which
/// keeps out samples that belong to a neighbour after sub-sample rounding or
/// while the discriminator settles. A pixel too short to keep a guarded sample
/// falls back to the one sample nearest its centre.
pub fn guarded_window(left: f64, right: f64) -> (u64, u64) {
    let guard = (right - left) * PIXEL_GUARD;
    let first = ceil_sample(left + guard);
    let end = ceil_sample(right - guard);
    if end > first {
        (first, end)
    } else {
        let center = ceil_sample((left + right) * 0.5 - 0.5);
        (center, center + 1)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use rstest::rstest;

    use super::*;

    fn wrapped() -> SampleRing<u32> {
        let mut ring = SampleRing::with_capacity(0, 128);
        ring.extend(0..96);
        ring.discard_before(64);
        ring.extend(96..160);
        ring
    }

    #[rstest]
    #[case(64, 160)]
    #[case(70, 90)]
    #[case(130, 160)]
    #[case(100, 140)]
    #[case(80, 80)]
    fn runs_read_what_get_reads_across_the_seam(#[case] first: u64, #[case] end: u64) {
        let ring = wrapped();
        let runs: Vec<u32> = ring
            .runs(first, end)
            .unwrap()
            .iter()
            .flat_map(|run| run.iter())
            .copied()
            .collect();
        let direct: Vec<u32> = (first..end).map(|sample| ring.get(sample).unwrap()).collect();
        assert_eq!(runs, direct);
        assert_eq!(direct, (first as u32..end as u32).collect::<Vec<_>>());
    }

    #[rstest]
    #[case(60, 80)]
    #[case(150, 170)]
    #[case(150, 140)]
    fn runs_reject_what_is_not_retained(#[case] first: u64, #[case] end: u64) {
        assert!(wrapped().runs(first, end).is_none());
    }

    #[rstest]
    #[case(0, 64)]
    #[case(100, 100)]
    #[case(159, 159)]
    #[case(400, 160)]
    fn a_tail_keeps_what_was_retained(#[case] sample: u64, #[case] first: u64) {
        let ring = wrapped();
        let tail = ring.tail_from(sample);
        assert_eq!(tail.first(), first);
        assert_eq!(tail.end(), ring.end());
        assert!((first..ring.end()).all(|sample| tail.get(sample) == ring.get(sample)));
    }

    #[test]
    fn a_restart_moves_the_next_position() {
        let mut ring = wrapped();
        ring.restart(1_000);
        assert!(ring.is_empty());
        ring.push(7);
        assert_eq!((ring.first(), ring.end(), ring.get(1_000)), (1_000, 1_001, Some(7)));
    }

    #[rstest]
    #[case(-0.25, 0)]
    #[case(0.0, 0)]
    #[case(2.0, 2)]
    #[case(2.000_1, 3)]
    #[case(f64::NAN, 0)]
    #[case(f64::INFINITY, u64::MAX)]
    fn ceil_sample_matches_a_clamped_ceiling(#[case] position: f64, #[case] expected: u64) {
        assert_eq!(ceil_sample(position), expected);
        assert_eq!(libm::ceil(position).max(0.0) as u64, expected);
    }

    #[rstest]
    #[case(10.0, 14.0, (11, 14))]
    #[case(10.0, 13.0, (11, 13))]
    #[case(10.2, 10.6, (10, 11))]
    #[case(10.6, 11.4, (11, 12))]
    fn guarded_windows_trim_edges_or_fall_back_to_the_centre(
        #[case] left: f64,
        #[case] right: f64,
        #[case] expected: (u64, u64),
    ) {
        assert_eq!(guarded_window(left, right), expected);
    }
}
