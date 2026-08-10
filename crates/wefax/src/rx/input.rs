use crate::error::WefaxError;

/// A borrowed run of demodulated frequencies with its absolute position.
///
/// One array rather than two: WEFAX has no per-line synchronization pulse, so
/// there is no second stream to carry alongside the frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemodulatedBlock<'a> {
    first_sample: u64,
    frequency_hz: &'a [f32],
}

impl<'a> DemodulatedBlock<'a> {
    /// Borrows a run of demodulated frequencies, rejecting unusable values.
    pub fn new(first_sample: u64, frequency_hz: &'a [f32]) -> Result<Self, WefaxError> {
        if let Some(offset) = frequency_hz.iter().position(|value| !value.is_finite()) {
            return Err(WefaxError::InvalidDemodulatedSample { offset });
        }
        if first_sample.checked_add(frequency_hz.len() as u64).is_none() {
            return Err(WefaxError::SamplePositionOverflow);
        }
        Ok(Self {
            first_sample,
            frequency_hz,
        })
    }

    /// Borrows values a demodulator has already validated.
    pub(crate) const fn already_checked(first_sample: u64, frequency_hz: &'a [f32]) -> Self {
        Self {
            first_sample,
            frequency_hz,
        }
    }

    /// Returns the absolute position of the first sample.
    pub const fn first_sample(&self) -> u64 {
        self.first_sample
    }

    /// Returns the absolute position one past the last sample.
    pub const fn end_sample(&self) -> u64 {
        self.first_sample + self.frequency_hz.len() as u64
    }

    /// Returns the demodulated frequencies in hertz.
    pub const fn frequency_hz(&self) -> &'a [f32] {
        self.frequency_hz
    }

    /// Returns how many samples the block carries.
    pub const fn len(&self) -> usize {
        self.frequency_hz.len()
    }

    /// Returns whether the block carries no samples.
    pub const fn is_empty(&self) -> bool {
        self.frequency_hz.is_empty()
    }

    /// Returns the part of this block from `offset` onwards.
    ///
    /// This is how a caller hands a decoder the samples on one side of an
    /// event it decided at: the tones that frame a transmission are found
    /// part way through a packet, and the decoder's own state changes there.
    pub fn from(&self, offset: usize) -> Self {
        let offset = offset.min(self.frequency_hz.len());
        Self {
            first_sample: self.first_sample + offset as u64,
            frequency_hz: &self.frequency_hz[offset..],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_reports_its_own_span() {
        let samples = [1_500.0, 1_900.0, 2_300.0];
        let block = DemodulatedBlock::new(100, &samples).unwrap();
        assert_eq!(block.first_sample(), 100);
        assert_eq!(block.end_sample(), 103);
        assert_eq!(block.len(), 3);
        assert!(!block.is_empty());
    }

    #[test]
    fn a_sub_block_carries_the_position_it_starts_at() {
        let samples = [1_500.0, 1_900.0, 2_300.0];
        let block = DemodulatedBlock::new(100, &samples).unwrap();
        let tail = block.from(2);
        assert_eq!(tail.first_sample(), 102);
        assert_eq!(tail.frequency_hz(), &[2_300.0]);
        assert!(block.from(9).is_empty());
    }

    #[test]
    fn an_unusable_sample_is_rejected_with_its_offset() {
        let samples = [1_500.0, f32::INFINITY];
        assert_eq!(
            DemodulatedBlock::new(0, &samples).unwrap_err(),
            WefaxError::InvalidDemodulatedSample { offset: 1 }
        );
    }

    #[test]
    fn a_block_running_past_the_end_of_the_timeline_is_rejected() {
        let samples = [1_500.0, 1_900.0];
        assert_eq!(
            DemodulatedBlock::new(u64::MAX, &samples).unwrap_err(),
            WefaxError::SamplePositionOverflow
        );
    }
}
