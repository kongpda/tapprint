//! Local HTTPS for browsers that refuse a secure page calling plain `http://`.
//!
//! Safari blocks an `https` till page from reaching `http://127.0.0.1`. TapPrint
//! therefore also answers on `https://127.0.0.1:<port + 1>` with its own
//! certificate. The certificate covers only this computer's loopback names.
//! The private key never leaves the settings folder, so what Safari is told to
//! trust can only vouch for 127.0.0.1 and localhost on this Mac. Setup needs
//! one click and the user's macOS approval; until then plain HTTP keeps
//! serving Chrome and Edge as before.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcgen::{CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair};

/// Apple distrusts server certificates valid for much longer than this.
const VALID_DAYS: u64 = 800;
/// Replace the certificate this long before it ends.
const RENEW_BEFORE_DAYS: u64 = 30;
const DAY: u64 = 86_400;

static SERVING: AtomicBool = AtomicBool::new(false);
static TRUSTED: AtomicBool = AtomicBool::new(false);

/// Is the HTTPS listener up?
pub fn serving() -> bool {
    SERVING.load(Ordering::Relaxed)
}

/// Last known answer to "does macOS trust the certificate?". Asking macOS
/// starts a process, so the window reads this instead.
pub fn is_trusted() -> bool {
    TRUSTED.load(Ordering::Relaxed)
}

fn mark_serving(certificate: &Path) {
    SERVING.store(true, Ordering::Relaxed);
    TRUSTED.store(trusted(certificate), Ordering::Relaxed);
}

/// Ask macOS to trust the certificate, then remember the answer.
pub fn set_up(folder: &Path) -> Result<(), String> {
    let identity = identity(folder)?;
    let result = trust(&identity.certificate_path);
    TRUSTED.store(trusted(&identity.certificate_path), Ordering::Relaxed);
    result
}

pub struct Identity {
    pub certificate: Vec<u8>,
    pub private_key: Vec<u8>,
    pub certificate_path: PathBuf,
}

/// The HTTPS port for a given HTTP port.
pub fn secure_port(port: u16) -> Option<u16> {
    port.checked_add(1)
}

/// Most HTTPS connections open at once. A till needs a handful.
const MAX_CONNECTIONS: usize = 32;
/// A connection with no traffic for this long is dropped.
const IDLE: Duration = Duration::from_secs(300);
/// A client that connects must finish the TLS handshake within this long.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// A side that stops taking bytes for this long is dropped.
const STALLED: Duration = Duration::from_secs(30);
const CHUNK: usize = 16 * 1024;

static OPEN: AtomicUsize = AtomicUsize::new(0);

/// Start HTTPS on `secure` with the certificate kept in `folder`. A saved
/// certificate and key that no longer work together, such as after a save
/// that was cut short, are replaced once instead of leaving Safari off for good.
pub fn start(secure: u16, backend: u16, folder: &Path) -> Result<(), String> {
    let ready = identity(folder).and_then(|identity| {
        let config = server_config(&identity)?;
        Ok((identity, config))
    });
    let (identity, config) = match ready {
        Ok(ready) => ready,
        Err(_) => {
            let identity = renew(folder)?;
            let config = server_config(&identity)?;
            (identity, config)
        }
    };
    listen(secure, backend, config)
        .map_err(|error| format!("port {secure} could not be opened ({error})"))?;
    mark_serving(&identity.certificate_path);
    Ok(())
}

fn server_config(identity: &Identity) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    let certificates = CertificateDer::pem_slice_iter(&identity.certificate)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("the saved certificate is unreadable: {error}"))?;
    let key = PrivateKeyDer::from_pem_slice(&identity.private_key)
        .map_err(|error| format!("the saved key is unreadable: {error}"))?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| error.to_string())?
    .with_no_client_auth()
    .with_single_cert(certificates, key)
    .map_err(|error| format!("the certificate and key do not match: {error}"))?;
    Ok(Arc::new(config))
}

/// Answer HTTPS on `secure` and hand each decrypted connection to the plain
/// listener on `backend`, which holds all the rules (host, origin, pairing).
fn listen(secure: u16, backend: u16, config: Arc<rustls::ServerConfig>) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", secure)).map_err(|error| error.to_string())?;
    thread::Builder::new()
        .name("tapprint-https".into())
        .spawn(move || {
            for socket in listener.incoming() {
                let Ok(socket) = socket else {
                    // Out of file handles, most likely. Do not spin on it.
                    thread::sleep(Duration::from_millis(100));
                    continue;
                };
                if OPEN.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    OPEN.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let config = Arc::clone(&config);
                let spawned = thread::Builder::new()
                    .name("tapprint-https-conn".into())
                    .spawn(move || {
                        let _ = relay(config, &socket, backend);
                        OPEN.fetch_sub(1, Ordering::AcqRel);
                    });
                if spawned.is_err() {
                    OPEN.fetch_sub(1, Ordering::AcqRel);
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

type Tls = Mutex<rustls::ServerConnection>;

fn lock(tls: &Tls) -> MutexGuard<'_, rustls::ServerConnection> {
    tls.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Send whatever TLS has ready for the client: handshake replies, alerts,
/// and encrypted data.
fn flush(tls: &mut rustls::ServerConnection, mut socket: &TcpStream) -> io::Result<()> {
    while tls.wants_write() {
        tls.write_tls(&mut socket)?;
    }
    Ok(())
}

/// Copy bytes both ways between one TLS client and the plain listener until
/// either side closes. Each direction has its own thread and sleeps in a
/// blocking read, so an idle connection costs nothing and a reply is passed
/// on at once. The TLS state is shared behind a lock, held only while bytes
/// are handed over, never while waiting for more.
fn relay(config: Arc<rustls::ServerConfig>, socket: &TcpStream, backend: u16) -> io::Result<()> {
    socket.set_read_timeout(Some(HANDSHAKE))?;
    socket.set_write_timeout(Some(STALLED))?;
    let plain = TcpStream::connect(("127.0.0.1", backend))?;
    plain.set_read_timeout(Some(IDLE))?;
    plain.set_write_timeout(Some(STALLED))?;
    let tls = Mutex::new(rustls::ServerConnection::new(config).map_err(io::Error::other)?);
    // Whichever direction ends first closes both sockets. That wakes the
    // other direction out of its read, so neither thread is left behind.
    let close = || {
        let _ = socket.shutdown(Shutdown::Both);
        let _ = plain.shutdown(Shutdown::Both);
    };
    thread::scope(|scope| {
        let replies = thread::Builder::new()
            .name("tapprint-https-reply".into())
            .spawn_scoped(scope, || {
                let result = to_client(&tls, &plain, socket);
                close();
                result
            });
        let requests = match &replies {
            Ok(_) => to_backend(&tls, socket, &plain),
            Err(_) => Err(io::Error::other("no thread for the replies")),
        };
        close();
        let replies = match replies {
            Ok(thread) => thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("the reply thread stopped"))),
            Err(error) => Err(error),
        };
        requests.and(replies)
    })
}

/// Client to listener: decrypt what the browser sends and pass it on.
fn to_backend(tls: &Tls, mut socket: &TcpStream, mut plain: &TcpStream) -> io::Result<()> {
    let mut wire = [0u8; CHUNK];
    let mut clear = Vec::new();
    let mut handshaking = true;
    loop {
        let read = socket.read(&mut wire)?;
        if read == 0 {
            return Ok(());
        }
        let mut pending = &wire[..read];
        let mut closed = false;
        while !pending.is_empty() && !closed {
            clear.clear();
            {
                let mut tls = lock(tls);
                tls.read_tls(&mut pending)?;
                if let Err(error) = tls.process_new_packets() {
                    // Tell the client why before hanging up.
                    let _ = flush(&mut tls, socket);
                    return Err(io::Error::new(io::ErrorKind::InvalidData, error));
                }
                flush(&mut tls, socket)?;
                let mut chunk = [0u8; CHUNK];
                loop {
                    match tls.reader().read(&mut chunk) {
                        // The client said goodbye.
                        Ok(0) => {
                            closed = true;
                            break;
                        }
                        Ok(count) => clear.extend_from_slice(&chunk[..count]),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => return Err(error),
                    }
                }
                if handshaking && !tls.is_handshaking() {
                    handshaking = false;
                    socket.set_read_timeout(Some(IDLE))?;
                }
            }
            // Written without the lock, so a reply can go out meanwhile.
            plain.write_all(&clear)?;
        }
        if closed {
            return Ok(());
        }
    }
}

/// Listener to client: encrypt each reply and send it.
fn to_client(tls: &Tls, mut plain: &TcpStream, socket: &TcpStream) -> io::Result<()> {
    let mut buffer = [0u8; CHUNK];
    loop {
        let read = plain.read(&mut buffer)?;
        let mut tls = lock(tls);
        if read == 0 {
            tls.send_close_notify();
            return flush(&mut tls, socket);
        }
        let mut rest = &buffer[..read];
        while !rest.is_empty() {
            let taken = tls.writer().write(rest)?;
            flush(&mut tls, socket)?;
            if taken == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            rest = &rest[taken..];
        }
    }
}

/// Read the saved certificate, or make one. A certificate near its end is
/// replaced, and the user is asked to trust the new one again.
pub fn identity(folder: &Path) -> Result<Identity, String> {
    let certificate_path = folder.join(CERTIFICATE_FILE);
    let key_path = folder.join(KEY_FILE);
    if let (Ok(certificate), Ok(private_key)) = (fs::read(&certificate_path), fs::read(&key_path))
        && !needs_renewal(&certificate_path)
    {
        return Ok(Identity {
            certificate,
            private_key,
            certificate_path,
        });
    }
    renew(folder)
}

const CERTIFICATE_FILE: &str = "localhost-cert.pem";
const KEY_FILE: &str = "localhost-key.pem";

/// Make a new certificate and key and save them over any old ones.
fn renew(folder: &Path) -> Result<Identity, String> {
    let certificate_path = folder.join(CERTIFICATE_FILE);
    let (certificate, private_key) = generate()?;
    fs::create_dir_all(folder)
        .map_err(|error| format!("could not create {}: {error}", folder.display()))?;
    write_private(&folder.join(KEY_FILE), private_key.as_bytes())?;
    fs::write(&certificate_path, certificate.as_bytes())
        .map_err(|error| format!("could not save the certificate: {error}"))?;
    Ok(Identity {
        certificate: certificate.into_bytes(),
        private_key: private_key.into_bytes(),
        certificate_path,
    })
}

fn needs_renewal(certificate: &Path) -> bool {
    let age = fs::metadata(certificate)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    match age {
        Some(age) => age > Duration::from_secs((VALID_DAYS - RENEW_BEFORE_DAYS) * DAY),
        None => true,
    }
}

fn generate() -> Result<(String, String), String> {
    // Only the names the listener answers to. It is bound to 127.0.0.1.
    let mut params = CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()])
        .map_err(|error| format!("could not prepare the certificate: {error}"))?;
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "TapPrint local helper");
    params.distinguished_name = name;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("the clock is wrong: {error}"))?
        .as_secs();
    // Start a day early so a slightly slow clock still accepts it.
    let (y, m, d) = civil(now.saturating_sub(DAY));
    params.not_before = rcgen::date_time_ymd(y, m, d);
    let (y, m, d) = civil(now + VALID_DAYS * DAY);
    params.not_after = rcgen::date_time_ymd(y, m, d);
    let key = KeyPair::generate().map_err(|error| format!("could not make a key: {error}"))?;
    let certificate = params
        .self_signed(&key)
        .map_err(|error| format!("could not make the certificate: {error}"))?;
    Ok((certificate.pem(), key.serialize_pem()))
}

/// Year, month and day (UTC) for Unix `seconds`.
fn civil(seconds: u64) -> (i32, u8, u8) {
    let z = (seconds / DAY) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (year, month, day)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let failed = |error: io::Error| format!("could not save the key: {error}");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(failed)?;
    // `mode` only applies to a new file. An old key file keeps its own.
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(failed)?;
    file.write_all(bytes).map_err(failed)
}

/// Does macOS already trust the certificate for 127.0.0.1?
pub fn trusted(certificate: &Path) -> bool {
    Command::new("security")
        .args(["verify-cert", "-p", "ssl", "-s", "127.0.0.1", "-c"])
        .arg(certificate)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Ask macOS to trust the certificate for this user. macOS shows its own
/// approval prompt, so this returns only after the user answers it.
pub fn trust(certificate: &Path) -> Result<(), String> {
    let keychain = crate::model::home_dir()
        .join("Library")
        .join("Keychains")
        .join("login.keychain-db");
    let output = Command::new("security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-p", "ssl", "-k"])
        .arg(keychain)
        .arg(certificate)
        .output()
        .map_err(|error| format!("could not run security: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    Err(if detail.is_empty() {
        "macOS did not approve the certificate.".to_string()
    } else {
        format!("macOS did not approve the certificate: {detail}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(951_782_400), (2000, 2, 29));
        assert_eq!(civil(1_782_000_000), (2026, 6, 21));
    }

    #[test]
    fn the_certificate_covers_loopback_and_is_reused() {
        let dir = scratch("tls");
        let first = identity(&dir).unwrap();
        assert!(String::from_utf8_lossy(&first.certificate).contains("BEGIN CERTIFICATE"));
        // A restart must keep the certificate, or Safari's trust would be lost.
        let second = identity(&dir).unwrap();
        assert_eq!(first.certificate, second.certificate);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A TLS client that trusts only `identity`, connected to `secure`.
    fn connect(
        identity: &Identity,
        secure: u16,
    ) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
        use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = rustls::RootCertStore::empty();
        for certificate in CertificateDer::pem_slice_iter(&identity.certificate) {
            roots.add(certificate.unwrap()).unwrap();
        }
        let client = Arc::new(
            rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let name = ServerName::try_from("127.0.0.1").unwrap();
        let socket = loop {
            if let Ok(socket) = TcpStream::connect(("127.0.0.1", secure)) {
                break socket;
            }
            thread::sleep(Duration::from_millis(10));
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        rustls::StreamOwned::new(rustls::ClientConnection::new(client, name).unwrap(), socket)
    }

    fn free_port() -> u16 {
        let probe = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        probe.local_addr().unwrap().port()
    }

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("tapprint-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn https_requests_reach_the_plain_listener_only_when_the_certificate_matches() {
        let folder = scratch("fwd");
        let identity = identity(&folder).unwrap();
        let backend = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let backend_port = backend.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut socket, _) = backend.accept().unwrap();
            let mut seen = [0u8; 64];
            let n = socket.read(&mut seen).unwrap();
            assert!(seen[..n].starts_with(b"ping"));
            socket.write_all(b"pong").unwrap();
        });
        let secure = free_port();
        listen(secure, backend_port, server_config(&identity).unwrap()).unwrap();

        let mut tls = connect(&identity, secure);
        tls.write_all(b"ping").unwrap();
        let mut answer = [0u8; 4];
        tls.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"pong");

        // A client that trusts some other certificate must refuse this one.
        let stranger = super::identity(&scratch("fwd-other")).unwrap();
        let mut refused = connect(&stranger, secure);
        assert!(refused.write_all(b"ping").is_err());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_whole_receipt_and_its_reply_cross_the_relay_unchanged() {
        // A receipt is up to 1.5 MB, far more than one TLS record or one
        // read. Every byte must arrive, in order, in both directions, and
        // the reply must end cleanly when the listener closes.
        let folder = scratch("big");
        let identity = identity(&folder).unwrap();
        let upload: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
        let reply: Vec<u8> = (0..400_000u32).map(|i| (i % 241) as u8).collect();
        let backend = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let backend_port = backend.local_addr().unwrap().port();
        let (expected, answer) = (upload.clone(), reply.clone());
        let server = thread::spawn(move || {
            let (mut socket, _) = backend.accept().unwrap();
            let mut got = vec![0u8; expected.len()];
            socket.read_exact(&mut got).unwrap();
            assert!(got == expected, "the upload was changed on the way");
            socket.write_all(&answer).unwrap();
        });
        let secure = free_port();
        listen(secure, backend_port, server_config(&identity).unwrap()).unwrap();

        let mut tls = connect(&identity, secure);
        tls.write_all(&upload).unwrap();
        let mut got = Vec::new();
        tls.read_to_end(&mut got).unwrap();
        assert!(got == reply, "the reply was changed on the way");
        server.join().unwrap();
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_reply_arrives_without_waiting_for_a_poll() {
        // The relay used to look at each side every 20 ms, which added up to
        // 40 ms to every call. Twenty round trips now take far less than the
        // old loop needed for one or two.
        let folder = scratch("fast");
        let identity = identity(&folder).unwrap();
        let backend = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let backend_port = backend.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut socket, _) = backend.accept().unwrap();
            let mut byte = [0u8; 1];
            while socket.read_exact(&mut byte).is_ok() {
                socket.write_all(&byte).unwrap();
            }
        });
        let secure = free_port();
        listen(secure, backend_port, server_config(&identity).unwrap()).unwrap();
        let mut tls = connect(&identity, secure);
        // The first trip carries the handshake. Leave it out of the timing.
        let mut byte = [7u8; 1];
        tls.write_all(&byte).unwrap();
        tls.read_exact(&mut byte).unwrap();
        let started = std::time::Instant::now();
        for trip in 0..20u8 {
            tls.write_all(&[trip]).unwrap();
            tls.read_exact(&mut byte).unwrap();
            assert_eq!(byte[0], trip);
        }
        let took = started.elapsed();
        assert!(took < Duration::from_millis(400), "{took:?}");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_key_that_does_not_fit_the_certificate_is_replaced_at_start() {
        // A save cut short can leave a new key beside the old certificate.
        // Without a repair, Safari support would stay off on every start.
        let folder = scratch("repair");
        let first = identity(&folder).unwrap();
        let other = generate().unwrap();
        fs::write(folder.join(KEY_FILE), other.1).unwrap();
        let backend = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let secure = free_port();
        start(secure, backend.local_addr().unwrap().port(), &folder).unwrap();
        let repaired = identity(&folder).unwrap();
        assert_ne!(first.certificate, repaired.certificate);
        assert!(server_config(&repaired).is_ok());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_key_file_is_private_even_when_it_already_existed() {
        use std::os::unix::fs::PermissionsExt;
        let folder = scratch("key-mode");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(KEY_FILE);
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"new").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
        let _ = fs::remove_dir_all(&folder);
    }
}
