//! Start TapPrint when the user logs in, so a rebooted till prints again
//! without anyone opening the app.
//!
//! - macOS: a LaunchAgent in `~/Library/LaunchAgents`.
//! - Windows: the `Run` key under `HKEY_CURRENT_USER`.
//! - Linux: an XDG autostart entry in `~/.config/autostart`.
//!
//! All three are per-user and need no administrator rights.

use std::path::{Path, PathBuf};

pub fn enabled() -> bool {
    platform::enabled()
}

pub fn set(on: bool) -> Result<(), String> {
    let program = std::env::current_exe()
        .map_err(|error| format!("could not find this app on disk: {error}"))?;
    if on {
        platform::enable(&program)
    } else {
        platform::disable()
    }
}

/// The app may have been moved since the login item was written. Point the
/// item at this copy, so login does not open an old or missing path.
pub fn refresh() {
    if enabled() {
        let _ = set(true);
    }
}

/// `/Applications/TapPrint.app` when running from a bundle, else the binary.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_bundle(program: &Path) -> Option<PathBuf> {
    let macos = program.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    is_bundle.then(|| bundle.to_path_buf())
}

#[cfg(target_os = "macos")]
mod platform {
    use std::fs;
    use std::path::{Path, PathBuf};

    const LABEL: &str = "com.khable.tapprint";

    fn agent_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }

    pub fn enabled() -> bool {
        agent_path().exists()
    }

    pub fn enable(program: &Path) -> Result<(), String> {
        // `open` starts the bundle the way Finder does, with its Dock icon and
        // single-copy rule. A bare binary path is the fallback for `cargo run`.
        let arguments = match super::app_bundle(program) {
            Some(bundle) => vec!["/usr/bin/open".to_string(), bundle.display().to_string()],
            None => vec![program.display().to_string()],
        };
        let arguments: String = arguments
            .iter()
            .map(|argument| format!("\t\t<string>{}</string>\n", xml_escape(argument)))
            .collect();
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
\t<key>Label</key>\n\t<string>{LABEL}</string>\n\
\t<key>ProgramArguments</key>\n\t<array>\n{arguments}\t</array>\n\
\t<key>RunAtLoad</key>\n\t<true/>\n\
</dict>\n</plist>\n"
        );
        let path = agent_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        if fs::read_to_string(&path).is_ok_and(|current| current == body) {
            return Ok(());
        }
        fs::write(&path, body)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    pub fn disable() -> Result<(), String> {
        match fs::remove_file(agent_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove the login item: {error}")),
        }
    }

    fn xml_escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::path::Path;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows::core::HSTRING;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "TapPrint";

    pub fn enabled() -> bool {
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(RUN),
                &HSTRING::from(NAME),
                RRF_RT_REG_SZ,
                None,
                None,
                None,
            )
        };
        status == ERROR_SUCCESS
    }

    pub fn enable(program: &Path) -> Result<(), String> {
        // Quoted, so a path with spaces such as `C:\Program Files` still runs.
        let command = format!("\"{}\"", program.display());
        let data: Vec<u16> = command.encode_utf16().chain([0]).collect();
        let bytes =
            u32::try_from(data.len() * 2).map_err(|_| "app path is too long".to_string())?;
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(RUN),
                &HSTRING::from(NAME),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                bytes,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("could not add the login item (error {})", status.0))
        }
    }

    pub fn disable() -> Result<(), String> {
        let status = unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN), &HSTRING::from(NAME))
        };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!(
                "could not remove the login item (error {})",
                status.0
            ))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use std::fs;
    use std::path::{Path, PathBuf};

    fn entry_path() -> PathBuf {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default()
                    .join(".config")
            });
        root.join("autostart").join("tapprint.desktop")
    }

    pub fn enabled() -> bool {
        entry_path().exists()
    }

    pub fn enable(program: &Path) -> Result<(), String> {
        let mut exec = String::new();
        for ch in program.display().to_string().chars() {
            if matches!(ch, '"' | '`' | '$' | '\\') {
                exec.push('\\');
            }
            // `%` starts a field code in `Exec`. A literal one is written twice.
            if ch == '%' {
                exec.push('%');
            }
            exec.push(ch);
        }
        let body = format!(
            "[Desktop Entry]\nType=Application\nName=TapPrint\nExec=\"{exec}\"\nX-GNOME-Autostart-enabled=true\n"
        );
        let path = entry_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        fs::write(&path, body)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    pub fn disable() -> Result<(), String> {
        match fs::remove_file(entry_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove the login item: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_inside_an_app_bundle_opens_the_bundle() {
        // Login must open the .app, not the inner binary, so macOS treats it
        // as the same app the cashier opens from Finder.
        let program = Path::new("/Applications/TapPrint.app/Contents/MacOS/tapprint");
        assert_eq!(
            app_bundle(program),
            Some(PathBuf::from("/Applications/TapPrint.app"))
        );
        assert_eq!(app_bundle(Path::new("/usr/local/bin/tapprint")), None);
    }
}
