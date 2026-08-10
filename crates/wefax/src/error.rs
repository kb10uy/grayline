use grayline_dsp::DspError;
use thiserror::Error;

/// An invalid WEFAX configuration or stream.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum WefaxError {
    /// A raster dimension is zero.
    #[error("raster width, height limit, and growth must all be positive")]
    InvalidRasterSize,
    /// The raster has reached the line limit it was created with.
    #[error("reception reached its limit of {max_lines} lines")]
    LineLimitReached {
        /// The limit the raster was created with.
        max_lines: usize,
    },
    /// The capture rate is below what the front end can demodulate.
    #[error("sample rate {0} Hz is below the supported minimum")]
    SampleRateTooLow(u32),
    /// An input sample is infinite or not a number.
    #[error("input sample at offset {index} is not finite")]
    NonFiniteSample {
        /// Offset of the offending sample within the packet.
        index: usize,
    },
    /// The absolute sample position would exceed what a `u64` can hold.
    #[error("absolute sample position overflowed")]
    SamplePositionOverflow,
    /// A line clock's epoch or line length is not a usable number.
    #[error("line clock epoch and length must be finite, and the length positive")]
    InvalidLineClock,
    /// No line rate was offered for the phasing signal to choose between.
    #[error("no candidate line rate was offered")]
    NoLineRateCandidates,
    /// A block does not continue from where the previous one ended.
    #[error("expected the block to start at sample {expected}, but it started at {actual}")]
    DemodulatedGap {
        /// Where the block had to start.
        expected: u64,
        /// Where it actually started.
        actual: u64,
    },
    /// A demodulated sample is infinite or not a number.
    #[error("demodulated sample at offset {offset} is not finite")]
    InvalidDemodulatedSample {
        /// Offset of the offending sample within the block.
        offset: usize,
    },
    /// A sample a pixel needed had already been discarded.
    #[error("sample {sample} is no longer retained")]
    SampleDiscarded {
        /// The position that was asked for.
        sample: u64,
    },
    /// Neither a start tone nor the configuration named a geometry.
    #[error("no index of cooperation or line rate has been selected")]
    FormatNotSelected,
    /// The reception has already ended.
    #[error("the reception has already ended")]
    AlreadyComplete,
    /// The operation needs a reception that is drawing its raster.
    #[error("no raster is being drawn")]
    NotImaging,
    /// A signal-processing stage rejected its configuration.
    #[error(transparent)]
    Dsp(#[from] DspError),
}
