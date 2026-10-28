# The Logger Link

## What it is

The operator logs contacts in their own logger, and the contact directory
([qso-directory.md](qso-directory.md)) deliberately keeps no log of its own.
What was missing was the way across: the callsign an FSK identifier delivered,
or the report typed into the panel, had to be typed a second time into the
logger. `grayline-logger-link` sends them there instead, and brings the
logger's form back so that the application works from it rather than from a
form of its own.

The logger is jl1his-logger's desktop client. It takes datagrams from other
programs on the same machine on the UDP addresses its settings list as
`[[listeners]]`, and a JSON datagram of its own protocol fills its entry form
field by field:

```json
{"v": 1, "type": "set", "fields": {"CALL": "JA1ABC", "RST_RCVD": "599"}}
```

Fields are named as ADIF names them, a field the message does not name is
left as it is, and an empty value clears its field. The protocol is described
in jl1his-logger's own `docs/guide/protocol.md`. Of what fills the form in,
only `set` is sent from here. `clear` would clear the whole form and `log`
would log it, and both are the operator's to do in the logger.

## What is sent

| Field | SSTV | RTTY |
| --- | --- | --- |
| `CALL` | The QSO panel's callsign | The contact's callsign |
| `RST_SENT` | The report being sent | The report being sent |
| `RST_RCVD` | The RSV received | The report received |
| `SRX_STRING` | The contest number received | — |
| `NAME` | — | The contact's name |
| `QTH` | — | The contact's QTH |

**The frequency and the mode are not sent.** The logger reads both from
`rigctld` itself, and a second source for them would only be a second answer
that could disagree. SSTV sends no name or QTH because its panel has none:
the directory answers for the template, and what it holds was filed once
rather than heard on the air for this contact.

SSTV's panel holds the received report and the contest number as one field,
`595012`, the way MMSSTV writes them and the way an identifier's number is
filled in. They are sent apart: the first three characters, when they are
digits, as the RSV, and what follows as the received exchange, which is what
the logger's form keeps it as. The serial number being sent is not sent at
all. The logger counts its own for the contest it is set up for, and writes
the sent exchange from it.

## When it is sent

Where a field is settled, never on a keystroke, so the logger is not handed
half a callsign:

- leaving a field of the panel;
- an FSK identifier or a contest number arriving, in SSTV;
- a callsign double-clicked out of the received text, the panel cleared, and a
  directory answer filling empty fields in, in RTTY;
- the sending being switched on, which sends what the panel already holds.

**Only what changed since it was last sent goes out.** A field corrected in
the logger is the operator's until the application's own value for it changes
again, rather than being written back over on every pass through the panel.
A field never sent counts as having been sent empty, so a panel field that was
never filled does not clear what was typed into the logger directly; one that
was sent and then emptied, as clearing the panel empties it, is cleared there
too. A datagram that could not be sent is not counted as sent and goes out with
the next change.

## Following the logger's form

The link also subscribes to the form, with `{"v": 1, "type": "subscribe"}`
from the socket it sends through, at once and every 5 seconds after. A logger
whose listener shares the form (Share the form, `subscribe` in its settings)
answers each one with the whole form, empty fields included, and sends it again
whenever the form settles on new values:

```json
{"v": 1, "type": "form", "fields": {"CALL": "JA1ABC", "NAME": "", "QTH": "", "RST_RCVD": "595", "RST_SENT": "595", "SRX_STRING": "012", "STX_STRING": "013"}}
```

A thread of the link's own waits for it and wakes the interface when the form
changes, so the interface is not drawn every few seconds to find out. Only a
datagram from the address the link sends to is read. The logger only takes a
subscription from the loopback address, so a logger on another machine never
shares its form, and the link goes on only sending to it.

**While the logger shares its form, the application has none.** The logger's
form is the one the contact is logged from, and a second form beside it would
be a second place to type the contact and a second answer that could
disagree. The QSO panel in SSTV and the contact fields in RTTY give way to the
callsign alone, which can be read and copied but not edited, the contact
directory's button beside it, and a note that the logger's form is being
followed. Each form that arrives is copied into the fields the templates and
the macros are written from:

| Field | SSTV | RTTY |
| --- | --- | --- |
| `CALL` | The callsign | The callsign |
| `RST_SENT` | The report being sent | The report being sent |
| `RST_RCVD` and `SRX_STRING` | The received report, joined as `595012` | `RST_RCVD` as the report received |
| `STX_STRING` | The serial being sent, when the logger counts one | — |
| `NAME`, `QTH` | — | The contact's name and QTH |

What still goes to the logger is what the application hears on the air: an
identifier and its number in SSTV, a callsign double-clicked out of the
received text in RTTY, and a name or QTH the contact directory knows for a
field that is empty. They go out as `set`, as they always did, and come back in
the next form. A form's fields count as sent, so nothing the form said is sent
back to it.

SSTV joins the received report and exchange into the one field its panel holds,
and a field the logger holds as an exchange alone, `012`, would be read back as
a report if it were split again. So the pair is kept as the logger sent it,
and sent as that pair for as long as the field still says what the logger
said.

**The logger is lost when it has not answered for 15 seconds**, or at once when
it sends `{"v": 1, "type": "bye"}` on quitting, on its listeners being changed,
or on signing out. The panel comes back holding what the logger's form last
held, so a contact half worked when the logger went is still there to finish.
When the logger is found again its form wins, except that a field it has
nothing for keeps what the panel holds and is sent to it: the panel was the
form while the logger was away, and what was typed into it then is the only
copy there is. Switching the link off unsubscribes with
`{"v": 1, "type": "unsubscribe"}`.

## Settings

```toml
[logger]
enabled = false
address = "127.0.0.1:2333"
```

The same section in both applications, so a station set up for one is set up
for the other by copying it across. The switch is also in the Settings menu.
It starts off, because the logger's listeners start off too, and a datagram
sent to a port nobody listens on is one more thing the operator did not ask
for. The default address is the logger's second default listener: its first,
`127.0.0.1:2237`, is WSJT-X's port.

There is no switch of its own for following the form. Whether the logger shares
it is decided in the logger, for each listener, and a subscription the logger
does not share is ignored there.

The address is an IP address and a port, not a host name. A name would be
looked up on the thread that draws the interface, and the logger is on the
same machine or on one whose address the operator knows. The socket is bound to
the loopback address when the logger is on it, so a link to the same machine
never listens where another one can reach it. A bad address and a failed send
go to the status line rather than in front of the interface: a contact that
did not reach the logger loses nothing that is not still on screen.

The headless applications the test suite builds send to a socket of the
suite's own rather than to the default address, because applying every menu
action switches the sending on, and the operator may have the logger running
and sharing its form.
