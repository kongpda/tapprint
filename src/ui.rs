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

use crate::model::{Paper, Shared, Tone, Transport};
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
        "{:?}|{}|{:?}|{}|{:?}|{}",
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
            .join(",")
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
    let bounds = Bounds::centered(None, size(px(480.), px(660.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("TapPrint")),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        is_resizable: true,
        window_min_size: Some(size(px(420.), px(520.))),
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

struct TillView {
    page: Page,
    show_code: bool,
    shared: Arc<Shared>,
    snapshot: crate::model::Snapshot,
    host: Entity<EditableTextState>,
    port: Entity<EditableTextState>,
    /// The first click on New code only asks. The second one replaces it.
    confirm_rotate: bool,
    /// The site whose Remove was clicked once. A second click removes it.
    confirm_remove: Option<String>,
    copied: bool,
    /// Read once when the window opens, then kept in step with the toggle.
    autostart: bool,
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
        let subscriptions = vec![
            cx.subscribe(&host, |_, _, _: &TextChanged, cx| cx.notify()),
            cx.subscribe(&port, |_, _, _: &TextChanged, cx| cx.notify()),
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
            _subscriptions: subscriptions,
        }
    }

    /// Re-read shared state. Any click also drops the one-shot hints.
    fn pull(&mut self, cx: &mut Context<Self>) {
        self.confirm_rotate = false;
        self.confirm_remove = None;
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

    fn safari_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let trusted = crate::tls::is_trusted();
        let card = card().child(label("SAFARI")).child(div().text_sm().child(if trusted {
            "Safari support is on"
        } else {
            "Safari support is not set up"
        }));
        if trusted {
            return card.child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("Till pages in Safari can print. Chrome and Edge need nothing extra."),
            );
        }
        card.child(
            div().text_xs().text_color(rgb(MUTED)).child(
                "Safari blocks plain connections from a secure till page. Allow TapPrint's local certificate once. macOS asks for your approval, and the certificate only covers this computer.",
            ),
        )
        .child(button(
            "safari-setup",
            "Set up Safari",
            true,
            cx.listener(|view, _: &ClickEvent, _, cx| view.set_up_safari(cx)),
        ))
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
}

impl Render for TillView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot.clone();
        let (pill_bg, pill_fg) = match snapshot.tone {
            Tone::Ready => (READY_BG, READY_FG),
            Tone::Busy => (BUSY_BG, BUSY_FG),
            Tone::Problem => (BAD_BG, BAD_FG),
        };
        let transport = match snapshot.transport {
            Transport::Usb => "USB",
            Transport::Tcp => "Network",
        };
        let saved_host = if snapshot.tcp_host.is_empty() {
            "No network printer saved".to_string()
        } else {
            format!("Saved {}:{}", snapshot.tcp_host, snapshot.tcp_port)
        };

        let mut content = div().flex().flex_col().gap_3();
        match self.page {
            Page::Printer => {
                let no_printer =
                    snapshot.transport == Transport::Usb && snapshot.printers.is_empty();
                content = content.child(
                    card()
                        .child(label("PRINTER"))
                        .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                            if no_printer {
                                "Connect your printer".to_string()
                            } else {
                                snapshot.status.clone()
                            },
                        ))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(if no_printer {
                            "Plug in a USB receipt printer, or choose Network below.".to_string()
                        } else {
                            format!(
                                "{transport} connection · {} mm paper",
                                snapshot.paper.millimetres()
                            )
                        }))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(choice(
                                    "transport-usb",
                                    "USB",
                                    snapshot.transport == Transport::Usb,
                                    cx.listener(|view, _: &ClickEvent, _, cx| {
                                        if view.shared.set_transport(Transport::Usb).is_ok() {
                                            view.shared.set_status(Tone::Ready, "Using USB");
                                        }
                                        view.pull(cx);
                                    }),
                                ))
                                .child(choice(
                                    "transport-network",
                                    "Network",
                                    snapshot.transport == Transport::Tcp,
                                    cx.listener(|view, _: &ClickEvent, _, cx| {
                                        if let Err(error) = view.shared.use_saved_network() {
                                            view.shared.set_status(Tone::Problem, error);
                                        }
                                        view.pull(cx);
                                    }),
                                )),
                        ),
                );
                content = match snapshot.transport {
                    Transport::Usb => content.child(self.printer_card(cx, &snapshot, transport)),
                    Transport::Tcp => content.child(self.network_card(cx, &saved_host)),
                };
                content = content
                    .child(self.paper_card(cx, &snapshot))
                    .child(self.job_buttons(cx))
                    .child(
                        div().text_xs().text_color(rgb(MUTED)).child(
                            snapshot
                                .last_job
                                .clone()
                                .unwrap_or_else(|| "No receipts printed this session.".to_string()),
                        ),
                    );
            }
            Page::Websites => {
                content = content
                    .child(self.sites_card(cx, &snapshot))
                    .child(self.activity_card(&snapshot));
            }
            Page::Settings => {
                if crate::tls::serving() {
                    content = content.child(self.safari_card(cx));
                }
                content = content.child(self.startup_card(cx)).child(self.code_card(cx, &snapshot)).child(self.raw_card(cx, &snapshot))
                    .child(card().child(label("ABOUT"))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(format!("TapPrint {} · 127.0.0.1:{}", env!("CARGO_PKG_VERSION"), snapshot.port)))
                        .child(div().text_xs().text_color(rgb(MUTED)).child("USB printers must use Printer Class. Remove a conflicting system print queue, then reconnect the printer."))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(format!("Settings: {}", snapshot.config_path))));
            }
        }
        let mut tabs = div().flex().gap_2().px_4().pb_3();
        for (id, title, page) in [
            ("tab-printer", "Printer", Page::Printer),
            ("tab-websites", "Websites", Page::Websites),
            ("tab-settings", "Settings", Page::Settings),
        ] {
            tabs = tabs.child(choice(
                id,
                title,
                self.page == page,
                cx.listener(move |view, _: &ClickEvent, _, cx| {
                    view.page = page;
                    view.show_code = false;
                    view.confirm_rotate = false;
                    cx.notify();
                }),
            ));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PAPER))
            .text_color(rgb(INK))
            .child(header(&snapshot, pill_bg, pill_fg))
            .children(self.pair_card(cx, &snapshot))
            .child(tabs)
            .child(
                div()
                    .id("settings")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_4()
                    .child(content),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(SOFT))
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("Only this computer")
                    .child(format!("{} receipts this session", snapshot.jobs)),
            )
    }
}

impl TillView {
    /// Shown above everything while a site waits, so it cannot be missed.
    fn pair_card(
        &self,
        cx: &mut Context<Self>,
        snapshot: &crate::model::Snapshot,
    ) -> Option<gpui::Div> {
        let request = snapshot.pair_request.clone()?;
        let serial = request.serial;
        // The address above is the only thing a page cannot fake. The name
        // below is whatever the page chose to call itself.
        let advice = if request.known {
            "This site can already print, so it should not need to ask. Allow only if you just cleared this browser or set up the till again. The code it had before stops working."
        } else {
            "Allow only if this is your shop's till site and you just pressed print or connect on it. It will be able to print and open the drawer. If unsure, choose Don't allow."
        };
        Some(
            div().px_4().pb_3().child(
                card()
                    .bg(rgb(BUSY_BG))
                    .text_color(rgb(BUSY_FG))
                    .child(label("A website wants to print"))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(request.origin),
                    )
                    .child(div().text_sm().child(advice))
                    .child(hint(format!(
                        "The page calls itself \u{201c}{}\u{201d}. It waits {} more seconds. Esc means Don't allow.",
                        request.client, request.seconds_left
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            // Same weight for both, so neither is the obvious click.
                            .child(choice(
                                "pair-allow",
                                "Allow",
                                false,
                                cx.listener(move |view, _: &ClickEvent, _, cx| {
                                    view.shared.answer_pair(serial, true);
                                    view.pull(cx);
                                }),
                            ))
                            .child(choice(
                                "pair-deny",
                                "Don't allow",
                                false,
                                cx.listener(move |view, _: &ClickEvent, _, cx| {
                                    view.shared.answer_pair(serial, false);
                                    view.pull(cx);
                                }),
                            )),
                    ),
            ),
        )
    }

    fn sites_card(&self, cx: &mut Context<Self>, snapshot: &crate::model::Snapshot) -> gpui::Div {
        let mut list = div().flex().flex_col().gap_2();
        if snapshot.sites.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("No websites paired yet. Choose Connect in your till page, then approve its request here."),
            );
        }
        for origin in &snapshot.sites {
            let remove = origin.clone();
            let asked = self.confirm_remove.as_ref() == Some(origin);
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().text_sm().child(origin.clone()))
                    .child(choice(
                        SharedString::from(format!("remove-{origin}")),
                        if asked {
                            "Click again to remove"
                        } else {
                            "Remove"
                        },
                        asked,
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
        let mut card = card().child(label("ALLOWED WEBSITES")).child(list);
        if self.confirm_remove.is_some() {
            card = card.child(hint(
                "The site stops printing until it asks again and you allow it.",
            ));
        }
        card
    }

    fn raw_card(&self, cx: &mut Context<Self>, snapshot: &crate::model::Snapshot) -> gpui::Div {
        card()
            .child(div().flex().items_center().justify_between().gap_2()
                .child(label("Raw printer commands"))
                .child(toggle("raw-toggle", "Allow raw printer commands", snapshot.allow_raw,
                    cx.listener(|view, _: &ClickEvent, _, cx| {
                        view.shared.set_allow_raw(!view.snapshot.allow_raw); view.pull(cx);
                    }))))
            .child(div().text_xs().text_color(rgb(MUTED)).child(
                "Off is recommended. Raw ESC/POS commands can change printer settings. Enable only if your till requires them.",
            ))
    }

    fn activity_card(&self, snapshot: &crate::model::Snapshot) -> gpui::Div {
        let mut rows = div().flex().flex_col().gap_1();
        if snapshot.clients.is_empty() {
            rows = rows.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("No till page is open."),
            );
        } else {
            for client in &snapshot.clients {
                let state = if client.live {
                    "connected".to_string()
                } else {
                    crate::model::ago_words(std::time::Duration::from_secs(client.seconds_ago))
                };
                rows = rows.child(
                    div()
                        .text_sm()
                        .child(format!("{}  ·  {state}", client.label)),
                );
            }
        }
        let last = snapshot
            .last_job
            .clone()
            .unwrap_or_else(|| "No print yet".to_string());
        let jobs = if snapshot.jobs == 1 { "job" } else { "jobs" };
        card()
            .child(label("RECENT ACTIVITY"))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("Till pages open on this computer."),
            )
            .child(rows)
            .child(hint(format!("Last job: {last}")))
            .child(hint(format!(
                "{} {jobs} printed since TapPrint opened  ·  version {}",
                snapshot.jobs,
                env!("CARGO_PKG_VERSION")
            )))
    }

    fn code_card(&self, cx: &mut Context<Self>, snapshot: &crate::model::Snapshot) -> gpui::Div {
        let copy_label = if self.copied { "Copied" } else { "Copy code" };
        let (rotate_label, rotate_hint) = if self.confirm_rotate {
            (
                "Replace code",
                "Anything using this code stops printing until you paste the new one. Allowed sites keep working. Click Replace code to go ahead.",
            )
        } else {
            (
                "New code",
                "Keep this code private. Use Copy code for manual integrations.",
            )
        };
        card()
            .child(label("MANUAL PAIRING"))
            .child(
                div().text_xs().text_color(rgb(MUTED)).child(
                    "Websites connect with Allow. Use this code only for manual integrations.",
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(SOFT))
                    .bg(rgb(PAPER))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .line_height(px(22.))
                            .child(if self.show_code {
                                grouped_code(&snapshot.pairing_code)
                            } else {
                                "•••• •••• •••• ••••".to_string()
                            }),
                    )
                    .child(
                        div()
                            .id("reveal-code")
                            .role(gpui::Role::Button)
                            .aria_label(if self.show_code {
                                "Hide pairing code"
                            } else {
                                "Show pairing code"
                            })
                            .focusable()
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(36.))
                            .h(px(36.))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(PAPER))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(SOFT)))
                            .focus_visible(|style| style.border_color(rgb(READY_FG)))
                            .on_click(cx.listener(|view, _: &ClickEvent, _, cx| {
                                view.show_code = !view.show_code;
                                cx.notify();
                            }))
                            .child(
                                gpui::svg()
                                    .data(if self.show_code { EYE_OFF } else { EYE })
                                    .w(px(18.))
                                    .h(px(18.))
                                    .text_color(rgb(MUTED)),
                            ),
                    ),
            )
            .child(hint(rotate_hint))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(choice(
                        "copy-code",
                        copy_label,
                        true,
                        cx.listener(|view, _: &ClickEvent, _, cx| view.copy_code(cx)),
                    ))
                    .child(choice(
                        "new-code",
                        rotate_label,
                        false,
                        cx.listener(|view, _: &ClickEvent, _, cx| view.rotate(cx)),
                    )),
            )
    }

    fn paper_card(&self, cx: &mut Context<Self>, snapshot: &crate::model::Snapshot) -> gpui::Div {
        card().child(label("Paper")).child(
            div()
                .flex()
                .gap_2()
                .child(choice(
                    "paper-80",
                    "80 mm",
                    snapshot.paper == Paper::Mm80,
                    cx.listener(|view, _: &ClickEvent, _, cx| {
                        view.shared.set_paper(Paper::Mm80);
                        view.pull(cx);
                    }),
                ))
                .child(choice(
                    "paper-58",
                    "58 mm",
                    snapshot.paper == Paper::Mm58,
                    cx.listener(|view, _: &ClickEvent, _, cx| {
                        view.shared.set_paper(Paper::Mm58);
                        view.pull(cx);
                    }),
                )),
        )
    }

    fn printer_card(&self, cx: &mut Context<Self>, snapshot: &crate::model::Snapshot, transport: &str) -> gpui::Div {
        let mut list = div().flex().flex_col().gap_2();
        if snapshot.printers.is_empty() {
            let message = if snapshot.listed {
                "No USB printer is plugged in."
            } else {
                "Looking for USB printers…"
            };
            list = list.child(div().text_sm().text_color(rgb(MUTED)).child(message));
        } else {
            for (id, name) in &snapshot.printers {
                let selected =
                    snapshot.transport == Transport::Usb && snapshot.usb_id.as_ref() == Some(id);
                let caption = if selected {
                    format!("{name}  ·  In use")
                } else {
                    name.clone()
                };
                let id = id.clone();
                list = list.child(choice(
                    SharedString::from(format!("usb-{id}")),
                    caption,
                    selected,
                    cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.shared.select_usb(id.clone());
                        view.pull(cx);
                    }),
                ));
            }
        }
        card()
            .child(label("USB printer"))
            .child(div().text_sm().child(format!(
                "{transport} devices · selected automatically when only one is available"
            )))
            .child(list)
    }

    fn network_card(&self, cx: &mut Context<Self>, saved_host: &str) -> gpui::Div {
        card()
            .key_context("TapPrint")
            .on_action(cx.listener(Self::on_save))
            .child(label("Network printer"))
            .child(hint(
                "Port 9100 is the usual raw printer port. Press Enter to use this address.",
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        text_input("tcp-host")
                            .state(self.host.downgrade())
                            .placeholder("192.168.1.20")
                            .placeholder_color(rgb(MUTED))
                            .caret_color(gpui::black())
                            .selection_color(hsla(0.58, 0.28, 0.72, 0.7))
                            .caret_blink_interval_500ms()
                            .flex_1()
                            .px_3()
                            .py_2()
                            .bg(rgb(PAPER))
                            .border_1()
                            .rounded_md()
                            .text_sm(),
                    )
                    .child(
                        text_input("tcp-port")
                            .state(self.port.downgrade())
                            .placeholder("9100")
                            .placeholder_color(rgb(MUTED))
                            .caret_color(gpui::black())
                            .selection_color(hsla(0.58, 0.28, 0.72, 0.7))
                            .caret_blink_interval_500ms()
                            .w(px(84.))
                            .px_3()
                            .py_2()
                            .bg(rgb(PAPER))
                            .border_1()
                            .rounded_md()
                            .text_sm(),
                    ),
            )
            .child(hint(saved_host.to_string()))
            .child(choice(
                "use-network",
                "Use this address",
                true,
                cx.listener(|view, _: &ClickEvent, _, cx| view.save_network(cx)),
            ))
    }

    fn startup_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(label("Start at login"))
                    .child(toggle(
                        "autostart-toggle",
                        "Start TapPrint at login",
                        self.autostart,
                        cx.listener(|view, _: &ClickEvent, _, cx| {
                            view.set_autostart(!view.autostart, cx)
                        }),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("Keep this till ready to print after a restart."),
            )
    }

    fn job_buttons(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap_2()
            .child(choice(
                "test-slip",
                "Print test slip",
                true,
                cx.listener(|view, _: &ClickEvent, _, cx| {
                    view.job("Printing a test slip", print::print_test, cx)
                }),
            ))
            .child(choice(
                "open-drawer",
                "Open drawer",
                false,
                cx.listener(|view, _: &ClickEvent, _, cx| {
                    view.job("Opening the drawer", print::open_drawer, cx)
                }),
            ))
    }
}

fn header(snapshot: &crate::model::Snapshot, pill_bg: u32, pill_fg: u32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_4()
        .pt_4()
        .pb_3()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("TapPrint"),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(READY_BG))
                        .text_color(rgb(READY_FG))
                        .text_xs()
                        .child("Running locally"),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("Your receipt printer, connected to your till."),
        )
        .when(snapshot.tone != Tone::Ready, |view| {
            view.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(pill_bg))
                    .text_color(rgb(pill_fg))
                    .text_xs()
                    .child(snapshot.status.clone()),
            )
        })
}

fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(rgb(SOFT))
        .bg(rgb(CARD))
}

fn label(text: &'static str) -> gpui::Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED))
        .child(text)
}

/// Small grey help text under a heading.
fn hint(text: impl Into<SharedString>) -> gpui::Div {
    div().text_xs().text_color(rgb(MUTED)).child(text.into())
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

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let label: SharedString = label.into();
    let (background, foreground) = if primary { (INK, CARD) } else { (CARD, INK) };
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .focusable()
        .tab_index(0)
        .on_click(on_click)
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(if primary { INK } else { SOFT }))
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_sm()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(if primary { MUTED } else { PAPER })))
        .active(|style| style.bg(rgb(MUTED)).text_color(rgb(CARD)))
        .focus_visible(|style| style.border_color(rgb(READY_FG)))
        .child(label)
}

fn choice(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    button(id, label, selected, on_click)
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
        .h(px(44.))
        .px_1()
        .flex()
        .items_center()
        .cursor_pointer()
        .rounded_md()
        .border_1()
        .border_color(rgb(CARD))
        .focus_visible(|style| style.border_color(rgb(READY_FG)))
        .hover(|style| style.bg(rgb(PAPER)))
        .child(
            div()
                .w(px(44.))
                .h(px(26.))
                .p(px(3.))
                .rounded_full()
                .flex()
                .items_center()
                .bg(rgb(if on { READY_FG } else { MUTED }))
                .when(on, |track| track.justify_end())
                .child(div().w(px(20.)).h(px(20.)).rounded_full().bg(rgb(CARD))),
        )
}
