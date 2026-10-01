//! Errors as toasts. A command's errors go to its directory's errors
//! window, as ever -- the core's, and every client's -- but that window
//! is not opened over the work: it goes to the stash, and what was just
//! written shows in a toast at the app's lower right, over whatever
//! columns are there, with Show All (the window shown in the
//! stash's preview) and ×. A toast goes by
//! itself after a while unless the pointer is on it, and at once on a
//! click anywhere but on a toast. Its words answer as
//! the window's would: B3 on one looks (plumbs a file:line, say), B2 runs
//! it, both from the errors window; B1 anywhere on a toast is its
//! Show All. The text is not for editing. An
//! errors window already open and showing lines is shown as before,
//! and toasts nothing.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{canvas, div, px, rgb, AnyElement, Context, MouseButton};

use apex_core::state::Layout;
use apex_core::{ViewId, WindowId};

use crate::app::Acme;

/// How long a toast stays when the pointer leaves it be.
const STAY: Duration = Duration::from_secs(8);
/// The most lines of it shown; Show All has the rest.
const LINES: usize = 6;

pub struct Toast {
    pub window: WindowId,
    pub text: String,
    pub at: Instant,
    pub hovered: bool,
}

impl Acme {
    /// Errors were written to `w` (an errors window) and it is to be
    /// shown: unless it is open and showing lines, it goes to the stash
    /// and a toast says what was written. True when it
    /// was taken care of so.
    pub fn toast_errors(&mut self, w: WindowId) -> bool {
        if self.node.window_kind(w) != apex_core::WinKind::Errors {
            return false;
        }
        let l = &self.node.state.layout;
        // one the user has open (brought back, or opened) and showing
        // lines: seen as it is written. One just made for these errors
        // is laid out too, but is not theirs yet.
        if self.errors_open.contains(&w) && l.slot(w).is_some_and(|s| s.frmax > 0) {
            return false;
        }
        // what was just written: the core selects it (acme's flushwarnings)
        let v = ViewId::Body(w);
        let text = self.node.selected_text(v).unwrap_or_default();
        if l.place_of(w).is_some() {
            let _ = self.node.stash_window(&mut self.log, w);
        }
        match self.toasts.iter_mut().find(|t| t.window == w) {
            Some(t) => {
                t.text.push_str(&text);
                t.at = Instant::now();
            }
            None => self.toasts.push(Toast { window: w, text, at: Instant::now(), hovered: false }),
        }
        true
    }

    /// Show All (or B1 on the toast): its window shown -- stashed, in the
    /// stash's preview and left there; open in a column, brought on
    /// screen -- and the toast gone.
    fn toast_show_all(&mut self, w: WindowId, cx: &mut Context<Self>) {
        let said = self.toasts.iter().find(|t| t.window == w).map(|t| t.text.clone()).unwrap_or_default();
        self.toasts.retain(|t| t.window != w);
        if self.node.state.layout.is_stashed(w) {
            self.peek_errors(w, &said, cx);
        } else {
            self.errors_open.insert(w);
            self.reveal_window(w, cx);
        }
    }

    /// A button went down at `p`, before anything else hears it: off
    /// every toast, they go.
    pub fn toasts_click(&mut self, p: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if self.toasts.is_empty() || self.toasts_at.borrow().iter().any(|b| b.contains(&p)) {
            return;
        }
        self.toasts.clear();
        cx.notify();
    }

    /// The toasts, at the feet of their columns in `l` (the layout drawn).
    pub fn toasts_overlay(&mut self, l: &Layout, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.toasts.retain(|t| t.hovered || t.at.elapsed() < STAY);
        let t = crate::theme::theme();
        let mut out = Vec::new();
        // one stack at the app's lower right, over whatever columns are
        // there, the newest at the bottom
        let mut below = 8.;
        for (i, toast) in self.toasts.iter().enumerate().rev() {
            let w = toast.window;
            let lines: Vec<&str> = toast.text.trim_end().lines().collect();
            let shown = lines[lines.len().saturating_sub(LINES)..].join("\n");
            let more = lines.len() > LINES;
            let width = (l.r.dx() as f32 - 16.).clamp(200., 520.);
            let height = 44. + 16. * shown.lines().count().max(1) as f32;
            let bottom = l.r.y1 as f32 - below;
            below += height + 6.;
            let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(4.)), blur_radius: px(14.), spread_radius: px(0.), inset: false };
            let name = match crate::sidebar::names(&self.node, w) { (what, at) if at.is_empty() => what, (what, at) => format!("{what}  {at}") };
            let at = self.toasts_at.clone();
            let card = div()
                .id(("toast", i))
                .child(canvas(move |b, _, _| at.borrow_mut().push(b), |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full())
                .absolute()
                .left(px(l.r.x1 as f32 - width - 8.))
                .top(px(bottom - height))
                .w(px(width))
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(10.))
                .rounded(px(9.))
                .bg(rgb(t.panel_bg))
                .border_1()
                .border_color(rgb(t.panel_border))
                .shadow(vec![shadow])
                .font_family(crate::fonts::ui())
                .child(self.overlay_mark())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .child(div().flex_1().min_w_0().truncate().text_color(rgb(t.panel_dim)).child(name))
                        .child(
                            div()
                                .id(("toast-all", i))
                                .flex_none()
                                .px(px(6.))
                                .rounded(px(5.))
                                .text_color(rgb(t.accent))
                                .hover(|s| s.bg(rgb(t.panel_hover)))
                                .cursor_default()
                                .child("Show All")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.toast_show_all(w, cx);
                                        cx.stop_propagation();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id(("toast-x", i))
                                .flex_none()
                                .px(px(4.))
                                .rounded(px(5.))
                                .text_color(rgb(t.panel_dim))
                                .hover(|s| s.bg(rgb(t.panel_hover)))
                                .cursor_default()
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.toasts.retain(|t| t.window != w);
                                        cx.notify();
                                        cx.stop_propagation();
                                    }),
                                ),
                        ),
                )
                .child(
                    div()
                        .font_family(crate::fonts::mono().family)
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(rgb(t.panel_text))
                        .overflow_hidden()
                        .children(shown.lines().enumerate().map(|(n, l)| toast_line(w, i, n, l, cx)).collect::<Vec<_>>())
                        .when(more, |d| d.child(div().text_color(rgb(t.panel_dim)).child("…"))),
                )
                .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                    if let Some(t) = this.toasts.iter_mut().find(|t| t.window == w) {
                        t.hovered = *over;
                        t.at = Instant::now();
                    }
                    cx.notify();
                }))
                // B1 anywhere on it (but ×, and ⌘ or ⌥ on a word) is Show All
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        if !e.modifiers.platform && !e.modifiers.alt {
                            this.toast_show_all(w, cx);
                        }
                        cx.stop_propagation();
                    }),
                );
            out.push(card.into_any_element());
        }
        out
    }
}

/// A toast's line, word by word: B3 on a word looks it up from the
/// errors window (as a click in the window would: a file:line opens),
/// B2 runs it there; B1 is the toast's (Show All). The spaces between
/// stay as they were.
fn toast_line(w: WindowId, toast: usize, n: usize, line: &str, cx: &mut Context<Acme>) -> AnyElement {
    let mut row = div().id(("toast-line", toast * 1000 + n)).flex().flex_row().overflow_hidden().whitespace_nowrap();
    let mut rest = line;
    let mut k = 0;
    // how far into the line, in characters: where a word is, for B3 to
    // look from there in the errors window as a click there would
    let mut col = 0;
    while !rest.is_empty() {
        let gap = rest.len() - rest.trim_start().len();
        if gap > 0 {
            row = row.child(div().flex_none().child(rest[..gap].to_string()));
            col += rest[..gap].chars().count();
            rest = &rest[gap..];
            continue;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = rest[..end].to_string();
        let at = col;
        col += word.chars().count();
        rest = &rest[end..];
        let (line1, line2) = (line.to_string(), line.to_string());
        let look = word.trim_matches(|c| !crate::app::is_file_char(c)).to_string();
        let run = word.trim_matches(|c| !crate::app::is_exec_char(c)).to_string();
        let (look1, run1) = (look.clone(), run.clone());
        k += 1;
        row = row.child(
            div()
                .id(("toast-word", (toast * 1000 + n) * 1000 + k))
                .flex_none()
                .rounded(px(3.))
                .hover(|s| s.bg(rgb(crate::theme::theme().panel_hover)))
                .child(word)
                // ⌘-click is B3 and ⌥-click B2, as anywhere
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        if e.modifiers.platform {
                            this.look_in_errors(w, &line1, at, &look1);
                        } else if e.modifiers.alt {
                            this.execute(apex_core::ExecCtx::Window(w), &run1, cx);
                        } else {
                            return;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.look_in_errors(w, &line2, at, &look);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, _, cx| {
                        this.execute(apex_core::ExecCtx::Window(w), &run, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
        );
    }
    row.into_any_element()
}

impl Acme {
    /// B3 on a toast's word: looked from where it is in the errors window
    /// (its line, the last of that text there; `col` characters in), as a
    /// click there would -- the server expanding from that point where
    /// the files are, so `./dir/file.rs:183:1:impl` is the file at its
    /// line. The word alone when the line is not to be found.
    pub fn look_in_errors(&mut self, w: WindowId, line: &str, col: usize, word: &str) {
        let at = self.node.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| {
            let text = self.node.state.buffer(b).ok()?.text.to_string();
            let i = text.rfind(line)?;
            let q = text[..i].chars().count() + col;
            Some(apex_core::Span { buffer: b, q0: q, q1: q })
        });
        self.look_at(apex_core::ExecCtx::Window(w), word, at, None, None, false, None);
    }
}

