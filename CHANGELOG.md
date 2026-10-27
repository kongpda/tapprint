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
