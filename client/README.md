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

- `new TapPrint({ port = 9240, code, storageKey = "tapprint.code", clientName })`. Pass `code` to use a manual pairing code instead of browser pairing.
- `status()` returns the status JSON. Printer names and the status line appear only when paired.
- `pair()` asks TapPrint to show "A website wants to print" with this site's origin. It waits up to 120 seconds and stores the code in `localStorage`.
- `isPaired()` is true when a code is stored. The code may still have been removed in the TapPrint window.
- `forget()` deletes the stored code.
- `print(source, { paperMm, cut = true, drawer = false, dither = false })` prints a `Blob` (PNG), `HTMLCanvasElement`, or `OffscreenCanvas`.
- `printRaw(bytes)` sends raw ESC/POS bytes exactly as given, so include your own cut and drawer commands. It fails with reason `raw_disabled` unless the person at the till turned on **Allow raw printer commands**.

Every failure throws `TapPrintError` with `status` (0 if TapPrint is not reachable) and `reason`. After a `401` the stored code is cleared and the reason is `unauthorized`.

Render canvases at 512 px wide for 80 mm paper or 360 px for 58 mm, or a whole multiple.

Use Chrome or Edge. Safari may block HTTPS pages from calling the loopback address.
