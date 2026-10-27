#!/bin/sh
# Builds dist/TapPrint.app. The Dock icon is the ink tile from
# scripts/draw-macos-icon.swift, and the window opens in front. A small
# receipt mark also goes in the menu bar when the bar has room. The ad-hoc
# signature lets this Mac open the app. Other Macs need the Developer ID
# steps in the README.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
if [ -z "$version" ]; then
    echo "tapprint: Cargo.toml has no version" >&2
    exit 1
fi

cargo build --release --manifest-path "$root/Cargo.toml"

target_dir=${CARGO_TARGET_DIR:-$root/target}
binary=$target_dir/release/tapprint
if [ ! -x "$binary" ]; then
    echo "tapprint: release binary missing at $binary" >&2
    exit 1
fi

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
</dict>
</plist>
EOF

# Local launch only. Developer ID replaces this before the app leaves the machine.
codesign --force --sign - "$app" >/dev/null

echo "$app"
