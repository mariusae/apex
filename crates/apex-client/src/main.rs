//! apex-ui: the UI client. Renders a session's state and turns mouse and
//! keys into log entries.
//!
//! `apex-ui [files]` (what the app bundle runs) attaches to the local
//! daemon, starting it if needed, and reopens the sessions it had open
//! last time (else the first existing one, else a new `local`).
//! `--session S` picks one; `--attach [SOCKET]` names the daemon;
//! `--via CMD` attaches through CMD's stdin/stdout (the `apex attach
//! host/session` path: CMD is `ssh host apex attach --stdio`); `--local`
//! runs the server in-process, the old prototype's way.

mod app;
mod shell;
mod term_element;
mod text_element;

use gpui::{
    black, div, prelude::*, px, size, App, Application, Bounds, Context, MouseButton, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};

use apex_core::{Body, ViewId};

use app::Acme;
use term_element::TermElement;
use text_element::TextElement;

impl Render for Acme {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync();
        self.layouts.clear();
        self.term_layouts.clear();
        let me = cx.entity();

        let root = div()
            .id("apex")
            .size_full()
            .bg(black())
            .flex()
            .flex_col()
            .gap(px(2.))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &shell::Undo, window, cx| this.menu_edit("undo", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Redo, window, cx| this.menu_edit("redo", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Cut, window, cx| this.menu_edit("cut", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Copy, window, cx| this.menu_edit("copy", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Paste, window, cx| this.menu_edit("paste", window, cx)))
            .on_action(cx.listener(|this, _: &shell::SelectAll, window, cx| this.menu_edit("select-all", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Sessions, _, cx| this.open_selector(cx)))
            .on_action(cx.listener(|_, _: &shell::CloseWindow, window, _| window.remove_window()))
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
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .child(self.titlebar(cx))
            .child(TextElement { acme: me.clone(), view: ViewId::Top });

        let mut row = div().flex().flex_row().flex_1().min_h_0().gap(px(2.));
        let layout = self.node.state.layout.clone();
        for col in &layout.cols {
            let mut c = div()
                .flex()
                .flex_col()
                .min_w_0()
                .bg(gpui::white())
                .gap(px(2.))
                .child(TextElement { acme: me.clone(), view: ViewId::ColTag(col.id) });
            {
                let s = c.style();
                s.flex_grow = Some((col.weight as f32).max(0.01));
                s.flex_shrink = Some(1.);
                s.flex_basis = Some(px(0.).into());
            }
            for slot in &col.wins {
                let w = slot.window;
                let Ok(win) = self.node.state.window(w) else { continue };
                let mut wd = div().flex().flex_col().overflow_hidden().child(TextElement { acme: me.clone(), view: ViewId::Tag(w) });
                match win.body {
                    Body::Text(_) => wd = wd.child(TextElement { acme: me.clone(), view: ViewId::Body(w) }),
                    Body::Term(t) => wd = wd.child(TermElement { acme: me.clone(), window: w, term: t }),
                }
                {
                    let s = wd.style();
                    s.flex_grow = Some(slot.weight as f32);
                    s.flex_shrink = Some(1.);
                    s.flex_basis = Some(px(0.).into());
                }
                c = c.child(wd);
            }
            row = row.child(c);
        }
        let root = root.child(row);
        match self.selector_panel(cx) {
            Some(panel) => root.child(panel),
            None => root,
        }
    }
}


/// Where a window's session lives.
#[derive(Clone, Debug)]
enum Target {
    /// The server in this process, the old prototype's way.
    Local(Vec<String>),
    Socket { socket: std::path::PathBuf, session: String, files: Vec<String> },
    Via { cmd: String, session: String, files: Vec<String> },
}

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut local = false;
    let mut socket: Option<std::path::PathBuf> = None;
    let mut via: Option<String> = None;
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
            "--session" => session = Some(args.next().expect("--session NAME")),
            // Finder passes this when launching a bundle
            a if a.starts_with("-psn_") => {}
            _ => files.push(a),
        }
    }
    let socket = socket.unwrap_or_else(apex_server::daemon::default_socket);

    Application::new().run(move |cx: &mut App| {
        cx.set_menus(shell::menus());
        cx.bind_keys(shell::bindings());
        cx.on_action(|_: &shell::Quit, cx| {
            shell::QUITTING.store(true, std::sync::atomic::Ordering::Relaxed);
            cx.quit();
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
        {
            let socket = socket.clone();
            cx.on_action(move |_: &shell::NewWindow, cx| {
                // another window on the active window's session
                let session = cx
                    .active_window()
                    .and_then(|w| w.downcast::<Acme>())
                    .and_then(|h| h.read(cx).ok().map(|a| a.session.clone()))
                    .unwrap_or_else(|| "local".to_string());
                if shell::ensure_daemon(&socket).is_ok() {
                    open_window(cx, Target::Socket { socket: socket.clone(), session, files: Vec::new() });
                    shell::save_open(cx);
                }
            });
        }

        let targets: Vec<Target> = if local {
            vec![Target::Local(files.clone())]
        } else if let Some(cmd) = via.clone() {
            vec![Target::Via { cmd, session: session.clone().unwrap_or_else(|| "local".into()), files: files.clone() }]
        } else {
            if let Err(e) = shell::ensure_daemon(&socket) {
                eprintln!("apex-ui: {e}");
                std::process::exit(1);
            }
            let sessions = match &session {
                Some(s) => {
                    let _ = apex_server::remote::new_session(&socket, s);
                    vec![s.clone()]
                }
                None => shell::plan(&socket).unwrap_or_else(|e| {
                    eprintln!("apex-ui: {e}");
                    std::process::exit(1);
                }),
            };
            sessions.into_iter().map(|s| Target::Socket { socket: socket.clone(), session: s, files: files.clone() }).collect()
        };
        for t in targets {
            open_window(cx, t);
        }
        shell::save_open(cx);
        cx.activate(true);
        cx.on_window_closed(|cx| {
            shell::save_open(cx);
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}

fn open_window(cx: &mut App, target: Target) {
    let n = cx.windows().len() as f32;
    let mut bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
    bounds.origin.x += px(24. * n);
    bounds.origin.y += px(24. * n);
    let title = match &target {
        Target::Local(_) => "apex".to_string(),
        Target::Socket { session, .. } => format!("{session} — apex"),
        Target::Via { cmd, session, .. } => format!("{session} on {} — apex", cmd.split_whitespace().nth(1).unwrap_or(cmd)),
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(10.), px(8.))),
            }),
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
                Target::Socket { .. } | Target::Via { .. } => {
                    // the reader thread pokes this channel; the task polls
                    // the link on the UI thread
                    let (wake_tx, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
                    let wake: apex_server::remote::Wake = std::sync::Arc::new(move || {
                        let _ = wake_tx.unbounded_send(());
                    });
                    let (at, session, files) = match target {
                        Target::Socket { socket, session, files } => (app::Where::Socket(socket), session, files),
                        Target::Via { cmd, session, files } => (app::Where::Via(cmd), session, files),
                        Target::Local(_) => unreachable!(),
                    };
                    let acme = match Acme::attach(cx, &at, &session, files, wake) {
                        Ok(a) => a,
                        Err(e) => {
                            eprintln!("apex-ui: attach {session}: {e}");
                            std::process::exit(1);
                        }
                    };
                    cx.spawn(async move |this, cx| {
                        use futures::StreamExt;
                        while wake_rx.next().await.is_some() {
                            let r = this.update(cx, |acme: &mut Acme, cx| {
                                if !acme.poll_remote() {
                                    eprintln!("apex-ui: server went away");
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
            window.focus(&focus);
            view
        },
    );
    if let Err(e) = opened {
        eprintln!("apex-ui: open window: {e}");
    }
}
