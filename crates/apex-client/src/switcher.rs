//! ctrl-tab: the connected sessions, the title bar's tabs, as Manifold's
//! ⌘E lays out a stack -- side by side across the whole window as iOS
//! lays out apps, each a live card of its session's window, the chosen
//! one in the middle with its neighbours running off the sides. Each
//! press moves the choice on, most recently shown first (this one, then
//! the parked ones by when they were parked), ctrl-shift-tab back; the
//! order is the one when control was pressed and holds while it is
//! held, so the presses walk the list rather than bouncing between the
//! last two. Letting go of control switches to the one chosen (a click
//! on a card does too); escape leaves things be.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, rgb, AnyElement, Context, FontWeight, MouseButton, Window};

use crate::app::Acme;
use crate::miniature::{snapshot, Mini};
use crate::pool::{Pool, TabId};

/// How long the cards take to slide to a new choice.
const SLIDE: Duration = Duration::from_millis(220);
/// The label over a card: its session's initial and name.
const LABEL: f32 = 26.;

pub struct Switcher {
    /// The connected sessions (the tabs), most recently shown first as
    /// they were when control was pressed: this window's, then the
    /// parked ones by when they were parked.
    pub entries: Vec<TabId>,
    pub index: usize,
    /// Where the cards were when the choice last moved, and when: they
    /// slide from there.
    from: (f32, Instant),
}

impl Switcher {
    /// Where the choice is drawn just now, as a fractional index.
    fn pos(&self) -> f32 {
        let (from, at) = self.from;
        let k = (at.elapsed().as_secs_f32() / SLIDE.as_secs_f32()).min(1.);
        // Manifold's curve, near enough: quick, then settling
        let e = 1. - (1. - k).powi(3);
        from + (self.index as f32 - from) * e
    }

    fn sliding(&self) -> bool {
        self.from.1.elapsed() < SLIDE
    }
}

impl Acme {
    fn switcher_entries(&self, cx: &Context<Self>) -> Vec<TabId> {
        let mut out: Vec<TabId> = vec![self.tab];
        for id in Pool::by_recency(cx) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    /// ctrl-tab (`back`: ctrl-shift-tab): the choice one on through the
    /// sessions; the first press brings the cards up with the one before
    /// this chosen.
    pub fn switcher_step(&mut self, back: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_none() {
            let entries = self.switcher_entries(cx);
            self.switcher = Some(Switcher { entries, index: 0, from: (0., Instant::now()) });
        }
        let Some(s) = self.switcher.as_mut() else { return };
        let n = s.entries.len();
        if n > 1 {
            s.from = (s.pos(), Instant::now());
            s.index = if back { (s.index + n - 1) % n } else { (s.index + 1) % n };
        }
        cx.notify();
    }

    /// Control let go (or a card clicked): to the session chosen, and
    /// that is where the window has settled.
    pub fn switcher_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.switcher.take() {
            if let Some(id) = s.entries.get(s.index).copied() {
                if id != self.tab {
                    self.switch_to(id, window, cx);
                }
            }
        }
        Pool::note_settled(cx, self.tab);
        cx.notify();
    }

    /// Escape with control still held: the cards go, nothing changes.
    pub fn close_switcher(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        cx.notify();
    }

    /// The cards, over the whole window, while control is held.
    pub fn switcher_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.switcher.as_ref()?;
        let t = crate::theme::theme();
        let dark = crate::theme::is_dark();
        let vp = window.viewport_size();
        let (ww, wh) = (f32::from(vp.width), f32::from(vp.height));
        // a card is the session's window, scaled as Manifold scales a pane
        let l = &self.node.state.layout;
        let (cw, ch) = if l.r.x1 > 0 && l.r.y1 > 0 { (l.r.x1 as f32, l.r.y1 as f32) } else { (ww, wh) };
        let scale = 0.66f32.min(ww * 0.66 / cw).min(wh * 0.66 / ch);
        let (w, h) = (cw * scale, ch * scale);
        let step = w * 0.74;
        let pos = s.pos();
        if s.sliding() {
            window.request_animation_frame();
        }
        let parked = Pool::parked_nodes(cx);
        let tabs = Pool::tabs(cx);
        let avatar_idle = if dark { 0x58585C } else { 0xB8B8BC };
        let mut cards: Vec<(f32, AnyElement)> = Vec::new();
        for (i, &id) in s.entries.iter().enumerate() {
            let offset = i as f32 - pos;
            if offset.abs() > 2.6 {
                continue; // well off the window's sides
            }
            // the choice full size and clear, the others a little less
            let near = offset.abs().min(1.);
            let shrink = 1. - 0.1 * near;
            let (cw_i, ch_i) = (w * shrink, h * shrink);
            let mid = ww / 2. + offset * step;
            let x = mid - cw_i / 2.;
            let y = wh / 2. - ch_i / 2. - 12. - LABEL / 2.;
            let chosen = i == s.index;
            let url = tabs.iter().find(|t| t.id == id).map(|t| t.url.clone());
            let name = url.as_ref().map(|u| u.session.clone()).unwrap_or_default();
            let mini: Option<Mini> = if id == self.tab && self.connected {
                Some(snapshot(&self.node, &t))
            } else {
                parked.iter().find(|(p, _, _)| *p == id).map(|(_, _, n)| snapshot(n, &t))
            };
            let word = if mini.is_none() { Pool::state(cx, id).word().unwrap_or("not connected").to_string() } else { String::new() };
            let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "·".into());
            let label = div()
                .h(px(LABEL))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(2.))
                .child(
                    div()
                        .flex_none()
                        .size(px(16.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgb(if id == self.tab { t.accent } else { avatar_idle }))
                        .text_color(rgb(0xFFFFFF))
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .child(initial),
                )
                .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(rgb(t.text)).child(name))
                .when_some(url.filter(|u| !u.is_local()).map(|u| u.arg), |d, host| d.child(div().flex_none().text_size(px(12.)).text_color(rgb(t.text_dim)).child(host)));
            let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false };
            let edge = if chosen { gpui::Hsla::from(rgb(t.accent)).opacity(0.8) } else { rgb(t.panel_border).into() };
            let body = gpui::canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    if let Some(m) = &mini {
                        m.paint(b, window, cx);
                    }
                },
            )
            .size_full();
            let holder = div()
                .relative()
                .w(px(cw_i))
                .h(px(ch_i))
                .rounded(px(12.))
                .overflow_hidden()
                .bg(rgb(t.body_bg))
                .border(px(if chosen { 2. } else { 0.5 }))
                .border_color(edge)
                .shadow(vec![shadow])
                .child(body)
                .when(!word.is_empty(), |d| {
                    d.child(div().absolute().top(px(0.)).left(px(0.)).size_full().flex().items_center().justify_center().text_size(px(13.)).text_color(rgb(t.text_dim)).child(word))
                });
            let card = div()
                .id(("switcher-card", i))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(cw_i))
                .flex()
                .flex_col()
                .opacity(1. - 0.2 * near)
                .font_family(crate::fonts::ui())
                .child(label)
                .child(holder)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if let Some(s) = this.switcher.as_mut() {
                            s.index = i;
                        }
                        this.switcher_commit(window, cx);
                        cx.stop_propagation();
                    }),
                );
            cards.push((offset.abs(), card.into_any_element()));
        }
        // the nearer the choice the more in front, the choice above all
        cards.sort_by(|a, b| b.0.total_cmp(&a.0));
        let ground = if dark { 0x141414 } else { 0xEDEDED };
        let mut overlay = div()
            .id("switcher")
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .bg(rgb(ground))
            // clicks off the cards do nothing, and reach nothing beneath
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.overlay_mark());
        for (_, c) in cards {
            overlay = overlay.child(c);
        }
        Some(overlay.into_any_element())
    }
}
