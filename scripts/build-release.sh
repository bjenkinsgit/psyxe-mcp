#!/bin/bash
# build-release.sh — Build the release binary, wrap in .app bundle, and sign.
#
# The .app bundle is required so macOS shows the binary in the Full Disk Access
# list (System Settings). The LaunchAgent (--watch-lock) needs FDA to read the
# NoteStore.sqlite database for native Apple Notes hashtags.

set -e

IDENTITY="Developer ID Application: Advanced Modeling Concepts, Inc. (RKMCP6WLZG)"
BINARY="target/release/prolog-router"
APP="target/release/PrologRouterDaemon.app"

echo "Building release..."
cargo build --release

echo "Creating .app bundle..."
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"

# Copy binary into the bundle (codesign requires regular files, not symlinks)
cp "$BINARY" "$APP/Contents/MacOS/prolog-router"

cat > "$APP/Contents/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.picoforge.prolog-router.daemon</string>
    <key>CFBundleName</key>
    <string>PrologRouterDaemon</string>
    <key>CFBundleExecutable</key>
    <string>prolog-router</string>
    <key>CFBundleVersion</key>
    <string>1.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSBackgroundOnly</key>
    <true/>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
PLIST

echo "Signing binary and .app bundle..."
codesign -s "$IDENTITY" -f "$BINARY"
codesign -s "$IDENTITY" -f "$APP"
codesign -v "$APP"

echo "Done: $APP (signed)"
echo ""
echo "To grant Full Disk Access:"
echo "  System Settings > Privacy & Security > Full Disk Access > +"
echo "  Navigate to: $(pwd)/$APP"
