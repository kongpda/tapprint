//! Network printer mode: lets apps that were never written for TapPrint
//! print through it.
//!
//! Point-of-sale software usually knows one way to reach a receipt printer
//! on a network: open a connection to port 9100 and send printer commands.
//! With this mode on, TapPrint answers on that port and passes each job to
//! the printer chosen in the window, USB or network. The other app is told
//! "the printer is at 127.0.0.1" and needs to know nothing else.
//!
//! It is off until the person turns it on. It listens on this computer only,
//! unless they also allow other devices, and even then it takes connections
//! from the local network alone, never from the internet.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{Shared, Tone, Transport};
use crate::print;

/// A pause this long after the last byte ends a job. Some apps keep one
/// connection open and send every receipt down it.
const JOB_GAP: Duration = Duration::from_millis(1200);
/// A connection that sends nothing for this long is closed.
const IDLE: Duration = Duration::from_secs(300);
/// Far more than any receipt. It only stops a runaway sender.
const LARGEST_JOB: usize = 4 * 1024 * 1024;
const MOST_CONNECTIONS: usize = 8;
/// How often the listener looks at whether it should stop.
const TICK: Duration = Duration::from_millis(200);

/// The listener that is running now, if any.
struct Running {
    port: u16,
    lan: bool,
    stop: Arc<AtomicBool>,
    listener: thread::JoinHandle<()>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Start, stop or move the listener so it matches the settings. Called at
/// start and after every change in the window.
pub fn apply(shared: &Arc<Shared>) {
    let snapshot = shared.snapshot();
    let mut running = RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let wanted = snapshot.share_on;
    let same = running
        .as_ref()
        .is_some_and(|now| now.port == snapshot.share_port && now.lan == snapshot.share_lan);
    if wanted && same {
        return;
    }
    if let Some(old) = running.take() {
        old.stop.store(true, Ordering::Release);
        old.listener.thread().unpark();
        let _ = old.listener.join();
    }
    if !wanted {
        return;
    }
    let host = if snapshot.share_lan {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    };
    let listener = match TcpListener::bind((host, snapshot.share_port)) {
        Ok(listener) => listener,
        Err(error) => {
            shared.set_status(
                Tone::Problem,
                format!(
                    "Network printer mode could not use port {} ({error}). Choose another port in Settings.",
                    snapshot.share_port
                ),
            );
            return;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let printer = Arc::clone(shared);
    let started = spawn(listener, Arc::clone(&stop), move |job| {
        deliver(&printer, &job);
    });
    match started {
        Ok(listener) => {
            *running = Some(Running {
                port: snapshot.share_port,
                lan: snapshot.share_lan,
                stop,
                listener,
            });
        }
        Err(error) => shared.set_status(
            Tone::Problem,
            format!("Network printer mode could not start: {error}"),
        ),
    }
}

/// Send one job from another app to the printer.
fn deliver(shared: &Shared, job: &[u8]) {
    let snapshot = shared.snapshot();
    // TapPrint's own printer must not be this listener, or a job would go
    // round for ever.
    let to_itself = snapshot.transport == Transport::Tcp
        && snapshot.tcp_port == snapshot.share_port
        && snapshot
            .tcp_host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .map(|address| address.is_loopback() || print::local_ipv4() == Some(to_v4(address)))
            .unwrap_or(snapshot.tcp_host.eq_ignore_ascii_case("localhost"));
    if to_itself {
        shared.set_status(
            Tone::Problem,
            "TapPrint is set to print to itself. Choose your real printer on the Printer tab.",
        );
        return;
    }
    // The outcome shows in the window. This protocol has no way to tell the
    // other app.
    let _ = print::print_raw(shared, job);
}

fn to_v4(address: IpAddr) -> Ipv4Addr {
    match address {
        IpAddr::V4(address) => address,
        IpAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
    }
}

/// May this caller print? This computer always. Another device only from
/// the local network: a home or shop network, or a direct link.
fn allowed(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(address) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        IpAddr::V6(address) => address.is_loopback(),
    }
}

/// Accept connections until `stop` is set, handing each finished job to `sink`.
fn spawn(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    sink: impl Fn(Vec<u8>) + Send + Sync + 'static,
) -> Result<thread::JoinHandle<()>, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let sink = Arc::new(sink);
    let open = Arc::new(AtomicUsize::new(0));
    thread::Builder::new()
        .name("tapprint-share".into())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let (socket, peer) = match listener.accept() {
                    Ok(accepted) => accepted,
                    Err(_) => {
                        thread::park_timeout(TICK);
                        continue;
                    }
                };
                if !allowed(peer.ip()) || open.fetch_add(1, Ordering::AcqRel) >= MOST_CONNECTIONS {
                    if allowed(peer.ip()) {
                        open.fetch_sub(1, Ordering::AcqRel);
                    }
                    continue;
                }
                let (sink, open, stop) = (Arc::clone(&sink), Arc::clone(&open), Arc::clone(&stop));
                let served = thread::Builder::new()
                    .name("tapprint-share-job".into())
                    .spawn({
                        let open = Arc::clone(&open);
                        move || {
                            let _ = receive(socket, &stop, sink.as_ref());
                            open.fetch_sub(1, Ordering::AcqRel);
                        }
                    });
                if served.is_err() {
                    open.fetch_sub(1, Ordering::AcqRel);
                }
            }
        })
        .map_err(|error| error.to_string())
}

/// `DLE EOT n`: an app asking "are you ready?" before it prints. Answered
/// here, so apps that wait for a reply do not hang. The answer says online,
/// with paper. A real fault shows in the TapPrint window when the job runs.
fn status_reply(bytes: &[u8]) -> Option<u8> {
    match bytes {
        [0x10, 0x04, 1] => Some(0x16),
        [0x10, 0x04, 2..=4] => Some(0x12),
        _ => None,
    }
}

/// Read jobs from one connection. A job ends when the app closes the
/// connection, or goes quiet for `JOB_GAP`.
fn receive(
    mut socket: TcpStream,
    stop: &AtomicBool,
    sink: &(impl Fn(Vec<u8>) + ?Sized),
) -> std::io::Result<()> {
    // The listener is non-blocking, and on some systems so is what it accepts.
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(TICK))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut job = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let mut last = Instant::now();
    while !stop.load(Ordering::Acquire) {
        match socket.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                last = Instant::now();
                if job.is_empty()
                    && let Some(reply) = status_reply(&chunk[..read])
                {
                    socket.write_all(&[reply])?;
                    continue;
                }
                job.extend_from_slice(&chunk[..read]);
                if job.len() > LARGEST_JOB {
                    // Not a receipt. Drop it and the caller.
                    return Ok(());
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                if !job.is_empty() && last.elapsed() >= JOB_GAP {
                    sink(std::mem::take(&mut job));
                    last = Instant::now();
                } else if job.is_empty() && last.elapsed() >= IDLE {
                    return Ok(());
                }
            }
            // Some apps drop the connection instead of closing it. What
            // arrived is still the receipt.
            Err(_) => break,
        }
    }
    if !job.is_empty() && !stop.load(Ordering::Acquire) {
        sink(job);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::mpsc;

    /// A listener on a free port. Finished jobs arrive on the channel.
    fn listening() -> (u16, mpsc::Receiver<Vec<u8>>, Arc<AtomicBool>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (send, jobs) = mpsc::channel();
        let send = Mutex::new(send);
        let stop = Arc::new(AtomicBool::new(false));
        spawn(listener, Arc::clone(&stop), move |job| {
            let _ = send.lock().unwrap().send(job);
        })
        .unwrap();
        (port, jobs, stop)
    }

    fn connect(port: u16) -> TcpStream {
        TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).unwrap()
    }

    const WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn a_job_is_what_the_app_sent_before_it_closed() {
        // The usual case: connect, send the receipt, close.
        let (port, jobs, stop) = listening();
        let mut app = connect(port);
        app.write_all(b"\x1b@Hello\n\x1dVB\x00").unwrap();
        drop(app);
        assert_eq!(jobs.recv_timeout(WAIT).unwrap(), b"\x1b@Hello\n\x1dVB\x00");
        stop.store(true, Ordering::Release);
    }

    #[test]
    fn stopping_the_listener_discards_an_incomplete_job() {
        let (port, jobs, stop) = listening();
        let mut app = connect(port);
        app.set_read_timeout(Some(WAIT)).unwrap();
        app.write_all(&[0x10, 0x04, 0x01]).unwrap();
        let mut reply = [0u8; 1];
        app.read_exact(&mut reply).unwrap();
        app.write_all(b"unfinished receipt").unwrap();
        assert!(matches!(
            jobs.recv_timeout(TICK),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        stop.store(true, Ordering::Release);
        app.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(app.read(&mut reply).unwrap(), 0);
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn an_app_that_keeps_the_connection_open_still_gets_each_receipt() {
        // Some software opens the printer once and never closes it. Without
        // a pause ending the job, its receipts would never print.
        let (port, jobs, stop) = listening();
        let mut app = connect(port);
        app.write_all(b"first").unwrap();
        assert_eq!(jobs.recv_timeout(WAIT).unwrap(), b"first");
        app.write_all(b"second").unwrap();
        assert_eq!(jobs.recv_timeout(WAIT).unwrap(), b"second");
        stop.store(true, Ordering::Release);
    }

    #[test]
    fn a_large_receipt_sent_in_pieces_is_one_job() {
        let (port, jobs, stop) = listening();
        let receipt: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut app = connect(port);
        for piece in receipt.chunks(7_000) {
            app.write_all(piece).unwrap();
        }
        drop(app);
        assert!(jobs.recv_timeout(WAIT).unwrap() == receipt);
        stop.store(true, Ordering::Release);
    }

    #[test]
    fn an_app_asking_whether_the_printer_is_ready_gets_an_answer() {
        // Many drivers ask first and wait. No answer would hang them, and
        // the question itself must not come out of the printer.
        let (port, jobs, stop) = listening();
        let mut app = connect(port);
        app.set_read_timeout(Some(WAIT)).unwrap();
        app.write_all(&[0x10, 0x04, 0x01]).unwrap();
        let mut reply = [0u8; 1];
        app.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0x16]);
        app.write_all(&[0x10, 0x04, 0x04]).unwrap();
        app.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0x12]);
        app.write_all(b"receipt").unwrap();
        drop(app);
        assert_eq!(jobs.recv_timeout(WAIT).unwrap(), b"receipt");
        stop.store(true, Ordering::Release);
    }

    #[test]
    fn something_far_too_big_to_be_a_receipt_is_not_printed() {
        let (port, jobs, stop) = listening();
        let mut app = connect(port);
        let block = vec![0u8; 64 * 1024];
        // The listener hangs up part way, so later writes may fail.
        for _ in 0..=(LARGEST_JOB / block.len()) + 4 {
            if app.write_all(&block).is_err() {
                break;
            }
        }
        drop(app);
        assert!(jobs.recv_timeout(Duration::from_secs(3)).is_err());
        stop.store(true, Ordering::Release);
    }

    #[test]
    fn only_this_computer_and_the_local_network_may_print() {
        for near in [
            "127.0.0.1",
            "192.168.1.20",
            "10.0.0.7",
            "172.16.4.2",
            "169.254.1.1",
            "::1",
        ] {
            assert!(allowed(near.parse().unwrap()), "{near}");
        }
        // Anything that could be the internet is turned away, even when
        // other devices are allowed.
        for far in [
            "8.8.8.8",
            "203.0.113.9",
            "172.32.0.1",
            "2001:db8::1",
            "fe80::1",
        ] {
            assert!(!allowed(far.parse().unwrap()), "{far}");
        }
    }

    #[test]
    fn a_stopped_listener_lets_go_of_its_port() {
        // Changing the port, or turning the mode off, must free the old one.
        let (port, _jobs, stop) = listening();
        stop.store(true, Ordering::Release);
        let give_up = Instant::now() + WAIT;
        loop {
            if TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
                break;
            }
            assert!(Instant::now() < give_up, "port {port} was never freed");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
