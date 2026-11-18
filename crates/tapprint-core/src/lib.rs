//! Print a receipt image on a thermal printer, with no printer driver.
//!
//! This is the printing half of [TapPrint](https://github.com/kongpda/tapprint),
//! for programs that want to print by themselves instead of asking the
//! TapPrint app to. It turns a PNG into the dots a thermal head burns, wraps
//! them in ESC/POS, and sends them to a USB printer (printer class) or a
//! network printer (port 9100).
//!
//! ```no_run
//! use tapprint_core::{Options, Printer};
//!
//! # fn main() -> Result<(), tapprint_core::JobError> {
//! let receipt = std::fs::read("receipt.png").expect("a receipt image");
//!
//! // The one USB receipt printer that is plugged in.
//! let printer = Printer::usb()?;
//! printer.print_png(&receipt, &Options::default())?;
//!
//! // Or a printer on the network, with the drawer opened after.
//! let printer = Printer::network("192.168.1.50", 9100);
//! printer.print_png(&receipt, &Options { drawer: true, ..Options::default() })?;
//! # Ok(())
//! # }
//! ```
//!
//! Draw the image 512 pixels wide for 80 mm paper, or 360 for 58 mm. Other
//! widths are scaled to fit.
//!
//! When a job fails, [`JobError::fault`] says whether printing again is safe:
//! after [`Fault::Partial`] part of the receipt may already be on paper.

pub mod escpos;
pub mod net;
pub mod raster;
pub mod usb;

use raster::{DOTS_58MM, DOTS_80MM};

/// Why a job failed. The caller needs this to know whether a retry is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The request itself was wrong. Nothing reached the printer.
    BadInput,
    /// The printer reported no paper. Nothing was sent.
    PaperOut,
    /// Some bytes reached the printer. Part of the receipt may be on paper.
    Partial,
    /// Nothing reached the printer.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobError {
    pub fault: Fault,
    pub message: String,
}

impl JobError {
    pub fn new(fault: Fault, message: impl Into<String>) -> Self {
        Self {
            fault,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JobError {}

/// The width of the paper roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    Mm80,
    Mm58,
}

impl Paper {
    pub fn dots(self) -> u32 {
        match self {
            Self::Mm80 => DOTS_80MM,
            Self::Mm58 => DOTS_58MM,
        }
    }

    pub fn millimetres(self) -> u16 {
        match self {
            Self::Mm80 => 80,
            Self::Mm58 => 58,
        }
    }

    pub fn from_millimetres(value: u16) -> Option<Self> {
        match value {
            80 => Some(Self::Mm80),
            58 => Some(Self::Mm58),
            _ => None,
        }
    }
}

/// A host name or IP address. Not a URL, so a saved value cannot point at a scheme.
pub fn check_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("enter the printer address".to_string());
    }
    if host.len() > 253 {
        return Err("that address is too long".to_string());
    }
    let allowed = host
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':' | '[' | ']'));
    if !allowed {
        return Err("use a host name or IP address, not a web address".to_string());
    }
    Ok(())
}

/// What to do besides printing the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub paper: Paper,
    /// Cut the paper after the receipt.
    pub cut: bool,
    /// Open the cash drawer after the receipt.
    pub drawer: bool,
    /// Floyd-Steinberg dithering, for photos and grey logos. Leave it off
    /// for text, which prints sharper without.
    pub dither: bool,
}

impl Default for Options {
    /// 80 mm paper, cut, no drawer, no dithering.
    fn default() -> Self {
        Self {
            paper: Paper::Mm80,
            cut: true,
            drawer: false,
            dither: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Usb(String),
    Network { host: String, port: u16 },
}

/// One printer to send jobs to.
///
/// A `Printer` holds no connection. Each job opens the printer, sends, and
/// lets go, so the printer can be unplugged and plugged back between jobs.
/// To send jobs from several threads, share one `Printer` behind a lock:
/// two jobs sent at once would mix their bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Printer {
    target: Target,
    drawer: [u8; 5],
    check_paper: bool,
}

impl Printer {
    fn new(target: Target) -> Self {
        Self {
            target,
            // Pin 2 for 50 ms: what most drawers expect.
            drawer: escpos::drawer_command(2, 50, 50),
            check_paper: true,
        }
    }

    /// The USB receipt printer that is plugged in. Fails when there is none,
    /// or more than one: then pick one from [`usb::list`] and use
    /// [`Printer::usb_device`].
    pub fn usb() -> Result<Self, JobError> {
        let fail = |message: String| JobError::new(Fault::Failed, message);
        let mut found = usb::list().map_err(fail)?;
        match found.len() {
            0 => Err(fail("no USB printer is plugged in".to_string())),
            1 => Ok(Self::new(Target::Usb(found.remove(0).id))),
            count => Err(fail(format!(
                "{count} USB printers are plugged in. Choose one with usb::list and Printer::usb_device."
            ))),
        }
    }

    /// A USB printer by the `id` that [`usb::list`] gave.
    pub fn usb_device(id: impl Into<String>) -> Self {
        Self::new(Target::Usb(id.into()))
    }

    /// A network printer by host name or IP address. The port is usually 9100.
    pub fn network(host: impl Into<String>, port: u16) -> Self {
        Self::new(Target::Network {
            host: host.into(),
            port,
        })
    }

    /// Which drawer pin to pulse, 2 or 5, and for how long. Most drawers
    /// need pin 2 for 50 ms, which is the default.
    pub fn drawer(mut self, pin: u8, on_ms: u16, off_ms: u16) -> Self {
        self.drawer = escpos::drawer_command(pin, on_ms, off_ms);
        self
    }

    /// Whether to ask the printer for its paper sensor before a receipt,
    /// and send nothing when the roll is out. On by default.
    pub fn check_paper(mut self, check: bool) -> Self {
        self.check_paper = check;
        self
    }

    /// Print a PNG receipt.
    pub fn print_png(&self, png: &[u8], options: &Options) -> Result<(), JobError> {
        let raster = raster::png_to_raster(png, options.paper.dots(), options.dither)
            .map_err(|error| JobError::new(Fault::BadInput, error.0))?;
        let drawer = options.drawer.then_some(self.drawer);
        let bytes = escpos::encode(&raster, options.cut, drawer)
            .map_err(|error| JobError::new(Fault::BadInput, error.0))?;
        self.send(&bytes, self.check_paper)
    }

    /// Send printer commands exactly as given.
    pub fn print_raw(&self, bytes: &[u8]) -> Result<(), JobError> {
        if bytes.is_empty() {
            return Err(JobError::new(Fault::BadInput, "nothing to print"));
        }
        self.send(bytes, false)
    }

    /// Open the cash drawer without printing. It opens even when the paper
    /// is out: the person at the till may still need change.
    pub fn open_drawer(&self) -> Result<(), JobError> {
        self.send(&escpos::drawer_pulse(self.drawer), false)
    }

    fn send(&self, bytes: &[u8], check_paper: bool) -> Result<(), JobError> {
        match &self.target {
            Target::Usb(id) => usb::write(id, bytes, check_paper),
            Target::Network { host, port } => net::write_tcp(host, *port, bytes, check_paper),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, GrayImage, ImageEncoder, Luma};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A network printer that reports paper present and returns what it was sent.
    fn printer_with_paper() -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut first = [0u8; 3];
            socket.read_exact(&mut first).unwrap();
            if first == escpos::PAPER_STATUS {
                socket.write_all(&[0x12]).unwrap();
            } else {
                got.extend_from_slice(&first);
            }
            let _ = socket.read_to_end(&mut got);
            got
        });
        (port, handle)
    }

    fn receipt() -> Vec<u8> {
        let image = GrayImage::from_pixel(512, 24, Luma([0]));
        let mut png = std::io::Cursor::new(Vec::new());
        PngEncoder::new(&mut png)
            .write_image(image.as_raw(), 512, 24, ExtendedColorType::L8)
            .unwrap();
        png.into_inner()
    }

    #[test]
    fn a_png_becomes_one_job_with_the_drawer_and_the_cut() {
        // The whole path a program takes: image in, printer bytes out.
        let (port, printer) = printer_with_paper();
        let options = Options {
            drawer: true,
            ..Options::default()
        };
        Printer::network("127.0.0.1", port)
            .print_png(&receipt(), &options)
            .unwrap();
        let job = printer.join().unwrap();
        assert_eq!(&job[..2], &[0x1b, 0x40], "the job must start with a reset");
        assert!(job.windows(4).any(|w| w == [0x1d, 0x76, 0x30, 0x00]));
        // Drawer on pin 2 for 50 ms, then the cut.
        assert!(job.ends_with(&[0x1b, 0x70, 0, 25, 25, 0x1d, 0x56, 0x42, 48]));
    }

    #[test]
    fn drawer_settings_and_no_cut_are_honoured() {
        let (port, printer) = printer_with_paper();
        let options = Options {
            cut: false,
            drawer: true,
            ..Options::default()
        };
        Printer::network("127.0.0.1", port)
            .drawer(5, 120, 240)
            .print_png(&receipt(), &options)
            .unwrap();
        let job = printer.join().unwrap();
        assert!(job.ends_with(&[0x1b, 0x70, 1, 60, 120]));
    }

    #[test]
    fn something_that_is_not_an_image_never_reaches_the_printer() {
        // No listener at all: a bad image must fail before any connection.
        let error = Printer::network("127.0.0.1", 9)
            .print_png(b"not a png", &Options::default())
            .unwrap_err();
        assert_eq!(error.fault, Fault::BadInput);
        let error = Printer::network("127.0.0.1", 9).print_raw(b"").unwrap_err();
        assert_eq!(error.fault, Fault::BadInput);
    }

    #[test]
    fn raw_commands_and_the_drawer_go_out_unchanged() {
        let (port, printer) = printer_with_paper();
        Printer::network("127.0.0.1", port)
            .print_raw(b"\x1b@Hello\n")
            .unwrap();
        assert_eq!(printer.join().unwrap(), b"\x1b@Hello\n");

        let (port, printer) = printer_with_paper();
        Printer::network("127.0.0.1", port).open_drawer().unwrap();
        assert_eq!(printer.join().unwrap(), [0x1b, 0x40, 0x1b, 0x70, 0, 25, 25]);
    }

    #[test]
    fn a_printer_that_is_not_there_fails_without_claiming_anything_printed() {
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let error = Printer::network("127.0.0.1", closed)
            .print_png(&receipt(), &Options::default())
            .unwrap_err();
        // Nothing was sent, so printing again is safe.
        assert_eq!(error.fault, Fault::Failed);
        assert!(!error.to_string().is_empty());
    }
}
