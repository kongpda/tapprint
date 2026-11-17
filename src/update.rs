//! Find a newer TapPrint on GitHub Releases, and install it on request.
//!
//! Checking is automatic: once shortly after start, then once a day. Nothing
//! is ever installed without a click, because a till must not restart in the
//! middle of a sale.
//!
//! An update is only installed when its checksums carry a valid signature
//! from the key in `release/update-key.pub`. The signature also names the
//! version, so an old signed release cannot be passed off as a new one.
//! Without a key in that file, the window offers the download page instead.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::model::Shared;

const RELEASES_API: &str = "https://api.github.com/repos/kongpda/tapprint";
const KEY_FILE: &str = include_str!("../release/update-key.pub");
/// Wait this long after start, so opening the app is never slowed down.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const SMALL_FILE: u64 = 64 * 1024;
/// No build is anywhere near this. It only stops an endless download.
const LARGEST_BUILD: u64 = 300 * 1024 * 1024;
/// How long a restarted copy keeps trying to take over the port.
pub const RESTART_WAIT: Duration = Duration::from_secs(20);

// One build per chip, so a Mac only downloads the half it can run.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const ASSET: &str = "TapPrint-macos-arm64.zip";
#[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
const ASSET: &str = "TapPrint-macos-x86_64.zip";
#[cfg(target_os = "windows")]
const ASSET: &str = "tapprint-windows-x86_64.exe";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const ASSET: &str = "tapprint-linux-x86_64";

/// A release newer than this copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The release page, for people who would sooner download it themselves.
    pub page: String,
    /// File name and download address of each file in the release.
    files: Vec<(String, String)>,
}

impl Release {
    fn file(&self, name: &str) -> Result<&str, String> {
        self.files
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, address)| address.as_str())
            .ok_or_else(|| format!("the release has no file named {name}"))
    }
}

/// Can this copy install updates itself? It needs the key that signs them.
pub fn can_install() -> bool {
    public_key().is_some()
}

/// Check now and then for as long as the app runs.
pub fn watch(shared: std::sync::Arc<Shared>) {
    let _ = std::thread::Builder::new()
        .name("tapprint-update".into())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK);
            loop {
                check(&shared);
                std::thread::sleep(CHECK_EVERY);
            }
        });
}

/// Look for a newer release, unless the person turned checking off. A
/// failed check is silent: the till may simply be offline.
pub fn check(shared: &Shared) {
    if !shared.snapshot().check_updates {
        return;
    }
    if let Ok(found) = newer_release(&api(), env!("CARGO_PKG_VERSION")) {
        shared.set_update(found);
    }
}

fn api() -> String {
    // A debug build can be pointed at a stand-in server for testing. A
    // release build always asks GitHub.
    if cfg!(debug_assertions)
        && let Ok(address) = std::env::var("TAPPRINT_UPDATE_API")
    {
        return address;
    }
    RELEASES_API.to_string()
}

fn public_key() -> Option<minisign_verify::PublicKey> {
    if cfg!(debug_assertions)
        && let Ok(key) = std::env::var("TAPPRINT_UPDATE_KEY")
    {
        return minisign_verify::PublicKey::from_base64(key.trim()).ok();
    }
    key_in(KEY_FILE)
}

/// The key in a minisign public key file: its last line that is not a comment.
fn key_in(file: &str) -> Option<minisign_verify::PublicKey> {
    let line = file
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with("untrusted comment:"))?;
    minisign_verify::PublicKey::from_base64(line).ok()
}

/// The latest release, when it is newer than `current`.
fn newer_release(api: &str, current: &str) -> Result<Option<Release>, String> {
    let body = match fetch(&format!("{api}/releases/latest"), SMALL_FILE) {
        Ok(body) => body,
        // No release has been published yet.
        Err(error) if error.contains("404") => return Ok(None),
        Err(error) => return Err(error),
    };
    let json: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "GitHub sent an unreadable reply".to_string())?;
    let tag = json["tag_name"].as_str().unwrap_or_default();
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    if !is_newer(&version, current) {
        return Ok(None);
    }
    let files = json["assets"]
        .as_array()
        .map(|assets| {
            assets
                .iter()
                .filter_map(|asset| {
                    Some((
                        asset["name"].as_str()?.to_string(),
                        asset["browser_download_url"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(Release {
        page: json["html_url"]
            .as_str()
            .filter(|page| page.starts_with("https://github.com/"))
            .unwrap_or("https://github.com/kongpda/tapprint/releases")
            .to_string(),
        version,
        files,
    }))
}

/// `1.2.3` as numbers, and whether a pre-release tag such as `-rc.1` follows.
fn parse(version: &str) -> Option<([u64; 3], bool)> {
    let (core, tag) = match version.split_once('-') {
        Some((core, tag)) if !tag.is_empty() => (core, true),
        Some(_) => return None,
        None => (version, false),
    };
    let mut parts = core.split('.');
    let numbers = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some((numbers, tag))
}

/// Only a finished release is ever offered. A copy that is itself a test
/// build, such as `0.1.0-rc.1`, is offered the finished `0.1.0`.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some((candidate, false)), Some((current, test_build))) => {
            candidate > current || (candidate == current && test_build)
        }
        _ => false,
    }
}

fn fetch(address: &str, limit: u64) -> Result<Vec<u8>, String> {
    let secure = address.starts_with("https://");
    if !secure && !cfg!(debug_assertions) {
        return Err("updates are only fetched over https".to_string());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into();
    let mut response = agent
        .get(address)
        .header(
            "User-Agent",
            concat!("tapprint/", env!("CARGO_PKG_VERSION")),
        )
        .header(
            "Accept",
            "application/vnd.github+json, application/octet-stream",
        )
        .call()
        .map_err(|error| format!("could not download {address}: {error}"))?;
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|error| format!("could not download {address}: {error}"))?;
    if body.len() as u64 > limit {
        return Err(format!("{address} is larger than expected"));
    }
    Ok(body)
}

/// Download `release`, check it, and put it in place of this copy. Returns
/// what to start. The running copy keeps working until the caller quits.
pub fn install(release: &Release, folder: &Path) -> Result<PathBuf, String> {
    let key = public_key().ok_or("this copy cannot check updates, so it will not install one")?;
    let sums = fetch(release.file("SHA256SUMS")?, SMALL_FILE)?;
    let signature = fetch(release.file("SHA256SUMS.minisig")?, SMALL_FILE)?;
    let expected = signed_checksum(&key, &sums, &signature, &release.version, ASSET)?;
    let build = fetch(release.file(ASSET)?, LARGEST_BUILD)?;
    if hex(&Sha256::digest(&build)) != expected {
        return Err("the download does not match its signed checksum. Nothing was changed.".into());
    }
    let program = std::env::current_exe()
        .map_err(|error| format!("could not find this app on disk: {error}"))?;
    let started = replace(&program, &build)?;
    // The new copy starts while this one still holds the port. This note
    // tells it to wait for the port instead of giving up.
    fs::create_dir_all(folder).ok();
    let _ = fs::write(restart_note(folder), b"");
    Ok(started)
}

/// The checksum of `file` from a signed `SHA256SUMS`, after checking that
/// the signature is ours and was made for exactly this `version`.
fn signed_checksum(
    key: &minisign_verify::PublicKey,
    sums: &[u8],
    signature: &[u8],
    version: &str,
    file: &str,
) -> Result<String, String> {
    let signature = std::str::from_utf8(signature)
        .ok()
        .and_then(|text| minisign_verify::Signature::decode(text).ok())
        .ok_or("the update's signature is unreadable")?;
    key.verify(sums, &signature, false)
        .map_err(|_| "the update is not signed by TapPrint. Nothing was changed.")?;
    // Signed along with the checksums. Without this, an old release with a
    // genuine signature could be offered again under a newer number.
    if signature.trusted_comment() != format!("tapprint {version}") {
        return Err("the update's signature is for another version. Nothing was changed.".into());
    }
    let sums = std::str::from_utf8(sums).map_err(|_| "the update's checksums are unreadable")?;
    sums.lines()
        .find_map(|line| {
            let (sum, name) = line.split_once(char::is_whitespace)?;
            (name.trim().trim_start_matches('*') == file).then(|| sum.to_ascii_lowercase())
        })
        .ok_or_else(|| format!("the update has no checksum for {file}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn restart_note(folder: &Path) -> PathBuf {
    folder.join("restarting")
}

/// Was this copy started by an update a moment ago? Then the old copy is
/// still letting go of the port, and it is worth waiting for.
pub fn restarting(folder: &Path) -> bool {
    let note = restart_note(folder);
    let fresh = fs::metadata(&note)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|written| SystemTime::now().duration_since(written).ok())
        .is_some_and(|age| age < Duration::from_secs(120));
    let _ = fs::remove_file(note);
    fresh
}

/// Start the copy `install` returned.
pub fn start(program: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/bin/open");
        command.arg("-n").arg(program);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(program);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("the update is in place, but could not be started: {error}"))
}

/// Remove what an earlier update left behind.
pub fn tidy() {
    if let Ok(program) = std::env::current_exe() {
        let _ = fs::remove_file(aside(&program));
    }
}

/// Where the old file waits while the new one is moved in.
fn aside(program: &Path) -> PathBuf {
    let mut name = program.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    program.with_file_name(name)
}

/// Swap the program file for `build`. The new file is written beside the old
/// one first, so the swap itself is a rename and cannot be left half done.
#[cfg(not(target_os = "macos"))]
fn replace(program: &Path, build: &[u8]) -> Result<PathBuf, String> {
    let fresh = {
        let mut name = program.file_name().unwrap_or_default().to_os_string();
        name.push(".new");
        program.with_file_name(name)
    };
    let failed = |what: &str, error: std::io::Error| {
        format!(
            "could not {what} {}: {error}. Nothing was changed.",
            program.display()
        )
    };
    fs::write(&fresh, build).map_err(|error| failed("write the update beside", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fresh, fs::Permissions::from_mode(0o755))
            .map_err(|error| failed("prepare the update for", error))?;
    }
    // Windows will not overwrite a running program, but it lets it be
    // renamed. So the old file steps aside, and is removed at the next start.
    let old = aside(program);
    let _ = fs::remove_file(&old);
    fs::rename(program, &old).map_err(|error| {
        let _ = fs::remove_file(&fresh);
        failed("move aside", error)
    })?;
    if let Err(error) = fs::rename(&fresh, program) {
        let _ = fs::rename(&old, program);
        let _ = fs::remove_file(&fresh);
        return Err(failed("replace", error));
    }
    Ok(program.to_path_buf())
}

/// Swap the whole `.app` for the one in the downloaded zip.
#[cfg(target_os = "macos")]
fn replace(program: &Path, build: &[u8]) -> Result<PathBuf, String> {
    let bundle = crate::autostart::app_bundle(program)
        .ok_or("this copy is not inside TapPrint.app, so it cannot update itself")?;
    let parent = bundle.parent().ok_or("TapPrint.app has no folder")?;
    let stage = parent.join(format!(".TapPrint.update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage);
    let cleanup = |message: String| {
        let _ = fs::remove_dir_all(&stage);
        message
    };
    fs::create_dir_all(&stage).map_err(|error| {
        format!(
            "could not write to {}: {error}. Move TapPrint to a folder you can change, or download the update.",
            parent.display()
        )
    })?;
    let zip = stage.join("update.zip");
    fs::write(&zip, build)
        .map_err(|error| cleanup(format!("could not save the update: {error}")))?;
    let run = |program: &str, arguments: &[&std::ffi::OsStr]| {
        Command::new(program)
            .args(arguments)
            .output()
            .is_ok_and(|output| output.status.success())
    };
    if !run(
        "/usr/bin/ditto",
        &[
            "-x".as_ref(),
            "-k".as_ref(),
            zip.as_os_str(),
            stage.as_os_str(),
        ],
    ) {
        return Err(cleanup(
            "could not unpack the update. Nothing was changed.".into(),
        ));
    }
    let fresh = stage.join("TapPrint.app");
    if !run(
        "/usr/bin/codesign",
        &["--verify".as_ref(), fresh.as_os_str()],
    ) {
        return Err(cleanup(
            "the update is damaged. Nothing was changed.".into(),
        ));
    }
    let old = stage.join("previous.app");
    fs::rename(&bundle, &old)
        .map_err(|error| cleanup(format!("could not move the old TapPrint aside: {error}")))?;
    if let Err(error) = fs::rename(&fresh, &bundle) {
        let _ = fs::rename(&old, &bundle);
        return Err(cleanup(format!(
            "could not put the update in place: {error}"
        )));
    }
    let _ = fs::remove_dir_all(&stage);
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway key made for these tests, and a `SHA256SUMS` signed with it
    // for version 0.2.0. The secret half was deleted after signing.
    const KEY: &str = "untrusted comment: minisign public key DB722D5EA8CB10E9\nRWTpEMuoXi1y2xLYEWUauKBc6jycVbdMpRqy7Mg/39aXuAU8Ij4Fps3s\n";
    const SUMS: &[u8] = b"aaaa  tapprint-linux-x86_64\nbbbb  TapPrint-macos-universal.zip\n";
    const SIGNATURE: &[u8] = b"untrusted comment: signature from minisign secret key\nRUTpEMuoXi1y20FJ9+MWAJVGfm0s4jQ0A42sb6nNOelg30lGejT6m8ba908TI5qJTlAkwF1+Nf1t7t3XuvW/ITZS/lsFqX6igQ8=\ntrusted comment: tapprint 0.2.0\nj45n5BxLxW7pJELefGgqDcIaWHljy1MAVgvtCRy2cQwvqA1aPOdprMrh0mWwDJDVVi08SAYMybsypttlVLsKAg==\n";

    #[test]
    fn only_a_higher_release_number_is_an_update() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        // A test build must never be handed to a till.
        assert!(!is_newer("0.2.0-rc.1", "0.1.0"));
        assert!(!is_newer("0.2.0-", "0.1.0"));
        // A till on a test build moves to the finished release of the same
        // number, and to anything later, but not to an older one.
        assert!(is_newer("0.1.0", "0.1.0-rc.1"));
        assert!(is_newer("0.2.0", "0.1.0-rc.1"));
        assert!(!is_newer("0.0.9", "0.1.0-rc.1"));
        assert!(!is_newer("0.1.0-rc.2", "0.1.0-rc.1"));
        assert!(!is_newer("latest", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
    }

    #[test]
    fn an_update_must_be_signed_for_exactly_its_version() {
        let key = key_in(KEY).unwrap();
        let sum = |sums: &[u8], signature: &[u8], version: &str, file: &str| {
            signed_checksum(&key, sums, signature, version, file)
        };
        assert_eq!(
            sum(SUMS, SIGNATURE, "0.2.0", "tapprint-linux-x86_64").as_deref(),
            Ok("aaaa")
        );
        assert_eq!(
            sum(SUMS, SIGNATURE, "0.2.0", "TapPrint-macos-universal.zip").as_deref(),
            Ok("bbbb")
        );
        // The same genuine release, offered again as a newer one.
        assert!(sum(SUMS, SIGNATURE, "0.3.0", "tapprint-linux-x86_64").is_err());
        // One checksum changed after signing.
        let forged = b"cccc  tapprint-linux-x86_64\nbbbb  TapPrint-macos-universal.zip\n";
        assert!(sum(forged, SIGNATURE, "0.2.0", "tapprint-linux-x86_64").is_err());
        assert!(sum(SUMS, b"not a signature", "0.2.0", "tapprint-linux-x86_64").is_err());
        assert!(sum(SUMS, SIGNATURE, "0.2.0", "tapprint.exe").is_err());
    }

    #[test]
    fn the_key_file_without_a_key_turns_installing_off() {
        // The repository starts with a comment and no key. Such a copy only
        // points at the download page.
        assert!(key_in("untrusted comment: no key yet\n").is_none());
        assert!(key_in("").is_none());
        assert!(key_in("untrusted comment: x\nnot base64 at all\n").is_none());
        assert!(key_in(KEY).is_some());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_program_file_is_swapped_and_the_old_one_tidied_away() {
        let folder = std::env::temp_dir().join(format!("tapprint-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let program = folder.join("tapprint-test-program");
        fs::write(&program, b"old").unwrap();
        let started = replace(&program, b"new build").unwrap();
        assert_eq!(started, program);
        assert_eq!(fs::read(&program).unwrap(), b"new build");
        // The old file is kept only until the next start.
        assert_eq!(fs::read(aside(&program)).unwrap(), b"old");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&program).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the update cannot be run");
        }
        // A second update replaces the first one's leftovers.
        replace(&program, b"newer build").unwrap();
        assert_eq!(fs::read(&program).unwrap(), b"newer build");
        assert_eq!(fs::read(aside(&program)).unwrap(), b"new build");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_copy_started_by_an_update_knows_to_wait_for_the_port() {
        let folder = std::env::temp_dir().join(format!("tapprint-note-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        assert!(!restarting(&folder));
        fs::write(restart_note(&folder), b"").unwrap();
        assert!(restarting(&folder));
        // The note is used once. An ordinary second copy must not wait.
        assert!(!restarting(&folder));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_release_is_read_from_githubs_reply() {
        use std::io::Write;
        // A stand-in for api.github.com, answering one request.
        let serve = |status: &'static str, body: &'static str| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let read = socket.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..read]).contains("/releases/latest"));
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            format!("http://127.0.0.1:{port}")
        };
        let reply = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/kongpda/tapprint/releases/tag/v0.2.0","assets":[{"name":"SHA256SUMS","browser_download_url":"https://example.invalid/SHA256SUMS"}]}"#;
        let found = newer_release(&serve("200 OK", reply), "0.1.0")
            .unwrap()
            .unwrap();
        assert_eq!(found.version, "0.2.0");
        assert_eq!(
            found.file("SHA256SUMS"),
            Ok("https://example.invalid/SHA256SUMS")
        );
        assert!(found.file("tapprint.exe").is_err());
        // Already on it: nothing to offer.
        assert_eq!(newer_release(&serve("200 OK", reply), "0.2.0"), Ok(None));
        // No release published yet is not an error.
        assert_eq!(
            newer_release(&serve("404 Not Found", "{}"), "0.1.0"),
            Ok(None)
        );
    }
}
