# Keyer

`gl-keyer` keys a rig's FSK input from a serial port's DTR or RTS line. It is
a process of its own that clients reach over the line protocol `rigctld`
speaks, so an application talks to it through the same transport client it
already talks to the rig through. The package is `grayline-keyer`, in
`tools/keyer/`.

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

## Why Not PTT

PTT is held for a whole transmission and switched once at each end of it, and
the applications already do that through rig control
([rig-control.md](rig-control.md)), queue and lead-in and failure handling
included. A rig keyed in its FSK mode transmits on `T 1` from `rigctld` like
in any other, so the keyer has nothing to add there, and a second place that
could key the transmitter would be a second set of rules for when it is
unkeyed.

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
| How it is timed | Line switching, timed by the keyer | A UART's TxD, timed by the UART; a keyer chip | `method::Method` |
| Where it goes | A serial port's DTR or RTS | GPIO, a parallel port | `line::ControlLine` |

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
takes the stream a code at a time and blocks until each has gone out; a UART
method would write the code and wait for it to drain, with no framer at all.

## Shape

```text
line.rs      ControlLine, and a serial port's DTR or RTS behind it
clock.rs     Clock, and the host clock waited on as tightly as it allows
method.rs    Method, and LineMethod: one deadline per element boundary
stream.rs    key_stream: one stream from lead-in to tail, from a CodeSource
session.rs   what clients asked for, with no I/O in it
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

`LineMethod` measures every boundary from the start of the stream rather than
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
| `\status` | `State`, `Sent`, `Queued`, `Elapsed` and `Late` in µs, and `Error` after a failure |
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
  down and Ctrl+C leave it at mark.

## Command Line

```text
gl-keyer serve --port COM5 --fsk-line dtr [--mark-asserted]
               [--listen 127.0.0.1:4534] [--max-transmit 600]
gl-keyer send  --port COM5 --fsk-line dtr [--mark-asserted]
               [--baud 45.45] [--stop 1.5] [--parity none] [--gap 0]
               [--diddle none] [--lead-in 0.5] [--tail 0.5] TEXT
gl-keyer ports
```

Mark is the released line unless `--mark-asserted` says otherwise, the
RS-232 idle level; which a given rig wants is its own. The default port, 4534,
is the next after `rigctld`'s 4532 and `rotctld`'s 4533. `send` keys one
message without the socket, for checking a station's wiring with the rig keyed
by hand or by `rigctl`; it reports how late the worst transition was.

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
- **The RTTY application.** Choosing FSK there means streaming to the keyer
  instead of playing audio while rig control keys PTT as it does now,
  telling the script which it is so it can select the rig's mode, and
  moving `protocol.rs`'s code alphabet into a crate both
  sides depend on.

## Verification

- `Framer` against hand-framed characters, stop lengths, parity, control
  codes, and diddle.
- `LineMethod` on a fake clock and a recording line: each boundary at its
  deadline, lateness measured without pushing later deadlines, polarity, and
  cancellation.
- `Session` transitions: ownership, the queue bound, stopping before and
  during keying, disconnects, and the time limit.
- The protocol's code alphabet and framing arguments round trip, and malformed
  input is refused.
- A keyer served on a loopback socket and driven by `grayline_rig::Rigctld`:
  a stream keys the line in framing order, a dropped connection stops its
  stream at mark, requests that conflict are refused, and rig commands are
  left to `rigctld`.
