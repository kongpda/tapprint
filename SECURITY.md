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
- **Unwanted sites.** A site needs a code bound to its `Origin`, granted by the person at the till in the TapPrint window. Denied origins are blocked for 10 minutes, unanswered ones for 1 minute, and one pairing request waits at a time. Each pairing issues a fresh code and revokes the old one.
- **Printer reconfiguration.** Raw ESC/POS is off unless the person at the till turns it on.
- **LAN scanning.** The page cannot choose the printer address.
- **Timing attacks on codes.** Codes are compared in constant time.
- **Local file disclosure.** The config file is mode 0600 on macOS and Linux.

What it does not defend against:

- Anyone with access to the till's user account, who can read the config file and the manual pairing code.
- Malware running as the same user. It can talk to the loopback port.
- Programs on this computer faking `Origin`. Only browsers set `Origin` honestly. Any local program can send a pairing request in a known site's name, so the person at the till must still decide. The window warns when an already allowed site asks again.
- A site the person at the till allowed. It can print receipts, and can send raw commands if raw mode is on.
- Anyone holding the manual pairing code. It works from any origin. Keep it private.
