//! Network receipt printers: sending a job to port 9100, asking for the
//! paper sensor first, and finding printers on the local network.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::escpos;
use crate::{Fault, JobError, check_host};

/// The usual raw printing port.
pub const RAW_PORT: u16 = 9100;
/// How long one address gets to answer. A printer on the same network
/// answers in a few milliseconds.
const FIND_WAIT: Duration = Duration::from_millis(400);
const FIND_THREADS: usize = 64;

/// Look for printers on this computer's own network, so nobody has to know
/// an IP address. Tries port 9100 on the 254 addresses beside this
/// computer's. Only ever started from the window, never by a web page, and
/// only on a private network.
pub fn find_network_printers() -> Vec<String> {
    let Some(here) = local_ipv4().filter(Ipv4Addr::is_private) else {
        return Vec::new();
    };
    let [a, b, c, _] = here.octets();
    let neighbours: Vec<Ipv4Addr> = (1..=254)
        .map(|last| Ipv4Addr::new(a, b, c, last))
        .filter(|address| *address != here)
        .collect();
    answering(&neighbours, RAW_PORT, FIND_WAIT)
        .iter()
        .map(Ipv4Addr::to_string)
        .collect()
}

/// The address this computer uses to reach its network. Nothing is sent:
/// choosing a route for a UDP socket is enough to learn it.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    // 192.0.2.1 is reserved for documentation, so it is never a real host.
    socket.connect("192.0.2.1:9").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(address) => Some(address),
        IpAddr::V6(_) => None,
    }
}

/// The addresses that accept a connection on `port`, in order.
fn answering(addresses: &[Ipv4Addr], port: u16, wait: Duration) -> Vec<Ipv4Addr> {
    let next = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..FIND_THREADS.min(addresses.len()) {
            scope.spawn(|| {
                while let Some(address) = addresses.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let target = SocketAddr::new(IpAddr::V4(*address), port);
                    if TcpStream::connect_timeout(&target, wait).is_ok() {
                        found
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(*address);
                    }
                }
            });
        }
    });
    let mut found = found
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    found.sort();
    found
}

/// Send `bytes` to a network printer. With `check_paper`, ask its paper
/// sensor first and send nothing when it says the roll is out.
pub fn write_tcp(host: &str, port: u16, bytes: &[u8], check_paper: bool) -> Result<(), JobError> {
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
    fn the_search_finds_only_addresses_that_answer_on_the_printer_port() {
        // Nobody at a till knows the printer's IP address. The search must
        // list the one that answers and leave out the ones that do not.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let silent = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = silent.local_addr().unwrap().port();
        drop(silent);
        let here = Ipv4Addr::LOCALHOST;
        let wait = Duration::from_millis(300);
        assert_eq!(answering(&[here], port, wait), [here]);
        assert!(answering(&[here], closed, wait).is_empty());
        assert!(answering(&[], port, wait).is_empty());
        // Many addresses, few threads' worth of work each: none is skipped.
        let many = vec![here; 40];
        assert_eq!(answering(&many, port, wait).len(), 40);
    }

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
        // Other tests may have used this port number before, and the miss
        // count is kept per address for the life of the process.
        note_answer(&format!("127.0.0.1:{port}"), true);
        let printer = std::thread::spawn(move || {
            let mut jobs = Vec::new();
            // Other programs on the machine sometimes probe a free local port
            // with an HTTP or TLS hello. Ours are at most a status query and
            // one byte.
            while jobs.len() < 4 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut got = Vec::new();
                let _ = socket.read_to_end(&mut got);
                if !got.is_empty() && got.len() <= 5 {
                    jobs.push(got);
                }
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
