# Changelog

## 0.1.0-rc.8

- **Open a file…** on the Printer tab opens a PNG, JPEG or PDF from this computer and shows it exactly as it will print, then **Print** sends it at the paper width. Each PDF page prints as its own slip, up to 50 pages and 5 m of paper. Photos are dithered, and phone photos print the right way up. Pages and the print server still send only PNG.
- A USB write that fails after it started is reported as "check the paper", not as safe to retry, so a server job is not printed twice.
- A print-server job with an unreadable field is reported and skipped instead of stopping every later job. A job that cannot be reported no longer makes TapPrint ask for it several times a second.
- A second printer of the same model is no longer taken for the receipt printer when that one is switched off.
- The browser client cleans `clientName` to what the header can carry, so a name in Khmer or another script no longer makes every call fail as unreachable. A Content-Security-Policy warning no longer sticks after HTTPS works.
- `tapprint print` says a timed-out job may still print, instead of "not running".
- Network printer mode drops a request from a web page instead of sending its body to the printer, so only sites allowed in the window can print. A "ready?" query sent right after a job is answered, not printed.
- A print-server job whose next slip the server refuses reports the slips already printed, so they are not offered again. After a paper-out or offline printer, TapPrint waits its normal interval before asking again instead of five times a second.
- A network printer that drops the connection on the paper check is reported as safe to retry, not "check the paper".
- A page that sends its receipt a byte at a time is cut off after 20 s, so it cannot hold the print queue. A broken picture from the print server no longer stops server printing until restart.
- **Only secure websites** is on for new installs. Existing settings are unchanged.
- Start at login opens TapPrint in the menu bar or tray with no window. The window still opens when a site asks to be allowed.
- The window says why **Print test slip** is unavailable and shows the result next to the buttons. Text fields show focus, status and help text is larger with more contrast, and the Websites tab explains its empty list.
- Esc turns a site down only while its request is on screen. Offline and failed print-server states show in red, and "Paused on this computer" is told apart from "Paused by owner".
- Settings groups **Raw printer commands** and **Network printer mode** under Advanced, and the drawer switch is now **Show drawer controls**. The tray menu says **Open TapPrint** and **Quit TapPrint** and ticks the printer connection in use. Buttons and menus use the same words.
- Clearer message for a damaged settings file. The README and troubleshooting guide cover Start at login, a site turned down by mistake, and updates for test builds.

## 0.1.0-rc.7

- Printer setup reports USB scanning, a missing printer, an unselected printer, or a missing network address. Test slips and manual drawer controls become available when a printer is configured, and the Printer tab asks you to save address changes before using them.
- Station credentials stay with their original print server. Switching servers requires a new connection code.
- Turning off network printer mode discards incomplete jobs and releases its listener immediately. Network-only tills skip background USB scans.
- Updated SHA-256 and heapless dependencies. Release assets include the exact libusb source used by the build and instructions to rebuild with a modified copy.
- Includes the network printer mode, CLI, headless mode, and Rust printing library developed since the published rc.4 build.

## 0.1.0

Real-printer validation for the first stable release is still pending.

First public release.

- Print from KHEvents (`0.1.0-rc.3`): connect TapPrint to khevents.app with a one-time code from the Printers page, and sellers on any phone or laptop can print on this printer. TapPrint asks for jobs about every 2 seconds, prints each slip like a local print, and reports what the printer did. Only PNG slips are accepted from the server. A slip that may have printed is never printed again by TapPrint. Pause, Disconnect and the last 20 jobs (with the sender's name) are on the Websites tab. Local printing is unchanged. See [docs/server-printing.md](docs/server-printing.md).
- The print server, its name in the window, and the GitHub repository for updates are build settings (`TAPPRINT_PRINT_SERVER`, `TAPPRINT_PRINT_SERVER_NAME`, `TAPPRINT_UPDATE_REPO`). The defaults are unchanged. The print station protocol is documented in [docs/print-station-protocol.md](docs/print-station-protocol.md).
- Third-party notices: releases carry `THIRD-PARTY-LICENSES.html`, the downloads carry libusb's LGPL notice, and CI checks every dependency's licence.
- Tray and menu-bar app for macOS, Windows, and Linux.
- Local HTTP API on `127.0.0.1:9240`: `GET /v1/status`, `POST /v1/pair`, `POST /v1/unpair`, `POST /v1/print`.
- Prints PNG receipts to USB (printer class) and network (port 9100) ESC/POS thermal printers. No vendor driver and no print dialog.
- One-click pairing: each site gets its own code bound to its origin, approved in the TapPrint window. Allowed sites can be removed there. Each pairing issues a fresh code, and the window warns when an allowed site asks again. A manual pairing code works for curl, servers, and kiosks.
- Optional raw ESC/POS mode (off by default).
- Paper widths 80 mm and 58 mm, cut control, cash drawer kick (pin and timing settings), and optional Floyd-Steinberg dithering.
- Paper-out check before printing (skipped for raw jobs, and for network printers that never answer it). Distinct errors for `paper_out` (nothing sent) and `maybe_printed` (check the paper).
- USB writes time out after 20 s, and the printer is soft-reset after a partial write.
- Two USB printers of the same model are labelled by port.
- Start-at-login toggle. Startup errors show as a dialog.
- Loopback only, Host check, constant-time code compare, config file mode 0600.
- Browser client in `client/` and a demo page in `examples/`.
- Reads the settings file of the earlier Receipt Till name.
- After any refusal or timeout, no site can ask to pair for 15 s.
- Requests with `Transfer-Encoding` get `400`, to avoid a tiny_http request-smuggling bug (CVE-2026-66752).
- The window has a USB / Network switch, and Allow and Don't allow look the same so neither is clicked by habit.
- Pairing shows a countdown, Esc means Don't allow, and the tray icon gets a dot while a site asks or something is wrong. Remove needs a second click.
- Failed jobs no longer count as printed. A printer name that does not resolve within 5 s fails the job instead of holding up the printer.
- A second copy of the app no longer rewrites the settings file before it finds the port taken.
- The client reads the stored code on every call, so a code paired in another tab is used and is not wiped by an old one.
- Safari support on macOS: TapPrint also answers on `https://127.0.0.1:9241` with its own loopback-only certificate. Settings > Set up Safari asks macOS to trust it. Chrome and Edge keep using plain HTTP. The HTTPS front uses rustls 0.23.
- `POST /v1/unpair` lets a site drop its own pairing. The browser client has `unpair()`.
- The browser client works in Safari: by default it tries plain HTTP, then the HTTPS port, and remembers which one answered. `secure: true` or `false` fixes the choice.
- The HTTPS relay no longer polls. An idle Safari connection costs nothing, and replies are no longer held back by up to 40 ms.
- A certificate and key that no longer fit together are replaced at start. The certificate no longer lists `::1`.
- New `503` `printer_busy`: at most six receipts wait for the printer, so status and pairing stay answered while a printer stalls.
- The window footer counts jobs, not receipts, because test slips and drawer pulses are counted too.
- The Mac app says why it uses the local network (`NSLocalNetworkUsageDescription`), for the macOS 15 prompt when a network printer is used.
- CI runs clippy on macOS, Windows, and Linux, builds the Mac bundle, and tests the browser client.
- Fixed: on a new install, choosing **Network** did nothing, so a network printer's address could not be typed in the window.
- The window has a **Cash drawer** setting for pin 2 or pin 5. It used to need a settings-file edit.
- A failed USB scan says "Could not look for USB printers" instead of a bare error, and is no longer shown to a till that prints over the network.
- Clearer wording in the window, and section headings in one style.
- The window is redesigned: smaller, one screen per tab with no scrolling, switches and segmented choices in place of rows of buttons, and help text behind an (i) beside each setting.
- **Find printers** looks for network printers, so nobody has to know an IP address. Choosing **Network** for the first time starts it, and one click on a result saves it.
- When TapPrint cannot be reached, the client says why if the browser tells: the page is not on HTTPS, the site's Content-Security-Policy blocks it, or the person blocked the site (`error.detail`).
- New guide for going live on a real website (`docs/deploying.md`).
- The pairing prompt warns in red when the site is on plain http (other than this computer). A new **Only secure websites** switch refuses such sites (`403` `insecure_origin`). It is off by default.
- Updates: TapPrint checks GitHub Releases once a day and shows a newer version in the window. **Update** downloads it, checks its signature, swaps it in and restarts. Nothing is installed without a click. **Check for updates** in Settings turns the check off.
- Releases carry the bare program for Windows and Linux beside the archives, and a signed `SHA256SUMS` when the signing key is set up.
- The Mac app is signed with a Developer ID and notarized by Apple when the release has the certificate, so macOS opens it without the "could not verify" block. `scripts/install-local.sh` and `scripts/macos-app.sh` sign with `TAPPRINT_SIGN_IDENTITY` when it is set.
- Mac releases are now two apps, `TapPrint-macos-arm64.zip` for Apple silicon and `TapPrint-macos-x86_64.zip` for Intel, in place of one universal app. Each is about half the size. An update downloads the one that matches the Mac.
- A waiting update also shows in the tray menu, since a till usually runs with the window closed. Turning **Check for updates** on looks at once. Updates are only ever fetched from this project's GitHub pages.
- Updates are signed: the release checksums carry a minisign signature that the app checks before installing.
- Settings has a Cash drawer switch. It is off on a new install and on for an existing one. When off, the Printer tab hides the pin choice and Open drawer.
- The version shows at the bottom of every tab, and Clear empties the Till pages list.
- The test slip is larger and taller, so it reads on paper.
- Windows: the program no longer needs the Visual C++ runtime, so it starts on a clean PC. It now has an icon and file details.
- Windows and Linux: a `tapprint-data` folder beside the program keeps the settings there, for portable use.
- Network printer mode: apps not made for TapPrint can print through it. Turn it on in Settings and give the other app the address shown (`127.0.0.1` port 9100). The port can be changed, and a second switch lets other devices on the local network print. Off by default.
- Command line: `tapprint print receipt.png` hands a file to the TapPrint that is already open, for desktop apps and scripts. `tapprint status` and `tapprint --help` too.
- New guide: `docs/other-apps.md`.
- For developers who do not want the app: the printing code is now a library, `tapprint-core`, that a Rust program adds as a dependency and calls directly (`Printer::usb()?.print_png(...)`). The app is built on the same library.
- `tapprint serve` runs TapPrint with no window and no tray icon, so another app in any language can ship the one program file, start it, and print through it. Websites cannot pair in this mode (`403` `pair_unavailable`); use the pairing code, the command line, or network printer mode.
