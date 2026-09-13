# Grayline WEFAX

`grayline-wefax` is the second mode this repository implements, and the first
one to reuse the core rather than define it. The signal it decodes is described
in [../wefax/protocol.md](../wefax/protocol.md); this document is about the
crate.

## Shape

One crate, `crates/wefax`, holding the protocol model, the receive front end,
and the decoder as modules. SSTV splits those across `grayline-sstv` and
`grayline-sstv-rx`, and the split earns itself there: the protocol model is
shared by a transmit path, a receive path, a template renderer, and a browser
build. WEFAX has one path and no transmitter, so a second crate would be a
boundary with nothing on either side of it.

Allocation-backed `no_std`, like `grayline-dsp` and `grayline-sstv`, and
checked by `cargo build -p grayline-wefax --no-default-features`.

```text
format.rs   index of cooperation, line rate, shift, derived geometry
image.rs    the growable grayscale raster
error.rs    WefaxError
rx/
  frontend.rs    band-pass, discrimination, level
  demodulator.rs PCM in, frequencies and framing tones out
  apt.rs         the tones that frame a transmission
  clock.rs       where each line begins
  phasing.rs     the fold that finds it
  decoder.rs     the state machine and pixel reconstruction
  slant.rs       refitting the clock from the picture
  input.rs       the borrowed block the decoder consumes
  event.rs       states, events, and outcomes
  config.rs      RxConfig
  pipeline.rs    the demodulator-to-decoder wiring
```

## What moved into the core

Two pieces of `crates/sstv-rx` became public `grayline-dsp` API for this:
`frequency::HilbertDiscriminator` and `detector::ToneDetector`. Both were
private to the SSTV front end until a second reading of what they need existed,
which is the point at which generalizing them stops being a guess. Both take
the band they work in from a design struct, because that is the only thing the
two callers answer differently. `crates/sstv-rx` was rewritten onto them with
its own tests unchanged, which is what shows the lift changed nothing.

The peak-follower level normalization and the receive band-pass stayed where
they are. WEFAX needs neither: its tone detection runs on the demodulated
stream, which carries no amplitude, and its band is not the SSTV band.

## Receive path

```text
normalized mono f32
  -> band-pass (1000-2800 Hz, Kaiser FIR)
  -> HilbertDiscriminator (1000-2800 Hz, 1800 Hz output cutoff)
  -> one frequency per input sample
  -> AptDetector, on the normalized deviation
  -> WefaxDecoder
  -> GrayRaster, a line at a time
```

The band runs to 2800 Hz rather than to the picture's own 2300 Hz because the
framing tones key the carrier at up to 675 Hz, which puts first-order sidebands
near 2975 Hz. Cutting the band to the picture would blunt the very detection
the front end exists to feed.

The output cutoff of 1800 Hz is a property of the signal, not of the capture
rate. Above it is the discriminator's own ripple at twice the carrier, which
starts at 3000 Hz for black; below it is the picture, and an IOC 576 line at
120 lines per minute is 3620 pixels per second. That this lands on the figure
the SSTV front end uses is not a coincidence — the same discriminator is
reading the same audio band.

As in the SSTV path, nothing is resampled or decimated: one captured sample
produces one demodulated frequency.

## The framing tones

The start and stop tones are the picture keyed black and white, so on the
demodulated stream they are square waves at 300, 450, or 675 Hz. Detection
therefore runs *after* the discriminator, on `WefaxBand::normalized`, which
makes it independent of the receiver's gain and of which shift is in use.

Three `ToneDetector`s, one per rate. A resonator's input gain follows the sine
of its angular frequency, so the three respond by more than a factor of two
differently, and differently again at each capture rate. Each is therefore
normalized at construction against a probe of its own tone: the detector is
cloned, driven with a second of a unit square wave, and its settled envelope
recorded. That is a few tens of thousands of iterations once per reception, it
is fully deterministic, and it removes the per-rate threshold table that would
otherwise be wrong the first time an unlisted rate was used.

A tone is accepted when its normalized strength passes 0.55 *and* it holds
seventy percent of the total for two continuous seconds. Both are needed: a
dithered chart excites all three roughly equally, so dominance alone is not
enough, and a picture with periodic content can favour one, so strength alone
is not either.

Unlike SSTV's VIS, the detector's latency costs nothing. A VIS header ends
exactly where the raster begins, so its envelope lag has to be corrected out;
a start tone is followed by thirty seconds of phasing, and the phasing fold is
circular, so where it thinks the tone ended does not matter.

## Phasing

The line period is known before the signal arrives, so the pulse is found by
folding rather than by hunting. Every demodulated sample is added to one of
`pixels_per_line` circular bins, every line lands its pulse on the same bins,
and everything else averages away. The pulse is then the window of the pulse's
own length whose mean stands highest — the same technique
`crates/sstv/src/rx/sync.rs` uses on a sync pulse, and for the same reason:
sliding a window of known length either way trades pulse bins for background
bins, so its extremum sits on the pulse whatever surrounds it, and no threshold
has to be chosen.

**The line rate comes out of the same fold.** Nothing on the air announces it,
so six candidate folds run in parallel and the one whose pulse is sharpest
wins; a candidate at the wrong rate smears the pulse across the whole line.
Six folds of 1810 bins is a few tens of kilobytes against a raster that runs to
megabytes.

**A rate estimate comes out of it too.** Each eight lines are folded separately
as well, the pulse is located in each, and the drift of its position across
those segments is fitted. That gives a slant estimate *before the first line is
drawn*, which the SSTV decoder has no equivalent of, and it is the single most
useful mitigation for a chart that is too featureless to correct later.

The phasing signal ends when the picture starts, not when a stopwatch says so:
the fraction of the recent signal that is black is tracked over one-second
windows, and two windows below eighty percent means the picture has begun.
Folding stops at the *first* such window, because a chart carries white
wherever it likes and enough of it would wash out a pulse that occupies a
twentieth of a line.

If the fold never produces a confident pulse, `RxConfig::phasing_fallback`
decides between drawing anyway — a picture rolled sideways is still readable,
and the operator can shift it — and giving up.

## Pixel reconstruction

The central five-eighths of each pixel's interval is averaged, exactly as the
SSTV decoder does, with the nearest sample standing in when the interval is
narrower than a sample. At the lowest capture rates that fallback is the normal
path rather than an edge case: 8000 Hz with IOC 576 at 240 lines per minute is
1.1 samples per pixel. That is a soft picture, not a failure.

Only two line periods of demodulated samples are retained, which is enough for
a phase correction that moves the raster backwards.

## Slant, and why it is on the raster

This is the largest divergence from the SSTV receive contract.

`grayline-sstv` corrects slant by retaining the demodulated stream and refitting
against it, both live and once more at completion. Twenty minutes of WEFAX at
48 kHz is 57.6 million demodulated values; even quantized to two bytes that is
115 megabytes. So there is no staging buffer and no `refine_staged` equivalent.

Instead: a line-length error displaces each row a little further than the one
before it, which is a shear and nothing else, so it can be measured on the
raster and corrected on the raster. Every sixteen lines, rows across the last
sixty-four are paired with the row eight further on, and the horizontal
displacement of each pair is found by minimizing the sum of absolute
differences over ±24 pixels, refined to sub-pixel by fitting a parabola to the
minimum. The eight-line baseline gives eight times the displacement for the
same noise.

The per-line drift is the *median* of those pairs, not the mean: a coastline or
a front that really does move sideways is one pair's answer, not the picture's.
A correction is applied only if the median absolute deviation of the pairs
stays under a pixel — the agreement test `crates/sstv/src/rx/decoder/sync_track.rs`
applies to its own observations — and only if the drift is worth the
disturbance. A pair is discarded outright if its best score does not beat the
typical score by a margin, which is what stops a featureless picture from
producing an arbitrary answer that several pairs happen to agree on.

Applying it pivots the clock on the current line, so rows already decoded keep
their positions, and then shears the raster about the same line so the rows
already drawn are corrected too. A raster of a few megabytes is one pass and a
few milliseconds.

There is no continuous phase tracker. WEFAX's only phase reference is the
phasing signal, and chasing picture content would walk the image sideways. The
phase moves only through the fold, an operator's shift, or the pivot of a slant
correction.

## Refitting a finished picture

`WefaxDecoder::refine` is the completion pass, and it exists because two things
can only be told once a reception is over.

**The rate.** The live tracker works on a sixty-four-line window so it can
correct a chart while it arrives, and that window is what sets the smallest
drift it can see: below about twenty-seven parts per million the displacement
over its baseline is lost in the noise, and twenty-seven parts per million is a
hundred pixels of shear across twenty minutes. A finished picture has no such
bound — the baseline can be hundreds of lines, and the displacement grows with
it while the noise measuring it does not. The fit runs at spans of 8, 16, 32 …
lines, each removing what it measured, so however far the picture leaned to
begin with the next span starts from a residual inside the ±24 pixels the
correlation searches over.

**The phase.** A live correction pivots on the line being decoded, so the rows
around it keep the positions they were read at and the top of the picture moves
instead. The phasing signal is the only thing that ever knew where a line
begins, so the walk those pivots caused is accumulated and put back at the end.
A shift the operator asked for is not part of that walk and is left alone —
undoing it would be the application arguing with them.

Both are applied to the raster, as the live correction is, and both move the
clock with it so the reported line rate stays honest. The application refits a
chart before it saves it, so what is written is what the operator is left
looking at; the refit is skipped when slant tracking is off, because an
operator who turned that off has said they want the geometry left alone.

**The picture stays the operator's afterwards.** Both corrections work on a
finished reception as well as a running one — a fit is an estimate, and a
chart the operator can see is wrong is one they should be able to straighten.
The slant control changes its pivot to suit: while a reception runs it pivots
on the line being decoded, so the rows around it keep the positions they were
read at, and once it has ended it pivots on the first line, because the
operator lines the top of the chart up by hand and then straightens the rest
against it.

That has one consequence for a worker that publishes what it decodes: a
correction changes the picture with no audio arriving, which is the whole of
what happens once a reception has ended. So `obey` reports whether it changed
anything the interface draws, and a change is published on its own account
rather than waiting for a block of audio that is not coming.

## Corrections compose exactly

A correction rolls each row by a whole number of pixels, and rounding each
correction on its own is not the same as rounding their sum. Every rounding
leaves an error that runs from minus half a pixel to plus half a pixel and back
as the row number climbs — a sawtooth whose period is that correction's own —
and several of those laid over each other are a visible zigzag along anything
vertical in the chart. A refit runs eight spans, and an operator nudging the
rate by hand runs one per press, so they stack up quickly.

`GrayRaster` therefore keeps the roll each row has been *asked* for, before
that was rounded, and a correction rotates a row by the difference between the
rounding of the new total and the rounding of the old one. Each row is then
rotated by the rounding of one number rather than by the sum of several
roundings, whatever route it took to get there. A row appended after a
correction starts at zero: it was read through the corrected clock and is
already where it belongs.

The rounding sends half a pixel upwards rather than away from zero, so that
adding a whole number of pixels moves the answer by exactly that many. Rounding
away from zero does not — it turns -3.5 into -4 and 0.5 into 1 — so a row
rotated by minus four and then asked to come back four pixels would land a
pixel past where it started, which is exactly what the phase restoration asks
for.

## The raster

`GrayRaster` grows a block of rows at a time — one minute of lines by default —
rather than doubling. Doubling leaves up to half the allocation unused and
copies everything retained so far each time, and a twenty-minute chart would do
that a dozen times over. The same reasoning is recorded for
`SampleBuffer::with_capacity` in `crates/sstv/src/rx/input.rs`.

IOC 576 is 1810 bytes per line, so twenty minutes at 120 lines per minute is
4.3 MB, and the worst case inside the default thirty-minute limit is 13 MB.
That the raster is this cheap is exactly what makes the slant argument above
work.

`rows(range)` and `raster_revision()` exist so a display can publish only the
rows added since it last looked, rather than cloning megabytes per frame.

## The application

`apps/wefax/` is `grayline-wefax-app`, binary `grayline-wefax`, built on
`grayline-shell` and `grayline-audio` exactly as the SSTV application is. It
carries its own `Identity`, its own Fluent catalogue, and nothing else that
`grayline-shell` already answers for.

**The window is the picture.** Two rows run along the bottom and the rest of
the window is the chart. WEFAX gives an operator very little to do — the
transmission is somebody else's — so the controls fit on one row: the index of
cooperation and the line rate to start on, automatic start and stop, slant
tracking, inversion, start/stop/clear, the phase nudges, the line-rate nudges,
and save. Under them is a status line with whatever the application last had to
say on the left and the meters and the receiver's state on the right — a
message is read from the left, and a state is looked up rather than read. What
is set once and left alone is on the menu instead: the input device, line-rate
detection, the narrow shift, automatic saving, and the language.

The line-rate controls carry the fitted rate between them, in parts per
million against the nominal one. That reading is what the clock is actually
running at rather than what has been asked for, so a manual correction the
tracker then refits away is seen to have been refitted away. It is the only
recourse for a chart with nothing to correlate — a mostly white one — where
the tracker has no observation to work from.

**A recording is dropped onto the window**, or named on the command line. The
file feeds the same bounded queue a capture stream does, so the receive worker
cannot tell one from the other and needs no path of its own; it is read no
faster than the decoder takes it, which is what keeps a twenty-minute recording
out of memory. There is no file dialog, for the same reason the application
opens its directories in the operator's file manager rather than listing them.

The feeder is `grayline-audio::WavSource`, shared with RTTY. The WEFAX
application supplies its decoder's minimum sample rate when opening a file.

Two things the file path exposed that a live device hides. A queue told more
than it can hold counts the rest as dropped, which reads as a hole in the
timeline and restarts the reception — so the feeder offers only what fits,
because the samples that did not fit have not happened yet. And publishing to
the interface is throttled, so the last block before the audio stops would
never be shown: a recording ends, and a chart finished on its final samples
would appear to have stopped part way through. The worker therefore publishes
once more when the input dries up.

**The chart is turned a quarter turn.** A line is 1810 pixels against a window
a few hundred points tall, so one decoded line is drawn as one column and time
runs left to right. The whole of a line is therefore always visible, and the
newest end of the chart stays against the right edge once it is longer than the
window holds.

`ui::strip::Strip` is what draws it, and its shape follows from the raster's
size. The texture is a ring of columns that doubles up to 8192, which is more
than the decoder's own line limit ever produces, so in practice it never wraps.
A decoded line is uploaded with `set_partial` as a single column rather than by
replacing the texture: a chart is megabytes, and re-uploading it thirty times a
second would cost more than decoding it. The gray levels are kept in RAM beside
the texture as well, because a correction rewrites lines already drawn and the
operator can save the picture at any point.

The worker publishes `StripUpdate`s rather than pictures, and the decoder's
`raster_revision()` is what tells it whether to append or resend: that counter
advances once per decoded line and once per correction, so a revision that ran
further than the line count means a correction moved what was already sent.
Uncollected updates merge in the mailbox, so an interface that missed a frame
gets one run of columns rather than a queue of them.

## Consequences an operator sees

- A stop tone has to be held for two seconds before it is trusted, so the last
  few lines of every reception are the tone rather than the picture. Every
  other radiofax program shows the same thing.
- Automatic starting can be defeated, and starting by hand is always available.
  Selective fading makes real keying ragged, and a dithered chart carries
  genuine energy at 300 Hz, so both misses and false starts are expected.
- A chart that is nearly all white will drift, because there is nothing to
  correlate and no synchronization pulse to fall back on. The rate the phasing
  signal handed over is what carries such a reception.

## Verification

`cargo test -p grayline-wefax` covers the model tables, the raster, the front
end at 8000, 11 025 and 48 000 Hz, the tone detectors across rates, the fold's
pulse position and rate estimate, line-rate inference across all six
candidates, pixel placement including the one-sample-per-pixel corner, slant
tracking on a picture sent at ±300 parts per million, and the streaming
contract. `crates/wefax/tests/integration.rs` receives complete synthesized
transmissions end to end and checks that packet size does not change the
picture.

There are no recorded fixtures. Every signal is built in the test that needs
it, from a phase-continuous oscillator, exactly as the SSTV front end's own
integration test does.

`tools/wefax-cli` decodes a WAV recording to a grayscale image with
`gl-wefax decode`, and is the way to try the path against something real.
