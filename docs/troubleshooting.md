# Troubleshooting

## The page cannot reach TapPrint

The client reports reason `unreachable`.

- **On a live site:** the page must be on HTTPS, and the site's Content-Security-Policy must allow TapPrint. See [Going live on a real website](deploying.md). The error's `detail` names the cause when the browser reveals it.
- **Is TapPrint running?** Look for its icon in the menu bar or system tray. `curl http://127.0.0.1:9240/v1/status` should answer.
- **Chrome or Edge:** the browser asks once whether the site may reach apps on this device. If that was blocked, open the site settings (the icon left of the address) and allow it.
- **Safari:** open TapPrint's **Settings** tab and click **Set up** beside **Safari**. Safari only works on a Mac.
- **Inside an iframe:** a cross-origin till page needs `allow="local-network-access; loopback-network"` on every `<iframe>` above it.

## Pairing fails with `insecure_origin`

**Only secure websites** is on in TapPrint's Settings tab and the till page is on plain `http://`. Serve the page over https, or turn the switch off.

## TapPrint does not start

A dialog says why. The usual cause is "Port 9240 is already in use": another copy is running. Quit it from the menu bar or tray, then open TapPrint again.

## The USB printer is not listed

- Set the printer's USB port to **Printer Class**. On an Epson TM-T88V that is in the Feed-button menu.
- Remove any print queue the system made for the printer, then plug it in again.
- **Windows:** the printer must use the driver Windows installs by itself. A vendor driver, or a driver changed with Zadig, hides it.
- **Linux:** add the udev rule in [building.md](building.md#linux-usb-notes).

## Two printers, and receipts go to the wrong one

Open the **Printer** tab and click the right printer. With two of the same model, each is labelled by where it is plugged in.

## The network printer does not print

- On the **Printer** tab choose **Network**. Click the printer TapPrint finds, or type its address and click **Save**. The port is usually 9100.
- **Find printers** only looks at the 254 addresses beside this computer's own, on port 9100. A printer on another network segment, or on another port, has to be typed in.
- Use the printer's IP address. A name that takes more than 5 seconds to look up fails the job.
- Click **Print test**. The message at the top of the window says what went wrong.

## The cash drawer does not open

On the **Printer** tab, beside **Cash drawer**, choose **Pin 5**, then click **Open drawer**. For a heavy drawer that needs a longer pulse, raise `drawer_on_ms` in the [settings file](api.md#settings-file).

## The receipt looks wrong

- **Blurry or thin text:** draw the image exactly 512 px wide for 80 mm paper, or 360 px for 58 mm. Other widths are scaled.
- **Photos print as black blocks:** pass `dither: true` (header `X-Dither: 1`).
- **Small receipt on wide paper, or cut off on the right:** the paper width in the window does not match the roll. Choose 80 mm or 58 mm on the **Printer** tab.

## "Check the paper before printing again"

Reason `maybe_printed` means the printer stopped part way through. Part of the receipt may be on paper, so look before sending it again. TapPrint resets the printer so the next receipt starts clean.

## The site has to pair again and again

The code is kept in the browser's `localStorage` for that site. Private windows, cleared site data, or a different browser profile lose it. Each new pairing also replaces the site's old code.

## Reporting a problem

Open an issue with the bug report template. Include the output of `curl http://127.0.0.1:9240/v1/status`.
