# HTTP API

Most pages should use the [browser client](../client/README.md). This page is for calling TapPrint directly.

TapPrint listens on `127.0.0.1` and the saved port (9240 by default). If the port is taken, it stops. It will not sit on the next port and catch the till page.

The host must be `127.0.0.1` or `localhost`, else the reply is `421`. CORS allows any origin, and replies carry `Access-Control-Allow-Private-Network: true`. Chrome and Edge use plain HTTP. Safari blocks an HTTPS page from calling plain HTTP, so on macOS TapPrint also serves the same API over HTTPS on `127.0.0.1:<port + 1>` (9241). Open the Settings tab, choose **Set up Safari**, and approve the macOS prompt once. The certificate is self-signed, covers only `127.0.0.1` and `localhost`, and its key stays in the settings folder. The browser client finds the right address by itself: it tries plain HTTP, then HTTPS.

Chrome 142 and later ask once per site before a page can reach apps on this computer (Local Network Access). Chrome 145 and later call this permission "Apps on device". The person at the till must click **Allow**. If they block it, the client reports `unreachable`. Reset it in the site settings (the icon left of the address). A cross-origin till page inside an iframe needs `allow="local-network-access; loopback-network"` on every `<iframe>` above it.

## Errors

```json
{"ok":false,"error":"A sentence for people.","reason":"machine_code"}
```

| Status | `reason` | Meaning |
| --- | --- | --- |
| 400 | `bad_request` | Malformed request, or `/v1/pair` without an http(s) `Origin`. |
| 401 | `unauthorized` | Missing, wrong, or removed code. |
| 401 | `not_paired` | `/v1/unpair` with a code that is not this site's own. |
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
| 503 | `printer_busy` | Six receipts are already waiting for the printer. Nothing was sent. Try again shortly. |

## `GET /v1/status`

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

## `POST /v1/pair`

One-click pairing from a browser page. The browser adds `Origin` itself. A request without an http(s) `Origin` gets `400`. The optional header `X-Till-Client` names the site in the window (letters, digits, spaces, `-` and `.`, up to 40 characters).

The TapPrint window comes to the front and shows "A website wants to print" with the origin, and **Allow** / **Don't allow**. The request waits up to 120 s.

Each pairing issues a fresh code. If the origin was already allowed, its old code stops working and the window warns that a known site should not need to ask again.

- `200` with `{"ok":true,"code":"<32 hex>"}`. The code is bound to that origin and is only valid on requests whose `Origin` matches.
- `403` `pair_denied`, `408` `pair_timeout`, `409` `pair_busy`, `429` `pair_blocked`.

Allowed sites are listed in the window and can be removed there.

## `POST /v1/unpair`

A site removes its own pairing. Send the site's code as `Authorization: Bearer <code>` from that origin. Returns `{"ok":true}`, or `401` `not_paired` for any other code or origin. The window's own pairing code cannot be removed this way.

## `POST /v1/print`

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

## curl examples

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

## Without the client

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

On macOS the same folder holds `localhost-cert.pem` and `localhost-key.pem`, the certificate and key for Safari. Delete both to make TapPrint create new ones at the next start. Safari then needs **Set up Safari** again.

TapPrint was called Receipt Till. On first start it reads an old `Receipt Till` (or `receipt-till`) settings file. The old file is left in place.
