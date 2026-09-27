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
mod shell;
mod sidebar;
mod miniature;
mod restart;
mod switcher;
mod webbar;
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
        // where things are is this frame's to say: a page's scrollbar left
        // over from where its window was would take the clicks there --
        // on the box of the window that is there now, which sits in the
        // same strip at the column's edge
        self.layouts.clear();
        self.term_layouts.clear();
        self.web_bars.clear();
        let me = cx.entity();
        let font = f32::from(text_element::font_for(false).line_height) as i32;

        let t = theme::theme();
        // the overlays record where they land this frame; the last thing
        // laid out cuts the web views' holes to match (`Webs::set_holes`)
        self.overlay_bounds.borrow_mut().clear();
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
            .on_action(cx.listener(|this, _: &shell::StashNext, _, cx| this.stash_walk_step(false, cx)))
            .on_action(cx.listener(|this, _: &shell::RestartServer, window, cx| this.restart_server_asked(window, cx)))
            .on_action(cx.listener(|this, _: &shell::StashBack, _, cx| this.stash_walk_step(true, cx)))
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
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
        // the sidebar, as Manifold's: pinned, down the left with the
        // content beside it; else floating over the content when the
        // pointer brings it, sliding in and out. The window's buttons are
        // on its top row and show only with it; there is no title bar, so
        // full screen is the whole screen
        let side = self.sidebar_shown();
        let slide = if side { None } else { self.sidebar_slide() };
        let lights = side || self.sidebar_out;
        if self.lights_shown != Some(lights) {
            web::set_traffic_lights(window, lights);
            self.lights_shown = Some(lights);
        }
        if slide.is_some_and(|t| t < 1.) {
            window.request_animation_frame();
        }
        let root = if side { root.flex_row().child(self.sidebar(false, cx)) } else { root };
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
            let root = root.child(blank);
            let root = match self.selector_panel(cx) {
                Some(panel) => root.child(panel),
                None => root,
            };
            // ctrl-tab's cards, over everything
            let root = match self.switcher_overlay(window, cx) {
                Some(o) => root.child(gpui::deferred(o).with_priority(3)),
                None => root,
            };
            let root = match slide {
                Some(t) => root.child(floating_sidebar(self.sidebar(true, cx), t)),
                None => root,
            };
            return root.into_any_element();
        }

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
        let hold = |c: CursorStyle| if dragging { held } else { c };
        let pointer = if dragging {
            held
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
        let mut area = rest(div().relative()).overflow_hidden().cursor(pointer);
        // web windows drawn this frame keep their native views; the rest hide
        let mut webs_shown = std::collections::HashSet::new();
        area = area.child(at(l.r.x0, l.r.y0, l.r.dx(), font, TextElement { acme: me.clone(), view: ViewId::Top }.into_any_element()).cursor(hold(CursorStyle::Arrow)).child(lane(Some(font as f32), hold(CursorStyle::OpenHand))));
        for (ci, col) in l.cols.iter().enumerate() {
            // hidden behind a column grown to the whole row (B3 on its box)
            if !l.shows(ci) {
                continue;
            }
            // squeezed to a strip (B2 on another column's box): its box and
            // its windows' boxes down it, tags without text, bodies without
            // anything in them -- no text to wrap into a strip's width, no
            // terminal to shrink to one column, no page to squeeze
            let strip = apex_core::tiling::is_strip(col.r);
            // acme's colinit: the column is white where no window is, the
            // tail below its last window (or its tag and the border under
            // it); everywhere else the black root is the borders between
            // the tag and the windows and between the windows
            let tail = col.wins.last().map(|s| s.r.y1).unwrap_or(col.r.y0 + font + apex_core::tiling::BORDER);
            // the column on the body's paper, as acme's is on white: what
            // the windows leave -- a body's last part line, the gaps
            // between them -- is paper, not the rule's grey; a hairline
            // where each window meets the one above it says where it
            // starts, folded to its tag or not
            area = area.child(fill(col.r.x0 as f32, col.r.y0 as f32, col.r.dx() as f32, col.r.dy() as f32, t.body_bg));
            for s in col.wins.iter() {
                let b = apex_core::tiling::BORDER as f32;
                let hair = 1. / scale;
                area = area.child(fill(s.r.x0 as f32, s.r.y0 as f32 - b / 2. - hair / 2., s.r.dx() as f32, hair, t.border));
            }
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
            area = area.child(at(col.r.x0, col.r.y0, col.r.dx(), font, TextElement { acme: me.clone(), view: ViewId::ColTag(col.id) }.into_any_element()).cursor(hold(CursorStyle::Arrow)).child(lane(Some(font as f32), hold(CursorStyle::OpenHand))));
            // the stash: the edges of the sheets put away, peeking out
            // under the column's windows as a stack of paper does, each
            // further one narrower and lower; in the accent's tint when
            // one of them wants the user
            if let Some((band, _)) = self.stash_geometry(ci) {
                let n = col.stash.len().min(apex_core::tiling::STASH_EDGES);
                let notified = col.stash.iter().any(|s| self.window_notified(s.slot.window));
                let edge = if notified { text_element::mix(t.tag_bg, t.accent, 0.10) } else { t.tag_bg };
                let (paper, line) = (t.body_bg, t.body_border);
                let sheets = canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        window.paint_quad(gpui::fill(b, gpui::rgb(paper)));
                        let top = b.top() + px(apex_core::tiling::BORDER as f32);
                        for i in (0..n).rev() {
                            let inset = px(2. + 4. * i as f32);
                            let bottom = top + px((apex_core::tiling::STASH_EDGE * (i as i32 + 1)) as f32);
                            let r = gpui::Bounds::new(gpui::point(b.left() + inset, top - px(4.)), gpui::size(b.size.width - inset * 2., bottom - top + px(4.)));
                            let radii = gpui::Corners { top_left: px(0.), top_right: px(0.), bottom_left: px(5.), bottom_right: px(5.) };
                            window.paint_quad(gpui::quad(r, radii, gpui::rgb(edge), gpui::Edges { top: px(0.), left: px(1.), right: px(1.), bottom: px(1.) }, gpui::rgb(line), gpui::BorderStyle::Solid));
                        }
                    },
                )
                .size_full();
                area = area.child(at(band.x0, band.y0, band.dx(), band.dy(), sheets.into_any_element()).cursor(hold(CursorStyle::Arrow)));
            }
            for (i, s) in col.wins.iter().enumerate() {
                if !col.safe && i > 0 {
                    continue; // obscured by the full-column window
                }
                let w = s.window;
                let Ok(win) = self.node.state.window(w) else { continue };
                let tag_h = if s.body.dy() > 0 { s.body.y0 - s.r.y0 } else { s.r.dy() };
                if win.body == Body::Web && !strip {
                    // a page's header: its handle, back and forward, and
                    // its address, in the tag's place
                    area = area.child(at(s.r.x0, s.r.y0, s.r.dx(), tag_h, self.web_header(w, tag_h as f32, cx)));
                } else {
                    area = area.child(at(s.r.x0, s.r.y0, s.r.dx(), tag_h, TextElement { acme: me.clone(), view: ViewId::Tag(w) }.into_any_element()).cursor(hold(CursorStyle::Arrow)).child(lane(Some(font as f32), hold(CursorStyle::OpenHand))));
                }
                if s.body.dy() > 0 && strip {
                    area = area.child(at(s.body.x0, s.body.y0, s.body.dx(), s.body.dy(), div().size_full().bg(gpui::rgb(t.body_bg)).into_any_element()));
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
                                        acme.webs.thumb(w)
                                    })
                                },
                                move |bounds, (t0, t1), window, _| {
                                    window.paint_quad(gpui::fill(bounds, gpui::rgb(bar_bg)));
                                    text_element::paint_scroller(window, bounds, t0 as f32, t1 as f32, text_element::rgb(thumb));
                                },
                            )
                            .w(px(sw))
                            .h_full();
                            div()
                                .size_full()
                                .flex()
                                .flex_row()
                                .bg(gpui::rgb(paper))
                                .child(bar)
                                .child(
                                    div().flex_1().h_full().cursor(cursor::NATIVE_CURSOR).child(
                                        canvas(
                                            move |bounds, window, cx| {
                                                me2.update(cx, |acme, _| acme.web_place(w, bounds, window));
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .size_full(),
                                    ),
                                )
                                .into_any_element()
                        }
                    };
                    // the arrow over text and terminals, and down the
                    // scrollbar; a page keeps its own pointer
                    let body = if matches!(win.body, Body::Web | Body::Html(_)) {
                        at(s.body.x0, s.body.y0, s.body.dx(), s.body.dy(), body).child(lane(None, hold(CursorStyle::Arrow)))
                    } else if matches!(win.body, Body::Term(_)) {
                        at(s.body.x0, s.body.y0, s.body.dx(), s.body.dy(), body).cursor(hold(CursorStyle::Arrow))
                    } else {
                        at(s.body.x0, s.body.y0, s.body.dx(), s.body.dy(), body).cursor(hold(CursorStyle::Arrow)).child(lane(None, hold(CursorStyle::Arrow)))
                    };
                    area = area.child(body);
                }
            }
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
        // the stash brought out: its tags, live, stacked over the column's
        // foot as sheets drawn out of the pile, each with its handle
        // (B1 back where it was, B2 back alone, a drag back where it is
        // let go) and its text (B2 runs Del or Put there as anywhere)
        if let Some(ci) = self.stash_open.and_then(|c| l.column_index(c)) {
            if let Some((band, rows)) = self.stash_geometry(ci) {
                let top = rows.first().map(|(_, r)| r.y0).unwrap_or(band.y0) - 6;
                let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.18), offset: gpui::point(px(0.), px(-2.)), blur_radius: px(12.), spread_radius: px(0.), inset: false };
                let card = div().size_full().bg(gpui::rgb(t.body_bg)).rounded_t(px(9.)).shadow(vec![shadow]).child(self.overlay_mark());
                area = area.child(at(band.x0, top, band.dx(), band.y1 - top, card.into_any_element()).cursor(hold(CursorStyle::Arrow)));
                for (w, r) in rows {
                    area = area.child(at(r.x0, r.y0, r.dx(), r.dy(), TextElement { acme: me.clone(), view: ViewId::Tag(w) }.into_any_element()).cursor(hold(CursorStyle::Arrow)).child(lane(Some(font as f32), hold(CursorStyle::OpenHand))));
                }
            }
        }
        // ⌘E's cards, over their column
        // a blank page just made: its address to be typed, at once
        let blank = self.node.state.windows.iter().find(|(w, win)| win.body == Body::Web && !self.url_asked.contains(*w) && self.node.window_name(**w).is_empty()).map(|(w, _)| *w);
        if let Some(w) = blank {
            self.url_asked.insert(w);
            self.url_edit_start(w, cx);
        }
        if self.stash_walk.as_ref().is_some_and(|s| s.done()) {
            self.stash_walk = None;
        }
        if let Some(o) = self.stash_walk_overlay(&l, window, cx) {
            area = area.child(o);
        }
        if let Some(m) = &self.menu {
            area = area.child(menu_element(m, font, self.overlay_mark()));
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
        // ctrl-tab's cards, over everything
        let root = match self.switcher_overlay(window, cx) {
            Some(o) => root.child(gpui::deferred(o).with_priority(3)),
            None => root,
        };
        let root = match slide {
            Some(t) => root.child(floating_sidebar(self.sidebar(true, cx), t)),
            None => root,
        };
        let holes = self.overlay_bounds.clone();
        let me3 = me.clone();
        let cutter = gpui::deferred(
            div().absolute().top(px(0.)).left(px(0.)).w(px(0.)).h(px(0.)).child(canvas(
                move |_, _, cx| {
                    let holes: Vec<Bounds<gpui::Pixels>> = holes.borrow().clone();
                    me3.update(cx, |acme, _| {
                        acme.webs.set_holes(&holes);
                        // the pages go quiet with the rest while the
                        // picker or the finder has the window
                        acme.webs.set_veil((acme.selector.is_some() || acme.finder.is_some()).then(shell::veil));
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
        cx.on_action(|_: &shell::PaletteSystem, cx| shell::set_palette(theme::Palette::System, cx));
        cx.on_action(|_: &shell::PaletteClassic, cx| shell::set_palette(theme::Palette::Classic, cx));
        cx.on_action(|_: &shell::PaletteGitHub, cx| shell::set_palette(theme::Palette::GitHub, cx));
        cx.on_action(|_: &shell::PaletteNova, cx| shell::set_palette(theme::Palette::Nova, cx));
        cx.on_action(|_: &shell::FontSystem, cx| shell::set_fonts(fonts::Set::System, cx));
        cx.on_action(|_: &shell::FontClassic, cx| shell::set_fonts(fonts::Set::Classic, cx));
        cx.on_action(|_: &shell::FontGo, cx| shell::set_fonts(fonts::Set::Go, cx));
        cx.on_action(|_: &shell::FontMona, cx| shell::set_fonts(fonts::Set::Mona, cx));
        cx.on_action(|_: &shell::FontNova, cx| shell::set_fonts(fonts::Set::Nova, cx));
        cx.on_action(|_: &shell::ToggleContrast, cx| shell::toggle_contrast(cx));
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
                traffic_light_position: Some(gpui::point(px(19.), px(6. + shell::SIDEBAR_HEADER / 2. - 6.5))),
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

/// The floating sidebar over the content, `t` of the way in: from 24
/// pixels to the left and faded, as Manifold's slides.
fn floating_sidebar(sidebar: impl IntoElement, t: f32) -> impl IntoElement {
    use gpui::{div, px};
    gpui::deferred(div().absolute().top(px(0.)).bottom(px(0.)).left(px(-24. * (1. - t))).opacity(t).child(sidebar)).with_priority(1)
}

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
