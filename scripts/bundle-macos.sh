#!/usr/bin/env bash
# Build a release binary and wrap it in dist/Nota.app
# NOTA_VERSION / NOTA_BUILD set the version shown in the app (CI sets them).
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="${NOTA_VERSION:-0.1.0}"
BUILD="${NOTA_BUILD:-1}"
export NOTA_VERSION="$VERSION"
cargo build --release
APP=dist/Nota.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/nota "$APP/Contents/MacOS/Nota"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Nota</string>
  <key>CFBundleDisplayName</key><string>Nota</string>
  <key>CFBundleIdentifier</key><string>site.stevenli.nota</string>
  <key>CFBundleExecutable</key><string>Nota</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${BUILD}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
codesign --force --deep -s - "$APP" 2>/dev/null || true
echo "Built $APP ($VERSION) — drag it into /Applications."
