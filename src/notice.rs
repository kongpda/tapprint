//! Show a startup failure to the cashier.
//!
//! The app is opened from Finder, the Start menu, or at login, so there is no
//! terminal to read `stderr`. Without this the app would vanish silently.

pub fn fatal(message: &str) {
    eprintln!("tapprint: {message}");
    show(message);
}

#[cfg(target_os = "macos")]
fn show(message: &str) {
    // The text goes in as an argument, not inside the script, so quotes in an
    // error message cannot change what AppleScript runs.
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display alert \"TapPrint could not start\" message (item 1 of argv) as critical",
            "-e",
            "end run",
            "--",
            message,
        ])
        .status();
}

#[cfg(target_os = "windows")]
fn show(message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::PCWSTR;

    let text: Vec<u16> = message.encode_utf16().chain([0]).collect();
    let title: Vec<u16> = "TapPrint could not start"
        .encode_utf16()
        .chain([0])
        .collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn show(message: &str) {
    // Most desktops ship one of these. If neither is there, stderr is all we have.
    for (program, args) in [
        (
            "zenity",
            vec!["--error", "--title=TapPrint", "--text", message],
        ),
        ("kdialog", vec!["--title", "TapPrint", "--error", message]),
    ] {
        if std::process::Command::new(program)
            .args(args)
            .status()
            .is_ok()
        {
            return;
        }
    }
}
