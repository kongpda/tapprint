#!/bin/sh
# Builds dist/TapPrint.app. Set TAPPRINT_BINARY to bundle a binary that is
# already built (the release workflow passes a universal one). The Dock icon is the ink tile from
# scripts/draw-macos-icon.swift, and the window opens in front. A small
# receipt mark also goes in the menu bar when the bar has room. The ad-hoc
# signature lets this Mac open the app. Other Macs need the Developer ID
# steps in docs/building.md.
set -eu

[ "$(uname -s)" = Darwin ] || { echo "tapprint: macOS is required" >&2; exit 1; }

root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
if [ -z "$version" ]; then
    echo "tapprint: Cargo.toml has no version" >&2
    exit 1
fi

# A release build hands in its own universal binary. Without one, build here.
if [ -n "${TAPPRINT_BINARY:-}" ]; then
    binary=$TAPPRINT_BINARY
else
    cargo build --release --locked --manifest-path "$root/Cargo.toml"
    target_dir=${CARGO_TARGET_DIR:-$root/target}
    binary=$target_dir/release/tapprint
fi
if [ ! -x "$binary" ]; then
    echo "tapprint: no binary at $binary" >&2
    exit 1
fi

mkdir -p "$root/dist"
iconset=$root/dist/AppIcon.iconset
rm -rf "$iconset"
swift "$root/scripts/draw-macos-icon.swift" "$iconset"
iconutil --convert icns --output "$root/dist/AppIcon.icns" "$iconset"

app="$root/dist/TapPrint.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/tapprint"
cp "$root/dist/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
chmod 755 "$app/Contents/MacOS/tapprint"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>TapPrint</string>
	<key>CFBundleExecutable</key>
	<string>tapprint</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundleIdentifier</key>
	<string>com.khable.tapprint</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>TapPrint</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${version}</string>
	<key>CFBundleVersion</key>
	<string>${version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSLocalNetworkUsageDescription</key>
	<string>TapPrint sends receipts to the network printer you chose.</string>
</dict>
</plist>
EOF

# Local launch only. Developer ID replaces this before the app leaves the machine.
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app" >/dev/null
codesign --verify "$app"

echo "$app"
