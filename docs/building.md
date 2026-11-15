# Building from source

You need Rust (stable, edition 2024).

```shell
cargo run              # development
cargo build --release
```

libusb is built from source, so no system libusb is needed.

**macOS.** Build and install locally (Xcode Command Line Tools required):

```shell
scripts/install-local.sh          # /Applications, then launch
scripts/install-local.sh --user   # ~/Applications, no admin rights
```

Use `--dest DIR` for another existing folder, `--no-launch` to install without opening, or `--force` to close a copy that will not quit. Finish printing before installing. The installer builds with the lockfile, generates all Retina icon sizes and the `.icns`, stamps the git commit in bundle metadata, ad-hoc signs and verifies the app, then replaces the installed copy with rollback on verification failure. Settings and paired sites stay in your user settings folder. Login startup remains controlled by **Start at login**.

To build a bundle without installing:

```shell
scripts/macos-app.sh
open "dist/TapPrint.app"
```

The script draws the icon and ad-hoc signs the bundle so this Mac can launch it. Set `TAPPRINT_BINARY=path` to bundle a binary you already built; the release workflow does this with its universal binary. Use `scripts/install-local.sh` to install it. Quit any copy already listening on port 9240 first, including an old Receipt Till, or the new one stops.

**Windows.** Build on a Windows PC. The window's GPU shaders are compiled there, so a build from a Mac does not run. Install Rust and the Visual Studio Build Tools, then run `cargo build --release`. The app is `target\release\tapprint.exe`. It runs without a console window. Left-click the tray icon to open the window and right-click it for the menu.

**Linux.** On Debian or Ubuntu:

```shell
sudo apt-get install pkg-config libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev \
  libudev-dev
cargo build --release
```

## Signing a Mac build

The Developer ID Application certificate is what Gatekeeper accepts. Build the app, then sign the bundle (this replaces the ad-hoc signature from `scripts/macos-app.sh`):

```shell
scripts/macos-app.sh
codesign --force --deep --options runtime --timestamp \
  --sign "Developer ID Application: Your Name (TEAMID)" \
  "dist/TapPrint.app"
ditto -c -k --keepParent "dist/TapPrint.app" "dist/TapPrint.zip"
xcrun notarytool submit "dist/TapPrint.zip" --keychain-profile "notary" --wait
xcrun stapler staple "dist/TapPrint.app"
```

`notarytool` needs a keychain profile created once with an app-specific Apple ID password. The Apple Development and Apple Distribution certificates do not replace this.

Windows builds want Authenticode, not this certificate. An unsigned exe shows SmartScreen once. A self-signed certificate does not remove the warning. Open-source projects can apply to SignPath Foundation for free signing.

## Linux USB notes

USB printers need a udev rule. Create `/etc/udev/rules.d/70-tapprint.rules`. The number must be below 73, so the `uaccess` tag takes effect:

```
SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ENV{ID_USB_INTERFACES}=="*:07????:*", MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

Reload the rules and plug the printer in again:

```shell
sudo udevadm control --reload-rules && sudo udevadm trigger
```

The rule matches any USB printer by its interface class. If your system does not set `ID_USB_INTERFACES`, match one printer by ID instead. Find the ID with `lsusb` (for example `04b8:0202`):

```
SUBSYSTEM=="usb", ATTR{idVendor}=="04b8", ATTR{idProduct}=="0202", MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

`uaccess` gives access to whoever is logged in at the screen. The group is for other users, such as a kiosk account. Debian and Ubuntu have `plugdev`. On Fedora or Arch, use a group that exists there, or create one with `sudo groupadd plugdev`. Then add the user and log in again:

```shell
sudo usermod -aG plugdev "$USER"
```

The Linux kernel's `usblp` driver claims printer-class devices. TapPrint detaches it while it prints and gives it back after.

## Printer setup notes

If the system already has a print queue for a USB printer, remove it and plug the printer in again. On an Epson TM-T88V, Printer Class is a setting in the Feed-button menu.

On Windows the printer must use the USB print driver Windows installs when it is plugged in (`usbprint`). A vendor driver, or a driver swapped to WinUSB with Zadig, hides it from TapPrint.
