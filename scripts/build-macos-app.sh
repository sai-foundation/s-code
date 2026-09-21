#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
[ "$(uname -s)" = Darwin ] || { echo 'macOS is required to build S-Code.app' >&2; exit 1; }
SWIFT="${SWIFT:-swift}"
MODE=release
case "${1:-}" in --debug) MODE=debug ;; '' ) ;; *) echo 'Usage: scripts/build-macos-app.sh [--debug]' >&2; exit 2 ;; esac
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/.work/macos-cargo}"
if [ "$MODE" = release ]; then
  cargo build --manifest-path "$ROOT/Cargo.toml" --locked --release -p s-code-daemon
else
  cargo build --manifest-path "$ROOT/Cargo.toml" --locked -p s-code-daemon
fi
"$SWIFT" build --package-path "$ROOT/clients/macos" --configuration "$MODE"
SWIFT_BIN="$("$SWIFT" build --package-path "$ROOT/clients/macos" --configuration "$MODE" --show-bin-path)"
DEST="$ROOT/.work/macos-dist"
mkdir -p "$DEST"
STAGE="$(mktemp -d "$DEST/.stage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT HUP INT TERM
APP="$STAGE/S-Code.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources"
cp "$SWIFT_BIN/SCodeDesktop" "$APP/Contents/MacOS/SCodeDesktop"
# Keep screenshot previews self-contained when the app moves away from the build tree.
cp -R "$SWIFT_BIN/SCodeDesktop_SCodeDesktop.bundle" "$APP/Contents/Resources/"
cp "$CARGO_TARGET_DIR/$MODE/s-code-daemon" "$APP/Contents/Helpers/s-code-daemon"
# Use the repository's existing S mark; icon rendering adds no runtime dependency.
"$SWIFT" "$ROOT/scripts/macos/create-icon.swift" "$STAGE/SCode.iconset"
iconutil -c icns "$STAGE/SCode.iconset" -o "$APP/Contents/Resources/SCode.icns"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>SCodeDesktop</string>
<key>CFBundleIdentifier</key><string>foundation.sai.scode.desktop</string>
<key>CFBundleName</key><string>S-Code</string>
<key>CFBundleDisplayName</key><string>S-Code</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleIconFile</key><string>SCode</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict></plist>
PLIST
IDENTITY="${S_CODE_CODESIGN_IDENTITY:--}"
codesign --force --options runtime --sign "$IDENTITY" "$APP/Contents/Helpers/s-code-daemon"
codesign --force --options runtime --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict "$APP"
plutil -lint "$APP/Contents/Info.plist"
# Replace only this script's prior output, after the complete new bundle validates.
if [ -d "$DEST/S-Code.app" ]; then mv "$DEST/S-Code.app" "$STAGE/previous.app"; fi
mv "$APP" "$DEST/S-Code.app"
printf 'Built %s\n' "$DEST/S-Code.app"
