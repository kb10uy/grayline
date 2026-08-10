# WEFAX

WEFAX, or radiofax, is how meteorological services broadcast weather charts and
satellite imagery on high frequency. This document describes the signal on the
air. What Grayline does with it is
[grayline/wefax.md](../grayline/wefax.md).

Unlike SSTV, WEFAX is not an amateur mode: the transmissions come from
government services, and a receiving station never sends one. There is
consequently no reference implementation to be compatible with in the way the
SSTV documents are compatible with MMSSTV, and no counterpart to VIS. What
identifies a transmission's geometry is carried in one tone at the start of it,
and only half of the geometry at that.

## Modulation

The picture is a frequency-modulated subcarrier, received through a
single-sideband receiver as audio. The gray scale runs linearly between two
frequencies:

| | Black | Center | White | Shift |
| --- | --- | --- | --- | --- |
| Standard | 1500 Hz | 1900 Hz | 2300 Hz | ±400 Hz |
| Narrow | 1750 Hz | 1900 Hz | 2050 Hz | ±150 Hz |

Almost everything on the air uses the standard shift. The narrow variant exists
and costs nothing to support, but it cannot be verified against real signals
here.

Because the picture is carried by frequency alone, a tuning error moves the
whole gray scale. An operator tunes by ear or by a level indicator rather than
by anything in the signal.

## Geometry

Two numbers describe the raster, and they are announced separately — or rather,
one is announced and the other is not.

**The index of cooperation** fixes how many pixels are in a line. It is the
product of the drum diameter and the line density of the mechanical machines
the format was designed for, and the pixels in a line are the index times pi:

| Index | Pixels per line |
| --- | --- |
| 576 | 1810 |
| 288 | 905 |

`576 × π` is 1809.557 and `288 × π` is 904.779. Rounding to nearest gives the
figures above; software that truncates uses 1809 and 904 instead. The
difference is a fraction of a pixel of slant across a whole picture.

**The line rate** fixes how long a line takes. Six rates are in use:

| Lines per minute | Line period |
| --- | --- |
| 60 | 1000 ms |
| 90 | 667 ms |
| 100 | 600 ms |
| 120 | 500 ms |
| 180 | 333 ms |
| 240 | 250 ms |

Marine and meteorological charts are almost always IOC 576 at 120 lines per
minute. Nothing in the transmission announces the line rate; see
[Automatic picture transmission](#automatic-picture-transmission).

At the two capture rates a sound card offers, every line rate is a whole number
of samples:

| Lines per minute | 48 000 Hz | 44 100 Hz |
| --- | --- | --- |
| 60 | 48 000 | 44 100 |
| 90 | 32 000 | 29 400 |
| 100 | 28 800 | 26 460 |
| 120 | 24 000 | 22 050 |
| 180 | 16 000 | 14 700 |
| 240 | 12 000 | 11 025 |

Other rates need not divide evenly — 11 025 Hz at 120 lines per minute is
5512.5 samples — but the length is always exact in binary, which matters for a
clock that has to stay honest over thousands of lines.

At 48 000 Hz, IOC 576 at 120 lines per minute is 13.26 samples per pixel. At
the low end of what a capture device might offer, 8000 Hz with IOC 576 at 240
lines per minute is 1.1 samples per pixel: correct, but soft.

## Automatic picture transmission

A transmission is framed by tones that are not tones in the audio sense. Each
is the picture signal itself keyed between black and white at a fixed rate, so
on the air it is a carrier hopping between 1500 and 2300 Hz, and after
demodulation it is a square wave at that rate around 1900 Hz.

| Tone | Rate | Length | Meaning |
| --- | --- | --- | --- |
| Start | 300 Hz | 5 s | The transmission that follows is IOC 576 |
| Start | 675 Hz | 5 s | The transmission that follows is IOC 288 |
| Stop | 450 Hz | 5 s | The transmission has ended |

The start tone therefore announces the index of cooperation and **nothing
else**. A receiver that wants the line rate has to be told, or has to work it
out from the phasing signal.

Ten seconds of black conventionally follow the stop tone.

## Phasing signal

Between the start tone and the picture, the transmitter sends about thirty
seconds of phasing lines. Each is black for the whole line except for a white
pulse occupying five percent of it. The pulse is what tells a receiver where a
line begins; without it a picture decodes correctly but rolled sideways by an
arbitrary amount.

Thirty seconds is nominal. Services send more or less of it, and a receiver
that ends the phasing signal on a stopwatch rather than on the picture starting
will lose the top of some charts and decode the bottom of the phasing signal
into others.

## Reception in practice

- A transmission runs for ten to twenty minutes. A capture clock that is 100
  parts per million off — ordinary for a sound card — accumulates 120 ms over
  twenty minutes, which is 434 pixels of shear across an IOC 576 picture. Rate
  correction is not optional.
- There is no per-line synchronization pulse. Once the phasing signal has
  ended, the only timing reference is the line rate itself.
- Sideband selection and some services invert the sense of the gray scale, so
  black and white swapping over is an operator control rather than a fault.
- Charts carry large uniform white areas, which is what makes correcting the
  rate from the picture harder than it sounds.

## References

- WMO, *Manual on Marine Meteorological Services* (WMO-No. 558), which defines
  the index of cooperation, the line rates, and the framing tones.
- ITU-R Recommendation M.1170, on radiotelegraph and facsimile procedures.
