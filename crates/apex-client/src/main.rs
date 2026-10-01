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
mod attention;
mod contrast;
mod cursor;
mod field;
mod fonts;
mod finder;
mod menu;
mod shelf;
mod shell;
mod sidebar;
mod commands;
mod completion;
mod cwdbar;
mod glide;
mod miniature;
mod restart;
mod strips;
mod switcher;
mod tagedit;
mod titlebar;
mod toasts;
mod webbar;
mod term_element;
mod pool;
mod procs;
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

/// A window's card inset in its space, across and down: the ground shows
/// between cards where acme drew its borders.
const CARD_X: i32 = 3;
const CARD_Y: i32 = 1;

impl Render for Acme {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // the overview's card gone to has grown to the window: go there
        self.overview_tick(window, cx);
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
        // where things are is this frame's to say: a page's scrollbar left
        // over from where its window was would take the clicks there --
        // on the box of the window that is there now, which sits in the
        // same strip at the column's edge
        // where ^F's list goes: under the name, as the last frame laid it
        // out (the layouts are drawn again below); not laid out yet, the
        // next frame's
        if self.completion_anchor() {
            cx.notify();
        }
        // and the path's picker: under what is typed in the tag
        if self.picker_anchor() {
            cx.notify();
        }
        self.layouts.clear();
        self.term_layouts.clear();
        self.web_bars.clear();
        self.shelf.preview_at.set(None);
        self.shelf.stack_at.set(None);
        let me = cx.entity();
        let font = f32::from(text_element::tag_line_height()) as i32;

        let t = theme::theme();
        // the overlays record where they land this frame; the last thing
        // laid out cuts the web views' holes to match (`Webs::set_holes`)
        self.overlay_bounds.borrow_mut().clear();
        self.toasts_at.borrow_mut().clear();
        let root = div()
            .id("apex")
            .size_full()
            // the interface's regular weight, as the font set asks for it
            // (H&Co's regular is not the weight called regular)
            .font_weight(fonts::weight(gpui::FontWeight::NORMAL))
            .bg(gpui::rgb(text_element::ground(&t)))
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
            .on_action(cx.listener(|this, _: &shell::NewTab, _, cx| this.open_selector(cx)))
            .on_action(cx.listener(|this, _: &shell::CloseTab, window, cx| this.close_current_session(window, cx)))
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
            .on_action(cx.listener(|this, _: &shell::PrevTab, window, cx| this.cycle_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &shell::NextTab, window, cx| this.cycle_tab(1, window, cx)))
            // the host's profile, the session's setup: opened, or made
            .on_action(cx.listener(|this, _: &shell::Profile, window, cx| this.menu_command("New ~/.apex/profile", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Goto, _, cx| this.open_finder(false, cx)))
            .on_action(cx.listener(|this, _: &shell::GotoAll, _, cx| this.open_finder(true, cx)))
            .on_action(cx.listener(|this, _: &shell::NextNotification, window, cx| this.next_notification(window, cx)))
            .on_action(cx.listener(|this, _: &shell::StashWindow, window, cx| this.stash_key(window, cx)))
            .on_action(cx.listener(|this, _: &shell::RestartServer, window, cx| this.restart_server_asked(window, cx)))
            .on_action(cx.listener(|this, _: &shell::Commands, _, cx| this.open_commands(cx)))
            .on_action(cx.listener(|this, _: &shell::ShowOverview, _, cx| this.toggle_overview(cx)))
            // a UI hack, on purpose: the keys just say the verbs, which a
            // tool answers
            .on_action(cx.listener(|this, _: &shell::NavBack, window, cx| this.menu_command("Back", window, cx)))
            .on_action(cx.listener(|this, _: &shell::NavFwd, window, cx| this.menu_command("Fwd", window, cx)))
            .on_action(cx.listener(|this, _: &shell::Reconnect, window, cx| {
                this.reconnect(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &shell::CloseWindow, window, cx| {
                // the session stays, parked: a window on it later is instant
                this.park_into_pool(cx);
                window.remove_window()
            }))
            .on_action(cx.listener(|_, _: &shell::ToggleFullScreen, window, _| window.toggle_fullscreen()))
            .on_key_down(cx.listener(Self::key_down))
            // a click off the toasts puts them away, wherever it lands (the
            // sidebar and the title bar keep their clicks to themselves)
            .capture_any_mouse_down(cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| this.toasts_click(e.position, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            // a force click is B3
            .on_mouse_pressure(cx.listener(Self::mouse_pressure))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            // a chord with the pointer off the window is the sweep's still
            .on_mouse_down_out(cx.listener(Self::mouse_down_out))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::mouse_up))
            // B4: the tools menu (a real fourth button, or shift-click)
            .on_mouse_down(MouseButton::Navigate(gpui::NavigationDirection::Back), cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Navigate(gpui::NavigationDirection::Back), cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Navigate(gpui::NavigationDirection::Back), cx.listener(Self::mouse_up))
            // B5, a mouse's forward button: Back
            .on_mouse_down(MouseButton::Navigate(gpui::NavigationDirection::Forward), cx.listener(Self::b5_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel));
        // full screen: acme's area is the whole screen, no title bar
        self.fullscreen = window.is_fullscreen();
        // full screen: AppKit's own title bar would slide down with the
        // menu bar, an empty bar over our strip; hidden while it lasts
        if self.native_bar_hidden != self.fullscreen {
            web::set_native_titlebar_hidden(window, self.fullscreen);
            self.native_bar_hidden = self.fullscreen;
        }
        // the sidebar, as Manifold's, down the left with the content beside
        // it, shown or not by the title bar's button (⌃⌘S)
        let side = self.sidebar_shown();
        // every host's sessions for it, asked in the background as it
        // shows, and again each minute it stays
        if side && self.sidebar_asked.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(60)) {
            self.sidebar_refresh(cx);
        }
        // the window's buttons, always on the title bar; every frame, as
        // they are looked at (cheaply): AppKit can make them anew behind
        // our back
        web::set_traffic_lights(window, true);
        // the title bar over all of it, acme's row and a pinned sidebar
        // under it
        let root = root.pt(px(title_h()));
        let root = if side { root.flex_row().child(self.sidebar(cx)) } else { root };
        // ctrl-tab: the session just gone to sliding in over the one it
        // replaced, which slides out beside it
        let vp = window.viewport_size();
        let area_left = self.left();
        let (slide_off, outgoing) = self.switch_slide(window, area_left, f32::from(vp.width) - area_left, f32::from(vp.height));
        // what fills the rest: across from a pinned sidebar, or all of it
        let rest = move |d: gpui::Div| if side { d.flex_1().min_w_0().h_full() } else { d.flex_1().min_h_0().w_full() };
        // a tab with nothing attached to it: no acme, just the page and
        // what the tab is waiting for in the middle of it. The window
        // still takes the keys that reach the other tabs, and the picker
        // still opens over it; the link goes on being made in the pool
        if let Some(what) = self.waiting.clone() {
            // a spinner over the words, as a Mac app waits
            let accent = text_element::rgb(t.accent);
            let spinner = canvas(|_, _, _| {}, move |b, _, window, _| text_element::paint_spinner(window, gpui::point(b.left() + b.size.width / 2., b.top() + b.size.height / 2.), 9., 2., accent)).w(px(24.)).h(px(24.));
            let blank = rest(div())
                .flex()
                .flex_col()
                .gap(px(10.))
                .items_center()
                .justify_center()
                .bg(gpui::rgb(t.body_bg))
                .child(spinner)
                .child(div().px(px(24.)).text_size(px(13.)).font_family(crate::fonts::ui()).text_color(gpui::rgb(t.text_dim)).child(what));
            let root = root.child(blank.relative().left(px(slide_off)));
            let root = root.child(self.title_bar(&me, cx));
            let root = match outgoing {
                Some(o) => root.child(o),
                None => root,
            };
            let root = match self.selector_panel(cx) {
                Some(panel) => root.child(panel),
                None => root,
            };
            // the overview (⌘⇧\), over everything
            let root = match self.overview_overlay(window, cx) {
                Some(o) => root.child(gpui::deferred(o).with_priority(3)),
                None => root,
            };
            let root = match self.session_preview(cx) {
                Some(p) => root.child(gpui::deferred(p).with_priority(3)),
                None => root,
            };
            return root.into_any_element();
        }

        // acme's tiling placed everything; draw each piece where it says
        // the tiling's layout, with whatever has just moved on its way
        let l = self.glided_layout();
        if self.glide.any() {
            window.request_animation_frame();
        }
        let at = |x: i32, y: i32, w: i32, h: i32, el: gpui::AnyElement| {
            div().absolute().left(px(x as f32)).top(px(y as f32)).w(px(w.max(0) as f32)).h(px(h.max(0) as f32)).overflow_hidden().child(el)
        };
        let fill = |x: f32, y: f32, w: f32, h: f32, c: u32| div().absolute().left(px(x)).top(px(y)).w(px(w.max(0.))).h(px(h.max(0.))).bg(gpui::rgb(c));
        // the system's pointers, as a Mac app's (the innermost hitbox's
        // style wins): the arrow over text as over everything else -- in
        // apex a click in text does far more than place an insertion
        // point, which is all the I-beam says; the open hand over what
        // drags (a window's handle, a column's box, the session's), the
        // closed hand everywhere while one is held; over a page, the
        // page's own
        use gpui::CursorStyle;
        let dragging = self.dragging_box();
        // the line between columns held: the pointer says left and right
        let held = if self.dragging_edge() { CursorStyle::ResizeLeftRight } else { CursorStyle::ClosedHand };
        // over what a ⌘- or ⌥-click would take: the link's hand
        let hinting = self.hint.is_some();
        let hold = |c: CursorStyle| if dragging { held } else if hinting { CursorStyle::PointingHand } else { c };
        let pointer = if dragging {
            held
        } else if hinting {
            CursorStyle::PointingHand
        } else if self.over_page(window) {
            cursor::NATIVE_CURSOR // the page's own, set as it asks
        } else {
            CursorStyle::Arrow
        };
        // a lane down a text's left: its handle (a row high) or its
        // scrollbar (all the way down), with its own pointer
        let lane = |h: Option<f32>, c: CursorStyle| {
            let d = div().absolute().left(px(0.)).top(px(0.)).w(px(crate::text_element::SCROLLWID)).cursor(c);
            match h {
                Some(h) => d.h(px(h)),
                None => d.h_full(),
            }
        };
        let mut area = rest(div().relative().left(px(slide_off))).overflow_hidden().cursor(pointer);
        // web windows drawn this frame keep their native views; the rest hide
        let mut webs_shown = std::collections::HashSet::new();
        // the ground the windows stand on, and the one the keys go to
        let ground = text_element::ground(&t);
        let key_window = self.key_window();
        let mut rings = Vec::new();
        for (ci, col) in l.cols.iter().enumerate() {
            // hidden behind a column grown to the whole row (B3 on its box)
            if !l.shows(ci) {
                continue;
            }
            // a strip (minimized, or stashed at the right): its box and
            // its windows' boxes down it, tags without text, bodies without
            // anything in them -- no text to wrap into a strip's width, no
            // terminal to shrink to one column, no page to squeeze
            let strip = apex_core::tiling::is_strip(col.r);
            // the column on the ground: its windows are cards on it, the
            // ground showing between them where acme drew black borders
            area = area.child(fill(col.r.x0 as f32, col.r.y0 as f32, col.r.dx() as f32, col.r.dy() as f32, ground));
            // a strip (`strips.rs`): stashed, the edges of sheets on their
            // sides, the whole of it the column's box; minimized, a slim
            // card where it stands, its windows' handles down it
            if strip {
                let el = if col.stashed { self.strip_element(ci, cx) } else { self.minimized_element(ci, cx) };
                area = area.child(at(col.r.x0, col.r.y0, col.r.dx(), col.r.dy(), el));
                continue;
            }
            area = area.child(at(col.r.x0, col.r.y0, col.r.dx(), font, TextElement { acme: me.clone(), view: ViewId::ColTag(col.id) }.into_any_element()).cursor(hold(CursorStyle::Arrow)).child(lane(Some(font as f32), hold(CursorStyle::OpenHand))));
            // a column with no windows: a faint word on what to do there,
            // where there is room for it (it takes no clicks: the ground's
            // buttons are acme's as ever)
            if col.wins.is_empty() && col.r.dx() >= EMPTY_W && col.r.dy() - font >= EMPTY_H {
                area = area.child(at(col.r.x0, col.r.y0 + font, col.r.dx(), col.r.dy() - font, empty_column(&t)));
            }
            for s in &col.wins {
                if col.hides(s.window) {
                    continue; // behind the window grown to the whole column
                }
                let w = s.window;
                let Ok(win) = self.node.state.window(w) else { continue };
                let tag_h = if s.body.dy() > 0 { s.body.y0 - s.r.y0 } else { s.r.dy() };
                // a card inset in the window's space, the ground round it:
                // the tag its top (and all of it when folded), the body
                // the rest
                let folded = s.body.dy() <= 0;
                let (tx, ty, tw) = (s.r.x0 + CARD_X, s.r.y0 + CARD_Y, s.r.dx() - 2 * CARD_X);
                let th = if folded { tag_h - 2 * CARD_Y } else { tag_h - CARD_Y };
                let (bx, bw, bh) = (s.body.x0 + CARD_X, s.body.dx() - 2 * CARD_X, s.body.dy() - CARD_Y);
                if win.body == Body::Web && !strip {
                    // a page's header: its handle, back and forward, and
                    // its address, in the tag's place
                    area = area.child(at(tx, ty, tw, th, self.web_header(w, th as f32, cx)));
                } else {
                    area = area.child(at(tx, ty, tw, th, TextElement { acme: me.clone(), view: ViewId::Tag(w) }.into_any_element()).cursor(hold(CursorStyle::Arrow)).child(lane(Some(font as f32), hold(CursorStyle::OpenHand))));
                }
                // the window the keys go to: a soft ring round its card,
                // drawn over it once every window is
                if key_window == Some(w) {
                    let ring = div().size_full().rounded(px(text_element::CARD_RADIUS)).border(px(1.5)).border_color(gpui::Hsla::from(gpui::rgb(t.accent)).opacity(0.55));
                    rings.push(at(tx, ty, tw, s.r.y1 - CARD_Y - ty, ring.into_any_element()));
                }
                if s.body.dy() > 0 && strip {
                    area = area.child(at(bx, s.body.y0, bw, bh, div().size_full().bg(gpui::rgb(t.body_bg)).into_any_element()));
                } else if s.body.dy() > 0 {
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
                            // acme's scrollbar, where a text window has it and
                            // as wide, drawn as it draws one: the page's own is
                            // hidden, and this one moves the page (WEB.md §2.2)
                            let me_bar = me.clone();
                            let (bar_bg, thumb) = (paper, t.body_border);
                            let sw = crate::text_element::SCROLLWID;
                            let bar = canvas(
                                move |bounds, _, cx| {
                                    me_bar.update(cx, |acme, _| {
                                        acme.web_bars.insert(w, bounds);
                                        let (t0, t1) = acme.webs.thumb(w);
                                        (t0, t1, acme.scroller(ViewId::Body(w), (t0 * 1e6) as u64))
                                    })
                                },
                                move |bounds, (t0, t1, (shows, fading)), window, _| {
                                    window.paint_quad(gpui::fill(bounds, gpui::rgb(bar_bg)));
                                    // an overlay scroller, as a text's is
                                    if shows > 0. {
                                        text_element::paint_scroller(window, bounds, t0 as f32, t1 as f32, text_element::rgb(thumb).opacity(shows));
                                    }
                                    if fading {
                                        window.request_animation_frame();
                                    }
                                },
                            )
                            .w(px(sw))
                            .h_full();
                            div()
                                .size_full()
                                .flex()
                                .flex_row()
                                .bg(gpui::rgb(paper))
                                // in by the key ring's width at the left and
                                // foot, which the native view would cover
                                .child(
                                    div().flex_1().h_full().pl(px(crate::web::PAGE_INSET)).pb(px(crate::web::PAGE_INSET)).cursor(cursor::NATIVE_CURSOR).child(
                                        canvas(
                                            move |bounds, window, cx| {
                                                me2.update(cx, |acme, _| acme.web_place(w, bounds, window));
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .size_full(),
                                    ),
                                )
                                // the scrollbar at its right, as a text's
                                .child(bar)
                                .into_any_element()
                        }
                    };
                    // the arrow over text and terminals, and down the
                    // scrollbar; a page keeps its own pointer
                    let body = if matches!(win.body, Body::Web | Body::Html(_)) {
                        let bar = div().absolute().right(px(0.)).top(px(0.)).w(px(crate::text_element::SCROLLWID)).h_full().cursor(hold(CursorStyle::Arrow));
                        at(bx, s.body.y0, bw, bh, body).child(bar)
                    } else if matches!(win.body, Body::Term(_)) {
                        at(bx, s.body.y0, bw, bh, body).cursor(hold(CursorStyle::Arrow))
                    } else {
                        at(bx, s.body.y0, bw, bh, body).cursor(hold(CursorStyle::Arrow)).child(lane(None, hold(CursorStyle::Arrow)))
                    };
                    area = area.child(body);
                }
            }
        }
        for r in rings {
            area = area.child(r);
        }
        // the lines between the columns: a drag of one makes the columns
        // on either side wider and narrower (the column's box moves it
        // too, and more)
        for (ci, col) in l.cols.iter().enumerate() {
            if ci == 0 || !l.shows(ci) || l.full.is_some() {
                continue;
            }
            let c = col.id;
            let edge = div().size_full().cursor(hold(CursorStyle::ResizeLeftRight)).on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| this.press_edge(c, e.position, cx)),
            );
            area = area.child(at(col.r.x0 - 6, col.r.y0, 7, col.r.dy(), edge.into_any_element()));
        }
        // where what is held would land: shaded, as Manifold shows where
        // a dragged sheet would go
        if let Some(r) = self.drag_preview() {
            let shade = div()
                .size_full()
                .rounded(px(6.))
                .bg(gpui::Hsla::from(gpui::rgb(t.accent)).opacity(0.12))
                .border_2()
                .border_color(gpui::Hsla::from(gpui::rgb(t.accent)).opacity(0.6));
            area = area.child(at(r.x0, r.y0, r.dx(), r.dy(), shade.into_any_element()));
        }
        // a strip under the pointer: its column, live, beside it
        if let Some(s) = self.strip_slice(&l, cx) {
            area = area.child(s);
        }
        // errors just written, by their columns' feet
        for toast in self.toasts_overlay(&l, cx) {
            area = area.child(toast);
        }
        // a blank page just made: its address to be typed, at once
        let blank = self.node.state.windows.iter().find(|(w, win)| win.body == Body::Web && !self.url_asked.contains(*w) && self.node.window_path(**w).is_empty()).map(|(w, _)| *w);
        if let Some(w) = blank {
            self.url_asked.insert(w);
            self.url_edit_start(w, cx);
        }
        if let Some(m) = &self.menu {
            area = area.child(menu_element(m, font, self.overlay_mark()));
        }
        let alive: std::collections::HashSet<apex_core::WindowId> = self.node.state.windows.keys().copied().collect();
        self.webs.settle(&webs_shown, |w| alive.contains(&w));
        let root = root.child(area);
        let root = root.child(self.title_bar(&me, cx));
        let root = match outgoing {
            Some(o) => root.child(o),
            None => root,
        };
        let root = match self.selector_panel(cx) {
            Some(panel) => root.child(panel),
            None => root,
        };
        let root = match self.finder_panel(cx) {
            Some(panel) => root.child(panel),
            None => root,
        };
        let root = match self.commands_panel(cx) {
            Some(panel) => root.child(panel),
            None => root,
        };
        // ^F's list under the caret
        let root = match self.completion_panel(cx) {
            Some(panel) => root.child(panel),
            None => root,
        };
        // a tag's path or label being typed, and the path's picker
        let root = root.children(self.tag_overlays(cx));
        // a process's card, under its pill
        let root = root.children(self.proc_card());
        // the title bar's crumbs' folders
        let root = root.children(self.cwd_panel(cx));
        // the overview (⌘⇧\), over everything
        let root = match self.overview_overlay(window, cx) {
            Some(o) => root.child(gpui::deferred(o).with_priority(3)),
            None => root,
        };
        // a session under the pointer in the sidebar: its window, live
        let root = match self.session_preview(cx) {
            Some(p) => root.child(gpui::deferred(p).with_priority(3)),
            None => root,
        };
        let holes = self.overlay_bounds.clone();
        let me3 = me.clone();
        let cutter = gpui::deferred(
            div().absolute().top(px(0.)).left(px(0.)).w(px(0.)).h(px(0.)).child(canvas(
                move |_, _, cx| {
                    let holes: Vec<(Bounds<gpui::Pixels>, gpui::Pixels)> = holes.borrow().clone();
                    me3.update(cx, |acme, _| {
                        acme.webs.set_holes(&holes);
                        // the pages go quiet with the rest while the
                        // picker or the finder has the window
                        acme.webs.set_veil((acme.selector.is_some() || acme.finder.is_some() || acme.commands.is_some()).then(shell::veil));
                    });
                },
                |_, _, _, _| {},
            )),
        )
        .with_priority(3);
        root.child(cutter).into_any_element()
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
        text_element::install_symbols();
        fonts::load();
        fonts::install(cx);
        pool::Pool::install(cx);
        theme::load();
        cx.set_menus(shell::menus());
        // the View menu: the theme, the choice marked in the menu
        cx.on_action(|_: &shell::ThemeLight, cx| shell::set_theme(theme::Mode::Light, cx));
        cx.on_action(|_: &shell::ThemeDark, cx| shell::set_theme(theme::Mode::Dark, cx));
        cx.on_action(|_: &shell::ThemeSystem, cx| shell::set_theme(theme::Mode::System, cx));
        cx.on_action(|_: &shell::ToggleSidebar, cx| shell::toggle_sidebar(cx));
        cx.on_action(|_: &shell::PaletteAlabaster, cx| shell::set_palette(theme::Palette::Alabaster, cx));
        cx.on_action(|_: &shell::PaletteXcode, cx| shell::set_palette(theme::Palette::Xcode, cx));
        cx.on_action(|_: &shell::PaletteClassic, cx| shell::set_palette(theme::Palette::Classic, cx));
        cx.on_action(|_: &shell::PaletteGitHub, cx| shell::set_palette(theme::Palette::GitHub, cx));
        cx.on_action(|_: &shell::PaletteNova, cx| shell::set_palette(theme::Palette::Nova, cx));
        cx.on_action(|_: &shell::PaletteRsms, cx| shell::set_palette(theme::Palette::Rsms, cx));
        cx.on_action(|_: &shell::FontSystem, cx| shell::set_fonts(fonts::Set::System, cx));
        cx.on_action(|_: &shell::FontClassic, cx| shell::set_fonts(fonts::Set::Classic, cx));
        cx.on_action(|_: &shell::FontGo, cx| shell::set_fonts(fonts::Set::Go, cx));
        cx.on_action(|_: &shell::FontMona, cx| shell::set_fonts(fonts::Set::Mona, cx));
        cx.on_action(|_: &shell::FontNova, cx| shell::set_fonts(fonts::Set::Nova, cx));
        cx.on_action(|_: &shell::FontHco, cx| shell::set_fonts(fonts::Set::Hco, cx));
        cx.on_action(|_: &shell::FontInter, cx| shell::set_fonts(fonts::Set::Inter, cx));
        cx.on_action(|_: &shell::FontGeist, cx| shell::set_fonts(fonts::Set::Geist, cx));
        cx.on_action(|_: &shell::FontStyrene, cx| shell::set_fonts(fonts::Set::Styrene, cx));
        cx.on_action(|_: &shell::FontBigger, cx| shell::resize_fonts(1, cx));
        cx.on_action(|_: &shell::FontSmaller, cx| shell::resize_fonts(-1, cx));
        cx.on_action(|_: &shell::FontActual, cx| shell::resize_fonts(0, cx));
        cx.on_action(|_: &shell::ToggleContrast, cx| shell::toggle_contrast(cx));
        cx.on_action(|_: &shell::ToggleBlink, cx| shell::toggle_blink(cx));
        cx.on_action(|_: &shell::ToggleSmoothCaret, cx| shell::toggle_smooth_caret(cx));
        cx.on_action(|_: &shell::ToggleLayoutAnimations, cx| shell::toggle_layout_animations(cx));
        cx.on_action(|_: &shell::CwdMarkDouble, cx| shell::set_cwd_mark(crate::theme::CwdMark::Double, cx));
        cx.on_action(|_: &shell::CwdMarkChip, cx| shell::set_cwd_mark(crate::theme::CwdMark::Chip, cx));
        cx.on_action(|_: &shell::CwdMarkBookmark, cx| shell::set_cwd_mark(crate::theme::CwdMark::Bookmark, cx));
        cx.on_action(|_: &shell::CwdMarkDotSlash, cx| shell::set_cwd_mark(crate::theme::CwdMark::DotSlash, cx));
        cx.on_action(|_: &shell::CwdMarkBolt, cx| shell::set_cwd_mark(crate::theme::CwdMark::Bolt, cx));
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
        let default = || session.clone().unwrap_or_else(|| apex_server::providers::DEFAULT_SESSION.to_string());
        // apex has one window; everything else it has open is a tab in it
        let mut stale: Option<String> = None;
        let (target, frame): (Target, Option<WindowBounds>) = if local {
            (Target::Local(files.clone()), None)
        } else if let Some(cmd) = via.clone() {
            (Target::Via { cmd, session: default(), files: files.clone() }, None)
        } else if let Some(u) = url.clone() {
            match SessionUrl::parse(&u) {
                Some(url) => (Target::Url { url, files: files.clone() }, None),
                None => {
                    eprintln!("apex-ui: bad session URL {u:?}");
                    std::process::exit(2);
                }
            }
        } else if let Some(dest) = remote.clone() {
            let d = apex_server::providers::Dest::parse(&dest);
            (Target::Url { url: SessionUrl { provider: d.provider, arg: d.name, session: default(), id: None }, files: files.clone() }, None)
        } else {
            match shell::ensure_daemon(&socket) {
                Ok(()) => {}
                // a daemon of another version: the window opens, says so
                // and offers to restart it
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                    shell::log_line(&format!("the daemon: {e}"));
                    stale = Some(e.to_string());
                }
                Err(e) => {
                    eprintln!("apex-ui: {e}");
                    std::process::exit(1);
                }
            }
            match &session {
                Some(s) => (Target::Url { url: SessionUrl::local(s), files: files.clone() }, None),
                None => {
                    let (url, frame) = shell::plan(&socket).unwrap_or_else(|e| {
                        eprintln!("apex-ui: {e}");
                        std::process::exit(1);
                    });
                    (Target::Url { url, files: files.clone() }, frame)
                }
            }
        };
        // a session elsewhere is attached once its window is open, so the
        // link that lands has a window to land in
        let start = match &target {
            Target::Url { url, files } if !url.is_local() => Some(files.clone()),
            _ => None,
        };
        let opened = open_window(cx, target, frame);
        if let (Some(why), Some(h)) = (stale.take(), opened) {
            let _ = h.update(cx, |acme, window, cx| acme.offer_restart(&why, window, cx));
        }
        if let Some(files) = start {
            if let Some(tab) = opened.and_then(|h| h.read(cx).ok().map(|a| a.tab)) {
                pool::Pool::start(cx, tab, pool::Why::Attaching, false, files);
            }
        }
        shell::save_open(cx);
        // the tabs of last time, attached again in the background and parked
        pool::Pool::restore(cx);
        // the pointer goes while text is typed, and only then: not for a
        // key that does something (gpui's default), which a tab switch is
        cx.set_cursor_hide_mode(gpui::CursorHideMode::OnTyping);
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

/// The window. apex has one, on a session, with the rest of what it has
/// open as tabs in it.
fn open_window(cx: &mut App, target: Target, frame: Option<WindowBounds>) -> Option<gpui::WindowHandle<Acme>> {
    let bounds = frame.unwrap_or_else(|| WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(760.)), cx)));
    let title = match &target {
        Target::Local(_) => "apex".to_string(),
        Target::Url { url, .. } => Acme::title(url),
        Target::Via { cmd, session, .. } => format!("{session} via {} — apex", cmd.split_whitespace().nth(1).unwrap_or(cmd)),
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(bounds),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: true,
                // the lights on the bar's centre line: their own height is
                // 13 as AppKit draws them, so the room above is what is
                // left of the bar (measured on screen, not by the book)
                // on the sidebar card's top row, as Manifold puts them: the
                // close button's middle 20 in from the card's edge and half
                // the row down (the card is 6 in from the window's)
                traffic_light_position: Some(gpui::point(px(14.), px(main_lights_y()))),
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
                    let (mut acme, mut rx) = Acme::new(cx, files);
                    acme.tab = pool::Pool::open(cx, &acme.url.clone());
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
                Target::Url { .. } | Target::Via { .. } => {
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
                        Target::Url { url, .. } if !url.is_local() => {
                            // a session elsewhere: the window opens now on a
                            // blank page and says so, and the pool makes the
                            // link (a binary to upload, a daemon to start
                            // there) as it makes every other -- `start`, once
                            // this window is open and can be found waiting
                            let mut a = offline_window(cx, &url, Vec::new(), wake.clone());
                            a.wait(&pool::Why::Attaching.sentence(&url));
                            a
                        }
                        Target::Url { url, files } => match Acme::attach(cx, &url, files.clone(), wake.clone()) {
                            Ok(a) => a,
                            Err(e) => offline(cx, &url, files, wake.clone(), &e),
                        },
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
                                    pool::Pool::note_open(cx, acme.tab, &acme.url.clone());
                                    // settled here, unless ctrl-tab is passing through
                                    if acme.switcher.is_none() {
                                        pool::Pool::note_settled(cx, acme.tab);
                                    }
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
            // (the window's link was made before the appearance was known,
            // and told light's colours: told again now)
            let dark = matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
            theme::set_system_dark(dark);
            if theme::mode() == theme::Mode::System && dark {
                shell::apply_theme(cx);
            }
            window.observe_window_appearance(|window, cx| {
                theme::set_system_dark(matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark));
                shell::apply_theme(cx);
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
    acme.wait_failed(msg.trim_end());
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
    // its tab, which is the app's name for it however the attach goes
    acme.tab = pool::Pool::open(cx, url);
    acme.wake = Some(wake);
    acme.connected = false;
    // remembered like any window, on the session it is meant for
    acme.socket = Some(apex_server::daemon::default_socket());
    acme
}

/// How far down the window's buttons stand on the title bar.
pub(crate) fn main_lights_y() -> f32 {
    // AppKit's buttons are 16 high in their frames (the circle in the
    // middle of it): centred on the bar, as the sidebar's button and the
    // top tag are
    (title_h() - 16.) / 2.
}

/// The title bar's height: a Mac title bar's with a toolbar's air, or the
/// top row's line and the border under it where that is taller. acme's
/// area starts below it (`Acme::top`); the window's buttons, the
/// sidebar's and the top tag are centred on it.
pub(crate) fn title_h() -> f32 {
    (f32::from(text_element::tag_line_height()) + apex_core::tiling::BORDER as f32).max(38.)
}

/// The least room an empty column shows its hint in.
const EMPTY_W: i32 = 200;
const EMPTY_H: i32 = 120;

/// An empty column's hint, in the middle of it: the ways to put something
/// there, each key (or command) and what it does, faint -- in the
/// interface's face, as the sidebar's, not the text's.
fn empty_column(t: &theme::Theme) -> gpui::AnyElement {
    use gpui::{div, prelude::*, px};
    let ink = gpui::Hsla::from(gpui::rgb(t.text_dim)).opacity(0.75);
    let faint = gpui::Hsla::from(gpui::rgb(t.text_dim)).opacity(0.55);
    let row = |key: &'static str, what: &'static str| {
        div()
            .flex()
            .flex_row()
            .gap(px(10.))
            .child(div().w(px(84.)).flex_none().text_right().font_weight(crate::fonts::weight(gpui::FontWeight::MEDIUM)).text_color(ink).child(key))
            .child(div().flex_none().text_color(faint).child(what))
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .font_family(crate::fonts::ui())
                .text_size(px(12.5))
                .child(row("⌘P", "go to a file"))
                .child(row("⌘N", "a new window"))
                .child(row("B2 Newterm", "a shell"))
                .child(row("⌘⇧P", "every command")),
        )
        .into_any_element()
}

/// Where the window's buttons end across the title bar, with air before
/// the sidebar's button after them.
const LIGHTS_W: f32 = 86.;

/// The tools menu painted as a Mac context menu: the card rounded and
/// lifted, a hairline round it; each row in the system font, the
/// highlighted one an accent pill in from the sides, the remembered one
/// checked; and the scrolling lane's thumb a slim scroller's.
fn menu_element(m: &menu::Menu, _font: i32, mark: gpui::AnyElement) -> gpui::AnyElement {
    use gpui::{div, px, rgb};
    let t = theme::theme();
    let r = m.menur;
    // children are placed from the menu's corner, inside its hairline
    const EDGE: i32 = 1;
    let mut el = div()
        .absolute()
        .left(px(r.x0 as f32))
        .top(px(r.y0 as f32))
        .w(px(r.dx() as f32))
        .h(px(r.dy() as f32))
        .bg(rgb(t.menu_bg))
        .border(px(EDGE as f32))
        .border_color(rgb(t.menu_border))
        .rounded(px(menu::RADIUS))
        .shadow_lg()
        .font_family(crate::fonts::ui())
        .child(mark);
    for i in 0..m.nitemdrawn {
        let ir = m.item_rect(i);
        let at = (i + m.off) as usize;
        let text = m.items.get(at).cloned().unwrap_or_default();
        let hl = i == m.lasti;
        let ink = if hl { t.menu_hl_text } else { t.menu_text };
        let mut row = div()
            .absolute()
            .left(px((ir.x0 - r.x0 - EDGE) as f32))
            .top(px((ir.y0 - r.y0 - EDGE) as f32))
            .w(px(ir.dx() as f32))
            .h(px(ir.dy() as f32))
            .rounded(px(menu::ROW_RADIUS))
            .flex()
            .items_center()
            .pl(px((menu::LEAD - menu::INSET) as f32))
            .text_size(px(13.))
            .text_color(rgb(ink))
            .when(hl, |d| d.bg(rgb(t.menu_hl)))
            .child(text);
        if m.checked == Some(at) {
            row = row.child(div().absolute().left(px(5.)).top(px(0.)).h_full().flex().items_center().text_size(px(12.)).text_color(rgb(ink)).child("✓"));
        }
        el = el.child(row);
    }
    if m.scrolling {
        let sr = m.scrollr;
        let th = m.thumb();
        el = el.child(
            div()
                .absolute()
                .left(px((sr.x0 - r.x0 - EDGE) as f32 + 3.5))
                .top(px((th.y0 - r.y0 - EDGE) as f32))
                .w(px(5.))
                .h(px(th.dy() as f32))
                .rounded(px(2.5))
                .bg(rgb(t.body_border)),
        );
    }
    el.into_any_element()
}

impl app::Acme {
    /// The title bar, as a Mac app's: the window's buttons (AppKit's, put
    /// there), the sidebar's button, a divider, and the top row -- acme's
    /// top tag, as editable as ever. Its bare parts move the window, and
    /// a double click there zooms it, as a title bar's do; past the top
    /// tag's text the tag's own click does (`mouse_down`).
    fn title_bar(&self, me: &gpui::Entity<app::Acme>, cx: &mut gpui::Context<Self>) -> gpui::AnyElement {
        use gpui::{div, prelude::*, px, MouseButton};
        let t = theme::theme();
        let h = title_h();
        let font = f32::from(text_element::tag_line_height());
        let bare = |id: &'static str| {
            div().id(id).flex_none().h_full().on_mouse_down(MouseButton::Left, |e: &gpui::MouseDownEvent, window, cx| {
                if e.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
                cx.stop_propagation();
            })
        };
        let side = self.sidebar_shown();
        // on the sidebar's card (its top, joined to the rest of it below
        // the bar) or on the bar's ground
        let under = if side { t.strip } else { text_element::ground(&t) };
        let toggle = div()
            .id("title-sidebar")
            .flex_none()
            .size(px(24.))
            .rounded(px(5.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .hover(move |s| s.bg(gpui::rgb(theme::step(under, 1))))
            .child(sidebar::sidebar_glyph(t.text_dim))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                shell::toggle_sidebar(cx);
                cx.stop_propagation();
            });
        // the window's buttons' room; in full screen AppKit takes them
        // away, and the rest moves up to the edge
        let lights = if self.fullscreen { 8. } else { LIGHTS_W };
        // the session: its name (a click to rename it), the chevron for
        // the others, a mark when one of them wants the user -- not while
        // the sidebar is out, which says all that; then acme's top row,
        // and the stash's room at the right end
        let rest = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .border_b(px(0.5))
            .border_color(gpui::rgb(t.body_border))
            .when(!side, |d| d.child(bare("title-lights").w(px(lights))).child(toggle).child(bare("title-gap0").w(px(6.))))
            // the sidebar out: the top row flush with it, as the columns
            // under it are, its square over their grips
            .when(!side, |d| {
                d.child(self.session_title(h, cx))
                    .child(bare("title-gap").w(px(10.)))
                    .child(div().flex_none().w(px(1.)).h(px(16.)).bg(gpui::rgb(t.body_border)))
                    .child(bare("title-gap2").w(px(8.)))
            })
            // the session's host and directory, the crumbs to change it by
            .children(self.cwd_bar(cx).map(|b| div().flex_shrink(1.).min_w(px(0.)).max_w(gpui::relative(0.45)).flex().flex_row().items_center().child(b).child(bare("title-gap3").w(px(3.)))))
            .child(div().flex_1().min_w_0().h(px(font)).relative().child(text_element::TextElement { acme: me.clone(), view: apex_core::ViewId::Top }).cursor(gpui::CursorStyle::Arrow))
            .child(bare("title-shelf").w(px(self.shelf_room(cx))));
        // the sidebar shown: its card goes up round the window's buttons
        // and its own, one with them as a Mac app's sidebar is; the bar
        // is the rest's
        let card_top = side.then(|| {
            let inset = sidebar::INSET;
            let toggle = div()
                .id("title-sidebar-card")
                .flex_none()
                .size(px(24.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_default()
                .hover(move |s| s.bg(gpui::rgb(theme::step(under, 1))))
                .child(sidebar::sidebar_glyph(t.text_dim))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    shell::toggle_sidebar(cx);
                    cx.stop_propagation();
                });
            div()
                .flex_none()
                .relative()
                .w(px(shell::SIDEBAR_W))
                .h_full()
                .child(
                    div()
                        .absolute()
                        .left(px(inset))
                        .top(px(inset))
                        .w(px(shell::SIDEBAR_W - 2. * inset))
                        .h(px(h - inset))
                        .rounded_t(px(10.))
                        .bg(gpui::rgb(t.strip))
                        .border_t_1()
                        .border_l_1()
                        .border_r_1()
                        .border_color(gpui::rgb(t.border))
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(bare("title-lights").w(px(lights - inset)))
                        .child(toggle)
                        .child(bare("title-card-rest").flex_1()),
                )
                // the ground round the card's top, a title bar's too
                .child(bare("title-card-edge").absolute().top(px(0.)).left(px(0.)).w_full().h(px(inset)))
        });
        div()
            .id("title-bar")
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .w_full()
            .h(px(h))
            .flex()
            .flex_row()
            .items_center()
            .bg(gpui::rgb(text_element::ground(&t)))
            .children(card_top)
            .child(rest)
            .children(self.shelf(h, self.node.state.layout.r.dx() as f32 + self.left(), cx))
            .into_any_element()
    }
}
