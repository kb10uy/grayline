use grayline_dsp::DspError;
use thiserror::Error;

/// An invalid RTTY configuration or stream.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RttyError {
    /// A baud rate is non-positive or not finite.
    #[error("baud rate must be finite and positive")]
    InvalidBaudRate,
    /// A character length is outside the start-stop framing range.
    #[error("bit length must be between 5 and 8")]
    InvalidBitLength,
    /// A pipeline was asked for a character length no codec exists for.
    ///
    /// The framing layer carries 6 to 8 data bits, but only five-bit Baudot
    /// has a character codec, so accepting the others here would decode them
    /// into nonsense rather than text.
    #[error("only five-bit Baudot is supported end to end")]
    UnsupportedBitLength,
    /// A tone pair is not usable at the given sample rate.
    #[error("mark and space must be finite, positive, distinct, and below Nyquist")]
    InvalidToneSet,
    /// The capture rate is below what the front end can demodulate.
    #[error("sample rate {0} Hz is below the supported minimum")]
    SampleRateTooLow(u32),
    /// The sample rate leaves too few samples in one bit for a majority vote.
    #[error("fewer than 8 samples per bit; raise the sample rate or lower the baud rate")]
    TooFewSamplesPerBit,
    /// An input sample is infinite or not a number.
    #[error("input sample at offset {index} is not finite")]
    NonFiniteSample {
        /// Offset of the offending sample within the packet.
        index: usize,
    },
    /// A character has no ITA2 code and cannot be transmitted.
    #[error("character {character:?} at byte offset {offset} has no ITA2 code")]
    UnmappableCharacter {
        /// The character that could not be encoded.
        character: char,
        /// Byte offset of the character within the input text.
        offset: usize,
    },
    /// A transmit duration is negative or not finite.
    #[error("lead-in, tail, ramp, and character gap must be finite and non-negative")]
    InvalidTransmitTiming,
    /// A signal-processing stage rejected its configuration.
    #[error(transparent)]
    Dsp(#[from] DspError),
}
