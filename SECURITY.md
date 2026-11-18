# Security policy

## Reporting a vulnerability

Please report privately. Use GitHub's **Report a vulnerability** button under the repository's **Security** tab (private security advisories). Do not open a public issue.

Give the TapPrint version, your OS, and steps to reproduce. We will reply as soon as we can, agree a fix, and credit you if you want.

## Supported versions

Only the latest release gets fixes.

## Threat model

TapPrint lets web pages print on a printer attached to the till. It must not let a hostile page, site, or network host do more than the person at the till allowed.

What it defends against:

- **Other computers.** It listens on `127.0.0.1` only.
- **DNS rebinding.** The `Host` header must be `127.0.0.1` or `localhost`.
- **Unwanted sites.** A site needs a code bound to its `Origin`, granted by the person at the till in the TapPrint window. Denied origins are blocked for 10 minutes, unanswered ones for 1 minute, and one pairing request waits at a time. After any refusal or timeout, no site can ask for 15 s. This slows a page that keeps asking, but does not stop it. The person at the till should close such a page. Each pairing issues a fresh code and revokes the old one.
- **Sites on plain http.** A page served over `http://` can be altered on its way to the till. The pairing prompt says so in red for any http site that is not this computer. **Only secure websites** in Settings refuses such sites: they cannot pair, and codes they hold stop working. It is off by default, because some shops serve the till from an in-house http address.
- **A forged update.** TapPrint asks GitHub once a day whether a newer release exists (this can be turned off in Settings) and never installs anything without a click. It installs only a download whose checksum is listed in a `SHA256SUMS` signed with the project's key, and the signature must name the exact version on offer, so an old release cannot be replayed as a new one. A copy built without the key does not install updates at all. It opens the download page.
- **Printer reconfiguration.** Raw ESC/POS is off unless the person at the till turns it on.
- **LAN scanning.** The page cannot choose the printer address. The window's **Find printers** does look for port 9100 on the addresses beside this computer's own, but only when the person at the till asks for it in the window, only on a private network, and no web page can start it or read its result.
- **A hostile or hijacked print server (Print from KHEvents).** TapPrint only ever calls `https://khevents.app` (a release build refuses anything else and follows no redirects, so the key is never sent to another host). The server can send only PNG pictures: each is checked for the PNG signature and a size limit, then drawn into dots by TapPrint itself, so the server cannot send printer commands, open the drawer or change printer settings. Job ids must be ULIDs before they go into a URL. Text from the server (job name, sender) is cleaned to one short line before it is shown.
- **A stolen print-station key.** The key opens only this printer's own jobs on the server, never orders or buyers. Removing the printer on the Printers page, or making a new code for it, stops the key at once, and TapPrint forgets it on the next call. **Disconnect** in the window forgets it here and tells the server.
- **Timing attacks on codes.** Codes are compared in constant time.
- **Local file disclosure.** The config file is mode 0600 on macOS and Linux.
- **A trusted certificate being misused.** For Safari on macOS, TapPrint makes its own certificate and, when the person at the till sets up Safari in the window and approves the macOS prompt, adds it to that user's keychain as trusted for TLS. The certificate names only `127.0.0.1` and `localhost`, is not a certificate authority, and lasts 800 days. Its key never leaves the settings folder and is mode 0600. So the trust it adds cannot be used to impersonate any website. The HTTPS port is bound to `127.0.0.1` and passes every request to the same checks as the plain port.

What it does not defend against:

- Whoever holds the update signing key. They can sign a release that tills will install when someone clicks Update.
- Anyone with access to the till's user account, who can read the config file, the manual pairing code and the print-station key. Remove the printer on the Printers page if the computer is lost.
- Malware running as the same user. It can talk to the loopback port. On macOS it can also read the certificate key and answer as `127.0.0.1` or `localhost` to this user's browsers, which gains it nothing it did not already have.
- Programs on this computer faking `Origin`. Only browsers set `Origin` honestly. Any local program can send a pairing request in a known site's name, so the person at the till must still decide. The window warns when an already allowed site asks again.
- A site the person at the till allowed. It can print receipts, and can send raw commands if raw mode is on.
- Anyone holding the manual pairing code. It works from any origin. Keep it private.

## Known issues in dependencies

Checked with `cargo audit` on 15 November 2026. CI runs it on every push.

- **tiny_http 0.12 request smuggling (CVE-2026-66752).** tiny_http mishandles `Transfer-Encoding: chunked`, and no fixed release exists. TapPrint refuses any request that has a `Transfer-Encoding` header, so the faulty code never runs. Browsers send `Content-Length` for `fetch` bodies.
- **libusb 1.0.27 denial of service (CVE-2026-47104, CVE-2026-23679).** TapPrint builds libusb from the source bundled in `libusb1-sys` 0.7.0, and that version is 1.0.27. libusb 1.0.30 fixes both issues. To trigger either one, an attacker must plug a malicious USB device into the till, and the worst result is a crash. TapPrint will move to a fixed libusb when `libusb1-sys` ships one.
- **Unmaintained crates.** `paste`, `ttf-parser`, `zbus_names`, `zvariant`, and `zvariant_derive` have no maintainer. They come in through the gpui UI library, and none has a known vulnerability.
