# Using TapPrint from a real website

On `localhost` the browser allows almost everything. A live site, such as `https://till.myshop.com`, meets rules that `localhost` skips. This page lists them.

They were checked with Chromium 153 loading the demo page from a non-local address and calling a running TapPrint.

## Checklist

1. **Serve the till page over HTTPS.** Chrome and Edge refuse calls to this computer from a plain `http://` site. `http://localhost` is the one exception, for development.
2. **Allow TapPrint in your Content-Security-Policy.** If the site sends a CSP, add both addresses to `connect-src`:

   ```
   connect-src 'self' http://127.0.0.1:9240 https://127.0.0.1:9241
   ```

3. **Expect one browser prompt per till.** Chrome and Edge ask whether the site may reach apps on this device. The person at the till clicks **Allow** once. On tills you manage, an administrator can allow the site in advance with Chrome's enterprise policy for local network access (`LocalNetworkAccessAllowedForUrls`, or `LoopbackNetworkAllowedForUrls` in newer versions; check Chrome's policy list for your version).
4. **Use one address for the till page.** A pairing belongs to one origin. `https://myshop.com` and `https://www.myshop.com` are different sites to TapPrint, and so are staging and production. Redirect to one of them.
5. **In an iframe**, every `<iframe>` above the till page needs `allow="local-network-access; loopback-network"`.
6. **Safari** works on a Mac after **Set up** beside **Safari** in TapPrint's Settings tab.

## http and https

TapPrint accepts both, and treats them differently:

| The till page is on | What happens |
| --- | --- |
| `https://…` | Works. This is the normal case. |
| `http://localhost` or `http://127.0.0.1` | Works. It is for development, and browsers treat it as secure. |
| any other `http://…`, such as `http://192.168.1.10` | Chrome and Edge block it. In Firefox it can pair, and the TapPrint prompt warns that the site is not secure. |

On plain http, someone on the same network can change the page before it reaches the till, and the changed page can print. If every till page you use is on https, turn on **Only secure websites** in TapPrint's Settings tab. An http site then gets `403` `insecure_origin` when it asks to pair, and a code it already holds stops working until the switch is turned off.

## What each till needs

Each till runs its own TapPrint and pairs once. Nothing is shared between tills.

The pairing is kept in the browser's storage for your site. A private window, cleared site data, or another browser profile loses it, and the till asks to pair again. Do not run the till in a private window.

## Your server cannot call TapPrint

TapPrint listens on `127.0.0.1` of the till. Only something running on the till can reach it: the page in the browser, or a program on that computer using the manual pairing code. A web server elsewhere cannot. Have the server send the receipt data to the page, and let the page print it.

## Keep the till page apart from the rest of the site

Any script running on the paired origin can print and open the drawer, because the pairing lives in that origin's storage. That includes analytics, chat widgets, and anything injected through a flaw elsewhere on the site.

- Put the till on its own address, such as `https://till.myshop.com`, and pair only that.
- Load no third-party scripts on it.
- Give it a strict Content-Security-Policy.

## When it does not connect

The client's error tells you which rule was hit:

| `error.detail` | Meaning |
| --- | --- |
| `insecure_page` | The page is not on HTTPS. |
| `blocked_by_csp` | The site's Content-Security-Policy forbids the call. |
| `permission_denied` | The person blocked the site in the browser's prompt. Allow it in the site settings. |
| not set | TapPrint is not running, or the browser gave no reason. |

All of them have `reason` `unreachable`.
