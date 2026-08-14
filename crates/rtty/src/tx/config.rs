use crate::{
    code::CodeSet,
    params::{ToneSet, TxFraming},
};

/// What the transmitter sends while a character gap has room for it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Diddle {
    /// Plain mark idle.
    #[default]
    None,
    /// Complete LTRS characters, the usual diddle.
    Ltrs,
    /// Complete NUL characters, MMTTY's "BLK" option.
    Blank,
}

/// Everything a transmitter needs to know.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TxConfig {
    /// The mark and space tones.
    pub tones: ToneSet,
    /// The start-stop framing to produce.
    pub framing: TxFraming,
    /// Swaps the tones, for an inverted sideband.
    pub reverse: bool,
    /// Which BELL convention the encoder writes.
    pub code_set: CodeSet,
    /// Sends every shift character twice, so losing one loses nothing.
    pub double_shift: bool,
    /// Re-announces the case after a space in figures, for UOS receivers.
    pub tx_unshift_on_space: bool,
    /// What fills the character gap, when there is one.
    ///
    /// MMTTY's diddle fills the wait for the operator's next keystroke; a
    /// file has no such wait, so here diddle fills the configured gap
    /// instead, and a zero gap means no diddle at all.
    pub diddle: Diddle,
    /// Idle inserted between characters, in bit periods.
    pub char_gap_bits: f64,
    /// The keying smoothing filter, what MMTTY calls GMSK, or none.
    ///
    /// The default 100 Hz window is MMTTY's shipped value. At 45.45 baud it
    /// spans almost a whole bit, so the tone barely settles within one — the
    /// original ships this way and it decodes, but it is why the round-trip
    /// tests run with the filter both on and off.
    pub smoothing_hz: Option<f64>,
    /// Whether to band-pass the output around the tones.
    pub band_pass: bool,
    /// Length of the on-gate quarter-sine fade at each end, in seconds.
    pub ramp_seconds: f64,
    /// Peak output amplitude in the `[-1, 1]` PCM scale.
    pub amplitude: f64,
    /// Mark idle before the first character, in seconds.
    ///
    /// A real transmission idles at mark before text; the receive
    /// integrators need it to settle before the first start bit.
    pub lead_in_seconds: f64,
    /// Mark idle after the last character, in seconds.
    pub tail_seconds: f64,
}

impl Default for TxConfig {
    fn default() -> Self {
        Self {
            tones: ToneSet::default(),
            framing: TxFraming::default(),
            reverse: false,
            code_set: CodeSet::default(),
            double_shift: false,
            tx_unshift_on_space: false,
            diddle: Diddle::default(),
            char_gap_bits: 0.0,
            smoothing_hz: Some(100.0),
            band_pass: true,
            ramp_seconds: 0.005,
            amplitude: 0.9,
            lead_in_seconds: 0.5,
            tail_seconds: 0.5,
        }
    }
}
