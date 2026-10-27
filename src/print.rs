//! Send a raster to the printer selected in the window.
//!
//! The browser chooses paper, cut, and drawer. It does not choose the host.
//! The address comes from the window, so a stolen pairing code can print, and
//! cannot scan the network.

use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use crate::escpos;
use crate::model::{Fault, JobError, Paper, Shared, Snapshot, Tone, Transport, check_host};
use crate::raster::{self, MonoRaster};
use crate::usb;

pub fn refresh_printers(shared: &Shared) {
    if shared.printer_busy() {
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

fn write_tcp(host: &str, port: u16, bytes: &[u8], check_paper: bool) -> Result<(), JobError> {
    let fail = |message: String| JobError::new(Fault::Failed, message);
    let host = host.trim();
    check_host(host).map_err(fail)?;
    // `[fe80::1]` is how an IPv6 address is typed. The resolver wants it bare.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let targets = resolve(bare, port).map_err(|error| fail(format!("{host}: {error}")))?;
    // A name can resolve to several addresses. Try each one, not only the first.
    let mut last_error = format!("could not resolve {host}");
    let mut connected = None;
    for address in targets {
        match TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => last_error = format!("{host}:{port}: {error}"),
        }
    }
    let mut stream = connected.ok_or_else(|| fail(last_error))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(20)))
        .map_err(|error| fail(error.to_string()))?;
    let key = format!("{host}:{port}");
    if check_paper && !is_silent(&key) {
        match tcp_paper_out(&mut stream) {
            Some(true) => return Err(JobError::new(Fault::PaperOut, escpos::PAPER_OUT)),
            Some(false) => note_answer(&key, true),
            None => note_answer(&key, false),
        }
    }
    // A write that fails part way may have left some of the job in the printer.
    stream.write_all(bytes).map_err(|error| {
        JobError::new(
            Fault::Partial,
            format!("{host}:{port}: {error}. Part of the receipt may have printed."),
        )
    })?;
    let _ = stream.flush();
    // Say we are done, then let the printer close first. Dropping a socket
    // that still has unread bytes from the printer sends a reset, and some
    // print servers then throw away the end of the job.
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut sink = [0u8; 256];
    let mut drained = 0usize;
    while drained < 64 * 1024 {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(read) => drained += read,
        }
    }
    Ok(())
}

/// Look up a printer name, giving up after `LOOKUP_TIMEOUT`. The system
/// lookup has no time limit, and a job waiting on it holds the printer, so
/// every other job would wait too. A plain address needs no lookup.
fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let (send, receive) = std::sync::mpsc::channel();
    let name = host.to_string();
    std::thread::Builder::new()
        .name("tapprint-lookup".to_string())
        .spawn(move || {
            let found = (name.as_str(), port)
                .to_socket_addrs()
                .map(Iterator::collect::<Vec<_>>);
            let _ = send.send(found);
        })
        .map_err(|error| format!("could not look up the name: {error}"))?;
    match receive.recv_timeout(LOOKUP_TIMEOUT) {
        Ok(Ok(found)) => Ok(found),
        Ok(Err(error)) => Err(format!("could not resolve the name: {error}")),
        Err(_) => {
            Err("the name took too long to resolve. Use the printer's IP address".to_string())
        }
    }
}

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Missed paper queries in a row, per printer. Many print servers never
/// answer, and half a second on every receipt adds up, so after
/// `SILENT_AFTER` misses the query is skipped until the app restarts. One
/// miss alone may be a printer still busy with the last job.
static MISSES: Mutex<Vec<(String, u8)>> = Mutex::new(Vec::new());
const SILENT_AFTER: u8 = 3;

fn is_silent(key: &str) -> bool {
    MISSES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|(printer, misses)| printer == key && *misses >= SILENT_AFTER)
}

fn note_answer(key: &str, answered: bool) {
    let mut misses = MISSES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    misses.retain(|(printer, _)| printer != key || !answered);
    if !answered {
        match misses.iter_mut().find(|(printer, _)| printer == key) {
            Some((_, count)) => *count = count.saturating_add(1),
            None => misses.push((key.to_string(), 1)),
        }
    }
}

/// Ask the printer for its paper sensor and wait half a second. `None`
/// when it gives no status reply, so the job is printed as before.
fn tcp_paper_out(stream: &mut TcpStream) -> Option<bool> {
    stream.write_all(&escpos::PAPER_STATUS).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut reply = [0u8; 1];
    match stream.read(&mut reply) {
        Ok(1) => escpos::roll_paper_out(reply[0]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn an_ipv6_address_typed_with_brackets_reaches_the_printer() {
        // The window accepts `[::1]` because that is how people write IPv6.
        let Ok(listener) = TcpListener::bind("[::1]:0") else {
            return; // This machine has no IPv6 loopback.
        };
        let port = listener.local_addr().unwrap().port();
        let reader = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            socket.read_to_end(&mut got).unwrap();
            got
        });
        write_tcp("[::1]", port, b"receipt", false).unwrap();
        assert_eq!(reader.join().unwrap(), b"receipt");
    }

    /// A network printer that answers the paper query with `reply`, or stays
    /// silent, and returns every byte it was sent.
    fn fake_printer(reply: Option<u8>) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut query = [0u8; 3];
            socket.read_exact(&mut query).unwrap();
            assert_eq!(query, escpos::PAPER_STATUS);
            if let Some(reply) = reply {
                socket.write_all(&[reply]).unwrap();
            }
            let mut rest = Vec::new();
            let _ = socket.read_to_end(&mut rest);
            rest
        });
        (port, handle)
    }

    #[test]
    fn a_network_printer_out_of_paper_gets_nothing() {
        // Sending a receipt to an empty printer loses it, and the page would
        // believe it printed. Nothing is sent, and the page is told why.
        let (port, printer) = fake_printer(Some(0x72));
        let error = write_tcp("127.0.0.1", port, b"receipt", true).unwrap_err();
        assert_eq!(error.fault, Fault::PaperOut);
        assert!(printer.join().unwrap().is_empty());
    }

    #[test]
    fn a_network_printer_with_paper_or_no_answer_still_prints() {
        let (port, printer) = fake_printer(Some(0x12));
        write_tcp("127.0.0.1", port, b"receipt", true).unwrap();
        assert_eq!(printer.join().unwrap(), b"receipt");

        // Many print servers never answer status queries. That must not block printing.
        let (port, printer) = fake_printer(None);
        write_tcp("127.0.0.1", port, b"receipt", true).unwrap();
        assert_eq!(printer.join().unwrap(), b"receipt");
    }

    #[test]
    fn a_printer_that_never_answers_is_not_asked_again() {
        // Asking costs half a second per receipt. After three silent replies
        // in a row the query is skipped, so later jobs go straight out.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let printer = std::thread::spawn(move || {
            let mut jobs = Vec::new();
            for _ in 0..4 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut got = Vec::new();
                let _ = socket.read_to_end(&mut got);
                jobs.push(got);
            }
            jobs
        });
        for job in [b"1", b"2", b"3", b"4"] {
            write_tcp("127.0.0.1", port, job, true).unwrap();
        }
        let jobs = printer.join().unwrap();
        for (job, sent) in jobs.iter().take(3).zip([b"1", b"2", b"3"]) {
            assert_eq!(*job, [&escpos::PAPER_STATUS[..], sent].concat());
        }
        assert_eq!(jobs[3], b"4");
    }
}
