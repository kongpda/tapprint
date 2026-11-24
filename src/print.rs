//! Send a raster to the printer selected in the window.
//!
//! The browser chooses paper, cut, and drawer. It does not choose the host.
//! The address comes from the window, so a stolen pairing code can print, and
//! cannot scan the network.

use std::fs;
use std::panic;
use std::path::Path;

use tapprint_core::net::write_tcp;
pub use tapprint_core::net::{RAW_PORT, find_network_printers, local_ipv4};

use crate::escpos;
use crate::model::{Fault, JobError, Paper, Shared, Snapshot, Tone, Transport};
use crate::raster::{self, MonoRaster};
use crate::usb;

pub fn refresh_printers(shared: &Shared) {
    if !shared.uses_usb() || shared.printer_busy() {
        return;
    }
    list_printers(shared);
}

fn list_printers(shared: &Shared) {
    match usb::list() {
        Ok(devices) => shared.set_printers(
            devices
                .into_iter()
                .map(|device| (device.id, device.label))
                .collect(),
        ),
        Err(error) => shared.note_usb_error(&error),
    }
}

/// What the page asked for besides the image.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub paper: Paper,
    pub cut: bool,
    pub drawer: bool,
    pub dither: bool,
}

pub fn print_test(shared: &Shared) -> Result<(), String> {
    let paper = shared.snapshot().paper;
    let raster = raster::test_slip(paper.dots());
    print_raster(shared, &raster, true, false).map_err(|error| error.message)
}

/// The drawer opens even when the paper is out: the cashier may still need change.
pub fn open_drawer(shared: &Shared) -> Result<(), String> {
    let bytes = escpos::drawer_pulse(shared.snapshot().drawer);
    write_bytes(shared, &bytes, "Opening the drawer", "Drawer opened", false)
        .map_err(|error| error.message)
}

pub fn print_png(shared: &Shared, png: &[u8], options: Options) -> Result<(), JobError> {
    match raster::png_to_raster(png, options.paper.dots(), options.dither) {
        Ok(raster) => print_raster(shared, &raster, options.cut, options.drawer),
        Err(error) => {
            // The page sent something unusable. Say so in the window too.
            let message = format!("The till sent an unusable image: {}", error.0);
            shared.set_status(Tone::Problem, message.as_str());
            shared.note_job(&message, false);
            Err(JobError::new(Fault::BadInput, error.0))
        }
    }
}

/// A local file over this is refused before it is read. A 48 MP phone JPEG is
/// about 20 MB.
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Decode a PNG, JPEG or PDF the person at the till picked in the window,
/// one raster per page, for the preview. Pages and the print server never
/// reach this: they send PNG through `print_png`.
pub fn load_file(path: &Path, paper: Paper) -> Result<Vec<MonoRaster>, String> {
    let bytes = read_file(path)?;
    // A decoder bug on a strange file must not take TapPrint down: the same
    // process prints for pages and the print server.
    panic::catch_unwind(|| raster::file_to_rasters(&bytes, paper.dots()))
        .map_err(|_| "TapPrint could not draw this file".to_string())?
        .map_err(|error| error.0)
}

/// Print the previewed pages, each cut as its own slip. They were fitted to
/// `paper`, so a roll changed in the meantime would print them the wrong width.
/// The first failure stops the rest, so a jam does not feed more pages.
/// The printer is held for the whole file, so a page or server job waiting
/// its turn does not come out between two of its slips.
pub fn print_loaded(shared: &Shared, pages: &[MonoRaster], paper: Paper) -> Result<(), String> {
    let _job = shared.printer_lock();
    if shared.snapshot().paper != paper {
        let message = "The paper size changed. Open the file again.";
        shared.set_status(Tone::Problem, message);
        return Err(message.to_string());
    }
    for (index, page) in pages.iter().enumerate() {
        let sent = escpos::encode(page, true, None)
            .map_err(|error| JobError::new(Fault::BadInput, error.0))
            .and_then(|bytes| write_locked(shared, &bytes, "Printing", "Printed", true));
        if let Err(error) = sent {
            if pages.len() == 1 {
                return Err(error.message);
            }
            let message = format!("Page {} of {}: {}", index + 1, pages.len(), error.message);
            shared.set_status(Tone::Problem, message.as_str());
            return Err(message);
        }
    }
    Ok(())
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err("the file is over 50 MB".to_string());
    }
    fs::read(path).map_err(|error| error.to_string())
}

/// Bytes from the page, sent unchanged. The caller checks that the cashier allowed it.
/// No paper check: the bytes may only open the drawer or ask for status.
pub fn print_raw(shared: &Shared, bytes: &[u8]) -> Result<(), JobError> {
    if bytes.is_empty() {
        return Err(JobError::new(Fault::BadInput, "nothing to print"));
    }
    write_bytes(shared, bytes, "Printing", "Printed raw commands", false)
}

fn print_raster(
    shared: &Shared,
    raster: &MonoRaster,
    cut: bool,
    drawer: bool,
) -> Result<(), JobError> {
    let drawer = drawer.then(|| shared.snapshot().drawer);
    let bytes = escpos::encode(raster, cut, drawer)
        .map_err(|error| JobError::new(Fault::BadInput, error.0))?;
    write_bytes(shared, &bytes, "Printing", "Printed", true)
}

fn write_bytes(
    shared: &Shared,
    bytes: &[u8],
    busy: &str,
    done: &str,
    check_paper: bool,
) -> Result<(), JobError> {
    let _job = shared.printer_lock();
    write_locked(shared, bytes, busy, done, check_paper)
}

/// `write_bytes` for a caller that already holds the printer lock.
fn write_locked(
    shared: &Shared,
    bytes: &[u8],
    busy: &str,
    done: &str,
    check_paper: bool,
) -> Result<(), JobError> {
    let snapshot = shared.snapshot();
    shared.set_status(Tone::Busy, busy);
    let result = match snapshot.transport {
        Transport::Tcp => write_tcp(&snapshot.tcp_host, snapshot.tcp_port, bytes, check_paper),
        Transport::Usb => usb_target(shared, &snapshot)
            .map_err(|message| JobError::new(Fault::Failed, message))
            .and_then(|id| usb::write(&id, bytes, check_paper)),
    };
    match &result {
        Ok(()) => {
            shared.set_status(Tone::Ready, done);
            shared.note_job(done, true);
        }
        Err(error) => {
            shared.set_status(Tone::Problem, error.message.clone());
            shared.note_job(&error.message, false);
        }
    }
    result
}

/// The saved printer. The background scan runs once a second, so a print in
/// the first second, or just after a replug, scans again before giving up.
fn usb_target(shared: &Shared, snapshot: &Snapshot) -> Result<String, String> {
    selected_usb(snapshot).or_else(|_| {
        list_printers(shared);
        selected_usb(&shared.snapshot())
    })
}

/// The saved printer, if it is plugged in. A lone printer is already saved by
/// `Shared::set_printers`, so a choice is only needed when there are several.
fn selected_usb(snapshot: &Snapshot) -> Result<String, String> {
    if !snapshot.listed {
        return Err("still looking for USB printers".to_string());
    }
    if let Some(id) = snapshot
        .usb_id
        .as_ref()
        .filter(|id| snapshot.printers.iter().any(|(found, _)| found == *id))
    {
        return Ok(id.clone());
    }
    if snapshot.printers.is_empty() {
        Err("no USB printer is plugged in".to_string())
    } else {
        Err("choose the USB printer in the window".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A preview is fitted to the roll that was loaded when it opened. If the
    // roll changed since, printing it would come out the wrong width, so it
    // must be refused before anything reaches the printer.
    #[test]
    fn a_preview_for_another_paper_width_is_not_printed() {
        let shared = Shared::for_tests();
        let other = if shared.snapshot().paper == Paper::Mm80 {
            Paper::Mm58
        } else {
            Paper::Mm80
        };
        let page = raster::test_slip(other.dots());
        let error = print_loaded(&shared, &[page], other).unwrap_err();
        assert_eq!(error, "The paper size changed. Open the file again.");
        assert_eq!(shared.snapshot().tone, Tone::Problem);
    }
}
