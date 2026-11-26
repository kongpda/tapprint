//! Print from a server: print the jobs that people send to this computer
//! through a print server, from any phone or laptop. The server is fixed when
//! TapPrint is built (`TAPPRINT_PRINT_SERVER`); the default is khevents.app.
//! The protocol is in `docs/print-station-protocol.md`.
//!
//! Pairing swaps a one-time code from the Printers page for a station key.
//! With it, TapPrint asks for its next job every couple of seconds, downloads
//! each slip as a PNG, prints it exactly like a local print, and tells the
//! server what the printer did.
//!
//! The server never sends printer commands: only pictures, which are checked
//! before they print. The key opens nothing but this printer's own jobs.
//!
//! Print at most once: a slip that may have reached the paper is reported as
//! such and never printed again by TapPrint. The server lets a person decide.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::model::{Fault, JobError, Link, Paper, ServerOutcome, Shared, Tone};
use crate::print;

/// The print server this build talks to. Set it when building:
/// `TAPPRINT_PRINT_SERVER=https://print.example.com cargo build --release`.
/// It is built in, not a setting, so no page or person at the till can send
/// the station key somewhere else.
pub const DEFAULT_SERVER: &str = "https://khevents.app";
const SERVER: &str = match option_env!("TAPPRINT_PRINT_SERVER") {
    Some(address) => address,
    None => DEFAULT_SERVER,
};

/// The server's name in the window, such as "KHEvents".
pub const SERVER_NAME: &str = match option_env!("TAPPRINT_PRINT_SERVER_NAME") {
    Some(name) => name,
    None => "KHEvents",
};

/// The key is a password: a release build only sends it over HTTPS.
const _: () = assert!(
    starts_with(SERVER.as_bytes(), b"https://"),
    "TAPPRINT_PRINT_SERVER must start with https://"
);

const fn starts_with(text: &[u8], prefix: &[u8]) -> bool {
    if text.len() < prefix.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if text[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// True when this build talks to khevents.app, so the window can name its pages.
pub fn is_khevents() -> bool {
    host() == "khevents.app"
}

/// The server's address without `https://`, for messages: "khevents.app".
pub fn host() -> &'static str {
    SERVER.trim_start_matches("https://").trim_end_matches('/')
}
const API: &str = "/api/v1/print-station";

/// A slip is at most 1.5 MB on the server. Anything far larger is not ours.
const MAX_SLIP_BYTES: u64 = 2 * 1024 * 1024;
const MAX_REPLY_BYTES: u64 = 64 * 1024;
/// The server allows 40 slips a job.
const MAX_SLIPS: u32 = 60;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// How often to look for a key while not connected, or paused here.
const IDLE: Duration = Duration::from_secs(1);
/// While paused here, still say "online" this often, so the Printers page
/// does not show the computer as gone.
const PAUSED_HEARTBEAT: Duration = Duration::from_secs(10);
const LONGEST_WAIT: Duration = Duration::from_secs(30);
const DISABLED_WAIT: Duration = Duration::from_secs(60);

const REMOVED: &str =
    "This printer was removed or connected again elsewhere. Make a new code on the Printers page.";

/// Where the print server is. A debug build can point at a local server.
pub fn server() -> String {
    if cfg!(debug_assertions)
        && let Ok(address) = std::env::var("TAPPRINT_SERVER")
    {
        return address.trim_end_matches('/').to_string();
    }
    SERVER.trim_end_matches('/').to_string()
}

/// What one call to the server came back with.
#[derive(Debug)]
enum Reply {
    Ok(Vec<u8>),
    /// The key no longer works.
    Unauthorized,
    /// Server printing is off (every call answers 404).
    Disabled,
    /// The server understood and said no, with its reason.
    Refused(String),
    /// No answer, or a server error. Worth trying again.
    Down(String),
}

pub struct Client {
    base: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(base: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            // The key must never follow a redirect to another host.
            .max_redirects(0)
            .https_only(!cfg!(debug_assertions))
            .user_agent(concat!("tapprint/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent,
        }
    }

    fn endpoint(&self) -> &str {
        &self.base
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
        limit: u64,
        paper: Paper,
    ) -> Reply {
        let url = format!("{}{API}{path}", self.base);
        let bearer = token.map(|token| format!("Bearer {token}"));
        let result = match (method, body) {
            ("POST", body) => {
                let mut request = self
                    .agent
                    .post(&url)
                    .header("Accept", "application/json")
                    .header("X-TapPrint-Version", env!("CARGO_PKG_VERSION"))
                    .header("X-Paper-Mm", paper.millimetres().to_string());
                if let Some(bearer) = &bearer {
                    request = request.header("Authorization", bearer);
                }
                let bytes = serde_json::to_vec(&body.unwrap_or_default()).unwrap_or_default();
                request.content_type("application/json").send(&bytes[..])
            }
            (method, _) => {
                let mut request = if method == "DELETE" {
                    self.agent.delete(&url)
                } else {
                    self.agent.get(&url)
                }
                .header("Accept", "application/json, image/png")
                .header("X-TapPrint-Version", env!("CARGO_PKG_VERSION"))
                .header("X-Paper-Mm", paper.millimetres().to_string());
                if let Some(bearer) = &bearer {
                    request = request.header("Authorization", bearer);
                }
                request.call()
            }
        };
        let mut response = match result {
            Ok(response) => response,
            Err(error) => return Reply::Down(format!("could not reach the server: {error}")),
        };
        let status = response.status().as_u16();
        let mut body = Vec::new();
        if response
            .body_mut()
            .as_reader()
            .take(limit.saturating_add(1))
            .read_to_end(&mut body)
            .is_err()
        {
            return Reply::Down("the server's answer was cut off".to_string());
        }
        if body.len() as u64 > limit {
            return Reply::Refused("the server's answer was too large".to_string());
        }
        match status {
            200..=299 => Reply::Ok(body),
            401 => Reply::Unauthorized,
            404 if path_is_station_wide(path) => Reply::Disabled,
            408 | 429 | 500..=599 => Reply::Down(match status {
                429 => "too many tries. Wait a minute and try again".to_string(),
                _ => format!("the server had a problem ({status})"),
            }),
            _ => Reply::Refused(server_message(&body)),
        }
    }
}

/// A 404 on these means the whole feature is off. On a job or slip it only
/// means that job is gone.
fn path_is_station_wide(path: &str) -> bool {
    matches!(path, "" | "/" | "/next" | "/pair")
}

/// The `message` of the server's error answer, cleaned for the window.
fn server_message(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("message")?.as_str().map(|text| clean(text, 200)))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "the server refused the request".to_string())
}

/// Text from the server, made safe to show: one line, no control characters.
fn clean(text: &str, max: usize) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct Paired {
    token: String,
    station: Named,
    organizer: Named,
}

#[derive(Debug, Deserialize)]
struct Named {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Next {
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    poll_seconds: Option<u64>,
    /// Read on its own, so one job with an odd field is reported and
    /// skipped instead of making every answer unreadable.
    job: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct Job {
    id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    sender: Option<String>,
    #[serde(default)]
    paper_mm: Option<u16>,
    slip_count: u32,
    #[serde(default)]
    first_slip: u32,
}

/// The code as typed, as the server stores it: 8 letters and digits.
pub fn normalized_code(typed: &str) -> Option<String> {
    let code: String = typed
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '-'))
        .map(|ch| ch.to_ascii_uppercase())
        .collect();
    (code.len() == 8 && code.chars().all(|ch| ch.is_ascii_alphanumeric())).then_some(code)
}

/// A job id goes into a URL path, so only a ULID is accepted.
fn valid_id(id: &str) -> bool {
    id.len() == 26 && id.chars().all(|ch| ch.is_ascii_alphanumeric())
}

/// The key goes into a header and the config file, so only a plain token of
/// 16 to 128 letters, digits, `_`, `-` or `.` is kept. KHEvents keys start
/// with `kps_`; other servers may use any such token.
fn well_formed_key(token: &str) -> bool {
    (16..=128).contains(&token.len())
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

/// The name the server shows for this computer.
fn device_name() -> String {
    let os = match std::env::consts::OS {
        "macos" => "Mac",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    format!("TapPrint on {os}")
}

/// Swap a code from the Printers page for this computer's key.
pub fn pair(shared: &Shared, client: &Client, typed: &str) -> Result<(), String> {
    let code = normalized_code(typed)
        .ok_or_else(|| "Type the 8-character code from the Printers page".to_string())?;
    let body = serde_json::json!({
        "code": code,
        "device_name": device_name(),
        "app_version": env!("CARGO_PKG_VERSION"),
    });
    let paper = shared.snapshot().paper;
    match client.call("POST", "/pair", None, Some(body), MAX_REPLY_BYTES, paper) {
        Reply::Ok(body) => {
            let paired: Envelope<Paired> = serde_json::from_slice(&body)
                .map_err(|_| format!("{} gave an answer TapPrint does not understand", host()))?;
            let token = paired.data.token;
            if !well_formed_key(&token) {
                return Err(format!(
                    "{} gave a key TapPrint does not understand",
                    host()
                ));
            }
            shared.connect_server(
                client.endpoint(),
                &token,
                &clean(&paired.data.station.name, 60),
                &clean(&paired.data.organizer.name, 80),
            )
        }
        Reply::Disabled => Err(format!("Server printing is off on {}", host())),
        Reply::Refused(message) => Err(sentence(&message)),
        Reply::Down(message) => Err(sentence(&message)),
        Reply::Unauthorized => Err("That code is wrong or has expired".to_string()),
    }
}

/// Stop printing for the server on this computer. The server is told when
/// it can be reached; the key is forgotten here either way.
pub fn disconnect(shared: &Shared, client: &Client) {
    if let Some(token) = shared.server_token(client.endpoint()) {
        let paper = shared.snapshot().paper;
        shared.disconnect_server(
            client.endpoint(),
            &token,
            &format!("Disconnected from {SERVER_NAME}"),
            false,
        );
        let _ = client.call("DELETE", "/", Some(&token), None, MAX_REPLY_BYTES, paper);
    }
}

fn sentence(text: &str) -> String {
    let mut text = text.trim().trim_end_matches('.').to_string();
    if let Some(first) = text.get(..1) {
        text = first.to_ascii_uppercase() + text.get(1..).unwrap_or_default();
    }
    text
}

/// Look for jobs for as long as the app runs.
pub fn watch(shared: Arc<Shared>) {
    let _ = std::thread::Builder::new()
        .name("tapprint-station".into())
        .spawn(move || {
            let client = Client::new(&server());
            let mut failures = 0u32;
            let mut last_heartbeat: Option<std::time::Instant> = None;
            loop {
                let wait = step(&shared, &client, &mut failures, &mut last_heartbeat);
                std::thread::sleep(wait);
            }
        });
}

/// One look at the server. Returns how long to wait before the next one.
fn step(
    shared: &Shared,
    client: &Client,
    failures: &mut u32,
    last_heartbeat: &mut Option<std::time::Instant>,
) -> Duration {
    let Some(token) = shared.server_token(client.endpoint()) else {
        *failures = 0;
        return IDLE;
    };
    let snapshot = shared.snapshot();
    if snapshot.server.paused {
        // Jobs wait on the server. Still say this computer is here.
        if last_heartbeat.is_some_and(|at| at.elapsed() < PAUSED_HEARTBEAT) {
            return IDLE;
        }
        *last_heartbeat = Some(std::time::Instant::now());
        return match client.call(
            "GET",
            "/",
            Some(&token),
            None,
            MAX_REPLY_BYTES,
            snapshot.paper,
        ) {
            Reply::Ok(_) => {
                shared.set_link(Link::Online, "", false);
                IDLE
            }
            other => settle(shared, client, &token, other, failures),
        };
    }
    match client.call(
        "GET",
        "/next",
        Some(&token),
        None,
        MAX_REPLY_BYTES,
        snapshot.paper,
    ) {
        Reply::Ok(body) => {
            *failures = 0;
            let Ok(next) = serde_json::from_slice::<Envelope<Next>>(&body) else {
                shared.set_link(
                    Link::Offline,
                    format!(
                        "{} gave an answer TapPrint does not understand. Update TapPrint.",
                        host()
                    ),
                    false,
                );
                return LONGEST_WAIT;
            };
            let next = next.data;
            let note = if next.paused {
                "Paused on the Printers page"
            } else {
                ""
            };
            shared.set_link(Link::Online, note, next.paused);
            let poll = Duration::from_secs(next.poll_seconds.unwrap_or(2).clamp(1, 30));
            let Some(job) = next.job else {
                return poll;
            };
            let ran = match serde_json::from_value::<Job>(job.clone()) {
                Ok(job) => run_job(shared, client, &token, &job),
                Err(_) if unreadable_job(shared, client, &token, &job) => Ran::Done,
                Err(_) => Ran::Held,
            };
            match ran {
                // There may be more waiting: look again straight away.
                Ran::Done => Duration::from_millis(200),
                // The server still holds this job, or offers it again until
                // the printer is fixed. Asking again at once would only
                // fetch it again, several times a second.
                Ran::Held | Ran::Retry => poll,
            }
        }
        other => settle(shared, client, &token, other, failures),
    }
}

/// A failed look: say why, and wait longer each time, up to 30 seconds.
fn settle(
    shared: &Shared,
    client: &Client,
    token: &str,
    reply: Reply,
    failures: &mut u32,
) -> Duration {
    match reply {
        Reply::Ok(_) => IDLE,
        Reply::Unauthorized => {
            shared.disconnect_server(client.endpoint(), token, REMOVED, true);
            *failures = 0;
            IDLE
        }
        Reply::Disabled => {
            shared.set_link(
                Link::Disabled,
                format!("Server printing is off on {}", host()),
                false,
            );
            DISABLED_WAIT
        }
        Reply::Refused(message) | Reply::Down(message) => {
            *failures = failures.saturating_add(1);
            shared.set_link(Link::Offline, sentence(&message), false);
            backoff(*failures)
        }
    }
}

fn backoff(failures: u32) -> Duration {
    let seconds = 2u64.saturating_pow(failures.min(5));
    Duration::from_secs(seconds).min(LONGEST_WAIT)
}

/// The server's word for what the printer did.
fn outcome_for(error: &JobError) -> (&'static str, ServerOutcome) {
    match error.fault {
        Fault::PaperOut => ("paper_out", ServerOutcome::Retrying),
        // Nothing reached the printer (unplugged, switched off): safe to retry.
        Fault::Failed => ("printer_busy", ServerOutcome::Retrying),
        Fault::Partial => ("maybe_printed", ServerOutcome::CheckPaper),
        Fault::BadInput => ("printer_error", ServerOutcome::Failed),
    }
}

/// A job whose fields cannot be read. Reported as an error when its id is
/// usable, so the queue moves on. Returns whether a report was sent.
fn unreadable_job(shared: &Shared, client: &Client, token: &str, job: &serde_json::Value) -> bool {
    let Some(id) = job["id"].as_str().filter(|id| valid_id(id)) else {
        // Never put an odd id in a URL. The server's hold on it lapses.
        return false;
    };
    let first_slip = job["first_slip"]
        .as_u64()
        .and_then(|slip| u32::try_from(slip).ok())
        .unwrap_or(0);
    report(
        shared,
        client,
        token,
        id,
        "printer_error",
        first_slip,
        Some("TapPrint could not read this job"),
    );
    shared.note_server_job("Print job", "", ServerOutcome::Failed);
    true
}

/// How a job ended, which sets how soon to look for the next one.
enum Ran {
    /// Reported as finished: the queue has moved on.
    Done,
    /// Reported for another try: the printer or the download failed.
    Retry,
    /// Not reported: the server still holds the job.
    Held,
}

/// Print one job's slips, from where an earlier try stopped, and report.
fn run_job(shared: &Shared, client: &Client, token: &str, job: &Job) -> Ran {
    if !valid_id(&job.id) {
        // Never put an odd id in a URL. The server's hold on it lapses.
        return Ran::Held;
    }
    let summary = match clean(&job.summary, 80) {
        text if text.is_empty() => "Print job".to_string(),
        text => text,
    };
    let sender = job
        .sender
        .as_deref()
        .map(|name| clean(name, 60))
        .unwrap_or_default();
    if job.slip_count == 0 || job.slip_count > MAX_SLIPS || job.first_slip >= job.slip_count {
        report(
            shared,
            client,
            token,
            &job.id,
            "printer_error",
            job.first_slip,
            Some("TapPrint could not read this job"),
        );
        shared.note_server_job(&summary, &sender, ServerOutcome::Failed);
        return Ran::Done;
    }
    let paper = job
        .paper_mm
        .and_then(Paper::from_millimetres)
        .unwrap_or(shared.snapshot().paper);
    let options = print::Options {
        paper,
        cut: true,
        drawer: false,
        dither: false,
    };
    let mut printed = job.first_slip;
    for position in job.first_slip..job.slip_count {
        let path = format!("/jobs/{}/slips/{position}", job.id);
        let png = match client.call("GET", &path, Some(token), None, MAX_SLIP_BYTES, paper) {
            Reply::Ok(png) => png,
            Reply::Unauthorized => {
                shared.disconnect_server(client.endpoint(), token, REMOVED, true);
                return Ran::Held;
            }
            Reply::Down(message) => {
                // Nothing of this slip printed. If the report cannot get
                // through either, the server marks the job Check paper.
                report(
                    shared,
                    client,
                    token,
                    &job.id,
                    "printer_busy",
                    printed,
                    Some(&format!("Could not download the slip: {message}")),
                );
                shared.note_server_job(&summary, &sender, ServerOutcome::Retrying);
                return Ran::Retry;
            }
            Reply::Disabled | Reply::Refused(_) => {
                // The job was cancelled or taken back. Stop here, but say
                // which slips already printed, so none is offered again.
                shared.note_server_job(&summary, &sender, ServerOutcome::Failed);
                if printed > job.first_slip {
                    report(
                        shared,
                        client,
                        token,
                        &job.id,
                        "printer_error",
                        printed,
                        Some("The server refused the next slip"),
                    );
                    return Ran::Done;
                }
                return Ran::Held;
            }
        };
        if !png.starts_with(PNG_SIGNATURE) {
            report(
                shared,
                client,
                token,
                &job.id,
                "printer_error",
                printed,
                Some("The server sent something that is not a picture"),
            );
            shared.note_server_job(&summary, &sender, ServerOutcome::Failed);
            return Ran::Done;
        }
        if let Err(error) = print::print_png(shared, &png, options) {
            let (outcome, shown) = outcome_for(&error);
            report(
                shared,
                client,
                token,
                &job.id,
                outcome,
                printed,
                Some(&error.message),
            );
            shared.note_server_job(&summary, &sender, shown);
            return match shown {
                ServerOutcome::Retrying => Ran::Retry,
                _ => Ran::Done,
            };
        }
        printed += 1;
    }
    report(shared, client, token, &job.id, "printed", printed, None);
    shared.note_server_job(&summary, &sender, ServerOutcome::Printed);
    let from = if sender.is_empty() {
        String::new()
    } else {
        format!(" from {sender}")
    };
    shared.set_status(Tone::Ready, format!("Printed {summary}{from}"));
    Ran::Done
}

/// Tell the server what happened. Tried three times: a lost "printed"
/// would leave the job for a person to check, never print it twice.
fn report(
    shared: &Shared,
    client: &Client,
    token: &str,
    id: &str,
    outcome: &str,
    printed: u32,
    message: Option<&str>,
) {
    let body = serde_json::json!({
        "outcome": outcome,
        "printed_slips": printed,
        "message": message.map(|text| clean(text, 400)),
    });
    let path = format!("/jobs/{id}/result");
    let paper = shared.snapshot().paper;
    for wait in [0u64, 1, 3] {
        std::thread::sleep(Duration::from_secs(wait));
        match client.call(
            "POST",
            &path,
            Some(token),
            Some(body.clone()),
            MAX_REPLY_BYTES,
            paper,
        ) {
            Reply::Down(_) => continue,
            Reply::Unauthorized => {
                shared.disconnect_server(client.endpoint(), token, REMOVED, true);
                return;
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Transport;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Mutex;

    const ID: &str = "01JD3V4X5Y6Z7A8B9C0D1E2F3G";

    #[test]
    fn only_a_plain_key_is_kept() {
        assert!(well_formed_key(&format!("kps_{}", "a1".repeat(24))));
        assert!(well_formed_key("0123456789abcdef-._"));
        // Too short to be a real secret.
        assert!(!well_formed_key("kps_short"));
        // Would break the Authorization header or the config file.
        assert!(!well_formed_key("kps_aaaaaaaaaaaaaaaa\r\nX-Evil: 1"));
        assert!(!well_formed_key("kps_aaaaaaaaaaaaaaaa bbb"));
        assert!(!well_formed_key(&"a".repeat(129)));
    }

    #[test]
    fn the_built_in_server_is_https_and_named() {
        assert!(server().starts_with("https://") || cfg!(debug_assertions));
        assert!(!host().is_empty() && !host().contains('/'));
        assert!(!SERVER_NAME.is_empty());
    }

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        let image = image::GrayImage::from_pixel(64, 24, image::Luma([0]));
        image::DynamicImage::ImageLuma8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// A stand-in for khevents.app. Answers each path from `routes`, and
    /// keeps every request it got as (method, path, body, authorization).
    struct FakeServer {
        base: String,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    /// Method, path, body and authorization of one request.
    type Seen = (String, String, String, String);

    type Route = (&'static str, &'static str, u16, Vec<u8>);

    fn fake_server(routes: Vec<Route>) -> FakeServer {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let method = request.method().to_string();
                let path = request.url().to_string();
                let auth = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Authorization"))
                    .map(|header| header.value.to_string())
                    .unwrap_or_default();
                log.lock()
                    .unwrap()
                    .push((method.clone(), path.clone(), body, auth));
                let found = routes.iter().find(|(want_method, want_path, _, _)| {
                    *want_method == method && path == format!("{API}{want_path}")
                });
                let response = match found {
                    Some((_, _, status, body)) => {
                        tiny_http::Response::from_data(body.clone()).with_status_code(*status)
                    }
                    None => tiny_http::Response::from_data(b"{}".to_vec()).with_status_code(404),
                };
                let _ = request.respond(response);
            }
        });
        FakeServer { base, seen }
    }

    /// A network printer that keeps everything it is sent.
    fn fake_printer(jobs: usize) -> (u16, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut got = Vec::new();
            while got.len() < jobs {
                let (mut socket, _) = listener.accept().unwrap();
                let mut query = [0u8; 3];
                if socket.read_exact(&mut query).is_err() {
                    continue;
                }
                // Paper is in.
                let _ = socket.write_all(&[0x12]);
                let mut rest = Vec::new();
                let _ = socket.read_to_end(&mut rest);
                if rest.len() > 16 {
                    got.push(rest);
                }
            }
            got
        });
        (port, handle)
    }

    fn shared(name: &str, printer_port: Option<u16>) -> Shared {
        let dir =
            std::env::temp_dir().join(format!("tapprint-station-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shared = Shared::load_from(dir.join("config.json"), None).unwrap();
        match printer_port {
            Some(port) => shared.set_network("127.0.0.1", port).unwrap(),
            // Port 9 on this computer: nothing listens, so nothing can print.
            None => shared.set_network("127.0.0.1", 9).unwrap(),
        }
        shared.set_transport(Transport::Tcp).unwrap();
        shared
    }

    fn next_with(job: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(
            &serde_json::json!({"data": {"paused": false, "poll_seconds": 2, "job": job}}),
        )
        .unwrap()
    }

    fn results(server: &FakeServer) -> Vec<serde_json::Value> {
        server
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, path, _, _)| method == "POST" && path.ends_with("/result"))
            .map(|(_, _, body, _)| serde_json::from_str(body).unwrap())
            .collect()
    }

    #[test]
    fn codes_are_read_the_way_people_type_them() {
        assert_eq!(normalized_code("k7qm-3xpa").as_deref(), Some("K7QM3XPA"));
        assert_eq!(normalized_code(" K7QM 3XPA ").as_deref(), Some("K7QM3XPA"));
        assert_eq!(normalized_code("K7QM-3XP"), None);
        assert_eq!(normalized_code("K7QM/3XPA"), None);
    }

    #[test]
    fn only_a_ulid_goes_into_a_job_address() {
        assert!(valid_id(ID));
        assert!(!valid_id("../../next"));
        assert!(!valid_id("01JD3V4X5Y6Z7A8B9C0D1E2F3"));
        assert!(!valid_id("01JD3V4X5Y6Z7A8B9C0D1E2F3G?x"));
    }

    #[test]
    fn server_text_is_one_short_line() {
        assert_eq!(
            clean("Receipt\n\u{1b}[31m · Order #12", 80),
            "Receipt [31m · Order #12"
        );
        assert_eq!(clean(&"a".repeat(300), 10).len(), 10);
    }

    #[test]
    fn a_printer_fault_maps_to_a_safe_server_outcome() {
        // Only "nothing reached the paper" may be retried by the server.
        let fault = |fault| outcome_for(&JobError::new(fault, "x")).0;
        assert_eq!(fault(Fault::PaperOut), "paper_out");
        assert_eq!(fault(Fault::Failed), "printer_busy");
        assert_eq!(fault(Fault::Partial), "maybe_printed");
        assert_eq!(fault(Fault::BadInput), "printer_error");
    }

    #[test]
    fn waits_grow_after_failures_and_stop_at_thirty_seconds() {
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(2), Duration::from_secs(4));
        assert_eq!(backoff(4), Duration::from_secs(16));
        assert_eq!(backoff(9), LONGEST_WAIT);
    }

    #[test]
    fn pairing_saves_the_key_and_names_but_never_shows_the_key() {
        let paired = serde_json::json!({"data": {
            "token": format!("kps_{}", "a1".repeat(24)),
            "station": {"id": "x", "name": "Counter Mac", "paused": false},
            "organizer": {"name": "Arts Dept", "slug": "arts"},
            "poll_seconds": 2,
        }});
        let server = fake_server(vec![(
            "POST",
            "/pair",
            201,
            serde_json::to_vec(&paired).unwrap(),
        )]);
        let shared = shared("pair", None);
        pair(&shared, &Client::new(&server.base), "abcd-efgh").unwrap();
        let view = shared.snapshot().server;
        assert!(view.connected);
        assert_eq!(view.station, "Counter Mac");
        assert_eq!(view.organizer, "Arts Dept");
        assert!(
            shared
                .server_token(&server.base)
                .unwrap()
                .starts_with("kps_")
        );
        let sent = &server.seen.lock().unwrap()[0];
        assert!(sent.2.contains("\"code\":\"ABCDEFGH\""));
    }

    #[test]
    fn a_wrong_code_shows_the_servers_reason() {
        let refused = br#"{"message":"That code is wrong or has expired. Make a new one on the Printers page.","code":"x"}"#;
        let server = fake_server(vec![("POST", "/pair", 422, refused.to_vec())]);
        let shared = shared("wrong-code", None);
        let error = pair(&shared, &Client::new(&server.base), "ABCD-EFGH").unwrap_err();
        assert!(error.starts_with("That code is wrong"));
        assert!(!shared.snapshot().server.connected);
    }

    #[test]
    fn a_job_prints_every_slip_and_reports_printed() {
        let (port, printer) = fake_printer(2);
        let job = serde_json::json!({"id": ID, "summary": "Receipt · Order #12", "sender": "Dara", "copy": 1, "paper_mm": 80, "slip_count": 2, "first_slip": 0});
        let server = fake_server(vec![
            ("GET", "/next", 200, next_with(job)),
            (
                "GET",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/slips/0",
                200,
                png(),
            ),
            (
                "GET",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/slips/1",
                200,
                png(),
            ),
            (
                "POST",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/result",
                200,
                b"{}".to_vec(),
            ),
        ]);
        let shared = shared("prints", Some(port));
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        let (mut failures, mut beat) = (0, None);
        step(
            &shared,
            &Client::new(&server.base),
            &mut failures,
            &mut beat,
        );
        assert_eq!(printer.join().unwrap().len(), 2);
        let reported = results(&server);
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0]["outcome"], "printed");
        assert_eq!(reported[0]["printed_slips"], 2);
        let view = shared.snapshot().server;
        assert_eq!(view.link, Link::Online);
        assert_eq!(view.recent[0].sender, "Dara");
        assert_eq!(view.recent[0].outcome, ServerOutcome::Printed);
        // Every call carries the key.
        assert!(
            server
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|seen| seen.3 == "Bearer kps_test")
        );
    }

    #[test]
    fn a_retry_starts_after_the_slips_already_printed() {
        let (port, printer) = fake_printer(1);
        let job =
            serde_json::json!({"id": ID, "summary": "Tickets", "slip_count": 3, "first_slip": 2});
        let server = fake_server(vec![
            ("GET", "/next", 200, next_with(job)),
            (
                "GET",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/slips/2",
                200,
                png(),
            ),
            (
                "POST",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/result",
                200,
                b"{}".to_vec(),
            ),
        ]);
        let shared = shared("resume", Some(port));
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        assert_eq!(printer.join().unwrap().len(), 1);
        let asked: Vec<String> = server
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|seen| seen.1.clone())
            .collect();
        assert!(
            !asked
                .iter()
                .any(|path| path.ends_with("/slips/0") || path.ends_with("/slips/1"))
        );
        assert_eq!(results(&server)[0]["printed_slips"], 3);
    }

    #[test]
    fn a_printer_that_cannot_be_reached_is_reported_for_a_retry() {
        let job =
            serde_json::json!({"id": ID, "summary": "Receipt", "slip_count": 1, "first_slip": 0});
        let server = fake_server(vec![
            ("GET", "/next", 200, next_with(job)),
            (
                "GET",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/slips/0",
                200,
                png(),
            ),
            (
                "POST",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/result",
                200,
                b"{}".to_vec(),
            ),
        ]);
        let shared = shared("unreachable", None);
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        let reported = results(&server);
        assert_eq!(reported[0]["outcome"], "printer_busy");
        assert_eq!(reported[0]["printed_slips"], 0);
        assert_eq!(
            shared.snapshot().server.recent[0].outcome,
            ServerOutcome::Retrying
        );
    }

    #[test]
    fn something_that_is_not_a_png_never_reaches_the_printer() {
        let job =
            serde_json::json!({"id": ID, "summary": "Receipt", "slip_count": 1, "first_slip": 0});
        let server = fake_server(vec![
            ("GET", "/next", 200, next_with(job)),
            (
                "GET",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/slips/0",
                200,
                b"\x1b@\x1dV\x00".to_vec(),
            ),
            (
                "POST",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/result",
                200,
                b"{}".to_vec(),
            ),
        ]);
        let shared = shared("not-png", None);
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        assert_eq!(results(&server)[0]["outcome"], "printer_error");
    }

    #[test]
    fn one_unreadable_job_is_reported_and_does_not_block_the_queue() {
        // `null` where a text belongs used to make the whole answer
        // unreadable, so the same job came back forever and nothing printed.
        let job = serde_json::json!({"id": ID, "summary": null, "slip_count": 1});
        let server = fake_server(vec![
            ("GET", "/next", 200, next_with(job)),
            (
                "POST",
                "/jobs/01JD3V4X5Y6Z7A8B9C0D1E2F3G/result",
                200,
                b"{}".to_vec(),
            ),
        ]);
        let shared = shared("unreadable-job", None);
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        let wait = step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        assert_eq!(results(&server)[0]["outcome"], "printer_error");
        // Reported, so the next job is asked for at once.
        assert!(wait < Duration::from_secs(1));
    }

    #[test]
    fn a_job_that_cannot_be_reported_is_not_fetched_again_at_once() {
        // An id that cannot go into a URL is never reported. Asking again
        // straight away would fetch the same job several times a second.
        let job = serde_json::json!({"id": "../../x", "slip_count": 1});
        let server = fake_server(vec![("GET", "/next", 200, next_with(job))]);
        let shared = shared("bad-id", None);
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        let wait = step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        assert!(wait >= Duration::from_secs(1));
        assert!(results(&server).is_empty());
    }

    #[test]
    fn a_removed_printer_forgets_its_key() {
        let server = fake_server(vec![("GET", "/next", 401, b"{}".to_vec())]);
        let shared = shared("removed", None);
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        assert!(shared.server_token(&server.base).is_none());
        assert!(shared.snapshot().status.contains("removed"));
    }

    #[test]
    fn a_reply_to_an_old_key_does_not_drop_a_new_one() {
        let shared = shared("old-key", None);
        shared
            .connect_server(DEFAULT_SERVER, "kps_new", "Counter", "Org")
            .unwrap();
        shared.disconnect_server(DEFAULT_SERVER, "kps_old", REMOVED, true);
        assert_eq!(
            shared.server_token(DEFAULT_SERVER).as_deref(),
            Some("kps_new")
        );
    }

    #[test]
    fn a_client_for_a_different_endpoint_cannot_use_the_station_key() {
        let server = fake_server(vec![]);
        let shared = shared("different-endpoint", None);
        shared
            .connect_server(DEFAULT_SERVER, "kps_test", "Counter", "Org")
            .unwrap();
        let client = Client::new(&server.base);
        assert_eq!(step(&shared, &client, &mut 0, &mut None), IDLE);
        disconnect(&shared, &client);
        assert!(server.seen.lock().unwrap().is_empty());
        assert_eq!(
            shared.server_token(DEFAULT_SERVER).as_deref(),
            Some("kps_test")
        );
    }

    #[test]
    fn server_printing_off_waits_a_minute_and_says_so() {
        let server = fake_server(vec![]);
        let shared = shared("disabled", None);
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        let wait = step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        assert_eq!(wait, DISABLED_WAIT);
        assert_eq!(shared.snapshot().server.link, Link::Disabled);
        // Still connected: turning it on again needs no new code.
        assert!(shared.server_token(&server.base).is_some());
    }

    #[test]
    fn paused_here_takes_no_jobs_but_still_says_it_is_here() {
        let job =
            serde_json::json!({"id": ID, "summary": "Receipt", "slip_count": 1, "first_slip": 0});
        let server = fake_server(vec![
            ("GET", "/next", 200, next_with(job)),
            ("GET", "/", 200, b"{}".to_vec()),
        ]);
        let shared = shared("paused", None);
        shared
            .connect_server(&server.base, "kps_test", "Counter", "Org")
            .unwrap();
        shared.set_server_paused(true);
        step(&shared, &Client::new(&server.base), &mut 0, &mut None);
        let asked: Vec<String> = server
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|seen| seen.1.clone())
            .collect();
        assert_eq!(asked, vec![format!("{API}/")]);
    }

    /// Against a real KHEvents with a waiting job (see docs/api.md):
    /// `TAPPRINT_E2E_SERVER=http://127.0.0.1:8000 TAPPRINT_E2E_CODE=ABCD-EFGH
    /// cargo test -- --ignored against_a_running_khevents`
    #[test]
    #[ignore = "needs a running KHEvents with a station code and a waiting job"]
    fn against_a_running_khevents() {
        let base = std::env::var("TAPPRINT_E2E_SERVER").unwrap();
        let code = std::env::var("TAPPRINT_E2E_CODE").unwrap();
        let slips: usize = std::env::var("TAPPRINT_E2E_SLIPS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1);
        let (port, printer) = fake_printer(slips);
        let shared = shared("e2e", Some(port));
        let client = Client::new(&base);
        pair(&shared, &client, &code).unwrap();
        step(&shared, &client, &mut 0, &mut None);
        assert_eq!(printer.join().unwrap().len(), slips);
        assert_eq!(
            shared.snapshot().server.recent[0].outcome,
            ServerOutcome::Printed
        );
        // Nothing left: the next look is empty, and the key still works.
        step(&shared, &client, &mut 0, &mut None);
        assert_eq!(shared.snapshot().server.link, Link::Online);
        disconnect(&shared, &client);
        assert!(shared.server_token(&base).is_none());
    }
}
