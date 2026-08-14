#!/usr/bin/env bash
# Stages an application bundle with an ad-hoc signature and wraps it in a
# drag-and-drop disk image. macOS only: sips, iconutil, codesign, and hdiutil.
# The last argument may be empty, which leaves that part out.
# Usage: build-app.sh <app> <binary> <licenses.html> <output.dmg> [templates]
set -euo pipefail

app="$1"
binary="$2"
licenses="$3"
dmg="$4"
templates="${5:-}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/apps/$app/Cargo.toml" | head -n 1)"

# NSMicrophoneUsageDescription is load-bearing: a bundled process that opens
# a capture device without it is killed by the system, so each application
# needs its own sentence rather than a shared one.
case "$app" in
  sstv)
    bundle="GraylineSSTV"
    display="Grayline SSTV"
    microphone="Grayline SSTV listens to the selected capture device to receive SSTV transmissions."
    ;;
  wefax)
    bundle="GraylineWEFAX"
    display="Grayline WEFAX"
    microphone="Grayline WEFAX listens to the selected capture device to receive weather fax transmissions."
    ;;
  *)
    echo "unknown application: $app" >&2
    exit 1
    ;;
esac

executable="grayline-$app"

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

app_dir="$staging/dmg/$bundle.app"
contents="$app_dir/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"

# The icon source is 512 px, so every iconset size up to 512 is a reduction;
# icon_512x512@2x would be an upscale and is left out.
iconset="$staging/$executable.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$root/apps/$app/assets/icon.png" \
    --out "$iconset/icon_${size}x${size}.png" > /dev/null
done
for size in 16 32 128 256; do
  sips -z "$((size * 2))" "$((size * 2))" "$root/apps/$app/assets/icon.png" \
    --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/$executable.icns"

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>$display</string>
  <key>CFBundleExecutable</key>
  <string>$executable</string>
  <key>CFBundleIconFile</key>
  <string>$executable</string>
  <key>CFBundleIdentifier</key>
  <string>org.kb10uy.grayline.$app</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$bundle</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>$microphone</string>
</dict>
</plist>
PLIST

install -m 755 "$binary" "$contents/MacOS/$executable"
cp "$root/LICENSE" "$licenses" "$contents/Resources/"
codesign --force -s - "$app_dir"

if [ -n "$templates" ]; then
  cp -R "$root/$templates" "$staging/dmg/templates"
fi
ln -s /Applications "$staging/dmg/Applications"
hdiutil create -volname "$display" -srcfolder "$staging/dmg" -format UDZO -ov "$dmg"
