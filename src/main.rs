//! TapPrint: a menu-bar helper that prints a PNG to a USB or network
//! thermal printer, with no vendor driver installed.

// A release build on Windows is a tray app. Without this, a console window
// opens next to it and closing that window quits the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod autostart;
mod cli;
mod http;
mod model;
mod notice;
mod print;
mod share;
mod station;
// Only macOS serves HTTPS. Other Unix systems build it for the tests alone,
// so the relay is checked wherever the tests run.
#[cfg(any(target_os = "macos", all(test, unix)))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod tls;
mod ui;
mod update;

use std::process::ExitCode;

// The printing itself lives in the library, so other programs can use it too.
use std::sync::Arc;
use tapprint_core::{escpos, raster, usb};

fn main() -> ExitCode {
    // `tapprint print receipt.png` and the like. They talk to the copy that
    // is already open and never open a window.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Some(done) = cli::run(&arguments) {
        return done;
    }
    // `tapprint serve`: everything but the window and the tray icon.
    let headless = arguments.first().is_some_and(|first| first == "serve");
    // A window app says why it could not start in a dialog. With no window
    // the reason goes to the console that started it.
    let fatal = |error: &str| {
        if headless {
            cli::console::attach();
            cli::console::say(&format!("tapprint: {error}"), true);
        } else {
            notice::fatal(error);
        }
        ExitCode::from(1)
    };
    let shared = match model::Shared::load() {
        Ok(shared) => Arc::new(shared),
        Err(error) => return fatal(&error),
    };
    if headless {
        shared.set_headless();
    }
    // Bind first. Only the copy that owns the port writes the settings file.
    if let Err(error) = http::serve(Arc::clone(&shared)).and_then(|()| shared.persist()) {
        return fatal(&error);
    }
    autostart::refresh();
    update::tidy();
    update::watch(Arc::clone(&shared));
    station::watch(Arc::clone(&shared));
    share::apply(&shared);
    if headless {
        return serve(&shared);
    }
    ui::run(shared);
    ExitCode::SUCCESS
}

/// Run until stopped, with no window. The window's one background job, the
/// look for USB printers every second, is done here instead.
fn serve(shared: &model::Shared) -> ExitCode {
    let snapshot = shared.snapshot();
    cli::console::attach();
    // The pairing code itself is never printed: a log is not a safe place.
    cli::console::say(
        &format!(
            "TapPrint {} is running with no window on 127.0.0.1:{}.\nIts pairing code is in {}.\nStop it with Ctrl+C.",
            env!("CARGO_PKG_VERSION"),
            snapshot.port,
            snapshot.config_path
        ),
        false,
    );
    loop {
        print::refresh_printers(shared);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
