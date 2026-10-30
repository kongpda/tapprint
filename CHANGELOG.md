# Changelog

## 0.1.0

First public release.

- Tray and menu-bar app for macOS, Windows, and Linux.
- Local HTTP API on `127.0.0.1:9240`: `GET /v1/status`, `POST /v1/pair`, `POST /v1/print`.
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
- `POST /v1/unpair` lets a site drop its own pairing. The browser client has `disconnectTapPrint`.
