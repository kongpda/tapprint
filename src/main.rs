//! TapPrint: a menu-bar helper that prints a PNG to a USB or network
//! thermal printer, with no vendor driver installed.

// A release build on Windows is a tray app. Without this, a console window
// opens next to it and closing that window quits the app.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod autostart;
mod escpos;
mod http;
mod model;
mod notice;
mod print;
mod raster;
// Only macOS serves HTTPS. Other Unix systems build it for the tests alone,
// so the relay is checked wherever the tests run.
#[cfg(any(target_os = "macos", all(test, unix)))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod tls;
mod ui;
mod update;
mod usb;

use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let shared = match model::Shared::load() {
        Ok(shared) => Arc::new(shared),
        Err(error) => {
            notice::fatal(&error);
            return ExitCode::from(1);
        }
    };
    // Bind first. Only the copy that owns the port writes the settings file.
    if let Err(error) = http::serve(Arc::clone(&shared)).and_then(|()| shared.persist()) {
        notice::fatal(&error);
        return ExitCode::from(1);
    }
    autostart::refresh();
    update::tidy();
    update::watch(Arc::clone(&shared));
    ui::run(shared);
    ExitCode::SUCCESS
}
