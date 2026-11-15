# TapPrint

TapPrint lets a web page print a receipt with no print dialog.

It is a small tray app for a till, kiosk, or box office. Your page sends a receipt image to TapPrint on the same computer, and TapPrint prints it on a USB or network thermal printer. No printer driver is needed. Because the receipt is an image, any script prints, including ones no printer code page covers, such as Khmer.

It runs on macOS, Windows, and Linux.

## Install

Download the build for your system from [GitHub Releases](https://github.com/kongpda/tapprint/releases) and check it against `SHA256SUMS`. Builds are not signed yet, so each system asks once:

- **macOS.** Open the app once, then go to **System Settings > Privacy & Security** and click **Open Anyway**.
- **Windows.** SmartScreen shows "Windows protected your PC". Choose **More info**, then **Run anyway**.
- **Linux.** Unpack the tar.gz and run `tapprint`. USB printers need the udev rule below.

To build it yourself, see [docs/building.md](docs/building.md).

## Connect the printer

**USB.** Set the printer's USB port to **Printer Class**. On an Epson TM-T88V that is in the Feed-button menu. If the system already has a print queue for the printer, remove it and plug the printer in again. On Windows, keep the driver Windows installs by itself; a vendor driver or Zadig hides the printer from TapPrint.

**Network.** In the TapPrint window choose **Network**, type the printer's address (port 9100 is usual), and click **Use this address**.

Then click **Print test slip**.

### Linux: allow USB access

Create `/etc/udev/rules.d/70-tapprint.rules`:

```
SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ENV{ID_USB_INTERFACES}=="*:07????:*", MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

Then reload the rules and plug the printer in again:

```shell
sudo udevadm control --reload-rules && sudo udevadm trigger
```

This gives USB printers to whoever is logged in at the screen. For another account, such as a kiosk user, add it to the `plugdev` group (create the group first on Fedora or Arch) and log in again. The file number must stay below 73. If the rule does not match your printer, see [Linux USB notes](docs/building.md#linux-usb-notes).

## Print from your page

Copy [`client/tapprint.js`](client/README.md) into your site.

```js
import { TapPrint } from "./tapprint.js";

const till = new TapPrint({ clientName: "MyShop" });

if (!till.isPaired()) await till.pair(); // the person at the till clicks Allow, once
await till.print(canvas, { drawer: true }); // canvas is 512 px wide
```

The first time, the TapPrint window shows your site's address and asks **Allow** or **Don't allow**. After that the site prints without asking.

Draw the receipt 512 px wide for 80 mm paper, or 360 px for 58 mm.

[`examples/index.html`](examples/index.html) is a working demo. To print from `curl`, a server, or your own code, see [docs/api.md](docs/api.md).

### Browsers

- **Chrome and Edge** ask once per site whether the page may reach apps on this computer. Click **Allow**.
- **Firefox** needs nothing.
- **Safari** (Mac only) needs one step: open TapPrint's **Settings** tab, choose **Set up Safari**, and approve the macOS prompt.

## The TapPrint window

- **Printer.** Choose USB or Network and the paper width. **Print test slip** and **Open drawer** check the hardware.
- **Websites.** The sites allowed to print. Click **Remove** twice to stop one.
- **Settings.** **Start at login**, the manual pairing code for `curl` and servers, and **Raw printer commands** (off unless your till needs it).

When a site asks to pair, the window comes to the front. Press Esc for **Don't allow**. The tray icon shows a dot while a site is asking or something needs attention.

If TapPrint cannot start, for example because another copy is running, a dialog says why.

## Security

- TapPrint listens only on this computer (`127.0.0.1`). Nothing on the network can reach it.
- A site prints only after the person at the till allows it. Its code works for that site alone and can be removed in the window.
- The page never chooses the printer address, so a site cannot use TapPrint to reach other devices on your network.
- Raw printer commands are off by default, because they can change printer settings.
- Keep the manual pairing code private. It works from anywhere on this computer.

The full threat model, and how to report a problem, are in [SECURITY.md](SECURITY.md).

## More

- [docs/api.md](docs/api.md): the HTTP API, error codes, and the settings file.
- [client/README.md](client/README.md): the browser client.
- [docs/building.md](docs/building.md): building from source and signing a Mac build.
- [CONTRIBUTING.md](CONTRIBUTING.md) and [CHANGELOG.md](CHANGELOG.md).

## Licence

MIT. See [LICENSE](LICENSE).
