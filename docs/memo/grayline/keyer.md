# Keyer

`gl-keyer` keys a rig's FSK input from a serial port's DTR or RTS line, or
hands the codes to a JL1HIS terminal unit that keys them itself
([The Terminal Unit](#the-terminal-unit)). It is a process of its own that
clients reach over the line protocol `rigctld` speaks, so an application talks
to it through the same transport client it already talks to the rig through.
The package is `grayline-keyer`, in `tools/keyer/`.

## Why a Process of Its Own

Rig control reaches the rig through `rigctld` and a Lua script
([rig-control.md](rig-control.md)), and that is right for everything that
happens once around a transmission. FSK keying happens once a bit: 22 ms at
45.45 baud, with a few percent of that as the whole timing budget. A socket
round trip and a script call per bit cannot be held to that, and Hamlib has no
FSK keying of its own to hand the bits to.

So the timing lives in the process that holds the port. A client sends a whole
stream ahead of time and the keyer lays every element boundary out against its
own clock. What crosses the socket is no longer timed, which is what lets it
stay a socket.

Speaking `rigctld`'s protocol is what keeps a second kind of transport out of
the applications: `grayline_rig::Rigctld` connects to the keyer unchanged.
What that framing leaves open is how the keyer's own commands are spelled,
and that lives in `grayline-keyer-protocol` (`crates/keyer-protocol/`), which
both ends depend on: the code alphabet, the `\rtty_open` arguments, the state
names `\status` reports, and `KeyerClient`, a client over `Rigctld` that
speaks them. The keyer keeps only what is its own: parsing commands and
rendering replies.

## Why Not PTT

PTT is held for a whole transmission and switched once at each end of it, and
the applications already do that through rig control
([rig-control.md](rig-control.md)), queue and lead-in and failure handling
included. A rig keyed in its FSK mode transmits on `T 1` from `rigctld` like
in any other, so the keyer has nothing to add there, and a second place that
could key the transmitter would be a second set of rules for when it is
unkeyed.

The terminal unit is the exception: it sends FSK only while its own PTT output
is keyed, so the keyer keys that output for each stream
([The Terminal Unit](#the-terminal-unit)). There is still one place that keys
the transmitter, not two. `\status` says which in its `PTT` value, `1` for a
keyer that keys PTT itself around each stream and `0` for one that leaves it
to the client, and a client that is told `1` does not key the rig. `PTT` is a
property of the keying method and does not change while the keyer runs; a
keyer that does not report it is taken to leave PTT to the client.

The one station this leaves out keys FSK on DTR and PTT on RTS of the same
port, with no CAT to key through: `rigctld` cannot open a port the keyer
holds. A PTT `ControlLine` beside the FSK one would cover it, and is left
until a station needs it. Until then the keyer releases the line it does not
drive when it opens the port, so a driver that raises it on open cannot key
the rig.

## Three Axes

What a keyer does decomposes three ways, and each is a seam a later method
plugs into without touching the others.

| Axis | Implemented | Later | Seam |
| --- | --- | --- | --- |
| What is sent | RTTY codes and their framing | CW text and speed | Protocol commands |
| How it is timed | Line switching, timed by the keyer; the terminal unit, timed by the unit | A UART's TxD, timed by the UART | `method::Method` |
| Where it goes | A serial port's DTR or RTS; the terminal unit's own output | GPIO, a parallel port | `line::ControlLine` |

The keyer is handed RTTY *codes*, not text and not elements. Text is the
application's: which code set, double shift, and unshift-on-space are encoding
decisions, and the application has already made them for the audio path. And
codes are what every timing method can start from — a UART takes the five-bit
code itself, while line switching expands it into elements.

That expansion is `grayline_rtty::tx::Framer`, shared with the audio
transmitter. A framer counts in whatever ticks its caller gives it — samples
for `Transmitter`, microseconds here — so a character cannot be framed one way
on the air and another way in the keyer. It frames each code with its gap and
diddle and leaves the lead-in and tail to the caller, because a keyer fed codes
as they arrive can run dry and has to idle at mark until more come, which only
it can tell apart from the end.

A new output line is a `ControlLine`. A new timing method is a `Method`, which
takes the stream a code at a time and blocks until each has gone out. A UART
method would write the code and wait for it to drain, with no framer at all.

A method whose hardware times the line itself, as `unit::Jl1hisTerminalUnit`
does for the terminal unit, blocks no longer than it takes to hand codes over
and to ask how far they have got. Two things in the trait are there for it.
`ready` says whether the method can take another code, and a stream idles
while it cannot, so codes the hardware has no room for stay in the session's
bounded queue. `finish` may answer `Flow::Pending`, and is then called again.
Either way `key_stream` comes round between polls, which is where a stream's
progress is recorded and its time limit enforced: a method that waited out the
hardware inside one call would report nothing until the end and could not be
timed out.

## Shape

```text
line.rs      ControlLine, and a serial port's DTR or RTS behind it
clock.rs     Clock, and the host clock waited on as tightly as it allows
method.rs    Method, and LineBitbang: one deadline per element boundary
unit.rs      Jl1hisTerminalUnit: the unit's control port, and its timeline
stream.rs    key_stream: one stream from lead-in to tail, from a CodeSource
session.rs   what clients asked for, with no I/O in it
error.rs     KeyerError, from the port, the line, and the listener
player.rs    the keying thread: owns the line and runs streams
protocol.rs  parsing commands and rendering replies
server.rs    the listener and one thread per connection
main.rs      gl-keyer serve, send, and ports
```

The line is driven from the keying thread alone: a connection asks and the
keying thread does it. A second thread on the port would have to share it, and
on Windows a synchronous handle serializes everything done to it, so a blocked
read would hold a line change behind it.

## Timing

`LineBitbang` measures every boundary from the start of the stream rather than
from the boundary before it, so a transition that happens late does not move
the ones after it. Each wait sleeps until a millisecond before its deadline
and spins the rest; on Windows the sleep is a high-resolution waitable timer,
and the keying thread runs at time-critical priority.

Lateness is measured after the line has been written, so it includes what the
serial driver takes — the USB control transfer on an adapter — and the worst
of it is reported by `\status` and by `gl-keyer send`. That number is what
says whether a given adapter is fit to key with.

A wait never goes more than 20 ms without looking at whether it was cancelled,
so a stop or a dropped connection lands within one element or that much of a
long one.

## Protocol

Commands are lines, and a leading `+` asks for the extended answer: the
command's name, one `Label: value` line per value, and a closing `RPRT`
carrying Hamlib's status, negated. Without the `+` a successful query answers
with its values alone and everything else with `RPRT`, as `rigctld` does.

| Command | Does |
| --- | --- |
| `\chk_vfo` | Answers `ChkVFO: 0`; `Rigctld::connect` asks it first |
| `\rtty_open key=value...` | Opens a stream; the keys are below |
| `\rtty_send <codes>` | Queues codes on the stream this connection opened |
| `\rtty_end` | No more codes; key the tail once the queue runs out |
| `\status` | `State`, `Sent`, `Queued`, `Elapsed` and `Late` in µs, `PTT`, and `Error` after a failure |
| `\stop` | Cuts the open stream short |
| `q` | Closes the connection |

`\rtty_open` takes `baud`, `stop` (`1`, `1.5`, `2`), `parity` (`none`,
`even`, `odd`, `mark`, `space`), `gap` in bits, `diddle` (`none`, `ltrs`,
`blank`), and `lead_in` and `tail` in seconds, each defaulting to
`TxConfig::default()`. Five data bits are implied.

Codes are written one character each: a five-bit code as `0`–`9` then `A`–`V`
by value, and the control codes as `h` (hold mark), `x` (carrier off), `d` and
`e` (diddle off and on). One `\rtty_send` carries at most 4,096, and a stream
holds at most 65,536 waiting.

An unknown command answers `-4` — every rig command, `T` included, is
`rigctld`'s — a malformed one `-1`, a stream that belongs to another
connection or is already open `-9`, and a queue that would overflow `-10`.

`Elapsed` is the stream's own timeline: what a transmitter would have played
of it. A client that wants to know which character has gone out converts it to
samples and asks its `TxSchedule`, as the audio path does with its playback
position. A stream that ran dry has idled for longer than the schedule says,
which is why a client sends a whole message before it ends the stream.

A carrier-off code holds mark: a line has no third state, and a rig keyed on
it keeps its carrier whatever the line does.

## Safety

Unkeying is the application's, through rig control, and a stream cut short
leaves FSK idling at mark under a carrier that is still on. What the keyer
guarantees is that nothing it was asked for outlives the asking.

- A stream belongs to the connection that opened it. When that connection
  goes, its stream stops.
- `--max-transmit`, ten minutes unless changed and off at zero, stops a stream
  that has run that long.
- Opening the port releases both lines and puts FSK at mark, and shutting
  down and Ctrl+C leave it at mark. A terminal unit is aborted instead, which
  releases its PTT and its FSK output, and it aborts by itself when a keyer
  that died without doing so lets go of DTR.

## The Terminal Unit

The JL1HIS terminal unit (`jl1his-terminal-unit`) is an RP2350 that the PC
sees as a USB microphone and a CDC ACM control port. It keys FSK from a PIO
state machine and takes text commands for it, so with `--method jl1his` the
keyer hands it code words over that port instead of switching a line, and the
timing is the unit's. The unit's control port protocol is its own repository's
`docs/control-port.md`; what the keyer uses of it is below.

The keyer needs a firmware with four things that the first one did not have:

| Needed | What it is |
| --- | --- |
| `SENDM <hex>` | `SEND` with each word's uppermost bit first on the line |
| `SENT=` in `STATUS` | Words that have left the queue to be keyed since `PTT ON`, not counting the unit's own diddle |
| `LAST=` in `STATUS` | How the latest transmission ended: `NONE`, `DONE`, `ABORT`, or `TOT` |
| Abort on DTR | The unit aborts when the host drops DTR |

A `STATUS` without `SENT` and `LAST` is unreadable and fails the stream.

- **Taking the unit over.** The port is opened with DTR asserted and kept so,
  which is what lets the unit abort when the keyer goes away however it goes.
  The keyer then sends Ctrl-C, which aborts in any mode, throws away whatever
  the unit says until it has been silent for 100 ms, and asks `VER`, whose
  answer is logged. A port that does not answer with a version within a
  second is not a unit.
- **Answers.** A command is answered by the next line that begins with `OK`
  or `ERR`; any other line is one of the unit's tagged lines and is skipped.
  Nothing ties an answer to its command but their order, so an answer that
  comes after its second has run out would be taken for the next command's.
  A missing or unreadable answer therefore marks the link, and the next abort
  is the whole of taking the unit over again, silence and `VER` included.
- **Framing.** Each stream starts with `SET BITS 5`, `BAUD`, `STOP`,
  `TXDELAY`, `TXTAIL`, and `DIDDLE OFF`, then a `STATUS` for the room in the
  unit's queue. `TXDELAY` and `TXTAIL` are the stream's lead-in and tail in
  whole milliseconds. Diddle stays off because the codes already carry their
  shifts: an LTRS the unit sent on its own while its queue was empty would
  move the receiver out of figures. The unit has no parity, no character gap,
  and no more than five bits, and keys 10 to 1200 baud with lead-in and tail
  up to 65.535 s, so a stream asking for anything else is refused when it
  starts. `INVERT`, `TOT`, `CODE`, and `USOS` are left as the operator set
  them; the last two concern the unit's own text conversion, which `SENDM`
  bypasses. Nothing is `SAVE`d.
- **Codes.** A code goes to the unit as it is held here, the first bit on
  the line uppermost, which is what `SENDM` reads; `SEND` reads the lowest
  bit first and would key every character backwards. Codes go up to 256 to a
  `SENDM`, two hexadecimal digits each, and never more than the room `STATUS`
  last reported less what has been handed over since. Up to 256 are held for
  a unit with no room; past that the method is not `ready` and the rest wait
  in the session's queue. Diddle on and off change nothing without a gap and
  are dropped. Hold mark and carrier off cannot be keyed between the unit's
  characters, so they fail the stream.
- **PTT.** The unit keys FSK only while its PTT output is keyed, and its codes
  wait for `TXDELAY` after `PTT ON`. The keyer sends `PTT ON` after the
  stream's first chance to hand codes over, which is when 256 are held or the
  stream has nothing more to give, so a message sent whole starts its lead-in
  with codes already queued. `PTT OFF` follows the last code handed over once
  the stream has ended; the unit releases PTT after its queue and `TXTAIL`.
  The keyer reports `PTT: 1` for the unit, so the application does not key
  the rig through rig control, and the unit's PTT output has to be what keys
  the rig. It is keyed and released once for every stream, so messages queued
  one behind another are separate transmissions with the rig receiving in
  between, where rig control would have held the carrier across them.
- **The timeline.** Every character takes the same time, so where the stream
  stands is the lead-in plus one character for each code `SENT` counts. The
  keyer asks `STATUS` every 20 ms. What is being keyed, the lead-in or the
  latest character, is counted on the host's clock from when the keyer
  learned it had begun, and no further than its length; the tail is counted
  the same way from the end of the last character or from `PTT OFF`,
  whichever is later. The keyer learns of a start no earlier than it
  happened, so `Elapsed` is never ahead of the air and at most one poll
  behind it, and a stream that ran dry stands still as its line does. Once
  the unit reports the stream done, `Elapsed` is the whole timeline. `Late`
  is always zero: the unit times the line and reports no lateness.
- **Cut short.** A stop, a dropped connection, `--max-transmit`, and shutdown
  send Ctrl-C, which discards the unit's queue and releases its PTT at once.
  A unit that reports PTT released before it was asked to, or with `LAST`
  other than `DONE`, fails the stream, and the error says whether its `TOT`
  ran out or it was aborted.
- **Sent.** `\status`'s `Sent` counts codes the method has taken rather than
  codes keyed.

## Command Line

```text
gl-keyer serve [--method bitbang] --port COM5 --fsk-line dtr [--mark-asserted]
               [--listen 127.0.0.1:4534] [--max-transmit 600]
gl-keyer serve --method jl1his --port COM7
               [--listen 127.0.0.1:4534] [--max-transmit 600]
gl-keyer send  [--method bitbang] --port COM5 --fsk-line dtr [--mark-asserted]
               [--baud 45.45] [--stop 1.5] [--parity none] [--gap 0]
               [--diddle none] [--lead-in 0.5] [--tail 0.5] TEXT
gl-keyer send  --method jl1his --port COM7 [...] TEXT
gl-keyer ports
```

`--method` chooses how the line is timed. `bitbang`, the default, switches the
DTR or RTS line `--fsk-line` names, timed by the keyer. `jl1his` hands the
codes to the terminal unit whose control port `--port` names; `--fsk-line` and
`--mark-asserted` are refused with it, since the unit's `INVERT` decides which
level is mark.

Mark is the released line unless `--mark-asserted` says otherwise, the
RS-232 idle level; which a given rig wants is its own. The default port, 4534,
is the next after `rigctld`'s 4532 and `rotctld`'s 4533. `send` keys one
message without the socket, for checking a station's wiring with the rig keyed
by hand or by `rigctl`; it reports how late the worst transition was.

## The RTTY Application

The RTTY application streams to the keyer in place of playing audio when its
configuration file asks for it:

```toml
[keyer]
enabled = true
address = "127.0.0.1:4534"
```

Like `[rig]`, the section has no editor and is written out with its defaults
on the first save: which line the rig's FSK input hangs off is wired once for
a station. Nothing else about a transmission changes. Messages queue the same
way and the sent text is echoed and underlined the same way. Which of the
keyer and rig control keys PTT is the keyer's to say, so that a station can
change the keyer's method and leave this file alone.

1. A worker connects to the keyer at once and asks `\status`, so a keyer that
   is not running is found out before the rig is keyed, and so the
   application knows from `PTT` whether keying the rig is its job.
2. If the keyer leaves PTT to the application, the rig is keyed through rig
   control with `ctx.keying` set to `"fsk"`, so the script can put the rig in
   its FSK mode before `T 1` ([rig-control.md](rig-control.md)). If the keyer
   keys PTT itself, rig control is not asked to transmit or, afterwards, to
   receive; the connection to the rig stays as it is.
3. Once a rig that was keyed reports it is transmitting — after rig control's
   `lead-in` — or at once if none was, the worker opens the stream and sends
   the whole message, then `\rtty_end`. Opening waits for the rig because the
   keyer starts keying the moment a stream opens. Sending everything before
   ending means the stream never runs dry: a dry stream idles at mark and
   falls behind its schedule.
4. The worker asks `\status` every 20 ms. `Elapsed`, in microseconds, is read
   as a playback position against a `TxSchedule` built at 1 MHz, which is what
   drives the echo, the underline, the time remaining, and the text returned
   by Stop.
5. When the stream completes, the worst `Late` is written to the log, since it
   is what says whether an adapter is fit to key with. Then the next queued
   message goes, or the rig is unkeyed.

Stop drops the worker, which sends `\stop` and closes the connection, so the
keyer stops the stream at mark either way. A keyer that cannot be reached,
refuses part of a stream, or ends one cut short fails the message. The
unsent text goes back to the draft with the queue behind it, as a rig failure
does. No audio device is needed to send. Tones, level, and reverse are not
used, because a keyed line has two states: the rig decides what they sound
like, and `--mark-asserted`, or the terminal unit's `INVERT`, decides which
state is mark. Nothing in the application's settings changes for a terminal
unit: only `gl-keyer serve --method jl1his` differs, the keyer's `PTT: 1`
takes the keying away from rig control, and a character gap or parity the
unit cannot key fails the message as soon as the keyer starts the stream,
reported as a failed stream. The status bar names the keyer's address while
the section is on, since nothing else on screen says the audio is not being
used.

## Later

- **CAT on the same port.** A rig with one USB serial port — CAT and the
  keying lines together — cannot have `rigctld` and the keyer both open it.
  The keyer would hold the port and relay its data lines to a TCP port, and
  `rigctld -r 127.0.0.1:<port>` reads a `host:port` path as a network
  connection whatever the rig model; Hamlib 4.0 through 4.7 all do. Whether a
  blocked read on a Windows synchronous handle delays a line change behind it
  is the first thing to check, and why the keying thread already owns the
  line.
- **Other methods.** A UART TxD `Method`, GPIO `ControlLine`s, a CW payload
  beside the RTTY one, and the PTT line described above if a station needs
  it.

## Verification

- `Framer` against hand-framed characters, stop lengths, parity, control
  codes, and diddle.
- `LineBitbang` on a fake clock and a recording line: each boundary at its
  deadline, lateness measured without pushing later deadlines, polarity, and
  cancellation.
- `Session` transitions: ownership, the queue bound, stopping before and
  during keying, disconnects, and the time limit.
- The protocol's code alphabet and framing arguments round trip, and malformed
  input is refused.
- A keyer served on a loopback socket and driven by `KeyerClient` and
  `grayline_rig::Rigctld`: a stream keys the line in framing order, a dropped
  connection stops its stream at mark, requests that conflict are refused,
  status says whether the keyer keys PTT itself, and rig commands are left to
  `rigctld`.
- `Jl1hisTerminalUnit` against a simulated terminal unit on a fake clock: a
  stream is set up, handed over, and followed to its tail; a code reaches the
  unit first bit uppermost; tagged lines are not taken for answers; every report
  of progress while the unit keys is no further than what it has keyed and no
  more than a poll behind; a stream that runs dry is not believed ahead of
  the unit; a stream longer than the unit's queue is handed over as room
  comes, with no `SENDM` refused; a `TOT` that runs out, in the characters or
  in the tail, fails the stream; the keyer's own time limit stops a stream
  the unit is still keying; a cancelled stream is aborted at the unit; a late
  answer fails its stream and not the one after; framing the unit cannot key
  is refused before anything is set; and a port that does not answer is not
  taken for a unit.
- In `grayline-keyer-protocol`: every state name reads back, and a status
  answer is read by its labels.
- In the RTTY application, against a stand-in keyer: the keyer is asked
  whether it keys PTT before anything is opened, the rig is keyed for a keyer
  only when the keyer leaves PTT to it and only until the stream has started,
  nothing is opened before the stream is started, a started stream is sent
  whole and followed to its end, a
  stream cut short and an unreachable keyer are failures that give the text
  back, and a cancelled stream is stopped at the keyer.
