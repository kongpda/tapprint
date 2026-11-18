# Printing from other apps

Pick by what the other app can do.

| The other app can… | Use | Setup in the other app |
| --- | --- | --- |
| print to a network receipt printer | [Network printer mode](#network-printer-mode) | Type one address. No code changes. |
| run a program or script | [Command line](#command-line) | One command. |
| run JavaScript in a web page | [Browser client](../client/README.md) | A few lines of code. |
| make an HTTP request | [HTTP API](api.md) | One request. |
| be written in Rust, with no app installed | [The library](#in-a-rust-program-the-library) | One dependency. |
| start a helper program, with no window | [`tapprint serve`](#in-any-other-language-run-it-with-no-window) | One program file shipped with the app. |

In every case TapPrint runs on the computer the printer is plugged into, and the other app talks to it there.

## Network printer mode

For point-of-sale software that was not made for TapPrint. Most of it can print to a "network", "LAN" or "Ethernet" receipt printer by address. TapPrint can stand in for that printer and pass each job on to your real one, USB or network.

**In TapPrint**

1. Open **Settings** and turn on **Network printer mode**.
2. The card shows the address to use, such as `127.0.0.1 port 9100`. Click **Copy**.

**In the other app**

1. Add a printer of the type network, LAN, Ethernet or TCP/IP. If it asks for a brand, choose Epson or ESC/POS.
2. Paste the address: IP `127.0.0.1`, port `9100`.
3. Print a test page from that app.

That is all. The receipt comes out of the printer chosen on TapPrint's **Printer** tab.

### Options

- **Port.** Change it if another program already uses 9100. Type a number from 1024 to 65535 and click **Save**, then give the other app the same number.
- **Other devices on this network.** Off by default, so only apps on this computer can print. Turn it on to let tablets, phones and other computers in the shop print through this computer. The card then shows this computer's own address, such as `192.168.1.23`, to type into those devices. Anyone on that network can then print. Devices on the internet are always refused.

### What to expect

- The other app decides what the receipt looks like. TapPrint sends its printer commands on unchanged, so the app must speak ESC/POS, as nearly all receipt software does.
- This protocol cannot report errors back. If the printer is off or out of paper, the other app still thinks it printed. The TapPrint window and tray icon show the problem.
- The printer's own paper width and cut settings are whatever the other app sends. TapPrint's paper and drawer settings do not apply to these jobs.
- On Windows the firewall may ask once when **Other devices on this network** is turned on. Allow it for private networks.

## Command line

For desktop apps and scripts. TapPrint must already be open.

```shell
tapprint print receipt.png
tapprint print receipt.png --paper 58 --drawer
cat receipt.png | tapprint print -
tapprint status
```

| Option | Effect |
| --- | --- |
| `--paper 80` or `58` | Paper width. Default: the width set in the window. |
| `--no-cut` | Do not cut after printing. |
| `--drawer` | Open the cash drawer after printing. |
| `--dither` | For photos and grey logos. |
| `--raw` | The file holds printer commands, not a PNG. Needs **Raw printer commands** on in Settings. |

The exit code is `0` when it printed, `1` when it failed (the reason is printed), and `2` when the command was wrong.

Where the program is:

- **Windows:** the `tapprint.exe` you run.
- **macOS:** `/Applications/TapPrint.app/Contents/MacOS/tapprint`.
- **Linux:** the `tapprint` file you run.

The command reads the pairing code from the settings file of the same user, so it needs no code typed in. It does not work for another user account on the same computer.

## Without installing the app

Two ways for a developer who wants receipt printing inside their own product, with nothing for their users to install or see.

### In a Rust program: the library

The printing code is a library, [`tapprint-core`](../crates/tapprint-core/README.md). Add it as a dependency and print from your own code. TapPrint itself is not needed at all.

```rust
use tapprint_core::{Options, Printer};

let receipt = std::fs::read("receipt.png")?;
Printer::usb()?.print_png(&receipt, &Options::default())?;
```

### In any other language: run it with no window

Ship the one TapPrint program file with your app and start it yourself:

```shell
tapprint serve
```

It runs with no window and no tray icon until you stop it. Your app then prints through it like any other: the [HTTP API](api.md) with the pairing code, `tapprint print`, or [network printer mode](#network-printer-mode).

- The printer and other settings come from the settings file, so set them there, or run the normal app once to choose them. See [Settings file](api.md#settings-file). A `tapprint-data` folder beside the program keeps the settings with it.
- The pairing code is in that file. It is never printed to the console.
- Websites cannot pair in this mode, because nobody can click **Allow**. They get `403` `pair_unavailable`.
- If it cannot start, the reason is printed and the exit code is 1.

## A web server cannot reach TapPrint directly

TapPrint listens on the computer with the printer, not on the internet. A server elsewhere cannot connect to it. There are two ways to print something a server decides:

- **Through the page.** The server sends the receipt to the web page open on the till, and the page prints it with the [browser client](../client/README.md). See [Going live on a real website](deploying.md).
- **TapPrint asks the server.** This is how [Print from KHEvents](server-printing.md) works: TapPrint itself fetches jobs, so nothing has to reach in from outside.
