# MMTTY DSP Implementation

This document describes the signal processing in the original MMTTY source. The
application structure is covered in [architecture.md](architecture.md), the
bit-level framing and character codec in [framing.md](framing.md), and the
signal itself in [rtty/protocol.md](../rtty/protocol.md).

The author's own account of these algorithms is `digital.txt` in the submodule.
It predates the FFT demodulator and the 2010 additions, so it describes three
demodulators where the code has four.

## Source Map

| Source | Responsibility |
| --- | --- |
| `fir.h`, `fir.cpp` | Filter design, convolution, resonators, LMS, resampling |
| `Rtty.h`, `Rtty.cpp` | VCO, discriminators, PLL, sliding FFT, AGC, ATC, modem |
| `Fft.h`, `Fft.cpp` | Spectrum collection for display and AFC |
| `Sound.h`, `Sound.cpp` | Processing order, prefilters, AFC decision |
| `Wave.h`, `Wave.cpp` | WinMM and MMW audio buffering |
| `ComLib.h`, `ComLib.cpp` | Sampling globals |

`Rtty.cpp` carries its own `CFIR`, `CFIR2`, and `CFIRX` classes alongside the
free functions in `fir.cpp`. `CFIR2` is the same double-buffered convolution
MMSSTV uses; `CFIRX` is its complex-sample counterpart. The filter *design*
functions, `MakeFilter` and `MakeIIR`, live only in `fir.cpp` and are shared.

## Sampling Model

The device is opened at `SampBase`, one of four nominal rates, while filters
and bit timing use the calibrated `SampFreq`. This lets a user correct sound
card clock error without asking the hardware for an odd rate, exactly as MMSSTV
does. `InitSampType()` at `ComLib.cpp:125` picks the arrangement:

| `SampFreq` | `SampType` | `SampBase` | `DemSamp` | `DemOver` | `FFT_SIZE` |
| --- | --- | --- | --- | --- | --- |
| ≥ 11600 | 3 | 12000 | `SampFreq`/2 | 1 | 2048 |
| ≥ 10000 | 0 | 11025 | `SampFreq`/2 | 1 | 2048 |
| ≥ 7000 | 1 | 8000 | `SampFreq` | 0 | 1024 |
| ≥ 5000 | 2 | 6000 | `SampFreq` | 0 | 1024 |

`DemSamp` is the rate the demodulator core actually runs at. At the two higher
rates the demodulator halves it, so its Nyquist frequency is a quarter of
`SampFreq` — about 2756 Hz at 11025 Hz. That is why MMTTY refuses a space
frequency above 2700 Hz (`SPACEH` in `ComLib.h:56`) and why the practical upper
limit for the space tone is around 2600 Hz.

Audio is carried as `double` with 16-bit PCM magnitude throughout. Time is
counted in samples.

## Receive Chain

```text
PCM input block
  -> optional dual-peak BPF and band-elimination pair  (CAA6YQ)
  -> optional prefilter FIR BPF                        (TSound::HBPF)
  -> optional LMS line enhancer or adaptive notch      (CLMS)
  -> FFT collection for display and AFC
  -> per-sample: CFSKDEM::Do
       -> limiter with AGC, optionally oversampled
       -> 1/2 decimation                               (CDECM2)
       -> one of four discriminators
       -> rectifier
       -> integrator: moving average or IIR low-pass
       -> optional ATC
       -> comparator -> bit
  -> framing state machine -> character ring
```

`TSound::Execute` at `Sound.cpp:241` runs the prefilters over the whole block,
then calls `CFSKDEM::Do` once per sample. Note the ordering: the prefilters and
the FFT run at `SampFreq`, everything inside `CFSKDEM` past the limiter runs at
`DemSamp`.

### Limiter

`CFSKDEM::Do` at `Rtty.cpp:1200` begins with a hard limiter. `digital.txt`
records that this replaced a post-detector AGC used up to version 1.19, which
turned out not to help. Two modes exist:

- **Fixed gain**: multiply by `m_LimitGain` and clip at ±16384.
- **Limiter AGC** (`m_LimitAGC`, the default): track the peak-to-peak excursion
  between successive positive-going zero crossings and set the gain to
  `64 × 16384 / (max − min)`, capped at 4096, before clipping.

With `m_LimitOverSampling` the limiter runs through `COVERLIMIT`, which
interpolates by four with an 80-tap FIR, limits at the higher rate, and
decimates back (`fir.cpp:2043`). This does not change decoding materially; it
reduces the phase distortion the limiter introduces, which is visible on the XY
scope.

The limiter is disabled entirely while transmitting, unless full-duplex echo is
in use (`Main.cpp:1618`).

### Decimation

When `DemOver` is set, `CFSKDEM::Do` processes only every second sample. The
sample it processes is the output of `CDECM2`, a 36-tap half-band-style FIR
decimator that consumes the current and previous input sample (`fir.h:60`,
`fir.cpp:261`). The odd samples are simply stored for the next call.

This is the oversampling arrangement `digital.txt` describes: running the
discriminator at half rate halves the filter order needed for the same shape
and doubles the time available per sample.

### Discriminators

`m_type` selects one of four, and `Main.cpp:1665` gives them their UI names:

| `m_type` | Name | Mechanism |
| --- | --- | --- |
| 0 | IIR | Two `CIIRTANK` resonators at mark and space |
| 1 | FIR | Two band-pass FIRs at mark and space |
| 2 | PLL | A single phase-locked loop |
| 3 | FFT | A two-bin sliding DFT |

Note that `CFSKDEM`'s constructor never assigns `m_type` or `m_atc`. Both are
read from the INI file at startup with their own indeterminate value as the
default (`Main.cpp:1961`, `Main.cpp:2025`), so the effective default is
whatever the shipped `Mmtty.ini` says — FIR (`DEMTYPE=1`, with `Tap=512`), and
ATC off, in all three shipped INI files. The author's own profile
(`je3hht.pro`) selects the FFT discriminator, and only `test.pro` selects IIR.

**IIR** uses the two-pole resonator at `fir.cpp:46`:

```text
a1 = 2·exp(−πBT)·cos(2πFT)
a2 = −exp(−2πBT)
b  = sin(2πFT) / ((fs/6) / B)
```

with `F` the tone frequency, `B` the bandwidth (default 60 Hz, giving Q ≈ 36),
and `T = 1/fs`. Narrowing `B` raises Q; too far and the resonator oscillates.

**FIR** builds two Kaiser-windowed band-pass filters, 60 dB attenuation, whose
half-width depends on the tap count (`Rtty.cpp:792`): ±40 Hz below 192 taps,
±30 Hz below 256, ±20 Hz at or above 256. Default is 72 taps. Unlike the
resonator it is linear phase, which shows plainly on the XY scope.

**PLL** (`Rtty.h:398`) is textbook: a multiplier phase detector, an IIR loop
filter (default second order, 250 Hz cutoff), a table-driven `CVCO`, and a
higher-order output filter (default fourth order, 200 Hz) to remove the sum
term the loop filter leaves. The loop cutoff must exceed the shift. Its output
is split into pseudo-mark and pseudo-space channels by sign after passing
through its own ATC:

```text
d = m_atcPLL.Do(d + 8192) − 8192
d ≥ 0 -> mark = d,  space = 0
d < 0 -> mark = 0,  space = −d
```

so that the remainder of the chain is common to all four types. The IIR
resonators still run in this mode, but only to drive the scope displays.

**FFT** (`CPHASE`, `Rtty.cpp:2252`) was added in 2010. It converts the real
signal to analytic form with a 20-tap Hilbert pair, mixes it down so the mark
lands at DC, and runs a sliding DFT of `m_TONES` bins over a window of

```text
fftShift  = shift × TONES / (TONES − 1)
symbolLen = TONES × fs / fftShift = (TONES − 1) × fs / shift
```

samples. With that window the bin spacing is `fftShift / TONES`, so bin 0 sits
on the mark and bin `TONES − 1` sits exactly on the space; those two magnitudes
become the mark and space channels. At the default four tones and 170 Hz shift
the window is 3/170 s ≈ 17.6 ms, a little under one bit at 45.45 baud. Tones
are selectable from 2 to 6, trading frequency resolution against response.

Two details of this path differ from the others. It takes the signal from
*before* the limiter, and it takes it without passing through `CDECM2` — the
sample is simply the every-other input sample while `CPHASE` is configured for
`DemSamp`. Its `CAGCX` stage exists but is commented out of the path
(`Rtty.cpp:2330`).

`CSlideFFT::Do` at `Rtty.cpp:2229` is a recursive sliding transform: each bin
subtracts the sample leaving the window, adds the sample entering it, and
rotates by its bin phasor. The rotation constant is scaled by 0.9999 rather
than being a pure rotation, which leaks old energy away and keeps rounding
error from accumulating without bound.

### Detector and Integrator

Detection is rectification — the code simply negates negative values, which
`digital.txt` notes is the digital equivalent of the diode. The two rectified
channels then go through an integrator, chosen by `m_lpf`:

- **Average** (`CSmooz`, the default): a moving average over
  `DemSamp / smoothFreq` samples. The "smoothing frequency" is the reciprocal
  of the averaging period, not a cutoff. Default 70 Hz.
- **IIR**: a Butterworth low-pass, default fifth order at 40 Hz. Because this
  one *is* a cutoff, it must be set lower than the equivalent average setting.

The author notes the IIR form is much cheaper and suggests roughly 40 Hz at
fifth order or 30 Hz at third.

### Automatic Threshold Control

`CATC` (`Rtty.cpp:1860`) was added in version 1.58 for signals with echo, where
the limiter alone does not put the comparator inputs at a workable level. It
tracks the minimum and maximum of its channel over 64-sample blocks, keeps
`m_Max + 1` such blocks (the "Time" setting, default 4), takes the extremes
over that history, and derives a threshold as the IIR-smoothed midpoint. The
sample is then expanded about that threshold by a factor of 1.1 and re-centred
on the constant `ATCC` (8192):

```text
th = iir((high + low) / 2)
d  = (d − th) × 1.1 + th + (ATCC − th)
```

Floors on the tracked extremes keep the expansion from running away on a quiet
channel. The default response is deliberately fast, which helps echo and hurts
clean signals; the author recommends turning ATC off when there is no echo.

### Comparator and Squelch

The bit is simply `mark ≥ space` (`Rtty.cpp:836`), inverted when Rev is set.
The magnitude of the difference feeds the signal-strength display and the
squelch: peaks are accumulated over 100 ms windows and smoothed over eight of
them, giving roughly a 0.8 s response (`Rtty.cpp:851`). Below the squelch
threshold the comparator output is forced to mark, which stops the framing
state machine from finding start bits in noise. The threshold is scaled by ten
while the limiter is active, because the limiter has already normalized the
level.

## Prefilters

Two optional stages sit ahead of the demodulator, both running at `SampFreq`
without oversampling, so they need a fair number of taps to be sharp
(`Sound.cpp:303`):

```text
Sound -> BPF -> Notch or LMS -> Limiter (demodulator)
```

The **BPF** is a plain FIR band-pass whose edges are the mark frequency minus
`FW` and the space frequency plus `FW` (`Sound.cpp:109`). The constructor
starts it at 56 taps and 100 Hz; the bundled profiles use 64 taps and 250 Hz.
It optionally follows AFC.

The **notch and LMS** share `CLMS` (`fir.cpp:76`), which is the same class
MMSSTV uses:

- With `m_Type` set it is a fixed FIR band-elimination filter, optionally two
  of them in series. The notch frequency can be placed by right-clicking the
  spectrum; placing it between mark and space makes it track the AFC centre.
- With `m_Type` clear it is a leaky LMS adaptive filter — a transversal filter
  whose coefficients are updated by `h ← h + 2μ·e·z` with leakage `γ` and an
  optional band-pass bias added each update. The author's own comment is that
  the default parameters make it counterproductive.

A third, separate prefilter is `CAA6YQ` (`Rtty.cpp:2561`), the "dual peak
filter" added in 2010: a 512-tap band-pass spanning mark to space in series
with a 256-tap band-elimination filter centred between them, which produces two
peaks on the two tones. It lives inside `CFSKDEM` rather than in `TSound` and
rebuilds its coefficients when AFC moves a tone by 5 Hz or more. When it is
enabled the signal-strength calculation is rescaled to compensate
(`Rtty.cpp:845`).

## Automatic Frequency Control

AFC works in the frequency domain, not from the demodulator, so it requires the
FFT display to be running. `TSound::DoAFC` at `Sound.cpp:479` is called from
the UI timer roughly every 300 ms and must not run on the audio thread; it
suspends that thread while it recalculates filter coefficients.

The algorithm searches outward from the current mark and space bins for local
peaks, stopping when a bin falls more than `AFC_PEAKDOWN` (128) below the best
found so far. The search span is the current shift scaled by the Sweep setting
on the inner side and 1.2 times that on the outer. It averages the bins it
visited to get a noise floor, and abandons the update if either peak fails to
exceed that floor by the SQ threshold, scaled by the display gain setting. A
detected separation below 140 Hz or above 1500 Hz is also rejected.

What it does with an accepted pair depends on `sys.m_FixShift`:

| Mode | Behavior |
| --- | --- |
| Free | Mark and space each move toward their detected peak |
| Fixed | The shift is held; only the centre frequency moves |
| HAM | The shift snaps to 170, 200, 220, or 240 Hz, and the pair moves |
| FSK | The shift snaps to the same set, but the centre never moves |

Movement is fractional — the error divided by the Time setting — so the
correction converges rather than jumping. Corrections smaller than 2 Hz are
ignored, results are rounded to whole hertz, and the pair is clamped to
`MARKL` (300 Hz) and `SPACEH` (2700 Hz).

Applying a correction does not always rebuild the filters. `AFCMarkFreq` only
recalculates coefficients when the tone has moved 5 Hz — 10 Hz for the FIR,
PLL, and FFT types — from the frequency the filters were last built for
(`Rtty.cpp:765`). The reported frequency tracks continuously; the filters step.

The FSK mode exists because in FSK operation the transmit frequency is set by
the radio, so an AFC that moved MMTTY's mark frequency would break zero-beat
with the other station.

## Transmit Chain

```text
Character queue
  -> CFSKMOD::Do, one PCM sample per call
       -> framing state machine -> mark/space/off decision
       -> optional TX LPF (moving average) for smooth transitions
       -> table-driven CVCO
       -> output gain
       -> optional TX BPF (48-tap FIR)
       -> amplitude ramp (CAMPCONT)
  -> PCM output FIFO
```

`CFSKMOD` at `Rtty.cpp:242` is deliberately simple. Its `CVCO` is set with the
space tone as the free-running frequency and a gain of `mark − space`, so the
framing state machine's output of 0 or 1 selects space or mark directly
(`Rtty.cpp:302`). Feeding the VCO from a continuous value instead of a binary
one is what the TX LPF does: a moving average over `SampFreq / m_LPFFreq`
samples, 100 Hz by default, which the author labels GMSK in the source and
which produces a smooth frequency transition rather than a step.

The TX BPF is a 48-tap Kaiser band-pass spanning mark − 150 Hz to space + 150 Hz
(`Rtty.cpp:297`), intended to reduce intermodulation products in the
transmitter. Neither TX filter has any effect in FSK mode, where the audio path
is unused.

`CAMPCONT` (`Rtty.cpp:2522`) ramps the output amplitude with a quarter-sine
shape whenever the keying state changes, so that PTT transitions do not produce
a click.

A third output state exists beyond mark and space: `m_out < 0` mutes the sample
entirely. That is what makes CW identification possible over an otherwise
FSK-only path — see [framing.md](framing.md).

## Spectrum Collection

`CFFT` (`Fft.cpp`) collects `FFT_SIZE` samples from the processed input block,
transforms them with a real-input split-radix routine, and produces an integer
magnitude array in `m_fft` that both the display and the AFC read. The window
shown is limited to `m_FFTWINDOW`, which corresponds to 3000 Hz at the 6000 Hz
sample type and 4000 Hz otherwise (`Sound.cpp:50`).

The XY scope takes the mark and space channels directly and interpolates them
by 2, 4, or 8 with `CINTPXY*` for display resolution only; this has no effect
on decoding.

## Where MMTTY and MMSSTV Diverge

Both programs share `fir.cpp`, `Fft.cpp`, `Wave.cpp`, and the double-buffered
`CFIR2`. Where they differ is instructive for a shared port:

- MMSSTV demodulates to a *frequency* value with a PLL, zero-crossing counter,
  or Hilbert discriminator, and the consumer is a raster. MMTTY demodulates to
  a *binary decision* through two tone channels, and the consumer is a byte
  stream.
- MMSSTV's oversampling is absent; MMTTY halves its rate for the demodulator.
- MMTTY's `CVCO` uses an integer table index and a table sized `2 × SampFreq`;
  MMSSTV's modulator is structured around a per-sample frequency queue instead.
- MMTTY's `CIIRTANK` resonator has no MMSSTV counterpart, and MMSSTV's
  `CSSTVSET` timing model has no MMTTY counterpart.
