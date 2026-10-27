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
        Quit
    ]
);

const PAPER: u32 = 0xf3f0e8;
const INK: u32 = 0x1c1915;
const MUTED: u32 = 0x6e675e;
const CARD: u32 = 0xfffcf7;
const SOFT: u32 = 0xe7e1d4;
const READY_BG: u32 = 0xe3efe4;
const READY_FG: u32 = 0x1d4a2c;
const BUSY_BG: u32 = 0xf4e7c8;
const BUSY_FG: u32 = 0x6b4d14;
const BAD_BG: u32 = 0xf6e3de;
const BAD_FG: u32 = 0x7a2d22;

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
    cx.bind_keys([KeyBinding::new("enter", SaveNetwork, Some("TapPrint"))]);
    cx.on_action(open_window)
        .on_action(print_test)
        .on_action(open_drawer)
        .on_action(use_usb)
        .on_action(use_network)
        .on_action(quit);

    let tray = Tray::builder()
        .icon(tray_icon()?)
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
        let _ = tray.set_title(
            Some(tray_title(snapshot.tone, snapshot.pair_request.is_some())),
            cx,
        );
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
    vec![
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
    ]
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
    let bounds = Bounds::centered(None, size(px(440.), px(760.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("TapPrint")),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        is_resizable: true,
        window_min_size: Some(size(px(400.), px(640.))),
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
    shared.set_transport(Transport::Usb);
    shared.set_status(Tone::Ready, "Using USB");
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

fn tray_icon() -> gpui_tray::Result<Icon> {
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
            if paper && !line {
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

struct TillView {
    shared: Arc<Shared>,
    snapshot: crate::model::Snapshot,
    host: Entity<EditableTextState>,
    port: Entity<EditableTextState>,
    /// The first click on New code only asks. The second one replaces it.
    confirm_rotate: bool,
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
            shared,
            snapshot,
            host,
            port,
            confirm_rotate: false,
            copied: false,
            autostart: crate::autostart::enabled(),
            _subscriptions: subscriptions,
        }
    }

    /// Re-read shared state. Any click also drops the one-shot hints.
    fn pull(&mut self, cx: &mut Context<Self>) {
        self.confirm_rotate = false;
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

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PAPER))
            .text_color(rgb(INK))
            .child(header(&snapshot, pill_bg, pill_fg))
            .children(self.pair_card(cx, &snapshot))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_4()
                    .pb_4()
                    .id("settings")
                    .overflow_y_scroll()
                    .child(self.activity_card(&snapshot))
                    .child(self.sites_card(cx, &snapshot))
                    .child(self.code_card(cx, &snapshot))
                    .child(self.paper_card(cx, &snapshot))
                    .child(self.printer_card(cx, &snapshot, transport))
                    .child(self.network_card(cx, &saved_host))
                    .child(self.job_buttons(cx))
                    .child(self.startup_card(cx))
                    .child(self.raw_card(cx, &snapshot))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("The printer's USB port must be in Printer Class. If the system already has a queue for it, remove that queue and plug it in again.")
                            .child(
                                div().mt_1().child(format!("Settings file {}", snapshot.config_path)),
                            ),
                    ),
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
        let advice = if request.known {
            format!(
                "From {}. This site can already print, so it should not need to ask. Allow only if you cleared this browser or moved the till. Its old code stops working.",
                request.client
            )
        } else {
            format!(
                "From {}. Allow it only if this is your till or shop site. It will be able to print and open the drawer.",
                request.client
            )
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
                    .child(div().text_xs().child(advice))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button(
                                "pair-allow",
                                "Allow",
                                true,
                                cx.listener(move |view, _: &ClickEvent, _, cx| {
                                    view.shared.answer_pair(serial, true);
                                    view.pull(cx);
                                }),
                            ))
                            .child(button(
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
                    .child("No site yet. A till page asks once, and you allow it here."),
            );
        }
        for origin in &snapshot.sites {
            let remove = origin.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().text_sm().child(origin.clone()))
                    .child(choice(
                        SharedString::from(format!("remove-{origin}")),
                        "Remove",
                        false,
                        cx.listener(move |view, _: &ClickEvent, _, cx| {
                            view.shared.remove_site(&remove);
                            view.pull(cx);
                        }),
                    )),
            );
        }
        card().child(label("Allowed sites")).child(list)
    }

    fn raw_card(&self, cx: &mut Context<Self>, snapshot: &crate::model::Snapshot) -> gpui::Div {
        card()
            .child(label("Allow raw printer commands"))
            .child(div().text_xs().text_color(rgb(MUTED)).child(
                "Lets paired pages send ESC/POS bytes as they are. Leave this off unless your till software needs it: raw commands can change printer settings.",
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(choice(
                        "raw-on",
                        "On",
                        snapshot.allow_raw,
                        cx.listener(|view, _: &ClickEvent, _, cx| {
                            view.shared.set_allow_raw(true);
                            view.pull(cx);
                        }),
                    ))
                    .child(choice(
                        "raw-off",
                        "Off",
                        !snapshot.allow_raw,
                        cx.listener(|view, _: &ClickEvent, _, cx| {
                            view.shared.set_allow_raw(false);
                            view.pull(cx);
                        }),
                    )),
            )
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
            .child(label("Connected"))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("Till pages open on this computer."),
            )
            .child(rows)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!("Last job: {last}")),
            )
            .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                "{} {jobs} since open  ·  {}",
                snapshot.jobs,
                env!("CARGO_PKG_VERSION")
            )))
    }

    fn code_card(&self, cx: &mut Context<Self>, snapshot: &crate::model::Snapshot) -> gpui::Div {
        let copy_label = if self.copied { "Copied" } else { "Copy code" };
        let (rotate_label, hint) = if self.confirm_rotate {
            (
                "Replace code",
                "Anything using this code stops printing until you paste the new one. Allowed sites keep working. Click Replace code to go ahead.",
            )
        } else {
            (
                "New code",
                "For scripts, servers, and pages that cannot ask. Click the code to copy it.",
            )
        };
        card()
            .child(label("Manual pairing code"))
            .child(
                div()
                    .id("pairing-code")
                    .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.copy_code(cx)))
                    .cursor_pointer()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(28.))
                    .child(grouped_code(&snapshot.pairing_code)),
            )
            .child(div().text_xs().text_color(rgb(MUTED)).child(hint))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        "copy-code",
                        copy_label,
                        true,
                        cx.listener(|view, _: &ClickEvent, _, cx| view.copy_code(cx)),
                    ))
                    .child(button(
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

    fn printer_card(
        &self,
        cx: &mut Context<Self>,
        snapshot: &crate::model::Snapshot,
        transport: &str,
    ) -> gpui::Div {
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
            .child(
                div()
                    .text_sm()
                    .child(format!("Sending receipts over {transport}")),
            )
            .child(list)
    }

    fn network_card(&self, cx: &mut Context<Self>, saved_host: &str) -> gpui::Div {
        card()
            .key_context("TapPrint")
            .on_action(cx.listener(Self::on_save))
            .child(label("Network printer"))
            .child(
                div().text_xs().text_color(rgb(MUTED)).child(
                    "Port 9100 is the usual raw printer port. Press Enter to use this address.",
                ),
            )
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
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(saved_host.to_string()),
            )
            .child(button(
                "use-network",
                "Use this address",
                true,
                cx.listener(|view, _: &ClickEvent, _, cx| view.save_network(cx)),
            ))
    }

    fn startup_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        card()
            .child(label("Start with this computer"))
            .child(div().text_xs().text_color(rgb(MUTED)).child(
                "Turn this on for the till, so receipts print after a restart without opening the app.",
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(choice(
                        "autostart-on",
                        "On",
                        self.autostart,
                        cx.listener(|view, _: &ClickEvent, _, cx| view.set_autostart(true, cx)),
                    ))
                    .child(choice(
                        "autostart-off",
                        "Off",
                        !self.autostart,
                        cx.listener(|view, _: &ClickEvent, _, cx| view.set_autostart(false, cx)),
                    )),
            )
    }

    fn job_buttons(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap_2()
            .child(button(
                "test-slip",
                "Print test slip",
                true,
                cx.listener(|view, _: &ClickEvent, _, cx| {
                    view.job("Printing a test slip", print::print_test, cx)
                }),
            ))
            .child(button(
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
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child("TapPrint"),
        )
        .child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgb(pill_bg))
                .text_color(rgb(pill_fg))
                .text_sm()
                .child(snapshot.status.clone()),
        )
        .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
            "127.0.0.1:{}  ·  only this computer",
            snapshot.port
        )))
}

fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .bg(rgb(CARD))
}

fn label(text: &'static str) -> gpui::Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED))
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

fn button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let (background, foreground) = if primary {
        (rgb(INK), rgb(CARD))
    } else {
        (rgb(SOFT), rgb(INK))
    };
    div()
        .id(id)
        .on_click(on_click)
        .px_3()
        .py_2()
        .rounded_md()
        .bg(background)
        .text_color(foreground)
        .text_sm()
        .cursor_pointer()
        .child(label)
}

fn choice(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let (background, foreground) = if selected {
        (rgb(INK), rgb(CARD))
    } else {
        (rgb(SOFT), rgb(INK))
    };
    div()
        .id(id)
        .on_click(on_click)
        .px_3()
        .py_2()
        .rounded_md()
        .bg(background)
        .text_color(foreground)
        .text_sm()
        .cursor_pointer()
        .child(label.into())
}
