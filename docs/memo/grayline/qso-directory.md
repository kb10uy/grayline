# The Contact Directory

## What it is, and what it is not

A directory of stations, not a log of contacts.

What it answers is "what do I know about this callsign", which is a property of
the station. Who was worked, when, on what band, and with what report is a
property of a contact, and that is what the operator's own logger is for. A
second copy of it here would be one nobody updates and everybody eventually
distrusts, so nothing in this store records that a contact took place: there is
no date, no band, no mode, no report, no confirmation state, and no export.

What it holds is `Map<Callsign, Map<Key, Value>>` and nothing more. It exists
because a transmit template that prints the other operator's name was otherwise
asking the operator to type that name again for a station they have worked
before, and because the RTTY macros described in
[rtty-impl-plan.md](rtty-impl-plan.md) read the same fields through
`${contact.<key>}`. Both applications use `grayline-qso`, including its shared
contact worker, to look up and edit station records.

## The record

A record is a callsign and a table of fields under it. A callsign is trimmed and
uppercased on the way in, and text that is not a callsign is refused rather than
stored — three to sixteen characters of `[A-Z0-9/]` holding at least one digit
and at least one letter. Refusing rather than accepting whatever arrived is what
keeps a half-typed field and a garbled FSK identifier from being filed as a
station.

A key is one identifier segment: a letter or underscore, then letters, digits
and underscores. The rule is not arbitrary. A key is read from a template as
`${contact.<key>}`, so a key holding a hyphen or a dot would be filed perfectly
well and then be unreachable from the only place it is wanted. `grayline-qso`
restates the rule that `grayline_variables::valid_variable_name` applies to
each segment rather than depending on that crate, so the directory depends on
nothing in the workspace; the SSTV application is where both are in view, and
it carries the test that holds them to each other. The key `callsign` is refused
outright, so a record can never shadow the field the operator typed.

### The keys this crate names

Naming a key, filling it in, and putting a field on screen for it are three
different questions, and running them together is what produced a dialog of
fourteen rows nobody wanted. The keys are named here so that the same fact
arrives under the same name whether it was typed or imported. Which of them an
application shows is settled separately, by the operator; see
[the fields a station uses](#the-fields-a-station-uses).

| Key | ADIF field |
| --- | --- |
| `name` | `NAME` |
| `name_latin` | — |
| `qth` | `QTH` |
| `qth_latin` | — |
| `grid` | `GRIDSQUARE` |
| `jcc` | — |
| `dxcc` | `COUNTRY` |
| `dxcc_id` | `DXCC` |
| `cq_zone` | `CQZ` |
| `itu_zone` | `ITUZ` |
| `continent` | `CONT` |
| `state` | `STATE` |
| `county` | `CNTY` |
| `iota` | `IOTA` |
| `qsl_manager` | `QSL_VIA` |
| `email` | `EMAIL` |
| `note` | `COMMENT`, else `NOTES` |

Three of them no import fills in, and they are named anyway because a key that
is only ever typed still has to be spelled the same way by everyone who reads
it. `name_latin` and `qth_latin` exist because **ITA2 carries no kanji**: a
station keeping a contact's name in its own script has nothing RTTY can send,
and needs somewhere to keep a form that it can. That is a problem shared by
every non-Latin script rather than a Japanese one, which is why the keys are not
spelled `romaji`. `jcc` holds a JCC or JCG code — one key rather than two,
because the codes are the same field of a QSO and nothing reads them apart.

A record is not limited to these. Whatever else the operator files keeps its own
name and is read back under it, which is what makes `${contact.rig}` printable
without anything here changing.

Everything an ADIF log says about *contacts* is dropped on the way in: `BAND`,
`MODE`, `FREQ`, the reports and the QSL fields. The operator's own side of a
logged contact — `OPERATOR`,
`STATION_CALLSIGN`, everything under `MY_` — describes the station reading the
file rather than the one it is filed under.

### The fields a station uses

What an operator files depends on where they operate and on what they work.
A station in Japan wants a JCC code and a name RTTY can send; a station anywhere
else wants neither and would rather not scroll past them. Nothing here can
decide which of those an operator is, so nothing here tries: the field list is
written in the settings, out of keys and of groups spelled `!name`.

```toml
[qso]
fields = ["!ja", "dxcc"]
```

| Group | Keys |
| --- | --- |
| `!core` | `name`, `qth`, `grid` |
| `!latin` | `name_latin`, `qth_latin` |
| `!ja` | `name`, `name_latin`, `qth`, `qth_latin`, `grid`, `jcc` |

Two of these are parts and one is a whole: `!ja` is what a station in Japan
would otherwise assemble out of the other two and a JCC code, offered whole so
the common case is one entry rather than three. A test holds it to that sum, so
the shorthand cannot drift from what it stands for.

Groups expand where they are written, so the order is the operator's own, and a
key named twice appears once and where it first appeared. `["!core"]` is the
default: three fields, which is what a contact is worth filing at all.
An empty list is taken as written — an operator who asked for no fields wants a
dialog holding only what the station already has, and handing the default back
would be arguing with them.

The list is deliberately not a set of country profiles, and it should not become
one. It is two parts, one shorthand, and the ability to name any key at all;
anything a station needs beyond that it names for itself, and the store files it
without being told it exists. `gl-qso keys` prints both the keys and the groups,
because a field list is written out of both.

## The store

`<data_dir>/Grayline/contacts.sqlite3`, one level above the per-application
directories, which is the place `grayline_shell::platform::FAMILY_DIRECTORY`
already reserves for what the family shares. Under the data directory rather
than the configuration one because this is accumulated data rather than a
setting, and because it should follow the operator between machines — the
opposite of the log, which describes the machine it was written on.

```sql
CREATE TABLE station (
    callsign     TEXT PRIMARY KEY NOT NULL,
    looked_up_at TEXT,
    found        INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE station_field (
    callsign   TEXT NOT NULL REFERENCES station(callsign) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    origin     TEXT NOT NULL,
    written_at TEXT NOT NULL,
    PRIMARY KEY (callsign, key)
) STRICT, WITHOUT ROWID;
```

A key and value table rather than a wide one, because the model is a map and the
key set is open. `station` still carries `looked_up_at` and `found`, the history
of the Wavelog lookup an earlier build kept. Nothing reads or writes them now,
and they stay rather than costing every store a migration for two columns that
take no room. Timestamps are RFC 3339 UTC text written with `jiff`,
which is what the rest of this project describes time with.

The shape is held in SQLite's own `PRAGMA user_version` rather than in a table
of its own, because the counter already exists. A file at a version above the
one the build knows is refused rather than opened: the columns it cannot see are
ones it would drop on the next write.

**Write-ahead logging is set rather than left at the default**, because two
applications in this family may be running at once and under the rollback
journal a reader blocks a writer. Two consequences follow. WAL does not work
over a network filesystem — a Windows roaming profile is a local copy
synchronized at logon, which is fine, but a store deliberately placed on a share
is not supported. And WAL leaves `-wal` and `-shm` files beside the store, which
a roaming profile would copy along with it, so the worker runs
`PRAGMA wal_checkpoint(TRUNCATE)` as it stops and what roams is one
self-contained file.

### Origins, and why the operator always wins

Every field records where it came from, and the sources are ordered:
`adif` < `wavelog` < `manual`. A write happens only when the origin doing the
writing is at least as strong as the one that wrote what is there. So an import
fills in what is missing and refreshes what another import put there, and
cannot displace a value the operator typed. Nothing writes `wavelog` any more;
it is what the lookup an earlier build made filed, and it is still read so that
what the operator types displaces it rather than the field reading as an
origin this build does not know. An origin the
file holds that the build does not recognize is read as the strongest there is:
a newer build wrote it, and guessing it weaker would be a way to lose it.

Dropping a field is its own operation rather than re-filing the record that is
left, because re-filing would stamp every surviving field with the origin doing
it and quietly turn an imported value into one no later import will ever refresh.

## No lookup

Earlier builds asked the operator's own Wavelog about a station the store had
nothing recent to say about, over its API, with a key kept in a
`credentials.toml` of the family's. That went once the applications came to
follow jl1his-logger's entry form ([logger-link.md](logger-link.md)). The
logger is where the contact is typed and logged, and a second source for the
station's name and QTH beside it was a second answer that could disagree, at
the price of an HTTP client, a TLS stack, a credential on disk, and a request
to a server on every new station.

What stays is the store, as the data the templates and the macros read:
filled by hand in the contact dialog or with `gl-qso set`, and by importing an
ADIF log. A `credentials.toml` an earlier build read is no longer read, and can
be deleted.

## Settings

```toml
[qso]
fields = ["!core"]
```

`grayline-qso::ContactSettings` is what the section holds, and its `read` and
`write` are the only code that parses or writes it: each application finds the
`[qso]` table in its own file, hands it over with the defaults it starts on,
and leaves its own keys in that table, such as SSTV's serial number, alone.
The `lookup` switch and the `[qso.wavelog]` address an earlier build wrote are
taken out on the next save, because a file that still offered them would be
offering a switch that does nothing.

The store's path is named by whoever builds the worker rather than discovered
inside it. A discovered path is one a test cannot escape: the suite would
otherwise write its store into the operator's own directory.

## ADIF import

`<FIELD:length>value` and `<FIELD:length:type>value`, with `<EOH>` ending the
header and `<EOR>` ending a record. The length is honoured rather than the value
being scanned to the next `<`, because a `COMMENT` may legally hold one. A
record the document never closed is still read: a log written by a program that
was still running has no final `<EOR>`, and the QSO it was in the middle of is
the most recent one there is.

The document arrives as bytes with an encoding named, and is walked as bytes
with each value decoded on its own. **Turbo HAMLOG and the loggers of its
generation write Shift_JIS**, which is exactly the log this project's first
operator is most likely to have, and reading one as UTF-8 fails on exactly the
fields — the name, the QTH — that the import was wanted for. It also cannot be
decoded first and then walked: a tag's length counts bytes of the document as it
was written, so a Shift_JIS `<QTH:6>東京都` says six, and six characters of the
decoded text would be the QTH and the three characters after it.

**A station worked many times is filed once, and each of its fields comes from
the most recent QSO that carried one.** Not the whole of the last QSO: a later
contact that left the QTH blank is not a retraction of the QTH an earlier one
recorded, it says nothing about the QTH at all, and taking the last record whole
would let one sparse contest entry erase everything a rag-chew years earlier
established. A record with no `QSO_DATE` that can be read sorts as the oldest, so
an undated entry never outranks a dated one. The whole import runs in one
transaction at `Origin::Adif`, so an interrupted import leaves the store as it
was rather than half filled with a log that would have to be imported again.

MMJASTA's `MMJASTA.MDT` is not supported and is not planned.
[../mmsstv/jasta.md](../mmsstv/jasta.md) describes it as a flat array of
fixed-size Win32 records belonging to a contest scorer this project does not
reimplement, and any station holding one also holds the ADIF it was exported
from. Log200 and Turbo HAMLOG's own formats are out for the same reason: both
export ADIF.

## `gl-qso`

A development tool under `tools/`, not shipped. It is how the store is filled
and inspected while there is no operator-facing import.

```text
gl-qso [--store <PATH>] <COMMAND>

  path                      where the store is
  keys                      the keys this build names, and the groups
  lookup <CALLSIGN>         [--format table|json]
  set <CALLSIGN> <K=V>...   writes at Origin::Manual
  unset <CALLSIGN> <KEY>...
  remove <CALLSIGN>         [--yes]
  list                      [--prefix <TEXT>] [--limit <N>]
  import <FILE.adi>...      [--encoding <utf-8|shift_jis|…>] [--dry-run]
```

A lookup that finds nothing exits 3, which clap's own 2 for a misused command line leaves
free.

## In the SSTV application

`grayline-qso::ContactWorker` owns the request channel, directory and published
snapshot for both desktop applications. `ContactPaths::open` opens the named
store; an absent path keeps tests in memory. The worker accepts the opened
directory, a thread name and a notification callback, without depending on
egui or application error types. Snapshot errors are shared `QsoError` values.
`ContactEditor` wraps it for an interface; each application hands that a wake
function.

The worker is kept for the session and writes corrected records off the thread
that draws the interface. Dropping it drains queued requests and checkpoints
the store before joining the thread.

A lookup is asked for when the callsign is **committed**, which is leaving the
field, and again when an FSK identifier arrives — the identifier path writes the
field directly and does not pass through the commit. Both go through one guard
that remembers the callsign last asked about, because the operator leaves the
field whether or not they changed anything and tabbing through the panel would
otherwise ask about the same station on every pass. Requests already queued
behind a slow write are collapsed to the last one: every answer but the
last would be about a station the operator has moved on from. A save is never
collapsed away, because it is what they asked to have written.

A failure goes to the status line rather than in front of the interface. Losing
the audio device stops reception outright and is worth a modal; a store that could
not be read loses nothing — the callsign is still typed, the template still composes,
and the transmission still goes out.

The dialog opens from a button beside the callsign in the QSO panel rather than
from the Settings menu, because the record is about the station on the air right
now and that panel is what the station on the air is worked from, while Settings
holds what is set once and then left alone. It offers the fields the settings
asked for, in the order they were written and whether or not this station has
any of them, so the dialog says what this operator files rather than only what
this contact happens to have; anything else the directory holds follows with its
name laid open for editing. A field the settings named that the build has no
label for stands under its own name, because a key the operator invented is one
no catalogue was ever going to know. A field the operator empties is dropped
rather than left alone: clearing a value is how a wrong one is taken back, and a
write that only ever added would leave it filed.

## In the RTTY application

RTTY uses the same `grayline-qso::ContactWorker` and `ContactSettings` as
SSTV, and the same `ContactEditor` around them: the lookup that does not ask
twice, the dialog's rows, and what a commit files and drops. Each application hands it the callsign
field it has. The dialog's widgets and the defaults remain application-owned,
including the field list a first run offers.
What differs downstream of the answer follows from ITA2 having no kanji.

**The default field list is `["!core", "!latin"]` rather than `["!core"]`.**
`latin` exists for this mode, and a station that files only a kanji name has
nothing a macro could put on the air; the dialog has to offer somewhere to put
the spelling that can be sent.

**An answer fills the panel in.** SSTV keeps the directory beside the QSO
panel and lets the template read it; RTTY's `contact.name` and `contact.qth`
are fields the operator types, so an answer that stayed in the directory would
leave the operator typing a name the application already had. It is written
into the fields they left empty, and only those: what is in the panel is what
they heard on the air. `name_latin` is preferred to `name` and `qth_latin` to
`qth`, and a value the transmitter has no code for is passed over rather than
written into a field that would then hold the send button down — the record
keeps it, and the SSTV application still prints it. While the logger's entry
form is followed, what is written goes to the logger as well, so a name the
store knows fills the logger's empty field rather than being typed there
([logger-link.md](logger-link.md)).

**The dialog runs no keyboard filter**, unlike every other field here whose
contents can reach the air. The store is the family's rather than this
application's: a name in kanji is a good entry, and refusing to type one would
file a worse record for the sake of a macro that reads the Latin spelling
instead.

A lookup is asked for when the callsign is committed, which is leaving any of
the contact fields, and again when a callsign is double-clicked out of the
received text, which does not pass through the field. The guard, the collapse,
and the failure going to the status bar rather than in front of the interface
are all as above.

## Open questions

**Portable callsigns.** `JA1ABC/1` is a different key from `JA1ABC` today.
[../mmsstv/jasta.md](../mmsstv/jasta.md) records MMJASTA's `ClipCC()` — reduce a
callsign to the segment carrying a digit, falling back to the one that is
neither `Q`-initial nor `MM`. Whether the two should be one entry changes what a key means, so it
should be settled deliberately rather than patched in.

**Getting a log in without the tool.** `gl-qso` is not shipped, so an operator
who does not build from source fills the directory by hand. A
File menu entry would be the follow-up, and it would be the first native file
dialog in any of these applications.

**Privacy.** The store accumulates other operators' names, addresses and email
addresses, unencrypted, in a file that follows the account between machines. The
manual has to say where it is and how to remove an entry.
