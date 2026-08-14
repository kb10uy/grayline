# MMTTY Rust Porting Notes

This document describes proposed boundaries for porting the original MMTTY
implementation in `docs/reference/mmtty` to Rust. The structure of the original
application is documented in [architecture.md](architecture.md), its signal
processing in [dsp.md](dsp.md), and its framing and character coding in
[framing.md](framing.md). The protocol itself is in
[rtty/protocol.md](../rtty/protocol.md).

## What Makes MMTTY Easier Than MMSSTV

MMSSTV's port is complicated by the fact that essential codec behavior lives in
the VCL main form: raster synchronization, line generation, and VIS handling
are all in `Main.cpp`. MMTTY does not have that problem. `CFSKDEM` takes
samples and produces bytes; `CFSKMOD` takes bytes and produces samples. Both
are ordinary C++ classes with no VCL types in their interfaces, and the
interface between the modem and the application is a byte queue.

What does live in `Main.cpp`, and would have to be reproduced outside the
modem, is the *policy* around it:

- The frequency-domain AFC decision (`TSound::DoAFC`, in `Sound.cpp`).
- Transmit queue pacing and the TX/RX turnaround sequence.
- Macro expansion and CW identifier generation.
- The parallel serial-port transmit path and its synchronization with audio.

Of those, only AFC is arguably part of the receiver. The rest is application
behavior.

## Proposed Module Boundaries

Mapped onto the module vocabulary in `AGENTS.md`:

| Rust module | Original implementation |
| --- | --- |
| `dsp` | `fir.cpp` and `Fft.cpp`, plus the filter classes duplicated in `Rtty.cpp` |
| `rtty_modes` | Baud rates, shifts, tone defaults, framing parameter sets |
| `demodulator` | `CFSKDEM` up to the comparator: limiter, decimator, the four discriminators, integrator, ATC, squelch |
| `rx_decoder` | `CFSKDEM::DoFSK` framing state machines and `CRTTY::ConvAscii` |
| `tx_encoder` | `CRTTY::ConvRTTY`, the control codes, diddle policy, and the `CFSKMOD` state machine |
| `modulator` | `CFSKMOD`'s VCO, TX LPF, TX BPF, and amplitude ramp |
| `audio` | Platform audio replacing `CWave` and `CXWave` |
| `application` | UI, macros, logging, PTT, CAT, TNC emulation, remote interface |

Note that AFC does not sit cleanly in any of them. It reads the FFT array,
which is produced from the audio block before the demodulator, and writes tone
frequencies into the demodulator. A reasonable target is to make it a
`demodulator`-level component that consumes a spectrum slice and emits a tone
pair, with the FFT itself in `dsp` and the timing decision in `application`.

## Required Core Scope

A faithful receiver needs all of:

- The limiter, including its AGC variant and the optional four-times
  oversampled form.
- Half-rate decimation and the resulting Nyquist constraint on the space tone.
- At least the IIR resonator discriminator; the FIR, PLL, and sliding-FFT
  variants are alternatives worth having but not required for a first port.
- Rectification and both integrator forms.
- ATC, because signals with echo are the case it was added for.
- The squelch, which suppresses framing on noise.
- Both framing state machines. The majority-vote decoder is the default and is
  the more robust of the two.
- The full stop-element wait table, since returning early is what tolerates a
  fast transmitter.

A faithful transmitter needs the framing state machine, the three keying states
(mark, space, and muted), the control codes, diddle with its wait policy, and
the amplitude ramp. The TX filters are optional quality features.

## Type and State Modeling

The original expresses several things as untyped integers that a Rust port
should make explicit:

- `m_type` selects among four structurally different discriminators. This is an
  enum whose variants own different state, not a mode flag on one struct.
- `m_StopLen` conflates a receive tolerance with a transmit element length,
  with two of its five values meaning the same thing on one side and different
  things on the other. Split it.
- `m_mode` is a state machine index whose values 0–8 and 256–262 are two
  different machines. Two enums, or one enum with two families of variants.
- `m_out` is tri-state: 1 mark, 0 space, negative muted. Make it an enum; the
  muted state is load-bearing for CW identification.
- `m_outfig` is tri-state: letters, figures, unknown. The unknown state is what
  implements TX UOS.
- `m_DisDiddle` is a counter where positive means "disabled for N more
  samples", zero means enabled, and −1 means "disabled indefinitely".

The character ring inside `CFSKDEM` receives writes from both the demodulator
and the modulator, so "received text" is really an echo of the channel. Whether
to keep that behavior is a product decision, but it should be an explicit one.

## Sampling and Timing

The original's split between `SampBase`, `SampFreq`, and `DemSamp` exists to
allow sound-card clock calibration without requesting an odd hardware rate, and
to run the demodulator at half rate. Both are worth keeping, but they should be
explicit rate parameters rather than globals.

Bit timing is an integer sample counter with no fractional accumulator. A port
should use a fractional accumulator, which costs nothing and removes a
per-bit-rate accuracy limit that the original works around only because RTTY
frames are short.

## Concurrency

The original shares the demodulator's character ring, the modulator's character
queue, the serial transmit queue, and the FFT result array between threads
through plain counters with no atomics, and the AFC path suspends the audio
thread to rebuild filter coefficients. A Rust port should give each stream an
owner and a bounded single-producer/single-consumer channel, and should apply
tone-frequency changes as a message the demodulator consumes at a sample
boundary rather than by stopping the producer.

## Platform Separation

These should stay outside the portable core:

- VCL controls, forms, timers, and canvases.
- WinMM audio callbacks and `WAVEHDR` buffers.
- Serial port handles, `TransmitCommChar`, and the EXTFSK DLL interface.
- Registered window messages, shared memory, and the remote-control protocol.
- TNC emulation, logger and radio plug-in interfaces.

## Shared Ground With the SSTV Port

`fir.cpp` and `Fft.cpp` are common to both programs, so `grayline-dsp` should
already cover Kaiser-windowed FIR design, `CFIR2`-style convolution, the
Butterworth IIR design, and the FFT. What MMTTY adds and MMSSTV does not have
is the two-pole resonator (`CIIRTANK`), the half-band decimator (`CDECM2`), the
four-times interpolate/decimate pair used by the oversampled limiter, and the
recursive sliding DFT (`CSlideFFT`). Those are the pieces to add to the shared
DSP crate rather than to an RTTY-specific one.

Conversely, nothing in the RTTY path needs image buffers, color conversion, or
the SSTV timing model, and the RTTY receiver's output is a character stream
rather than a raster — so the receive contract that
[grayline/wefax.md](../grayline/wefax.md) already had to part company with does
not apply here at all.
