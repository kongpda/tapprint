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

## Windows: antivirus or SmartScreen stops TapPrint

The Windows program is not code-signed yet, so Windows has no publisher to trust.

- **"Windows protected your PC" (SmartScreen).** Choose **More info**, then **Run anyway**. It asks once per download.
- **An antivirus removes or blocks it.** Some products distrust any new, unsigned program, more so one that starts at login, listens on a local port and can update itself, all of which TapPrint does on purpose. Check the file against `SHA256SUMS` from the release page, then allow it in the antivirus. If Microsoft Defender flags it, report it as a false positive at https://www.microsoft.com/wdsi/filesubmission.
- **"VCRUNTIME140.dll was not found".** That is 0.1.0-rc.1 or older. Later builds need no extra runtime.

## Linux: TapPrint does not open

The Linux program needs glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 and later) and these libraries, which desktop systems already have: `libxkbcommon`, `libxkbcommon-x11`, `libxcb`, `libfontconfig`, `libudev`. The window also needs a working Vulkan or OpenGL driver.

## Another app cannot print through TapPrint

- Turn on **Network printer mode** in TapPrint's Settings, and give the other app exactly the address and port shown there.
- **"could not use port 9100".** Another program has that port. Type a different port in the card, click **Save**, and use the same number in the other app.
- **A tablet or another computer cannot print.** Turn on **Other devices on this network**, use the address the card then shows (not `127.0.0.1`), and allow TapPrint through the firewall for private networks.
- **The app says it printed but nothing came out.** Look at the TapPrint window: the printer problem shows there. This kind of connection cannot report errors back to the app.
- **"TapPrint is set to print to itself".** On the Printer tab, the network address points back at TapPrint. Choose your real printer.

## TapPrint does not start

A dialog says why. The usual cause is "Port 9240 is already in use": another copy is running. Quit it from the menu bar or tray, then open TapPrint again.

"Could not read settings" means `config.json` is damaged, often by a hand edit. Fix the file, or rename it and open TapPrint again to start with fresh settings. The dialog names the file. On Linux the dialog needs `zenity` or `kdialog`; without either, run `tapprint` from a terminal to see the message.

After a restart, if the page cannot reach TapPrint, turn on **Start at login** in Settings.

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

The drawer controls are hidden until you turn on **Show drawer controls** in **Settings**. Then, on the **Printer** tab, beside **Cash drawer**, choose **Pin 5**, then click **Open cash drawer**. For a heavy drawer that needs a longer pulse, raise `drawer_on_ms` in the [settings file](api.md#settings-file).

## The receipt looks wrong

- **Blurry or thin text:** draw the image exactly 512 px wide for 80 mm paper, or 360 px for 58 mm. Other widths are scaled.
- **Photos print as black blocks:** pass `dither: true` (header `X-Dither: 1`).
- **Small receipt on wide paper, or cut off on the right:** the paper width in the window does not match the roll. Choose 80 mm or 58 mm on the **Printer** tab.

## "Check the paper before printing again"

Reason `maybe_printed` means the printer stopped part way through. Part of the receipt may be on paper, so look before sending it again. TapPrint resets the printer so the next receipt starts clean.

## A site was turned down by mistake

**Don't allow** (or Esc while the request shows) blocks that site from asking again for 10 minutes, so a page cannot keep the prompt on screen. To let it ask at once, quit TapPrint from the menu bar or tray and open it again.

## The site has to pair again and again

The code is kept in the browser's `localStorage` for that site. Private windows, cleared site data, or a different browser profile lose it. Each new pairing also replaces the site's old code.

## Updating

- **The window says a version is ready.** Click **Update**. TapPrint downloads it, checks its signature, and restarts. Finish any receipt first.
- **The button says Download, not Update.** This copy has no signing key built in, so it will not replace itself. The button opens the download page.
- **"Could not write to …" on a Mac.** TapPrint is in a folder your account cannot change. Move it to `~/Applications`, or download the update by hand.
- **No update is ever offered.** The till needs to reach `api.github.com`. To stop it asking, turn off **Check for updates** in Settings.
- **A test build (`-rc`) is never offered the next test build.** Only finished releases are offered. Install a newer test build by hand from the Releases page.

## Reporting a problem

Open an issue with the bug report template. Include the output of `curl http://127.0.0.1:9240/v1/status`.
