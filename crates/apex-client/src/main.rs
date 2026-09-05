//! apex: the UI client. Renders a session's state and turns mouse and keys
//! into log entries. `apex [files]` runs the server in-process;
//! `apex --attach [SOCKET] [files]` attaches to a running `apexd`.

mod app;
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
        root.child(row)
    }
}

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut attach: Option<std::path::PathBuf> = None;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--attach" => {
                attach = Some(match args.peek() {
                    Some(p) if p.ends_with(".sock") => std::path::PathBuf::from(args.next().unwrap()),
                    _ => apex_server::daemon::default_socket(),
                });
            }
            _ => files.push(a),
        }
    }
    let title = match &attach {
        Some(p) => format!("apex — {}", p.display()),
        None => "apex".to_string(),
    };
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some(title.clone().into()), ..Default::default() }),
                ..Default::default()
            },
            |window, cx| {
                let files = files.clone();
                let attach = attach.clone();
                let view = cx.new(|cx| match attach {
                    None => {
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
                    Some(socket) => {
                        // the reader thread pokes this channel; the task
                        // polls the link on the UI thread
                        let (wake_tx, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
                        let wake: apex_server::remote::Wake = std::sync::Arc::new(move || {
                            let _ = wake_tx.unbounded_send(());
                        });
                        let acme = match Acme::attach(cx, &socket, "main", files, wake) {
                            Ok(a) => a,
                            Err(e) => {
                                eprintln!("apex: attach {}: {e}", socket.display());
                                std::process::exit(1);
                            }
                        };
                        cx.spawn(async move |this, cx| {
                            use futures::StreamExt;
                            while wake_rx.next().await.is_some() {
                                let r = this.update(cx, |acme: &mut Acme, cx| {
                                    if !acme.poll_remote() {
                                        eprintln!("apex: server went away");
                                        cx.quit();
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
        )
        .expect("open window");
        cx.activate(true);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}
