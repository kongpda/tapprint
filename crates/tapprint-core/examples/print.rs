//! Print a PNG without the TapPrint app.
//!
//!     cargo run -p tapprint-core --example print -- receipt.png
//!     cargo run -p tapprint-core --example print -- receipt.png 192.168.1.50:9100

use tapprint_core::{Options, Printer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let file = arguments
        .next()
        .ok_or("usage: print FILE.png [HOST:PORT]")?;
    let receipt = std::fs::read(&file)?;

    // With an address, a network printer. Without, the USB printer plugged in.
    let printer = match arguments.next() {
        Some(address) => {
            let (host, port) = address
                .rsplit_once(':')
                .ok_or("the address must look like 192.168.1.50:9100")?;
            Printer::network(host, port.parse()?)
        }
        None => Printer::usb()?,
    };

    printer.print_png(&receipt, &Options::default())?;
    println!("Printed {file}.");
    Ok(())
}
