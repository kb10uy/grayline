use crate::params::ToneSet;

/// How the detected pair is allowed to move the demodulator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AfcMode {
    /// Both tones follow their detected peaks independently.
    Free,
    /// The shift stays fixed; only the center follows.
    FixedShift,
    /// The shift snaps to 170, 200, 220, or 240 Hz and the pair follows.
    #[default]
    Ham,
    /// The shift snaps as in `Ham`, but the center does not move — for a
    /// transmitter keyed directly, whose audio center is the radio's tuning.
    Fsk,
}

/// Parameters for automatic frequency control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AfcConfig {
    /// How the detected pair may move the demodulator.
    pub mode: AfcMode,
    /// The lowest tone the search will consider, in hertz. MMTTY's `MARKL`
    /// is 300.
    pub search_floor_hz: f64,
    /// The highest tone the search will consider, in hertz. MMTTY's
    /// `SPACEH` is 2700 — its half-rate Nyquist ceiling, kept here only as a
    /// default; this path does not decimate, so the setting may be raised.
    pub search_ceiling_hz: f64,
}

impl Default for AfcConfig {
    fn default() -> Self {
        Self {
            mode: AfcMode::default(),
            search_floor_hz: 300.0,
            search_ceiling_hz: 2_700.0,
        }
    }
}

/// The narrowest tone separation the search will accept, in hertz
/// (`Sound.cpp:626`).
const MINIMUM_SHIFT_HZ: f64 = 140.0;
/// The widest tone separation the search will accept, in hertz.
const MAXIMUM_SHIFT_HZ: f64 = 1_500.0;
/// Corrections smaller than this are ignored, in hertz.
const CORRECTION_DEADBAND_HZ: f64 = 2.0;
/// The fraction of an error one update applies, MMTTY's AFC time constant.
const TIME_CONSTANT: f64 = 4.0;
/// How far above the noise floor a peak must sit to be believed.
///
/// MMTTY compares its integer display values against `AFC_SQ`; the spectrum
/// here is unnormalized, so the test is a ratio, calibrated by the
/// integration tests: a detuned clean signal must pass and noise alone must
/// not.
const ACCEPTANCE_RATIO: f64 = 2.0;

/// One frequency-control step over a magnitude spectrum.
///
/// The caller owns the FFT and its cadence; this consumes one spectrum slice
/// and produces the adjusted pair, in the shape `porting.md` recommends. The
/// original's asymmetric sweep windows (`Sweep` scales the inward bound and
/// stretches the outward one by 1.2) are deliberately simplified to a
/// symmetric search between the configured floor and ceiling.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Afc {
    config: AfcConfig,
}

impl Afc {
    pub(crate) fn new(config: AfcConfig) -> Self {
        Self { config }
    }

    /// Proposes a new pair from `magnitudes`, or nothing worth applying.
    ///
    /// `bin_hz` is the width of one bin; `magnitudes.len()` bins cover zero
    /// to Nyquist.
    pub(crate) fn adjust(&self, magnitudes: &[f64], bin_hz: f64, tones: ToneSet) -> Option<ToneSet> {
        let floor_bin = libm::ceil(self.config.search_floor_hz / bin_hz) as usize;
        let ceiling_bin = ((self.config.search_ceiling_hz / bin_hz) as usize).min(magnitudes.len().saturating_sub(2));
        if floor_bin + 2 >= ceiling_bin {
            return None;
        }

        // The two strongest local peaks at least a minimum shift apart; the
        // lower is the mark. The original walks outward from the current
        // bins instead, which cannot find a pair that has drifted past the
        // midpoint of the old one — this search can, and is recorded as a
        // deliberate simplification.
        let (best_bin, best_peak) = strongest_peak(magnitudes, floor_bin, ceiling_bin, None, bin_hz, 0.0)?;
        let (other_bin, other_peak) = strongest_peak(
            magnitudes,
            floor_bin,
            ceiling_bin,
            Some(best_bin),
            bin_hz,
            MINIMUM_SHIFT_HZ,
        )?;
        let noise_floor =
            magnitudes[floor_bin..=ceiling_bin].iter().sum::<f64>() / (ceiling_bin - floor_bin + 1) as f64;
        if best_peak < noise_floor * ACCEPTANCE_RATIO || other_peak < noise_floor * ACCEPTANCE_RATIO {
            return None;
        }

        let (mark_bin, space_bin) = if best_bin < other_bin {
            (best_bin, other_bin)
        } else {
            (other_bin, best_bin)
        };
        let detected_mark = (mark_bin as f64 + interpolate(magnitudes, mark_bin)) * bin_hz;
        let detected_space = (space_bin as f64 + interpolate(magnitudes, space_bin)) * bin_hz;
        let separation = detected_space - detected_mark;
        if !(MINIMUM_SHIFT_HZ..=MAXIMUM_SHIFT_HZ).contains(&separation) {
            return None;
        }

        let shift = tones.shift_hz();
        let (target_mark, target_space) = match self.config.mode {
            AfcMode::Free => (detected_mark, detected_space),
            AfcMode::FixedShift => {
                let center = (detected_mark + detected_space) * 0.5;
                (center - shift * 0.5, center + shift * 0.5)
            }
            AfcMode::Ham => {
                let snapped = snap_shift(separation);
                (detected_mark, detected_mark + snapped)
            }
            AfcMode::Fsk => {
                let snapped = snap_shift(separation);
                let center = tones.center_hz();
                (center - snapped * 0.5, center + snapped * 0.5)
            }
        };

        let adjusted = ToneSet {
            mark_hz: self.step(tones.mark_hz, target_mark),
            space_hz: self.step(tones.space_hz, target_space),
        };
        (adjusted != tones).then_some(adjusted)
    }

    /// Applies one fractional step toward `target`, in whole hertz.
    fn step(&self, current: f64, target: f64) -> f64 {
        if (target - current).abs() < CORRECTION_DEADBAND_HZ {
            return current;
        }
        let moved = current + (target - current) / TIME_CONSTANT;
        libm::round(moved).clamp(self.config.search_floor_hz, self.config.search_ceiling_hz)
    }
}

/// Snaps a separation onto the four amateur shifts, MMTTY's HAM thresholds.
fn snap_shift(separation_hz: f64) -> f64 {
    if separation_hz > 230.0 {
        240.0
    } else if separation_hz > 210.0 {
        220.0
    } else if separation_hz > 185.0 {
        200.0
    } else {
        170.0
    }
}

/// Finds the strongest local maximum in `[from, to]`, at least
/// `separation_hz` away from `avoid`.
fn strongest_peak(
    magnitudes: &[f64],
    from: usize,
    to: usize,
    avoid: Option<usize>,
    bin_hz: f64,
    separation_hz: f64,
) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for bin in from.max(1)..=to {
        let magnitude = magnitudes[bin];
        if magnitude < magnitudes[bin - 1] || magnitude < magnitudes[bin + 1] {
            continue;
        }
        if let Some(avoid) = avoid
            && (bin as f64 - avoid as f64).abs() * bin_hz < separation_hz
        {
            continue;
        }
        if best.is_none_or(|(_, peak)| magnitude > peak) {
            best = Some((bin, magnitude));
        }
    }
    best
}

/// Parabolic sub-bin interpolation on the log magnitude around `bin`.
///
/// The FFT length targets bins no wider than 6 Hz, and this refinement is
/// what makes the original's "ignore corrections under 2 Hz" rule meaningful
/// at every sample rate.
fn interpolate(magnitudes: &[f64], bin: usize) -> f64 {
    if bin == 0 || bin + 1 >= magnitudes.len() {
        return 0.0;
    }
    let floor = 1.0e-12;
    let left = libm::log(magnitudes[bin - 1].max(floor));
    let center = libm::log(magnitudes[bin].max(floor));
    let right = libm::log(magnitudes[bin + 1].max(floor));
    let denominator = left - 2.0 * center + right;
    if denominator.abs() < 1.0e-12 {
        return 0.0;
    }
    (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use super::*;
    use rstest::rstest;

    const BIN_HZ: f64 = 5.859_375;

    fn spectrum(peaks: &[(f64, f64)]) -> Vec<f64> {
        let mut magnitudes = vec![1.0; 512];
        for &(frequency_hz, magnitude) in peaks {
            let position = frequency_hz / BIN_HZ;
            let bin = position as usize;
            let fraction = position - bin as f64;
            magnitudes[bin] += magnitude * (1.0 - fraction);
            magnitudes[bin + 1] += magnitude * fraction;
        }
        magnitudes
    }

    fn converge(afc: &Afc, magnitudes: &[f64], mut tones: ToneSet) -> ToneSet {
        for _ in 0..64 {
            match afc.adjust(magnitudes, BIN_HZ, tones) {
                Some(adjusted) => tones = adjusted,
                None => break,
            }
        }
        tones
    }

    #[test]
    fn a_detuned_pair_converges_onto_the_peaks() {
        let afc = Afc::new(AfcConfig::default());
        let magnitudes = spectrum(&[(2_155.0, 100.0), (2_325.0, 100.0)]);
        let tones = converge(&afc, &magnitudes, ToneSet::AFSK_170);
        assert!((tones.mark_hz - 2_155.0).abs() <= 3.0, "mark at {}", tones.mark_hz);
        assert!((tones.space_hz - 2_325.0).abs() <= 3.0, "space at {}", tones.space_hz);
    }

    #[test]
    fn noise_alone_moves_nothing() {
        let afc = Afc::new(AfcConfig::default());
        let magnitudes = vec![1.0; 512];
        assert_eq!(afc.adjust(&magnitudes, BIN_HZ, ToneSet::AFSK_170), None);
    }

    #[rstest]
    #[case(2_200.0, 2_300.0)]
    #[case(500.0, 2_500.0)]
    fn out_of_range_separations_are_rejected(#[case] mark_hz: f64, #[case] space_hz: f64) {
        let afc = Afc::new(AfcConfig::default());
        let magnitudes = spectrum(&[(mark_hz, 100.0), (space_hz, 100.0)]);
        let tones = ToneSet::from_center_and_shift((mark_hz + space_hz) * 0.5, 170.0);
        assert_eq!(afc.adjust(&magnitudes, BIN_HZ, tones), None);
    }

    #[test]
    fn corrections_under_two_hertz_are_ignored() {
        let afc = Afc::new(AfcConfig {
            mode: AfcMode::Free,
            ..AfcConfig::default()
        });
        let magnitudes = spectrum(&[(2_125.5, 100.0), (2_295.5, 100.0)]);
        let tones = converge(&afc, &magnitudes, ToneSet::AFSK_170);
        assert_eq!(tones, ToneSet::AFSK_170);
    }

    #[rstest]
    #[case(160.0, 170.0)]
    #[case(200.0, 200.0)]
    #[case(215.0, 220.0)]
    #[case(250.0, 240.0)]
    fn ham_mode_snaps_the_shift(#[case] real_shift: f64, #[case] snapped: f64) {
        let afc = Afc::new(AfcConfig::default());
        let magnitudes = spectrum(&[(2_125.0, 100.0), (2_125.0 + real_shift, 100.0)]);
        // Each tone settles within the 2 Hz deadband of its target, so the
        // shift can sit up to 4 Hz from the snapped value.
        let tones = converge(&afc, &magnitudes, ToneSet::AFSK_170);
        assert!(
            (tones.shift_hz() - snapped).abs() <= 4.0,
            "shift settled at {}",
            tones.shift_hz()
        );
    }

    #[test]
    fn fsk_mode_keeps_the_center_where_it_was() {
        let afc = Afc::new(AfcConfig {
            mode: AfcMode::Fsk,
            ..AfcConfig::default()
        });
        let magnitudes = spectrum(&[(2_155.0, 100.0), (2_355.0, 100.0)]);
        let tones = converge(&afc, &magnitudes, ToneSet::AFSK_170);
        assert!((tones.center_hz() - ToneSet::AFSK_170.center_hz()).abs() <= 2.5);
        assert!((tones.shift_hz() - 200.0).abs() <= 5.0);
    }
}
