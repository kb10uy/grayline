/// Alternation rate of the stop tone, in hertz.
pub const APT_STOP_HZ: f64 = 450.0;
/// Nominal length of an automatic picture transmission tone, in seconds.
pub const APT_TONE_SECONDS: f64 = 5.0;
/// Nominal length of the phasing signal, in seconds.
pub const PHASING_SECONDS: f64 = 30.0;
/// Fraction of a phasing line occupied by the white pulse.
pub const PHASING_WHITE_FRACTION: f64 = 0.05;

/// The index of cooperation, which fixes the number of pixels in a line.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ioc {
    /// The index marine and meteorological charts are sent at.
    Ioc576,
    /// The index used for satellite imagery and some coarse charts.
    Ioc288,
}

impl Ioc {
    /// Every index this crate decodes.
    pub const ALL: [Self; 2] = [Self::Ioc576, Self::Ioc288];

    /// Returns the index itself.
    pub const fn index(self) -> u32 {
        match self {
            Self::Ioc576 => 576,
            Self::Ioc288 => 288,
        }
    }

    /// Returns the pixels in one line, which is the index times pi.
    ///
    /// `576 * PI` is 1809.557 and `288 * PI` is 904.779, both rounded to
    /// nearest. Software that truncates instead produces a line one pixel
    /// narrower; the difference is visible only as a fraction of a pixel of
    /// slant across a whole picture.
    pub const fn pixels_per_line(self) -> usize {
        match self {
            Self::Ioc576 => 1_810,
            Self::Ioc288 => 905,
        }
    }

    /// Returns the alternation rate of the start tone announcing this index.
    pub const fn apt_start_hz(self) -> f64 {
        match self {
            Self::Ioc576 => 300.0,
            Self::Ioc288 => 675.0,
        }
    }
}

/// The transmitted line rate, in lines per minute.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LinesPerMinute {
    /// 60 lines per minute.
    L60,
    /// 90 lines per minute.
    L90,
    /// 100 lines per minute.
    L100,
    /// 120 lines per minute, which marine radiofax uses.
    L120,
    /// 180 lines per minute.
    L180,
    /// 240 lines per minute.
    L240,
}

impl LinesPerMinute {
    /// Every rate this crate decodes, in ascending order.
    pub const ALL: [Self; 6] = [Self::L60, Self::L90, Self::L100, Self::L120, Self::L180, Self::L240];

    /// Returns the rate itself.
    pub const fn as_lpm(self) -> u32 {
        match self {
            Self::L60 => 60,
            Self::L90 => 90,
            Self::L100 => 100,
            Self::L120 => 120,
            Self::L180 => 180,
            Self::L240 => 240,
        }
    }

    /// Returns the length of one line in seconds.
    pub fn line_seconds(self) -> f64 {
        60.0 / f64::from(self.as_lpm())
    }

    /// Returns the length of one line in samples, which may be fractional.
    pub fn samples_per_line(self, sample_rate_hz: u32) -> f64 {
        f64::from(sample_rate_hz) * 60.0 / f64::from(self.as_lpm())
    }

    /// Returns how many whole lines fit in an interval.
    pub fn lines_in(self, seconds: f64) -> usize {
        if !seconds.is_finite() || seconds <= 0.0 {
            return 0;
        }
        (seconds / self.line_seconds()) as usize
    }
}

/// The transmitted geometry: an index of cooperation and a line rate.
///
/// The two are announced separately, and only the first of them is announced
/// at all: the start tone names the index, and nothing on the air names the
/// rate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Format {
    /// The index of cooperation.
    pub ioc: Ioc,
    /// The line rate.
    pub lines_per_minute: LinesPerMinute,
}

impl Format {
    /// The geometry marine radiofax charts are sent at.
    pub const MARINE: Self = Self {
        ioc: Ioc::Ioc576,
        lines_per_minute: LinesPerMinute::L120,
    };

    /// Returns the pixels in one line.
    pub const fn pixels_per_line(self) -> usize {
        self.ioc.pixels_per_line()
    }

    /// Returns the length of one line in samples, which may be fractional.
    pub fn samples_per_line(self, sample_rate_hz: u32) -> f64 {
        self.lines_per_minute.samples_per_line(sample_rate_hz)
    }

    /// Returns the samples covering one pixel, which may be less than one.
    pub fn samples_per_pixel(self, sample_rate_hz: u32) -> f64 {
        self.samples_per_line(sample_rate_hz) / self.pixels_per_line() as f64
    }
}

/// The frequencies the black and white ends of the picture are carried at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WefaxBand {
    /// The frequency mid-gray is carried at, in hertz.
    pub center_hz: f64,
    /// The excursion to either end of the gray scale, in hertz.
    pub deviation_hz: f64,
}

impl WefaxBand {
    /// Black at 1500 Hz and white at 2300 Hz: the standard shift.
    pub const WIDE: Self = Self {
        center_hz: 1_900.0,
        deviation_hz: 400.0,
    };
    /// Black at 1750 Hz and white at 2050 Hz: the narrow-shift variant.
    pub const NARROW: Self = Self {
        center_hz: 1_900.0,
        deviation_hz: 150.0,
    };

    /// Returns the frequency black is carried at, in hertz.
    pub const fn black_hz(self) -> f64 {
        self.center_hz - self.deviation_hz
    }

    /// Returns the frequency white is carried at, in hertz.
    pub const fn white_hz(self) -> f64 {
        self.center_hz + self.deviation_hz
    }

    /// Converts a demodulated frequency to a gray level.
    ///
    /// Rounds to nearest over the 255 steps, so every level [`frequency_hz`]
    /// produces converts back to itself. A frequency outside the band clamps
    /// to its nearer end rather than wrapping.
    ///
    /// [`frequency_hz`]: Self::frequency_hz
    pub fn level(self, frequency_hz: f64) -> u8 {
        let scaled = (frequency_hz - self.black_hz()) / (self.deviation_hz * 2.0) * 255.0;
        if !scaled.is_finite() {
            return 0;
        }
        libm::round(scaled).clamp(0.0, 255.0) as u8
    }

    /// Converts a gray level to the frequency it is carried at.
    pub fn frequency_hz(self, level: u8) -> f64 {
        self.black_hz() + f64::from(level) / 255.0 * self.deviation_hz * 2.0
    }

    /// Returns the deviation from center, normalized to `-1.0..=1.0` at the
    /// band edges.
    ///
    /// This is the form the automatic picture transmission tones are detected
    /// in: on this scale a keyed black-and-white carrier is a unit square
    /// wave whatever the shift in use.
    pub fn normalized(self, frequency_hz: f64) -> f64 {
        if !frequency_hz.is_finite() {
            return 0.0;
        }
        ((frequency_hz - self.center_hz) / self.deviation_hz).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(Ioc::Ioc576, 1_810)]
    #[case(Ioc::Ioc288, 905)]
    fn pixels_per_line_rounds_the_index_times_pi(#[case] ioc: Ioc, #[case] expected: usize) {
        assert_eq!(ioc.pixels_per_line(), expected);
        let exact = f64::from(ioc.index()) * core::f64::consts::PI;
        assert_eq!(libm::round(exact) as usize, expected);
    }

    #[rstest]
    #[case(48_000)]
    #[case(44_100)]
    fn the_standard_capture_rates_give_whole_lines(#[case] rate: u32) {
        for lines_per_minute in LinesPerMinute::ALL {
            let samples = lines_per_minute.samples_per_line(rate);
            assert_eq!(samples.fract(), 0.0, "{lines_per_minute:?} at {rate} Hz gave {samples}");
        }
    }

    #[rstest]
    #[case(11_025)]
    #[case(8_000)]
    fn a_fractional_line_length_is_still_exact(#[case] rate: u32) {
        // Not every rate divides a line evenly: 11 025 Hz at 120 lines per
        // minute is 5512.5 samples. What matters is that the length is exact
        // in binary, so a clock built by multiplying from an epoch accumulates
        // nothing over a reception thousands of lines long.
        for lines_per_minute in LinesPerMinute::ALL {
            let samples = lines_per_minute.samples_per_line(rate);
            let exact = f64::from(rate) * 60.0 == samples * f64::from(lines_per_minute.as_lpm());
            assert!(exact, "{lines_per_minute:?} at {rate} Hz gave {samples}");
        }
    }

    #[rstest]
    #[case(LinesPerMinute::L60, 48_000.0)]
    #[case(LinesPerMinute::L90, 32_000.0)]
    #[case(LinesPerMinute::L100, 28_800.0)]
    #[case(LinesPerMinute::L120, 24_000.0)]
    #[case(LinesPerMinute::L180, 16_000.0)]
    #[case(LinesPerMinute::L240, 12_000.0)]
    fn line_lengths_match_the_rate(#[case] lines_per_minute: LinesPerMinute, #[case] expected: f64) {
        assert_eq!(lines_per_minute.samples_per_line(48_000), expected);
    }

    #[test]
    fn lines_in_counts_whole_lines_only() {
        assert_eq!(LinesPerMinute::L120.lines_in(30.0), 60);
        assert_eq!(LinesPerMinute::L120.lines_in(0.75), 1);
        assert_eq!(LinesPerMinute::L120.lines_in(-1.0), 0);
        assert_eq!(LinesPerMinute::L120.lines_in(f64::NAN), 0);
    }

    #[test]
    fn marine_geometry_is_the_expected_pixel_rate() {
        let format = Format::MARINE;
        assert_eq!(format.pixels_per_line(), 1_810);
        assert_eq!(format.samples_per_line(48_000), 24_000.0);
        assert!((format.samples_per_pixel(48_000) - 13.259_668).abs() < 1.0e-6);
    }

    #[rstest]
    #[case(WefaxBand::WIDE)]
    #[case(WefaxBand::NARROW)]
    fn every_level_survives_a_round_trip(#[case] band: WefaxBand) {
        for level in 0..=u8::MAX {
            assert_eq!(band.level(band.frequency_hz(level)), level, "level {level}");
        }
    }

    #[rstest]
    #[case(WefaxBand::WIDE, 1_500.0, 2_300.0)]
    #[case(WefaxBand::NARROW, 1_750.0, 2_050.0)]
    fn the_band_edges_are_the_gray_scale_ends(#[case] band: WefaxBand, #[case] black: f64, #[case] white: f64) {
        assert_eq!(band.black_hz(), black);
        assert_eq!(band.white_hz(), white);
        assert_eq!(band.level(black), 0);
        assert_eq!(band.level(white), 255);
    }

    #[test]
    fn frequencies_outside_the_band_clamp_to_its_ends() {
        let band = WefaxBand::WIDE;
        assert_eq!(band.level(1_000.0), 0);
        assert_eq!(band.level(3_000.0), 255);
        assert_eq!(band.level(f64::NAN), 0);
    }

    #[test]
    fn normalizing_maps_the_band_edges_to_plus_and_minus_one() {
        let band = WefaxBand::WIDE;
        assert_eq!(band.normalized(band.black_hz()), -1.0);
        assert_eq!(band.normalized(band.center_hz), 0.0);
        assert_eq!(band.normalized(band.white_hz()), 1.0);
        assert_eq!(band.normalized(5_000.0), 1.0);
        assert_eq!(band.normalized(f64::NAN), 0.0);
    }
}
