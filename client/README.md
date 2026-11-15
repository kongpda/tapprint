# tapprint-client

A small browser client for [TapPrint](../README.md). It has no dependencies and ships as one ES module with types.

Copy `tapprint.js` (and `tapprint.d.ts` for TypeScript) into your site. This package is not published to npm.

```js
import { TapPrint, TapPrintError } from "./tapprint.js";

const till = new TapPrint({ clientName: "MyShop" });

if (!till.isPaired()) await till.pair(); // the person at the till clicks Allow

try {
  await till.print(canvas, { drawer: true });
} catch (err) {
  if (err instanceof TapPrintError) {
    if (err.reason === "unauthorized") {
      // The code was removed in the TapPrint window. Pair again.
    } else if (err.reason === "maybe_printed") {
      // Check the paper before retrying.
    }
    console.error(err.status, err.reason, err.message);
  }
}
```

## API

- `new TapPrint({ port = 9240, secure = "auto", code, storageKey = "tapprint.code", clientName })`. Pass `code` to use a manual pairing code instead of browser pairing. `secure` picks the address: `false` is plain HTTP on `port`, `true` is HTTPS on `port + 1`, and `"auto"` uses whichever answers.
- `status()` returns the status JSON. Printer names and the status line appear only when paired.
- `pair()` asks TapPrint to show "A website wants to print" with this site's origin. It waits up to 120 seconds and stores the code in `localStorage`.
- `isPaired()` is true when a code is stored. The code may still have been removed in the TapPrint window.
- `unpair()` tells TapPrint to remove this site, then deletes the stored code. The site disappears from the TapPrint window. It does nothing when no code is stored.
- `forget()` deletes the stored code only. TapPrint still lists the site.
- `print(source, { paperMm, cut = true, drawer = false, dither = false })` prints a `Blob` (PNG), `HTMLCanvasElement`, or `OffscreenCanvas`.
- `printRaw(bytes)` sends raw ESC/POS bytes exactly as given, so include your own cut and drawer commands. It fails with reason `raw_disabled` unless the person at the till turned on **Raw printer commands**.

## Errors

Every failure throws `TapPrintError` with `status` (0 if TapPrint is not reachable) and `reason`. After a `401` the stored code is cleared and the reason is `unauthorized`.

Render canvases at 512 px wide for 80 mm paper or 360 px for 58 mm, or a whole multiple.

What to do for the reasons a till page meets most:

| `reason` | What happened | What the page should do |
| --- | --- | --- |
| `unreachable` | TapPrint is not running, or the browser blocked the call. | Ask the person to start TapPrint. In Chrome or Edge, allow access to apps on this device. |
| `unauthorized` | The site was removed in the TapPrint window. | Call `pair()` again, then print. |
| `pair_denied`, `pair_timeout` | The person chose **Don't allow**, or did not answer in 120 s. | Say so. Do not ask again at once: TapPrint refuses for a while (`pair_blocked`). |
| `paper_out` | No paper. Nothing was printed. | Ask for a new roll, then print again. |
| `printer_busy` | Too many receipts are waiting. Nothing was printed. | Print again in a few seconds. |
| `printer_error` | The printer could not be reached. Nothing was printed. | Show `error.message`, which says what to check, then print again. |
| `maybe_printed` | The printer stopped part way. | Ask the person to look at the paper before printing again, or the customer may get two. |

The full list is in the [HTTP API](../docs/api.md#errors).

## Browsers

Chrome, Edge, and Firefox call TapPrint over plain `http://127.0.0.1`. Safari blocks that from an HTTPS page, so on a Mac TapPrint also answers over HTTPS on the next port (see the [HTTP API](../docs/api.md)). With `secure: "auto"` the client reads the status over plain HTTP first and over HTTPS second, and keeps the one that answered. Only that status read is tried twice. A print or a pairing request is sent once, to one address.

In Safari the person at the till must choose **Set up Safari** on TapPrint's Settings tab once. Until then every call fails with reason `unreachable`.

## Tests

```shell
node --test client/tapprint.test.mjs
```
