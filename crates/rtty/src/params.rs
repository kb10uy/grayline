//! Baud rate, tone pairs, and start-stop framing parameters.

use crate::RttyError;

/// A signalling rate in bits per second.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct BaudRate(f64);

impl BaudRate {
    /// The amateur standard: 60 words per minute from the Teletype Model 15.
    pub const AMATEUR: Self = Self(45.45);

    /// Validates a rate.
    ///
    /// Any finite positive rate is accepted here; MMTTY's speed selector
    /// offers 22 through 300 baud, and the real lower bound depends on the
    /// sample rate, which the pipelines check as samples per bit.
    pub fn new(bits_per_second: f64) -> Result<Self, RttyError> {
        if !bits_per_second.is_finite() || bits_per_second <= 0.0 {
            return Err(RttyError::InvalidBaudRate);
        }
        Ok(Self(bits_per_second))
    }

    /// Returns the rate in bits per second.
    pub const fn bits_per_second(self) -> f64 {
        self.0
    }

    /// Returns the duration of one bit in seconds.
    pub const fn bit_seconds(self) -> f64 {
        1.0 / self.0
    }
}

impl Default for BaudRate {
    fn default() -> Self {
        Self::AMATEUR
    }
}

/// The mark and space frequencies of one FSK channel, in hertz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneSet {
    /// The tone for a logical one and the idle line.
    pub mark_hz: f64,
    /// The tone for a logical zero.
    pub space_hz: f64,
}

impl ToneSet {
    /// The common AFSK pair: 2125 Hz mark, 2295 Hz space, a 170 Hz shift.
    pub const AFSK_170: Self = Self {
        mark_hz: 2_125.0,
        space_hz: 2_295.0,
    };

    /// Builds the pair around a center frequency, mark below space.
    pub fn from_center_and_shift(center_hz: f64, shift_hz: f64) -> Self {
        Self {
            mark_hz: center_hz - shift_hz * 0.5,
            space_hz: center_hz + shift_hz * 0.5,
        }
    }

    /// Swaps mark and space, which is how an inverted sideband is decoded.
    pub const fn reversed(self) -> Self {
        Self {
            mark_hz: self.space_hz,
            space_hz: self.mark_hz,
        }
    }

    /// Returns the separation between the tones in hertz.
    pub fn shift_hz(self) -> f64 {
        (self.space_hz - self.mark_hz).abs()
    }

    /// Returns the arithmetic mean of the tones in hertz.
    pub const fn center_hz(self) -> f64 {
        (self.mark_hz + self.space_hz) * 0.5
    }

    /// Returns the lower of the two tones, whichever role it plays.
    pub fn low_hz(self) -> f64 {
        self.mark_hz.min(self.space_hz)
    }

    /// Returns the higher of the two tones, whichever role it plays.
    pub fn high_hz(self) -> f64 {
        self.mark_hz.max(self.space_hz)
    }

    /// Checks that both tones are usable at a sample rate.
    ///
    /// Both tones are checked because [`ToneSet::reversed`] swaps their
    /// roles, so a valid pair has to stay valid with the roles exchanged.
    pub fn validate(self, sample_rate_hz: f64) -> Result<(), RttyError> {
        let nyquist = sample_rate_hz * 0.5;
        for tone in [self.mark_hz, self.space_hz] {
            if !tone.is_finite() || tone <= 0.0 || tone >= nyquist {
                return Err(RttyError::InvalidToneSet);
            }
        }
        if self.mark_hz == self.space_hz {
            return Err(RttyError::InvalidToneSet);
        }
        Ok(())
    }
}

impl Default for ToneSet {
    fn default() -> Self {
        Self::AFSK_170
    }
}

/// The stop element a transmitter sends.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StopElement {
    /// One bit, the minimum the framing permits.
    One,
    /// One and a half bits, what mechanical teleprinters produced and what
    /// amateur RTTY transmits.
    #[default]
    OneAndAHalf,
    /// Two bits, the legacy commercial element.
    Two,
}

impl StopElement {
    /// Returns how much of the element extends beyond the first bit.
    pub const fn extra_bits(self) -> f64 {
        match self {
            Self::One => 0.0,
            Self::OneAndAHalf => 0.5,
            Self::Two => 1.0,
        }
    }
}

/// How a receiver confirms a stop bit and waits before the next start bit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StopTolerance {
    /// Expect only one bit of stop.
    One,
    /// Resynchronize slightly early against a 1.5-bit element, MMTTY's
    /// empirical compromise for transmitters running fast.
    #[default]
    Ratio142,
    /// Expect the full mechanical 1.5-bit element.
    OneAndAHalf,
    /// Expect a two-bit element.
    Two,
}

impl StopTolerance {
    /// Returns the length of the stop confirmation window in bits.
    ///
    /// Only the one-bit tolerance shortens the window, to 7/8 of a bit, so
    /// that confirmation ends before the next start bit can begin
    /// (`Rtty.cpp:1066`).
    pub const fn confirm_bits(self) -> f64 {
        match self {
            Self::One => 0.875,
            Self::Ratio142 | Self::OneAndAHalf | Self::Two => 1.0,
        }
    }

    /// Returns how long to wait after storing a character, in bits.
    ///
    /// These are the majority-vote decoder's fractions (`Rtty.cpp:1150`),
    /// which differ from the ordinary decoder's table in
    /// `docs/memo/mmtty/framing.md`: the majority machine sits half a bit
    /// later, so it returns to idle earlier by the same amount.
    pub const fn resync_bits(self) -> f64 {
        match self {
            Self::One => 0.0,
            Self::Ratio142 => 0.4,
            Self::OneAndAHalf => 0.375,
            Self::Two => 0.875,
        }
    }
}

/// The parity bit between the data bits and the stop element.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Parity {
    /// No parity bit, the Baudot norm.
    #[default]
    None,
    /// The bit MMTTY labels even: mark when the count of data marks is even.
    Even,
    /// The bit MMTTY labels odd: mark when the count of data marks is odd.
    Odd,
    /// A constant mark.
    Mark,
    /// A constant space.
    Space,
}

impl Parity {
    /// Returns the bit a transmitter sends for `data_marks` mark data bits,
    /// or nothing when parity is off.
    pub const fn transmitted_bit(self, data_marks: u32) -> Option<bool> {
        match self {
            Self::None => None,
            Self::Even => Some(data_marks.is_multiple_of(2)),
            Self::Odd => Some(!data_marks.is_multiple_of(2)),
            Self::Mark => Some(true),
            Self::Space => Some(false),
        }
    }

    /// Returns whether a received parity bit is valid for `data_marks`.
    pub const fn is_valid(self, data_marks: u32, bit: bool) -> bool {
        match self.transmitted_bit(data_marks) {
            Some(expected) => expected == bit,
            None => true,
        }
    }
}

/// The number of data bits in one character.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitLength(u8);

impl BitLength {
    /// Five bits, the Baudot character.
    pub const FIVE: Self = Self(5);

    /// Validates a length between five (Baudot) and eight (ASCII).
    pub fn new(bits: u8) -> Result<Self, RttyError> {
        if !(5..=8).contains(&bits) {
            return Err(RttyError::InvalidBitLength);
        }
        Ok(Self(bits))
    }

    /// Returns the number of data bits.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl Default for BitLength {
    fn default() -> Self {
        Self::FIVE
    }
}

/// The framing a transmitter produces.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TxFraming {
    /// The signalling rate.
    pub baud: BaudRate,
    /// Data bits per character.
    pub bits: BitLength,
    /// The parity bit, if any.
    pub parity: Parity,
    /// The stop element to send.
    pub stop: StopElement,
}

/// The framing a receiver expects.
///
/// The transmit element and the receive tolerance are separate types on
/// purpose: MMTTY holds them in one enumeration whose values only partly
/// coincide, and the two sides never need each other's cases.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RxFraming {
    /// The signalling rate.
    pub baud: BaudRate,
    /// Data bits per character.
    pub bits: BitLength,
    /// The parity bit, if any.
    pub parity: Parity,
    /// The stop tolerance to apply.
    pub stop: StopTolerance,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn center_and_shift_round_trip() {
        let tones = ToneSet::from_center_and_shift(2_210.0, 170.0);
        assert_eq!(tones, ToneSet::AFSK_170);
        assert_eq!(tones.center_hz(), 2_210.0);
        assert_eq!(tones.shift_hz(), 170.0);
    }

    #[test]
    fn reversing_twice_is_the_identity() {
        assert_eq!(ToneSet::AFSK_170.reversed().reversed(), ToneSet::AFSK_170);
    }

    #[test]
    fn low_and_high_ignore_the_roles() {
        let reversed = ToneSet::AFSK_170.reversed();
        assert_eq!(reversed.low_hz(), 2_125.0);
        assert_eq!(reversed.high_hz(), 2_295.0);
    }

    #[rstest]
    #[case(StopTolerance::One, 0.875, 0.0)]
    #[case(StopTolerance::Ratio142, 1.0, 0.4)]
    #[case(StopTolerance::OneAndAHalf, 1.0, 0.375)]
    #[case(StopTolerance::Two, 1.0, 0.875)]
    fn stop_tolerance_matches_the_majority_decoder_table(
        #[case] tolerance: StopTolerance,
        #[case] confirm: f64,
        #[case] resync: f64,
    ) {
        assert_eq!(tolerance.confirm_bits(), confirm);
        assert_eq!(tolerance.resync_bits(), resync);
    }

    #[rstest]
    #[case(0.0)]
    #[case(-45.45)]
    #[case(f64::NAN)]
    #[case(f64::INFINITY)]
    fn invalid_baud_rates_are_rejected(#[case] rate: f64) {
        assert_eq!(BaudRate::new(rate).unwrap_err(), RttyError::InvalidBaudRate);
    }

    #[rstest]
    #[case(4)]
    #[case(9)]
    fn out_of_range_bit_lengths_are_rejected(#[case] bits: u8) {
        assert_eq!(BitLength::new(bits).unwrap_err(), RttyError::InvalidBitLength);
    }

    #[rstest]
    #[case(ToneSet { mark_hz: 2_125.0, space_hz: 3_000.0 })]
    #[case(ToneSet { mark_hz: 3_000.0, space_hz: 2_125.0 })]
    #[case(ToneSet { mark_hz: 2_125.0, space_hz: 2_125.0 })]
    #[case(ToneSet { mark_hz: 0.0, space_hz: 2_295.0 })]
    #[case(ToneSet { mark_hz: f64::NAN, space_hz: 2_295.0 })]
    fn validation_rejects_unusable_pairs_in_either_role(#[case] tones: ToneSet) {
        assert_eq!(tones.validate(6_000.0).unwrap_err(), RttyError::InvalidToneSet);
        assert_eq!(
            tones.reversed().validate(6_000.0).unwrap_err(),
            RttyError::InvalidToneSet
        );
    }

    #[test]
    fn parity_bits_are_self_consistent() {
        for parity in [Parity::Even, Parity::Odd, Parity::Mark, Parity::Space] {
            for marks in 0..6 {
                let bit = parity.transmitted_bit(marks).unwrap();
                assert!(parity.is_valid(marks, bit));
                assert!(!parity.is_valid(marks, !bit));
            }
        }
        assert!(Parity::None.is_valid(3, true));
        assert!(Parity::None.is_valid(3, false));
    }
}
