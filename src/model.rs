//! Pairing code, paper size, and which printer the till is using.
//!
//! The code is generated once and kept in the config file, mode 0600 on Unix.
//! The browser must send it back with every print. The status endpoint never
//! returns it. A short code is replaced on load: a six-digit code can be
//! guessed by any page that can reach this computer. A 128-bit code cannot,
//! so wrong codes are not locked out. A lockout would only let a hostile page
//! in another tab block the real till.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use crate::escpos;
pub use tapprint_core::{Fault, JobError, Paper, check_host};

pub const DEFAULT_PORT: u16 = 9240;
/// The port network receipt printers answer on, and so the one other apps expect.
pub const SHARE_PORT: u16 = 9100;
const CODE_BYTES: usize = 16;
/// A site the cashier turned down cannot ask again for this long, so a
/// hostile page cannot keep the prompt on screen.
const DENY_FOR: Duration = Duration::from_secs(600);
/// A site that asked and got no answer waits this long before asking again,
/// so a page cannot pop the window up over and over.
const UNANSWERED_FOR: Duration = Duration::from_secs(60);
/// After any No or timeout, no site may ask for this long. One page can own
/// many origins, and must not be able to keep a prompt up by rotating them.
const PROMPT_GAP: Duration = Duration::from_secs(15);
/// Start of the status shown when USB devices could not be listed.
const USB_SCAN_FAILED: &str = "Could not look for USB printers";
const TYPE_ADDRESS: &str = "Enter the printer's address below";
const NO_PRINTER: &str = "No printer found. Plug in your printer and switch it on.";
const CHOOSE_PRINTER: &str =
    "The saved USB printer is not plugged in. Choose a printer in the window.";

/// Why a site was not given a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    BadOrigin,
    /// The site is on plain http and this till only allows https sites.
    Insecure,
    /// TapPrint runs with no window, so nobody can be asked.
    NoWindow,
    Busy,
    Blocked,
    Denied,
    TimedOut,
    Failed(String),
}

/// A website the cashier allowed to print, and the code only it may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Site {
    origin: String,
    code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Usb,
    Tcp,
}

/// How the window should colour the latest status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ready,
    Busy,
    Problem,
}

/// Missing fields take their defaults, so an older or hand-edited file still loads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct FileConfig {
    port: u16,
    pairing_code: String,
    paper_mm: u16,
    transport: Transport,
    tcp_host: String,
    tcp_port: u16,
    usb_id: Option<String>,
    /// Off unless the cashier turns it on. Raw commands can change printer settings.
    allow_raw: bool,
    /// New installs start with the drawer controls hidden. A file saved
    /// before this setting existed keeps them.
    #[serde(default = "drawer_enabled_for_old_files")]
    drawer_enabled: bool,
    drawer_pin: u8,
    drawer_on_ms: u16,
    drawer_off_ms: u16,
    /// Refuse websites on plain http, except pages on this computer.
    https_only: bool,
    /// Ask GitHub once a day whether a newer TapPrint exists.
    check_updates: bool,
    /// Network printer mode: answer on `share_port` like a network receipt
    /// printer, so apps not written for TapPrint can print through it.
    share_on: bool,
    share_port: u16,
    /// Also answer other devices on the local network, not this computer only.
    share_lan: bool,
    sites: Vec<Site>,
    /// Print from KHEvents: this computer's print-station key. Empty while
    /// not connected. Like the codes above, it is kept in this file only.
    server_token: String,
    /// The printer's name and organizer, as khevents.app gave them.
    server_station: String,
    server_organizer: String,
    /// Stop taking server jobs without disconnecting. They wait on the server.
    server_paused: bool,
}

fn drawer_enabled_for_old_files() -> bool {
    true
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            pairing_code: String::new(),
            paper_mm: 80,
            transport: Transport::Usb,
            tcp_host: String::new(),
            tcp_port: 9100,
            usb_id: None,
            allow_raw: false,
            drawer_enabled: false,
            drawer_pin: 2,
            drawer_on_ms: 50,
            drawer_off_ms: 50,
            https_only: false,
            check_updates: true,
            share_on: false,
            share_port: SHARE_PORT,
            share_lan: false,
            sites: Vec::new(),
            server_token: String::new(),
            server_station: String::new(),
            server_organizer: String::new(),
            server_paused: false,
        }
    }
}

/// How the link to khevents.app is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Link {
    /// Not connected to any organizer.
    #[default]
    Off,
    /// Connected; the first answer has not come yet.
    Starting,
    Online,
    /// The last call failed. TapPrint keeps trying.
    Offline,
    /// khevents.app has server printing turned off.
    Disabled,
}

/// What happened to one job from the server, for the window's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerOutcome {
    Printed,
    /// Nothing reached the paper. The server tries again shortly.
    Retrying,
    /// Part of it may have printed. The server never retries this by itself.
    CheckPaper,
    Failed,
}

impl ServerOutcome {
    pub fn words(self) -> &'static str {
        match self {
            Self::Printed => "Printed",
            Self::Retrying => "Will retry",
            Self::CheckPaper => "Check paper",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerJob {
    pub summary: String,
    pub sender: String,
    pub outcome: ServerOutcome,
    pub ago: String,
}

/// Print from KHEvents, as the window shows it. The key is never included.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerView {
    pub connected: bool,
    pub station: String,
    pub organizer: String,
    pub paused: bool,
    /// Paused on the Printers page, not here.
    pub paused_there: bool,
    pub link: Link,
    /// Why the link is not online, in a sentence.
    pub note: String,
    pub jobs: u32,
    pub recent: Vec<ServerJob>,
}

/// How many server jobs the window lists.
const RECENT_SERVER_JOBS: usize = 20;

/// A till page that has called this app. Other computers cannot connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TillClient {
    pub label: String,
    pub seconds_ago: u64,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub port: u16,
    pub pairing_code: String,
    pub paper: Paper,
    pub transport: Transport,
    pub tcp_host: String,
    pub tcp_port: u16,
    pub usb_id: Option<String>,
    pub listed: bool,
    pub tone: Tone,
    pub status: String,
    pub config_path: String,
    pub printers: Vec<(String, String)>,
    pub clients: Vec<TillClient>,
    pub jobs: u32,
    pub last_job: Option<String>,
    pub allow_raw: bool,
    pub drawer: [u8; 5],
    /// Show the drawer pin choice and the Open drawer button.
    pub drawer_enabled: bool,
    /// The printer connector pin the drawer is wired to: 2 or 5.
    pub drawer_pin: u8,
    /// Only https websites, and pages on this computer, may print.
    pub https_only: bool,
    pub check_updates: bool,
    pub share_on: bool,
    pub share_port: u16,
    pub share_lan: bool,
    /// A newer release, once a check has found one.
    pub update: Option<crate::update::Release>,
    /// Origins allowed to print with their own code.
    pub sites: Vec<String>,
    /// A site waiting for the cashier.
    pub pair_request: Option<PairRequest>,
    pub server: ServerView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairRequest {
    /// Passed back with the answer, so a click meant for an older request is ignored.
    pub serial: u64,
    pub origin: String,
    /// How the page named itself. Not checked.
    pub client: String,
    /// The origin can already print. A second request is unusual, so the window warns.
    pub known: bool,
    /// The site is on plain http, so someone on its network could alter the page.
    pub insecure: bool,
    /// Whole seconds until the page stops waiting.
    pub seconds_left: u64,
}

pub struct Shared {
    inner: Mutex<State>,
    /// Wakes the request waiting in `request_pair` when the cashier answers.
    pair_answered: Condvar,
    /// One job at a time. The test button and the browser must not interleave bytes.
    printing: Mutex<()>,
    /// Held from reading the config to renaming the file, so an older copy
    /// can never be written over a newer one, such as a fresh pairing code.
    saving: Mutex<()>,
}

struct State {
    config: FileConfig,
    path: PathBuf,
    tone: Tone,
    status: String,
    printers: Vec<(String, String)>,
    listed: bool,
    clients: Vec<SeenClient>,
    jobs: u32,
    last_job: Option<(String, Instant)>,
    pairing: Option<PendingPair>,
    pair_serial: u64,
    /// Origins that may not ask again until the given time.
    blocked: Vec<(String, Instant)>,
    /// No site may ask before this time. See `PROMPT_GAP`.
    quiet_until: Option<Instant>,
    /// Running with no window or tray, started by `tapprint serve`.
    headless: bool,
    update: Option<crate::update::Release>,
    link: Link,
    link_note: String,
    paused_there: bool,
    server_jobs: u32,
    recent: Vec<(String, String, ServerOutcome, Instant)>,
}

struct PendingPair {
    serial: u64,
    origin: String,
    client: String,
    deadline: Instant,
    answer: Option<bool>,
}

struct SeenClient {
    label: String,
    at: Instant,
}

impl Shared {
    /// Read the settings without writing them. A second copy of the app reads
    /// them too, then stops because the port is taken. If it wrote here, it
    /// could undo a change the running copy just saved. Call `persist` once
    /// the port is bound.
    pub fn load() -> Result<Self, String> {
        Self::read_from(config_path(), Some(legacy_config_path()))
    }

    #[cfg(test)]
    pub(crate) fn load_from(path: PathBuf, legacy: Option<PathBuf>) -> Result<Self, String> {
        let shared = Self::read_from(path, legacy)?;
        shared.persist()?;
        Ok(shared)
    }

    fn read_from(path: PathBuf, legacy: Option<PathBuf>) -> Result<Self, String> {
        let (config, notice) = if path.exists() {
            read_config(&path)?
        } else if let Some(legacy) = legacy.filter(|legacy| legacy.exists()) {
            // Keep the pairing code from a Receipt Till install, so the till page
            // does not need the code again after the rename.
            read_config(&legacy)?
        } else {
            (fresh_config()?, None)
        };
        Ok(Self::from_parts(config, path, notice))
    }

    fn from_parts(config: FileConfig, path: PathBuf, notice: Option<String>) -> Self {
        let (tone, status) = match notice {
            Some(message) => (Tone::Problem, message),
            None => (Tone::Ready, "Ready to print".to_string()),
        };
        Self {
            inner: Mutex::new(State {
                config,
                path,
                tone,
                status,
                printers: Vec::new(),
                listed: false,
                clients: Vec::new(),
                jobs: 0,
                last_job: None,
                pairing: None,
                pair_serial: 0,
                blocked: Vec::new(),
                quiet_until: None,
                headless: false,
                update: None,
                link: Link::Off,
                link_note: String::new(),
                paused_there: false,
                server_jobs: 0,
                recent: Vec::new(),
            }),
            pair_answered: Condvar::new(),
            printing: Mutex::new(()),
            saving: Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = self.lock();
        // "Ready" must mean a printer is there. A USB till with an empty scan
        // is not ready, whatever the last message said.
        let no_printer = state.tone == Tone::Ready
            && state.config.transport == Transport::Usb
            && state.listed
            && state.printers.is_empty();
        Snapshot {
            port: state.config.port,
            pairing_code: state.config.pairing_code.clone(),
            paper: Paper::from_millimetres(state.config.paper_mm).unwrap_or(Paper::Mm80),
            transport: state.config.transport,
            tcp_host: state.config.tcp_host.clone(),
            tcp_port: state.config.tcp_port,
            usb_id: state.config.usb_id.clone(),
            listed: state.listed,
            tone: if no_printer {
                Tone::Problem
            } else {
                state.tone
            },
            status: if no_printer {
                NO_PRINTER.to_string()
            } else {
                state.status.clone()
            },
            config_path: state.path.display().to_string(),
            printers: state.printers.clone(),
            clients: state
                .clients
                .iter()
                .filter_map(|client| {
                    let ago = client.at.elapsed();
                    if ago > Duration::from_secs(600) {
                        return None;
                    }
                    Some(TillClient {
                        label: client.label.clone(),
                        seconds_ago: ago.as_secs(),
                        live: ago <= Duration::from_secs(12),
                    })
                })
                .collect(),
            jobs: state.jobs,
            last_job: state
                .last_job
                .as_ref()
                .map(|(summary, at)| format!("{summary} · {}", ago_words(at.elapsed()))),
            allow_raw: state.config.allow_raw,
            drawer: escpos::drawer_command(
                state.config.drawer_pin,
                state.config.drawer_on_ms,
                state.config.drawer_off_ms,
            ),
            drawer_enabled: state.config.drawer_enabled,
            drawer_pin: state.config.drawer_pin,
            https_only: state.config.https_only,
            check_updates: state.config.check_updates,
            share_on: state.config.share_on,
            share_port: state.config.share_port,
            share_lan: state.config.share_lan,
            update: state.update.clone(),
            sites: state
                .config
                .sites
                .iter()
                .map(|site| site.origin.clone())
                .collect(),
            pair_request: state
                .pairing
                .as_ref()
                .filter(|pending| pending.answer.is_none())
                .map(|pending| PairRequest {
                    serial: pending.serial,
                    origin: pending.origin.clone(),
                    client: pending.client.clone(),
                    known: state
                        .config
                        .sites
                        .iter()
                        .any(|site| site.origin == pending.origin),
                    insecure: insecure_origin(&pending.origin),
                    seconds_left: pending
                        .deadline
                        .saturating_duration_since(Instant::now())
                        .as_secs(),
                }),
            server: ServerView {
                connected: !state.config.server_token.is_empty(),
                station: state.config.server_station.clone(),
                organizer: state.config.server_organizer.clone(),
                paused: state.config.server_paused,
                paused_there: state.paused_there,
                link: if state.config.server_token.is_empty() {
                    Link::Off
                } else {
                    state.link
                },
                note: state.link_note.clone(),
                jobs: state.server_jobs,
                recent: state
                    .recent
                    .iter()
                    .map(|(summary, sender, outcome, at)| ServerJob {
                        summary: summary.clone(),
                        sender: sender.clone(),
                        outcome: *outcome,
                        ago: ago_words(at.elapsed()),
                    })
                    .collect(),
            },
        }
    }

    /// The print-station key, while connected. Only the station thread reads it.
    pub fn server_token(&self) -> Option<String> {
        let state = self.lock();
        (!state.config.server_token.is_empty()).then(|| state.config.server_token.clone())
    }

    /// Save the key khevents.app gave for a pairing code.
    pub fn connect_server(
        &self,
        token: &str,
        station: &str,
        organizer: &str,
    ) -> Result<(), String> {
        {
            let mut state = self.lock();
            state.config.server_token = token.to_string();
            state.config.server_station = station.to_string();
            state.config.server_organizer = organizer.to_string();
            state.config.server_paused = false;
            state.link = Link::Starting;
            state.link_note = String::new();
            state.paused_there = false;
            state.recent.clear();
            state.server_jobs = 0;
            state.tone = Tone::Ready;
            state.status = format!("Connected to {organizer} as {station}");
        }
        self.persist()
    }

    /// Forget the key. `why` goes on the status line.
    pub fn disconnect_server(&self, why: &str, problem: bool) {
        {
            let mut state = self.lock();
            if state.config.server_token.is_empty() {
                return;
            }
            state.config.server_token = String::new();
            state.config.server_paused = false;
            state.link = Link::Off;
            state.link_note = String::new();
            state.paused_there = false;
            state.tone = if problem { Tone::Problem } else { Tone::Ready };
            state.status = why.to_string();
        }
        let _ = self.persist();
    }

    /// Forget the key only if it is still `token`. A reply to an old key must
    /// not disconnect a pairing made a moment ago.
    pub fn drop_server_token(&self, token: &str, why: &str) {
        if self.server_token().as_deref() == Some(token) {
            self.disconnect_server(why, true);
        }
    }

    pub fn set_server_paused(&self, paused: bool) {
        {
            let mut state = self.lock();
            state.config.server_paused = paused;
            state.tone = Tone::Ready;
            state.status = if paused {
                "Paused: server jobs wait until you turn this back on".to_string()
            } else {
                "Taking print jobs from KHEvents".to_string()
            };
        }
        let _ = self.persist();
    }

    pub fn set_link(&self, link: Link, note: impl Into<String>, paused_there: bool) {
        let mut state = self.lock();
        state.link = link;
        state.link_note = note.into();
        state.paused_there = paused_there;
    }

    /// Remember a server job for the window. Nothing here is written to disk.
    pub fn note_server_job(&self, summary: &str, sender: &str, outcome: ServerOutcome) {
        let mut state = self.lock();
        if outcome == ServerOutcome::Printed {
            state.server_jobs = state.server_jobs.saturating_add(1);
        }
        state.recent.insert(
            0,
            (
                summary.to_string(),
                sender.to_string(),
                outcome,
                Instant::now(),
            ),
        );
        state.recent.truncate(RECENT_SERVER_JOBS);
    }

    /// Remember a till page on this computer. The label is already bounded.
    pub fn note_client(&self, label: &str) {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 80 {
            return;
        }
        let mut state = self.lock();
        if let Some(index) = state
            .clients
            .iter()
            .position(|client| client.label == label)
        {
            let mut client = state.clients.remove(index);
            client.at = Instant::now();
            state.clients.insert(0, client);
            return;
        }
        state.clients.insert(
            0,
            SeenClient {
                label: label.to_string(),
                at: Instant::now(),
            },
        );
        state.clients.truncate(6);
    }

    /// Remember the latest print or drawer pulse. Only jobs that went out are
    /// counted. Nothing here is written to disk.
    pub fn note_job(&self, summary: &str, done: bool) {
        let mut summary = summary.trim().to_string();
        if summary.is_empty() {
            return;
        }
        if summary.chars().count() > 80 {
            summary = summary.chars().take(80).collect();
        }
        let mut state = self.lock();
        if done {
            state.jobs = state.jobs.saturating_add(1);
        }
        state.last_job = Some((summary, Instant::now()));
    }

    /// Errors arrive in lowercase from deep in the stack. The window shows a sentence.
    pub fn set_status(&self, tone: Tone, status: impl Into<String>) {
        let mut status = status.into();
        if let Some(first) = status.get(..1) {
            status = first.to_ascii_uppercase() + status.get(1..).unwrap_or_default();
        }
        let mut state = self.lock();
        if state.tone == tone && state.status == status {
            return;
        }
        state.tone = tone;
        state.status = status;
    }

    pub fn set_printers(&self, printers: Vec<(String, String)>) {
        let mut changed = false;
        {
            let mut state = self.lock();
            let saved = state.config.usb_id.clone();
            let saved_present = saved
                .as_ref()
                .is_some_and(|saved| printers.iter().any(|(id, _)| id == saved));
            // A printer moved to another port comes back with a new id. When it
            // is the only printer and the same model, follow it. A different
            // model is a different printer, so the cashier must choose it.
            let mut missing = false;
            if !saved_present && let [(only, _)] = printers.as_slice() {
                let same_model = saved.as_deref().is_none_or(|saved| {
                    crate::usb::model_of(saved).is_some_and(|model| {
                        crate::usb::model_of(only).is_some_and(|found| found == model)
                    })
                });
                if same_model {
                    state.config.usb_id = Some(only.clone());
                    changed = true;
                } else {
                    missing = true;
                }
            } else if !saved_present && saved.is_some() && !printers.is_empty() {
                missing = true;
            }
            let missing = missing && state.config.transport == Transport::Usb;
            if missing && state.tone != Tone::Busy {
                state.tone = Tone::Problem;
                state.status = CHOOSE_PRINTER.to_string();
            } else if !missing && state.status == CHOOSE_PRINTER {
                state.tone = Tone::Ready;
                state.status = "Ready to print".to_string();
            }
            if state.status.starts_with(USB_SCAN_FAILED) {
                state.tone = Tone::Ready;
                state.status = "Ready to print".to_string();
            }
            state.printers = printers;
            state.listed = true;
        }
        if changed {
            let _ = self.persist();
        }
    }

    /// The USB scan failed. Only a till that prints over USB is told: on a
    /// network printer the message would be noise, repeated every second.
    pub fn note_usb_error(&self, error: &str) {
        let mut state = self.lock();
        state.printers.clear();
        // The scan ran, so the window stops saying it is still looking.
        state.listed = true;
        // The scan repeats every second. It must not wipe out another
        // problem the person has not read yet, such as a failed update.
        let other_problem =
            state.tone == Tone::Problem && !state.status.starts_with(USB_SCAN_FAILED);
        if state.config.transport == Transport::Usb && state.tone != Tone::Busy && !other_problem {
            state.tone = Tone::Problem;
            state.status = format!("{USB_SCAN_FAILED} ({error}).");
        }
    }

    /// Drawers are wired to pin 2 or pin 5 of the printer's drawer socket.
    pub fn set_drawer_pin(&self, pin: u8) {
        if !matches!(pin, 2 | 5) {
            return;
        }
        {
            let mut state = self.lock();
            state.config.drawer_pin = pin;
            state.tone = Tone::Ready;
            state.status = format!("The drawer opens on pin {pin}");
        }
        let _ = self.persist();
    }

    pub fn set_paper(&self, paper: Paper) {
        self.lock().config.paper_mm = paper.millimetres();
        let _ = self.persist();
    }

    /// `Err` when it could not be saved. The status already says so.
    pub fn set_transport(&self, transport: Transport) -> Result<(), String> {
        self.lock().config.transport = transport;
        self.persist()
    }

    pub fn select_usb(&self, id: String) {
        {
            let mut state = self.lock();
            state.config.usb_id = Some(id);
            state.config.transport = Transport::Usb;
            state.tone = Tone::Ready;
            state.status = "Using the selected USB printer".to_string();
        }
        let _ = self.persist();
    }

    /// Save the address typed in the window and switch to the network printer.
    pub fn set_network(&self, host: &str, port: u16) -> Result<(), String> {
        let host = host.trim();
        check_host(host)?;
        if port == 0 {
            return Err("port must be a number from 1 to 65535".to_string());
        }
        {
            let mut state = self.lock();
            state.config.tcp_host = host.to_string();
            state.config.tcp_port = port;
            state.config.transport = Transport::Tcp;
            state.tone = Tone::Ready;
            state.status = format!("Using network printer {host}:{port}");
        }
        self.persist()
    }

    /// Switch to the network printer, using the address already stored. With
    /// no usable address yet, still switch: the window then shows the address
    /// fields, which is the only place the address can be typed.
    pub fn use_saved_network(&self) -> Result<(), String> {
        {
            let mut state = self.lock();
            state.config.transport = Transport::Tcp;
            state.tone = Tone::Ready;
            state.status = if check_host(state.config.tcp_host.trim()).is_ok() {
                format!(
                    "Using network printer {}:{}",
                    state.config.tcp_host, state.config.tcp_port
                )
            } else {
                TYPE_ADDRESS.to_string()
            };
        }
        self.persist()
    }

    pub fn rotate_code(&self) -> Result<(), String> {
        let code = pairing_code()?;
        {
            let mut state = self.lock();
            state.config.pairing_code = code;
            state.tone = Tone::Ready;
            state.status = "Pairing code replaced. Copy the new one into the till.".to_string();
        }
        self.persist()
    }

    /// The code shown in the window works from anywhere. A site code works
    /// only on requests from the site it was given to.
    pub fn authorize(&self, origin: Option<&str>, offered: &str) -> bool {
        let state = self.lock();
        if codes_match(&state.config.pairing_code, offered) {
            return true;
        }
        let Some(origin) = origin.and_then(normalized_origin) else {
            return false;
        };
        if state.config.https_only && insecure_origin(&origin) {
            return false;
        }
        state
            .config
            .sites
            .iter()
            .any(|site| site.origin == origin && codes_match(&site.code, offered))
    }

    /// Ask the cashier whether `origin` may print, and wait for the answer.
    /// The browser sets `Origin`, so a page cannot ask in another site's name.
    pub fn request_pair(
        &self,
        origin: &str,
        client: &str,
        wait: Duration,
    ) -> Result<String, PairError> {
        let origin = normalized_origin(origin).ok_or(PairError::BadOrigin)?;
        let deadline = Instant::now() + wait;
        let mut state = self.lock();
        if state.config.https_only && insecure_origin(&origin) {
            return Err(PairError::Insecure);
        }
        // With no window there is no Allow button, and a request would only
        // hang until it timed out.
        if state.headless {
            return Err(PairError::NoWindow);
        }
        let now = Instant::now();
        state.blocked.retain(|(_, until)| *until > now);
        if state.blocked.iter().any(|(blocked, _)| *blocked == origin)
            || state.quiet_until.is_some_and(|until| until > now)
        {
            return Err(PairError::Blocked);
        }
        if state.pairing.is_some() {
            return Err(PairError::Busy);
        }
        state.pair_serial = state.pair_serial.wrapping_add(1);
        let serial = state.pair_serial;
        state.pairing = Some(PendingPair {
            serial,
            origin: origin.clone(),
            client: client.to_string(),
            deadline,
            answer: None,
        });
        let allowed = loop {
            let answer = match state.pairing.as_ref() {
                Some(pending) if pending.serial == serial => pending.answer,
                // Cleared by someone else. Treat it as a no.
                _ => Some(false),
            };
            if let Some(answer) = answer {
                break answer;
            }
            let now = Instant::now();
            if now >= deadline {
                state.pairing = None;
                state.blocked.push((origin.clone(), now + UNANSWERED_FOR));
                state.quiet_until = Some(now + PROMPT_GAP);
                note_pairing(&mut state, format!("{origin} stopped waiting to pair"));
                return Err(PairError::TimedOut);
            }
            state = match self.pair_answered.wait_timeout(state, deadline - now) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        };
        state.pairing = None;
        if !allowed {
            let now = Instant::now();
            state.blocked.push((origin.clone(), now + DENY_FOR));
            state.quiet_until = Some(now + PROMPT_GAP);
            note_pairing(&mut state, format!("{origin} was not allowed to print"));
            return Err(PairError::Denied);
        }
        // A fresh code every time. Anything that held the old one, such as a
        // program on this computer that faked the site's origin, loses it.
        let code = pairing_code().map_err(PairError::Failed)?;
        let previous = state
            .config
            .sites
            .iter()
            .position(|site| site.origin == origin)
            .map(|index| state.config.sites.remove(index));
        state.config.sites.push(Site {
            origin: origin.clone(),
            code: code.clone(),
        });
        note_pairing(&mut state, format!("{origin} can now print"));
        drop(state);
        if let Err(error) = self.persist() {
            // The page is told it failed, so the code it never got must not work.
            let mut state = self.lock();
            state.config.sites.retain(|site| site.code != code);
            if let Some(previous) = previous
                && !state.config.sites.iter().any(|site| site.origin == origin)
            {
                state.config.sites.push(previous);
            }
            drop(state);
            // Another save may have written the new code in the meantime.
            let _ = self.persist();
            return Err(PairError::Failed(error));
        }
        Ok(code)
    }

    /// The cashier's answer to request `serial`. A late click on a request
    /// that already ended, or a second click, changes nothing.
    pub fn answer_pair(&self, serial: u64, allow: bool) {
        if let Some(pending) = self.lock().pairing.as_mut()
            && pending.serial == serial
            && pending.answer.is_none()
        {
            pending.answer = Some(allow);
        }
        self.pair_answered.notify_all();
    }

    /// A site gives up its own code. The code must be that site's: a page
    /// cannot remove another site, and the window's own code is not a site's.
    pub fn unpair(&self, origin: Option<&str>, offered: &str) -> bool {
        let Some(origin) = origin.and_then(normalized_origin) else {
            return false;
        };
        let owns = self
            .lock()
            .config
            .sites
            .iter()
            .any(|site| site.origin == origin && codes_match(&site.code, offered));
        if owns {
            self.remove_site(&origin);
        }
        owns
    }

    /// The site's code stops working at once.
    pub fn remove_site(&self, origin: &str) {
        {
            let mut state = self.lock();
            state.config.sites.retain(|site| site.origin != origin);
            state.tone = Tone::Ready;
            state.status = format!("{origin} can no longer print");
        }
        let _ = self.persist();
    }

    /// There is no window: pairing requests are refused at once.
    pub fn set_headless(&self) {
        self.lock().headless = true;
    }

    /// What the latest check found: a newer release, or none.
    pub fn set_update(&self, update: Option<crate::update::Release>) {
        self.lock().update = update;
    }

    pub fn set_check_updates(&self, on: bool) {
        {
            let mut state = self.lock();
            state.config.check_updates = on;
            if !on {
                state.update = None;
            }
            state.tone = Tone::Ready;
            state.status = if on {
                "TapPrint will look for updates".to_string()
            } else {
                "TapPrint will not look for updates".to_string()
            };
        }
        let _ = self.persist();
    }

    /// Network printer mode on or off. The caller then runs `share::apply`.
    pub fn set_share_on(&self, on: bool) {
        {
            let mut state = self.lock();
            state.config.share_on = on;
            state.tone = Tone::Ready;
            state.status = if on {
                format!(
                    "Other apps can print to 127.0.0.1 port {}",
                    state.config.share_port
                )
            } else {
                "Network printer mode is off".to_string()
            };
        }
        let _ = self.persist();
    }

    /// The port other apps print to. It cannot be one TapPrint already uses.
    pub fn set_share_port(&self, port: u16) -> Result<(), String> {
        {
            let mut state = self.lock();
            check_share_port(port, state.config.port)?;
            state.config.share_port = port;
            state.tone = Tone::Ready;
            state.status = format!("Other apps print to port {port}");
        }
        self.persist()
    }

    /// Whether other devices on the local network may print too.
    pub fn set_share_lan(&self, on: bool) {
        {
            let mut state = self.lock();
            state.config.share_lan = on;
            state.tone = Tone::Ready;
            state.status = if on {
                "Other devices on this network can print".to_string()
            } else {
                "Only this computer can print".to_string()
            };
        }
        let _ = self.persist();
    }

    /// With this on, a website on plain http can neither pair nor print.
    /// Pages on this computer are not affected.
    pub fn set_https_only(&self, on: bool) {
        {
            let mut state = self.lock();
            state.config.https_only = on;
            state.tone = Tone::Ready;
            state.status = if on {
                "Only secure (https) websites may print".to_string()
            } else {
                "Websites on http may print too".to_string()
            };
        }
        let _ = self.persist();
    }

    /// Hides or shows the drawer controls in the window. The drawer command
    /// over HTTP is not affected.
    pub fn set_drawer_enabled(&self, on: bool) {
        {
            let mut state = self.lock();
            state.config.drawer_enabled = on;
        }
        let _ = self.persist();
    }

    /// Forget the till pages listed in the window. A page that calls again is listed again.
    pub fn clear_clients(&self) {
        self.lock().clients.clear();
    }

    pub fn set_allow_raw(&self, on: bool) {
        {
            let mut state = self.lock();
            state.config.allow_raw = on;
            state.tone = Tone::Ready;
            state.status = if on {
                "Paired pages may send raw printer commands".to_string()
            } else {
                "Raw printer commands are blocked".to_string()
            };
        }
        let _ = self.persist();
    }

    /// Hold this across the whole job so two callers cannot mix their bytes.
    pub fn printer_lock(&self) -> MutexGuard<'_, ()> {
        self.printing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn printer_busy(&self) -> bool {
        // A job that panicked leaves the lock poisoned, not held.
        matches!(self.printing.try_lock(), Err(TryLockError::WouldBlock))
    }

    pub fn persist(&self) -> Result<(), String> {
        match self.save() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.set_status(Tone::Problem, format!("Could not save settings: {error}"));
                Err(error)
            }
        }
    }

    fn save(&self) -> Result<(), String> {
        let _saving = self
            .saving
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (path, body) = {
            let state = self.lock();
            let body = serde_json::to_vec_pretty(&state.config)
                .map_err(|error| format!("could not write settings: {error}"))?;
            (state.path.clone(), body)
        };
        write_config(&path, &body)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn fresh_config() -> Result<FileConfig, String> {
    Ok(FileConfig {
        pairing_code: pairing_code()?,
        ..FileConfig::default()
    })
}

fn read_config(path: &Path) -> Result<(FileConfig, Option<String>), String> {
    let body = fs::read(path).map_err(|error| {
        format!(
            "could not read {}. It was left unchanged. {error}",
            path.display()
        )
    })?;
    let mut config: FileConfig = serde_json::from_slice(&body).map_err(|_| {
        format!(
            "could not read settings in {}. They were left unchanged.",
            path.display()
        )
    })?;
    let mut notice = None;
    if config.port < 1024 {
        config.port = DEFAULT_PORT;
    }
    if Paper::from_millimetres(config.paper_mm).is_none() {
        config.paper_mm = 80;
    }
    if config.tcp_port == 0 {
        config.tcp_port = 9100;
    }
    if check_share_port(config.share_port, config.port).is_err() {
        config.share_port = SHARE_PORT;
    }
    if !matches!(config.drawer_pin, 2 | 5) {
        config.drawer_pin = 2;
    }
    config.drawer_on_ms = config.drawer_on_ms.clamp(2, 510);
    config.drawer_off_ms = config.drawer_off_ms.clamp(2, 510);
    // A hand-edited entry that is not a usable origin and code is dropped,
    // so it can never match a request.
    config.sites = std::mem::take(&mut config.sites)
        .into_iter()
        .filter_map(|site| {
            Some(Site {
                origin: normalized_origin(&site.origin)?,
                code: normalized_code(&site.code)?,
            })
        })
        .collect();
    config.tcp_host = config.tcp_host.trim().to_string();
    if !config.tcp_host.is_empty() && check_host(&config.tcp_host).is_err() {
        notice = Some("The saved printer address is not usable. Enter it again.".to_string());
    }
    if let Some(code) = normalized_code(&config.pairing_code) {
        config.pairing_code = code;
    } else {
        config.pairing_code = pairing_code()?;
        notice = Some(
            "The pairing code was too short, so it was replaced. Copy the new one into the till."
                .to_string(),
        );
    }
    Ok((config, notice))
}

/// A port other apps can print to: not a system port, and not one of the two
/// TapPrint's own API answers on.
fn check_share_port(port: u16, api_port: u16) -> Result<(), String> {
    if port < 1024 {
        return Err("choose a port from 1024 to 65535".to_string());
    }
    if port == api_port || Some(port) == api_port.checked_add(1) {
        return Err(format!(
            "port {port} is the one web pages use. Choose another."
        ));
    }
    Ok(())
}

/// The port and manual code of the TapPrint on this computer, for the
/// command line. Read from the settings file, which only this user can read,
/// and never written.
pub fn local_access() -> Result<(u16, String), String> {
    let path = config_path();
    if !path.exists() {
        return Err(
            "TapPrint has not been opened on this computer yet. Open it once first.".to_string(),
        );
    }
    let (config, _) = read_config(&path)?;
    Ok((config.port, config.pairing_code))
}

/// A pairing result for the status line. A printer problem stays on screen.
fn note_pairing(state: &mut State, message: String) {
    if state.tone != Tone::Problem {
        state.tone = Tone::Ready;
        state.status = message;
    }
}

/// `https://shop.example` or `http://localhost:3000`, lowercased. Browsers
/// send exactly this form. `null`, paths, and other schemes are refused.
fn normalized_origin(origin: &str) -> Option<String> {
    let origin = origin.trim().to_ascii_lowercase();
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    let allowed = !rest.is_empty()
        && origin.len() <= 200
        && rest
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':' | '[' | ']'));
    allowed.then_some(origin)
}

/// A normalized origin on plain http that is not this computer. Browsers
/// treat `http://localhost` as secure, because it never leaves the machine.
/// Anywhere else, http lets someone on the network change the page.
fn insecure_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let host = match rest.strip_prefix('[') {
        // `[::1]:3000`
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => rest.split(':').next().unwrap_or_default(),
    };
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback());
    !local
}

fn pairing_code() -> Result<String, String> {
    let mut bytes = [0u8; CODE_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("could not generate a pairing code: {error}"))?;
    Ok(hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Lowercase hex of the expected length, or nothing.
fn normalized_code(code: &str) -> Option<String> {
    // `code_bytes` already holds the lowercase hex digits as ASCII.
    String::from_utf8(code_bytes(code)?.to_vec()).ok()
}

fn code_bytes(code: &str) -> Option<[u8; 32]> {
    let mut bytes = [0u8; 32];
    let mut index = 0usize;
    for byte in code.bytes() {
        if matches!(byte, b' ' | b'-' | b'\t') {
            continue;
        }
        if index >= 32 {
            return None;
        }
        bytes[index] = match byte {
            b'0'..=b'9' | b'a'..=b'f' => byte,
            b'A'..=b'F' => byte - b'A' + b'a',
            _ => return None,
        };
        index += 1;
    }
    if index == 32 { Some(bytes) } else { None }
}

fn codes_match(expected: &str, offered: &str) -> bool {
    match (code_bytes(expected), code_bytes(offered)) {
        (Some(expected), Some(offered)) => constant_time_eq(&expected, &offered),
        _ => false,
    }
}

/// Compares every byte, so the time taken does not reveal how much matched.
fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

fn write_config(path: &Path, body: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|error| format!("could not write {}: {error}", tmp.display()))?;
        file.write_all(body)
            .map_err(|error| format!("could not write {}: {error}", tmp.display()))?;
        file.sync_all()
            .map_err(|error| format!("could not write {}: {error}", tmp.display()))?;
    }
    // `rename` replaces the old file on every platform. Never delete it first:
    // if the rename then failed, the pairing code would be gone.
    fs::rename(&tmp, path)
        .map_err(|error| format!("could not replace {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// The name of the folder that, placed beside the program, keeps the
/// settings there instead of in the user's profile.
const PORTABLE_FOLDER: &str = "tapprint-data";

fn config_path() -> PathBuf {
    #[cfg(not(target_os = "macos"))]
    if let Some(folder) = std::env::current_exe()
        .ok()
        .and_then(|program| portable_folder(&program))
    {
        return folder.join("config.json");
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let folder = "TapPrint";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let folder = "tapprint";
    config_root().join(folder).join("config.json")
}

/// Portable use: with a `tapprint-data` folder beside the program, settings
/// live in it, so the program and its settings travel together on a USB
/// stick or in one folder. Only a folder someone made counts: it is never
/// created here, so a normal install is not changed by where it is put.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn portable_folder(program: &Path) -> Option<PathBuf> {
    let folder = program.parent()?.join(PORTABLE_FOLDER);
    folder.is_dir().then_some(folder)
}

/// The folder holding the settings file and, on macOS, the local certificate.
pub fn data_dir() -> PathBuf {
    let mut path = config_path();
    path.pop();
    path
}

/// Where the app kept its settings when it was called Receipt Till.
fn legacy_config_path() -> PathBuf {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let folder = "Receipt Till";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let folder = "receipt-till";
    config_root().join(folder).join("config.json")
}

fn config_root() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir().join("Library").join("Application Support")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
    }
}

/// Name a caller from the till header and the browser, without keeping the raw header.
pub fn client_label(product: Option<&str>, user_agent: Option<&str>) -> String {
    let product = product.and_then(clean_product);
    let browser = user_agent.and_then(browser_family);
    match (product, browser) {
        (Some(product), Some(browser)) => format!("{product} · {browser}"),
        (Some(product), None) => product,
        (None, Some(browser)) => browser.to_string(),
        (None, None) => "This computer".to_string(),
    }
}

fn clean_product(value: &str) -> Option<String> {
    let value = value.trim();
    let count = value.chars().count();
    if count == 0 || count > 40 {
        return None;
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '.'))
    {
        return None;
    }
    Some(value.to_string())
}

fn browser_family(agent: &str) -> Option<&'static str> {
    let agent: String = agent.chars().take(180).collect();
    let lower = agent.to_ascii_lowercase();
    if lower.contains("edg/") {
        Some("Edge")
    } else if lower.contains("chrome/") || lower.contains("crios/") {
        Some("Chrome")
    } else if lower.contains("firefox/") {
        Some("Firefox")
    } else if lower.contains("safari/") {
        Some("Safari")
    } else if lower.contains("curl/") {
        Some("curl")
    } else {
        None
    }
}

pub fn ago_words(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 5 {
        "just now".to_string()
    } else if seconds < 60 {
        format!("{seconds}s ago")
    } else {
        format!("{} min ago", seconds / 60)
    }
}

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    // An update must not make a working cash drawer vanish for a till that
    // already saved its settings, while a fresh install starts without one.
    #[test]
    fn drawer_controls_stay_for_old_files_and_start_hidden_for_new_ones() {
        let old: FileConfig = serde_json::from_str(r#"{"paper_mm":80}"#).unwrap();
        assert!(old.drawer_enabled);
        assert!(!FileConfig::default().drawer_enabled);
    }

    #[test]
    fn clearing_till_pages_empties_the_list() {
        let shared = Shared::from_parts(FileConfig::default(), PathBuf::new(), None);
        shared.note_client("KHEvents · Chrome");
        assert_eq!(shared.snapshot().clients.len(), 1);
        shared.clear_clients();
        assert!(shared.snapshot().clients.is_empty());
    }

    fn isolated(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tapprint-{name}-{}-{}",
            std::process::id(),
            name.len()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("config.json")
    }

    #[test]
    fn pairing_codes_must_match_every_byte() {
        let code = "0123456789abcdef0123456789abcdef";
        assert!(codes_match(code, code));
        assert!(!codes_match(code, "0123456789abcdef0123456789abcdee"));
        assert!(!codes_match(code, "0123456789abcdef0123456789abcde"));
        assert!(!codes_match(code, ""));
    }

    #[test]
    fn a_data_folder_beside_the_program_makes_it_portable() {
        let folder = std::env::temp_dir().join(format!("tapprint-portable-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let program = folder.join("tapprint");
        // No folder: an ordinary install, settings in the user's profile.
        assert_eq!(portable_folder(&program), None);
        // A file of that name is not a folder someone set up.
        fs::write(folder.join(PORTABLE_FOLDER), b"").unwrap();
        assert_eq!(portable_folder(&program), None);
        fs::remove_file(folder.join(PORTABLE_FOLDER)).unwrap();
        fs::create_dir(folder.join(PORTABLE_FOLDER)).unwrap();
        assert_eq!(
            portable_folder(&program),
            Some(folder.join(PORTABLE_FOLDER))
        );
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn network_printer_mode_is_off_until_asked_for_and_keeps_its_settings() {
        let path = isolated("share");
        let shared = Shared::load_from(path.clone(), None).unwrap();
        let fresh = shared.snapshot();
        assert!(!fresh.share_on);
        assert!(!fresh.share_lan);
        assert_eq!(fresh.share_port, 9100);

        shared.set_share_on(true);
        shared.set_share_lan(true);
        shared.set_share_port(9101).unwrap();
        // A port web pages already use would break printing from the browser.
        assert!(shared.set_share_port(DEFAULT_PORT).is_err());
        assert!(shared.set_share_port(DEFAULT_PORT + 1).is_err());
        assert!(shared.set_share_port(80).is_err());
        let kept = Shared::load_from(path, None).unwrap().snapshot();
        assert!(kept.share_on && kept.share_lan);
        assert_eq!(kept.share_port, 9101);
    }

    #[test]
    fn with_no_window_a_site_is_told_at_once_that_it_cannot_pair() {
        // Nobody can click Allow. Waiting two minutes to say so would look
        // like a hang, and the manual code still works.
        let shared = Shared::load_from(isolated("headless"), None).unwrap();
        shared.set_headless();
        let started = Instant::now();
        assert_eq!(
            shared.request_pair("https://shop.example", "x", Duration::from_secs(30)),
            Err(PairError::NoWindow)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let code = shared.snapshot().pairing_code;
        assert!(shared.authorize(None, &code));
    }

    #[test]
    fn host_must_be_a_name_or_address() {
        assert!(check_host("192.168.1.20").is_ok());
        assert!(check_host("printer.local").is_ok());
        assert!(check_host("[fe80::1]").is_ok());
        assert!(check_host("").is_err());
        assert!(check_host("http://192.168.1.20").is_err());
        assert!(check_host("192.168.1.20/receipt").is_err());
    }

    #[test]
    fn a_new_code_is_32_hex_digits_and_accepts_uppercase() {
        let shared = Shared::load_from(isolated("fresh"), None).unwrap();
        let code = shared.snapshot().pairing_code;
        assert_eq!(code.len(), 32);
        assert!(code.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert!(shared.authorize(None, &code.to_ascii_uppercase()));
        let mut grouped = String::new();
        for (index, ch) in code.chars().enumerate() {
            if index > 0 && index % 4 == 0 {
                grouped.push(' ');
            }
            grouped.push(ch);
        }
        assert!(shared.authorize(None, &grouped));
        assert!(!shared.authorize(None, "nope"));
    }

    #[test]
    fn the_pairing_code_survives_a_restart() {
        let path = isolated("restart");
        let first = Shared::load_from(path.clone(), None).unwrap().snapshot();
        let second = Shared::load_from(path.clone(), None).unwrap().snapshot();
        let third = Shared::load_from(path, None).unwrap().snapshot();
        assert_eq!(first.pairing_code, second.pairing_code);
        assert_eq!(second.pairing_code, third.pairing_code);
        assert_eq!(third.tone, Tone::Ready);
    }

    #[test]
    fn wrong_codes_from_another_page_never_lock_out_the_till() {
        let shared = Shared::load_from(isolated("flood"), None).unwrap();
        let code = shared.snapshot().pairing_code;
        for _ in 0..100 {
            assert!(!shared.authorize(None, "nope"));
        }
        assert!(shared.authorize(None, &code));
    }

    #[test]
    fn a_receipt_till_install_keeps_its_pairing_code() {
        let legacy = isolated("legacy");
        let code = "ab".repeat(16);
        fs::write(
            &legacy,
            format!(r#"{{"pairing_code":"{code}","paper_mm":58}}"#),
        )
        .unwrap();
        let path = isolated("renamed");
        let shared = Shared::load_from(path.clone(), Some(legacy)).unwrap();
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.pairing_code, code);
        assert_eq!(snapshot.paper, Paper::Mm58);
        assert_eq!(snapshot.port, DEFAULT_PORT);
        assert!(path.exists());
    }

    #[test]
    fn a_replugged_printer_is_followed_to_its_new_address() {
        let shared = Shared::load_from(isolated("replug"), None).unwrap();
        shared.set_printers(vec![("04b8:0202:1:4".to_string(), "USB".to_string())]);
        assert_eq!(shared.snapshot().usb_id.as_deref(), Some("04b8:0202:1:4"));
        shared.set_printers(vec![("04b8:0202:1:7".to_string(), "USB".to_string())]);
        assert_eq!(shared.snapshot().usb_id.as_deref(), Some("04b8:0202:1:7"));
        // With two printers the app cannot guess, so the cashier picks one.
        shared.set_printers(vec![
            ("04b8:0202:1:8".to_string(), "USB".to_string()),
            ("04b8:0e15:1:9".to_string(), "USB".to_string()),
        ]);
        assert_eq!(shared.snapshot().usb_id.as_deref(), Some("04b8:0202:1:7"));
    }

    #[test]
    fn network_can_be_chosen_before_any_address_is_saved() {
        // The address fields only show once Network is chosen. Refusing the
        // switch for lack of an address left no way to type the first one.
        let shared = Shared::load_from(isolated("first-network"), None).unwrap();
        shared.use_saved_network().unwrap();
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.transport, Transport::Tcp);
        assert_eq!(snapshot.tone, Tone::Ready);
        assert_eq!(snapshot.status, TYPE_ADDRESS);

        shared.set_network("192.168.1.20", 9100).unwrap();
        shared.set_transport(Transport::Usb).unwrap();
        shared.use_saved_network().unwrap();
        assert_eq!(
            shared.snapshot().status,
            "Using network printer 192.168.1.20:9100"
        );
    }

    #[test]
    fn ready_needs_a_printer_on_a_usb_till() {
        let shared = Shared::load_from(isolated("no-printer"), None).unwrap();
        // Before the first scan the window says it is looking, not that it failed.
        assert_eq!(shared.snapshot().tone, Tone::Ready);
        shared.set_printers(Vec::new());
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.tone, Tone::Problem);
        assert_eq!(snapshot.status, NO_PRINTER);
        // A network printer is not found by the USB scan, so it stays ready.
        shared.set_network("192.168.1.20", 9100).unwrap();
        assert_eq!(shared.snapshot().tone, Tone::Ready);
    }

    #[test]
    fn a_failed_usb_scan_is_only_shown_to_a_usb_till() {
        let shared = Shared::load_from(isolated("usb-scan"), None).unwrap();
        shared.note_usb_error("access denied");
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.tone, Tone::Problem);
        assert!(snapshot.status.contains("access denied"));
        // The scan ran, so the window does not say it is still looking.
        assert!(snapshot.listed);
        // A scan that works again clears the message.
        shared.set_printers(Vec::new());
        assert_eq!(shared.snapshot().status, NO_PRINTER);

        // Another problem on screen is not replaced by the repeating scan.
        shared.set_status(Tone::Problem, "the update could not be installed");
        shared.note_usb_error("access denied");
        assert_eq!(
            shared.snapshot().status,
            "The update could not be installed"
        );

        // A till on a network printer never hears about USB.
        shared.set_network("192.168.1.20", 9100).unwrap();
        shared.note_usb_error("access denied");
        assert_eq!(shared.snapshot().tone, Tone::Ready);
    }

    #[test]
    fn the_drawer_pin_can_be_changed_and_is_kept() {
        let path = isolated("drawer-pin");
        let shared = Shared::load_from(path.clone(), None).unwrap();
        assert_eq!(shared.snapshot().drawer_pin, 2);
        shared.set_drawer_pin(5);
        shared.set_drawer_pin(9); // not a pin a drawer can be on
        let reloaded = Shared::load_from(path, None).unwrap().snapshot();
        assert_eq!(reloaded.drawer_pin, 5);
        assert_eq!(reloaded.drawer[2], 1);
    }

    #[test]
    fn a_different_model_is_never_picked_up_without_asking() {
        // The kitchen printer plugged in while the receipt printer is away
        // must not start getting receipts on its own.
        let shared = Shared::load_from(isolated("swap"), None).unwrap();
        shared.set_printers(vec![("04b8:0202:1-4".to_string(), "USB".to_string())]);
        shared.set_printers(vec![("0519:0001:1-5".to_string(), "USB".to_string())]);
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.usb_id.as_deref(), Some("04b8:0202:1-4"));
        assert_eq!(snapshot.tone, Tone::Problem);
        assert_eq!(snapshot.status, CHOOSE_PRINTER);
        // The receipt printer comes back, and the warning goes away.
        shared.set_printers(vec![("04b8:0202:1-4".to_string(), "USB".to_string())]);
        assert_eq!(shared.snapshot().tone, Tone::Ready);
    }

    fn answer_when_asked(
        shared: &std::sync::Arc<Shared>,
        allow: bool,
    ) -> std::thread::JoinHandle<()> {
        let shared = std::sync::Arc::clone(shared);
        std::thread::spawn(move || {
            for _ in 0..500 {
                if let Some(request) = shared.snapshot().pair_request {
                    shared.answer_pair(request.serial, allow);
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("no pairing request showed up");
        })
    }

    #[test]
    fn a_copy_that_only_reads_the_settings_does_not_write_them() {
        // A second copy of the app reads the settings, then stops because the
        // port is taken. Writing on the way out could undo a change the
        // running copy just saved, such as a site that was removed.
        let path = isolated("second-copy");
        let shared = Shared::read_from(path.clone(), None).unwrap();
        assert!(!path.exists());
        shared.persist().unwrap();
        assert!(path.exists());
    }

    #[test]
    fn the_cashier_sees_how_long_the_site_will_wait() {
        let shared = std::sync::Arc::new(Shared::load_from(isolated("countdown"), None).unwrap());
        let watcher = {
            let shared = std::sync::Arc::clone(&shared);
            std::thread::spawn(move || {
                for _ in 0..500 {
                    if let Some(request) = shared.snapshot().pair_request {
                        shared.answer_pair(request.serial, false);
                        return request.seconds_left;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                panic!("no pairing request showed up");
            })
        };
        let _ = shared.request_pair("https://shop.example", "x", Duration::from_secs(30));
        let left = watcher.join().unwrap();
        assert!((25..=30).contains(&left), "{left}");
    }

    #[test]
    fn a_site_can_drop_only_its_own_pairing() {
        let path = isolated("unpair");
        let shared = std::sync::Arc::new(Shared::load_from(path, None).unwrap());
        let cashier = answer_when_asked(&shared, true);
        let code = shared
            .request_pair("https://shop.example", "Shop", Duration::from_secs(5))
            .unwrap();
        cashier.join().unwrap();
        // Another site holding the code, or no origin, cannot revoke it.
        assert!(!shared.unpair(Some("https://evil.example"), &code));
        assert!(!shared.unpair(None, &code));
        assert!(shared.authorize(Some("https://shop.example"), &code));
        assert!(shared.unpair(Some("https://shop.example"), &code));
        assert!(!shared.authorize(Some("https://shop.example"), &code));
        assert!(shared.snapshot().sites.is_empty());
    }

    #[test]
    fn an_allowed_site_gets_a_code_that_only_works_from_that_site() {
        let path = isolated("pair");
        let shared = std::sync::Arc::new(Shared::load_from(path.clone(), None).unwrap());
        let cashier = answer_when_asked(&shared, true);
        let code = shared
            .request_pair(
                "https://Shop.Example",
                "Shop · Chrome",
                Duration::from_secs(5),
            )
            .unwrap();
        cashier.join().unwrap();
        assert!(shared.authorize(Some("https://shop.example"), &code));
        // Another site that somehow learned the code still cannot print.
        assert!(!shared.authorize(Some("https://evil.example"), &code));
        assert!(!shared.authorize(None, &code));
        assert_eq!(shared.snapshot().sites, ["https://shop.example"]);

        // The site survives a restart, and removing it stops its code at once.
        let reloaded = Shared::load_from(path, None).unwrap();
        assert!(reloaded.authorize(Some("https://shop.example"), &code));
        reloaded.remove_site("https://shop.example");
        assert!(!reloaded.authorize(Some("https://shop.example"), &code));
    }

    #[test]
    fn plain_http_is_insecure_except_on_this_computer() {
        for origin in [
            "http://192.168.1.10",
            "http://till.local:8080",
            "http://shop.example",
            "http://localhost.evil.example",
        ] {
            assert!(insecure_origin(origin), "{origin}");
        }
        for origin in [
            "https://shop.example",
            "http://localhost",
            "http://localhost:3000",
            "http://app.localhost:3000",
            "http://127.0.0.1:5173",
            "http://[::1]:3000",
        ] {
            assert!(!insecure_origin(origin), "{origin}");
        }
    }

    #[test]
    fn an_http_site_is_flagged_and_can_be_shut_out() {
        // Some shops run the till from an in-house http server, so http is
        // allowed by default. The prompt says so, and one switch refuses it.
        let shared = std::sync::Arc::new(Shared::load_from(isolated("http-site"), None).unwrap());
        let watcher = {
            let shared = std::sync::Arc::clone(&shared);
            std::thread::spawn(move || {
                for _ in 0..500 {
                    if let Some(request) = shared.snapshot().pair_request {
                        shared.answer_pair(request.serial, true);
                        return request.insecure;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                panic!("no pairing request showed up");
            })
        };
        let code = shared
            .request_pair("http://192.168.1.10", "x", Duration::from_secs(5))
            .unwrap();
        assert!(watcher.join().unwrap(), "the prompt did not flag http");
        assert!(shared.authorize(Some("http://192.168.1.10"), &code));

        shared.set_https_only(true);
        // The code it already holds stops working, and it cannot ask again.
        assert!(!shared.authorize(Some("http://192.168.1.10"), &code));
        assert_eq!(
            shared.request_pair("http://192.168.1.10", "x", Duration::from_secs(5)),
            Err(PairError::Insecure)
        );
        // Nobody was asked, so nothing is on screen.
        assert!(shared.snapshot().pair_request.is_none());

        // Turned off again, the same code works: nothing was thrown away.
        shared.set_https_only(false);
        assert!(shared.authorize(Some("http://192.168.1.10"), &code));
    }

    #[test]
    fn a_turned_down_site_cannot_keep_asking() {
        // A hostile page must not be able to hold the prompt on screen.
        let shared = std::sync::Arc::new(Shared::load_from(isolated("deny"), None).unwrap());
        let cashier = answer_when_asked(&shared, false);
        let first = shared.request_pair("https://evil.example", "x", Duration::from_secs(5));
        cashier.join().unwrap();
        assert_eq!(first, Err(PairError::Denied));
        assert_eq!(
            shared.request_pair("https://evil.example", "x", Duration::from_secs(5)),
            Err(PairError::Blocked)
        );
        assert!(shared.snapshot().sites.is_empty());
    }

    #[test]
    fn pairing_needs_a_web_origin_and_gives_up_when_nobody_answers() {
        let shared = Shared::load_from(isolated("unanswered"), None).unwrap();
        for origin in [
            "null",
            "file://",
            "chrome-extension://abc",
            "https://",
            "https://a/b",
        ] {
            assert_eq!(
                shared.request_pair(origin, "x", Duration::from_secs(5)),
                Err(PairError::BadOrigin),
                "{origin}"
            );
        }
        assert_eq!(
            shared.request_pair("http://localhost:3000", "x", Duration::from_millis(50)),
            Err(PairError::TimedOut)
        );
        // The prompt is gone, so the window does not ask about a page that left.
        assert!(shared.snapshot().pair_request.is_none());
        // And the page cannot pop the window straight back up.
        assert_eq!(
            shared.request_pair("http://localhost:3000", "x", Duration::from_millis(50)),
            Err(PairError::Blocked)
        );
    }

    #[test]
    fn pairing_again_replaces_the_code_and_warns_the_cashier() {
        // A second request for a site that can already print is unusual. It
        // may be a program on this computer faking the origin, so the window
        // says so, and the old code stops working.
        let shared = std::sync::Arc::new(Shared::load_from(isolated("pair-again"), None).unwrap());
        let cashier = answer_when_asked(&shared, true);
        let first = shared
            .request_pair("https://shop.example", "x", Duration::from_secs(5))
            .unwrap();
        cashier.join().unwrap();

        let watcher = {
            let shared = std::sync::Arc::clone(&shared);
            std::thread::spawn(move || {
                for _ in 0..500 {
                    if let Some(request) = shared.snapshot().pair_request {
                        assert!(request.known);
                        shared.answer_pair(request.serial, true);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                panic!("no pairing request showed up");
            })
        };
        let second = shared
            .request_pair("https://shop.example", "x", Duration::from_secs(5))
            .unwrap();
        watcher.join().unwrap();
        assert_ne!(first, second);
        assert!(!shared.authorize(Some("https://shop.example"), &first));
        assert!(shared.authorize(Some("https://shop.example"), &second));
        assert_eq!(shared.snapshot().sites, ["https://shop.example"]);
    }

    #[test]
    fn a_click_meant_for_an_earlier_request_is_ignored() {
        // The cashier clicks Allow just as one request times out and another
        // site's request appears. That click must not allow the new site.
        let shared = std::sync::Arc::new(Shared::load_from(isolated("stale"), None).unwrap());
        let late = {
            let shared = std::sync::Arc::clone(&shared);
            std::thread::spawn(move || {
                for _ in 0..500 {
                    if let Some(request) = shared.snapshot().pair_request {
                        shared.answer_pair(request.serial - 1, true);
                        return request.serial;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                panic!("no pairing request showed up");
            })
        };
        assert_eq!(
            shared.request_pair("https://evil.example", "x", Duration::from_millis(300)),
            Err(PairError::TimedOut)
        );
        late.join().unwrap();
        assert!(shared.snapshot().sites.is_empty());
    }

    #[test]
    fn a_pairing_that_could_not_be_saved_leaves_no_working_code() {
        // The page hears "failed" and gets no code, so no code may work.
        let path = isolated("unsaved");
        let shared = std::sync::Arc::new(Shared::load_from(path.clone(), None).unwrap());
        // A folder where the temporary file goes makes every save fail.
        fs::create_dir_all(path.with_extension("json.tmp")).unwrap();
        let cashier = answer_when_asked(&shared, true);
        let result = shared.request_pair("https://shop.example", "x", Duration::from_secs(5));
        cashier.join().unwrap();
        assert!(matches!(result, Err(PairError::Failed(_))), "{result:?}");
        assert!(shared.snapshot().sites.is_empty());
    }

    #[test]
    fn after_a_no_every_site_waits_before_it_can_ask() {
        // One page can own many origins. Without a pause it could keep a
        // prompt on screen by asking from a new one each time.
        let shared = std::sync::Arc::new(Shared::load_from(isolated("gap"), None).unwrap());
        let cashier = answer_when_asked(&shared, false);
        let _ = shared.request_pair("https://a1.evil.example", "x", Duration::from_secs(5));
        cashier.join().unwrap();
        assert_eq!(
            shared.request_pair("https://a2.evil.example", "x", Duration::from_secs(5)),
            Err(PairError::Blocked)
        );
    }

    #[test]
    fn a_job_that_panicked_does_not_stop_the_printer_scan() {
        // The scan skips while a job runs. A crashed job must not look like
        // one that runs forever, or a replugged printer is never seen.
        let shared = std::sync::Arc::new(Shared::load_from(isolated("poison"), None).unwrap());
        let crashing = std::sync::Arc::clone(&shared);
        let _ = std::thread::spawn(move || {
            let _job = crashing.printer_lock();
            panic!("job crashed");
        })
        .join();
        assert!(!shared.printer_busy());
    }

    #[test]
    fn a_pairing_message_does_not_hide_a_printer_problem() {
        let shared = Shared::load_from(isolated("keep-problem"), None).unwrap();
        shared.set_status(Tone::Problem, "the printer is out of paper");
        let _ = shared.request_pair("https://shop.example", "x", Duration::from_millis(20));
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.tone, Tone::Problem);
        assert_eq!(snapshot.status, "The printer is out of paper");
    }

    #[test]
    fn drawer_settings_from_the_file_become_the_pulse() {
        let path = isolated("drawer");
        fs::write(
            &path,
            format!(
                r#"{{"pairing_code":"{}","drawer_pin":5,"drawer_on_ms":100,"drawer_off_ms":9000}}"#,
                "cd".repeat(16)
            ),
        )
        .unwrap();
        let shared = Shared::load_from(path, None).unwrap();
        assert_eq!(shared.snapshot().drawer, [0x1b, 0x70, 1, 50, 255]);
        assert!(!shared.snapshot().allow_raw);
    }

    #[test]
    fn a_short_code_is_replaced_and_a_broken_file_is_left_alone() {
        let path = isolated("repair");
        fs::write(
            &path,
            r#"{"port":9240,"pairing_code":"123456","paper_mm":80,"transport":"usb","tcp_host":"","tcp_port":9100,"usb_id":null}"#,
        )
        .unwrap();
        let shared = Shared::load_from(path.clone(), None).unwrap();
        assert_eq!(shared.snapshot().pairing_code.len(), 32);
        assert_ne!(shared.snapshot().pairing_code, "123456");
        assert!(shared.snapshot().status.contains("too short"));

        let broken = isolated("broken");
        fs::write(&broken, b"{ this is not json").unwrap();
        let error = match Shared::load_from(broken.clone(), None) {
            Err(error) => error,
            Ok(_) => panic!("a broken settings file was accepted"),
        };
        assert!(error.contains("left unchanged"));
        assert_eq!(fs::read(&broken).unwrap(), b"{ this is not json");
    }

    #[test]
    fn settings_file_is_private_on_unix() {
        let path = isolated("mode");
        let _shared = Shared::load_from(path.clone(), None).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn a_till_page_is_named_without_keeping_a_raw_header() {
        assert_eq!(
            client_label(Some("KHEvents"), Some("Mozilla Chrome/120.0 Safari/537")),
            "KHEvents · Chrome"
        );
        assert_eq!(
            client_label(Some("KHEvents <script>"), Some("curl/8.0")),
            "curl"
        );
        assert_eq!(client_label(Some("   "), None), "This computer");
        assert_eq!(
            client_label(Some(&"A".repeat(41)), Some("Firefox/120")),
            "Firefox"
        );

        let shared = Shared::load_from(isolated("clients"), None).unwrap();
        shared.note_client("KHEvents · Chrome");
        shared.note_client("");
        shared.note_job("Printed", true);
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.clients.len(), 1);
        assert!(snapshot.clients[0].live);
        assert_eq!(snapshot.clients[0].label, "KHEvents · Chrome");
        assert_eq!(snapshot.jobs, 1);
        assert!(snapshot.last_job.as_deref().unwrap().starts_with("Printed"));

        // "Jobs printed" must not grow when the printer failed. The failure
        // still shows as the last job, so the cashier sees what happened.
        shared.note_job("The printer is out of paper", false);
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.jobs, 1);
        assert!(
            snapshot
                .last_job
                .as_deref()
                .unwrap()
                .contains("out of paper")
        );
        assert!(!snapshot.last_job.as_deref().unwrap().contains("pairing"));
    }
}
