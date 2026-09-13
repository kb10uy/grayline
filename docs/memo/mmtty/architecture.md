# MMTTY Architecture

This document describes the overall structure of the original MMTTY application
in `docs/reference/mmtty`. Signal processing and the character codec are
documented separately:

- [MMTTY DSP Implementation](dsp.md)
- [MMTTY Framing and Character Coding](framing.md)
- [RTTY on the Air](../rtty/protocol.md)
- [MMTTY Rust Porting Notes](porting.md)

## Overview

MMTTY is a Win32 desktop application by JE3HHT Makoto Mori, written for
Borland/Embarcadero C++Builder and the Visual Component Library. Version 1.70
in the submodule carries later maintenance by JA7UDE Nobuyuki Oba, AA6YQ Dave
Bernstein, and others. It shares its ancestry, its idioms, and much of its
support code with MMSSTV; `fir.cpp`, `Fft.cpp`, `Wave.cpp`, `ComLib.cpp`, the
log files, and the logger and radio plug-in interfaces are recognizably the
same lineage.

```text
mmtty.exe
  |-- TMmttyWd                UI, orchestration, keyboard, macros, logging
  |-- TSound                  Real-time audio worker
  |   |-- CWave               WinMM/MMW audio input and output
  |   |-- CFSKDEM             Demodulation and character framing
  |   |-- CFSKMOD             Character framing and FSK modulation
  |   |-- CLMS                Optional adaptive notch or line enhancer
  |   `-- CFFT                Spectrum collection
  |-- CRTTY                   Baudot/ASCII conversion
  |-- CComm                   PTT, and FSK keying through a serial TxD line
  |-- CCradio                 Radio CAT polling
  |-- CMMTnc / CTextFifo      TNC emulation and plug-in TNC interface
  |-- CLogFile / CLogLink     QSO logging and logger integration
  `-- VCL dialogs and viewers
```

Unlike MMSSTV, whose main form owns essential codec behavior, MMTTY keeps the
whole modem inside `Rtty.cpp`. `CFSKDEM` produces decoded *bytes*, not
intermediate signal values, and `CFSKMOD` accepts bytes and produces PCM. The
main form supplies characters and consumes characters. This makes MMTTY's
signal path markedly easier to lift out than MMSSTV's.

What the main form does own is everything around that modem: the transmit
character queue and its pacing, macro expansion, the CW identifier, the
frequency-domain AFC decision, and the alternative transmit path over a serial
port.

## Project Structure

The entry point is `WinMain` in `mmtty.cpp:71`. It refuses to start a second
instance unless `-Z` is given, initializes VCL, creates the global `TMmttyWd`
form, and runs the VCL message loop. The `USEUNIT` and `USEFORM` declarations
at `mmtty.cpp:24` form an effective module manifest.

| File | Purpose |
| --- | --- |
| `mmtty.cbproj` | C++Builder VCL project |
| `mmtty.bpr` | Legacy C++Builder 5 project |
| `mmtty.cpp` | Entry point and unit manifest |
| `Main.cpp`, `Main.h` | Main form: UI, orchestration, TX queue, AFC trigger |
| `Rtty.cpp`, `Rtty.h` | The complete modem: demodulator, modulator, codec |
| `Sound.cpp`, `Sound.h` | Audio worker thread, AFC, spectrum display |
| `fir.cpp`, `fir.h` | Filter design, convolution, resonators, LMS, resampling |
| `Fft.cpp`, `Fft.h` | Spectrum collection |
| `Wave.cpp`, `Wave.h` | WinMM and MMW audio buffering |
| `Comm.cpp`, `Comm.h` | Serial PTT, serial FSK keying, EXTFSK plug-ins |
| `ctnc.cpp`, `ctnc.h` | TNC emulation and the MMT plug-in interface |
| `cradio.cpp`, `radioset.cpp` | Radio CAT control |
| `LogFile.cpp`, `LogList.cpp`, `LogConv.cpp` | QSO log storage, UI, import/export |
| `Loglink.cpp`, `MMlink.cpp`, `Hamlog5.cpp` | External logger integration |
| `mmcg.cpp` | JCC/JCG locality database lookup |
| `ComLib.cpp`, `ComLib.h` | Sampling globals, settings, text widgets, helpers |
| `*.dfm` | VCL form resources |
| `*.pro` | Saved parameter profiles |

Note that `Rtty.h` and `Rtty.cpp` are far broader than their names suggest:
along with the RTTY modem they hold the VCO, sliding FFT, AGC, ATC, scope
capture, and a duplicate set of FIR classes that shadow the ones in `fir.cpp`.

## Main Form

`TMmttyWd`, declared at `Main.h:138` and implemented in the roughly 9,900-line
`Main.cpp`, is both the main window and the application controller. It manages:

- The receive text pane, the transmit type-ahead buffer, and the QSO panel.
- Transmit and receive state transitions, including PTT and the TX/RX tail.
- Macro definition, expansion, repeat timers, and the CW identifier.
- Frequency and shift entry, and the AFC result applied to the demodulator.
- Spectrum, waterfall, XY scope, and squelch displays.
- QSO logging, dupe checking, contest fields, and logger integration.
- Serial PTT, serial FSK, radio CAT, TNC emulation, and the remote interface.
- Settings persistence and the profile system.

The sound worker owns `CFSKDEM` and `CFSKMOD` by value; the form reaches them
through `pSound->FSKDEM` and `pSound->FSKMOD` rather than through aliases. The
codec objects are two `CRTTY` members of the form itself (`Main.h:671`): `rtty`
carries the live shift state for receive and transmit, and `rttysub` exists
only to test whether a typed key is representable at all (`Main.cpp:3665`).

## Global State

| Global | Purpose |
| --- | --- |
| `MmttyWd` | Main form instance |
| `sys` | The `SYSSET` settings aggregate (`ComLib.h:225`) |
| `COMM` | Serial port parameters for PTT and FSK |
| `RADIO` | Radio CAT configuration |
| `TNC` | TNC emulation configuration |
| `Log` / `LogLink` | QSO log and external logger link |
| `Remote` | Bit flags for the remote-control mode |
| `FSKCount`, `FSKCount1`, `FSKCount2`, `FSKDeff` | Sound/UART transmit sync counters |

`SYSSET` mixes audio device selection, DSP defaults, colors, fonts, macro text,
shortcut keys, log options, and help paths in one structure, in the same way
MMSSTV's does. Several lower-level modules include `ComLib.h` or `Main.h`,
producing circular dependencies around the main form.

The sampling globals live in `ComLib.cpp:48` and are selected by
`InitSampType()` at `ComLib.cpp:125`:

| Value | Meaning |
| --- | --- |
| `SampFreq` | Calibrated logical rate used by filters and bit timing |
| `SampBase` | Nominal rate the device is opened at |
| `DemSamp` | Rate the demodulator core runs at |
| `DemOver` | Whether the demodulator decimates by two |
| `SampSize` | Audio worker block size |
| `FFT_SIZE` | Spectrum transform size |
| `SampType` | Index of the selected nominal rate |

Four nominal rates are supported — 6000, 8000, 11025, and 12000 Hz — and a
calibrated `SampFreq` anywhere from 5000 to 12500 Hz selects among them
(`Option.cpp:1439`). At 11025 and 12000 the demodulator halves the rate; at
6000 and 8000 it does not.

## Runtime Data Flow

### Receive

```text
Audio device
  -> CWave input FIFO
  -> TSound block loop
  -> optional prefilter BPF and LMS/notch
  -> CFSKDEM::Do, one sample at a time
  -> limiter, discriminator, integrator, ATC, comparator
  -> framing state machine
  -> 512-byte character ring
  -> UI timer: TMmttyWd::RecvJob
  -> CRTTY::ConvAscii
  -> receive text pane, log file, TNC, remote client
```

`TSound::Execute` at `Sound.cpp:241` reads a block, runs the optional
prefilters over it, hands it to the FFT collector, then feeds each sample to
`CFSKDEM::Do`. Decoded bytes accumulate in a ring inside the demodulator and
are drained on the UI timer by `TMmttyWd::RecvJob` at `Main.cpp:2607`, which
converts them to text and distributes them.

### Transmit

```text
Type-ahead buffer or macro text
  -> TMmttyWd::ConvString    macro expansion
  -> CRTTY::ConvRTTY         ASCII to Baudot, with shift insertion
  -> CFSKMOD::PutData        2048-byte queue
  -> CFSKMOD::Do             framing, VCO, optional BPF
  -> CWave output FIFO
  -> audio device
```

with an optional parallel path:

```text
  -> CComm::PutChar          256-byte queue
  -> bit-reversal
  -> UART TxD line as FSK keying
```

The UI timer at `Main.cpp:2790` tops the modulator queue up only while it holds
two characters or fewer, so that a stop request takes effect quickly and so
that macro and keyboard input interleave sensibly. Everything else waits in the
type-ahead buffer, where it is still editable.
`CFSKMOD::Do` at `Rtty.cpp:364` produces
exactly one PCM sample per call and pulls the next character when its bit
counter expires.

## Threading and Buffering

| Context | Responsibilities |
| --- | --- |
| VCL main thread | UI, text panes, macros, TX queue refill, AFC decision, logging |
| `TSound` worker | Audio I/O, prefilters, demodulation, modulation, FFT collection |
| `CComm` worker | Serial FSK character output and its own diddle generation |
| `CCradio` worker | Radio CAT polling |
| WinMM callbacks | Audio FIFO bookkeeping and event signaling |

Only the WinMM FIFOs use events and critical sections. The higher-level queues
— the demodulator's character ring, the modulator's character queue, the serial
transmit queue, and the FFT result array — are shared between threads through
plain counters and indices with no atomics or barriers, relying on
single-producer/single-consumer access and x86 memory behavior.

Two pieces of cross-thread coordination deserve note. `TSound::DoAFC` is called
from the UI timer and suspends the audio thread around the filter
recalculation, which is why its comment says it must not be called from within
the thread (`Sound.cpp:477`). And when both the sound card and the serial port
are transmitting, `FSKCount`, `FSKCount1`, `FSKCount2`, and `FSKDeff` are
plain globals used to keep the two transmitters from drifting more than three
characters apart (`Comm.cpp:29`).

## Transmit Paths

MMTTY can key the transmitter three ways, selected by `sys.m_TxPort`
(`ComLib.h:73`):

| Setting | Behavior |
| --- | --- |
| `txSound` | AFSK only. Audio carries the signal; TxD is unused |
| `txTXD` | Both. The sound card sets the pace and TxD mirrors it |
| `txTXDOnly` | FSK only. The UART sets the pace and audio output stops |

In the TxD modes the serial port is opened with the RTTY parameters themselves
— 45 baud, five data bits, and one and a half or two stop bits — so that the
UART's own framing *is* the RTTY framing (`Main.cpp:1496`). This is why the
port must physically support five-bit characters, and why MMTTY falls back from
two stop bits to one and a half if the open fails, and warns that many
USB serial adapters cannot do the low baud rate at all.

When a real port cannot be used, `CComm::Open` falls back to loading a DLL
named after the selected port with an `.fsk` or `.dll` extension and driving it
through five exported functions — this is the EXTFSK interface
(`Comm.cpp:562`).

## Demodulator Configuration and Profiles

The demodulator carries far more adjustable parameters than a user can be
expected to set from scratch, so MMTTY stores complete named parameter sets as
**profiles**. A profile is an INI section holding every demodulator,
modulator, decoder, and encoder setting; the bundled `*.pro` files are
examples, and the `[Define1024]` section name encodes the sampling
configuration the profile was captured at.

Settings otherwise live in `mmtty.ini` next to the executable, read and written
by `ReadRegister` and `WriteRegister` at `Main.cpp:1842` and `Main.cpp:2231`.

## Remote Control and TNC Emulation

MMTTY is widely used as an RTTY engine inside other programs, and it offers
three separate mechanisms for that.

**Remote mode** starts MMTTY with a command-line option that reduces it to a
control panel or hides it entirely, and drives it through a registered window
message plus a shared-memory block. The message enumeration is at `Main.h:47`:
`RXM_*` values travel into MMTTY and `TXM_*` values out of it. The shared block
`COMARRAY` at `Main.h:115` carries the spectrum array, the XY scope arrays,
version and sample-rate information, error flags, and the profile names. The
documented startup options are `-r`, `-s`, `-t`, `-u`, `-f`, `-d`, `-m`,
`-h<hwnd>`, `-n`, `-p`, `-Z`, `-a`, `-C<name>`, and `-T<seconds>`.

**TNC emulation** makes MMTTY answer a subset of the AEA PK-232 or Kantronics
KAM command set over a second serial port, so that software written for a
hardware TNC can drive it unmodified (`ctnc.h:33`).

**MMT plug-ins** are DLLs loaded through the interface at `ctnc.h:129`, which
receive characters, PTT changes, spectrum data, and XY scope data, and can
inject characters. The related `MML` logger and `MMR` radio plug-in interfaces
are shared with MMSSTV.

## Logging and Platform Integration

| Component | Responsibility |
| --- | --- |
| `CLogFile` | Native QSO log, indexing, dupe check, search, import, export |
| `CLogLink` | Hamlog and external logger communication |
| `CMMLink` | Dynamically loaded logger plug-in API |
| `CMMRadio` | Dynamically loaded radio plug-in API |
| `CComm` | COM port RTS/DTR PTT and TxD FSK keying |
| `CCradio` | Built-in CAT polling for Yaesu, ICOM, Kenwood, TenTec, and JRC families |
| `CMMCG` | JCC/JCG code lookup for Japanese QTH entry |

The log is the same `MMLOG DATA Ver1.00` format MMSSTV writes: a 256-byte
header followed by fixed-length `SDMMLOG` records (`LogFile.h:26`).

## Persistent Data

| Data | Format |
| --- | --- |
| Settings | `mmtty.ini`, plain INI |
| Profiles | INI sections, or standalone `*.pro` files |
| QSO log | `*.mdt`, the MMLOG record format |
| Received text | Plain text, optionally with timestamps |
| Recorded audio | `*.mmv` |

`CWaveFile` reads and writes MMV audio, which is not a RIFF/WAV container: a
four-byte header of `0x55 0xAA`, the `SampType` index, and a zero, followed by
signed 16-bit PCM. A recording made at one sample type is resampled on playback
if the current configuration differs.

## External Dependencies

- Borland/Embarcadero VCL and RTL.
- Win32, WinMM, and serial APIs.
- Registered window messages, file mapping, and `WM_COPYDATA`.
- Optional MMW audio, EXTFSK keying, MMT TNC, MML logger, and MMR radio DLLs.

The original source is therefore both a protocol reference and a record of
application behavior, but unlike MMSSTV its modem is genuinely separable from
its user interface.
