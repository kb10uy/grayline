# Grayline SSTV Architecture

This document defines the intended architecture of the Rust implementation and
maps the current codebase onto that design. It is both a guide for new work and
a record of which architectural pieces already exist.

The architecture of the original MMSSTV application is described in
[mmsstv/architecture.md](../mmsstv/architecture.md). See [mmsstv/porting.md](../mmsstv/porting.md) for the
mapping from the original implementation to the proposed Rust boundaries,
[mmsstv/dsp.md](../mmsstv/dsp.md) for original DSP details, and
[sstv/modes.md](../sstv/modes.md) for mode and timing data. The portable
transmit overlay format is described in [template-design.md](template-design.md).
The desktop application layer, its audio boundary, and its user interface are
described in [gui-design.md](gui-design.md).

## Design Goals

Grayline SSTV aims to preserve the signal-processing and protocol behavior of MMSSTV
without preserving its Win32/VCL structure. The design should have these
properties:

- The reusable SSTV core is independent of UI, audio devices, radio control,
  logging, persistence, and operating-system APIs.
- Receive and transmit processing can run deterministically on in-memory data
  for tests and offline tools.
- Streaming boundaries have explicit ownership, timing, and error contracts.
- Stateful processors own their configuration and mutable state; there are no
  equivalents of MMSSTV's global `sys`, `SSTVSET`, or main-form state.
- Mode definitions, raster timing, and color conversion are shared by transmit
  and receive paths.
- Platform and application layers depend on the core, never the reverse.
- Real-time integrations use bounded queues or equivalent backpressure instead
  of implicitly shared buffers and counters.

The smallest portable core should remain usable without an application or live
audio backend. Where practical, core crates should support `no_std` with
allocation rather than requiring the standard library.

## Target Layers

The target architecture separates numerical processing, SSTV protocol logic,
platform integration, and application behavior.

| Layer | Responsibility | Current location |
| --- | --- | --- |
| DSP | Filters, FFT, Hilbert transforms, oscillators, PLL, tone detection, and frequency measurement | `grayline-dsp` |
| Protocol model | Modes, identifiers, image geometry, raster descriptions, timing, frequencies, images, and color conversion | `grayline-sstv` |
| Receive front end | PCM preprocessing, frequency demodulation, sync confidence, VIS/FSK detection, and AFC | `grayline-sstv-rx`, `grayline-sstv-fskid` |
| Receive decoder | Raster acquisition, clock estimation, synchronization, slant correction, and pixel reconstruction | `grayline-sstv::rx` |
| Transmit encoder | Image-to-raster conversion, headers, VIS, scan lines, and identifiers | `grayline-sstv::tx` |
| Modulator | Timed frequencies to PCM samples | `grayline-tone-tx` |
| Audio adapters | Platform-specific input and output streams | `grayline-audio`; capture and playback implemented |
| Rig transport | How a rig is reached: a `rigctld` socket | `grayline-rig`; implemented |
| Rig policy | What the rig is told, and when | `rigcontrol.lua`, hosted by `grayline-sstv` |
| Integration | Composition of core stages for a particular environment | `gl-sstv`, `gl-wefax`, and `web-demo` |
| Template composition | KDL scene parsing, variables, RGBA overlay rendering, and RGB composition | `grayline-sstv-template` |
| Application | UI, configuration, history, template editing, logging, PTT, CAT, and orchestration | `grayline-sstv` receive interface; designed in [gui-design.md](gui-design.md) |

These are responsibility boundaries, not a requirement that every row become a
separate crate. Closely related protocol types currently live together in
`grayline-sstv`; they should be split only when a concrete dependency or reuse
need justifies it.

## Target Data Flow

Receive and transmit are explicit pipelines with shared protocol and image
types.

```text
Receive:
AudioSource
  -> Preprocessor / Demodulator
  -> demodulated frequency and synchronization stream
  -> RxDecoder
  -> ImageSink

Transmit:
ImageSource
  -> TxEncoder
  -> timed frequency stream
  -> Modulator
  -> AudioSink
```

The processing stages should not know whether their source or sink is a device,
a file, a test vector, or another in-memory component. Platform adapters select
buffer sizes and scheduling without changing protocol behavior.

### Current Receive Flow

The complete receive integration currently available is the offline
`gl-sstv decode` path:

```text
WAV file
  -> packetized first-channel normalized PCM
  -> grayline-sstv-rx::Demodulator
  -> incremental demodulated blocks
  -> grayline-sstv::RxDecoder
  -> staged global slant refinement
  -> BMP/JPEG/PNG image
```

A live receive path also exists in `grayline-sstv`, where the same stages run on a
worker thread fed by `grayline-audio` instead of a WAV reader.

`gl-sstv decode` reads and processes PCM packets without retaining the complete WAV
or a separate complete demodulated array. Its packet size defaults to 1024 mono
samples and is configurable with `--packet-size`. Demodulation and raster
decoding run sequentially in one thread, while bounded staging may retain
demodulated samples for the optional whole-image refinement pass. There is no
live audio source yet.

### Current Transmit Flow

The complete offline transmit integration is the `gl-sstv encode` path:

```text
background BMP/JPEG/PNG
  -> cover resize and center crop to mode dimensions
KDL template + ${station.callsign} + background as rximage
  -> RGBA overlay and RGB composition
  -> grayline-sstv::TransmissionEncoder
  -> VOX + VIS + raster + footer + FSKID + trailing silence
  -> grayline-tone-tx::Modulator
  -> packetized normalized PCM
  -> 48 kHz mono 16-bit WAV
```

`grayline-tone-tx` fills caller-owned output blocks and never retains the
complete PCM stream. It preserves oscillator phase across tone changes, treats
zero frequency as exact silence, and converts absolute picosecond deadlines to
sample endpoints without accumulating per-tone rounding error.

## Core Contracts

### Images and Modes

`RgbImage` is the owned, row-major image exchanged with the codec. `Mode` and
`ModeSpec` provide protocol identifiers, transport geometry, active rows,
raster periods, signal bands, and support status. Raster descriptions and color
conversion are shared by transmit and receive processing so that family-specific
ordering is defined once.

The mode inventory contains metadata for all 43 MMSSTV modes. Raster encoding
and decoding are currently implemented for:

- Robot 36 and Robot 72.
- Scottie 1, Scottie 2, and Scottie DX.
- Martin 1 and Martin 2.
- PD50, PD90, PD120, PD160, PD180, PD240, and PD290.

The other 29 modes remain metadata-only. This includes AVT, SC2, Pasokon, MR,
MP, ML, Robot 24, monochrome Robot modes, and narrow modes. Unsupported
behavior is represented explicitly rather than silently approximated.

### Demodulated Receive Data

The boundary between demodulation and raster decoding consists of physical
sample positions plus parallel frequency and normalized synchronization-strength
samples. `DemodulatedBlock` enforces continuity and value validation at this
boundary.

Frequency and synchronization strength are causal detector outputs and remain at
the sample positions where they were produced. The synchronization envelope is
used to find a pulse, never to time one. Its lag behind the frequency stream
runs to several milliseconds, and because the envelope is a ratio against
competing tone detectors, its weighted center also moves with the picture tones
either side of the pulse. Both errors would displace the whole picture
horizontally.

Every measured sync center is therefore refined on the frequency stream, where
the pulse is the window of one sync duration whose mean frequency is lowest.
Sliding a window of the known length beats reading edges off a threshold on two
counts: the discriminator ripples at twice the tone it tracks, which breaks a
threshold into fragments, and a threshold crossing sits at a different point on
each flank whenever the tones either side of the pulse differ. Displacing the
window either way trades sync samples for higher ones, so its minimum is on the
pulse whatever surrounds it. The frequency stream's own group delay needs no
compensation at all: pixel windows are read from that same stream, so a raster
placed on a frequency-domain center samples every pixel where its content
actually is. Acquisition, live phase correction, and staged slant refinement all
work in that one time base.

`RxConfig` still carries the envelope's approximate lag, but only to place the
search window around a detected pulse, so a rough figure is enough. Inputs whose
two streams are already aligned use a zero delay. A pulse that the search window
cannot see whole — one cut short by the start or the end of the retained samples
— has no usable center and is left out of the fit rather than pulling it.

`RxDecoder` is stateful and streaming. It exposes acquisition, decoding,
completion, and stopped states, consumes an explicit prefix of each input block,
and reports typed events and errors. Its responsibilities include:

- Mode detection from the spacing of synchronization pulses, so a transmission
  joined after its header, or that never sent one, can still be received. This
  is MMSSTV's `CSYNCINT`: a short history of measured intervals is matched
  against every candidate's line period at one-, two-, and three-line multiples,
  the multiples being what tolerates pulses lost to noise. A period can be
  matched by more than one mode — Robot 36 at two lines is exactly Robot 72 at
  one — so ties resolve towards the smallest multiple, the reading that assumes
  no pulse was missed. Nothing in such a signal says which row the reception
  started on, so the picture decodes correctly but vertically rolled, as it does
  in MMSSTV.
- Starting a header-identified reception at once. A VIS header ends where the
  raster begins, so `RasterStart::AfterHeader` seeds the clock at the first
  input sample — plus the leading segments a mode sends before its first raster
  unit — and the first row decodes about one line period later, as in MMSSTV.
  Detection is timed on the synchronization envelope, so the seeded phase is
  late by that envelope's lag; the samples the lag covers were dropped with the
  header, so the raster starts there and live phase correction takes the error
  out.
- Initial raster phase acquisition from four recurring synchronization pulses
  for a reception that has no header to start from, buffering at most five
  periods when the first pulse is incomplete. The phase is averaged over those
  pulses, leaving out the first one because the buffer can begin part way
  through it.
- Skipping the leading raster units whose picture arrived before mode detection
  finished. Those rows stay blank and count as delivered, which is what the
  operator sees in MMSSTV too, instead of failing a reception over samples that
  were never received.
- Family-specific RGB and luminance/chroma reconstruction.
- Stable live raster-phase correction. A correction may move the raster
  backwards, so the working window keeps one raster period behind the unit being
  decoded rather than trimming to its start.
- Optional automatic stop based on synchronization history, and `RxDecoder::stop`
  for a caller that can see a reception is over for a reason the decoder cannot
  observe. A signal that stops arriving altogether is one: no further input
  advances the raster, so no line is ever scored badly. Both reach the same
  terminal state and keep the rows decoded so far.
- Optional bounded staging and deterministic whole-image reconstruction.

Both starts fix the raster phase only. A reception begins on the configured
physical sample rate, as MMSSTV starts on its calibrated `SSTVSET.m_SampFreq`,
because neither a header nor the startup window says anything about the rate
that beats the calibrated figure.

Raster rate correction has two stages, as in MMSSTV. `RxConfig::live_slant`
refits the rate during decoding and redraws the rows already decoded from
retained samples, and `refine_staged` performs one more precise global fit after
completion. Both estimate a single global sample rate and raster epoch; local
raster warping is outside the implemented contract.

Refinement does not reuse synchronization observations collected through the
provisional live clock. It first acquires a stable clock from the first 32
staged periods, re-observes synchronization centers across the immutable staged
stream, and fits the final global clock from those observations. This keeps
early progressive display from biasing the completed-image slant correction.

### Timed Transmit Data

`TxEncoder` owns an image and yields `TimedTone` values. Each value carries a
frequency, a protocol component, and an exact deadline relative to transmission
start. Deadlines, rather than rounded sample counts, preserve protocol timing
until a modulator chooses a physical sample rate.

`TxEncoder` emits conventional VIS framing and mode raster data.
`TransmissionEncoder` wraps it with MMSSTV's built-in conventional VOX framing,
a 300 ms footer, a validated callsign FSKID with an optional contest number
after it, and 500 ms of trailing silence.
`TransmissionEncoder::without_identifier` sends the same transmission with
neither the FSKID nor the footer, which exists only to introduce it.
PCM conversion remains a separate modulator responsibility.

### FSK Identification

`grayline-sstv-fskid` keeps six-bit FSKID framing separate from audio tone detection.
`FskDecoder` consumes samples classified as mark, space, or ambiguous and
returns validated `FskRecord` values, which are either an `FskId` or the
`FskNumber` of a contest record following it. The receive front end owns the
1900/2100 Hz detectors and supplies those classifications.

The implementation is divided by responsibility:

- `grayline-sstv-fskid` owns the sample-driven acquisition timing, six-bit assembly,
  callsign and contest-record framing, checksum validation, bounded identifier
  and number values, and the allocation-free physical transmit event iterator.
- `grayline-sstv-rx` reuses its existing AFC-adjusted 1900 and 2100 Hz
  resonators and converts their normalized envelopes to mark, space, or
  ambiguous samples.
- `gl-sstv decode` carries validated identifiers in `DecodeReport` and writes
  each one to stdout as `fskid: CALLSIGN`.
- `grayline-sstv::TransmissionEncoder` places encoded FSKID events after the
  conventional image footer, and `gl-sstv encode` supplies its normalized
  callsign.

The core accepts classified detector samples rather than audio amplitudes. It is
therefore independent of audio backends and detector scaling, while preserving
the protocol's timing.

Callsign and contest records are implemented in both directions, in both of the
number's forms: text, and a twelve-bit count printed to at least three digits.
The encoder chooses between them the way MMSSTV chooses, so a number that is
three digits and fits twelve bits is counted and everything else is spelled.
N-VIS events remain future work. See [sstv/fskid.md](../sstv/fskid.md) for the
protocol definition and [mmsstv/fskid.md](../mmsstv/fskid.md) for the original
detector this one follows.

## State, Ownership, and Concurrency

DSP and codec objects own all mutable processing state. Input data is borrowed
for the duration of a processing call or moved into an owning stage such as
`TxEncoder`. Configuration is passed to the subsystem that uses it instead of
being read from global application settings.

Core APIs do not create threads or select an asynchronous runtime. This keeps
them deterministic and allows applications to choose an execution model. A
future live pipeline should place bounded queues between independently scheduled
audio and codec stages, preserve sample positions across those queues, and make
overflow or backpressure behavior explicit.

The receive staging option is bounded by a caller-provided sample limit. It is a
deliberate offline or deferred-refinement facility, not an unbounded hidden
queue.

Retained samples are stored quantized rather than as the floats every caller
works in: a frequency as a `u16` in sixteenths of a hertz, saturating at four
kilohertz, and a synchronization strength as a `u8`. Three bytes per sample
rather than eight is what decides whether the longest mode's reception fits on
a machine with little memory, and the resolution given up is far below what the
demodulator resolves — a sixteenth of a hertz is a fiftieth of one of the 256
levels a pixel can take. The conversion is confined to the retained buffer, so
acquisition, synchronization, and pixel reconstruction all read the same floats
they did before.

The decoder reserves the retained buffer once, at the smaller of the caller's
limit and twice the mode's own raster. Growing into a long reception instead
doubles its way there, which both leaves up to half the allocation unused and
copies everything retained so far on each doubling; bounding the reservation
against the raster keeps a caller that states a large limit for another reason,
such as an offline decoder given the length of a recording, from reserving all
of it for a picture that occupies a fraction.

## Platform Boundary

The following concerns belong outside the portable DSP and SSTV core:

- Audio device enumeration, callbacks, and stream formats.
- Windows messages, handles, VCL controls, and other GUI toolkit types.
- PTT and CAT transports.
- Logging services and external logger integrations. `grayline-qso` is the
  first of these: a directory of stations read by callsign, described in
  [qso-directory.md](qso-directory.md). It is a directory rather than a log,
  and it deliberately keeps nothing about a contact.
- History, template editing, settings persistence, and application file
  management.
- Thread scheduling and application-level queue policy.

Integration crates may adapt these facilities to core data types, but platform
types must not appear in reusable core APIs.

## Current Crate Structure

The workspace currently contains seventeen packages:

| Package | Architectural role | Current status |
| --- | --- | --- |
| `grayline-dsp` | Portable numerical layer | Implemented |
| `grayline-sstv` | Protocol model, images, transmit encoder, and receive decoder | 14 modes implemented |
| `grayline-sstv-fskid` | FSKID protocol encoding and decoding | Callsign and contest number, transmit and receive, implemented |
| `grayline-tone-tx` | Timed-tone PCM modulation | Streaming phase-continuous modulation implemented |
| `grayline-sstv-rx` | Receive front end | Incremental conventional-VIS demodulation implemented |
| `grayline-sstv-template` | Portable application-support layer | KDL parsing and SVG-backed RGBA rendering implemented |
| `grayline-sstv-cli` | Offline receive and transmit integration, as `gl-sstv` | Implemented |
| `web-demo` | Browser receive integration | Implemented |
| `grayline-audio` | Host audio adapters | Bounded capture and playback implemented |
| `grayline-rig` | Rig transports | `rigctld` client implemented |
| `grayline-qso` | Contact directory: shared store, Wavelog lookup, ADIF import | Implemented; described in [qso-directory.md](qso-directory.md) |
| `grayline-qso-cli` | Contact directory command line, as `gl-qso` | Implemented |
| `grayline-shell` | Platform integration, localization, and the log | Implemented |
| `grayline-wefax` | WEFAX protocol model, receive front end, and decoder | Receive implemented; described in [wefax.md](wefax.md) |
| `grayline-wefax-cli` | Offline WEFAX receive integration, as `gl-wefax` | Implemented |
| `grayline-wefax-app` | Application composition root | egui interface with live receive |
| `grayline-sstv-app` | Application composition root | egui interface with live receive and transmit |

Their current dependency direction is:

```text
grayline-sstv-fskid ----------------> grayline-sstv
grayline-dsp ------------------> grayline-tone-tx
grayline-audio ----------+
grayline-sstv-rx ----+
grayline-sstv-fskid ----------+
grayline-sstv -----------+-> grayline-tone-tx
grayline-sstv-fskid ---------+-> grayline-sstv-rx --+
grayline-sstv ----------+                       +-> grayline-sstv-cli
grayline-sstv-fskid ----------------------------------+
grayline-audio ----------+
grayline-sstv-rx ----+
grayline-sstv-fskid ----------+
grayline-sstv -----------+-> grayline-sstv-template
grayline-sstv-fskid ----------+
grayline-tone-tx ------+
grayline-sstv -----------+-> grayline-sstv-cli
grayline-sstv-template -------+
grayline-sstv-rx ----+
grayline-sstv-fskid ----------+
grayline-sstv -----------+-> web-demo

grayline-audio ----------+
grayline-sstv-rx ----+
grayline-sstv-fskid ----------+
grayline-tone-tx ------+
grayline-qso ------------+
grayline-rig ------------+-> grayline-sstv-app
grayline-sstv -----------+
grayline-sstv-template -------+

grayline-qso ------------> grayline-qso-cli

grayline-dsp ------------> grayline-wefax --> grayline-wefax-cli

grayline-audio ----------+
grayline-shell ----------+-> grayline-wefax-app
grayline-wefax ----------+
```

`grayline-wefax` depends on `grayline-dsp` and on nothing else in this
workspace. That it needs no part of `grayline-sstv` is the point of the split
between a mode's crates and the core: WEFAX shares the numerical layer, and
shares nothing of SSTV's protocol. Where it needed something the SSTV front end
had, that piece moved down into `grayline-dsp` rather than across.

`grayline-audio` is the platform audio boundary. It exposes normalized mono
`f32` samples with stream positions and keeps the host API out of its public
surface, so no core crate gains an audio dependency.
Playback similarly exposes a bounded mono `f32` writer while the callback
duplicates samples across the physical output channels. The application primes
the queue before starting the device and treats an active underrun as a broken
transmission.

`grayline-rig` is the platform rig boundary, and it is a socket rather than a
library: Hamlib runs as the operator's own `rigctld` process, which keeps a C
build out of every platform's toolchain and leaves the serial port available to
whatever else the station runs. It is also the only way the application reaches
a rig, because Hamlib already covers the CI-V and DTR/RTS keying that other
SSTV software arranges separately.

The crate is how a rig is reached and not what it is told: what is sent at each
moment is a Lua script the application hosts, because what a rig wants around a
transmission differs by rig and by station. The whole arrangement is described
in [rig-control.md](rig-control.md).

`grayline-dsp`, `grayline-sstv`, and `grayline-wefax` build as allocation-backed
`no_std` crates by default. `grayline-sstv-fskid` is also `no_std`. Audio file and image format dependencies
remain in `grayline-sstv-cli` and `grayline-wefax-cli`, outside the portable core.
`grayline-sstv-template` is a
standard-library application-support crate: it depends on `grayline-sstv` only at
the received-image and final RGB composition boundaries. It does not expose
SSTV modes to the template format or make the protocol crate depend on template
rendering.

## Current Implementation Detail

`grayline-dsp` provides radix-2 FFT, windowed real spectra, FIR and IIR design and
processing, Hilbert transforms, zero-crossing frequency measurement, a
phase-continuous VCO, PLL and Hilbert phase-difference frequency discrimination,
and resonator tone detection. The standalone FFT and PLL are not currently part
of the WAV receive path; that path uses the Hilbert phase-difference
discriminator.

`frequency::HilbertDiscriminator` and `detector::ToneDetector` were the SSTV
front end's own until a second mode needed them, which is the point at which
the reading that would generalize them exists. Both take the band they work in
from a design struct rather than from a constant, because that is the only
thing the two callers answer differently: the discriminator's reported range
and output cutoff follow the mode's signal band, and the tone detector's
envelope cutoff follows how quickly the tone it looks for comes and goes. What
stays internal is what both callers want the same — the sample-rate-keyed phase
lag, the Hilbert passband margins, and the filter responses.

`grayline-sstv-rx` provides a stateful `Demodulator` that accepts contiguous
normalized mono PCM packets and emits owned demodulated chunks with absolute
sample positions, a VIS mode event, and completed FSK identifiers. The mode is
identified once and kept, which is what an offline decode of one transmission
wants; `set_header_restart` opts into the live receiver's behavior instead,
where a header arriving mid-reception starts the reception over. Only a header
does so, because sync spacing is present throughout every picture. The existing
`demodulate` batch function is a convenience wrapper over that API and keeps the
offline behavior. The
front end performs band-pass filtering, level normalization, VIS/FSK tone
detection, conventional VIS decoding, zero-crossing AFC measurement, and
Hilbert frequency discrimination.

Conventional VIS decoding arms on the start bit, as MMSSTV does: fifteen
milliseconds of sync-tone dominance — longer than the break between the
leaders — opens the ten 30-millisecond bit cells, and the frame is accepted on
the parity-inclusive code table. The leaders are not required, which is what
reads a header through noise and catches a transmission joined mid-leader.

Because the trigger is that cheap, it is the cells that decide, and each is
checked as it completes rather than at the end of the ten. A framing cell has
to be sync-dominant and a bit cell has to name one of its two tones, both to
the same margin the trigger itself asked for, and both against the 1900 Hz
leader detector as well — which is what separates a header from a picture,
whose image tones sit far closer to 1900 Hz than to either bit frequency. This
follows MMSSTV, which abandons a header the moment a cell fails to look like a
bit. It matters most for the modes with 20-millisecond synchronization pulses:
those are longer than the trigger, so every line of one arms the decoder, and
only the cells stop the picture from eventually spelling a valid code and
restarting a live reception on itself. Abandoning at the failed cell rather
than at the tenth also keeps the decoder deaf for one cell instead of three
hundred milliseconds, so a real header arriving right after a false arm is
still read. `set_vis_detection` opts into
`VisDetection::Strict`, which admits a start bit only when leader tone
accumulated recently — an integrating gate rather than a per-sample chain, so
a noisy leader still counts — for an operator who would rather miss a header
than start on a false one. Its synchronization envelope is causal, with
its calibrated delay relative to the frequency output carried as metadata rather
than implemented by shifting the sample array. It requires at least a 6000 Hz
sample rate. The Hilbert transformer spans 100 Hz to 100 Hz below Nyquist, as
in MMSSTV; the preceding receive band-pass filter limits the SSTV audio band.

The live receive path does not resample or decimate PCM. Each captured mono
sample produces one demodulated frequency and synchronization value after VIS
detection. This matches MMSSTV's normal receive path; its rate-dependent Hilbert
phase span still emits one result per input sample, while its explicit
decimation is limited to displays and offline file conversion.

Raster conversion intentionally differs from MMSSTV's first-sample selection.
The Rust decoder averages the central five-eighths of the transmitted pixel
interval, leaving a narrow guard against adjacent-component contamination. This
is a deterministic anti-noise reconstruction policy rather than a downsampled
intermediate stream. Live phase correction adjusts the raster clock without
inserting or deleting demodulated samples.

`gl-sstv decode` composes the existing receive stages packet by packet. It uses the
first WAV channel, enables live raster synchronization and bounded in-memory
staging, performs global slant refinement, and saves BMP, JPEG, or PNG according
to the output extension.

The GUI receive worker enables the same bounded staging by default, with the
limit derived from the mode rather than fixed: one raster plus the trailing
audio a refinement is fitted against. A fixed five-minute window was both far
more than the short modes can use and less than PD290 needs, since that mode
runs for nearly five minutes on its own and its refinement could therefore run
the retention out before the tail arrived. Its Slant
control applies to the next reception and performs a whole-image global
rate/epoch refinement at completion. Disabling it during a reception suppresses
that refinement; enabling it after reception has started cannot reconstruct the
missing unstaged prefix and therefore takes effect on the next reception.

`gl-sstv encode` prepares the background at the selected mode's transport size,
renders the template with `${station.callsign}` and the background available as
`rximage`, and streams a complete framed transmission through
`grayline-tone-tx` into `hound::WavWriter`. It uses bounded 1024-sample PCM
blocks rather than generating the complete waveform in memory.

`grayline-sstv-template` strictly parses ordered KDL v2 layers, resolves frame-relative
geometry and caller-provided variables, image assets (PNG, JPEG, BMP, WebP),
received images, and fonts, then generates static SVG for `resvg`. It returns a
straight-alpha RGBA overlay and can source-over composite that overlay into
`grayline-sstv::image::RgbImage`. It does not select or prepare the background
image and does not access history or the filesystem implicitly.

## Application Storage

The desktop application uses the operating system's standard per-user
directories. Portable storage beside the executable is not supported.

| Content | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Configuration | `%APPDATA%\Grayline\sstv\config.toml` | `~/Library/Application Support/Grayline/sstv/config.toml` | `$XDG_CONFIG_HOME/grayline/sstv/config.toml` |
| Templates and assets | `%APPDATA%\Grayline\sstv\templates`, `%APPDATA%\Grayline\sstv\assets` | `~/Library/Application Support/Grayline/sstv/templates`, `~/Library/Application Support/Grayline/sstv/assets` | `$XDG_DATA_HOME/grayline/sstv/templates`, `$XDG_DATA_HOME/grayline/sstv/assets` |
| User images | `Pictures\Grayline SSTV` | `~/Pictures/Grayline SSTV` | `$XDG_PICTURES_DIR/Grayline SSTV` |

The image directory contains `Stocks`, `Sent`, and `Received`. Images are kept
directly in those directories without year or month subdivisions. Templates
are KDL files stored directly in `templates`; reusable template images and
other resources are stored under `assets`.

At startup the application creates all of these directories and creates an
empty, valid `config.toml` when it does not already exist. Existing
configuration files are never replaced. The application preserves comments and
unknown keys while saving its language, UI scale, device, library, mode, DSP,
history, and station-callsign settings. A `[variables]` table holds the
operator's own template variables as plain string keys, read by templates as
`${custom.<name>}`; a key that no `${...}` expression could hold is dropped on
load the way every other unusable value in the file is. Keys are assigned
rather than the table being rewritten, so a comment beside one survives a save
that did not touch it.

The GUI template list is populated from regular `.kdl` files directly inside
`templates`. The stock list is populated from regular files directly inside
`Stocks` when the configured `image` crate decoders can read their dimensions.
Both lists can be refreshed independently and provide an action that opens the
corresponding directory in Explorer, Finder, or the desktop file manager. The
File menu opens each of the directories above the same way, the configuration
by the directory holding its file: they hold the operator's own files in the
operator's own directories, so browsing them belongs to the file manager rather
than to a session the application would have to keep.

The composition worker caches the parsed selected template, decoded stock
image, mode-sized background, and encoded template assets. Template selection
or template-list refresh invalidates the template and asset caches; stock
selection or stock-list refresh invalidates the decoded background. Other
composition changes, including variables, timestamps, radio readings, received
images, and mode changes, perform no template or image file I/O. A mode change
resizes the already decoded stock image. Advancing the template generation
drops all encoded assets from the previous template before rendering the new
one, including when the new template has no image layers.

## Planned Gaps

The architecture is not complete until the following boundaries have production
implementations:

- Transmit and receive raster processing for the remaining modes.
- Audio detection of extended VIS and N-VIS.
- Contest FSK records, narrow N-VIS transmission, and optional CW identification.
- Template editing.
- Real-world received-audio regression fixtures.

These should extend the dependency structure above rather than placing platform
or application behavior into the core crates.

## Verification Strategy

Core behavior is tested with deterministic in-memory signals and images. The
test suite covers numerical primitives, mode metadata and timing, all currently
supported raster families, transmit/receive round trips, synchronization and
slant behavior, malformed streaming input, FSKID at multiple sample rates, and
a synthesized WAV-to-PNG integration path. The transmit integration test
encodes a complete Robot 36 WAV and decodes its image and FSKID. Template tests
cover strict KDL validation, all initial layer kinds, caller-resolved PNG and
receive images, straight-alpha rendering, and RGB source-over composition.

Run the complete verification set from the workspace root:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build --workspace
```
