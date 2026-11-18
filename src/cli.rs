//! Printing from a command line, for desktop apps and scripts that can run a
//! program but not make an HTTP call.
//!
//!     tapprint print receipt.png --drawer
//!
//! The command does not touch the printer itself. It hands the file to the
//! TapPrint that is already running, so the job waits its turn with all the
//! others and shows in the window like any other.

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
TapPrint: print receipts with no print dialog.

Usage:
  tapprint                      Open TapPrint.
  tapprint print FILE [options] Print a PNG receipt. FILE can be - for standard input.
  tapprint status               Show whether TapPrint is running and ready.
  tapprint --version
  tapprint --help

Options for print:
  --paper 80|58   Paper width in mm. Default: the width set in the window.
  --no-cut        Do not cut after printing.
  --drawer        Open the cash drawer after printing.
  --dither        Dither, for photos and grey logos.
  --raw           FILE holds printer commands, not a PNG. Needs
                  \"Raw printer commands\" turned on in Settings.

TapPrint must already be open. Exit code 0 means printed, 1 means it failed,
2 means the command was not understood.";

/// Run a command, or `None` when there is none and the app should open.
/// Anything not recognized opens the app: macOS can pass arguments of its own.
pub fn run(arguments: &[String]) -> Option<ExitCode> {
    let command = arguments.first()?.as_str();
    let outcome = match command {
        "print" => print(&arguments[1..]),
        "status" => status(),
        "--help" | "-h" | "help" => Ok(HELP.to_string()),
        "--version" | "-V" => Ok(format!("tapprint {}", env!("CARGO_PKG_VERSION"))),
        _ => return None,
    };
    console::attach();
    Some(match outcome {
        Ok(message) => {
            console::say(&message, false);
            ExitCode::SUCCESS
        }
        Err(Failure::Usage(message)) => {
            console::say(&format!("{message}\n\n{HELP}"), true);
            ExitCode::from(2)
        }
        Err(Failure::Failed(message)) => {
            console::say(&message, true);
            ExitCode::FAILURE
        }
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The command was wrong.
    Usage(String),
    /// The command was right and the job did not work.
    Failed(String),
}

/// What `tapprint print` was asked to do.
#[derive(Debug, PartialEq, Eq)]
struct Job {
    file: String,
    raw: bool,
    headers: Vec<(&'static str, String)>,
}

fn parse(arguments: &[String]) -> Result<Job, Failure> {
    let usage = |message: &str| Failure::Usage(message.to_string());
    let mut file = None;
    let mut raw = false;
    let mut headers = Vec::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--paper" => {
                let width = rest.next().ok_or_else(|| usage("--paper needs 80 or 58"))?;
                if width != "80" && width != "58" {
                    return Err(usage("--paper must be 80 or 58"));
                }
                headers.push(("X-Paper-Mm", width.clone()));
            }
            "--no-cut" => headers.push(("X-Cut", "0".to_string())),
            "--drawer" => headers.push(("X-Drawer", "1".to_string())),
            "--dither" => headers.push(("X-Dither", "1".to_string())),
            "--raw" => raw = true,
            other if other.starts_with("--") => {
                return Err(Failure::Usage(format!("unknown option {other}")));
            }
            other if file.is_none() => file = Some(other.to_string()),
            _ => return Err(usage("print takes one file")),
        }
    }
    Ok(Job {
        file: file.ok_or_else(|| usage("print needs a file"))?,
        raw,
        headers,
    })
}

fn print(arguments: &[String]) -> Result<String, Failure> {
    let job = parse(arguments)?;
    let body = if job.file == "-" {
        let mut body = Vec::new();
        std::io::stdin()
            .read_to_end(&mut body)
            .map_err(|error| Failure::Failed(format!("could not read standard input: {error}")))?;
        body
    } else {
        std::fs::read(&job.file)
            .map_err(|error| Failure::Failed(format!("could not read {}: {error}", job.file)))?
    };
    let (port, code) = crate::model::local_access().map_err(Failure::Failed)?;
    let mut request = agent()
        .post(format!("http://127.0.0.1:{port}/v1/print"))
        .header("Authorization", format!("Bearer {code}"))
        .header("X-Till-Client", "Command line")
        .header(
            "Content-Type",
            if job.raw {
                "application/vnd.escpos"
            } else {
                "image/png"
            },
        );
    for (name, value) in &job.headers {
        request = request.header(*name, value);
    }
    let reply = request.send(&body[..]);
    answer(reply, "Printed.")
}

fn status() -> Result<String, Failure> {
    let (port, code) = crate::model::local_access().map_err(Failure::Failed)?;
    let reply = agent()
        .get(format!("http://127.0.0.1:{port}/v1/status"))
        .header("Authorization", format!("Bearer {code}"))
        .call();
    let mut reply = reply.map_err(|error| Failure::Failed(not_running(&error)))?;
    let json: serde_json::Value = serde_json::from_reader(reply.body_mut().as_reader())
        .map_err(|_| Failure::Failed("TapPrint sent an unreadable reply".to_string()))?;
    Ok(format!(
        "TapPrint is running on port {port}: {}. {}",
        json["state"].as_str().unwrap_or("unknown"),
        json["status"].as_str().unwrap_or_default()
    ))
}

/// Every reply is read, including errors: TapPrint's error text says what to do.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn not_running(error: &ureq::Error) -> String {
    format!("TapPrint is not running. Open it, then try again. ({error})")
}

fn answer(
    reply: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    done: &str,
) -> Result<String, Failure> {
    let mut reply = reply.map_err(|error| Failure::Failed(not_running(&error)))?;
    let ok = reply.status().is_success();
    let json: serde_json::Value =
        serde_json::from_reader(reply.body_mut().as_reader()).unwrap_or_default();
    if ok && json["ok"].as_bool() != Some(false) {
        return Ok(done.to_string());
    }
    Err(Failure::Failed(
        json["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("TapPrint answered {}", reply.status())),
    ))
}

/// Where messages go. A released Windows build is a window app with no
/// console of its own, so it borrows the one it was started from.
mod console {
    #[cfg(not(windows))]
    pub fn attach() {}

    #[cfg(not(windows))]
    pub fn say(message: &str, error: bool) {
        if error {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
    }

    #[cfg(windows)]
    pub fn attach() {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // Fails when started from something with no console. Then output
        // only reaches a pipe or file it was redirected to.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    #[cfg(windows)]
    pub fn say(message: &str, error: bool) {
        use std::io::Write;
        // Redirected output (`> file`, a pipe, another program) goes here.
        if error {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
        // The borrowed console, when there is one and nothing was redirected.
        if let Ok(mut console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            let _ = writeln!(console, "{message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn print_options_become_the_same_headers_a_web_page_sends() {
        let job = parse(&words("receipt.png --paper 58 --no-cut --drawer --dither")).unwrap();
        assert_eq!(job.file, "receipt.png");
        assert!(!job.raw);
        assert_eq!(
            job.headers,
            [
                ("X-Paper-Mm", "58".to_string()),
                ("X-Cut", "0".to_string()),
                ("X-Drawer", "1".to_string()),
                ("X-Dither", "1".to_string()),
            ]
        );
        // Options may come before the file, and - means standard input.
        let piped = parse(&words("--raw -")).unwrap();
        assert!(piped.raw);
        assert_eq!(piped.file, "-");
    }

    #[test]
    fn a_wrong_command_is_explained_not_guessed() {
        for wrong in [
            "",
            "--paper 72 a.png",
            "--paper",
            "a.png b.png",
            "a.png --colour",
        ] {
            assert!(
                matches!(parse(&words(wrong)), Err(Failure::Usage(_))),
                "{wrong:?}"
            );
        }
    }

    #[test]
    fn only_known_commands_keep_the_window_from_opening() {
        // macOS and desktop launchers can pass arguments of their own. They
        // must open the app as usual, not print an error and quit.
        assert!(run(&[]).is_none());
        assert!(run(&words("-psn_0_12345")).is_none());
        assert!(run(&words("receipt.png")).is_none());
        assert!(run(&words("--version")).is_some());
        assert!(run(&words("help")).is_some());
    }
}
