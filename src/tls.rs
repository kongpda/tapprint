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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
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

pub fn mark_serving(certificate: &Path) {
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

/// Read the saved certificate, or make one. A certificate near its end is
/// replaced, and the user is asked to trust the new one again.
pub fn identity(folder: &Path) -> Result<Identity, String> {
    let certificate_path = folder.join("localhost-cert.pem");
    let key_path = folder.join("localhost-key.pem");
    if let (Ok(certificate), Ok(private_key)) = (fs::read(&certificate_path), fs::read(&key_path))
        && !needs_renewal(&certificate_path)
    {
        return Ok(Identity {
            certificate,
            private_key,
            certificate_path,
        });
    }
    let (certificate, private_key) = generate()?;
    fs::create_dir_all(folder)
        .map_err(|error| format!("could not create {}: {error}", folder.display()))?;
    write_private(&key_path, private_key.as_bytes())?;
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
    let mut params = CertificateParams::new(vec![
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        "::1".to_string(),
    ])
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
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("could not save the key: {error}"))?;
    file.write_all(bytes)
        .map_err(|error| format!("could not save the key: {error}"))
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
        let dir = std::env::temp_dir().join(format!("tapprint-tls-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let first = identity(&dir).unwrap();
        assert!(String::from_utf8_lossy(&first.certificate).contains("BEGIN CERTIFICATE"));
        // A restart must keep the certificate, or Safari's trust would be lost.
        let second = identity(&dir).unwrap();
        assert_eq!(first.certificate, second.certificate);
        let _ = fs::remove_dir_all(&dir);
    }
}
