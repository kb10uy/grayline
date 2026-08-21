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
before, and because the RTTY macros planned in
[rtty-impl-plan.md](rtty-impl-plan.md) want the same answer in `{name}` form. It
is reached through `grayline-qso`, which belongs to no one application: it is
the second thing in this family that both applications share, after the shell.

## The record

A record is a callsign and a table of fields under it. A callsign is trimmed and
uppercased on the way in, and text that is not a callsign is refused rather than
stored — three to sixteen characters of `[A-Z0-9/]` holding at least one digit
and at least one letter. Refusing rather than accepting whatever arrived is what
keeps a half-typed field and a garbled FSK identifier from becoming a question
put to somebody's logger.

A key is one identifier segment: a letter or underscore, then letters, digits
and underscores. The rule is not arbitrary. A key is read from a template as
`${contact.<key>}`, so a key holding a hyphen or a dot would be filed perfectly
well and then be unreachable from the only place it is wanted. `grayline-qso`
restates the rule that `grayline_sstv_template::valid_variable_name` applies to
each segment rather than depending on that crate, which would drag the SVG stack
into a command-line tool; the SSTV application is where both are in view, and it
carries the test that holds them to each other. The key `callsign` is refused
outright, so a record can never shadow the field the operator typed.

### The keys this crate names

Naming a key, filling it in, and putting a field on screen for it are three
different questions, and running them together is what produced a dialog of
fourteen rows nobody wanted. The keys are named here so that the same fact
arrives under the same name whichever source it came from. Which of them an
application shows is settled separately, by the operator; see
[the fields a station uses](#the-fields-a-station-uses).

| Key | Wavelog field | ADIF field |
| --- | --- | --- |
| `name` | `name` | `NAME` |
| `name_latin` | — | — |
| `qth` | `location` | `QTH` |
| `qth_latin` | — | — |
| `grid` | `gridsquare` | `GRIDSQUARE` |
| `jcc` | — | — |
| `dxcc` | `dxcc` (the entity name) | `COUNTRY` |
| `dxcc_id` | `dxcc_id` | `DXCC` |
| `cq_zone` | `dxcc_cqz` | `CQZ` |
| `itu_zone` | — | `ITUZ` |
| `continent` | `cont` | `CONT` |
| `state` | `state` | `STATE` |
| `county` | `us_county` | `CNTY` |
| `iota` | `iota_ref` | `IOTA` |
| `qsl_manager` | `qsl_manager` | `QSL_VIA` |
| `email` | — | `EMAIL` |
| `note` | — | `COMMENT`, else `NOTES` |

Four of them no source fills in, and they are named anyway because a key that
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

Everything an upstream source says about *contacts* is dropped on the way in.
From Wavelog that is `call_worked`, `call_confirmed`, `dxcc_confirmed` and
`lotw_member`; from ADIF it is `BAND`, `MODE`, `FREQ`, the reports and the QSL
fields. `bearing`, `dxcc_lat` and `dxcc_long` are dropped for a different
reason: the bearing is computed from the asking station's own grid rather than
being a property of the station asked about, and the latitude and longitude give
the DXCC entity's centroid, which would contradict a real QTH printed beside
them. The operator's own side of a logged contact — `OPERATOR`,
`STATION_CALLSIGN`, everything under `MY_` — describes the station reading the
file rather than the one it is filed under.

### The fields a station uses

What an operator files depends on where they operate and on what they work.
A station in Japan wants a JCC code and a name RTTY can send; a station anywhere
else wants neither and would rather not scroll past them. Nothing here can
decide which of those an operator is, so nothing here tries: the field list is
written in the settings, out of keys and of groups spelled `!name`.

```toml
[contact]
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
default: three fields, which is what a contact is worth looking up for at all.
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
key set is open. `station` carries the lookup history rather than a field,
because a station a lookup found nothing for is still one the store has to
remember asking about. Timestamps are RFC 3339 UTC text written with `jiff`,
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

Every field records where it came from, and the three sources are ordered:
`adif` < `wavelog` < `manual`. A write happens only when the origin doing the
writing is at least as strong as the one that wrote what is there. So an import
or a lookup fills in what is missing and refreshes what another import or lookup
put there, and neither can displace a value the operator typed. An origin the
file holds that the build does not recognize is read as the strongest there is:
a newer build wrote it, and guessing it weaker would be a way to lose it.

Dropping a field is its own operation rather than re-filing the record that is
left, because re-filing would stamp every surviving field with the origin doing
it and quietly turn a fetched value into one no later lookup will ever refresh.

## The lookup

The store first. The logger only when the store has nothing recent enough to
say: a callsign that has never been asked about and has nothing filed under it,
or one whose last answer is older than thirty days, or whose last answer of
"nothing" is older than a day. Thirty days because what this keeps is what a
station *is* — a name does not change and a QTH changes on the scale of house
moves; a day for a miss because a station absent from the callbook last week may
be in it today, and a new licensee is exactly the operator whose details are
worth having.

A logger that refuses is reported beside what the store knew rather than instead
of it: a Wavelog that is down is no reason to forget what is already filed, and
the operator with no network is the one who most needs the cached answer. A
failure is also not recorded as having asked, because nothing was learned and
the next attempt should not be held off.

### Wavelog

`POST /api/private_lookup`, with the API key in the JSON body. Installations
differ on whether `index.php` is in the path, so the client tries
`{base}/api/private_lookup`, retries once on a 404 with
`{base}/index.php/api/private_lookup`, and remembers which answered.

`band` and `mode` are left out of the request. They decide only the worked and
confirmed flags, and this reads none of them.

The answer is read field by field out of a `serde_json::Value` rather than into
a derived type. The rule this project actually keeps is that *its own* persisted
formats are hand-mapped so a hand-edited file survives a save, which is why the
settings walk a `toml_edit` document key by key. Wavelog's answer is a foreign
document, read once, never written back, and with its unknown keys deliberately
discarded; reading it a field at a time is the same house style applied to a
second format rather than a departure from it. A field that is absent, null, or
empty is dropped rather than filed, because a stored empty value would read back
as an answer and would occupy the place a later real one should fill. An
instance that does not know a callsign answers with a success whose fields are
empty, so "found" means the mapped record holds something rather than that the
request succeeded.

An error carries the status and the host and never the request or the body: the
body holds the API key, and an error quoting the exchange would put it wherever
the interface prints errors.

### Cloudlog is not supported, and that is a decision

Cloudlog's rich `lookup()` endpoint authenticates with a session cookie rather
than with an API key, so it cannot be reached by a program that is not a browser
someone has logged into. The one endpoint an API key does open,
`logbook_check_callsign`, answers `{"callsign": …, "result": "Found"}` and
nothing else: no name, no grid, no QTH, nothing a template could print. What it
does answer is whether the station has been worked before, which is a fact about
a contact and precisely the kind of thing this directory exists not to keep.
Offering a source that can only answer a question the feature does not ask would
be worse than offering none.

### Where the key lives

Not in the application's settings file. That file is rewritten at the end of any
frame that changed a setting, by code that has no idea it is handling a
credential; it sits beside the rig script and the band plan; the File menu
offers to open it; and it is the first thing anyone pastes into a bug report. A
secret in a file whose whole design is "open this and read it" is a secret that
leaks through the routine action the project encourages.

It lives in `<config_dir>/Grayline/credentials.toml`, shared by the family,
created with mode `0600` where the platform has one, read and never written
back. `GRAYLINE_WAVELOG_KEY` overrides it, and `GRAYLINE_WAVELOG_URL` overrides
the address, which is what a station run from a script or a container uses —
the same arrangement `grayline_shell`'s manual URL already has. The settings
file carries the switch and the instance's address only:

```toml
[contact]
lookup = true

[contact.wavelog]
url = "https://log.example.org"
```

The address is written even while it is empty, for the same reason the rig ports
are: this file is where it is edited, and an operator who has to invent the key
before they can change it has no way to learn it exists.

Both paths are named by whoever builds the worker rather than discovered inside
it. A discovered path is one a test cannot escape: the suite would otherwise
write its store into the operator's own directory and read the operator's own
API key, and a headless application putting questions to a live logger is not
something a test run should be able to do.

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

  path                      where the store and the credentials file are
  keys                      the keys this build names, and the groups
  lookup <CALLSIGN>         [--remote] [--refresh] [--url <URL>] [--format table|json]
  set <CALLSIGN> <K=V>...   writes at Origin::Manual
  unset <CALLSIGN> <KEY>...
  remove <CALLSIGN>         [--yes]
  list                      [--prefix <TEXT>] [--limit <N>]
  import <FILE.adi>...      [--encoding <utf-8|shift_jis|…>] [--dry-run]
  credential set            [--url <URL>]
  credential clear
```

`credential set` reads the key from standard input and never from an argument:
an argument lands in the shell history and in the process list. It writes with
`create_new` and refuses to replace a file that is there, the way the SSTV
application refuses to replace a rig script it did not write. A lookup that
finds nothing exits 3, which clap's own 2 for a misused command line leaves
free.

## In the SSTV application

The worker in `apps/sstv/src/worker/contact.rs` follows the rig worker's shape —
a request channel, a snapshot read once per frame — with two differences. It
takes a `Waker`, because a lookup happens about once a contact and polling for
one would mean redrawing forever to catch it. And it is started once and kept
for the whole session, because it holds a store rather than a socket, works with
the network switched off, and is where a corrected record is written; turning
the lookup off takes the logger away from it rather than the directory.

A lookup is asked for when the callsign is **committed**, which is leaving the
field, and again when an FSK identifier arrives — the identifier path writes the
field directly and does not pass through the commit. Both go through one guard
that remembers the callsign last asked about, because the operator leaves the
field whether or not they changed anything and tabbing through the panel would
otherwise ask about the same station on every pass. Requests already queued
behind a slow round trip are collapsed to the last one: every answer but the
last would be about a station the operator has moved on from. A save is never
collapsed away, because it is what they asked to have written.

Four things together keep the traffic down to about one request per new station
per month: text that is not a callsign never leaves the process, only a change
of callsign asks, the store's own lookup history answers for the TTL, and the
queue collapses.

A failure goes to the status line rather than in front of the interface. Losing
the audio device stops reception outright and is worth a modal; a lookup that
failed loses nothing — the callsign is still typed, the template still composes,
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
write that only ever added would hand it straight back on the next lookup.

## Open questions

**Portable callsigns.** `JA1ABC/1` is a different key from `JA1ABC` today.
[../mmsstv/jasta.md](../mmsstv/jasta.md) records MMJASTA's `ClipCC()` — reduce a
callsign to the segment carrying a digit, falling back to the one that is
neither `Q`-initial nor `MM` — and Wavelog answers a `suffix_slash` for the same
reason. Whether the two should be one entry changes what a key means, so it
should be settled deliberately rather than patched in.

**Getting a log in without the tool.** `gl-qso` is not shipped, so an operator
who does not build from source fills the directory by lookup and by hand. A
File menu entry would be the follow-up, and it would be the first native file
dialog in any of these applications.

**Privacy.** The store accumulates other operators' names, addresses and email
addresses, unencrypted, in a file that follows the account between machines. The
manual has to say where it is and how to remove an entry.
