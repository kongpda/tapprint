# TapPrint

Print receipts from a web page with no print dialog and no printer driver.

TapPrint is a tray app for macOS, Windows, and Linux. Your page sends a receipt image; TapPrint prints it on a USB or network thermal printer.

<img src="docs/window.png" alt="The TapPrint window, set to a network printer" width="360">

## Install

1. Download from [Releases](https://github.com/kongpda/tapprint/releases) and open it. Builds are unsigned, so allow it once: **Open Anyway** on macOS (System Settings > Privacy & Security), **More info > Run anyway** on Windows.
2. Connect the printer.
   - **USB:** set the printer's USB port to **Printer Class**.
   - **Network:** choose **Network** in the window and type the printer's address.
3. Click **Print test slip**.

Linux needs a udev rule for USB printers. See [docs/building.md](docs/building.md#linux-usb-notes).

## Print from your page

Copy [`client/tapprint.js`](client/tapprint.js) into your site.

```js
import { TapPrint } from "./tapprint.js";

const till = new TapPrint({ clientName: "MyShop" });

if (!till.isPaired()) await till.pair(); // click Allow in the TapPrint window, once
await till.print(canvas, { drawer: true });
```

Draw the canvas 512 px wide for 80 mm paper, or 360 px for 58 mm.

Safari needs one extra step: in TapPrint, open **Settings** and choose **Set up Safari**.

## Security

TapPrint listens only on this computer. A site can print only after the person at the till clicks **Allow**, and can be removed in the window at any time. Details are in [SECURITY.md](SECURITY.md).

## Docs

- [Browser client](client/README.md)
- [HTTP API](docs/api.md), for `curl` and servers
- [Troubleshooting](docs/troubleshooting.md)
- [Building from source](docs/building.md)
- [Contributing](CONTRIBUTING.md) · [Changelog](CHANGELOG.md)

MIT licence. See [LICENSE](LICENSE).
