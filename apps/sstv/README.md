# Grayline SSTV

Receives and transmits SSTV (Slow Scan Television) for amateur radio, over the
protocol, DSP, template-rendering, and WAV components this repository holds.

Development documentation is in [docs/memo/](../../docs/memo/README.md),
divided into the SSTV protocols themselves, the behavior of the original
MMSSTV, and this project. The operator's manual is published separately, at
<https://grayline.jl1his.radio/sstv/>, which the application's Help menu
opens.

## Application

Grayline SSTV is the desktop interface, built with egui and eframe:

```text
cargo run -p grayline-sstv-app
```

Selecting an input device opens a capture stream and starts a worker that
demodulates the audio, detects the mode from VIS, and decodes the image
progressively. Mode, decoded rows, input level, synchronization strength, and
decoded FSKID callsigns come from that worker.

To transmit, select an output device from Settings, enter My call, select a KDL
template and stock image, and choose Set for transmit after the composite
preview is ready. TX streams the complete VOX, VIS, raster, footer, FSKID, and
trailing-silence sequence to the selected device. The same button stops an
active transmission. See
[docs/memo/grayline/gui-design.md](../../docs/memo/grayline/gui-design.md)
for the design and remaining work.

Rig control goes through Hamlib's `rigctld` rather than a linked library, so
there is nothing to build and the serial port stays available to the logger.
Start `rigctld` for your rig, connect from the Radio panel, and transmissions
key it and read its frequency into `${radio.frequency}` and `${radio.band}`.
The same panel changes band and steps up and down it.

What is actually sent is decided by a Lua script, because a station keys its
rig in more ways than one protocol covers. The band plan the radio panel offers
comes from a file beside it. Both are built in and need no files; write either
out from Settings › Rig Control, as `rigcontrol.lua` and `bands.toml` beside
`config.toml`, to take it over. See
[docs/memo/grayline/rig-control.md](../../docs/memo/grayline/rig-control.md).

On Linux the window icon comes from a desktop entry rather than from the
application, because a Wayland compositor has no other way to learn one. The
application names itself `grayline-sstv`, and the compositor looks for the
entry of the same name; installing it and the icon it points at is what makes
the icon appear in the task switcher and the dock:

```text
install -Dm644 apps/sstv/assets/grayline-sstv.desktop \
  ~/.local/share/applications/grayline-sstv.desktop
install -Dm644 apps/sstv/assets/icon.png \
  ~/.local/share/icons/hicolor/512x512/apps/grayline-sstv.png
update-desktop-database ~/.local/share/applications
```

The entry's `Exec=grayline-sstv` expects the executable on `PATH`, which
`cargo install --path apps/sstv` arranges; point it at the build directory
instead if you are running from `cargo run`.

[assets/templates/](../../assets/templates) holds the five templates MMSSTV
ships, ported to the KDL format. Copy the ones you want into the application's
templates directory; each file records in a comment what its original did that
this format cannot.

## Command-line tools

`gl-sstv` encodes and decodes WAV files without starting the application. It is
a development tool and is not shipped in the release archives; run it from the
checkout, or install it with `cargo install --path tools/sstv-cli`. Every
subcommand describes itself under `--help`.

### Encode

`gl-sstv encode` renders a KDL template over a background image and writes a
complete SSTV transmission as streaming 48 kHz mono 16-bit PCM:

```text
cargo run -p grayline-sstv-cli -- encode [--callsign CALLSIGN] <TEMPLATE.kdl> <BACKGROUND_IMAGE> <MODE> <OUTPUT.wav>
```

The callsign defaults to `N0CALL`, is uppercased, replaces `${station.callsign}`
in the template, and is sent as the trailing FSKID. `${tx.timestamp.utc}` and
`${tx.timestamp.local}` are set from the clock. The prepared background is also
available to `rximage` layers. Backgrounds are resized to cover the selected
mode and center-cropped. Template image assets are resolved relative to the
template file.

Supported transmit modes are Robot 36/72, Scottie 1/2/DX, Martin 1/2, and
PD50/90/120/160/180/240/290. Mode arguments ignore ASCII case, spaces, hyphens,
and underscores.

### Decode

```text
cargo run -p grayline-sstv-cli -- decode [--packet-size SAMPLES] <INPUT.wav> <OUTPUT_IMAGE>
```

A decode that saved less than a whole picture exits 3, a misused command line
exits 2, and any other failure exits 1.

## Web demo

The receive path also builds for WebAssembly, and
<https://rssstv.kb10uy.dev/> runs it in the browser: drop a recording on the
page, or point a microphone at a receiver, and the picture is decoded by the
same Rust the desktop application uses. Its images are identical to the ones
`gl-sstv decode` produces from the same files. Pushing to `master` deploys it.

```text
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
wasm-pack build apps/web-demo --target web --out-dir www/pkg --release
python -m http.server -d apps/web-demo/www 8080
```

A server is required: `file://` blocks the module and the microphone needs a
secure context. See
[docs/memo/grayline/web-demo.md](../../docs/memo/grayline/web-demo.md).
