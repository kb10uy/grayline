# Grayline

Amateur radio digital mode applications in Rust, over a shared signal
processing and platform core.

Each mode ships as its own application rather than as one multimode program,
because what a mode needs on screen follows from the kind of signal it carries:
an image mode wants a raster and a receive library, a text mode wants a
waterfall and a transmit buffer. The core underneath them is shared; the
interfaces are not.

## Applications

| Application | Directory | Status |
| --- | --- | --- |
| [Grayline SSTV](apps/sstv/README.md) | `apps/sstv/` | Released |
| Grayline WEFAX | `apps/wefax/` | Receive implemented |
| Grayline RTTY | `apps/rtty/` | Live receive and buffered AFSK transmit implemented |
| Grayline PSK | — | Planned |

[apps/web-demo/](apps/web-demo/) builds the SSTV receive path for WebAssembly,
running at <https://rssstv.kb10uy.dev/>.

## Layout

- `apps/` — one directory per application, plus the browser demo.
- `crates/` — the libraries. Directory names carry no prefix; the packages
  they hold are named `grayline-*`.
- `tools/` — development command-line tools that are not shipped. One
  directory per mode, named `<mode>-cli`, holding the package
  `grayline-<mode>-cli` and the binary `gl-<mode>`. `tools/qso-cli/`
  provides `gl-qso` for the shared contact directory and ADIF import.
- `assets/` — data shipped outside any one crate, such as the ported MMSSTV
  templates under `assets/templates/`.
- `docs/memo/` — development documentation, divided by subject and indexed by
  [docs/memo/README.md](docs/memo/README.md).
- `docs/reference/mmsstv/` — the original MMSSTV source, a submodule kept as
  the behavioral reference.
- `docs/reference/mmtty/` — the original MMTTY source, a submodule kept as the
  behavioral reference for RTTY.

The shared libraries are `dsp`, `audio`, `rig`, `shell`, `qso`, and
`variables`. The protocol crates carry their mode's name. `tone-tx` converts
SSTV timed-tone events to PCM and still depends on `grayline-sstv`. `shell` is what an application is built out of before
it knows which signal it carries: the window, the platform integration, the
message lookup, and the log.
`crates/sstv-rx` still holds the SSTV receive front end whole; the parts of it
that are not specific to SSTV move down into the core as the second mode needs
them.

## Building

```text
cargo build --workspace
cargo test --workspace
```

## License

LGPL-3.0-or-later. See [LICENSE](LICENSE).

## Special Thanks

- [mm-open.org](http://mm-open.org)
- *Mako* JE3HHT
