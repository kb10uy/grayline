# Repository Guidelines

## Overview

This project implements SSTV (Slow Scan Television) software for amateur radio
in Rust. The goal is to port the core behavior of MMSSTV while separating its
signal-processing and protocol logic from the original Win32/VCL application.

The repository is a monorepo holding one application per mode, over a shared
signal-processing and platform core. SSTV is the first; WEFAX, RTTY, and PSK
are planned.

The repository contains:

- `apps/`: one directory per shipped application, plus the browser demo.
  `apps/sstv/` is the SSTV desktop application and `apps/wefax/` the WEFAX
  one.
- `crates/`: the libraries. Directory names carry no prefix; the packages they
  hold are named `grayline-*`. `crates/shell/` holds what every application
  needs and no mode decides: platform integration, the localization machinery,
  and the log. An application supplies what makes it itself through
  `grayline_shell::Identity` and an `i18n::Catalog`.
- `tools/`: development command-line tools that are not shipped. One directory
  per mode, named `<mode>-cli`, holding the package `grayline-<mode>-cli` and
  the binary `gl-<mode>`; `gl-sstv` encodes and decodes WAV files and
  `gl-wefax` decodes them. Each binary parses its command line with clap's
  derive interface and reaches the work through its own library, so a
  subcommand stays a thin layer over a tested function.
- `assets/`: data the repository ships outside any one crate, such as the
  ported MMSSTV templates under `assets/templates/`.
- `docs/reference/mmsstv/`: the original MMSSTV source code, included as a Git
  submodule and used as the behavioral reference.
- `docs/memo/`: development documentation, divided by subject.
  `docs/memo/README.md` indexes it.
  - `docs/memo/sstv/`: the protocols themselves — modes, timing, VIS, and
    FSKID — independent of any one implementation.
  - `docs/memo/wefax/`: the WEFAX signal on the air — modulation, index of
    cooperation, line rates, framing tones, and the phasing signal.
  - `docs/memo/mmsstv/`: the behavior of the original application, including
    its DSP implementation and where it departs from published descriptions.
  - `docs/memo/grayline/`: this project — target architecture, the desktop
    application, and the transmit overlay format.

Put a new development document under the directory matching what it is about. A
protocol description answers to the on-air signal, a description of MMSSTV
answers to its source, and a description of this project answers to this
repository's code; a document that would answer to two of those belongs in two
documents.

Treat `docs/reference/mmsstv/` as reference material. Do not modify the submodule
unless the task explicitly requires changes to the original source.

## Architecture

The Rust implementation should keep the reusable SSTV core independent of UI,
audio backends, radio control, logging, and other platform integrations.

Use these conceptual boundaries as the implementation grows:

- `dsp`: FIR/IIR filters, FFT, Hilbert transforms, PLL, oscillators, and related
  numerical primitives.
- `sstv_modes`: mode definitions, image geometry, timing, VIS values, and shared
  protocol constants.
- `demodulator`: audio-to-frequency demodulation, sync detection, VIS/FSK mode
  detection, AFC, and receive state.
- `rx_decoder`: raster synchronization and conversion of demodulated samples to
  image pixels.
- `tx_encoder`: conversion of images to timed frequency sequences, including
  headers, VIS, scan lines, and identifiers.
- `modulator`: conversion of timed frequency values to PCM samples.
- `audio`: platform-specific audio input and output adapters.
- `image`: owned image buffers and RGB/luminance/chroma conversion.
- `application`: UI, history, templates, logging, PTT, CAT, and orchestration.

The intended data flow is:

```text
Receive:
AudioSource -> Preprocessor -> Demodulator -> RxDecoder -> ImageSink

Transmit:
ImageSource -> TxEncoder -> Modulator -> AudioSink
```

Keep core modules deterministic and testable with in-memory samples and images.
Platform modules should depend on the core; the core must not depend on platform
or application code. Prefer explicit ownership and bounded queues over global
state or implicitly shared buffers.

When reproducing MMSSTV behavior, consult both `sstv.cpp` and `Main.cpp`. The
original receive scan conversion, transmit line generation, VIS generation, and
queue scheduling are partly embedded in the VCL main form rather than isolated
in the original DSP classes.

## Code Style

- Use Rust edition 2024.
- Follow standard Rust naming and formatting conventions.
- Use LF line endings for all text files. There is no `.gitattributes` to
  correct this after the fact, so every edit has to write LF itself. On Windows
  a tool that opens a file in the platform's text mode converts LF to CRLF
  without reporting it: Python's `pathlib.write_text` and `open(path, "w")`,
  PowerShell's `>` redirection, and `Set-Content` all do. Write bytes instead,
  or edit in place with a tool that preserves what is already there.
- Check `git diff --stat` before committing. A small edit that reports the
  whole file as changed has had its line endings rewritten, and the fix is to
  rewrite the file with LF rather than to commit it.
- Combine imports from the same crate into a single `use` statement within each
  module scope, except when different `cfg` attributes require separate imports.
- Avoid comments by default. Add comments only when explicitly requested by the
  user.
- Prefer the smallest correct implementation and avoid speculative abstractions.
- Model ownership and state transitions explicitly; avoid global mutable state.
- Keep platform-specific types and dependencies out of reusable core APIs.
- Prefer `modname.rs` and `modname/` style over `mod.rs` style.
- Use `rstest` features for parameterized tests, fixtures, and test cases where
  they improve coverage or reduce repetition.

## Documentation

- Write documentation under `docs/memo/` in English.
- The operator's manual is not in this repository. It is published per
  application at <https://grayline.jl1his.radio/sstv/> and
  <https://grayline.jl1his.radio/wefax/>, which each application's Help menu
  opens through `grayline_shell::manual_url`; the addresses are the
  `manual_url` field of each `identity::IDENTITY`.
- When a new implementation or fix changes behavior, APIs, architecture, mode
  support, limitations, or any other documented area, update the relevant
  documentation in the same change. A change to what the operator sees or does
  belongs in the published manual as well, which is a change made where that
  site is written rather than here.

## Build and Test

This repository uses a Cargo workspace. Run commands from the workspace root.

- Build all workspace members with `cargo build --workspace`.
- Run all tests with `cargo test --workspace`.
- Check that `grayline-sstv` and `grayline-wefax` still build without `std`
  using `cargo build -p grayline-sstv --no-default-features` and
  `cargo build -p grayline-wefax --no-default-features`. A workspace build does
  not cover this: another member enabling the `std` feature hides a core
  primitive used through `std` alone, so the crate can stop being `no_std`
  without any workspace command noticing.
- Check that the browser demo still builds for its own target using
  `cargo clippy -p grayline-web-demo --target wasm32-unknown-unknown`. A
  workspace build does not cover this either: on the host the JavaScript
  bindings compile to stubs nothing calls, so the crate can stop building for
  wasm without any workspace command noticing.
- Run Clippy with `cargo clippy --workspace --all-targets`.
- Run performance benchmarks with `cargo bench -p <crate>`. The core crates
  carry Criterion benches; compare against a stored baseline with
  `cargo bench -p <crate> -- --baseline <name>` and store one with
  `--save-baseline <name>`.
- Check formatting with `cargo fmt --all --check`.
- Apply formatting with `cargo fmt --all` when needed.

Before completing a code change, run at minimum:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build --workspace
cargo build -p grayline-sstv --no-default-features
cargo build -p grayline-wefax --no-default-features
cargo clippy -p grayline-web-demo --target wasm32-unknown-unknown
```

Add focused unit tests for DSP and protocol behavior. Prefer deterministic test
vectors and parameterized `rstest` cases for mode tables, timing values, color
conversion, and signal-processing edge cases.
