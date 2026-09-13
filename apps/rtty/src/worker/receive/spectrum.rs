//! The band spectrum the scope window draws.
//!
//! Computed here rather than taken from the receiver. The pipeline runs a
//! transform of its own for the frequency control, and a display sharing it
//! would tie the length and the cadence a picture wants to the ones a search
//! needs: changing either would silently change the other. This one is a
//! fixed 2048 points at whatever rate the device runs, transformed when a
//! snapshot is published and not otherwise, so an unopened scope costs the
//! copy of the samples and nothing else.

use grayline_dsp::transform::{RealSpectrum, SpectrumWindow};

/// Samples one transform reads.
///
/// Bins are 23 Hz wide at 48 kHz and 5 Hz at 11 kHz — finer than the pixels
/// they are drawn into either way, which is what a picture wants and is not
/// what the frequency control's own transform is sized for.
const LENGTH: usize = 2_048;

/// The highest frequency published, in hertz.
///
/// MMTTY draws to 3000 or 4000 Hz depending on the rate it is running at
/// (`docs/memo/mmtty/dsp.md`); this takes the wider of the two, because the
/// panel's own mark ceiling is 3000 Hz and an 850 Hz shift puts space above
/// it.
const CEILING_HZ: f64 = 4_000.0;

/// The magnitude a full-scale tone puts in one bin.
///
/// A Hann window passes half the amplitude and a real transform splits the
/// rest between the two halves of the spectrum, so a sine at full scale
/// reaches a quarter of the transform's length. This is the reference the
/// display's decibel scale is drawn against.
pub const FULL_SCALE: f32 = LENGTH as f32 / 4.0;

/// The newest samples, and the transform that reads them.
pub(super) struct Spectrum {
    analyzer: RealSpectrum,
    /// The last [`LENGTH`] samples, oldest first.
    history: Vec<f64>,
    /// How many of them have been captured, so a window that has just opened
    /// draws nothing rather than a transform of silence that was never heard.
    filled: usize,
    /// Bins up to the ceiling, which is all that is published.
    bins: usize,
    bin_hz: f32,
}

impl Spectrum {
    /// Returns an analyzer for a stream at `sample_rate_hz`.
    ///
    /// `None` where the transform could not be built, which the length being
    /// a constant power of two rules out: a scope with no spectrum in it is
    /// still a scope, so this is not a failure the reception is told about.
    pub(super) fn new(sample_rate_hz: u32) -> Option<Self> {
        let analyzer = RealSpectrum::new(LENGTH, SpectrumWindow::Hann).ok()?;
        let bin_hz = f64::from(sample_rate_hz) / LENGTH as f64;
        // One bin past the ceiling, so the line drawn to it has somewhere to
        // end; a rate whose Nyquist is below the ceiling publishes the lot.
        let bins = ((CEILING_HZ / bin_hz) as usize + 2).min(analyzer.bin_count());
        Some(Self {
            analyzer,
            history: vec![0.0; LENGTH],
            filled: 0,
            bins,
            bin_hz: bin_hz as f32,
        })
    }

    /// Keeps the newest samples of `block`, dropping what they push out.
    pub(super) fn observe(&mut self, block: &[f32]) {
        let taken = block.len().min(LENGTH);
        self.history.copy_within(taken.., 0);
        let kept = &block[block.len() - taken..];
        for (slot, &sample) in self.history[LENGTH - taken..].iter_mut().zip(kept) {
            *slot = f64::from(sample);
        }
        self.filled = (self.filled + taken).min(LENGTH);
    }

    /// The band as it stands, or nothing until a transform's worth of audio
    /// has arrived.
    pub(super) fn magnitudes(&mut self) -> Vec<f32> {
        if self.filled < LENGTH {
            return Vec::new();
        }
        let Ok(bins) = self.analyzer.transform(&self.history) else {
            return Vec::new();
        };
        bins[..self.bins].iter().map(|bin| bin.magnitude() as f32).collect()
    }

    /// The width of one published bin, which is how a display places them.
    pub(super) const fn bin_hz(&self) -> f32 {
        self.bin_hz
    }
}

#[cfg(test)]
mod tests {
    use core::f64::consts::TAU;

    use super::*;

    const RATE: u32 = 8_000;

    fn tone(frequency_hz: f64, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| (TAU * frequency_hz * index as f64 / f64::from(RATE)).sin() as f32)
            .collect()
    }

    fn peak_bin(magnitudes: &[f32]) -> usize {
        magnitudes
            .iter()
            .enumerate()
            .max_by(|(_, one), (_, other)| one.total_cmp(other))
            .map(|(bin, _)| bin)
            .expect("a transform produces bins")
    }

    #[test]
    fn a_tone_lands_in_the_bin_it_belongs_to() {
        let mut spectrum = Spectrum::new(RATE).unwrap();
        spectrum.observe(&tone(1_000.0, LENGTH));
        let magnitudes = spectrum.magnitudes();

        let peak = peak_bin(&magnitudes) as f32 * spectrum.bin_hz();
        assert!((peak - 1_000.0).abs() <= spectrum.bin_hz(), "peaked at {peak} Hz");
    }

    /// A window that opened part way through a block would otherwise draw the
    /// zeros in front of the audio as a band full of signal.
    #[test]
    fn nothing_is_drawn_until_the_transform_is_full() {
        let mut spectrum = Spectrum::new(RATE).unwrap();
        spectrum.observe(&tone(1_000.0, LENGTH - 1));
        assert!(spectrum.magnitudes().is_empty());

        spectrum.observe(&tone(1_000.0, 1));
        assert!(!spectrum.magnitudes().is_empty());
    }

    /// The history is what was heard last, so a block longer than it is kept
    /// by its tail rather than by its head.
    #[test]
    fn a_block_longer_than_the_transform_keeps_its_newest_samples() {
        let mut spectrum = Spectrum::new(RATE).unwrap();
        let mut block = tone(500.0, LENGTH);
        block.extend(tone(2_000.0, LENGTH));
        spectrum.observe(&block);

        let magnitudes = spectrum.magnitudes();
        let peak = peak_bin(&magnitudes) as f32 * spectrum.bin_hz();
        assert!((peak - 2_000.0).abs() <= spectrum.bin_hz(), "peaked at {peak} Hz");
    }

    /// Nothing above the ceiling is drawn, so nothing above it is published.
    #[test]
    fn the_published_band_stops_at_the_ceiling() {
        let mut spectrum = Spectrum::new(48_000).unwrap();
        spectrum.observe(&[0.0; LENGTH]);
        let top = spectrum.magnitudes().len() as f32 * spectrum.bin_hz();
        assert!(top > CEILING_HZ as f32, "the ceiling should be reached: {top} Hz");
        assert!(top < CEILING_HZ as f32 + 3.0 * spectrum.bin_hz(), "and not overshot");
    }

    /// A rate whose Nyquist is below the ceiling has fewer bins than the
    /// ceiling asks for, and asking for them anyway would read past the
    /// transform.
    #[test]
    fn a_narrow_band_publishes_what_it_has() {
        let mut spectrum = Spectrum::new(6_000).unwrap();
        spectrum.observe(&[0.0; LENGTH]);
        assert_eq!(spectrum.magnitudes().len(), LENGTH / 2 + 1);
    }
}
