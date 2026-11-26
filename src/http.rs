//! Loopback API the till page calls.
//!
//! Bound to 127.0.0.1 and the configured port only. A second copy cannot
//! slide onto the next port. Printing requires a pairing code: the one shown
//! in the window, or one a site got from `/v1/pair` after the cashier allowed
//! it. Status does not reveal either. The Host header must name this
//! computer, so a site that rebinds its own domain to 127.0.0.1 cannot read
//! the status either.

use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::model::{Fault, JobError, PairError, Paper, Shared, Snapshot, Tone, Transport};
use crate::print;

const MAX_BODY: usize = 1_500_000;
/// How long a pairing request waits for the cashier to answer.
const PAIR_WAIT: Duration = Duration::from_secs(120);
/// Enough that a status check is answered while prints queue behind a slow
/// printer and a pairing request waits. Printer bytes still go out one job at a time.
const WORKERS: usize = 8;
/// Receipts that may wait for the printer at once. Each one holds a worker
/// until the printer takes it, and a stalled printer holds a job for up to
/// 20 s. The workers left over keep answering status, pairing and unpairing.
const MAX_PRINTS: usize = WORKERS - 2;
const _: () = assert!(MAX_PRINTS < WORKERS);

static PRINTS: AtomicUsize = AtomicUsize::new(0);

/// One place in the print queue, given back when the job ends.
struct PrintSlot;

impl PrintSlot {
    fn take() -> Option<Self> {
        if PRINTS.fetch_add(1, Ordering::AcqRel) >= MAX_PRINTS {
            PRINTS.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self)
    }
}

impl Drop for PrintSlot {
    fn drop(&mut self) {
        PRINTS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn serve(shared: Arc<Shared>) -> Result<(), String> {
    let port = shared.snapshot().port;
    // Right after an update the old copy still holds the port for a moment.
    let give_up = std::time::Instant::now() + crate::update::RESTART_WAIT;
    let waiting = crate::update::restarting(&crate::model::data_dir());
    let server = loop {
        match Server::http(format!("127.0.0.1:{port}")) {
            Err(_) if waiting && std::time::Instant::now() < give_up => {
                thread::sleep(Duration::from_millis(250));
            }
            other => break other,
        }
    };
    let server = server.map_err(|error| {
        format!(
            "Port {port} on this computer is already in use ({error}). TapPrint, or the older Receipt Till, may already be running: look for it in the menu bar or the system tray. Quit it, then open TapPrint again."
        )
    })?;
    spawn_workers(server, &shared, port, "http")?;
    // Safari needs HTTPS. Failing here must not stop Chrome and Edge printing.
    #[cfg(target_os = "macos")]
    if let Err(error) = serve_secure(port) {
        shared.set_status(Tone::Problem, format!("Safari support is off: {error}"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn serve_secure(port: u16) -> Result<(), String> {
    let secure = crate::tls::secure_port(port).ok_or("no port is free for HTTPS")?;
    crate::tls::start(secure, port, &crate::model::data_dir())
}

fn spawn_workers(
    server: Server,
    shared: &Arc<Shared>,
    port: u16,
    scheme: &str,
) -> Result<(), String> {
    let server = Arc::new(server);
    for worker in 0..WORKERS {
        let server = Arc::clone(&server);
        let shared = Arc::clone(shared);
        thread::Builder::new()
            .name(format!("tapprint-{scheme}-{worker}"))
            .spawn(move || {
                while let Ok(request) = server.recv() {
                    // One bad request must not take down a listener. The
                    // connection drops, and the next request is served.
                    let _ = catch_unwind(AssertUnwindSafe(|| handle(&shared, port, request)));
                }
            })
            .map_err(|error| format!("could not start the local server: {error}"))?;
    }
    Ok(())
}

fn handle(shared: &Shared, port: u16, mut request: Request) {
    if !local_host(header(&request, "host").as_deref(), port) {
        let _ = respond(
            request,
            StatusCode(421),
            error_json("unknown host", "unknown_host"),
        );
        return;
    }
    // A page's `fetch` always sends `Content-Length`. Chunked framing is
    // refused outright: tiny_http 0.12 mishandles it (CVE-2026-66752).
    if header(&request, "transfer-encoding").is_some() {
        let _ = respond(
            request,
            StatusCode(400),
            error_json("send the body with Content-Length", "bad_request"),
        );
        return;
    }
    let method = request.method().as_str().to_string();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or(url.as_str()).to_string();
    if method == "OPTIONS" {
        let _ = respond(request, StatusCode(204), Vec::new());
        return;
    }
    if method == "GET" && path == "/v1/status" {
        // Any page in the browser can read status. Only the till page, which
        // holds the code, is listed in the window as connected.
        let paired = shared.authorize(
            header(&request, "origin").as_deref(),
            &bearer(&request).unwrap_or_default(),
        );
        if paired {
            note_caller(shared, &request);
        }
        let body = status_json(&shared.snapshot(), paired);
        let _ = respond(request, StatusCode(200), body);
        return;
    }
    if method == "POST" && path == "/v1/pair" {
        let (status, body) = pair(shared, &request);
        let _ = respond(request, status, body);
        return;
    }
    if method == "POST" && path == "/v1/unpair" {
        let removed = shared.unpair(
            header(&request, "origin").as_deref(),
            &bearer(&request).unwrap_or_default(),
        );
        let (status, body) = if removed {
            (
                StatusCode(200),
                serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap_or_default(),
            )
        } else {
            (StatusCode(401), error_json("not paired", "not_paired"))
        };
        let _ = respond(request, status, body);
        return;
    }
    if method == "POST" && path == "/v1/print" {
        let (status, body) = print(shared, &mut request);
        let _ = respond(request, status, body);
        return;
    }
    let body = error_json("not found", "not_found");
    let _ = respond(request, StatusCode(404), body);
}

/// A site asks to print. The window asks the cashier, and this request waits
/// for the answer. Only a browser page sends `Origin`, and it cannot fake it.
fn pair(shared: &Shared, request: &Request) -> (StatusCode, Vec<u8>) {
    let Some(origin) = header(request, "origin") else {
        return (
            StatusCode(400),
            error_json(
                "pairing needs a web page. Use the code shown in the window instead.",
                "bad_request",
            ),
        );
    };
    let client = caller_label(request);
    match shared.request_pair(&origin, &client, PAIR_WAIT) {
        Ok(code) => {
            note_caller(shared, request);
            (
                StatusCode(200),
                serde_json::to_vec(&serde_json::json!({"ok": true, "code": code}))
                    .unwrap_or_default(),
            )
        }
        Err(error) => {
            let (status, message, reason) = pair_failure(&error);
            (StatusCode(status), error_json(&message, reason))
        }
    }
}

fn pair_failure(error: &PairError) -> (u16, String, &'static str) {
    match error {
        PairError::BadOrigin => (
            400,
            "only http and https pages can pair".to_string(),
            "bad_request",
        ),
        PairError::NoWindow => (
            403,
            "this TapPrint runs with no window, so nobody can allow a website. Use its pairing code instead.".to_string(),
            "pair_unavailable",
        ),
        PairError::Insecure => (
            403,
            "this till only allows secure (https) websites. Serve the page over https.".to_string(),
            "insecure_origin",
        ),
        PairError::Denied => (
            403,
            "the cashier did not allow this site".to_string(),
            "pair_denied",
        ),
        PairError::TimedOut => (
            408,
            "nobody answered in the TapPrint window".to_string(),
            "pair_timeout",
        ),
        PairError::Busy => (
            409,
            "another site is waiting for an answer. Try again shortly.".to_string(),
            "pair_busy",
        ),
        PairError::Blocked => (
            429,
            // Also returned for a short pause after any refusal, so the
            // cashier is not asked again straight away.
            "pairing is paused. Try again later.".to_string(),
            "pair_blocked",
        ),
        PairError::Failed(message) => (500, message.clone(), "pair_failed"),
    }
}

/// A receipt arrives in well under a second. A body sent a byte at a time
/// would hold a print slot, and six of them would turn every till away.
const BODY_TIME: Duration = Duration::from_secs(20);

/// Stops reading once `until` has passed.
struct Deadline<R> {
    reader: R,
    until: Instant,
}

impl<R: Read> Read for Deadline<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if Instant::now() > self.until {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.reader.read(buf)
    }
}

fn print(shared: &Shared, request: &mut Request) -> (StatusCode, Vec<u8>) {
    let code = bearer(request).unwrap_or_default();
    if !shared.authorize(header(request, "origin").as_deref(), &code) {
        return (
            StatusCode(401),
            error_json("pairing code was rejected", "unauthorized"),
        );
    }
    note_caller(shared, request);
    let content_type = header(request, "content-type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let raw = content_type.starts_with("application/vnd.escpos");
    if !raw && !content_type.starts_with("image/png") {
        return (
            StatusCode(415),
            error_json(
                "content type must be image/png or application/vnd.escpos",
                "unsupported_type",
            ),
        );
    }
    if raw && !shared.snapshot().allow_raw {
        return (
            StatusCode(403),
            error_json(
                "raw printer commands are off. Turn them on in the TapPrint window.",
                "raw_disabled",
            ),
        );
    }
    let paper = match paper_header(shared, request) {
        Ok(paper) => paper,
        Err(message) => return (StatusCode(400), error_json(&message, "bad_request")),
    };
    let options = print::Options {
        paper,
        cut: header(request, "x-cut").is_none_or(|value| value != "0"),
        drawer: header(request, "x-drawer").is_some_and(|value| value == "1"),
        dither: header(request, "x-dither").is_some_and(|value| value == "1"),
    };
    // Taken before the body is read, so a full queue costs the page nothing.
    let Some(_slot) = PrintSlot::take() else {
        return (
            StatusCode(503),
            error_json(
                "the printer is busy with other receipts. Nothing was printed. Try again shortly.",
                "printer_busy",
            ),
        );
    };
    let mut body = Vec::new();
    if (Deadline {
        reader: request.as_reader(),
        until: Instant::now() + BODY_TIME,
    })
    .take(u64::try_from(MAX_BODY.saturating_add(1)).unwrap_or(u64::MAX))
    .read_to_end(&mut body)
    .is_err()
    {
        return (
            StatusCode(400),
            error_json("could not read the request body", "bad_request"),
        );
    }
    if body.len() > MAX_BODY {
        return (
            StatusCode(413),
            error_json("the body is larger than 1.5 MB", "too_large"),
        );
    }
    let result = if raw {
        print::print_raw(shared, &body)
    } else {
        print::print_png(shared, &body, options)
    };
    match result {
        Ok(()) => (
            StatusCode(200),
            serde_json::to_vec(&serde_json::json!({"ok": true})).unwrap_or_default(),
        ),
        Err(error) => {
            let (status, reason) = job_failure(&error);
            (StatusCode(status), error_json(&error.message, reason))
        }
    }
}

/// The status code tells the page whether printing again is safe.
fn job_failure(error: &JobError) -> (u16, &'static str) {
    match error.fault {
        Fault::BadInput => (400, "bad_request"),
        // Nothing was sent. Load paper and print again.
        Fault::PaperOut => (503, "paper_out"),
        // Printing again may give the customer two receipts.
        Fault::Partial => (502, "maybe_printed"),
        Fault::Failed => (502, "printer_error"),
    }
}

fn paper_header(shared: &Shared, request: &Request) -> Result<Paper, String> {
    match header(request, "x-paper-mm") {
        None => Ok(shared.snapshot().paper),
        Some(value) => {
            let parsed = value.parse::<u16>().ok().and_then(Paper::from_millimetres);
            parsed.ok_or_else(|| "X-Paper-Mm must be 80 or 58".to_string())
        }
    }
}

/// Status for the till page. The pairing code is never included.
///
/// Any website open in the browser can call this. Without the code it gets
/// only a coarse state. The status line can hold a LAN printer address or an
/// error, so only the paired till page sees it.
fn status_json(snapshot: &Snapshot, paired: bool) -> Vec<u8> {
    let mut body = serde_json::json!({
        "ok": true,
        "paired": paired,
        "port": snapshot.port,
        "paper_mm": snapshot.paper.millimetres(),
        "transport": match snapshot.transport {
            Transport::Usb => "usb",
            Transport::Tcp => "tcp",
        },
        "state": match snapshot.tone {
            Tone::Ready => "ready",
            Tone::Busy => "busy",
            Tone::Problem => "problem",
        },
        "raw_enabled": snapshot.allow_raw,
    });
    if paired {
        body["printers"] = serde_json::json!(
            snapshot
                .printers
                .iter()
                .map(|(_, label)| label)
                .collect::<Vec<_>>()
        );
        body["status"] = serde_json::json!(snapshot.status);
    }
    serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec())
}

/// `reason` is a fixed word a page can branch on. `error` is for people.
fn error_json(message: &str, reason: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"ok": false, "error": message, "reason": reason}))
        .unwrap_or_default()
}

fn bearer(request: &Request) -> Option<String> {
    let value = header(request, "authorization")?;
    if value.len() > 80 {
        return None;
    }
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        Some(token.trim().to_string())
    } else {
        None
    }
}

fn note_caller(shared: &Shared, request: &Request) {
    shared.note_client(&caller_label(request));
}

/// How the page names itself, or its browser when it does not. Not trusted.
fn caller_label(request: &Request) -> String {
    crate::model::client_label(
        header(request, "x-till-client").as_deref(),
        header(request, "user-agent").as_deref(),
    )
}

/// `127.0.0.1:<port>` or `localhost:<port>`. Anything else came through a name
/// this app does not own.
fn local_host(host: Option<&str>, port: u16) -> bool {
    let Some((name, given)) = host.and_then(|host| host.rsplit_once(':')) else {
        return false;
    };
    let given = given.parse::<u16>().ok();
    // On a Mac the HTTPS front forwards here with its own port in `Host`.
    #[cfg(target_os = "macos")]
    let is_ours = given == Some(port) || given == crate::tls::secure_port(port);
    #[cfg(not(target_os = "macos"))]
    let is_ours = given == Some(port);
    is_ours && (name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost"))
}

fn header(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|item| item.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|item| item.value.as_str().trim().to_string())
}

fn respond(request: Request, status: StatusCode, body: Vec<u8>) -> Result<(), ()> {
    let length = body.len();
    let mut response = Response::new(status, Vec::new(), Cursor::new(body), Some(length), None);
    for (name, value) in [
        ("Content-Type", "application/json"),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
        (
            "Access-Control-Allow-Headers",
            "authorization, content-type, x-paper-mm, x-cut, x-drawer, x-dither, x-till-client",
        ),
        ("Access-Control-Allow-Private-Network", "true"),
        ("Access-Control-Max-Age", "600"),
    ] {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    request.respond(response).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DEFAULT_PORT;

    #[test]
    fn a_chunked_request_is_refused_before_it_is_read() {
        // tiny_http 0.12 lets `Transfer-Encoding` override `Content-Length`
        // (CVE-2026-66752). No browser needs it, so it never reaches a handler.
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("tapprint-chunked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shared = Shared::load_from(dir.join("config.json"), None).unwrap();
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            client,
            "POST /v1/print HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n0\r\n\r\n"
        )
        .unwrap();
        handle(&shared, port, server.recv().unwrap());
        let mut reply = [0u8; 12];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"HTTP/1.1 400");
    }

    /// The real listener and workers on a free port, with fresh settings.
    fn running(name: &str) -> (Arc<Shared>, u16) {
        let dir = std::env::temp_dir().join(format!("tapprint-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shared = Arc::new(Shared::load_from(dir.join("config.json"), None).unwrap());
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        spawn_workers(server, &shared, port, name).unwrap();
        (shared, port)
    }

    /// One HTTP call, as a browser would make it. Returns the status, the
    /// header block in lowercase, and the JSON body.
    fn call(
        port: u16,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, String, serde_json::Value) {
        use std::io::Write;
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        client.write_all(request.as_bytes()).unwrap();
        // The listener may answer and close before a large body is all sent.
        let _ = client.write_all(body);
        let mut reply = Vec::new();
        let _ = client.read_to_end(&mut reply);
        let reply = String::from_utf8_lossy(&reply).into_owned();
        let (head, body) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let json = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        (status, head.to_ascii_lowercase(), json)
    }

    #[test]
    fn a_site_pairs_prints_and_unpairs_over_http() {
        // The whole path a till page takes, through the real listener.
        let (shared, port) = running("flow");
        let shop = ("Origin", "https://shop.example");

        let (status, _, body) = call(port, "GET", "/v1/status", &[shop], b"");
        assert_eq!(status, 200);
        assert_eq!(body["paired"], false);
        assert!(body.get("printers").is_none());

        // The person at the till clicks Allow while the request waits.
        let cashier = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || {
                for _ in 0..1_000 {
                    if let Some(request) = shared.snapshot().pair_request {
                        assert_eq!(request.origin, "https://shop.example");
                        shared.answer_pair(request.serial, true);
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                panic!("no pairing request showed up");
            })
        };
        let named = ("X-Till-Client", "MyShop");
        let (status, _, body) = call(port, "POST", "/v1/pair", &[shop, named], b"");
        cashier.join().unwrap();
        assert_eq!(status, 200, "{body}");
        let code = body["code"].as_str().unwrap().to_string();
        let bearer = format!("Bearer {code}");
        let auth = ("Authorization", bearer.as_str());

        let (_, _, body) = call(port, "GET", "/v1/status", &[shop, auth], b"");
        assert_eq!(body["paired"], true);
        assert!(body["printers"].is_array());

        // The code is the shop's. From any other site it is worth nothing.
        let thief = ("Origin", "https://evil.example");
        let png = ("Content-Type", "image/png");
        let (status, _, body) = call(port, "POST", "/v1/print", &[thief, auth, png], b"x");
        assert_eq!(
            (status, body["reason"].as_str()),
            (401, Some("unauthorized"))
        );
        let (status, _, body) = call(port, "POST", "/v1/unpair", &[thief, auth], b"");
        assert_eq!((status, body["reason"].as_str()), (401, Some("not_paired")));

        // From the shop it is let in. These fail on the content, before any
        // printer is touched.
        let (status, _, body) = call(port, "POST", "/v1/print", &[shop, auth, png], b"not a png");
        assert_eq!(
            (status, body["reason"].as_str()),
            (400, Some("bad_request"))
        );
        let pdf = ("Content-Type", "application/pdf");
        let (status, _, body) = call(port, "POST", "/v1/print", &[shop, auth, pdf], b"x");
        assert_eq!(
            (status, body["reason"].as_str()),
            (415, Some("unsupported_type"))
        );
        let raw = ("Content-Type", "application/vnd.escpos");
        let (status, _, body) = call(port, "POST", "/v1/print", &[shop, auth, raw], b"x");
        assert_eq!(
            (status, body["reason"].as_str()),
            (403, Some("raw_disabled"))
        );
        let wide = ("X-Paper-Mm", "72");
        let (status, _, _) = call(port, "POST", "/v1/print", &[shop, auth, png, wide], b"x");
        assert_eq!(status, 400);
        let huge = vec![0u8; MAX_BODY + 1];
        let (status, _, body) = call(port, "POST", "/v1/print", &[shop, auth, png], &huge);
        assert_eq!((status, body["reason"].as_str()), (413, Some("too_large")));

        let (status, _, body) = call(port, "POST", "/v1/unpair", &[shop, auth], b"");
        assert_eq!((status, &body["ok"]), (200, &serde_json::Value::Bool(true)));
        let (status, _, _) = call(port, "POST", "/v1/print", &[shop, auth, png], b"x");
        assert_eq!(status, 401);
        assert!(shared.snapshot().sites.is_empty());
    }

    #[test]
    fn a_browser_preflight_is_answered_and_strangers_are_turned_away() {
        let (_shared, port) = running("edges");
        // Chrome asks first, because the page sends headers of its own.
        let (status, head, _) = call(port, "OPTIONS", "/v1/print", &[], b"");
        assert_eq!(status, 204);
        assert!(head.contains("access-control-allow-origin: *"));
        assert!(head.contains("access-control-allow-private-network: true"));
        for header in ["authorization", "content-type", "x-drawer", "x-till-client"] {
            assert!(head.contains(header), "{header} is not allowed: {head}");
        }

        let (status, _, body) = call(port, "GET", "/v1/nothing", &[], b"");
        assert_eq!((status, body["reason"].as_str()), (404, Some("not_found")));
        // Pairing is for web pages. A program has the code in the window.
        let (status, _, body) = call(port, "POST", "/v1/pair", &[], b"");
        assert_eq!(
            (status, body["reason"].as_str()),
            (400, Some("bad_request"))
        );
        let png = ("Content-Type", "image/png");
        let (status, _, body) = call(port, "POST", "/v1/print", &[png], b"x");
        assert_eq!(
            (status, body["reason"].as_str()),
            (401, Some("unauthorized"))
        );

        // A site that points its own name at this computer is refused.
        use std::io::Write;
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            client,
            "GET /v1/status HTTP/1.1\r\nHost: evil.example:{port}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut reply = [0u8; 12];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"HTTP/1.1 421");
    }

    #[test]
    fn a_full_print_queue_leaves_workers_free_for_status() {
        // Eight prints stuck behind a stalled printer used to hold every
        // worker, so the page could not even read the status. The queue is
        // now shorter than the worker pool, and a place comes back when a
        // job ends.
        let held: Vec<PrintSlot> = std::iter::from_fn(PrintSlot::take).take(WORKERS).collect();
        assert_eq!(held.len(), MAX_PRINTS);
        assert!(PrintSlot::take().is_none());
        drop(held);
        assert!(PrintSlot::take().is_some());
    }

    #[test]
    fn only_this_computer_names_are_accepted_as_host() {
        assert!(local_host(Some("127.0.0.1:9240"), 9240));
        assert!(local_host(Some("localhost:9240"), 9240));
        assert!(!local_host(Some("127.0.0.1:9999"), 9240));
        // A rebound domain still carries its own name in the Host header.
        assert!(!local_host(Some("evil.example:9240"), 9240));
        assert!(!local_host(Some("127.0.0.1"), 9240));
        assert!(!local_host(None, 9240));
    }

    #[test]
    fn status_does_not_include_the_pairing_code() {
        let code = "ab".repeat(16);
        let snapshot = Snapshot {
            port: DEFAULT_PORT,
            pairing_code: code.clone(),
            paper: Paper::Mm80,
            transport: Transport::Usb,
            tcp_host: String::new(),
            tcp_port: 9100,
            usb_id: None,
            listed: true,
            printer_setup: crate::model::PrinterSetup::SelectUsb,
            tone: Tone::Problem,
            status: "192.168.1.50:9100: connection refused".to_string(),
            config_path: String::new(),
            printers: vec![("id".to_string(), "USB 04b8:0202".to_string())],
            clients: Vec::new(),
            jobs: 0,
            last_job: None,
            allow_raw: false,
            drawer: [0x1b, 0x70, 0, 25, 25],
            drawer_enabled: true,
            drawer_pin: 2,
            https_only: false,
            check_updates: true,
            share_on: false,
            share_port: 9100,
            share_lan: false,
            update: None,
            sites: vec!["https://shop.example".to_string()],
            pair_request: None,
            server: crate::model::ServerView::default(),
        };
        let paired = String::from_utf8(status_json(&snapshot, true)).unwrap();
        assert!(!paired.contains(&code));
        assert!(paired.contains("USB 04b8:0202"));
        assert!(paired.contains("192.168.1.50"));
        assert!(paired.contains("\"state\":\"problem\""));

        // Another website in the same browser learns only that there is a problem.
        let stranger = String::from_utf8(status_json(&snapshot, false)).unwrap();
        assert!(!stranger.contains(&code));
        assert!(!stranger.contains("192.168.1.50"));
        assert!(!stranger.contains("USB 04b8:0202"));
        assert!(stranger.contains("\"state\":\"problem\""));
        assert!(!stranger.contains("shop.example"));
    }

    #[test]
    fn the_status_code_tells_the_page_whether_a_retry_is_safe() {
        // 503 means nothing printed, so the page may print again after
        // paper is loaded. 502 maybe_printed means a retry could double up.
        let failure = |fault| job_failure(&JobError::new(fault, "x"));
        assert_eq!(failure(Fault::PaperOut), (503, "paper_out"));
        assert_eq!(failure(Fault::Partial), (502, "maybe_printed"));
        assert_eq!(failure(Fault::Failed), (502, "printer_error"));
        assert_eq!(failure(Fault::BadInput), (400, "bad_request"));
        assert_eq!(pair_failure(&PairError::Blocked).0, 429);
        assert_eq!(pair_failure(&PairError::Busy).0, 409);
    }
}
