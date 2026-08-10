use crate::{
    error::WefaxError,
    format::{Format, Ioc},
    image::GrayRaster,
    rx::phasing::PhasingResult,
};

/// Observable state of a receive decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxState {
    /// Waiting for a start tone, or for a manual start.
    Idle,
    /// A start tone is being heard; its index of cooperation is known.
    Starting {
        /// The index the tone announced.
        ioc: Ioc,
    },
    /// Folding the phasing signal to find where a line begins.
    Phasing,
    /// Drawing the raster.
    Imaging {
        /// How many lines have been decoded.
        lines: usize,
    },
    /// The transmission ended on its own stop tone.
    Complete {
        /// How many lines were decoded.
        lines: usize,
    },
    /// Decoding ended for a reason other than the stop tone.
    Stopped {
        /// How many lines were decoded.
        lines: usize,
        /// Why decoding ended.
        reason: StopReason,
    },
}

impl RxState {
    /// Returns how many lines have been decoded, in every state that has any.
    pub const fn lines(self) -> usize {
        match self {
            Self::Idle | Self::Starting { .. } | Self::Phasing => 0,
            Self::Imaging { lines } | Self::Complete { lines } | Self::Stopped { lines, .. } => lines,
        }
    }

    /// Returns whether no further input will change anything.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Complete { .. } | Self::Stopped { .. })
    }
}

/// Why decoding ended without a stop tone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    /// The caller stopped it.
    Manual,
    /// The raster reached the line limit it was created with.
    LineLimit,
    /// The caller reported the signal gone.
    SignalLost,
    /// The phasing signal never produced a confident pulse.
    PhasingNotAcquired,
}

/// Something the decoder decided, in the order it decided it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RxEvent {
    /// A start tone named the index of cooperation.
    AptStartDetected {
        /// The index the tone announced.
        ioc: Ioc,
        /// Absolute position the tone was accepted at.
        sample: u64,
    },
    /// Both halves of the geometry are now known.
    FormatSelected {
        /// The geometry decoding will use.
        format: Format,
    },
    /// The phasing signal placed the start of a line.
    PhasingAcquired {
        /// Where line zero begins, in absolute samples.
        epoch_samples: f64,
        /// Where the pulse sat within the line, in pixels.
        offset_pixels: f64,
        /// How far the pulse stood above the rest of the line.
        contrast: f32,
        /// How many lines the fold covered.
        lines: usize,
    },
    /// One line of the raster is complete.
    LineDecoded {
        /// The line's index.
        line: usize,
    },
    /// The line clock was refitted, and the rows already drawn moved with it.
    SlantAdjusted {
        /// The line the correction pivoted on.
        line: usize,
        /// The corrected line length in samples.
        samples_per_line: f64,
        /// How wrong the previous length was.
        error_ppm: f64,
    },
    /// The raster was moved sideways, and the rows already drawn with it.
    PhaseAdjusted {
        /// The line the shift was applied at.
        line: usize,
        /// How far the raster moved, in pixels.
        displacement_pixels: f64,
    },
    /// A stop tone ended the transmission.
    AptStopDetected {
        /// Absolute position the tone was accepted at.
        sample: u64,
    },
    /// Decoding ended without a stop tone.
    Stopped {
        /// Why decoding ended.
        reason: StopReason,
    },
}

/// What refitting a finished picture changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Refinement {
    /// The line length the whole picture fits, in samples.
    pub samples_per_line: f64,
    /// How far that stood from the length decoding ended on, in parts per
    /// million.
    pub error_ppm: f64,
    /// How far the picture was moved back towards the phase the phasing
    /// signal established, in pixels.
    pub displacement_pixels: f64,
}

/// What one call to the decoder consumed and left it in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxProcessResult {
    /// How many samples of the block were taken.
    pub consumed: usize,
    /// The state the decoder is now in.
    pub state: RxState,
}

/// A failure part way through a block, with what had already been taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxProcessError {
    consumed: usize,
    error: WefaxError,
}

impl RxProcessError {
    pub(crate) const fn new(consumed: usize, error: WefaxError) -> Self {
        Self { consumed, error }
    }

    /// Returns how many samples were taken before the failure.
    ///
    /// A caller resuming after the failure sends the rest of the block from
    /// here, rather than sending it again from the start.
    pub const fn consumed(self) -> usize {
        self.consumed
    }

    /// Returns what went wrong.
    pub const fn error(self) -> WefaxError {
        self.error
    }
}

impl From<RxProcessError> for WefaxError {
    fn from(error: RxProcessError) -> Self {
        error.error
    }
}

/// Everything a finished reception leaves behind.
#[derive(Clone, Debug, PartialEq)]
pub struct RxOutcome {
    /// The state the decoder ended in.
    pub state: RxState,
    /// The geometry it settled on, if it got that far.
    pub format: Option<Format>,
    /// The picture, if any line was decoded.
    pub raster: Option<GrayRaster>,
    /// The line length decoding ended on.
    pub samples_per_line: Option<f64>,
    /// What the phasing signal reported.
    pub phasing: Option<PhasingResult>,
}
