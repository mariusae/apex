//! The sidebar (the modern-mac experiment): the sessions down the left
//! as vertical tabs, in a card that floats a little in from the
//! window's edges as Manifold's does, the window's buttons at its top.
//! The one shown has its windows listed under it, column by column, each
//! with its handle's dot: a click on one reveals it and lands on it, as
//! taking a notification does. Nothing here is new to apex but the way
//! in: the rows are the tabs the strip had, and a window row is acme's
//! `show`.

use gpui::{div, prelude::*, px, rgb, BoxShadow, Context, FontWeight, MouseButton};

use crate::app::Acme;
use crate::pool::Pool;
use crate::shell::{pjw, SIDEBAR_HEADER};
use crate::theme;

/// How far the card sits in from the window's edges.
const INSET: f32 = 6.;
const ROW_H: f32 = 30.;
const WIN_ROW_H: f32 = 24.;

impl Acme {
    /// The sidebar: pinned (`floating` false), a column down the window's
    /// left on the columns' ground; floating, the card alone over the
    /// content, a hole cut for it in any page under it.
    pub fn sidebar(&self, floating: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme();
        let dark = theme::is_dark();
        let card = t.strip;
        let hover = theme::step(card, 1);
        let chosen = if dark { 0x3A3A3C } else { 0xFFFFFF };
        let avatar_idle = if dark { 0x58585C } else { 0xB8B8BC };
        let mut list = div().id("sessions").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.)).px(px(6.));
        for (i, tab) in Pool::tabs(cx).into_iter().enumerate() {
            let id = tab.id;
            let current = id == self.tab;
            let u = tab.url.clone();
            let name = if current && self.in_process() { "in-process".to_string() } else { u.session.clone() };
            // what the tab is doing, else where it is when elsewhere
            let word = self.tab_word(&tab, cx);
            let second = word.clone().or_else(|| (!u.is_local()).then(|| u.arg.clone()));
            let notified = self.tab_notified(id, cx);
            let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "·".into());
            let avatar = div()
                .flex_none()
                .size(px(18.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(if current { t.accent } else { avatar_idle }))
                .text_color(rgb(0xFFFFFF))
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .child(initial);
            let mut text = div().flex_1().min_w_0().flex().flex_col().child(
                div()
                    .truncate()
                    .text_size(px(13.))
                    .line_height(px(16.))
                    .font_weight(if current { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .text_color(rgb(if word.is_some() { t.text_dim } else { t.text }))
                    .child(name),
            );
            if let Some(s) = second {
                text = text.child(div().truncate().text_size(px(11.)).line_height(px(13.)).text_color(rgb(t.text_dim)).child(s));
            }
            let mut row = div()
                .id(("session", i))
                .flex_none()
                .min_h(px(ROW_H))
                .py(px(4.))
                .px(px(8.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(avatar)
                .child(text);
            if notified {
                row = row.child(div().flex_none().child(pjw(14., t.text)));
            }
            row = if current {
                row.bg(rgb(chosen)).shadow(vec![BoxShadow { color: gpui::hsla(0., 0., 0., if dark { 0.4 } else { 0.08 }), offset: gpui::point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
            } else {
                row.cursor_default().hover(move |s| s.bg(rgb(hover)))
            };
            let url = u.clone();
            row = row
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if !current {
                            this.switch_to(id, window, cx);
                        }
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.open_rename(url.clone(), cx);
                        cx.stop_propagation();
                    }),
                );
            list = list.child(row);
            if current && self.waiting.is_none() {
                list = list.child(self.window_rows(cx));
            }
        }
        // one more row: a session not here yet, as the strip's + was
        let new = div()
            .id("new-session")
            .flex_none()
            .h(px(ROW_H))
            .px(px(8.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_default()
            .hover(move |s| s.bg(rgb(hover)))
            .text_color(rgb(t.text_dim))
            .child(div().flex_none().w(px(18.)).flex().justify_center().text_size(px(16.)).child("+"))
            .child(div().text_size(px(13.)).child("New Session"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selector.is_some() {
                        this.close_selector(cx);
                    } else {
                        this.open_selector(cx);
                    }
                    cx.stop_propagation();
                }),
            );
        list = list.child(new);
        // the top: the window's buttons, and the one that puts the
        // sidebar away; held anywhere else along it, the window moves
        let toggle = div()
            .id("sidebar-toggle")
            .size(px(22.))
            .rounded(px(5.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .hover(move |s| s.bg(rgb(hover)))
            .child(sidebar_glyph(t.text_dim))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    crate::shell::toggle_sidebar(cx);
                    cx.stop_propagation();
                }),
            );
        let header = div()
            .flex_none()
            .h(px(SIDEBAR_HEADER))
            .flex()
            .items_center()
            .justify_end()
            .pr(px(8.))
            .child(toggle)
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.start_window_move();
                cx.stop_propagation();
            });
        let shadow = BoxShadow { color: gpui::hsla(0., 0., 0., if dark { 0.5 } else { 0.10 }), offset: gpui::point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false };
        let outer = div().id("sidebar").flex_none().w(px(crate::shell::SIDEBAR_W)).h_full().p(px(INSET)).font_family(crate::fonts::ui());
        let outer = if floating { outer } else { outer.bg(rgb(t.column)) };
        outer
            .child(
                div()
                    .relative()
                    .size_full()
                    .rounded(px(10.))
                    .when(floating, |d| d.child(self.overlay_mark()))
                    .bg(rgb(card))
                    .border_1()
                    .border_color(rgb(t.border))
                    .shadow(vec![shadow])
                    .flex()
                    .flex_col()
                    .pb(px(6.))
                    .child(header)
                    .child(list),
            )
            // a click in the sidebar is the sidebar's, not acme's
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
    }

    /// The shown session's windows, column by column in the order the
    /// columns stand, each as its handle shows it and named by the last
    /// part of its name, the folder it is in after it.
    fn window_rows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme();
        let hover = theme::step(t.strip, 1);
        let mut rows = div().flex().flex_col().pb(px(4.));
        let l = &self.node.state.layout;
        for col in &l.cols {
            // stashed ones too, where they stand in the column, their
            // names in the secondary ink (a click brings one back)
            for (w, stashed) in apex_core::tiling::stash_order(col) {
                let mut name = self.node.window_name(w);
                // a blank page, its address not yet typed
                if name.is_empty() && self.node.state.window(w).is_ok_and(|x| x.body == apex_core::Body::Web) {
                    name = "New page".into();
                }
                let (label, dir) = split_name(&name);
                let d = crate::text_element::dot(
                    t,
                    false,
                    self.node.window_unsaved(w),
                    self.node.window_live(w) || self.node.state.window(w).is_ok_and(|x| x.body == apex_core::Body::Web),
                    self.node.window_working(w),
                    self.window_notified(w),
                );
                let row = div()
                    .id(("win", w.0))
                    .flex_none()
                    .h(px(WIN_ROW_H))
                    .pl(px(14.))
                    .pr(px(8.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_default()
                    .hover(move |s| s.bg(rgb(hover)))
                    .child(dot_element(&d))
                    .child(div().flex_none().max_w(px(120.)).truncate().text_size(px(12.5)).text_color(rgb(if stashed { t.text_dim } else { t.text })).child(label))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(rgb(t.text_dim)).child(dir))
                    .when(d.badge, |r| r.child(div().flex_none().child(pjw(12., t.accent))))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.reveal_window(w, cx);
                            cx.stop_propagation();
                        }),
                    );
                rows = rows.child(row);
            }
        }
        rows
    }
}

/// A window's name as a row shows it: its last part (a directory's with
/// its slash), and the folder it is in, home as `~`.
fn split_name(name: &str) -> (String, String) {
    let trimmed = name.trim_end_matches('/');
    let slash = if name.ends_with('/') && !trimmed.is_empty() { "/" } else { "" };
    let (dir, last) = match trimmed.rfind('/') {
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => ("", trimmed),
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = match dir.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => dir.to_string(),
    };
    let label = if last.is_empty() { name.to_string() } else { format!("{last}{slash}") };
    (label, dir)
}

/// The handle's dot as a row draws it: the same marks, at a row's size.
fn dot_element(d: &crate::text_element::Dot) -> impl IntoElement {
    let mut el = div().flex_none().size(px(9.)).rounded_full();
    if let Some(f) = d.fill {
        el = el.bg(rgb(f));
    }
    if let Some(r) = d.ring {
        el = el.border(px(1.25)).border_color(rgb(r));
    }
    if let Some(c) = d.core {
        el = el.flex().items_center().justify_center().child(div().size(px(3.5)).rounded_full().bg(rgb(c)));
    }
    el
}

/// The sidebar button's glyph: a window with a panel down its left.
fn sidebar_glyph(ink: u32) -> impl IntoElement {
    div()
        .w(px(15.))
        .h(px(12.))
        .rounded(px(3.))
        .border(px(1.25))
        .border_color(rgb(ink))
        .flex()
        .child(div().w(px(4.5)).h_full().border_r(px(1.25)).border_color(rgb(ink)))
}

#[cfg(test)]
mod tests {
    use super::split_name;

    #[test]
    fn a_window_row_names_the_last_part_and_its_folder() {
        assert_eq!(split_name("/tmp/proj/main.rs"), ("main.rs".into(), "/tmp/proj".into()));
        assert_eq!(split_name("/tmp/proj/"), ("proj/".into(), "/tmp".into()));
        assert_eq!(split_name("+Errors"), ("+Errors".into(), "".into()));
    }
}
