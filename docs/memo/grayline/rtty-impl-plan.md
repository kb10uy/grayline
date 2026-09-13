# Grayline RTTY Application — Implementation Memo

This is a working memo, not a description of code that exists. It records the
design agreed on 2026-08-15 for `apps/rtty`, the desktop RTTY application, and
is written down so the decisions survive between working sessions. The
`grayline-rtty` core and the `gl-rtty` command-line tool are already on branch
`rtty`; the application is the milestone after them, and it is the last of the
RTTY items listed under Planned Gaps in
[grayline/architecture.md](../grayline/architecture.md).

**Progress.** Steps 1 through 5 of the order below are implemented, so
`apps/rtty` receives, transmits, and can be watched: the skeleton, the receive
worker and its session, the scrollback pane, the tuning and squelch panel, the
scope window, the transmit panel with its queue and macros, and the station and
contact fields, together with the repository work step 2 drags with it. The
contact directory is wired in as well, which the plan below never named; it is
described under Macros and Templates and in
[qso-directory.md](qso-directory.md). Aligned save, the received-text history
log, the rig frequency readout, and the release workflow are not written. Where
the implementation departs from what is described below, the departure is
recorded in the section it belongs to.

Everything the core already offers is assumed rather than restated here:
[grayline/rtty.md](../grayline/rtty.md) covers the crate and where it parts from
MMTTY, [rtty/protocol.md](protocol.md) covers the signal, and
[grayline/gui-design.md](../grayline/gui-design.md) covers the application
frame — egui through eframe, the native and in-window menu pair, the platform
module, the file and help menus — all of which this application inherits
unchanged.

## Skeleton

The application is started from `apps/wefax` rather than `apps/sstv`, because
the WEFAX application is the smaller of the two and carries no image library,
template composer, or rig script host.

Near-verbatim reuse: `main.rs`, `identity.rs`, `locales.rs` and its tests,
`build.rs`, both menu renderers under `ui/menu/`, `storage/paths.rs`,
`storage/config.rs`, and `worker/wav.rs`, together with the worker pattern
itself — the `Waker` in `apps/wefax/src/worker.rs`, the mailbox, and the
controls handle. Audio stays entirely inside `crates/audio`; the application
adds no audio code of its own.

The menu model and both renderers now live in `grayline-shell::menu`; each
application supplies its own action type and menu contents.

Genuinely new work is confined to three things: a receive-text scrollback
widget, a transmit message queue with its worker, and the RTTY settings and
controls themselves.

Package naming follows the existing pair: the package is `grayline-rtty-app`
so it does not collide with the `grayline-rtty` library, and the binary is
`grayline-rtty`. The CLI binary `gl-rtty` is unaffected.

`apps/rtty/assets/icon.png` and `icon.ico` were generated in the family's
visual language — the rounded blue frame, the navy sky, the sun behind the
grayline, the name across the top — with the palette sampled from the WEFAX
icon. They are placeholders in the sense that the lettering is a system face
rather than the drawn one the other two carry; everything that reads them
(the Windows resource, the desktop entry, the identity) is done.

## Layout

A left/right split first, then a top/bottom split of the left side.

The right pane is full-height and of fixed width, like the SSTV side panel. It
holds the settings the main interface exposes — tone pair, baud rate, AFC,
squelch, column count, and the button that opens the scope — with the QSO entry
fields below them: his call, his name, and RST sent and received. The left pane
splits into receive text on top and the transmit area below: the message queue
with its sent-text underline, the input line, and the macro buttons.

As built, the pane holds the mark tone, the shift, the speed, reverse, AFC,
and the squelch with its threshold over a meter of what the threshold is being
set against. Two departures, both following
[gui-design.md](gui-design.md)'s rule that only controls which do something are
built: there is no column count, because it would be pinned to one and a
control that cannot move says nothing an operator can act on — the column list
is `DecodePath::ALL` and a second demodulator is what changes it — and there
were no QSO fields, because what reads them is the macro engine, which arrives
with transmit. The contact fields are there now, in a section below the
squelch. Unshift-on-space and the threshold corrector are on the Settings menu
rather than the panel: they are set once for a station's habits rather than
worked while listening. The panel gained one control the plan did not name,
`Take Detected Pair`, because what AFC found is lost the next time the
receiver is built and the operator had no way to keep it.

**This station's own callsign, name, and QTH are not on the panel at all.**
They are a modal window behind the Settings menu, which is where
`apps/sstv/src/ui/view/dialogs.rs` keeps the same three and for the same
reason: none of them belongs to the contact being worked. A callsign is
entered when the application is first set up and then left for years, while
everything on the panel is worked at every exchange. The window runs the same
keyboard filter the message field does, since what is typed into it reaches
the air through the macros that read it.

The same window carries **the fields the operator invents**, which the SSTV
application keeps in a second dialog of its own. One window here rather than
two, because the RTTY list is short and everything in it is the same kind of
thing: something this station says about itself that a macro signs with. A
field is named and given a value in two columns with a row per field, reached
from a macro as `${custom.<name>}`. The prefix is what keeps the operator's
names and the built-in ones apart, so a name added to the built-in set later
cannot take one out from under a macro already using it.

A name is taken up when the field is left rather than on every keystroke,
because half a name is a different field; a name no `${...}` expression could
hold is drawn in the fault colour and kept out of what the macros see, but left
on screen to be corrected. The name never leaves the configuration, so it does
not run the keyboard filter; the value does, because it goes on the air through
whichever macro names it. Storage is a `[variables]` table whose keys are
assigned rather than rewritten, so a comment written beside one by hand
survives a save that did not touch it, and an unusable key in a hand-edited
file is dropped the way every other unusable value is.

Panel claim order, which egui makes load-bearing, is: status bar at the bottom,
then the right panel so it runs the full height above the status bar, then the
transmit panel at the bottom of what is left, and finally the central panel,
which becomes the receive text. This is the order `apps/sstv/src/ui/view.rs`
already claims its panels in, and the reasoning recorded there applies
unchanged — fixed and exact width rather than resizable, because everything in
the panel is laid out from the width it is given, and a panel squeezed by a
narrow window otherwise keeps the squeezed width. The module split mirrors
`apps/sstv/src/ui/view/{panels,status_bar,dialogs}.rs`.

## Receive Columns

The interface is designed for N parallel decode paths: one `ReceivePipeline`
per column, every one of them fed the same PCM, each with its own header
showing the path name, signal-to-noise, the mark-minus-space magnitude, and the
current case. The cost is affordable — about 1.2 ms per second of audio per
pipeline at 48 kHz — so several columns are a real option rather than a
theoretical one.

**The first version is fixed at one column.** The mechanism around it is built
for more: a descriptor enum naming the path, per-column headers, and a column
count in the settings pane that starts pinned to one. The path names in the
mockup, "PLL + majority" and "FFT + repeat synthesis", are demodulators the
core does not have yet; they are the discriminators deliberately deferred in
[mmtty/porting.md](../mmtty/porting.md), and repeat synthesis is further out
still and to be ignored for now.

The CER column from the mockup is dropped. Character error rate has no
definition worth showing without ground truth to compare against.

When columns do return, cross-column diff highlighting and an aligned save can
both use the `sample` field that every `RxEvent::Character` carries to line the
columns up against each other.

Per-column headers can read `ReceivePipeline::signal_strength`,
`ReceivePipeline::case`, and `ReceivePipeline::tones` as they stand. The
mark-minus-space figure is not exposed today and arrives with the monitor tap
below.

As built, a header carries the path's name at one end and, at the other, the
pair it is actually detecting, the case, and the signal reading. The tone the
comparator is on is not shown: at baud rate it is a light flickering faster
than it can be read, and the pair reading and the signal meter are what tuning
is actually done on. The pair is drawn in the same family as the labels beside
it rather than in the monospaced one — two families laid out at one size do not
share a baseline, and the proportional family's figures are tabular, so the
reading keeps its width as the frequency control moves it.

A settings change rebuilds every pipeline, because a receiver is built from
its configuration and cannot be retuned in place. That costs a few
milliseconds of filter settling and nothing else — the text lives in the
interface — and it is why the worker compares the whole settings set on each
block rather than acting on a change flag.

## Scope Window

A separate window rather than a panel: an egui 0.35 deferred viewport with the
always-on-top flag, opened from the panel beside the text or from the View
menu, and remembered in the configuration, because an operator who works with
it open should not have to open it every session. Deferred rather than
immediate: the main window sleeps between the frames a reception gives it, and
the two windows should not have to wake together. That is what the lock in
`ui/scope.rs` is for — the callback egui runs is handed the state rather than
the application — and the application fills that state once per frame from the
same snapshot the rest of the interface reads, so the two cannot disagree
about what is arriving.

It shows two things. The band spectrum is computed application-side with the
`grayline-dsp` FFT over raw PCM, with its own transform length of 2048 and its
own cadence, deliberately independent of the pipeline's own analysis and of
AFC, so that changing one does not silently change the other. It is computed
in the receive worker rather than in the interface, which departs from the
plan and not from its reasons: the raw PCM is already there, the publish
interval is the frame-rate throttle the plan asked for — one snapshot every
33 ms — and a closed window then costs nothing whatever, because neither the
transform nor the samples it reads exist while it is closed. Magnitudes are
published to 4000 Hz, MMTTY's own wider display ceiling
([../mmtty/dsp.md](../mmtty/dsp.md)), which is what a 3000 Hz mark and an
850 Hz shift need; the decibel scale they are drawn on is the interface's.

The XY scope draws the monitor tap's channel pairs, the core addition listed
below, which was written first. The tap is opened and closed through a control
of the worker's own rather than through `WorkerSettings`, because a settings
change rebuilds every pipeline and opening a window must not throw away the
reception being watched — the same reason the core made it a setting rather
than a construction argument. Only the first decode path is tapped, every
column being fed the same audio, and the pairs are decimated to about four
thousand a second, which is already more than the width of the picture. MMTTY
interpolates its own channels up to get that resolution because it demodulates
at half rate; this path does not decimate, so it throws pairs away instead
([../grayline/rtty.md](rtty.md)).

Two things the picture does that the plan did not say. The trace is scaled by
the strongest pair on it, because a channel carries a rectified envelope whose
height depends on how much of the band-pass the tones fill, and what is read
off the picture is the shape rather than the level — the level is the meter
beside the text. And the line where the two channels are equal is drawn under
it, because that line is the comparator's own decision: a pair on the tones
swings between the axes either side of it, and a mistuned one collapses onto
it.

Every frame carries a sequence number. Nothing on a scope is compared against
a threshold — a trace that looks like the last one is still a new trace — so
the number is what tells the interface, whose `Visible` comparison otherwise
keeps it asleep through a band that has not moved, that there is a picture
worth drawing. The window is then asked for a frame of its own rather than
left polling for one.

## Transmit

Buffered, not live keying: the operator types a message and presses send. This
was decided deliberately, and it settles an open core question — the existing
`Transmitter`, whose contract is that the end of its code iterator is the end
of the transmission, is exactly right for it, so the live-keying core extension
discussed earlier is **not** needed.

The transmit worker mirrors `apps/sstv/src/worker/transmit.rs`: a bounded
queue, the `TxPhase` states, prefill before the stream starts, and cancel on
drop.

The sent-text underline follows `Playback::played_samples` from
`crates/audio/src/playback.rs`, mapped through the boundary table `TxSchedule`
computes before the transmission starts.

**As built.** Each queued message is its own `Transmitter` run on its own
playback stream, rather than one stream carrying the queue. That is what puts a
mark idle between messages without anything having to insert one, since every
transmission brings its own lead-in and tail, and it is what lets an abort drop
the stream and stop the carrier where the operator pressed rather than at the
end of what had already been generated. It also means the schedule is built
against the rate the device actually opened at.

The interface departs from the plan's list in four places, each because
something the plan did not name turned out to be load-bearing.

- **Enter writes a line; Ctrl+Enter sends.** MMTTY's Enter puts CR LF on the
  air, so the habit is already a line ending, and a field that sent on Enter
  would put half a message out every time the operator reached for a new line.
  Escape stops, from wherever the keyboard is.
- **The message field only ever holds what can be sent.** A keystroke with no
  Baudot code does not appear, which is the answer a teleprinter with no such
  key gives; pasted and expanded text keeps what it cannot send and draws it in
  red with the send button held, because a block that silently lost part of
  itself would hide the loss. Both paths upper case what they take, so the
  field shows what leaves. `ui/input.rs` does this by rewriting the frame's
  input events before the field is added, which is possible because egui
  reports typing, pasting, and IME commits as separate events; an IME commit is
  treated as typing, so a Japanese string committed by mistake simply
  disappears. The same filter runs on the station and contact fields, since
  what is typed into them is typed to be sent.
- **What is sent is printed into the received text** in a colour of its own,
  following the played position rather than the generated audio. A contact is
  one exchange rather than two, and a transcript holding only half of it would
  have to be read against a transmit field already cleared for the next
  message. It is also what makes the history log, when it is written, a log of
  the whole contact.
- **Stopping gives the unsent text back**, ahead of whatever is in the field,
  and so does a message that could not open a stream. Both are text the
  operator wrote, and dropping either would lose it with nothing saying so.

A transmit level is on the panel, which the plan did not list: a sound card
feeding a rig needs one, and the fader is squared for the reason recorded
beside the SSTV application's own. An underrun is reported once per
transmission, because a queue that ran dry put a gap in a character that the
receiving station reads as noise.

PTT is VOX in the first version. That is what avoids the SSTV rig worker lift —
roughly 1500 lines of application-local Lua host — and keeps the milestone
small. A rig frequency readout in the status bar needs only a thin read-only
rigctld client through the shared `crates/rig`, and is deferrable.

## Macros and Templates

MMTTY's `ConvString` (`Main.cpp:4142`) is the reference for what a macro can
say: `%m` my call, `%c` his call, `%n` his name with "OM" as the fallback, `%q`
QTH, `%r` and `%s` the two RSTs, `%R` and `%N` the RST split into three digits
and a contest number, `%D` and `%T` UTC date and time, and `%g`/`%f` the
time-of-day greeting.

Grayline adopts the semantics but not the syntax: readable placeholders
instead of percent letters.

**As built, the interpolation is the family's rather than this
application's.** `crates/variables` holds the expression syntax, the rule for
what may be named, and the values a name can stand for; it was lifted out of
`grayline-sstv-template`, which had it first and now re-exports it. RTTY holds
only the half that is its own: which names exist and what each stands for. Two
readers of one syntax drift, and an operator who has written a template should
have written a macro.

So a macro reads exactly as a template does: `$$` is one dollar sign, a dollar
that opens nothing is one too, `${name:format}` writes a timestamp however the
macro asks, and a name nothing was provided for is a refusal rather than a gap.
The refusal is reported on the status bar naming the name; the message is not
written into the field at all, because what it would put there is text with a
hole where a callsign belongs. The braces cost nothing on the air, being
outside ITA2; the dollar sign is FIGS-D in the Bell table and so sendable, but
a name is never written without its braces.

The names are the SSTV templates' too — `station.callsign`, `station.qth`,
`station.grid`, `contact.callsign`, `contact.name`, `contact.qth`,
`report.sent`, `report.received`, `application.version`, and
`tx.timestamp.{utc,local}`. Three of SSTV's are missing because this
application has nothing to answer with: `radio.*` wants the rig control that is
not written, `rx.timestamp.*` wants a reception with a beginning and an end,
and `report.number` wants the contest serial the plan deferred. One is added,
`greeting`, which is MMTTY's `%g` and which no template ever wanted.
`contact.name` keeps MMTTY's `OM` fallback, which is not the kind of
placeholder the SSTV templates refuse: `OM` is what an operator actually says
to a station whose name they have not been told.

**The contact directory adds the rest of the `contact.` names.** Whatever is
filed under the callsign being worked is folded in under that prefix, so a
macro reaches `${contact.grid}` or `${contact.jcc}` although the panel has no
field for either. The panel is written over the top of it wherever the two name
the same thing: what is typed beside the received text is what the operator
heard on the air, and what is filed is what somebody wrote down once. A
`contact.` name nothing answered expands to nothing rather than refusing, which
is the one exception to the rule above and is the SSTV composer's rule for the
same reason — which fields a station has is a property of that station rather
than something the operator writing the macro could know. It stays scoped to
`contact.`: a typo in a `custom.` name is still worth reporting, because those
are invented in a window that already refuses unusable ones.

Macro buttons answer to F1 through F12. A press expands at that moment and
writes the result into the field at the caret, so the time a message names is
the time it was written and what is about to go out can still be edited; a
macro marked `send = true` goes out as a message of its own and leaves a
half-written reply alone. Definitions live in the configuration TOML, written
once on first run and then never rewritten, because the file is where they are
edited and rewriting the array on every save would reformat what the operator
put in it. A callsign reaches the contact field by being double-clicked out of
the line that printed it.

Deliberately skipped: `%L`/`%F` raw shift insertion, `%E` (a buffered send ends
by itself), the CW identifier, the `%R`/`%N` contest split, and an
in-application macro editor — the File menu opens the directory the file is in,
and the red marking reports a macro that cannot be sent the moment it is
pressed.

## Core Additions

Only two, and both are small:

1. A monitor tap on the receive front end exposing decimated mark/space
   resonator output pairs, for the XY scope and the per-column mark-minus-space
   figure.
2. A helper returning per-`TxCode` durations, for the sent-text underline.

Everything else the application needs already exists in `crates/rtty`,
`crates/audio`, `crates/dsp`, and `crates/shell`.

The first is written, as `rx/monitor.rs` and described in
[rtty.md](rtty.md): what it hands out is the pair the comparator compares —
rectified, integrated, and corrected — rather than the resonator outputs, both
because that is what MMTTY's own XY scope draws and because the signed
difference of the two is the tuning figure the header wants.

The second is written, as `tx/schedule.rs`. It grew past the per-code duration
the plan named, into `TxSchedule`: given the message text rather than a code
list, it reports where each *character* finishes going out. That is the figure
every caller actually wanted — the underline, the echo into the received text,
the remaining-time readout, and the unsent remainder an aborted transmission
gives back — and the character-to-code mapping it needs belongs beside the
encoder that creates the discrepancy rather than in the application. The
per-code duration is still there as the private `Timing::code_samples`, now
shared with `Transmitter` so the two cannot disagree.

## Implementation Order

1. The two core additions above.
2. The application skeleton, plus the repository work it drags with it: a row
   in the CI application matrix, the corresponding exclusion from the core job,
   and a case in `package/build-app.sh`.
3. The receive worker, its session, and the scrollback widget.
4. The scope window.
5. Transmit and macros.
6. Aligned save and the rig frequency readout.
7. Tests, history log, documentation, and the release workflow.

Repository touch-points for step 2, each of which is hard-coded per application
and must be edited by hand:

- `.github/workflows/ci.yml`: add `--exclude grayline-rtty-app` to the core
  job's `SCOPE` (currently line 51, beside the two existing exclusions), and add
  a third entry to the `app` job's matrix `include` list.
- `package/build-app.sh`: add an `rtty` case with its bundle name, display
  name, and its own `NSMicrophoneUsageDescription` sentence — the script exits
  on an unknown application, so this cannot be forgotten silently.
- `.github/workflows/release-rtty.yml`, on the tag pattern `rtty-v*`,
  alongside the existing `release-sstv.yml` and `release-wefax.yml`.

Step 7 in full: `App::headless()` tests and `egui_kittest` runs over every
locale, the locale key-scan tests copied wholesale from the existing
applications, a received-text history log, and documentation — the Planned Gaps
entry in [grayline/architecture.md](../grayline/architecture.md) updated,
`AGENTS.md`, and the README.

## Open Questions

- Whether the `%R`/`%N` contest split makes the first version.
- The storage shape of the received-text history log.

Related: the decimation question in the internal notes stays out of scope here.
The application changes nothing about it, and
[grayline/rtty.md](../grayline/rtty.md) keeps its account of why nothing is
decimated.
