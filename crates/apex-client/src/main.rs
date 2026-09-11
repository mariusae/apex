#![allow(unexpected_cfgs)] // objc's macros mention cargo-clippy
//! apex-ui: the UI client. Renders a session's state and turns mouse and
//! keys into log entries.
//!
//! `apex-ui [files]` (what the app bundle runs) attaches to the local
//! daemon, starting it if needed, and reopens the sessions it had open
//! last time (else the first existing one, else a new `local`).
//! `--session S` picks one; `--attach [SOCKET]` names the daemon;
//! `--via CMD` attaches through CMD's stdin/stdout; `--remote DEST`
//! attaches to a destination through its provider (`user@host`, or
//! `provider:name`); `--local` runs the server in-process, the old
//! prototype's way.

mod app;
mod cursor;
mod field;
mod finder;
mod menu;
mod shell;
mod switcher;
mod term_element;
mod pool;
mod text_element;
mod theme;
mod warp;
mod web;

use gpui::{
    canvas, div, prelude::*, px, size, App, Bounds, Context, MouseButton, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};

use apex_core::{Body, ViewId};

use apex_server::providers::SessionUrl;
use app::Acme;
use term_element::TermElement;
use text_element::TextElement;

impl Render for Acme {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(loc) = self.pending_switch.take() {
            self.switch_for(loc, window, cx);
        }
        if self.leave_requested {
            self.leave(window, cx);
        }
        if self.close_requested {
            shell::log_line(&format!("closing the window on {}", self.url));
            self.park_into_pool(cx);
            window.remove_window(); // Exit: the session stays, parked
            return div().into_any_element();
        }
        // a pending mouse warp uses the layouts of the frame just drawn
        self.resolve_warp(window, cx);
        self.sync();
        self.measure(window.viewport_size());
        self.sync();
        self.schedule_warp(window);
        let title = self.current_title();
        if title != self.title_shown {
            if std::env::var_os("APEX_DEBUG").is_some() {
                eprintln!("apex-ui: title: {title}");
            }
            window.set_window_title(&title);
            self.title_shown = title;
        }
        self.layouts.clear();
        self.term_layouts.clear();
        let me = cx.entity();
        let font = f32::from(text_element::font_for(false).line_height) as i32;

        let t = theme::theme();
        let root = div()
            .id("apex")
            .size_full()
            .bg(gpui::rgb(t.border))
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &shell::Undo, window, cx| this.menu_edit("undo", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Redo, window, cx| this.menu_edit("redo", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Cut, window, cx| this.menu_edit("cut", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Copy, window, cx| this.menu_edit("copy", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Paste, window, cx| this.menu_edit("paste", window, cx)))
            .on_action(cx.listener(|this, _: &shell::SelectAll, window, cx| this.menu_command("Edit ,", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Put, window, cx| this.menu_command("Put", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Get, window, cx| this.menu_command("Get", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Del, window, cx| this.menu_command("Del", window, cx)))
            .on_action(cx.listener(|this, _: &shell::NewFile, window, cx| this.menu_command("New", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Sessions, _, cx| this.open_selector(cx)))
            .on_action(cx.listener(|this, _: &shell::PreviousSession, window, cx| this.previous_session(window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab1, window, cx| this.go_to_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab2, window, cx| this.go_to_tab(2, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab3, window, cx| this.go_to_tab(3, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab4, window, cx| this.go_to_tab(4, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab5, window, cx| this.go_to_tab(5, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab6, window, cx| this.go_to_tab(6, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab7, window, cx| this.go_to_tab(7, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab8, window, cx| this.go_to_tab(8, window, cx)))
            .on_action(cx.listener(|this, _: &shell::Tab9, window, cx| this.go_to_tab(9, window, cx)))
            // the host's profile, the session's setup: opened, or made
            .on_action(cx.listener(|this, _: &shell::Profile, window, cx| this.menu_command("New ~/.apex/profile", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Goto, _, cx| this.open_finder(cx)))
            // a UI hack, on purpose: the keys just say the verbs, which a
            // tool answers
            .on_action(cx.listener(|this, _: &shell::NavBack, window, cx| this.menu_command("Back", window, cx)))
            .on_action(cx.listener(|this, _: &shell::NavFwd, window, cx| this.menu_command("Fwd", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Reconnect, window, cx| {
                this.reconnect(window);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &shell::CloseWindow, window, cx| {
                // the session stays, parked: a window on it later is instant
                this.park_into_pool(cx);
                window.remove_window()
            }))
            .on_action(cx.listener(|_, _: &shell::ToggleFullScreen, window, _| window.toggle_fullscreen()))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::mouse_up))
            // B4: the tools menu (a real fourth button, or shift-click)
            .on_mouse_down(MouseButton::Navigate(gpui::NavigationDirection::Back), cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Navigate(gpui::NavigationDirection::Back), cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Navigate(gpui::NavigationDirection::Back), cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel));
        // full screen: acme's area is the whole screen, no title bar
        self.fullscreen = window.is_fullscreen();
        let root = if self.fullscreen { root } else { root.child(self.titlebar(cx)) };

        // acme's tiling placed everything; draw each piece where it says
        let l = self.node.state.layout.clone();
        let at = |x: i32, y: i32, w: i32, h: i32, el: gpui::AnyElement| {
            div().absolute().left(px(x as f32)).top(px(y as f32)).w(px(w.max(0) as f32)).h(px(h.max(0) as f32)).overflow_hidden().child(el)
        };
        // acme's Border is scalesize(display, 2): 2 device pixels at 1x,
        // (2*dpi+66)/133 above (devdraw says 110 per unit of scale), 3 at
        // 2x; the tiling's gap is 2 logical pixels, 4 at 2x. The gaps stay
        // as laid out (integer logical pixels, crisp text), and the device
        // pixels beyond acme's are painted over in the neighbour's colour:
        // a window's or a column's tag reaches up, a column's contents
        // reach left, so the black that shows is acme's
        let scale = window.scale_factor();
        let acme_border = if scale <= 1. { 2. } else { ((2. * (scale * 110.).floor() + 66.) / 133.).floor() };
        let extra = ((apex_core::tiling::BORDER as f32 * scale - acme_border).max(0.) / scale).min(apex_core::tiling::BORDER as f32);
        let fill = |x: f32, y: f32, w: f32, h: f32, c: u32| div().absolute().left(px(x)).top(px(y)).w(px(w.max(0.))).h(px(h.max(0.))).bg(gpui::rgb(c));
        // acme's pointer over acme's part of the window only; the box while
        // a layout box is held (the innermost hitbox's style wins)
        let pointer = if self.dragging_box() {
            cursor::BOX_CURSOR
        } else if self.over_page(window) {
            cursor::NATIVE_CURSOR // the page's own, set as it asks
        } else {
            cursor::BIG_ARROW
        };
        let mut area = div().relative().flex_1().min_h_0().w_full().overflow_hidden().cursor(pointer);
        // web windows drawn this frame keep their native views; the rest hide
        let mut webs_shown = std::collections::HashSet::new();
        area = area.child(at(l.r.x0, l.r.y0, l.r.dx(), font, TextElement { acme: me.clone(), view: ViewId::Top }.into_any_element()));
        for col in &l.cols {
            // acme's colinit: the column is white where no window is, the
            // tail below its last window (or its tag and the border under
            // it); everywhere else the black root is the borders between
            // the tag and the windows and between the windows
            let tail = col.wins.last().map(|s| s.r.y1).unwrap_or(col.r.y0 + font + apex_core::tiling::BORDER);
            if tail < col.r.y1 {
                area = area.child(at(col.r.x0, tail, col.r.dx(), col.r.y1 - tail, div().size_full().bg(gpui::rgb(t.column)).into_any_element()));
            }
            if extra > 0. {
                // the borders trimmed to acme's: above the column tag and
                // each window's tag (their colour), and, for a column with
                // one to its left, along its left edge in what is there
                let (x0, w, e) = (col.r.x0 as f32, col.r.dx() as f32, extra);
                let left = if col.r.x0 > 0 { e } else { 0. };
                area = area.child(fill(x0, col.r.y0 as f32 - e, w, e, t.tag_bg));
                if left > 0. {
                    area = area.child(fill(x0 - left, col.r.y0 as f32 - e, left, (font as f32) + e, t.tag_bg));
                    if tail < col.r.y1 {
                        area = area.child(fill(x0 - left, tail as f32, left, (col.r.y1 - tail) as f32, t.column));
                    }
                }
                for (i, s) in col.wins.iter().enumerate() {
                    if !col.safe && i > 0 {
                        continue;
                    }
                    let Ok(win) = self.node.state.window(s.window) else { continue };
                    let tag_h = if s.body.dy() > 0 { s.body.y0 - s.r.y0 } else { s.r.dy() };
                    area = area.child(fill(s.r.x0 as f32, s.r.y0 as f32 - e, s.r.dx() as f32, e, t.tag_bg));
                    if left > 0. {
                        area = area.child(fill(x0 - left, s.r.y0 as f32 - e, left, tag_h as f32 + e, t.tag_bg));
                        if s.body.dy() > 0 {
                            let c = match win.body {
                                Body::Web | Body::Html(_) => 0xffffff,
                                _ => t.body_bg,
                            };
                            area = area.child(fill(x0 - left, s.body.y0 as f32, left, s.body.dy() as f32, c));
                        }
                    }
                }
            }
            area = area.child(at(col.r.x0, col.r.y0, col.r.dx(), font, TextElement { acme: me.clone(), view: ViewId::ColTag(col.id) }.into_any_element()));
            for (i, s) in col.wins.iter().enumerate() {
                if !col.safe && i > 0 {
                    continue; // obscured by the full-column window
                }
                let w = s.window;
                let Ok(win) = self.node.state.window(w) else { continue };
                let tag_h = if s.body.dy() > 0 { s.body.y0 - s.r.y0 } else { s.r.dy() };
                area = area.child(at(s.r.x0, s.r.y0, s.r.dx(), tag_h, TextElement { acme: me.clone(), view: ViewId::Tag(w) }.into_any_element()));
                if s.body.dy() > 0 {
                    let body = match win.body {
                        Body::Text(_) => TextElement { acme: me.clone(), view: ViewId::Body(w) }.into_any_element(),
                        Body::Term(t) => TermElement { acme: me.clone(), window: w, term: t }.into_any_element(),
                        Body::Web | Body::Html(_) => {
                            // the native view goes where this canvas lands;
                            // over it the pointer is the page's own (a style
                            // whose cursor sets nothing, the innermost hitbox's
                            // style winning), so links get the hand
                            webs_shown.insert(w);
                            let me2 = me.clone();
                            // the paper under the view: what shows while an
                            // overlay (the picker, the finder, the tools
                            // menu) has the native view hidden, else the
                            // root's black would
                            let paper = if matches!(win.body, Body::Html(_)) { t.body_bg } else { t.column };
                            div()
                                .size_full()
                                .bg(gpui::rgb(paper))
                                .cursor(cursor::NATIVE_CURSOR)
                                .child(
                                    canvas(
                                        move |bounds, window, cx| {
                                            me2.update(cx, |acme, _| acme.web_place(w, bounds, window));
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .size_full(),
                                )
                                .into_any_element()
                        }
                    };
                    area = area.child(at(s.body.x0, s.body.y0, s.body.dx(), s.body.dy(), body));
                }
            }
        }
        if let Some(m) = &self.menu {
            area = area.child(menu_element(m, font));
        }
        let alive: std::collections::HashSet<apex_core::WindowId> = self.node.state.windows.keys().copied().collect();
        self.webs.settle(&webs_shown, |w| alive.contains(&w));
        let root = root.child(area);
        let root = match self.selector_panel(cx) {
            Some(panel) => root.child(panel),
            None => root,
        };
        let root = match self.finder_panel(cx) {
            Some(panel) => root.child(panel),
            None => root,
        };
        match self.switcher_panel(cx) {
            Some(panel) => root.child(panel).into_any_element(),
            None => root.into_any_element(),
        }
    }
}

/// Where a window's session lives.
#[derive(Clone, Debug)]
enum Target {
    /// The server in this process, the old prototype's way.
    Local(Vec<String>),
    /// A session URL: `local:///name` here, or through a provider.
    Url { url: SessionUrl, files: Vec<String> },
    /// Through an arbitrary command's stdin/stdout.
    Via { cmd: String, session: String, files: Vec<String> },
    /// A window with the session picker open, attached to nothing yet:
    /// what a new window is until a session is chosen (`url` is the one
    /// the picker calls current, the active window's).
    Chooser { url: SessionUrl },
}

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut local = false;
    let mut socket: Option<std::path::PathBuf> = None;
    let mut via: Option<String> = None;
    let mut remote: Option<String> = None;
    let mut url: Option<String> = None;
    let mut session: Option<String> = None;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--local" => local = true,
            "--attach" => {
                socket = Some(match args.peek() {
                    Some(p) if p.ends_with(".sock") => std::path::PathBuf::from(args.next().unwrap()),
                    _ => apex_server::daemon::default_socket(),
                });
            }
            "--via" => via = Some(args.next().expect("--via CMD")),
            "--remote" | "--ssh" => remote = Some(args.next().expect("--remote DESTINATION")),
            "--url" => url = Some(args.next().expect("--url URL")),
            "--session" => session = Some(args.next().expect("--session NAME")),
            // Finder passes this when launching a bundle
            a if a.starts_with("-psn_") => {}
            _ => files.push(a),
        }
    }
    if let Some(p) = &socket {
        std::env::set_var("APEX_SOCKET", p);
    }
    let socket = socket.unwrap_or_else(apex_server::daemon::default_socket);

    // from the Finder we start with LaunchServices' bare environment
    let adopted = shell::adopt_login_shell_environment();
    if std::env::var_os("APEX_DEBUG_ENV").is_some() {
        eprintln!("apex-ui: adopted from the login shell: {adopted:?}; PATH={}", std::env::var("PATH").unwrap_or_default());
    }
    gpui_platform::application().run(move |cx: &mut App| {
        cursor::install();
        pool::Pool::install(cx);
        theme::load();
        cx.set_menus(shell::menus());
        // the View menu: the theme, the choice marked in the menu
        cx.on_action(|_: &shell::ThemeLight, cx| shell::set_theme(theme::Mode::Light, cx));
        cx.on_action(|_: &shell::ThemeDark, cx| shell::set_theme(theme::Mode::Dark, cx));
        cx.on_action(|_: &shell::ThemeSystem, cx| shell::set_theme(theme::Mode::System, cx));
        cx.bind_keys(shell::bindings());
        cx.on_action(|_: &shell::Quit, cx| {
            // the action arrives while the focused window is mid-update,
            // where it cannot be read: everything here waits for that
            cx.defer(|cx| {
                // the windows as they are now come back next time
                shell::save_open(cx);
                shell::QUITTING.store(true, std::sync::atomic::Ordering::Relaxed);
                // every link ends before we do: the bridges go with us, and
                // the daemons see the attachments leave
                for w in cx.windows() {
                    if let Some(h) = w.downcast::<Acme>() {
                        let _ = h.update(cx, |acme, _, _| acme.close_link());
                    }
                }
                pool::Pool::close_all(cx);
                cx.quit();
            });
        });
        cx.on_action(|_: &shell::HideApp, cx| cx.hide());
        cx.on_action(|_: &shell::About, _| eprintln!("apex: acme, remade — https://github.com/apex"));
        cx.on_action(|_: &shell::InstallCli, cx| {
            let msg = match shell::install_cli() {
                Ok(where_) => format!("apex command installed: {where_}\n"),
                Err(e) => format!("apex command not installed: {e}\n"),
            };
            eprint!("{msg}");
            if let Some(h) = cx.active_window().and_then(|w| w.downcast::<Acme>()) {
                let _ = h.update(cx, |acme, _, cx| {
                    acme.notice(&msg);
                    cx.notify();
                });
            }
        });
        cx.on_action(move |_: &shell::NewWindow, cx| {
            // the first window goes to the local default; another one asks
            // which session, in the picker, before attaching anywhere
            // "any window at all" decides, not the active one: during action
            // dispatch there may be no active window to ask
            let ours: Vec<gpui::WindowHandle<Acme>> = cx.windows().into_iter().filter_map(|w| w.downcast::<Acme>()).collect();
            let current = cx
                .active_window()
                .and_then(|w| w.downcast::<Acme>())
                .into_iter()
                .chain(ours.iter().copied())
                .find_map(|h| h.read(cx).ok().map(|a| a.url.clone()));
            shell::log_line(&format!("new window: {} open, current {:?}", ours.len(), current.as_ref().map(|u| u.to_string())));
            match current {
                None if ours.is_empty() => {
                    open_window(cx, Target::Url { url: SessionUrl::local(apex_server::providers::DEFAULT_SESSION), files: Vec::new() }, None);
                }
                current => {
                    let url = current.unwrap_or_else(|| SessionUrl::local(apex_server::providers::DEFAULT_SESSION));
                    if let Some(h) = open_window(cx, Target::Chooser { url }, None) {
                        let _ = h.update(cx, |acme, _, cx| acme.open_selector(cx));
                    }
                }
            }
            cx.defer(|cx| shell::save_open(cx));
        });

        let default = || session.clone().unwrap_or_else(|| apex_server::providers::DEFAULT_SESSION.to_string());
        let targets: Vec<(Target, Option<WindowBounds>)> = if local {
            vec![(Target::Local(files.clone()), None)]
        } else if let Some(cmd) = via.clone() {
            vec![(Target::Via { cmd, session: default(), files: files.clone() }, None)]
        } else if let Some(u) = url.clone() {
            match SessionUrl::parse(&u) {
                Some(url) => vec![(Target::Url { url, files: files.clone() }, None)],
                None => {
                    eprintln!("apex-ui: bad session URL {u:?}");
                    std::process::exit(2);
                }
            }
        } else if let Some(dest) = remote.clone() {
            let d = apex_server::providers::Dest::parse(&dest);
            vec![(Target::Url { url: SessionUrl { provider: d.provider, arg: d.name, session: default(), id: None }, files: files.clone() }, None)]
        } else {
            if let Err(e) = shell::ensure_daemon(&socket) {
                eprintln!("apex-ui: {e}");
                std::process::exit(1);
            }
            let urls = match &session {
                Some(s) => vec![(SessionUrl::local(s), None)],
                None => shell::plan(&socket).unwrap_or_else(|e| {
                    eprintln!("apex-ui: {e}");
                    std::process::exit(1);
                }),
            };
            urls.into_iter().map(|(url, frame)| (Target::Url { url, files: files.clone() }, frame)).collect()
        };
        let shown: Vec<SessionUrl> = targets.iter().filter_map(|(t, _)| match t {
            Target::Url { url, .. } => Some(url.clone()),
            _ => None,
        }).collect();
        for (t, frame) in targets {
            open_window(cx, t, frame);
        }
        shell::save_open(cx);
        // the tabs of last time, attached again in the background and parked
        pool::Pool::restore(cx, &shown);
        cx.activate(true);
        // quitting from the Dock or by AppleScript does not run our Quit
        // action: remember the windows before they close on the way out
        cx.on_app_quit(|cx| {
            shell::save_open(cx);
            shell::QUITTING.store(true, std::sync::atomic::Ordering::Relaxed);
            gpui::Task::ready(())
        })
        .detach();
        cx.on_window_closed(|cx, _| {
            shell::save_open(cx);
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}

fn open_window(cx: &mut App, target: Target, frame: Option<WindowBounds>) -> Option<gpui::WindowHandle<Acme>> {
    let n = cx.windows().len() as f32;
    let bounds = frame.unwrap_or_else(|| {
        let mut b = Bounds::centered(None, size(px(1100.), px(760.)), cx);
        b.origin.x += px(24. * n);
        b.origin.y += px(24. * n);
        WindowBounds::Windowed(b)
    });
    let title = match &target {
        Target::Local(_) => "apex".to_string(),
        Target::Url { url, .. } => Acme::title(url),
        Target::Via { cmd, session, .. } => format!("{session} via {} — apex", cmd.split_whitespace().nth(1).unwrap_or(cmd)),
        Target::Chooser { .. } => "choose a session — apex".to_string(),
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(bounds),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(10.), px(8.))),
            }),
            // the title bar is ours: AppKit must not take a drag there as a
            // window move (a tab dragged reorders the tabs); the strip's
            // own handler moves the window (`start_window_move`)
            app_owns_titlebar_drag: true,
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| match target {
                Target::Local(files) => {
                    let (acme, mut rx) = Acme::new(cx, files);
                    cx.spawn(async move |this, cx| {
                        use futures::StreamExt;
                        while let Some(ev) = rx.next().await {
                            let r = this.update(cx, |acme: &mut Acme, cx| {
                                acme.pump(ev);
                                cx.notify();
                            });
                            if r.is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    acme
                }
                Target::Url { .. } | Target::Via { .. } | Target::Chooser { .. } => {
                    // the reader thread pokes this channel; the task polls
                    // the link on the UI thread
                    let (wake_tx, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
                    let wake: apex_server::remote::Wake = std::sync::Arc::new(move || {
                        let _ = wake_tx.unbounded_send(());
                    });
                    let acme = match target {
                        Target::Via { cmd, session, files } => match Acme::attach_via(cx, &cmd, &session, files, wake.clone()) {
                            Ok(a) => a,
                            Err(e) => {
                                eprintln!("apex-ui: attach via {cmd}: {e}");
                                std::process::exit(1);
                            }
                        },
                        Target::Url { url, files } if !url.is_local() => {
                            // a session elsewhere: the window opens now and
                            // says so; the attach (a binary to upload, a daemon
                            // to start there) runs on a thread and comes back
                            let mut a = offline_window(cx, &url, Vec::new(), wake.clone());
                            a.notice(&format!("{}: attaching…\n", url.describe()));
                            shell::log_line(&format!("attaching to {url} in the background"));
                            let (u, w) = (url.clone(), wake.clone());
                            let connecting = cx.background_executor().spawn(async move { Acme::connect_blocking(&u, w) });
                            cx.spawn_in(window, async move |this, cx| {
                                let r = connecting.await;
                                let _ = this.update_in(cx, |acme, window, cx| {
                                    match r {
                                        Ok((link, log, node, target)) => {
                                            if let Err(e) = acme.adopt(link, log, node, target, &url, files, window) {
                                                acme.notice(&Acme::connect_error(&url, &e));
                                            } else {
                                                shell::log_line(&format!("attached to {url}"));
                                            }
                                        }
                                        Err(e) => {
                                            shell::log_line(&format!("attach {url}: {e}"));
                                            acme.notice(&Acme::connect_error(&url, &e));
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                            a
                        }
                        Target::Url { url, files } => match Acme::attach(cx, &url, files.clone(), wake.clone()) {
                            Ok(a) => a,
                            Err(e) => offline(cx, &url, files, wake.clone(), &e),
                        },
                        Target::Chooser { url } => {
                            let mut a = offline_window(cx, &url, Vec::new(), wake.clone());
                            a.chooser = true;
                            a
                        }
                        Target::Local(_) => unreachable!(),
                    };
                    cx.spawn_in(window, async move |this, cx| {
                        use futures::StreamExt;
                        while wake_rx.next().await.is_some() {
                            let r = this.update_in(cx, |acme: &mut Acme, window, cx| {
                                if !acme.poll_remote() {
                                    eprintln!("apex-ui: server went away");
                                }
                                // a place in another session, from a tool or
                                // a Back: switched here, drawn or not
                                if let Some(loc) = acme.pending_switch.take() {
                                    acme.switch_for(loc, window, cx);
                                }
                                if acme.connected {
                                    pool::Pool::note_open(cx, &acme.url.clone());
                                }
                                acme.settle_snarf(cx);
                                if acme.leave_requested {
                                    acme.leave(window, cx);
                                }
                                if acme.close_requested {
                                    acme.close_now(cx);
                                }
                                cx.notify();
                            });
                            if r.is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    acme
                }
            });
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
            // the system's appearance, for the System theme: as it is now,
            // and as it changes
            theme::set_system_dark(matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark));
            window.observe_window_appearance(|window, cx| {
                theme::set_system_dark(matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark));
                cx.refresh_windows();
            }).detach();
            // cmd-` back into this window: the pointer where it was
            view.update(cx, |_, cx| {
                cx.observe_window_activation(window, |acme: &mut Acme, window, _| {
                    let active = window.is_window_active();
                    acme.window_activated(active, window);
                })
                .detach();
                // where the window is, remembered as it moves: after this
                // update, since save_open reads every window, this one too
                cx.observe_window_bounds(window, |_, _, cx| cx.defer(|cx| shell::save_open(cx))).detach();
            });
            view
        },
    );
    if let Ok(h) = &opened {
        // the red button too: the session stays, parked
        let h = *h;
        let _ = h.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, move |_, cx| {
                let _ = h.update(cx, |acme, _, cx| acme.park_into_pool(cx));
                true
            });
        });
    }
    match opened {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("apex-ui: open window: {e}");
            None
        }
    }
}

/// A window with nothing behind it when the local daemon cannot be
/// attached (another build, say): an in-process session showing the
/// error, pointed at `url` so Reconnect (⌘⇧R) tries again.
fn offline(cx: &mut gpui::Context<Acme>, url: &SessionUrl, files: Vec<String>, wake: apex_server::remote::Wake, e: &std::io::Error) -> Acme {
    eprintln!("apex-ui: attach {url}: {e}");
    let mut acme = offline_window(cx, url, files, wake);
    let msg = Acme::connect_error(url, e);
    acme.notice(&msg);
    acme
}

/// An in-process window pointed at `url` but attached to nothing, that a
/// Reconnect or the picker attaches later.
fn offline_window(cx: &mut gpui::Context<Acme>, url: &SessionUrl, files: Vec<String>, wake: apex_server::remote::Wake) -> Acme {
    let (mut acme, mut rx) = Acme::new(cx, files);
    cx.spawn(async move |this, cx| {
        use futures::StreamExt;
        while let Some(ev) = rx.next().await {
            let r = this.update(cx, |acme: &mut Acme, cx| {
                acme.pump(ev);
                cx.notify();
            });
            if r.is_err() {
                break;
            }
        }
    })
    .detach();
    acme.url = url.clone();
    acme.session = url.session.clone();
    acme.wake = Some(wake);
    acme.connected = false;
    // remembered like any window, on the session it is meant for
    acme.socket = Some(apex_server::daemon::default_socket());
    acme
}

/// menuhit's painting: the box, its border, the items centred, the
/// highlighted one in negative, and the scroll bar when there is one.
fn menu_element(m: &menu::Menu, font: i32) -> gpui::AnyElement {
    use gpui::{div, px, rgb};
    let r = m.menur;
    let mut el = div()
        .absolute()
        .left(px(r.x0 as f32))
        .top(px(r.y0 as f32))
        .w(px(r.dx() as f32))
        .h(px(r.dy() as f32))
        .bg(rgb(theme::theme().menu_bg))
        .border(px(menu::BLACKBORDER as f32))
        .border_color(rgb(theme::theme().menu_border));
    // children are placed relative to the menu's own origin
    for i in 0..m.nitemdrawn {
        let ir = m.item_rect(i);
        let text = m.items.get((i + m.off) as usize).cloned().unwrap_or_default();
        let hl = i == m.lasti;
        el = el.child(
            div()
                .absolute()
                .left(px((ir.x0 - r.x0 - menu::BLACKBORDER) as f32))
                .top(px((ir.y0 - r.y0 - menu::BLACKBORDER) as f32))
                .w(px(ir.dx() as f32))
                .h(px(ir.dy() as f32))
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(if hl { theme::theme().menu_hl } else { theme::theme().menu_bg }))
                .text_color(rgb(if hl { theme::theme().menu_hl_text } else { theme::theme().menu_text }))
                .font_family("Lucida Grande")
                .text_size(px(13.))
                .line_height(px(font as f32))
                .child(text),
        );
    }
    if m.scrolling {
        let sr = m.scrollr;
        let th = m.thumb();
        el = el.child(
            div()
                .absolute()
                .left(px((sr.x0 - r.x0 - menu::BLACKBORDER) as f32))
                .top(px((sr.y0 - r.y0 - menu::BLACKBORDER) as f32))
                .w(px(sr.dx() as f32))
                .h(px(sr.dy() as f32))
                .bg(rgb(theme::theme().menu_bg))
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px((th.y0 - sr.y0) as f32))
                        .w(px(sr.dx() as f32))
                        .h(px(th.dy() as f32))
                        .border(px(1.))
                        .border_color(rgb(theme::theme().menu_border))
                        .bg(rgb(theme::theme().menu_hl)),
                ),
        );
    }
    el.into_any_element()
}
