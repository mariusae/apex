//! The sidebar (the modern-mac experiment): the sessions down the left
//! as vertical tabs, in a card a little in from the window's edges as
//! Manifold's is -- every known host's, under the host (this Mac first),
//! asked for in the background: those open in the app as their tabs,
//! the others fainter, a click opening one. A host that does not answer
//! keeps what it had last, and says so.
//! The one shown has its windows listed under it, column by column, each
//! with its handle's dot: a click on one reveals it and lands on it, as
//! taking a notification does. Nothing here is new to apex but the way
//! in: the rows are the tabs the strip had, and a window row is acme's
//! `show`.

use gpui::{div, prelude::*, px, rgb, BoxShadow, Context, FontWeight, MouseButton};

use crate::app::Acme;
use crate::pool::{Pool, Tab};
use crate::shell::{Host, Loading};
use crate::shell::pjw;
use crate::theme;

/// How far the card sits in from the window's edges.
pub const INSET: f32 = 6.;
const ROW_H: f32 = 30.;
const WIN_ROW_H: f32 = 24.;

impl Acme {
    /// The sidebar: a column down the window's left, under the title bar,
    /// its card on the columns' ground.
    pub fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme();
        let dark = theme::is_dark();
        let card = t.strip;
        let hover = theme::step(card, 1);
        let chosen = if dark { 0x3A3A3C } else { 0xFFFFFF };
        let avatar_idle = if dark { 0x58585C } else { 0xB8B8BC };
        let mut list = div().id("sessions").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(2.)).px(px(6.));
        let entries = self.sidebar_entries(cx);
        let headed = entries.iter().filter(|e| matches!(e, Entry::Host(..))).count() > 1;
        for entry in entries {
            let (i, tab) = match entry {
                // a host's name over its sessions (when there is more than
                // one host), and whether it could be asked
                Entry::Host(k, name, down) => {
                    if headed {
                        list = list.child(
                            div()
                                .id(("host", k))
                                .flex_none()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.))
                                .px(px(8.))
                                .pt(px(if k == 0 { 2. } else { 10. }))
                                .pb(px(2.))
                                .text_size(px(11.))
                                .font_weight(crate::fonts::weight(FontWeight::MEDIUM))
                                .text_color(rgb(t.text_dim))
                                .child(name)
                                .when(down, |d| d.child(div().font_weight(crate::fonts::weight(FontWeight::NORMAL)).child("unreachable"))),
                        );
                    }
                    continue;
                }
                // a session not open in the app: fainter, a click opens it
                Entry::Known(k, url) => {
                    let name = url.session.clone();
                    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "·".into());
                    list = list.child(
                        div()
                            .id(("known-session", k))
                            .flex_none()
                            .min_h(px(ROW_H))
                            .py(px(4.))
                            .px(px(8.))
                            .rounded(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .cursor_default()
                            .hover(move |s| s.bg(rgb(hover)))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(18.))
                                    .rounded_full()
                                    .border(px(1.25))
                                    .border_color(rgb(avatar_idle))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgb(t.text_dim))
                                    .text_size(px(10.))
                                    .font_weight(crate::fonts::weight(FontWeight::BOLD))
                                    .child(initial),
                            )
                            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(rgb(t.text_dim)).child(name))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.switch_to_url(&url, window, cx);
                                    cx.defer(|cx| crate::shell::save_open(cx));
                                    cx.notify();
                                    cx.stop_propagation();
                                }),
                            ),
                    );
                    continue;
                }
                Entry::Tab(i, tab) => (i, tab),
            };
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
                .font_weight(crate::fonts::weight(FontWeight::BOLD))
                .child(initial);
            let mut text = div().flex_1().min_w_0().flex().flex_col().child(
                div()
                    .truncate()
                    .text_size(px(13.))
                    .line_height(px(16.))
                    .font_weight(crate::fonts::weight(if current { FontWeight::MEDIUM } else { FontWeight::NORMAL }))
                    .text_color(rgb(if word.is_some() { t.text_dim } else { t.text }))
                    .child(name),
            );
            if let Some(s) = second {
                text = text.child(div().truncate().text_size(px(11.)).line_height(px(13.)).text_color(rgb(t.text_dim)).child(s));
            }
            // where the row is drawn: its preview stands beside it
            let rows = self.sidebar_rows.clone();
            let mut row = div()
                .id(("session", i))
                .relative()
                .child(gpui::canvas(move |b, _, _| {
                    rows.borrow_mut().insert(id, b);
                }, |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full())
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
            // the pointer on the row: an × to let the session go, as a
            // tab's had (the session stays on its host; the tab goes)
            if self.sidebar_hover == Some(id) {
                row = row.child(
                    div()
                        .id(("session-close", i))
                        .flex_none()
                        .size(px(18.))
                        .rounded(px(5.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(14.))
                        .text_color(rgb(t.text_dim))
                        .hover(move |s| s.bg(rgb(theme::step(if current { chosen } else { hover }, 1))).text_color(rgb(t.text)))
                        .child("×")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                if current {
                                    this.close_current_session(window, cx);
                                } else {
                                    crate::pool::Pool::let_go(cx, id);
                                }
                                this.sidebar_hover = None;
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        ),
                );
            } else if notified && !current {
                // pjw is for another session wanting the user, never this one
                row = row.child(div().flex_none().child(pjw(14., t.text)));
            }
            row = if current {
                row.bg(rgb(chosen)).shadow(vec![BoxShadow { color: gpui::hsla(0., 0., 0., if dark { 0.4 } else { 0.08 }), offset: gpui::point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])
            } else {
                row.cursor_default().hover(move |s| s.bg(rgb(hover)))
            };
            let url = u.clone();
            row = row
                .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                    if *over {
                        this.sidebar_hover = Some(id);
                    } else if this.sidebar_hover == Some(id) {
                        this.sidebar_hover = None;
                    }
                    cx.notify();
                }))
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
        let shadow = BoxShadow { color: gpui::hsla(0., 0., 0., if dark { 0.5 } else { 0.10 }), offset: gpui::point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false };
        // round the card: the ground the windows stand on, as under the
        // column tags and between the cards
        div()
            .id("sidebar")
            .flex_none()
            .h_full()
            .font_family(crate::fonts::ui())
            .w(px(crate::shell::SIDEBAR_W))
            .px(px(INSET))
            .pb(px(INSET))
            .bg(rgb(crate::text_element::ground(&t)))
            .child(
                // its top is the title bar's, round the window's buttons
                // (`title_bar`): joined to it, square and open there
                div()
                    .relative()
                    .size_full()
                    .rounded_b(px(10.))
                    .bg(rgb(card))
                    .border_l_1()
                    .border_r_1()
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .shadow(vec![shadow])
                    .flex()
                    .flex_col()
                    .pb(px(6.))
                    .pt(px(6.))
                    .child(list),
            )
            // a click in the sidebar is the sidebar's, not acme's
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
    }

    /// The shown session's windows, column by column in the order the
    /// columns stand, each as its handle shows it and named by the last
    /// part of its name, the folder it is in after it; then the stash,
    /// the latest put away first (a click brings one back where it was).
    fn window_rows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme();
        let mut rows = div().flex().flex_col().pb(px(4.));
        let l = &self.node.state.layout;
        for col in &l.cols {
            for s in &col.wins {
                rows = rows.child(self.window_row(s.window, false, cx));
            }
        }
        if !l.stash.is_empty() {
            rows = rows.child(
                div()
                    .flex_none()
                    .pl(px(14.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .font_weight(crate::fonts::weight(FontWeight::MEDIUM))
                    .text_color(rgb(t.text_dim))
                    .child("Stashed"),
            );
            for s in l.stash.iter().rev() {
                rows = rows.child(self.window_row(s.slot.window, true, cx));
            }
        }
        rows
    }

    /// Window `w`'s row: a click on a laid-out one reveals it and lands
    /// on it; on a stashed one, brings it back.
    fn window_row(&self, w: apex_core::WindowId, stashed: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme();
        let hover = theme::step(t.strip, 1);
        let mut name = self.node.window_name(w);
        // a blank page, its address not yet typed
        if name.is_empty() && self.node.state.window(w).is_ok_and(|x| x.body == apex_core::Body::Web) {
            name = "New page".into();
        }
        let (label, dir) = split_name(&name);
        let d = self.window_dot(w);
        div()
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
            .child(div().flex_none().max_w(px(120.)).truncate().text_size(px(12.5)).text_color(rgb(t.text)).child(label))
            .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(rgb(t.text_dim)).child(dir))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if stashed {
                        this.unstash(w, cx);
                    } else {
                        this.reveal_window(w, cx);
                    }
                    cx.stop_propagation();
                }),
            )
    }
}

/// A window's name as a row shows it: its last part (a directory's with
/// its slash), and the folder it is in, home as `~`.
pub(crate) fn split_name(name: &str) -> (String, String) {
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
pub(crate) fn dot_element(d: &crate::text_element::Dot) -> impl IntoElement {
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
pub fn sidebar_glyph(ink: u32) -> impl IntoElement {
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

/// A row of the sidebar's list.
enum Entry {
    /// A host's name (its place in the list, the name, whether it failed to answer).
    Host(usize, String, bool),
    /// One of the app's tabs (its place among them).
    Tab(usize, Tab),
    /// A host's session not open in the app.
    Known(usize, apex_server::providers::SessionUrl),
}

impl Acme {
    /// The sidebar's rows: each host (this Mac first, then the known
    /// ones, then any other a tab is on), its tabs under it, then its
    /// other sessions.
    fn sidebar_entries(&self, cx: &gpui::App) -> Vec<Entry> {
        let tabs = Pool::tabs(cx);
        let mut hosts: Vec<Host> = self.sidebar_hosts.iter().map(|(h, _)| h.clone()).collect();
        if hosts.is_empty() {
            hosts = crate::shell::known_hosts();
        }
        for t in &tabs {
            let h = Host::of(&t.url);
            if !hosts.contains(&h) {
                hosts.push(h);
            }
        }
        let same = |a: &apex_server::providers::SessionUrl, b: &apex_server::providers::SessionUrl| a.provider == b.provider && a.arg == b.arg && a.session == b.session;
        let mut out = Vec::new();
        let mut k = 0;
        for (hi, h) in hosts.iter().enumerate() {
            let loading = self.sidebar_hosts.iter().find(|(x, _)| x == h).map(|(_, l)| l);
            let name = if h.is_local() { "This Mac".to_string() } else { h.arg.clone() };
            out.push(Entry::Host(hi, name, matches!(loading, Some(Loading::Failed(..)))));
            for (i, t) in tabs.iter().enumerate().filter(|(_, t)| Host::of(&t.url) == *h) {
                out.push(Entry::Tab(i, t.clone()));
            }
            for s in loading.map(|l| l.sessions()).unwrap_or_default() {
                let u = h.url_of(s);
                if !tabs.iter().any(|t| same(&t.url, &u)) {
                    out.push(Entry::Known(k, u));
                    k += 1;
                }
            }
        }
        out
    }

    /// Every known host asked for its sessions, in the background: what
    /// it had last shown until it answers, and kept if it does not.
    pub fn sidebar_refresh(&mut self, cx: &mut Context<Self>) {
        self.sidebar_asked = Some(std::time::Instant::now());
        let mut hosts = crate::shell::known_hosts();
        for t in Pool::tabs(cx) {
            let h = Host::of(&t.url);
            if !hosts.contains(&h) {
                hosts.push(h);
            }
        }
        let mut known = crate::shell::known_sessions();
        for h in &hosts {
            if !self.sidebar_hosts.iter().any(|(x, _)| x == h) {
                self.sidebar_hosts.push((h.clone(), Loading::Seeded(known.remove(h).unwrap_or_default())));
            }
        }
        self.sidebar_hosts.sort_by_key(|(h, _)| hosts.iter().position(|x| x == h).unwrap_or(usize::MAX));
        for h in hosts {
            let (host, socket) = (h.clone(), self.socket.clone());
            let asking = cx.background_executor().spawn(async move {
                if host.is_local() {
                    let Some(socket) = socket else { return Err("no daemon".to_string()) };
                    apex_server::remote::list_sessions(&socket).map_err(|e| e.to_string())
                } else {
                    apex_server::providers::list_sessions(&host.dest()).map_err(|e| e.to_string())
                }
            });
            cx.spawn(async move |this, cx| {
                let r = asking.await;
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |acme, cx| {
                        let before = acme.sidebar_hosts.iter().find(|(x, _)| *x == h).map(|(_, l)| l.sessions().to_vec()).unwrap_or_default();
                        let loaded = match r {
                            Ok(names) => {
                                crate::shell::note_sessions(&h, &names);
                                Loading::Ready(names)
                            }
                            Err(e) => Loading::Failed(before, e),
                        };
                        match acme.sidebar_hosts.iter_mut().find(|(x, _)| *x == h) {
                            Some(e) => e.1 = loaded,
                            None => acme.sidebar_hosts.push((h.clone(), loaded)),
                        }
                        cx.notify();
                    });
                });
            })
            .detach();
        }
    }
}
