//! The till window and the menu-bar icon.
//!
//! The window is the whole setup: pairing code, paper, USB printer, and the
//! network address. Nothing here asks the cashier to edit a JSON file.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, Entity, FontWeight, Global,
    KeyBinding, Menu, MenuItem, SharedString, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, actions, div, hsla, prelude::*, px, rgb, size,
};
use gpui_ce_elements::editable_text::{
    EditableTextState, StringStorage, TextChanged,
    actions::{DEFAULT_INPUT_CONTEXT, default_bindings},
    text_input,
};
use gpui_ce_platform::application;
use gpui_tray::{Icon, Tray};

use crate::model::{Link, Paper, ServerOutcome, Shared, Snapshot, Tone, Transport};
use crate::print;

actions!(
    tapprint,
    [
        OpenWindow,
        PrintTest,
        OpenDrawer,
        UseUsb,
        UseNetwork,
        SaveNetwork,
        ConnectStation,
        DenyPair,
        Quit
    ]
);

const PAPER: u32 = 0xf5f6f8;
const INK: u32 = 0x17212b;
const MUTED: u32 = 0x657180;
const CARD: u32 = 0xffffff;
const SOFT: u32 = 0xe5e9ee;
const READY_BG: u32 = 0xe8f4ef;
const READY_FG: u32 = 0x17674d;
const BUSY_BG: u32 = 0xfff4db;
const BUSY_FG: u32 = 0x775b18;
const BAD_BG: u32 = 0xfceeed;
const BAD_FG: u32 = 0x9d3833;

// Embedded vector icons keep the local bundle self-contained.
const EYE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></svg>"#;
const INFO: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7.5v.5"/></svg>"#;
const EYE_OFF: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m3 3 18 18M10.6 5.1A12 12 0 0 1 12 5c6.5 0 10 7 10 7a18 18 0 0 1-3 4M6 6.5A19 19 0 0 0 2 12s3.5 7 10 7a13 13 0 0 0 5.5-1.5M10 10a3 3 0 0 0 4 4"/></svg>"#;

struct AppGlobal {
    shared: Arc<Shared>,
    window: RefCell<Option<WindowHandle<TillView>>>,
    tray: Tray,
    menu_key: RefCell<String>,
    /// The site the window was last raised for, so it is raised once per request.
    asked: RefCell<Option<String>>,
}

impl Global for AppGlobal {}

pub fn run(shared: Arc<Shared>) {
    let app = application().with_quit_mode(gpui::QuitMode::Explicit);
    // macOS: a Dock click, or opening the app again from Finder, after the
    // window was closed. Without this the click does nothing.
    app.on_reopen(|cx| open_window(&OpenWindow, cx));
    app.run(move |cx| {
        if let Err(error) = start(cx, shared) {
            crate::notice::fatal(&format!("could not add the menu-bar icon: {error}"));
            // Without a tray or window the app cannot be seen or quit.
            std::process::exit(1);
        }
    });
}

fn start(cx: &mut App, shared: Arc<Shared>) -> gpui_tray::Result<()> {
    cx.bind_keys(default_bindings().as_keybindings(Some(DEFAULT_INPUT_CONTEXT)));
    cx.bind_keys([
        KeyBinding::new("enter", SaveNetwork, Some("TapPrint")),
        KeyBinding::new("enter", ConnectStation, Some("TapPrintStation")),
        // Esc anywhere in the window turns a waiting site down.
        KeyBinding::new("escape", DenyPair, None),
    ]);
    cx.on_action(open_window)
        .on_action(deny_pair)
        .on_action(print_test)
        .on_action(open_drawer)
        .on_action(use_usb)
        .on_action(use_network)
        .on_action(quit);

    let tray = Tray::builder()
        .icon(tray_icon(false)?)
        .tooltip("TapPrint")
        // Windows and Linux: a left click opens the window, a right click the
        // menu. On macOS the menu opens on any click.
        .on_activate(OpenWindow)
        .menu(build_menu)
        .build(cx)?;
    cx.set_global(AppGlobal {
        shared,
        window: RefCell::new(None),
        tray,
        menu_key: RefCell::new(String::new()),
        asked: RefCell::new(None),
    });
    // gpui-ce 0.3 on macOS still holds its platform lock while this start
    // callback runs, so opening a window here deadlocks. Open it on the next
    // run-loop turn instead.
    cx.spawn(async |cx| cx.update(|cx| open_window(&OpenWindow, cx)))
        .detach();
    watch_printers(cx);
    Ok(())
}

fn watch_printers(cx: &mut App) {
    let Some(shared) = cx
        .try_global::<AppGlobal>()
        .map(|global| global.shared.clone())
    else {
        return;
    };
    cx.spawn(async move |cx| {
        loop {
            let shared = shared.clone();
            let _ = cx
                .background_executor()
                .spawn(async move {
                    print::refresh_printers(&shared);
                })
                .await;
            cx.update(sync_window);
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }
    })
    .detach();
}

fn sync_window(cx: &mut App) {
    let Some((window, tray, shared, previous)) = cx.try_global::<AppGlobal>().map(|global| {
        (
            *global.window.borrow(),
            global.tray.clone(),
            global.shared.clone(),
            global.menu_key.borrow().clone(),
        )
    }) else {
        return;
    };
    let snapshot = shared.snapshot();
    // A site is waiting for the cashier. Bring the window up so they see it.
    let asking = snapshot
        .pair_request
        .as_ref()
        .map(|request| request.origin.clone());
    let raise = cx
        .try_global::<AppGlobal>()
        .is_some_and(|global| global.asked.replace(asking.clone()) != asking && asking.is_some());
    if raise {
        open_window(&OpenWindow, cx);
    }
    let window = window.or_else(|| {
        cx.try_global::<AppGlobal>()
            .and_then(|global| *global.window.borrow())
    });
    // The system may refuse to bring the window to the front while the
    // cashier works in the browser. Flash the taskbar or bounce the Dock too.
    if raise && let Some(handle) = window {
        let _ = handle.update(cx, |_view, window, _cx| window.request_attention());
    }
    let key = format!(
        "{:?}|{}|{:?}|{}|{:?}|{}|{:?}",
        snapshot.tone,
        snapshot.status,
        snapshot.transport,
        snapshot.paper.millimetres(),
        asking,
        snapshot
            .printers
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>()
            .join(","),
        snapshot.update.as_ref().map(|release| &release.version)
    );
    if let Some(handle) = window {
        let _ = handle.update(cx, |view, _window, cx| {
            if view.snapshot != snapshot {
                view.snapshot = snapshot.clone();
                cx.notify();
            }
        });
    }
    if key != previous {
        if let Some(global) = cx.try_global::<AppGlobal>() {
            *global.menu_key.borrow_mut() = key;
        }
        let asking = snapshot.pair_request.is_some();
        let _ = tray.set_title(Some(tray_title(snapshot.tone, asking)), cx);
        // Windows shows no title beside the icon, so the dot and the tooltip
        // carry the same signal there.
        let _ = tray.set_tooltip(Some(tray_tooltip(snapshot.tone, asking)), cx);
        if let Ok(icon) = tray_icon(asking || snapshot.tone == Tone::Problem) {
            let _ = tray.set_icon(icon, cx);
        }
        let _ = tray.refresh_menu(cx);
    }
}

fn build_menu(cx: &mut App) -> Vec<MenuItem> {
    let snapshot = cx
        .try_global::<AppGlobal>()
        .map(|global| global.shared.snapshot());
    let summary = snapshot
        .as_ref()
        .map(|snapshot| match snapshot.tone {
            Tone::Problem => snapshot.status.clone(),
            _ => {
                let transport = match snapshot.transport {
                    Transport::Usb => "USB",
                    Transport::Tcp => "Network",
                };
                format!(
                    "{} · {transport} · {} mm",
                    snapshot.status,
                    snapshot.paper.millimetres()
                )
            }
        })
        .unwrap_or_else(|| "Starting".to_string());
    let mut items = Vec::new();
    if snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.pair_request.is_some())
    {
        items.push(MenuItem::action(
            "A website wants to print. Answer…",
            OpenWindow,
        ));
    }
    // A till usually runs with the window closed, so the menu says it too.
    if let Some(release) = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.update.as_ref())
    {
        items.push(MenuItem::action(
            format!("Version {} is ready. Update…", release.version),
            OpenWindow,
        ));
    }
    items.extend([
        MenuItem::action(summary, gpui::NoAction).disabled(true),
        MenuItem::separator(),
        MenuItem::action("Open", OpenWindow),
        MenuItem::action("Print a test slip", PrintTest),
        MenuItem::action("Open cash drawer", OpenDrawer),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Printer").items([
            MenuItem::action("Use USB", UseUsb),
            MenuItem::action("Use network", UseNetwork),
        ])),
        MenuItem::separator(),
        MenuItem::action("Quit", Quit),
    ]);
    items
}

fn open_window(_: &OpenWindow, cx: &mut App) {
    let existing = cx
        .try_global::<AppGlobal>()
        .and_then(|global| *global.window.borrow());
    if let Some(handle) = existing
        && handle
            .update(cx, |_view, window, _cx| {
                window.activate();
            })
            .is_ok()
    {
        cx.activate(true);
        return;
    }
    let Some(shared) = cx
        .try_global::<AppGlobal>()
        .map(|global| global.shared.clone())
    else {
        return;
    };
    let bounds = Bounds::centered(None, size(px(440.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("TapPrint")),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        is_resizable: true,
        window_min_size: Some(size(px(380.), px(460.))),
        ..Default::default()
    };
    let opened = cx.open_window(options, |_window, cx| {
        cx.new(|cx| TillView::new(shared, cx))
    });
    match opened {
        Ok(handle) => {
            if let Some(global) = cx.try_global::<AppGlobal>() {
                *global.window.borrow_mut() = Some(handle);
            }
            cx.activate(true);
        }
        Err(error) => {
            eprintln!("tapprint: could not open the window: {error}");
        }
    }
}

fn print_test(_: &PrintTest, cx: &mut App) {
    run_job(cx, "Printing a test slip", print::print_test);
    sync_window(cx);
}

fn open_drawer(_: &OpenDrawer, cx: &mut App) {
    run_job(cx, "Opening the drawer", print::open_drawer);
    sync_window(cx);
}

/// Run a printer job off the main thread. The caller refreshes the window,
/// because a view cannot be updated from inside its own listener.
fn run_job(cx: &mut App, busy: &str, job: fn(&Shared) -> Result<(), String>) {
    let Some(shared) = cx
        .try_global::<AppGlobal>()
        .map(|global| global.shared.clone())
    else {
        return;
    };
    shared.set_status(Tone::Busy, busy);
    cx.spawn(async move |cx| {
        let _ = cx
            .background_executor()
            .spawn(async move { job(&shared) })
            .await;
        cx.update(sync_window);
    })
    .detach();
}

fn use_usb(_: &UseUsb, cx: &mut App) {
    let Some(shared) = cx
        .try_global::<AppGlobal>()
        .map(|global| global.shared.clone())
    else {
        return;
    };
    if shared.set_transport(Transport::Usb).is_ok() {
        shared.set_status(Tone::Ready, "Using USB");
    }
    sync_window(cx);
}

fn use_network(_: &UseNetwork, cx: &mut App) {
    let Some(shared) = cx
        .try_global::<AppGlobal>()
        .map(|global| global.shared.clone())
    else {
        return;
    };
    if let Err(error) = shared.use_saved_network() {
        shared.set_status(Tone::Problem, error);
    }
    sync_window(cx);
}

fn deny_pair(_: &DenyPair, cx: &mut App) {
    let Some(shared) = cx
        .try_global::<AppGlobal>()
        .map(|global| global.shared.clone())
    else {
        return;
    };
    if let Some(request) = shared.snapshot().pair_request {
        shared.answer_pair(request.serial, false);
        sync_window(cx);
    }
}

fn quit(_: &Quit, cx: &mut App) {
    let tray = cx
        .try_global::<AppGlobal>()
        .map(|global| global.tray.clone());
    if let Some(tray) = tray {
        let _ = tray.close(cx);
    }
    cx.quit();
}

fn tray_title(tone: Tone, asking: bool) -> &'static str {
    if asking {
        "Print ?"
    } else if tone == Tone::Problem {
        "Print !"
    } else {
        ""
    }
}

fn tray_tooltip(tone: Tone, asking: bool) -> &'static str {
    if asking {
        "TapPrint: a website wants to print"
    } else if tone == Tone::Problem {
        "TapPrint: needs attention"
    } else {
        "TapPrint"
    }
}

/// A receipt slip. `alert` adds a dot in the top-right corner.
fn tray_icon(alert: bool) -> gpui_tray::Result<Icon> {
    // 22 points at 2x. Black with alpha is a menu-bar template: macOS tints it
    // for the menu bar, and the gaps in the slip stay the bar's own color.
    const SIZE: u32 = 44;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let paper = (10..34).contains(&x) && (4..40).contains(&y);
            let line = paper
                && (14..30).contains(&x)
                && ((14..16).contains(&y) || (20..22).contains(&y) || (26..28).contains(&y));
            let (dx, dy) = (x.abs_diff(36), y.abs_diff(8));
            let dot = alert && dx * dx + dy * dy <= 49;
            // A clear ring keeps the dot apart from the slip.
            let ring = alert && !dot && dx * dx + dy * dy <= 100;
            if dot || (paper && !line && !ring) {
                let index = ((y * SIZE + x) * 4) as usize;
                if let Some(slot) = rgba.get_mut(index..index + 4) {
                    slot.copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
    }
    let builder = Icon::from_rgba(rgba, SIZE, SIZE);
    #[cfg(target_os = "macos")]
    let builder = builder.scale_factor(2.0).template(true);
    builder.build()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Printer,
    Websites,
    Settings,
}

/// The search for printers on the network.
#[derive(Clone, PartialEq, Eq)]
enum Search {
    Idle,
    Running,
    Done(Vec<String>),
}

// Help stays out of the way until its (i) is clicked. One note is open at a time.
const HELP_USB: &str = "Set the printer's USB port to Printer Class. If the system made a print queue for it, remove the queue and plug the printer in again.";
const HELP_NETWORK: &str = "Find printers looks on this computer's own network. If yours is not found, type its address. The port is almost always 9100.";
const HELP_DRAWER: &str = "Most drawers use pin 2. If Open drawer does nothing, try pin 5.";
const HELP_SITES: &str =
    "When your till page asks to print, it appears at the top of this window for you to allow.";
const HELP_RAW: &str = "Lets allowed websites send their own printer commands, which can change printer settings. Leave it off unless your till needs it.";
const HELP_HTTPS: &str = "Refuses websites that are not on https. Pages on this computer still work. Leave it off only if your till runs from an http address.";
const HELP_UPDATES: &str = "Asks GitHub once a day whether a newer TapPrint exists. Nothing is installed until you click Update.";
/// The print server's help, named for the server this build talks to.
fn help_server() -> String {
    if crate::station::is_khevents() {
        "Sellers on any phone or laptop can print on this printer. On khevents.app, an owner or admin opens Console, Printers, Add a printer, and types the code here. Keep TapPrint running.".to_string()
    } else {
        format!(
            "Anyone on your team can print on this printer from any phone or laptop. Add a printer on {}, and type its code here. Keep TapPrint running.",
            crate::station::host()
        )
    }
}

fn help_server_pause() -> String {
    format!(
        "While off, jobs wait on {} for up to 15 minutes. Senders see them waiting.",
        crate::station::host()
    )
}

/// "PRINT FROM KHEVENTS", for the card's heading.
static SERVER_HEADING: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("PRINT FROM {}", crate::station::SERVER_NAME.to_uppercase())
});

static SERVER_TOGGLE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("Take print jobs from {}", crate::station::SERVER_NAME));
const HELP_CODE: &str = "For curl, servers, and kiosks. It works from any program on this computer, so keep it private. Websites do not need it.";
const HELP_PAIR_NEW: &str = "It will be able to print and open the drawer. The address above is the one thing a page cannot fake. If unsure, choose Don't allow.";
const HELP_PAIR_KNOWN: &str = "This site can already print, so it should not need to ask. Allow only if you just cleared its browser or set the till up again. Its old code stops working.";
#[cfg(target_os = "macos")]
const HELP_SAFARI: &str = "Safari blocks plain connections from a secure page. This adds a certificate that only covers this computer. macOS asks for your approval.";

struct TillView {
    page: Page,
    show_code: bool,
    shared: Arc<Shared>,
    snapshot: Snapshot,
    host: Entity<EditableTextState>,
    port: Entity<EditableTextState>,
    /// The first click on New code only asks. The second one replaces it.
    confirm_rotate: bool,
    /// The site whose Remove was clicked once. A second click removes it.
    confirm_remove: Option<String>,
    copied: bool,
    /// Read once when the window opens, then kept in step with the toggle.
    autostart: bool,
    /// The help note that is open, by the name of its (i).
    help: Option<&'static str>,
    search: Search,
    /// An update is being downloaded and put in place.
    updating: bool,
    /// The code from the Printers page on the print server.
    station_code: Entity<EditableTextState>,
    /// A code is being sent to the print server.
    connecting: bool,
    /// The first click on Disconnect only asks.
    confirm_disconnect: bool,
    _subscriptions: Vec<Subscription>,
}

impl TillView {
    fn new(shared: Arc<Shared>, cx: &mut Context<Self>) -> Self {
        let snapshot = shared.snapshot();
        let host = cx
            .new(|cx| EditableTextState::new(StringStorage::from(snapshot.tcp_host.as_str()), cx));
        let port = cx.new(|cx| {
            EditableTextState::new(StringStorage::from(snapshot.tcp_port.to_string()), cx)
        });
        let station_code = cx.new(|cx| EditableTextState::new(StringStorage::from(""), cx));
        let subscriptions = vec![
            cx.subscribe(&host, |_, _, _: &TextChanged, cx| cx.notify()),
            cx.subscribe(&port, |_, _, _: &TextChanged, cx| cx.notify()),
            cx.subscribe(&station_code, |_, _, _: &TextChanged, cx| cx.notify()),
        ];
        Self {
            page: Page::Printer,
            show_code: false,
            shared,
            snapshot,
            host,
            port,
            confirm_rotate: false,
            confirm_remove: None,
            copied: false,
            autostart: crate::autostart::enabled(),
            help: None,
            search: Search::Idle,
            updating: false,
            station_code,
            connecting: false,
            confirm_disconnect: false,
            _subscriptions: subscriptions,
        }
    }

    /// Re-read shared state. Any click also drops the one-shot hints.
    fn pull(&mut self, cx: &mut Context<Self>) {
        self.confirm_rotate = false;
        self.confirm_remove = None;
        self.confirm_disconnect = false;
        self.copied = false;
        self.snapshot = self.shared.snapshot();
        cx.notify();
    }

    fn copy_code(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.snapshot.pairing_code.clone(),
        ));
        self.pull(cx);
        self.copied = true;
    }

    fn rotate(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_rotate {
            self.pull(cx);
            self.confirm_rotate = true;
            return;
        }
        if let Err(error) = self.shared.rotate_code() {
            self.shared.set_status(Tone::Problem, error);
        }
        self.pull(cx);
    }

    /// macOS shows its own approval prompt, so this waits off the UI thread.
    #[cfg(target_os = "macos")]
    fn set_up_safari(&mut self, cx: &mut Context<Self>) {
        self.shared
            .set_status(Tone::Busy, "Waiting for macOS to approve Safari support");
        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::tls::set_up(&crate::model::data_dir()) })
                .await;
            cx.update(|cx| {
                if let Some(global) = cx.try_global::<AppGlobal>() {
                    match result {
                        Ok(()) => global
                            .shared
                            .set_status(Tone::Ready, "Safari can now reach TapPrint"),
                        Err(error) => global.shared.set_status(Tone::Problem, error),
                    }
                }
                sync_window(cx);
            });
        })
        .detach();
        self.pull(cx);
    }

    fn set_autostart(&mut self, on: bool, cx: &mut Context<Self>) {
        match crate::autostart::set(on) {
            Ok(()) => {
                self.autostart = on;
                let message = if on {
                    "TapPrint will start when you log in"
                } else {
                    "TapPrint will not start at login"
                };
                self.shared.set_status(Tone::Ready, message);
            }
            Err(error) => self.shared.set_status(Tone::Problem, error),
        }
        self.pull(cx);
    }

    fn job(&mut self, busy: &str, job: fn(&Shared) -> Result<(), String>, cx: &mut Context<Self>) {
        run_job(cx, busy, job);
        self.pull(cx);
    }

    fn on_connect(&mut self, _: &ConnectStation, _window: &mut Window, cx: &mut Context<Self>) {
        self.connect_station(cx);
    }

    /// Send the code to the print server off the UI thread.
    fn connect_station(&mut self, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let code = self.station_code.read(cx).as_str().to_string();
        if crate::station::normalized_code(&code).is_none() {
            self.shared.set_status(
                Tone::Problem,
                "Type the 8-character code from the Printers page",
            );
            self.pull(cx);
            return;
        }
        self.connecting = true;
        self.shared.set_status(
            Tone::Busy,
            format!("Connecting to {}", crate::station::host()),
        );
        let shared = Arc::clone(&self.shared);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = crate::station::Client::new(&crate::station::server());
                    crate::station::pair(&shared, &client, &code)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.connecting = false;
                match result {
                    Ok(()) => view
                        .station_code
                        .update(cx, |state, cx| state.emplace("", cx)),
                    Err(error) => view.shared.set_status(Tone::Problem, error),
                }
                view.pull(cx);
            });
        })
        .detach();
        self.pull(cx);
    }

    fn disconnect_station(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_disconnect {
            self.pull(cx);
            self.confirm_disconnect = true;
            return;
        }
        let shared = Arc::clone(&self.shared);
        // Telling the server can take a moment; the key is forgotten at once.
        let _ = std::thread::Builder::new()
            .name("tapprint-disconnect".into())
            .spawn(move || {
                let client = crate::station::Client::new(&crate::station::server());
                crate::station::disconnect(&shared, &client);
            });
        self.pull(cx);
    }

    fn on_save(&mut self, _: &SaveNetwork, _window: &mut Window, cx: &mut Context<Self>) {
        self.save_network(cx);
    }

    fn save_network(&mut self, cx: &mut Context<Self>) {
        let host = self.host.read(cx).as_str().trim().to_string();
        let port_text = self.port.read(cx).as_str().trim().to_string();
        let Some(port) = port_text.parse::<u16>().ok().filter(|port| *port > 0) else {
            self.shared
                .set_status(Tone::Problem, "Port must be a number from 1 to 65535");
            self.pull(cx);
            return;
        };
        if let Err(error) = self.shared.set_network(&host, port) {
            self.shared.set_status(Tone::Problem, error);
        }
        self.pull(cx);
    }

    /// Switch to the network printer. With no address saved yet, start
    /// looking for one straight away, so the next click can be the last.
    fn choose_network(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.shared.use_saved_network() {
            self.shared.set_status(Tone::Problem, error);
        }
        self.pull(cx);
        if self.snapshot.tcp_host.is_empty() && self.search == Search::Idle {
            self.find_printers(cx);
        }
    }

    fn find_printers(&mut self, cx: &mut Context<Self>) {
        if self.search == Search::Running {
            return;
        }
        self.search = Search::Running;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async { print::find_network_printers() })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.search = Search::Done(found);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// One click on a found printer fills the fields and saves it.
    fn use_found(&mut self, host: &str, cx: &mut Context<Self>) {
        self.host.update(cx, |state, cx| state.emplace(host, cx));
        self.port.update(cx, |state, cx| {
            state.emplace(&print::RAW_PORT.to_string(), cx);
        });
        self.save_network(cx);
    }

    /// Download the update, check it, swap it in, start it, and quit. The
    /// running copy keeps printing until the new one is in place.
    fn install_update(&mut self, release: crate::update::Release, cx: &mut Context<Self>) {
        if self.updating {
            return;
        }
        if self.shared.printer_busy() {
            self.shared
                .set_status(Tone::Problem, "Wait for the receipt to finish, then update");
            self.pull(cx);
            return;
        }
        self.updating = true;
        self.shared
            .set_status(Tone::Busy, format!("Getting version {}", release.version));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let program = crate::update::install(&release, &crate::model::data_dir())?;
                    crate::update::start(&program)
                })
                .await;
            match result {
                Ok(()) => {
                    cx.update(|cx| quit(&Quit, cx));
                }
                Err(error) => {
                    let _ = this.update(cx, |view, cx| {
                        view.updating = false;
                        view.shared.set_status(Tone::Problem, error);
                        view.pull(cx);
                    });
                }
            }
        })
        .detach();
        self.pull(cx);
    }

    /// Shown while a newer release is waiting. One click installs it, or
    /// opens the download page when this copy cannot check a signature.
    fn update_card(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> Option<gpui::Div> {
        let release = snapshot.update.clone()?;
        let can_install = crate::update::can_install();
        let line = row().child(
            div()
                .text_sm()
                .child(format!("Version {} is ready", release.version)),
        );
        let line = if self.updating {
            line.child(pill("Updating…".to_string(), true))
        } else {
            line.child(button(
                "update-now",
                if can_install { "Update" } else { "Download" },
                Kind::Primary,
                cx.listener(move |view, _: &ClickEvent, _, cx| {
                    if can_install {
                        view.install_update(release.clone(), cx);
                    } else {
                        cx.open_url(&release.page);
                    }
                }),
            ))
        };
        Some(
            div().px_4().pb_3().child(
                card()
                    .border_color(rgb(READY_FG))
                    .bg(rgb(READY_BG))
                    .child(line),
            ),
        )
    }

    /// The (i) beside a setting. It opens the note with the same name.
    fn info(&self, name: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let open = self.help == Some(name);
        icon_button(
            SharedString::from(format!("info-{name}")),
            INFO,
            if open { "Hide help" } else { "Show help" },
            open,
            cx.listener(move |view, _: &ClickEvent, _, cx| {
                view.help = if view.help == Some(name) {
                    None
                } else {
                    Some(name)
                };
                cx.notify();
            }),
        )
    }

    /// The note for `name`, when its (i) was clicked.
    fn note(&self, name: &'static str, text: impl Into<SharedString>) -> Option<gpui::Div> {
        (self.help == Some(name)).then(|| {
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgb(PAPER))
                .text_xs()
                .text_color(rgb(MUTED))
                .child(text.into())
        })
    }

    /// A setting's name with its (i).
    fn named(&self, title: &'static str, help: &'static str, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(div().text_sm().child(title))
            .child(self.info(help, cx))
    }
}

impl Render for TillView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot.clone();
        let content = match self.page {
            Page::Printer => self.printer_page(cx, &snapshot),
            Page::Websites => self.websites_page(cx, &snapshot),
            Page::Settings => self.settings_page(cx, &snapshot),
        };
        let mut tabs = segments().mx_4().mb_3();
        for (id, title, page) in [
            ("tab-printer", "Printer", Page::Printer),
            ("tab-websites", "Websites", Page::Websites),
            ("tab-settings", "Settings", Page::Settings),
        ] {
            tabs = tabs.child(
                segment(
                    id,
                    title,
                    self.page == page,
                    cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.page = page;
                        view.show_code = false;
                        view.help = None;
                        // A half-made choice must not still be armed on return.
                        view.confirm_rotate = false;
                        view.confirm_remove = None;
                        view.confirm_disconnect = false;
                        view.copied = false;
                        cx.notify();
                    }),
                )
                .flex_1(),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PAPER))
            .text_color(rgb(INK))
            .child(header(&snapshot))
            .children(self.pair_card(cx, &snapshot))
            .children(self.update_card(cx, &snapshot))
            .child(tabs)
            .child(
                div()
                    .id("page")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_4()
                    .child(content),
            )
    }
}

impl TillView {
    /// Shown above everything while a site waits, so it cannot be missed.
    fn pair_card(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> Option<gpui::Div> {
        let request = snapshot.pair_request.clone()?;
        let serial = request.serial;
        let (advice, help) = if request.known {
            ("Already allowed. Allow again?", HELP_PAIR_KNOWN)
        } else {
            ("Allow only if this is your till site.", HELP_PAIR_NEW)
        };
        Some(
            div().px_4().pb_3().child(
                card()
                    .border_color(rgb(BUSY_FG))
                    .bg(rgb(BUSY_BG))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label("WANTS TO PRINT"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(BUSY_FG))
                                    .child(format!("{} s", request.seconds_left)),
                            ),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(request.origin),
                    )
                    .when(request.insecure, |card| {
                        card.child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(BAD_BG))
                                .text_color(rgb(BAD_FG))
                                .text_xs()
                                .child("Not a secure (https) site. Someone on its network could alter the page."),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_sm().child(advice))
                            .child(self.info("pair", cx)),
                    )
                    .children(self.note(
                        "pair",
                        format!(
                            "{help} The page calls itself \u{201c}{}\u{201d}. Esc means Don't allow.",
                            request.client
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            // Same weight for both, so neither is the obvious click.
                            .child(
                                button(
                                    "pair-allow",
                                    "Allow",
                                    Kind::Plain,
                                    cx.listener(move |view, _: &ClickEvent, _, cx| {
                                        view.shared.answer_pair(serial, true);
                                        view.pull(cx);
                                    }),
                                )
                                .flex_1(),
                            )
                            .child(
                                button(
                                    "pair-deny",
                                    "Don't allow",
                                    Kind::Plain,
                                    cx.listener(move |view, _: &ClickEvent, _, cx| {
                                        view.shared.answer_pair(serial, false);
                                        view.pull(cx);
                                    }),
                                )
                                .flex_1(),
                            ),
                    ),
            ),
        )
    }

    fn printer_page(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> gpui::Div {
        let usb = snapshot.transport == Transport::Usb;
        let connection = card()
            .child(
                row().child(div().text_sm().child("Connection")).child(
                    segments()
                        .child(segment(
                            "transport-usb",
                            "USB",
                            usb,
                            cx.listener(|view, _: &ClickEvent, _, cx| {
                                if view.shared.set_transport(Transport::Usb).is_ok() {
                                    view.shared.set_status(Tone::Ready, "Using USB");
                                }
                                view.pull(cx);
                            }),
                        ))
                        .child(segment(
                            "transport-network",
                            "Network",
                            !usb,
                            cx.listener(|view, _: &ClickEvent, _, cx| view.choose_network(cx)),
                        )),
                ),
            )
            .child(divider())
            .child(if usb {
                self.usb_list(cx, snapshot)
            } else {
                self.network_form(cx, snapshot)
            });

        let mut paper = segments();
        for (id, title, size) in [
            ("paper-80", "80 mm", Paper::Mm80),
            ("paper-58", "58 mm", Paper::Mm58),
        ] {
            paper = paper.child(segment(
                id,
                title,
                snapshot.paper == size,
                cx.listener(move |view, _: &ClickEvent, _, cx| {
                    view.shared.set_paper(size);
                    view.pull(cx);
                }),
            ));
        }
        let mut pins = segments();
        for (id, title, pin) in [
            ("drawer-pin-2", "Pin 2", 2u8),
            ("drawer-pin-5", "Pin 5", 5u8),
        ] {
            pins = pins.child(segment(
                id,
                title,
                snapshot.drawer_pin == pin,
                cx.listener(move |view, _: &ClickEvent, _, cx| {
                    view.shared.set_drawer_pin(pin);
                    view.pull(cx);
                }),
            ));
        }
        let options = card()
            .child(row().child(div().text_sm().child("Paper")).child(paper))
            .child(divider())
            .child(
                row()
                    .child(self.named("Cash drawer", "drawer", cx))
                    .child(pins),
            )
            .children(self.note("drawer", HELP_DRAWER));

        let actions = div()
            .flex()
            .gap_2()
            .child(
                button(
                    "test-slip",
                    "Print test",
                    Kind::Primary,
                    cx.listener(|view, _: &ClickEvent, _, cx| {
                        view.job("Printing a test slip", print::print_test, cx)
                    }),
                )
                .flex_1(),
            )
            .child(
                button(
                    "open-drawer",
                    "Open drawer",
                    Kind::Plain,
                    cx.listener(|view, _: &ClickEvent, _, cx| {
                        view.job("Opening the drawer", print::open_drawer, cx)
                    }),
                )
                .flex_1(),
            );

        let last = snapshot
            .last_job
            .clone()
            .unwrap_or_else(|| "Nothing printed yet".to_string());
        page()
            .child(connection)
            .child(options)
            .child(actions)
            .child(hint(last).px_1())
    }

    fn usb_list(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> gpui::Div {
        let mut list = div().flex().flex_col().gap_1();
        if snapshot.printers.is_empty() {
            let message = if snapshot.listed {
                "Plug in your printer"
            } else {
                "Looking for printers…"
            };
            return list
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().text_sm().text_color(rgb(MUTED)).child(message))
                        .child(self.info("usb", cx)),
                )
                .children(self.note("usb", HELP_USB));
        }
        for (id, name) in &snapshot.printers {
            let selected = snapshot.usb_id.as_ref() == Some(id);
            let id = id.clone();
            list = list.child(option_row(
                SharedString::from(format!("usb-{id}")),
                name.clone(),
                selected,
                cx.listener(move |view, _: &ClickEvent, _, cx| {
                    view.shared.select_usb(id.clone());
                    view.pull(cx);
                }),
            ));
        }
        list
    }

    fn network_form(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> gpui::Div {
        let typed = self.host.read(cx).as_str().trim().to_string();
        let port = self.port.read(cx).as_str().trim().to_string();
        let saved = !snapshot.tcp_host.is_empty()
            && typed == snapshot.tcp_host
            && port == snapshot.tcp_port.to_string();
        let mut form = div()
            .key_context("TapPrint")
            .on_action(cx.listener(Self::on_save))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        field(
                            text_input("tcp-host").state(self.host.downgrade()),
                            "Printer address",
                        )
                        .flex_1(),
                    )
                    .child(
                        field(text_input("tcp-port").state(self.port.downgrade()), "9100")
                            .w(px(68.)),
                    )
                    .child(button(
                        "use-network",
                        if saved { "Saved" } else { "Save" },
                        if saved { Kind::Plain } else { Kind::Primary },
                        cx.listener(|view, _: &ClickEvent, _, cx| view.save_network(cx)),
                    )),
            );
        let searching = self.search == Search::Running;
        form =
            form.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_sm().text_color(rgb(MUTED)).child(
                                match &self.search {
                                    Search::Idle => "Don't know the address?".to_string(),
                                    Search::Running => "Looking on your network…".to_string(),
                                    Search::Done(found) if found.is_empty() => {
                                        "No printer found".to_string()
                                    }
                                    Search::Done(found) if found.len() == 1 => {
                                        "Found 1 printer".to_string()
                                    }
                                    Search::Done(found) => {
                                        format!("Found {} printers", found.len())
                                    }
                                },
                            ))
                            .child(self.info("network", cx)),
                    )
                    .when(!searching, |line| {
                        line.child(button(
                            "find-printers",
                            if self.search == Search::Idle {
                                "Find printers"
                            } else {
                                "Look again"
                            },
                            Kind::Plain,
                            cx.listener(|view, _: &ClickEvent, _, cx| view.find_printers(cx)),
                        ))
                    }),
            );
        if let Search::Done(found) = &self.search {
            for host in found {
                let selected = *host == snapshot.tcp_host;
                let pick = host.clone();
                form = form.child(option_row(
                    SharedString::from(format!("found-{host}")),
                    host.clone(),
                    selected,
                    cx.listener(move |view, _: &ClickEvent, _, cx| view.use_found(&pick, cx)),
                ));
            }
        }
        form.children(self.note("network", HELP_NETWORK))
    }

    fn websites_page(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> gpui::Div {
        let mut sites = card().child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(label("ALLOWED WEBSITES"))
                .child(self.info("sites", cx)),
        );
        sites = sites.children(self.note("sites", HELP_SITES));
        if snapshot.sites.is_empty() {
            sites = sites.child(div().text_sm().text_color(rgb(MUTED)).child("None yet"));
        }
        for origin in &snapshot.sites {
            let remove = origin.clone();
            let asked = self.confirm_remove.as_ref() == Some(origin);
            sites = sites.child(
                row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(origin.clone()),
                    )
                    .child(button(
                        SharedString::from(format!("remove-{origin}")),
                        if asked { "Really remove?" } else { "Remove" },
                        if asked { Kind::Danger } else { Kind::Plain },
                        cx.listener(move |view, _: &ClickEvent, _, cx| {
                            let confirmed = view.confirm_remove.as_ref() == Some(&remove);
                            if confirmed {
                                view.shared.remove_site(&remove);
                            }
                            view.pull(cx);
                            if !confirmed {
                                view.confirm_remove = Some(remove.clone());
                            }
                        }),
                    )),
            );
        }
        let mut body = page().child(self.server_card(cx, snapshot)).child(sites);
        if !snapshot.clients.is_empty() {
            let mut open = card().child(label("TILL PAGES"));
            for client in &snapshot.clients {
                let state = if client.live {
                    "connected".to_string()
                } else {
                    crate::model::ago_words(Duration::from_secs(client.seconds_ago))
                };
                open = open.child(
                    row()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .truncate()
                                .child(client.label.clone()),
                        )
                        .child(pill(state, client.live)),
                );
            }
            body = body.child(open);
        }
        body
    }

    /// Print from the server: connect with a code, then the link and recent jobs.
    fn server_card(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> gpui::Div {
        let server = &snapshot.server;
        let mut body = card().child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(label(SERVER_HEADING.as_str()))
                .child(self.info("server", cx)),
        );
        body = body.children(self.note("server", help_server()));
        if !server.connected {
            let typed = self.station_code.read(cx).as_str().to_string();
            let ready = crate::station::normalized_code(&typed).is_some();
            return body
                .child(hint(format!(
                    "Sellers on any device can print here. Type the code from the Printers page on {}.",
                    crate::station::host()
                )))
                .child(
                    div()
                        .key_context("TapPrintStation")
                        .on_action(cx.listener(Self::on_connect))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            field(
                                text_input("station-code").state(self.station_code.downgrade()),
                                "XXXX-XXXX",
                            )
                            .flex_1(),
                        )
                        .child(button(
                            "station-connect",
                            if self.connecting {
                                "Connecting…"
                            } else {
                                "Connect"
                            },
                            if ready && !self.connecting {
                                Kind::Primary
                            } else {
                                Kind::Plain
                            },
                            cx.listener(|view, _: &ClickEvent, _, cx| view.connect_station(cx)),
                        )),
                );
        }
        let (state, good) = match (server.link, server.paused, server.paused_there) {
            (_, true, _) => ("Paused here".to_string(), false),
            (Link::Online, _, true) => ("Paused".to_string(), false),
            (Link::Online, _, _) => ("Online".to_string(), true),
            (Link::Starting, _, _) => ("Connecting".to_string(), false),
            (Link::Disabled, _, _) => ("Off".to_string(), false),
            (Link::Offline | Link::Off, _, _) => ("Offline".to_string(), false),
        };
        body = body.child(
            row()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().truncate().child(server.station.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .truncate()
                                .child(server.organizer.clone()),
                        ),
                )
                .child(pill(state, good)),
        );
        if !server.note.is_empty() && !server.paused {
            body = body.child(hint(server.note.clone()));
        }
        body = body
            .child(divider())
            .child(
                row()
                    .child(self.named("Take print jobs", "server-pause", cx))
                    .child(toggle(
                        "server-toggle",
                        SERVER_TOGGLE.as_str(),
                        !server.paused,
                        cx.listener(|view, _: &ClickEvent, _, cx| {
                            let paused = !view.snapshot.server.paused;
                            view.shared.set_server_paused(paused);
                            view.pull(cx);
                        }),
                    )),
            )
            .children(self.note("server-pause", help_server_pause()));
        if !server.recent.is_empty() {
            body = body.child(divider()).child(label("RECENT JOBS"));
            for (index, job) in server.recent.iter().enumerate() {
                let from = if job.sender.is_empty() {
                    job.ago.clone()
                } else {
                    format!("{} · {}", job.sender, job.ago)
                };
                body = body.child(
                    row()
                        .id(SharedString::from(format!("server-job-{index}")))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().truncate().child(job.summary.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .truncate()
                                        .child(from),
                                ),
                        )
                        .child(pill(
                            job.outcome.words().to_string(),
                            job.outcome == ServerOutcome::Printed,
                        )),
                );
            }
        }
        let asked = self.confirm_disconnect;
        body.child(divider()).child(
            row()
                .child(hint(if asked {
                    "Sellers can no longer print here until you connect again.".to_string()
                } else {
                    format!(
                        "{} printed since TapPrint started. The full log is on the Printers page.",
                        server.jobs
                    )
                }))
                .child(button(
                    "station-disconnect",
                    if asked {
                        "Really disconnect?"
                    } else {
                        "Disconnect"
                    },
                    if asked { Kind::Danger } else { Kind::Plain },
                    cx.listener(|view, _: &ClickEvent, _, cx| view.disconnect_station(cx)),
                )),
        )
    }

    fn settings_page(&self, cx: &mut Context<Self>, snapshot: &Snapshot) -> gpui::Div {
        let mut switches = card().child(
            row()
                .child(div().text_sm().child("Start at login"))
                .child(toggle(
                    "autostart-toggle",
                    "Start TapPrint at login",
                    self.autostart,
                    cx.listener(|view, _: &ClickEvent, _, cx| {
                        view.set_autostart(!view.autostart, cx)
                    }),
                )),
        );
        #[cfg(target_os = "macos")]
        if crate::tls::serving() {
            let line = row().child(self.named("Safari", "safari", cx));
            let line = if crate::tls::is_trusted() {
                line.child(pill("On".to_string(), true))
            } else {
                line.child(button(
                    "safari-setup",
                    "Set up",
                    Kind::Primary,
                    cx.listener(|view, _: &ClickEvent, _, cx| view.set_up_safari(cx)),
                ))
            };
            switches = switches
                .child(divider())
                .child(line)
                .children(self.note("safari", HELP_SAFARI));
        }
        switches = switches
            .child(divider())
            .child(
                row()
                    .child(self.named("Check for updates", "updates", cx))
                    .child(toggle(
                        "updates-toggle",
                        "Check for updates",
                        snapshot.check_updates,
                        cx.listener(|view, _: &ClickEvent, _, cx| {
                            let on = !view.snapshot.check_updates;
                            view.shared.set_check_updates(on);
                            if on {
                                // Look now, not at the next daily check.
                                let shared = Arc::clone(&view.shared);
                                let _ = std::thread::Builder::new()
                                    .name("tapprint-update-now".into())
                                    .spawn(move || crate::update::check(&shared));
                            }
                            view.pull(cx);
                        }),
                    )),
            )
            .children(self.note("updates", HELP_UPDATES))
            .child(divider())
            .child(
                row()
                    .child(self.named("Only secure websites", "https", cx))
                    .child(toggle(
                        "https-toggle",
                        "Only allow secure (https) websites",
                        snapshot.https_only,
                        cx.listener(|view, _: &ClickEvent, _, cx| {
                            view.shared.set_https_only(!view.snapshot.https_only);
                            view.pull(cx);
                        }),
                    )),
            )
            .children(self.note("https", HELP_HTTPS))
            .child(divider())
            .child(
                row()
                    .child(self.named("Raw printer commands", "raw", cx))
                    .child(toggle(
                        "raw-toggle",
                        "Allow raw printer commands",
                        snapshot.allow_raw,
                        cx.listener(|view, _: &ClickEvent, _, cx| {
                            view.shared.set_allow_raw(!view.snapshot.allow_raw);
                            view.pull(cx);
                        }),
                    )),
            )
            .children(self.note("raw", HELP_RAW));

        let code = card()
            .child(
                row().child(self.named("Pairing code", "code", cx)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icon_button(
                            "reveal-code",
                            if self.show_code { EYE_OFF } else { EYE },
                            if self.show_code {
                                "Hide pairing code"
                            } else {
                                "Show pairing code"
                            },
                            false,
                            cx.listener(|view, _: &ClickEvent, _, cx| {
                                view.show_code = !view.show_code;
                                cx.notify();
                            }),
                        ))
                        .child(button(
                            "copy-code",
                            if self.copied { "Copied" } else { "Copy" },
                            Kind::Plain,
                            cx.listener(|view, _: &ClickEvent, _, cx| view.copy_code(cx)),
                        ))
                        .child(button(
                            "new-code",
                            if self.confirm_rotate {
                                "Really replace?"
                            } else {
                                "New"
                            },
                            if self.confirm_rotate {
                                Kind::Danger
                            } else {
                                Kind::Plain
                            },
                            cx.listener(|view, _: &ClickEvent, _, cx| view.rotate(cx)),
                        )),
                ),
            )
            .children(self.note("code", HELP_CODE))
            .when(self.show_code, |card| {
                card.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(PAPER))
                        .text_sm()
                        .child(grouped_code(&snapshot.pairing_code)),
                )
            })
            .when(self.confirm_rotate, |card| {
                card.child(hint(
                    "Anything using the old code stops printing. Allowed websites keep working.",
                ))
            });

        let about = div()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .child(hint(format!(
                "TapPrint {} · port {}",
                env!("CARGO_PKG_VERSION"),
                snapshot.port
            )))
            .child(self.info("about", cx));
        page()
            .child(switches)
            .child(code)
            .child(about)
            .children(self.note(
                "about",
                format!("Settings are kept in {}", snapshot.config_path),
            ))
    }
}

/// The app name and one word for how things are. The sentence under it is
/// the latest status, or the problem to fix.
fn header(snapshot: &Snapshot) -> gpui::Div {
    let (colour, word) = match snapshot.tone {
        Tone::Ready => (READY_FG, "Ready"),
        Tone::Busy => (BUSY_FG, "Working"),
        Tone::Problem => (BAD_FG, "Check"),
    };
    let problem = snapshot.tone == Tone::Problem;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_4()
        .pt_3()
        .pb_3()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("TapPrint"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(rgb(colour)))
                        .child(div().text_xs().text_color(rgb(colour)).child(word)),
                ),
        )
        .child(
            div()
                .text_xs()
                .when(problem, |line| {
                    line.px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(BAD_BG))
                        .text_color(rgb(BAD_FG))
                })
                .when(!problem, |line| line.text_color(rgb(MUTED)))
                .child(snapshot.status.clone()),
        )
}

fn page() -> gpui::Div {
    div().flex().flex_col().gap_3()
}

fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(rgb(SOFT))
        .bg(rgb(CARD))
}

/// One setting: its name on the left, its control on the right.
fn row() -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .min_h(px(36.))
}

fn divider() -> gpui::Div {
    div().h(px(1.)).bg(rgb(SOFT))
}

fn label(text: &'static str) -> gpui::Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED))
        .child(text)
}

/// Small grey text.
fn hint(text: impl Into<SharedString>) -> gpui::Div {
    div().text_xs().text_color(rgb(MUTED)).child(text.into())
}

/// A small rounded tag, green when it reports something good.
fn pill(text: String, good: bool) -> gpui::Div {
    let (background, foreground) = if good {
        (READY_BG, READY_FG)
    } else {
        (PAPER, MUTED)
    };
    div()
        .px_2()
        .py(px(2.))
        .rounded_full()
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_xs()
        .child(text)
}

fn grouped_code(code: &str) -> String {
    let mut grouped = String::with_capacity(code.len().saturating_add(code.len() / 4));
    for (index, ch) in code.chars().enumerate() {
        if index > 0 && index % 4 == 0 {
            grouped.push(' ');
        }
        grouped.push(ch);
    }
    grouped
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The one thing to do next.
    Primary,
    Plain,
    /// The second click of something that cannot be undone.
    Danger,
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    kind: Kind,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let label: SharedString = label.into();
    let (background, foreground, edge, hover) = match kind {
        Kind::Primary => (INK, CARD, INK, MUTED),
        Kind::Plain => (CARD, INK, SOFT, PAPER),
        Kind::Danger => (BAD_BG, BAD_FG, BAD_FG, BAD_BG),
    };
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .focusable()
        .tab_index(0)
        .on_click(on_click)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(edge))
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_sm()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(hover)))
        .active(|style| style.opacity(0.7))
        .focus_visible(|style| style.border_color(rgb(READY_FG)))
        .child(label)
}

/// A square button that shows only an icon. `label` is what a screen reader says.
fn icon_button(
    id: impl Into<gpui::ElementId>,
    icon: &'static [u8],
    label: &'static str,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .focusable()
        .tab_index(0)
        .on_click(on_click)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .w(px(28.))
        .h(px(28.))
        .rounded_full()
        .border_1()
        .border_color(hsla(0., 0., 0., 0.))
        .cursor_pointer()
        .when(on, |style| style.bg(rgb(SOFT)))
        .hover(|style| style.bg(rgb(SOFT)))
        .focus_visible(|style| style.border_color(rgb(READY_FG)))
        .child(
            gpui::svg()
                .data(icon)
                .w(px(16.))
                .h(px(16.))
                .text_color(rgb(if on { INK } else { MUTED })),
        )
}

/// The track that holds a set of segments: a choice of one among a few.
fn segments() -> gpui::Div {
    div()
        .flex()
        .flex_shrink_0()
        .p(px(3.))
        .gap(px(2.))
        .rounded_lg()
        .bg(rgb(SOFT))
}

fn segment(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .focusable()
        .tab_index(0)
        .on_click(on_click)
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.))
        .px_3()
        .rounded_md()
        .border_1()
        .border_color(hsla(0., 0., 0., 0.))
        .text_sm()
        .cursor_pointer()
        .when(selected, |style| {
            style.bg(rgb(CARD)).font_weight(FontWeight::SEMIBOLD)
        })
        .when(!selected, |style| {
            style
                .text_color(rgb(MUTED))
                .hover(|style| style.text_color(rgb(INK)))
        })
        .focus_visible(|style| style.border_color(rgb(READY_FG)))
        .child(label)
}

/// One of a list to pick from: a printer on USB, or one found on the network.
fn option_row(
    id: impl Into<gpui::ElementId>,
    name: String,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(name.clone())
        .focusable()
        .tab_index(0)
        .on_click(on_click)
        .flex()
        .items_center()
        .gap_2()
        .h(px(36.))
        .px_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(if selected { READY_FG } else { SOFT }))
        .when(selected, |style| style.bg(rgb(READY_BG)))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(if selected { READY_BG } else { PAPER })))
        .focus_visible(|style| style.border_color(rgb(READY_FG)))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.))
                .h(px(16.))
                .rounded_full()
                .border_1()
                .border_color(rgb(if selected { READY_FG } else { MUTED }))
                .when(selected, |dot| {
                    dot.child(div().w(px(8.)).h(px(8.)).rounded_full().bg(rgb(READY_FG)))
                }),
        )
        .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
        .when(selected, |style| {
            style.child(div().text_xs().text_color(rgb(READY_FG)).child("In use"))
        })
}

/// The shared look of a text field.
fn field(
    input: gpui_ce_elements::editable_text::EditableTextElement,
    placeholder: &'static str,
) -> gpui_ce_elements::editable_text::EditableTextElement {
    input
        .placeholder(placeholder)
        .placeholder_color(rgb(MUTED))
        .caret_color(gpui::black())
        .selection_color(hsla(0.58, 0.28, 0.72, 0.7))
        .caret_blink_interval_500ms()
        .h(px(32.))
        .px_2()
        .py(px(5.))
        .bg(rgb(PAPER))
        .border_1()
        .border_color(rgb(SOFT))
        .rounded_md()
        .text_sm()
}

fn toggle(
    id: &'static str,
    label: &'static str,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::Switch)
        .aria_label(label)
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .focusable()
        .tab_index(0)
        .on_click(on_click)
        .flex()
        .flex_shrink_0()
        .items_center()
        .cursor_pointer()
        .rounded_full()
        .border_1()
        .border_color(hsla(0., 0., 0., 0.))
        .focus_visible(|style| style.border_color(rgb(READY_FG)))
        .child(
            div()
                .w(px(40.))
                .h(px(24.))
                .p(px(3.))
                .rounded_full()
                .flex()
                .items_center()
                .bg(rgb(if on { READY_FG } else { MUTED }))
                .when(on, |track| track.justify_end())
                .child(div().w(px(18.)).h(px(18.)).rounded_full().bg(rgb(CARD))),
        )
}
