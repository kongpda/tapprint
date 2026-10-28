# TapPrint

TapPrint lets a web page print a receipt silently. It is a small tray app (menu bar on macOS) for a till, kiosk, or box office. The page sends a receipt PNG to `http://127.0.0.1:9240`. TapPrint turns it into ESC/POS dots and writes them to a USB or network (port 9100) thermal printer. No print dialog appears. No vendor driver is needed. Because the receipt is an image, any script prints, such as Khmer, which no printer code page covers.

It runs on macOS, Windows, and Linux. It is written in Rust with a gpui GUI.

## How it compares

| Tool | What it is | Where TapPrint differs |
| --- | --- | --- |
| QZ Tray | Java desktop app with a mature API. The core is free. Silent printing without a trust prompt needs a paid support and certificate package (about $449 to $749 a year). | TapPrint has no Java and no certificate. The person at the till allows a site once. |
| WebUSB receipt-printer libraries | Pure browser code. No install. | On Windows the OS print driver claims the device, so WebUSB cannot open it. TapPrint talks to the driver Windows installs. |
| NickelBridge | Localhost service that turns JSON into text receipts. | TapPrint prints images, so any script and any layout works. |
| receiptd | Go daemon for LAN printers. Pre-alpha. | TapPrint also drives USB printers and has a GUI. |
| receiptline | Library that turns markup into ESC/POS. | It is a library, not a helper app. You could use it to build bytes and send them with TapPrint's raw mode. |
| JSPrintManager, PrintNode | Commercial services. | TapPrint is MIT-licensed and runs fully on the till. |

Pick the tool that fits. These are all fine choices for different needs.

## Install

Download the build for your system from [GitHub Releases](https://github.com/kongpda/tapprint/releases). Check it against `SHA256SUMS`.

Builds are not signed yet.

- **macOS.** The app is ad-hoc signed only, so macOS refuses it the first time. Open it once, then go to **System Settings > Privacy & Security** and click **Open Anyway** (macOS 15 and later no longer offer right-click **Open**). Or run `xattr -dr com.apple.quarantine /Applications/TapPrint.app`.
- **Windows.** SmartScreen shows "Windows protected your PC" once. Choose **More info**, then **Run anyway**.
- **Linux.** Unpack the tar.gz and run `tapprint`. USB printers need a udev rule (below).

### Printer setup

The printer's USB port must be in **Printer Class** (the standard USB printer interface). On a TM-T88V that is a setting in the printer's Feed-button menu. If the system already has a queue for the printer, remove it and plug the printer in again.

On Windows the printer must use the USB print driver Windows installs when it is plugged in (`usbprint`). An Epson or other vendor driver, or a driver swapped to WinUSB with Zadig, hides it from TapPrint.

### Linux udev rule

Create `/etc/udev/rules.d/70-tapprint.rules`. The number must be below 73, so the `uaccess` tag takes effect:

```
# Give the logged-in user, and the plugdev group, access to USB printers (interface class 07).
SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ENV{ID_USB_INTERFACES}=="*:07????:*", MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

If your system does not set `ID_USB_INTERFACES`, match one printer by ID instead. Find it with `lsusb` (for example `04b8:0202`):

```
SUBSYSTEM=="usb", ATTR{idVendor}=="04b8", ATTR{idProduct}=="0202", MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

`uaccess` gives access to whoever is logged in at the screen. The group is for other users, such as a kiosk account. Debian and Ubuntu have `plugdev`. On Fedora or Arch, use a group that exists there, or create one with `sudo groupadd plugdev`.

Then add your user to the group, reload the rules, and replug the printer:

```shell
sudo usermod -aG plugdev "$USER"
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Log out and in again for the group to apply.

The Linux kernel's `usblp` driver claims printer-class devices. TapPrint detaches it while it prints and gives it back after.

## Quick start

1. Start TapPrint on the till. The window opens.
2. On your page, load the client in [`client/`](client/README.md) and pair once:

```js
import { TapPrint } from "./tapprint.js";

const till = new TapPrint({ clientName: "MyShop" });

if (!till.isPaired()) await till.pair(); // the TapPrint window shows "A website wants to print" with https://myshop.example
await till.print(canvas, { drawer: true }); // canvas is 512 px wide
```

`examples/index.html` is a full demo page. Serve the repository folder with any static server and open `/examples/`.

Start TapPrint with the computer by turning on **Start with this computer** in the window. It adds a per-user login item and needs no administrator rights. If TapPrint cannot start (for example port 9240 is in use), a dialog says so.

## Using the window

- It lists the sites allowed to print. Click **Remove** twice to cut a site's access.
- It shows the manual pairing code, for `curl`, servers, and kiosks.
- **Allow raw printer commands** is off by default.
- Choose USB or network and type the printer address. **Use this address** saves it. With two printers of the same model, USB printers are labelled by port.
- **Print test slip** checks the head. **Open drawer** pulses the cash drawer.
- When a site asks to pair, the window comes to the front and shows how long the site will wait. Press Esc for **Don't allow**. If the system keeps the window behind, the taskbar flashes (Windows) or the Dock icon bounces (macOS).
- The tray icon gets a dot, and the menu bar title shows `Print ?` or `Print !`, when a site is asking or something needs attention. Windows shows the dot and the tooltip only.

## HTTP API

TapPrint listens on `127.0.0.1` and the saved port (9240 by default). If the port is taken, it stops. It will not sit on the next port and catch the till page.

The host must be `127.0.0.1` or `localhost`, else the reply is `421`. CORS allows any origin, and replies carry `Access-Control-Allow-Private-Network: true`. Use Chrome or Edge. Safari may block an HTTPS page from calling the loopback address.

Chrome 142 and later ask once per site before a page can reach apps on this computer (Local Network Access). Chrome 145 and later call this permission "Apps on device". The person at the till must click **Allow**. If they block it, the client reports `unreachable`. Reset it in the site settings (the icon left of the address). A cross-origin till page inside an iframe needs `allow="local-network-access; loopback-network"` on every `<iframe>` above it.

### Errors

```json
{"ok":false,"error":"A sentence for people.","reason":"machine_code"}
```

| Status | `reason` | Meaning |
| --- | --- | --- |
| 400 | `bad_request` | Malformed request, or `/v1/pair` without an http(s) `Origin`. |
| 401 | `unauthorized` | Missing, wrong, or removed code. |
| 403 | `raw_disabled` | Raw ESC/POS sent while **Allow raw printer commands** is off. |
| 403 | `pair_denied` | The person at the till clicked **Don't allow**. |
| 404 | `not_found` | Unknown path. |
| 408 | `pair_timeout` | Nobody answered within 120 s. |
| 409 | `pair_busy` | Another pairing request is waiting. |
| 413 | `too_large` | Body over 1.5 MB. |
| 415 | `unsupported_type` | `Content-Type` is not `image/png` or `application/vnd.escpos`. |
| 421 | `unknown_host` | Host is not `127.0.0.1` or `localhost`. |
| 429 | `pair_blocked` | This origin was denied in the last 10 minutes or left a request unanswered in the last minute. Also returned to every site for 15 s after any refusal or timeout. |
| 500 | `pair_failed` | Allowed, but the settings could not be saved. No code was issued. |
| 502 | `maybe_printed` | Part of the receipt may have printed. Check the paper before retrying, or the customer gets two. TapPrint resets the printer so the next receipt is clean. |
| 502 | `printer_error` | The printer failed. |
| 503 | `paper_out` | Out of paper. Nothing was sent. |

### `GET /v1/status`

No code needed. Always returns:

| Field | Meaning |
| --- | --- |
| `ok` | `true` |
| `paired` | `true` if the request carried a valid code |
| `port` | the listening port |
| `paper_mm` | 80 or 58 |
| `transport` | `"usb"` or `"tcp"` |
| `state` | `"ready"`, `"busy"`, or `"problem"` |
| `raw_enabled` | whether raw ESC/POS is allowed |

Only when paired: `printers` (array of printer names) and `status` (human text). Unpaired callers do not see these because the status text can hold a printer's LAN address.

### `POST /v1/pair`

One-click pairing from a browser page. The browser adds `Origin` itself. A request without an http(s) `Origin` gets `400`. The optional header `X-Till-Client` names the site in the window (letters, digits, spaces, `-` and `.`, up to 40 characters).

The TapPrint window comes to the front and shows "A website wants to print" with the origin, and **Allow** / **Don't allow**. The request waits up to 120 s.

Each pairing issues a fresh code. If the origin was already allowed, its old code stops working and the window warns that a known site should not need to ask again.

- `200` with `{"ok":true,"code":"<32 hex>"}`. The code is bound to that origin and is only valid on requests whose `Origin` matches.
- `403` `pair_denied`, `408` `pair_timeout`, `409` `pair_busy`, `429` `pair_blocked`.

Allowed sites are listed in the window and can be removed there.

### `POST /v1/print`

Header `Authorization: Bearer <code>`. The code is either:

- a site code from `/v1/pair` (only from that origin), or
- the manual pairing code shown in the window. It works from anywhere. Spaces in it are ignored. Use it for `curl`, servers, and kiosks.

The body is one of:

- `Content-Type: image/png`. A receipt image.
- `Content-Type: application/vnd.escpos`. Raw ESC/POS bytes, sent unchanged. Only when **Allow raw printer commands** is on, else `403` `raw_disabled`. It is off by default because raw commands can rewrite printer settings.

Optional headers:

| Header | Effect |
| --- | --- |
| `X-Paper-Mm: 80` or `58` | Paper width. Default: the width in the window. |
| `X-Cut: 0` | Skip the cut. |
| `X-Drawer: 1` | Kick the cash drawer after printing. |
| `X-Dither: 1` | Floyd-Steinberg dithering, for photos and grey logos. Leave it off for text. |
| `X-Till-Client` | Name shown in the window. |

The body can be up to 1.5 MB. The image is scaled to 512 dots on 80 mm paper and 360 dots on 58 mm paper. Render at exactly that width, or a whole multiple, for the sharpest text. Other widths are scaled, which softens thin lines. A transparent background prints as white paper.

Paper-out is checked before printing: USB printers report it on the printer-class port status, and network printers answer `DLE EOT 4` (TapPrint waits 0.5 s for the reply).

USB writes time out after 20 s. After a partial write the printer is soft-reset.

### curl examples

```shell
# Status without a code
curl http://127.0.0.1:9240/v1/status

# Status with the manual code (adds printers and status text)
curl -H "Authorization: Bearer <pairing code>" http://127.0.0.1:9240/v1/status

# Print a PNG, 80 mm, open the drawer
curl -X POST http://127.0.0.1:9240/v1/print \
  -H "Authorization: Bearer <pairing code>" \
  -H "Content-Type: image/png" \
  -H "X-Paper-Mm: 80" \
  -H "X-Drawer: 1" \
  --data-binary @receipt.png

# Raw ESC/POS (needs "Allow raw printer commands"): text, feed, cut
printf 'Hello\n\n\n\035V\102\000' | curl -X POST http://127.0.0.1:9240/v1/print \
  -H "Authorization: Bearer <pairing code>" \
  -H "Content-Type: application/vnd.escpos" \
  --data-binary @-
```

### Without the client

```js
const response = await fetch("http://127.0.0.1:9240/v1/print", {
  method: "POST",
  headers: {
    Authorization: `Bearer ${code}`,
    "Content-Type": "image/png",
    "X-Till-Client": "MyShop",
    "X-Drawer": "1",
  },
  body: pngBlob,
});
const result = await response.json();
if (!result.ok) throw new Error(result.error);
```

Render Khmer and any QR code into the PNG. The printer has no Khmer code page.

## Settings file

| OS | Path |
| --- | --- |
| macOS | `~/Library/Application Support/TapPrint/config.json` |
| Windows | `%APPDATA%\TapPrint\config.json` |
| Linux | `$XDG_CONFIG_HOME/tapprint/config.json` |

Keys: `port` (9240), `pairing_code`, `paper_mm`, `transport` (`usb` or `tcp`), `tcp_host`, `tcp_port`, `usb_id`, `allow_raw` (false), `drawer_pin` (2 or 5, default 2), `drawer_on_ms` and `drawer_off_ms` (50 and 50), and `sites` (a list of `{origin, code}`).

TapPrint was called Receipt Till. On first start it reads an old `Receipt Till` (or `receipt-till`) settings file. The old file is left in place.

## Security model

- TapPrint listens on loopback only (`127.0.0.1`). Nothing outside the computer can reach it.
- The `Host` header must be `127.0.0.1` or `localhost`. A site that rebinds its own domain to this computer gets `421`.
- Each site gets its own code from `/v1/pair`, after the person at the till clicks Allow. The code works only with that site's `Origin`. Remove the site in the window to revoke it.
- Denied origins are blocked for 10 minutes. Only one pairing request waits at a time, and after any refusal or timeout no site can ask for 15 s. A page can still ask again after that, so deny and close a page that keeps asking.
- The manual code (128-bit) works from anywhere. Keep it private. There is no lockout: a 128-bit code cannot be guessed, and a lockout would let any open tab block the till.
- Raw ESC/POS is off by default, because raw commands can rewrite printer settings.
- The page never gives the printer address. It stays in the window, so a code can print and cannot be used to scan your LAN.
- Codes are compared in constant time.
- The settings file is readable only by your user (mode 0600) on macOS and Linux.
- Unpaired callers get no printer names or status text.

See [SECURITY.md](SECURITY.md) to report a problem.

## Building from source

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

Use `--dest DIR` for another existing folder, `--no-launch` to install without opening, or `--force` to close a copy that will not quit. Finish printing before installing. The installer builds with the lockfile, generates all Retina icon sizes and the `.icns`, stamps the git commit in bundle metadata, ad-hoc signs and verifies the app, then replaces the installed copy with rollback on verification failure. Settings and paired sites stay in your user settings folder. Login startup remains controlled by **Start with this computer**.

To build a bundle without installing:

```shell
scripts/macos-app.sh
open "dist/TapPrint.app"
```

The script draws the icon and ad-hoc signs the bundle so this Mac can launch it. Use `scripts/install-local.sh` to install it. Quit any copy already listening on port 9240 first, including an old Receipt Till, or the new one stops.

**Windows.** Build on a Windows PC. The window's GPU shaders are compiled there, so a build from a Mac does not run. Install Rust and the Visual Studio Build Tools, then run `cargo build --release`. The app is `target\release\tapprint.exe`. It runs without a console window. Left-click the tray icon to open the window and right-click it for the menu.

**Linux.** On Debian or Ubuntu:

```shell
sudo apt-get install pkg-config libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev \
  libssl-dev libudev-dev libasound2-dev libzstd-dev
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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Changes are listed in [CHANGELOG.md](CHANGELOG.md).

## Licence

MIT. See [LICENSE](LICENSE).
