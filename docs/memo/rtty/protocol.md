# RTTY on the Air

This document describes amateur radio teleprinting — RTTY — as a signal,
independent of any one implementation. Most numeric values here are derived from
the MMTTY source in `docs/reference/mmtty`, which this project treats as the
reference implementation, but the subject is what appears on the air rather than
what that program does with it. MMTTY's own behavior is documented in
[mmtty/dsp.md](../mmtty/dsp.md) and [mmtty/framing.md](../mmtty/framing.md).

## Modulation

RTTY is binary frequency-shift keying. Two tones alternate: **mark** for a
logical one and idle line, **space** for a logical zero. The separation between
them is the **shift**; their arithmetic mean is the **center frequency**.

Two ways of producing the signal are in common use and are indistinguishable on
the air:

| Method | How the tones are made |
| --- | --- |
| FSK | The transmitter is keyed directly and shifts its carrier |
| AFSK | Two audio tones are fed to an SSB transmitter |

In AFSK the audio tones are usually 2125 Hz mark and 2295 Hz space, which is
what MMTTY defaults to (`Rtty.cpp:271`). On LSB these map to a mark that is
*higher* in RF than the space, which is the amateur convention; on USB the
sense inverts, and the receiver has to swap mark and space to decode. Software
calls that swap **reverse** or **Rev**.

Because the mark/space assignment is a convention rather than something carried
in the signal, a station receiving inverted text has no way to tell from the
data alone whether the transmitter or its own setting is wrong.

## Shift and Speed

Amateur RTTY uses a 170 Hz shift almost universally. MMTTY's automatic
frequency control recognizes four shifts as valid convergence targets, which is
a fair statement of what is found on the amateur bands (`Sound.cpp:692`):

| Shift | Use |
| --- | --- |
| 170 Hz | The amateur standard |
| 200 Hz | Occasionally used; MMTTY toggles between this and 170 |
| 220 Hz | Legacy equipment |
| 240 Hz | Legacy equipment |

Wider shifts — 425 Hz and 850 Hz — belong to commercial and military practice
and are not normally heard on the amateur bands. MMTTY accepts any shift from
10 Hz to just under 1500 Hz as a manual setting (`Main.cpp:3164`), and its AFC
will not accept a detected separation below 140 Hz or above 1500 Hz
(`Sound.cpp:626`).

The standard amateur speed is **45.45 baud**, which is 60 words per minute and
derives from the 22 ms bit period of the Teletype Model 15. MMTTY's speed
selector offers the following, which covers amateur and legacy commercial
practice (`Main.cpp:1854`):

```text
22, 45, 45.45, 50, 56, 75, 100, 110, 150, 200, 300
```

45.45 baud with a 170 Hz shift gives a modulation index of about 3.7, which is
why RTTY occupies roughly 250 Hz of spectrum despite its low data rate.

## Character Framing

RTTY is start–stop asynchronous, exactly as a wired teleprinter is. Each
character is:

```text
       +---------+----+----+----+----+----+---------+
 mark  |         |    |    |    |    |    | stop    |  mark (idle)
       |  start  | b1 | b2 | b3 | b4 | b5 |         |
 space +---------+----+----+----+----+----+
```

- One **start** bit, always space, one bit long.
- Five **data** bits, transmitted b1 first.
- One **stop** element, always mark, of 1, 1.5, or 2 bit lengths.

The 1.5-bit stop is what mechanical teleprinters produced and is what amateur
RTTY transmits. Receivers are usually more tolerant than transmitters: MMTTY
transmits 1.5 bits whenever anything but 1 or 2 is selected, while offering 1,
1.42, 1.5, and 2 bits as *receive* stop lengths, the 1.42 figure being an
empirical compromise that resynchronizes slightly early
(`Rtty.cpp:256`, `Rtty.cpp:656`).

A character therefore occupies 7.5 bit times, or 165 ms at 45.45 baud. There is
no parity, no framing beyond the stop element, and no error detection of any
kind. A missed start bit corrupts every following character until the receiver
happens to resynchronize on an idle period.

Between characters the line rests at mark. Two idle conventions exist:

- **Continuous mark**, an unmodulated carrier at the mark frequency.
- **Diddle**, in which the transmitter continuously sends LTRS (11111) instead.
  Diddle keeps the receiving decoder bit-synchronized and keeps a mechanical
  printer from drifting into figures case, at the cost of never letting the
  channel go quiet.

## The Character Code

RTTY carries ITA2 ("Baudot", though it is really Murray's code). Five bits give
32 combinations, which is not enough for letters and digits together, so the
code is split into two **cases** selected by two of the combinations:

- **LTRS** (11111) switches to the letters case.
- **FIGS** (11011) switches to the figures case.

Both are absolute, not momentary: once FIGS is received, every following
character is read from the figures column until LTRS arrives. LTRS is also the
natural idle character, so noise that fabricates one merely returns the
receiver to letters.

The table below is MMTTY's, read from `_LTR` and `_FIG` at `Rtty.cpp:1462`. The
bit column shows b1 first, which is transmission order.

| Bits | Letters | Figures | Bits | Letters | Figures |
| --- | --- | --- | --- | --- | --- |
| 00000 | NUL | NUL | 10000 | E | 3 |
| 00001 | T | 5 | 10001 | Z | " |
| 00010 | CR | CR | 10010 | D | $ |
| 00011 | O | 9 | 10011 | B | ? |
| 00100 | space | space | 10100 | S | BELL |
| 00101 | H | national | 10101 | Y | 6 |
| 00110 | N | , | 10110 | F | ! |
| 00111 | M | . | 10111 | X | / |
| 01000 | LF | LF | 11000 | A | - |
| 01001 | L | ) | 11001 | W | 2 |
| 01010 | R | 4 | 11010 | J | ' |
| 01011 | G | & | 11011 | FIGS | FIGS |
| 01100 | I | 8 | 11100 | U | 7 |
| 01101 | P | 0 | 11101 | Q | 1 |
| 01110 | C | : | 11110 | K | ( |
| 01111 | V | ; | 11111 | LTRS | LTRS |

Three figures positions are not fixed by ITA2 and vary by country. The table
above follows US teleprinter practice, which is what amateur RTTY uses: FIGS-F
is `!`, FIGS-G is `&`, and FIGS-H is `#`. MMTTY declines to emit `#` for
FIGS-H, printing a lowercase `h` instead, because `#` is one of its own macro
metacharacters.

BELL is the one position where amateur practice genuinely splits. In US
teleprinter code BELL sits at FIGS-S (10100) and FIGS-J (11010) is the
apostrophe; in the international ITA2 assignment BELL is at FIGS-J. MMTTY calls
the first arrangement **S-BELL** and the second **J-BELL** and makes it a
setting (`Rtty.cpp:1486`). A station using the wrong one prints an apostrophe
where a bell was meant, or rings where an apostrophe was sent.

Line endings are CR (00010) followed by LF (01000), in that order, because that
is the order in which a teleprinter's carriage and platen must move.

## Unshift On Space

Because a corrupted LTRS leaves the receiver stuck in figures case for the rest
of the transmission, receivers commonly implement **UOS**: a received space
character also returns the decoder to letters case. This is a receive-side
heuristic and costs nothing when the sender was in letters already, but it
mangles the one thing that legitimately contains spaces inside figures case —
a group of digits separated by spaces.

The transmitting station can cooperate by re-sending FIGS after every space
while in figures case, so that a UOS receiver stays in step. MMTTY calls this
**TX UOS** and implements it by marking its shift state as unknown after a
space, which forces the next character to carry a fresh shift
(`Rtty.cpp:1581`). A related transmit option sends every shift character twice,
so that losing one to noise does not lose the case change.

Neither measure is negotiated. Both are guesses about what the other station
does.

## Station Identification

RTTY carries no in-band station identifier. Where an identifier is required
outside the printed text, the usual method is to key the carrier on and off in
Morse — the transmitter stops modulating and the mark tone itself becomes the
CW element. MMTTY builds these from a run of mark-on and carrier-off commands
of three bit periods each, giving a Morse dot of about 66 ms at 45.45 baud
(`Main.cpp:4077`).
