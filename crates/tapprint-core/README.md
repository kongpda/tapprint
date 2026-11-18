# tapprint-core

Print a PNG receipt on a thermal printer from your own Rust program. No printer driver, no print dialog, and no TapPrint app to install.

This is the printing half of [TapPrint](https://github.com/kongpda/tapprint). It turns an image into the dots a thermal head burns, wraps them in ESC/POS, and sends them to a USB printer (printer class) or a network printer (port 9100). Because the receipt is an image, any script prints, such as Khmer.

```toml
[dependencies]
tapprint-core = { git = "https://github.com/kongpda/tapprint" }
```

```rust
use tapprint_core::{Options, Printer};

fn main() -> Result<(), tapprint_core::JobError> {
    let receipt = std::fs::read("receipt.png").expect("a receipt image");

    // The one USB receipt printer that is plugged in.
    let printer = Printer::usb()?;
    printer.print_png(&receipt, &Options::default())?;

    // Or a printer on the network, with the drawer opened after.
    let printer = Printer::network("192.168.1.50", 9100);
    printer.print_png(&receipt, &Options { drawer: true, ..Options::default() })?;
    Ok(())
}
```

Draw the image 512 pixels wide for 80 mm paper, or 360 for 58 mm. Other widths are scaled to fit.

## What it does

- `Printer::usb()`, `Printer::usb_device(id)`, `Printer::network(host, port)`.
- `print_png` with paper width, cut, drawer and dithering. `print_raw` for your own ESC/POS. `open_drawer`.
- `usb::list()` lists USB receipt printers. `net::find_network_printers()` looks for printers on the local network.
- A paper check before each receipt: nothing is sent to a printer that reports no paper.
- `JobError::fault` says whether printing again is safe. After `Fault::Partial`, part of the receipt may already be on paper.

The lower layers are public too: `raster` (image to dots), `escpos` (dots to bytes), `usb` and `net` (sending).

## Each system

- **macOS and Linux** talk to the printer through libusb, which is built in. Nothing to install. On Linux the user needs access to the USB device; see the [udev rule](https://github.com/kongpda/tapprint/blob/main/docs/building.md#linux-usb-notes).
- **Windows** uses the USB print driver Windows installs by itself. A vendor driver, or one swapped with Zadig, hides the printer.
- The printer's USB port must be in **Printer Class** mode.

## Sending from several threads

A `Printer` holds no connection: each job opens the printer, sends, and lets go. Two jobs sent at the same moment would mix their bytes, so share one `Printer` behind a `Mutex` if more than one thread prints.

## Try it

```shell
cargo run -p tapprint-core --example print -- receipt.png
cargo run -p tapprint-core --example print -- receipt.png 192.168.1.50:9100
```

Needs Rust 1.88 or newer. MIT licence.
