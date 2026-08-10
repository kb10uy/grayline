use alloc::collections::VecDeque;

use crate::SstvError;

/// Parallel demodulator output arrays at contiguous absolute sample positions.
#[derive(Clone, Copy, Debug)]
pub struct DemodulatedBlock<'a> {
    first_sample: u64,
    frequency_hz: &'a [f32],
    sync_strength: &'a [f32],
}

impl<'a> DemodulatedBlock<'a> {
    /// Constructs a block. The decoder validates lengths, values, and continuity.
    pub const fn new(first_sample: u64, frequency_hz: &'a [f32], sync_strength: &'a [f32]) -> Self {
        Self {
            first_sample,
            frequency_hz,
            sync_strength,
        }
    }

    /// Returns the absolute position of the first sample.
    pub const fn first_sample(self) -> u64 {
        self.first_sample
    }

    /// Returns the actual demodulated frequencies in hertz.
    pub const fn frequency_hz(self) -> &'a [f32] {
        self.frequency_hz
    }

    /// Returns normalized synchronization strengths.
    pub const fn sync_strength(self) -> &'a [f32] {
        self.sync_strength
    }

    pub(super) fn validate_continuity(self, expected: Option<u64>) -> Result<(), SstvError> {
        if self.frequency_hz.len() != self.sync_strength.len() {
            return Err(SstvError::DemodulatedLengthMismatch);
        }
        if let Some(expected) = expected
            && self.first_sample != expected
        {
            return Err(SstvError::DemodulatedGap {
                expected,
                actual: self.first_sample,
            });
        }
        self.first_sample
            .checked_add(self.frequency_hz.len() as u64)
            .ok_or(SstvError::SamplePositionOverflow)?;
        Ok(())
    }

    pub(super) fn validate_range(self, offset: usize, count: usize) -> Result<(), SstvError> {
        for (relative, (&frequency, &sync)) in self.frequency_hz[offset..offset + count]
            .iter()
            .zip(&self.sync_strength[offset..offset + count])
            .enumerate()
        {
            if !frequency.is_finite() || frequency < 0.0 || !sync.is_finite() || !(0.0..=1.0).contains(&sync) {
                return Err(SstvError::InvalidDemodulatedSample {
                    offset: offset + relative,
                });
            }
        }
        Ok(())
    }
}

/// Steps per hertz a retained frequency is stored in.
///
/// A picture is carried between 1500 and 2300 Hz, so a sixteenth of a hertz is
/// a fiftieth of one of the 256 levels a pixel can take, and far below what the
/// demodulator itself resolves. Retaining a whole reception is what costs the
/// most memory a receiver asks for, and storing the pair as two bytes and one
/// rather than as two floats is what makes the longest mode fit on a machine
/// with little to spare.
const FREQUENCY_SCALE: f32 = 16.0;

/// Highest frequency the stored form can carry, above which it saturates.
///
/// Four kilohertz is well clear of the band any SSTV tone occupies, so nothing
/// a reception is decoded from reaches it.
const FREQUENCY_CEILING: f32 = u16::MAX as f32 / FREQUENCY_SCALE;

fn store_frequency(hertz: f32) -> u16 {
    // Rounded by addition rather than by `round`, which the core library does
    // not offer, and safe because the value is clamped non-negative first.
    (hertz.clamp(0.0, FREQUENCY_CEILING) * FREQUENCY_SCALE + 0.5) as u16
}

pub(super) fn load_frequency(stored: u16) -> f32 {
    f32::from(stored) / FREQUENCY_SCALE
}

fn store_sync(strength: f32) -> u8 {
    (strength.clamp(0.0, 1.0) * f32::from(u8::MAX) + 0.5) as u8
}

pub(super) fn load_sync(stored: u8) -> f32 {
    f32::from(stored) / f32::from(u8::MAX)
}

fn split_runs<'a, T>(head: &'a [T], tail: &'a [T], start: usize, stop: usize) -> [&'a [T]; 2] {
    if stop <= head.len() {
        [&head[start..stop], &[]]
    } else if start >= head.len() {
        [&tail[start - head.len()..stop - head.len()], &[]]
    } else {
        [&head[start..], &tail[..stop - head.len()]]
    }
}

/// Demodulated samples at contiguous absolute positions, in a retained form.
///
/// The values are quantized on the way in and read back as the floats every
/// caller works in, so the resolution loss is confined to this type. See
/// [`FREQUENCY_SCALE`] for what that resolution buys.
#[derive(Clone, Debug)]
pub(super) struct SampleBuffer {
    first: u64,
    frequency: VecDeque<u16>,
    sync: VecDeque<u8>,
}

impl SampleBuffer {
    pub(super) fn new(first: u64) -> Self {
        Self {
            first,
            frequency: VecDeque::new(),
            sync: VecDeque::new(),
        }
    }

    /// Builds a buffer holding `samples` before it has to grow.
    ///
    /// A buffer that grows into a long reception doubles its way there, which
    /// both leaves up to half the allocation unused and copies everything
    /// retained so far each time it doubles. Asking for the room once is what
    /// keeps the peak down to what is actually held.
    pub(super) fn with_capacity(first: u64, samples: usize) -> Self {
        Self {
            first,
            frequency: VecDeque::with_capacity(samples),
            sync: VecDeque::with_capacity(samples),
        }
    }

    pub(super) fn first(&self) -> u64 {
        self.first
    }

    pub(super) fn end(&self) -> u64 {
        self.first + self.frequency.len() as u64
    }

    pub(super) fn len(&self) -> usize {
        self.frequency.len()
    }

    pub(super) fn append(&mut self, block: DemodulatedBlock<'_>, count: usize) {
        self.frequency
            .extend(block.frequency_hz[..count].iter().map(|hertz| store_frequency(*hertz)));
        self.sync.extend(
            block.sync_strength[..count]
                .iter()
                .map(|strength| store_sync(*strength)),
        );
    }

    pub(super) fn frequency(&self, sample: u64) -> Option<f32> {
        let index = usize::try_from(sample.checked_sub(self.first)?).ok()?;
        self.frequency.get(index).copied().map(load_frequency)
    }

    /// Returns how many synchronization strengths are retained.
    pub(super) fn sync_len(&self) -> usize {
        self.sync.len()
    }

    #[cfg(test)]
    pub(super) fn sync(&self, sample: u64) -> Option<f32> {
        let index = usize::try_from(sample.checked_sub(self.first)?).ok()?;
        self.sync.get(index).copied().map(load_sync)
    }

    /// Returns the retained raw frequencies covering `[first, end)`, oldest
    /// first, as the at most two runs the ring holds them in.
    ///
    /// Reading a range this way replaces one bounds check and index
    /// translation per sample with one per range, which is what the per-unit
    /// averaging and scanning loops spend their time on otherwise.
    pub(super) fn frequency_runs(&self, first: u64, end: u64) -> Option<[&[u16]; 2]> {
        let (start, stop) = self.range_indices(first, end)?;
        let (head, tail) = self.frequency.as_slices();
        Some(split_runs(head, tail, start, stop))
    }

    /// Returns the retained raw synchronization strengths covering `[first, end)`.
    pub(super) fn sync_runs(&self, first: u64, end: u64) -> Option<[&[u8]; 2]> {
        let (start, stop) = self.range_indices(first, end)?;
        let (head, tail) = self.sync.as_slices();
        Some(split_runs(head, tail, start, stop))
    }

    /// Sums the retained frequencies over `[first, end)`, in hertz.
    ///
    /// The stored form is integral, so the sum is exact: dividing it once by
    /// the count reads back identically to loading and adding every sample.
    pub(super) fn frequency_sum(&self, first: u64, end: u64) -> Option<f64> {
        let runs = self.frequency_runs(first, end)?;
        let sum = runs
            .iter()
            .flat_map(|run| run.iter())
            .map(|&stored| u64::from(stored))
            .sum::<u64>();
        Some(sum as f64 / f64::from(FREQUENCY_SCALE))
    }

    fn range_indices(&self, first: u64, end: u64) -> Option<(usize, usize)> {
        if first < self.first || end < first || end > self.end() {
            return None;
        }
        let start = usize::try_from(first - self.first).ok()?;
        let stop = usize::try_from(end - self.first).ok()?;
        Some((start, stop))
    }

    /// Copies the samples at `sample` and later into a new buffer.
    ///
    /// Live slant correction rebuilds the decoding window from retained
    /// samples after the raster clock moves, and only the undecoded remainder
    /// is needed, so this copies a short tail rather than the whole buffer.
    pub(super) fn tail_from(&self, sample: u64) -> Self {
        let skip = usize::try_from(sample.saturating_sub(self.first))
            .unwrap_or(usize::MAX)
            .min(self.len());
        fn tail_of<T: Copy>(source: &VecDeque<T>, skip: usize) -> VecDeque<T> {
            let (head, tail) = source.as_slices();
            let head_skip = skip.min(head.len());
            let mut copied = VecDeque::with_capacity(source.len() - skip);
            copied.extend(head[head_skip..].iter().copied());
            copied.extend(tail[skip - head_skip..].iter().copied());
            copied
        }
        Self {
            first: self.first + skip as u64,
            frequency: tail_of(&self.frequency, skip),
            sync: tail_of(&self.sync, skip),
        }
    }

    pub(super) fn discard_before(&mut self, sample: u64) {
        let count = sample.saturating_sub(self.first).min(self.len() as u64) as usize;
        self.frequency.drain(..count);
        self.sync.drain(..count);
        self.first += count as u64;
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use rstest::rstest;

    use super::*;

    fn retained(frequency: &[f32], sync: &[f32]) -> SampleBuffer {
        let mut buffer = SampleBuffer::new(0);
        buffer.append(DemodulatedBlock::new(0, frequency, sync), frequency.len());
        buffer
    }

    /// The retained form is what a refinement re-decodes the picture from, so
    /// what it loses has to stay far below what one pixel level spans. A pixel
    /// covers 800 hertz over 256 levels, or a little over three hertz each.
    #[rstest]
    #[case(0.0)]
    #[case(1_200.0)]
    #[case(1_500.0)]
    #[case(1_900.375)]
    #[case(2_300.0)]
    #[case(2_999.97)]
    fn retained_frequencies_round_trip_within_one_step(#[case] hertz: f32) {
        let buffer = retained(&[hertz], &[0.0]);
        let read = buffer.frequency(0).expect("the sample just appended");
        assert!(
            (read - hertz).abs() <= 1.0 / FREQUENCY_SCALE,
            "{hertz} Hz read back as {read} Hz"
        );
    }

    /// Nothing a reception is decoded from reaches the ceiling, but a value
    /// that does must not wrap around into the picture band.
    #[test]
    fn frequencies_above_the_ceiling_saturate() {
        let buffer = retained(&[f32::MAX, 8_000.0], &[0.0, 0.0]);
        assert_eq!(buffer.frequency(0), Some(FREQUENCY_CEILING));
        assert_eq!(buffer.frequency(1), Some(FREQUENCY_CEILING));
    }

    /// Acquisition extracts pulse runs at 0.50 and live synchronization accepts
    /// a peak at 0.35 with a contrast of 0.20, so the step has to be far finer
    /// than the gaps between those.
    #[rstest]
    #[case(0.0)]
    #[case(0.2)]
    #[case(0.35)]
    #[case(0.5)]
    #[case(1.0)]
    fn retained_sync_round_trips_within_one_step(#[case] strength: f32) {
        let buffer = retained(&[0.0], &[strength]);
        let read = buffer.sync(0).expect("the sample just appended");
        assert!(
            (read - strength).abs() <= 1.0 / f32::from(u8::MAX),
            "{strength} read back as {read}"
        );
    }

    #[test]
    fn a_reserved_buffer_starts_empty() {
        let buffer = SampleBuffer::with_capacity(64, 4_096);
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.first(), 64);
        assert_eq!(buffer.sync_len(), 0);
        assert!(buffer.sync_runs(64, 65).is_none());
    }

    /// Bulk range reads have to agree with per-sample reads wherever the two
    /// retained runs happen to sit in the ring, including across the seam.
    #[test]
    fn range_reads_match_per_sample_reads_across_the_ring_seam() {
        let mut buffer = SampleBuffer::with_capacity(0, 128);
        let frequency: Vec<f32> = (0..96).map(|index| 1_500.0 + index as f32).collect();
        let sync: Vec<f32> = (0..96).map(|index| (index % 5) as f32 / 4.0).collect();
        buffer.append(DemodulatedBlock::new(0, &frequency, &sync), 96);
        buffer.discard_before(64);
        let frequency: Vec<f32> = (96..160).map(|index| 1_500.0 + index as f32).collect();
        let sync: Vec<f32> = (96..160).map(|index| (index % 7) as f32 / 6.0).collect();
        buffer.append(DemodulatedBlock::new(96, &frequency, &sync), 64);

        for (first, end) in [(64, 160), (70, 90), (130, 160), (100, 140)] {
            let runs = buffer.frequency_runs(first, end).unwrap();
            let bulk: Vec<f32> = runs
                .iter()
                .flat_map(|run| run.iter())
                .map(|&stored| load_frequency(stored))
                .collect();
            let direct: Vec<f32> = (first..end).map(|sample| buffer.frequency(sample).unwrap()).collect();
            assert_eq!(bulk, direct, "frequencies over {first}..{end}");

            let sum = buffer.frequency_sum(first, end).unwrap();
            let expected: f64 = direct.iter().map(|&value| f64::from(value)).sum();
            assert_eq!(sum, expected, "sum over {first}..{end}");

            let runs = buffer.sync_runs(first, end).unwrap();
            let bulk: Vec<f32> = runs
                .iter()
                .flat_map(|run| run.iter())
                .map(|&stored| load_sync(stored))
                .collect();
            let direct: Vec<f32> = (first..end).map(|sample| buffer.sync(sample).unwrap()).collect();
            assert_eq!(bulk, direct, "sync over {first}..{end}");
        }

        assert!(buffer.frequency_runs(60, 80).is_none());
        assert!(buffer.frequency_runs(150, 170).is_none());
        assert!(buffer.frequency_sum(150, 140).is_none());
    }

    /// Copying a tail carries the retained values, not the ones they came from.
    #[test]
    fn a_tail_keeps_what_was_retained() {
        let buffer = retained(&[1_500.0, 1_900.0, 2_300.0], &[0.1, 0.6, 0.9]);
        let tail = buffer.tail_from(1);
        assert_eq!(tail.first(), 1);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail.frequency(1), buffer.frequency(1));
        assert_eq!(tail.sync(2), buffer.sync(2));
    }
}
