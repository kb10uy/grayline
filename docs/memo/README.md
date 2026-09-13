# Grayline Development Documentation

The documentation is divided by what each document is about, because the three
subjects answer to different authorities. A protocol description answers to the
on-air signal, a description of MMSSTV answers to its source, and a description
of Grayline answers to this repository's code.

None of it is written for the operator. The manual is, and it is published
outside this repository, at <https://grayline.jl1his.radio/sstv/>,
<https://grayline.jl1his.radio/wefax/>, and
<https://grayline.jl1his.radio/rtty/>, which each application's Help menu
opens.

## `sstv/` — the protocols

SSTV as it exists on the air, independent of any one implementation. Values
here are largely derived from the MMSSTV source, which remains the most
complete reference for modes that have no formal standard, but the subject is
the signal rather than the program.

- [sstv/modes.md](sstv/modes.md): modes, geometry, timing, and VIS
  identification.
- [sstv/fskid.md](sstv/fskid.md): the FSK station-identification protocol,
  including the callsign, contest, and narrow N-VIS records.

## `wefax/` — the protocols

WEFAX as it exists on the air. Unlike SSTV it is not an amateur mode and has no
reference implementation this project follows, so these values answer to the
published standards rather than to anyone's source.

- [wefax/protocol.md](wefax/protocol.md): modulation, index of cooperation,
  line rates, the framing tones, and the phasing signal.

## `rtty/` — the protocols

RTTY as it exists on the air. As with SSTV, the values are largely derived from
the reference implementation — MMTTY — but the subject is the signal.

- [rtty/protocol.md](rtty/protocol.md): modulation, shift and speed, start-stop
  framing, the ITA2 character code, and the unshift-on-space convention.

## `mmsstv/` — the original implementation

The behavior of the original MMSSTV source in `docs/reference/mmsstv`, which this
project treats as the reference implementation. These documents describe what
that program does, including where it departs from published descriptions.

- [mmsstv/architecture.md](mmsstv/architecture.md): the application's
  structure, state, and data flow.
- [mmsstv/dsp.md](mmsstv/dsp.md): filters, discriminators, synchronization, and
  clock correction.
- [mmsstv/modes.md](mmsstv/modes.md): how its mode table is written, and where
  it differs from public tables.
- [mmsstv/fskid.md](mmsstv/fskid.md): its FSKID detector and acquisition.
- [mmsstv/jasta.md](mmsstv/jasta.md): MMJASTA, the contest scorer bundled with
  the source, and the MDT log format it reads.
- [mmsstv/porting.md](mmsstv/porting.md): reading the original source for the
  Rust port.

## `mmtty/` — the original implementation

The behavior of the original MMTTY source in `docs/reference/mmtty`, which this
project treats as the reference implementation for RTTY. MMTTY shares an author
and much of its support code with MMSSTV, so these documents note where the two
agree and where they part.

- [mmtty/architecture.md](mmtty/architecture.md): the application's structure,
  state, threads, transmit paths, and external interfaces.
- [mmtty/dsp.md](mmtty/dsp.md): the four demodulators, the limiter, the
  integrators, ATC, the prefilters, AFC, and the modulator.
- [mmtty/framing.md](mmtty/framing.md): the start-stop framing state machines,
  the Baudot conversion tables, diddle, and the CW identifier.
- [mmtty/porting.md](mmtty/porting.md): reading the original source for the
  Rust port.

## `grayline/` — this project

The Rust implementation: what it is meant to be, and what it currently is.

- [grayline/architecture.md](grayline/architecture.md): target architecture and the
  current crate structure.
- [grayline/gui-design.md](grayline/gui-design.md): the desktop application, its
  audio boundary, and its platform integration.
- [grayline/wefax.md](grayline/wefax.md): the WEFAX receive crate, what it took
  from the core on its way in, and where it deliberately parts company with the
  SSTV receive contract.
- [grayline/rtty.md](grayline/rtty.md): the RTTY crate, what it lifted into the
  core, and where it deliberately parts company with MMTTY — including that
  nothing is decimated.
- [grayline/rtty-impl-plan.md](grayline/rtty-impl-plan.md): the RTTY desktop
  application design, implementation status, and remaining work.
- [grayline/rig-control.md](grayline/rig-control.md): the transports the rig is
  reached over, the script that decides what is sent, and the band plan both
  read, including the implemented SSTV Lua host and radio panel.
- [grayline/qso-directory.md](grayline/qso-directory.md): the directory of
  stations a template looks a callsign up in, the store behind it, and why it
  is not a log.
- [grayline/template-design.md](grayline/template-design.md): the portable transmit
  overlay format.
- [grayline/web-demo.md](grayline/web-demo.md): the browser build of the receive
  path, what the page has to do around it, and what the audio APIs do to the
  samples on the way in.
- [grayline/release.md](grayline/release.md): what CI checks, and how a tag becomes
  a release.
