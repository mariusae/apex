//! Errors as toasts. A command's errors go to its directory's `+Errors`
//! window, as ever -- the core's, and every client's -- but that window
//! is not opened over the work: it goes to its column's stash, and what
//! was just written shows in a toast at the foot of that column, by the
//! stash, with Show All (the window brought back) and ×. A toast goes by
//! itself after a while unless the pointer is on it. An `+Errors` window
//! already open and showing lines is shown as before, and toasts nothing.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, rgb, AnyElement, Context, MouseButton};

use apex_core::state::Layout;
use apex_core::{tiling, ViewId, WindowId};

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
    /// Errors were written to `w` (an `+Errors` window) and it is to be
    /// shown: unless it is open and showing lines, it goes to its
    /// column's stash and a toast says what was written. True when it
    /// was taken care of so.
    pub fn toast_errors(&mut self, w: WindowId) -> bool {
        if !self.node.window_name(w).ends_with(apex_core::node::ERRORS) {
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
            let _ = self.node.grow_window(&mut self.log, w, 3);
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

    /// The toasts, at the feet of their columns in `l` (the layout drawn).
    pub fn toasts_overlay(&mut self, l: &Layout, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.toasts.retain(|t| t.hovered || t.at.elapsed() < STAY);
        let t = crate::theme::theme();
        let mut out = Vec::new();
        // one stack per column, the newest at the bottom
        let mut up_by: std::collections::HashMap<apex_core::ColumnId, f32> = std::collections::HashMap::new();
        for (i, toast) in self.toasts.iter().enumerate().rev() {
            let w = toast.window;
            let Some(col) = l.column_of(w).and_then(|c| l.column(c)) else { continue };
            let lines: Vec<&str> = toast.text.trim_end().lines().collect();
            let shown = lines[lines.len().saturating_sub(LINES)..].join("\n");
            let more = lines.len() > LINES;
            let width = (col.r.dx() as f32 - 16.).clamp(160., 420.);
            let below = up_by.entry(col.id).or_insert(tiling::stash_band(col) as f32 + 8.);
            let height = 44. + 16. * shown.lines().count().max(1) as f32;
            let bottom = col.r.y1 as f32 - *below;
            *below += height + 6.;
            let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(4.)), blur_radius: px(14.), spread_radius: px(0.), inset: false };
            let name = self.node.window_name(w);
            let card = div()
                .id(("toast", i))
                .absolute()
                .left(px(col.r.x1 as f32 - width - 8.))
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
                                        this.toasts.retain(|t| t.window != w);
                                        this.errors_open.insert(w);
                                        this.reveal_window(w, cx);
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
                        .children(shown.lines().map(|l| div().truncate().child(l.to_string())).collect::<Vec<_>>())
                        .when(more, |d| d.child(div().text_color(rgb(t.panel_dim)).child("…"))),
                )
                .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                    if let Some(t) = this.toasts.iter_mut().find(|t| t.window == w) {
                        t.hovered = *over;
                        t.at = Instant::now();
                    }
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            out.push(card.into_any_element());
        }
        out
    }
}
