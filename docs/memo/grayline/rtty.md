# Grayline RTTY

`grayline-rtty` is the third mode this repository implements and the first
that transmits as well as receives. The signal is described in
[../rtty/protocol.md](../rtty/protocol.md) and the reference implementation in
[../mmtty/](../mmtty/); this document is about the crate, and it is where the
decisions live that the MMTTY documents deliberately do not make.

## Shape

One crate, `crates/rtty`, holding the protocol model, the receive path, and
the transmit path as modules. The model — the ITA2 tables, the baud rate, the
tone pair, the framing parameters — is about two hundred lines, and its only
consumers are this crate's own `rx` and `tx`. SSTV's split earns itself
because four crates consume `grayline-sstv`; a boundary around something
smaller than a module would have nothing on either side. The day a third
consumer appears is the day to split.

Allocation-backed `no_std`, like the other cores, checked by
`cargo build -p grayline-rtty --no-default-features`.

```text
params.rs   BaudRate, ToneSet, StopElement, StopTolerance, Parity, BitLength,
            TxFraming, RxFraming
code.rs     the ITA2 tables, Ita2Decoder, Ita2Encoder
error.rs    RttyError
rx/
  frontend.rs   band-pass, normalization, the resonator pair, the comparator
  atc.rs        the automatic threshold corrector
  squelch.rs    signal strength and the squelch
  afc.rs        automatic frequency control over a spectrum slice
  framing.rs    the majority-vote start-stop machine
  config.rs     RxConfig
  event.rs      RxEvent, RxOutcome
  pipeline.rs   the wiring, and the AFC's FFT cadence
tx/
  encoder.rs    text to TxCode streams
  modulator.rs  keying, the VCO, the filters, and the gate ramp
  config.rs     TxConfig
```

`tools/rtty-cli` is `gl-rtty`: `encode` writes a WAV, `decode` reads one.
Decoded text goes to standard output (or the named file) and nothing else
does; the summary and every warning go to standard error, so text piped
onward is only the text.

## What moved into the core

Two pieces became `grayline-dsp` API for this, both on the second-caller rule
recorded in [wefax.md](wefax.md).

`filter::MovingAverage` is MMTTY's `CSmooz`, the boxcar average that is both
the default receive integrator (70 Hz) and the transmit keying filter
(100 Hz) — two callers inside this one milestone, at different rates and
window lengths, for different reasons. The original re-sums its whole ring on
every sample; this one keeps a running sum and re-sums once per lap, so it is
amortized O(1) and the drift a running sum accumulates is discarded every
lap. Until the window fills it divides by the samples seen, as the original
does — dividing by the window length would under-report the first 22 ms of a
reception and the comparator would see a false space.

`level::PeakNormalizer` is the peak-follower that lived inline in
`crates/sstv-rx/src/frontend.rs`. WEFAX did not need it because it detects
tones on a demodulated stream that carries no amplitude; RTTY detects tones
on the waveform, so the second caller arrived and the three lines were
lifted. `sstv-rx` was rewritten onto it with its tests unchanged, and a unit
test in `grayline-dsp` reproduces the old inline formula's output exactly.

Adding `DspError::{InvalidDuration, InvalidLevel}` for their validation is a
breaking change for any exhaustive `match` outside the workspace, so the next
`grayline-dsp` release takes the 0.x breaking bump.

## What deliberately did not

`ToneDetector` keeps its fixed second-order Butterworth envelope. RTTY's
integrator is a boxcar or a fifth-order Butterworth, but RTTY does not want a
`ToneDetector` at all: it wants a *pair* of channels, the rectified value
before integration, and the ATC between the rectifier and the comparator — a
different contract, assembled here from `Resonator`, `abs()`, and the
integrator, all public API. Widening `ToneDetectorDesign` would break its two
existing callers to serve a third that would not use it. If a third caller
ever wants a configurable envelope, the move is an
`EnvelopeDesign::{Butterworth, MovingAverage}` field plus a compatibility
constructor, updating `crates/sstv-rx/src/frontend.rs`,
`crates/wefax/src/rx/apt.rs`, and `crates/dsp/benches/primitives.rs` in the
same change.

The decimators in MMTTY's path (`CDECM2` and the ×4 pair) were not ported;
see below.

## Receive path

```text
normalized mono f32
  -> band-pass FIR: min(mark, space) − width .. max(mark, space) + width,
     Kaiser, order scaled from 24 at 11025 Hz
  -> PeakNormalizer (0.1 s decay)
  -> Resonator ×2, 60 Hz bandwidth (MMTTY's IIRBW)
  -> rectify
  -> integrate ×2: MovingAverage{70 Hz} default, or Butterworth{5, 40 Hz}
  -> ATC ×2 (optional, off by default as MMTTY ships)
  -> |mark − space| -> signal strength -> squelch
  -> comparator: mark >= space
  -> majority-vote framing -> ITA2 -> characters
```

All frequency ranges are built from `min(mark, space)` and
`max(mark, space)`, and `ToneSet::validate` checks both tones against
Nyquist, because `reversed()` swaps the roles: an inverted sideband is
decoded by folding the reversal into the pair at construction.

The 0.1 s normalizer decay is 4.5 bit times at 45.45 baud — fast enough to
follow selective fading, slow enough not to pump within one character.

**The limiter was not ported.** MMTTY's default limiter measures peak-to-peak
between zero crossings and clips hard at ±16384, with a ×4-oversampled
variant whose only stated effect is cosmetic on the XY scope
([../mmtty/dsp.md](../mmtty/dsp.md)). The comparator downstream is the ratio
`mark >= space`, which is scale-invariant; what normalization actually buys
is a meaningful squelch threshold and strength meter, and the decaying peak
follower buys the same thing. This is a deliberate divergence.

**ATC.** The port of `CATC`: 64-sample extremum blocks, a short remembered
list, a 100 Hz-smoothed midpoint threshold, and a 1.1× expansion around it.
The constants `ATCC` and `ATCW` are 8192 and 1024 on the original's ±16384
scale; mapping 16384 to 1.0 puts them at 0.5 and 0.0625 here. Off by
default, as every shipped MMTTY profile has it.

**Squelch.** The peak of `|mark − space|` over 100 ms windows, averaged over
eight windows. The comparator is clamped to mark while the squelch is closed
— but only while the framing machine is hunting for a start bit, as the
original does (`Rtty.cpp:859`): a character in flight when the squelch
closes is allowed to finish. Without the squelch, an offline decode prints
noise for the entire recording either side of the transmission, which makes
it the most operator-visible feature in the crate. MMTTY multiplies its
threshold tenfold while its limiter runs; this path always normalizes, so
the threshold lives on the normalized scale directly and the default (0.25)
was calibrated by the integration tests: broadband noise alone peaks near
0.15, a clean or 10 dB signal reads about 0.5, at every supported rate.

**AFC** consumes a magnitude-spectrum slice and proposes a tone pair — the
component boundary [../mmtty/porting.md](../mmtty/porting.md) recommends. The
pipeline owns the cadence: a ring over the *raw* input (the band-pass exists
to hide exactly the detuned signal the AFC is looking for), a Hann window,
the smallest power-of-two FFT with bins no wider than 6 Hz, a hop of half the
window. The search takes the two strongest local peaks at least 140 Hz apart
— the original walks outward from the current bins, which cannot find a pair
that drifted past the old midpoint — with sub-bin parabolic interpolation,
so the original's "ignore corrections under 2 Hz, round to whole hertz,
step by a quarter of the error" rules stay meaningful at every rate. All
thresholds are ratios against the visited noise floor, because the spectrum
is unnormalized. The original's asymmetric sweep windows are simplified to a
symmetric floor/ceiling, and the 300/2700 Hz clamps are configuration rather
than constants: 2700 is MMTTY's half-rate Nyquist ceiling, and this path
does not decimate. Corrections retune the resonators through
`Resonator::set_frequency`, which keeps their state, on every update — the
original rebuilds its filters only past 5 Hz of movement, but a resonator's
coefficients are three lines. The band-pass keeps its original design: a FIR
cannot retune without discarding state, and the default width leaves
hundreds of hertz of passband to move in.

**The monitor tap** (`rx/monitor.rs`) hands out `ChannelLevels`, the pair the
comparator compared: rectified, integrated, and corrected. It exists for a
display and nothing downstream reads it. `ReceivePipeline::channels` answers
with the newest pair always, which is the signed tuning figure a receive
column's header shows; `set_monitor` opens a decimated bounded ring that
`drain_monitor` empties, which is what an XY scope draws. Both are settings
rather than construction arguments, because a scope is opened and closed
while a reception runs and rebuilding the pipeline to open one would throw
away the reception being watched. The decimation is the caller's: only the
caller knows how many points it is going to draw. The ring drops its oldest
pair rather than its newest, because a caller that stopped draining is one
whose display stopped drawing, and what it wants when it returns is the
signal now. MMTTY's XY scope takes the same two channels and interpolates
them ×2, ×4, or ×8 for display resolution alone
([../mmtty/dsp.md](../mmtty/dsp.md)); this one decimates instead, for the
reason the section below gives — the channels here run at the capture rate.

## Nothing is decimated

MMTTY halves the rate before demodulating and runs its limiter at ×4; the
porting notes once listed both as candidates for the shared DSP crate. They
were not ported, and the reasons are recorded here because they are this
project's reasons, not MMTTY's:

- The receive path's contract, recorded in
  [architecture.md](architecture.md) and [wefax.md](wefax.md), is that one
  captured sample produces one demodulated decision. A third mode does not
  get to break that quietly.
- The performance ground has expired. Halving the rate halves filter orders
  and doubles per-sample time — a 1998 constraint. This path's per-sample
  cost is two resonators, two moving averages, a comparator, and one FIR,
  and the integrator is *cheaper* per sample than MMTTY's own (`CSmooz`
  re-sums its window every sample). `cargo bench -p grayline-dsp`'s
  `rtty_channel` measures the claim rather than asserting it.
- Decimation is where MMTTY's 2700 Hz space-tone ceiling comes from — the
  quarter-rate Nyquist. Without it, any pair the capture rate admits works,
  which the integration tests exercise down to a 915/1085 Hz pair.

## Framing

One state machine, the majority-vote decoder, because it is MMTTY's default
and strictly the more robust of its two: every sample in a bit period votes.
The center-sampling machine was not written — it would be a second complete
state machine offset half a bit, not a flag on this one — and one variant
does not get an enum; the day a second machine arrives they become siblings.

The machine follows the original's two-stage start (`Rtty.cpp:879`,
`:1023`): the falling edge starts a vote over the *first half* of the start
bit only, a mark majority rejects it as noise, and the second half passes
unvoted. Data bits vote over whole periods. The stop element is confirmed
over one bit — except under the 1-bit tolerance, where the window shrinks to
7/8 so confirmation ends before the next start bit begins (`Rtty.cpp:1066`).
After a character is stored the machine waits a *fraction* of the remaining
element and hunts again, which is what tolerates a fast transmitter:

| tolerance | confirm | wait after storing |
| --- | --- | --- |
| 1 | 7/8 bit | none |
| 1.42 | 1 bit | 2/5 bit |
| 1.5 | 1 bit | 3/8 bit |
| 2 | 1 bit | 7/8 bit |

These are the majority-vote decoder's fractions (`Rtty.cpp:1150`); the table
in [../mmtty/framing.md](../mmtty/framing.md) is the *ordinary* decoder's,
whose sample points sit half a bit earlier. The two differ on purpose.

Bit timing is a fractional sample accumulator, the pattern of
`crates/sstv-fskid/src/decoder.rs`. The original's integer down-counter
loses almost two samples per character at 45.45 baud and survives only
because every start bit resynchronizes; there is no reason to port the
error.

The transmit element and the receive tolerance are separate types,
`StopElement` and `StopTolerance` inside `TxFraming` and `RxFraming` —
MMTTY holds both in one five-value enumeration of which each side uses a
different subset, and the porting notes call for the split.

A character whose stop element fails still comes out of the machine, flagged
(`FramingOutcome::Code { stop_was_space }`), because
`ignore_framing_errors` needs to keep it *and* count the error, which is
what the original's `ignoreFream` does. Parity and the 6-to-8-bit lengths
are implemented and tested at the framing layer — a counter and one extra
state — but `ReceivePipeline` and `Transmitter` reject any `BitLength` but
five, because only Baudot has a character codec here; accepting the others
would decode them into nonsense, and the ASCII day is the day the
restriction lifts.

## The character code

Both tables are indexed with b1 — the first-transmitted bit — as the most
significant bit of the code, the same convention as MMTTY's `_LTR` and
`_FIG`. It is stated once, in `code.rs`, and pinned by tests on asymmetric
patterns, because flipping it is the single most likely bug in the crate.

Two deliberate divergences from MMTTY:

- **FIGS-H prints `#`.** MMTTY prints a lowercase `h` there only because `#`
  is one of its own macro metacharacters. There is no macro language here,
  so the US teleprinter assignment stands.
- **`_ ~ [ ]` remain ordinary text.** The original burns those four ASCII
  characters onto its transmit control codes; here the control codes are
  typed (`TxCode`), so nothing is taken from the character set. They are
  simply unmappable, like any other character ITA2 does not carry.

BELL follows the `CodeSet`: S-BELL (the amateur norm) rings at FIGS-S,
J-BELL at FIGS-J, and the decoder emits U+0007, which `gl-rtty decode
--strip-bell` drops. NUL, LTRS, and FIGS print nothing; the shifts surface
as `RxEvent::CaseChanged`. CR and LF are passed through exactly as received
— CR before LF is the protocol's own line ending, and translating it would
discard what was actually sent.

## Transmit path

Three keying states, `Mark`, `Space`, and `Muted`. Mute writes an exact zero
and does not advance the oscillator; it is a keying state, not the transmit
gate, and exists for the CW identification the control codes already
support. The gate — a quarter-sine amplitude ramp — shapes only the start
and the end of the whole transmission, exactly as `CAMPCONT` receives the
on/off gate `m_AmpVal` and never the keying (`Rtty.cpp:627`): ramping
amplitude at mark-space transitions would amplitude-modulate the FSK.

The keying drives the VCO's *control input* — `Vco::new(rate, space,
mark − space)` with 1.0 for mark — and the transmit smoothing filter (the
100 Hz `MovingAverage` MMTTY calls GMSK) smooths that control, not the
output. At 45.45 baud a 100 Hz window spans most of a bit, so the tone
barely settles within one; that is the original's shipped default and it
decodes, but it is why the round-trip tests run with the filter both on and
off. The 48-tap band-pass around the tones is the original's, built from
`min`/`max` so it survives reversal.

`grayline-tone-tx` is not reused, for three independent reasons: it depends
on `grayline-sstv` for `TimedTone`, it cannot express mute, and it switches
the VCO's free-running frequency at tone boundaries while RTTY moves the
control input continuously — there is no tone list to give it.

**Diddle is gap filling.** MMTTY's diddle fills the wait for the operator's
next keystroke; a file has no such wait, and `Iterator::next() == None` is a
permanent end, not an empty queue. So diddle here fills the configured
inter-character gap (`char_gap_bits`) with complete idle characters, and the
end of the code stream always ends the transmission. Two divergences follow
and are intentional: a zero gap means no diddle, and the LTRS filler
actually repeats *whichever shift the stream last announced* — the codes are
already encoded, so a bare LTRS in the middle of figures text would move the
receiver's case out from under them. The NUL filler is case-neutral and
unaffected. The original's random diddle and wait timers are transmit pacing
a file never exercises and were left out.

The lead-in and tail (0.5 s of mark each, by default) are what a real
transmission sounds like, and the receive integrators need the lead-in to
settle before the first start bit.

## Consequences an operator sees

- Rev is a guess. Nothing in the signal says which sideband the transmitter
  used, so inverted text means either station may be the wrong one.
- Unshift-on-space breaks digit groups separated by spaces unless the
  *transmitter* re-announces FIGS after each space (TX UOS). Both are on the
  command line; the receive side defaults on, as MMTTY's does, and the
  integration tests pin the mangled decode so it stays a documented
  consequence.
- A missed start bit corrupts everything until the line idles long enough
  to resynchronize. The framing has no other recovery; RTTY carries none.
- **AFC left on with no signal walks off the pair.** The search takes the two
  strongest peaks in the configured range, and on an empty band those are
  whatever the receiver's own noise is loudest at — mains harmonics near the
  300 Hz floor, in practice, which snap to a 170 Hz shift as readily as a
  transmission does. The acceptance ratio against the visited noise floor
  does not catch it, because a spectrum with a hum peak in it has a mean the
  peak clears twice over. Nothing gates the correction on there being a
  signal: the demodulator squelch cannot, since a pair far enough off to need
  AFC is exactly the pair that fails to open it, which
  `afc_pulls_a_detuned_transmission_back` pins. So `gl-rtty` leaves AFC off by
  default and `apps/rtty` does the same, switching it on being what an
  operator does once there is something to follow. The fix, when it is
  written, is a signal test of the AFC's own — MMTTY has one, `AFC_SQ`, on an
  AGC-normalized spectrum this path does not produce.

## Verification

`cargo test -p grayline-rtty` covers the ITA2 tables in both directions and
both code sets, the framing machine over synthetic bit streams at four rates
and four speeds — including a ±2 % transmitter, the 7/8 confirmation window,
parity, and the wider bit lengths — the front end across shifts and rates,
the ATC, the squelch, the AFC's convergence and rejections, and the
modulator's tones, mute, gate, and chunk independence.

`crates/rtty/tests/integration.rs` closes the loop: encode with this crate's
transmitter, decode with its receiver, at four rates, three speeds, four
tone pairs, with and without smoothing, parity, diddle, ATC, and across
packet sizes. Two vectors are independent of the transmitter: a bit pattern
derived by hand from the protocol table and synthesized with a plain sine in
the test, and the transmitter's own keying checked bit-by-bit against the
same hand sequence — so a bit-order mistake shared by both sides still
fails. Noise alone must print nothing and a 10 dB signal must decode, which
is the pair of tests that pins the default squelch threshold; a +90 Hz
detuned transmission must decode with AFC and must not without it.

There are no recorded fixtures, as everywhere else in the repository.
