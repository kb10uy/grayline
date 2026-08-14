# MMTTY Framing and Character Coding

This document describes how MMTTY turns the comparator's bit decisions into
characters and back. The signal processing that produces those decisions is in
[dsp.md](dsp.md); the protocol these implement is in
[rtty/protocol.md](../rtty/protocol.md); the application around them is in
[architecture.md](architecture.md).

Three classes are involved, all in `Rtty.cpp`:

| Class | Role |
| --- | --- |
| `CFSKDEM` | Receive framing state machine, `Rtty.cpp:828` |
| `CFSKMOD` | Transmit framing and keying, `Rtty.cpp:364` |
| `CRTTY` | Baudot/ASCII conversion and shift tracking, `Rtty.cpp:1475` |

## Framing Parameters

The demodulator and modulator each carry their own copy of the framing
parameters, and the main form keeps them in step.

| Parameter | Values |
| --- | --- |
| `m_BitLen` | 5 (Baudot), 6, 7, or 8 (ASCII) |
| `m_StopLen` | 0 = 1 bit, 1 = 1.5, 2 = 2, 3 = 1, 4 = 1.42 |
| `m_Parity` | 0 none, 1 even, 2 odd, 3 mark, 4 space |
| `m_BaudRate` | Bit rate; `m_ReCount = round(DemSamp / baud)` samples per bit |

The two odd `m_StopLen` values are receive-side only. `CFSKMOD` treats 3 and 4
as 1.5 bits (`Rtty.cpp:256`), so selecting a 1-bit or 1.42-bit receive stop
still transmits the mechanical teleprinter's 1.5. The HAM button forces the
whole set to amateur defaults: 45.45 baud, five bits, no parity, and the
configured 1 or 1.42-bit receive stop (`Main.cpp:6137`).

Timing is a single down-counter, `m_Count`, reloaded with `m_ReCount` at each
state transition and decremented once per demodulator sample. There is no
fractional accumulator, so at rates where `DemSamp / baud` is not an integer the
rounding error accumulates within a character and is discarded at the next
start bit. At 45.45 baud and `DemSamp` = 5512.5, one bit is 121.28 samples and
the counter uses 121, so the sample point has crept about two samples — under
2% of a bit — by the stop bit. Resynchronizing on every start bit is what keeps
this workable, and it is why the tolerance shrinks at higher baud rates where
the same absolute rounding error is a larger fraction of a bit.

## Receive Framing

`CFSKDEM::DoFSK` runs once per demodulator sample. `m_mode` holds the state;
values below 256 are the ordinary decoder and values from 256 up are the
majority-vote decoder.

### Ordinary Decoder

| State | Meaning |
| --- | --- |
| 0 | Waiting for a start bit; a space sets `m_Count` to half a bit |
| 1 | Half-bit confirmation; a mark here rejects the start bit as noise |
| 2 | Sampling data bits at one-bit intervals, MSB-first into `m_Data` |
| 3 | Parity bit, if enabled |
| 4 | Stop bit check; a mark stores the character, a space is a framing error |
| 5, 6 | Waiting out the remainder of the stop element |
| 7 | Parity error recovery |
| 8 | Framing error recovery; wait for the line to return to mark |

The half-bit confirmation in state 1 is the classic start-bit validation: the
decoder finds the falling edge, waits half a bit, and only accepts the
character if the line is still spacing at what should be the centre of the
start bit. Every subsequent bit is then sampled a full bit later, which places
the sample points at the centres of the data bits.

After the stop bit is accepted, the decoder waits a fraction of a bit rather
than the full stop element before looking for the next start bit
(`Rtty.cpp:971`):

| `m_StopLen` | Wait after storing the character |
| --- | --- |
| 2 (2 bits) | 11/8 bit |
| 1 (1.5 bits) | 7/8 bit |
| 4 (1.42 bits) | 4/5 bit |
| 0 or 3 (1 bit) | 3/8 bit |

Returning early is deliberate: a transmitter running slightly fast would
otherwise have its next start bit arrive before the decoder was looking for it.

`m_ignoreFream` — the framing-error-ignore option — stores the character
regardless of the stop bit state. It recovers characters whose stop element was
lost, at the cost of printing noise when there is no signal.

### Majority-Vote Decoder

With `m_majority` set, the decoder counts mark and space samples across each
whole bit period instead of sampling at its centre, and takes the majority
(`Rtty.cpp:1023`). Because it accumulates over a whole bit rather than sampling
at the middle of one, the state machine is offset by half a bit relative to the
ordinary decoder — its sync marks land on the bit transitions, not the bit
centres.

The stop-bit waits differ correspondingly, and for a 1-bit stop the machine
short-circuits by setting `m_mode = -1` before incrementing, landing back in
state 0 directly.

This is the default. It costs nothing but counters and is markedly more robust
against impulse noise than a single sample per bit.

### Character Buffer

Accepted characters go into a 512-byte ring inside the demodulator
(`DEMBUFMAX`), drained by the UI timer through `GetData`, which returns −1 when
empty. Transmitted characters are also written into this ring by the modulator
so that outgoing text appears in the receive pane; that is what
`CFSKMOD::pDem` is for (`Rtty.cpp:449`).

## Transmit Framing

`CFSKMOD::Do` produces one PCM sample per call and advances a mirror-image
state machine when its counter expires:

| State | Meaning |
| --- | --- |
| 0 | Fetch the next character and emit the start bit, or idle |
| 1 | Shift data bits out, MSB-first from the position selected by `m_BitLen` |
| 2 | Parity bit, then the stop element |
| 3 | Stop element completion |

The stop element is produced in two parts: state 2 holds mark for the *extra*
time beyond one bit — one sample for a 1-bit stop, `m_ReCount/2 − 1` for 1.5,
`m_ReCount − 1` for 2 — and state 3 holds it for one further full bit period.

State 0 is where all the transmit policy lives. In order, it:

1. Decrements the character-wait counter if one is pending.
2. Checks the sound/UART synchronization counters when TxD output is enabled,
   so that the audio does not run ahead of the serial port.
3. Takes the next character if the queue is non-empty, handling the control
   codes below.
4. Otherwise emits a diddle character, if diddle is enabled and `m_BitLen < 6`.
5. Otherwise holds mark for one sample and marks itself idle.

### Control Codes

Four byte values in the transmit queue are commands rather than characters
(`Rtty.cpp:410`). `CRTTY::ConvRTTY` produces them from four ASCII characters
that are consequently unavailable as text:

| Byte | Source character | Effect |
| --- | --- | --- |
| `0xFF` | `_` | Hold mark for three bit periods |
| `0xFE` | `~` | Hold the carrier off for three bit periods |
| `0xFD` | `[` | Disable diddle until re-enabled |
| `0xFC` | `]` | Re-enable diddle |

The mute is a third keying state: `m_out < 0` zeroes the output sample
entirely, rather than selecting a tone.

### CW Identification

Those first two codes exist to key the carrier in Morse. `TMmttyWd::StoreCWID`
at `Main.cpp:4077` expands each character of a `%{...}` macro into a run of `_`
and `~`, using a table where the low byte is the element count and the high
bits are the elements themselves, one meaning dot and zero meaning dash. A dot
emits one `_`, a dash three, and every element is followed by one `~`; two more
`~` separate characters, and unknown characters become a run of `~` alone.

At three bit periods per element and 45.45 baud, a dot is about 66 ms, which
puts the identifier near 18 words per minute. The macro is emitted with a
leading `[` so that diddle does not interrupt it.

### Diddle

When the transmit queue is empty and the character length is five or six bits,
`CFSKMOD` emits an idle character rather than a steady mark. `m_diddle`
selects which: 1 sends NUL ("BLK"), anything else sends LTRS. With
`m_RandomDiddle`, one time in four the other character is sent instead, which
makes the idle stream look less like a carrier to an automatic squelch.

Diddle is suppressed while `m_DisDiddle` is set. `XMIT` sets it for a quarter
second at the start of a transmission (`Main.cpp:3603`) and `ToRX` sets it
permanently when a transmission ends.

Two independent waits pace the output, both counted in thirds of a bit:
`m_CharWait` between characters and `m_DiddleWait` between diddles. The
`m_CharWaitDiddle` option inverts the arrangement so that diddle is emitted
*during* the wait rather than the wait replacing it, and `m_WaitTimer` stops
applying the wait after the fourth diddle following a real character.

## Character Conversion

`CRTTY` holds one table for each direction.

`_LTR` and `_FIG` at `Rtty.cpp:1462` map a received five-bit code to ASCII,
indexed with the first-received bit as the most significant. `ConvAscii`
returns 0 for LTRS, FIGS, and NUL, updating `m_fig` instead, which is how the
caller knows to update the FIG indicator rather than print something. With UOS
enabled, a space received in figures case also clears `m_fig`.

`_TTY` at `Rtty.cpp:1434` maps ASCII 0x20–0x7F to a code and a target case.
`ConvRTTY` returns the code in its low byte and, when the case differs from the
case last sent, the required LTRS or FIGS in the second byte. `m_dblsft`
duplicates that shift character. `m_outfig` tracks the transmitted case, with a
third value (2) meaning "unknown", which forces the next character to carry a
shift; TX UOS sets it after a space in figures case so that a UOS receiver
stays in step (`Rtty.cpp:1581`).

Lowercase ASCII maps to the same codes as uppercase. Characters with no
assignment map to code 0 and case 2 — that is, nothing at all — and
`TMmttyWd::PushKey` rejects them at the keyboard so that they never reach the
queue (`Main.cpp:3665`).

Three figures positions carry MMTTY-specific placeholders, `h` for the national
FIGS-H position, `s` for BELL, and the apostrophe for FIGS-J. `SetCodeSet` at
`Rtty.cpp:1486` swaps the last two for the J-BELL convention, both on transmit
(the apostrophe key produces code 0x14, FIGS-S) and on display.

`InvShift` at `Rtty.cpp:1624` converts a character to its counterpart in the
other case. It backs the "read this line in the other shift" feature, which
recovers text printed in the wrong case after a lost shift character.

## Serial FSK Framing

When the transmitter is keyed through a serial TxD line, the UART does the
framing. `CComm::Execute` at `Comm.cpp:157` runs its own copy of the transmit
policy — the same control codes, the same diddle generation, the same character
waits — but emits each character with `TransmitCommChar` instead of running a
state machine.

Two adaptations are needed. A UART transmits least significant bit first while
RTTY transmits b1 first, so `CComm::OutData` at `Comm.cpp:118` reverses the
five bits through a lookup table. And the port must be opened with the RTTY
framing itself: `TMmttyWd::SetFSKPara` at `Main.cpp:1496` sets the baud rate
from the demodulator, the byte size from `m_BitLen`, and the stop bits from
`m_StopLen`. Any stop selection other than one bit becomes `TWOSTOPBITS` at six
data bits or more and `ONE5STOPBITS` below that, except that a five-bit
character is then promoted back to two stop bits — and if the driver rejects
that combination, `OpenClosePTT` retries once with one and a half
(`Main.cpp:1579`).

Where both the sound card and the serial port are transmitting, four globals
keep them together: `FSKCount` counts characters queued but not yet sent on the
serial side, `FSKCount1` and `FSKCount2` count characters actually emitted by
each, and `FSKDeff` holds their difference. The audio side will not start a
character when the serial side is more than a few behind, and the serial side
inserts extra character waits when it is ahead by more than `DEFFSOUND`, which
is three (`Comm.cpp:29`).

## Observations

- The receive and transmit stop-length enumerations are not the same mapping,
  and the two objects that use them hold independent copies. Any port should
  make the receive tolerance and the transmit element separate concepts rather
  than one setting.
- Bit timing is integer samples with no fractional accumulator. This is
  adequate only because the frame is short and every start bit resynchronizes.
- The character ring carries both received and transmitted characters, so the
  "received text" stream is really an echo of the channel as MMTTY sees it.
- `m_Data` is a `BYTE`, so the transmit state machine's comparison of it
  against `0x00fe` is a comparison against 254 and is reachable
  (`Rtty.cpp:457`); it produces a fifty-bit carrier-off pause.
- Nothing in the framing layer is specific to five-bit Baudot. Setting
  `m_BitLen` to 7 or 8 gives an asynchronous ASCII teleprinter, and
  `TMmttyWd::RecvJob` bypasses `CRTTY` entirely in that case
  (`Main.cpp:2615`).
