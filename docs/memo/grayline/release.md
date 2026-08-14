# Continuous Integration and Releases

Five workflows in `.github/workflows/` cover the repository: `ci.yml` checks
every change, `release-sstv.yml` and `release-wefax.yml` publish what a tag
names by calling the shared `release-app.yml`, and `deploy.yml` publishes the
browser demo.

## CI

`ci.yml` runs on pushes to `master` and on pull requests, in six jobs that
between them run the commands `AGENTS.md` requires before a change is
complete. The jobs are split along the lines the work actually divides on,
which is what lets them run at the same time rather than one after another.

`format` runs `cargo fmt --all --check` and nothing else. It needs neither the
system packages nor the compiled dependencies the other jobs wait for, so a
misformatted change is reported in under a minute instead of behind a build.

`core` runs Clippy with warnings denied, the tests, and a build over the
workspace with the two applications excluded, followed by the `no_std` builds.
The exclusion list is named once in the job's environment and reused by the
three commands, so a member that leaves the core cannot be dropped from one of
them alone. The browser demo stays in this job: its host build is cheap,
because it depends on the SSTV crates and `wasm-bindgen` and not on the
graphics stack, and the `wasm` job below is what actually holds it to its own
target.

`app` is a matrix over `grayline-sstv-app` and `grayline-wefax-app`, running
Clippy, the tests, and a build for one package each. Each application is a long
tail of its own; running the three beside each other trades runner minutes for
wall clock, and a failure names which application broke rather than which
command did.
Each matrix leg keys its own cache: the two dependency graphs meet at eframe —
which `crates/shell` already pulls in, so the core compiles it too — and
diverge after it, and one key over both would have each run overwriting the
other's entry.

These three run on Linux alone. That is a deliberate asymmetry rather than full
coverage: Linux selects `platform/other.rs` and the in-window menu bar, which
is the configuration least likely to be exercised during development on
Windows, so CI covers the side the author does not. macOS compiles nowhere
until a release builds it, and `platform/macos.rs` is the code this leaves
unchecked.

Only ALSA needs a system package to build. The window, the graphics context,
and the Wayland and X11 clients are all opened at run time rather than linked,
so `libasound2-dev` is the whole list. `mold` is beside it because
`.cargo/config.toml` names it as the linker for every Linux build of this tree.

`licenses` checks the dependency graph rather than the code: `cargo deny check
licenses` against `deny.toml`, `cargo deny check advisories` against the
RustSec database, and `cargo about generate` against `about.toml` and
`about.hbs`. The last one is there because the release archives carry that
page, and a template that cannot produce it should fail on the change that
broke it rather than during a release.

An advisory published against a dependency turns CI red on changes that have
nothing to do with it. That is the intended behavior: the alternative is
learning about it when a release is already being cut.

`wasm` builds `apps/web-demo` for `wasm32-unknown-unknown` and runs Clippy against
that target, then builds the page with `wasm-pack`. It is separate for the
reason the no-std steps are: the host build compiles the JavaScript bindings to
stubs nothing calls, so it proves nothing about the target the demo ships to.
Building the page rather than only the crate is what keeps a broken deploy from
reaching `master`.

## Deploying the demo

`deploy.yml` runs on pushes to `master`, builds the WebAssembly module into
`apps/web-demo/www`, and uploads that directory to Cloudflare Workers with
`wrangler`. It needs two repository secrets, `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`; the token needs permission to edit Workers scripts.

The build happens here rather than on Cloudflare because Cloudflare's build
image carries Node, Python, Go, and Ruby and no Rust. Connecting the repository
to Workers Builds would mean installing a toolchain on every deploy before
anything of this project is compiled, which is a worse trade than uploading a
directory that has already been built.

The demo is an assets-only Worker: `apps/web-demo/wrangler.toml` names a directory
and no `main`, because the page decodes in the browser and there is nothing for
a Worker script to do. It is still a Worker rather than a Pages project, which
leaves room to put a script beside the assets later without moving the site. The
same file claims `rssstv.kb10uy.dev` as a custom domain, which needs the zone to
be on Cloudflare and manages the record and the certificate itself.

Every path in the page is relative, which is what lets the same tree be served
from a local directory and from a Worker subdomain without rewriting.

## Releases

Each application is released on its own, under a tag that names which one:

```text
git tag sstv-v0.3.1
git push origin sstv-v0.3.1

git tag wefax-v0.1.0
git push origin wefax-v0.1.0
```

`apps/sstv/` and `apps/wefax/` carry their own `version` rather than the
workspace's, which is what makes that possible: a shared number would move one
application's version every time the other shipped, and would have given WEFAX
a first release numbered from how far SSTV had already got. The libraries under
`crates/` keep `version.workspace = true`; nothing publishes them separately,
and the number they carry is the workspace's own.

`release-sstv.yml` and `release-wefax.yml` are the two entry points. Each runs
on its own tag pattern, can be dispatched manually with the tag to build, and
does nothing but call `release-app.yml` with what makes that application
itself: the directory under `apps/`, the product name the release is titled
with, whether the archives carry the templates, and the release notes. A third
application is a third caller rather than a copy of the build.

`release-app.yml` holds everything the applications share, in three jobs.

`prepare` resolves the tag and refuses to continue unless it matches the
version of that application's package, because the executable carries the
version compiled into it and Windows records it in the resource. Bump `version`
in `apps/<app>/Cargo.toml` before tagging.

`build` is a matrix of three targets, each on its own runner:

| Target | Runner | Archive |
| --- | --- | --- |
| `x86_64-pc-windows-msvc` | `windows-latest` | `.zip` |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | `.tar.gz` |
| `aarch64-apple-darwin` | `macos-latest` | `.dmg` |

Each builds the one application with `--locked`, so a release is built from the
committed `Cargo.lock` and not from whatever resolves that day. The archives are
named `grayline-<app>-v<version>-<target>`, after the version rather than the
tag, so the application's name appears once instead of twice.

Every archive holds the executable, `LICENSE`, a `licenses.html` generated on
that platform, and the application's own `README.md` if it has one. The
operator's manual is not among them: it is published at
`https://grayline.jl1his.radio/<app>/`, which the Help menu opens, so a
correction reaches every copy without a release and an archive does not carry a
manual that has since been rewritten. The development documentation under
`docs/memo/` is not archived either: it answers to this repository's code
rather than to the operator, and a release that carried it would be handing out
notes on an implementation instead of a manual. The license page is generated
per platform rather than once for all three because the
dependency graph differs by target: a page built on Linux would list neither
`muda` nor `windows-sys`. The Linux archive also carries the desktop entry and
the icon from `apps/<app>/assets/`, which a Wayland compositor needs to find the
window icon.

What the two applications ship beyond that differs, and is what the callers
decide:

| | SSTV | WEFAX |
| --- | --- | --- |
| `templates/` | yes | no |
| `README.md` | yes | no |

macOS gets a bundle in a disk image rather than an archive of bare files.
`package/build-app.sh` stages it, taking the application as its first argument
and the templates directory as an optional one; the bundle name, the
identifier, and the microphone permission text come from a case over the
application, because a bundled process that opens a capture device without that
text is killed by the system.

## The Windows C runtime

`.cargo/config.toml` links the MSVC CRT statically for
`x86_64-pc-windows-msvc`. A dynamically linked build imports
`VCRUNTIME140.dll`, which is part of the Visual C++ redistributable and not of
Windows, while the `api-ms-win-crt-*` imports beside it resolve to
`ucrtbase.dll` and are an operating system component. Only the first one is a
problem, and it is a problem this distribution cannot solve any other way: the
archive has no step that could install a redistributable, and a missing DLL
fails in the loader before the program can report anything.

Statically linking takes the UCRT along with it, so a CRT fix now arrives by
rebuilding rather than through Windows Update. That is the accepted cost. The
alternative of shipping `VCRUNTIME140.dll` beside the executable has the same
servicing property, since an application-local copy is not updated either, and
adds a way to break: an executable copied out of the extracted directory stops
starting.

The build job checks the invariant rather than trusting it, because `RUSTFLAGS`
in the environment replaces the flag in `.cargo/config.toml` without a word
about it.

`release` collects the archives, writes `SHA256SUMS` over them, and publishes
a GitHub release. Re-running a tag that was already released replaces its
archives rather than failing, so a rebuild is a re-run. The notes come from the
caller with `@VERSION@` where the version goes, so the prose that describes what
an archive holds sits beside the inputs that decided it.

Nothing is code signed. A macOS user has to clear the quarantine attribute
before the first run, and the release notes say so.
