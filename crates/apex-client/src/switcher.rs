//! ctrl-tab: the connected sessions, the title bar's tabs, walked live.
//! Each press shows the next session in the window at once, sliding in
//! from the right over the one it replaces (ctrl-shift-tab: the one
//! before, from the left), and the walk goes on while control is held;
//! letting go leaves the window on the one it came to. The order is
//! the one when control was pressed and holds while it is held, most
//! recently settled on first (this one, then the parked ones), so the
//! presses walk the list rather than bouncing between the last two, and
//! the sessions passed on the way are not taken as settled on. Escape
//! goes back to where the walk began.
//!
//! ⌘⇧\: all the sessions at once, as Mission Control shows the windows
//! -- a grid of cards over the window, each its session's window drawn
//! small and live; a click on one goes to it, escape (or ⌘⇧\ again)
//! leaves things be.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, rgb, AnyElement, Context, FontWeight, MouseButton, Window};

use crate::app::Acme;
use crate::glide::ease;
use crate::miniature::{snapshot, Mini};
use crate::pool::{Pool, TabId};

/// How long a session takes to slide in.
const SLIDE: Duration = Duration::from_millis(220);
/// The label over a card: its session's initial and name.
const LABEL: f32 = 26.;
/// How long the overview takes to come up.
const OVERVIEW_RISE: Duration = Duration::from_millis(180);

pub struct Switcher {
    /// The connected sessions (the tabs), most recently settled on first
    /// as they were when control was pressed: this window's, then the
    /// parked ones.
    pub entries: Vec<TabId>,
    pub index: usize,
}

/// The session shown sliding in over the one it replaced: which way
/// (1 from the right, -1 from the left), since when, and the one it
/// replaced as it was, sliding out.
pub struct SwitchSlide {
    dir: f32,
    at: Instant,
    outgoing: Option<std::rc::Rc<Mini>>,
}

impl SwitchSlide {
    /// How far across the window the incoming session still is, 0 to 1.
    fn left(&self) -> f32 {
        1. - ease(self.at.elapsed().as_secs_f32() / SLIDE.as_secs_f32())
    }
}

/// The overview (⌘⇧\): when it came up.
pub struct Overview {
    opened: Instant,
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

    /// Session `id` shown, sliding in from `dir`'s side over this one.
    fn slide_to(&mut self, id: TabId, dir: f32, window: &mut Window, cx: &mut Context<Self>) {
        if id == self.tab {
            return;
        }
        let t = crate::theme::theme();
        let outgoing = (self.connected && self.waiting.is_none()).then(|| std::rc::Rc::new(snapshot(&self.node, &t)));
        self.switch_to(id, window, cx);
        self.switch_slide = Some(SwitchSlide { dir, at: Instant::now(), outgoing });
        cx.notify();
    }

    /// ctrl-tab (`back`: ctrl-shift-tab): the next session in the walk,
    /// shown at once.
    pub fn switcher_step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_none() {
            let entries = self.switcher_entries(cx);
            self.switcher = Some(Switcher { entries, index: 0 });
        }
        let Some(s) = self.switcher.as_mut() else { return };
        let n = s.entries.len();
        if n < 2 {
            return;
        }
        s.index = if back { (s.index + n - 1) % n } else { (s.index + 1) % n };
        let id = s.entries[s.index];
        self.slide_to(id, if back { -1. } else { 1. }, window, cx);
    }

    /// Control let go: the window stays on the session it came to, and
    /// that is where it has settled.
    pub fn switcher_commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        Pool::note_settled(cx, self.tab);
        cx.notify();
    }

    /// Escape with control still held: back to where the walk began.
    pub fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.switcher.take() {
            if let Some(&home) = s.entries.first() {
                self.slide_to(home, -1., window, cx);
            }
        }
        cx.notify();
    }

    /// Where the content is drawn across while a session slides in (0
    /// once it has), and the one it replaced drawn beside it, sliding
    /// out, in a box `width` wide from `left` across.
    pub fn switch_slide(&mut self, window: &mut Window, left: f32, width: f32, height: f32) -> (f32, Option<AnyElement>) {
        let Some(s) = self.switch_slide.as_ref() else { return (0., None) };
        let k = s.left();
        if k <= 0. {
            self.switch_slide = None;
            return (0., None);
        }
        window.request_animation_frame();
        let off = s.dir * width * k;
        let out = s.outgoing.clone().map(|m| {
            let x = off - s.dir * width;
            let t = crate::theme::theme();
            let body = gpui::canvas(|_, _, _| {}, move |b, _, window, cx| m.paint(b, window, cx)).size_full();
            div()
                .absolute()
                .left(px(left))
                .top(px(0.))
                .w(px(width))
                .h(px(height))
                .overflow_hidden()
                .child(div().absolute().left(px(x)).top(px(0.)).w(px(width)).h(px(height)).bg(rgb(t.column)).child(body))
                .into_any_element()
        });
        (off, out)
    }

    /// ⌘⇧\: the overview up, or away.
    pub fn toggle_overview(&mut self, cx: &mut Context<Self>) {
        self.overview = match self.overview {
            Some(_) => None,
            None => Some(Overview { opened: Instant::now() }),
        };
        cx.notify();
    }

    /// Session `id` from the overview: gone to, and settled on.
    fn overview_pick(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.overview = None;
        if id != self.tab {
            self.switch_to(id, window, cx);
        }
        Pool::note_settled(cx, self.tab);
        cx.notify();
    }

    /// Session `id`'s card, `w` by `h` (its window, small and live) with
    /// its label over it; the shown one's edge in the accent.
    fn session_card(&self, id: TabId, w: f32, h: f32, cx: &mut Context<Self>) -> gpui::Div {
        let t = crate::theme::theme();
        let dark = crate::theme::is_dark();
        let avatar_idle = if dark { 0x58585C } else { 0xB8B8BC };
        let parked = Pool::parked_nodes(cx);
        let tabs = Pool::tabs(cx);
        let url = tabs.iter().find(|t| t.id == id).map(|t| t.url.clone());
        let name = url.as_ref().map(|u| u.session.clone()).unwrap_or_default();
        let shown = id == self.tab;
        let mini: Option<Mini> = if shown && self.connected {
            Some(snapshot(&self.node, &t))
        } else {
            parked.iter().find(|(p, _, _)| *p == id).map(|(_, _, n)| snapshot(n, &t))
        };
        let word = if mini.is_none() { Pool::state(cx, id).word().unwrap_or("not connected").to_string() } else { String::new() };
        let notified = self.tab_notified(id, cx);
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
                    .bg(rgb(if shown { t.accent } else { avatar_idle }))
                    .text_color(rgb(0xFFFFFF))
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .child(initial),
            )
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(rgb(t.text)).child(name))
            .when(notified && !shown, |d| d.child(div().flex_none().child(crate::shell::pjw(13., t.accent))))
            .when_some(url.filter(|u| !u.is_local()).map(|u| u.arg), |d, host| d.child(div().flex_none().text_size(px(12.)).text_color(rgb(t.text_dim)).child(host)));
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false };
        let edge: gpui::Hsla = if shown { gpui::Hsla::from(rgb(t.accent)).opacity(0.8) } else { rgb(t.panel_border).into() };
        let accent: gpui::Hsla = gpui::Hsla::from(rgb(t.accent)).opacity(0.6);
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
            .w(px(w))
            .h(px(h))
            .rounded(px(12.))
            .overflow_hidden()
            .bg(rgb(t.body_bg))
            .border(px(if shown { 2. } else { 1. }))
            .border_color(edge)
            .shadow(vec![shadow])
            .when(!shown, |d| d.group_hover("overview-card", move |s| s.border_color(accent)))
            .child(body)
            .when(!word.is_empty(), |d| {
                d.child(div().absolute().top(px(0.)).left(px(0.)).size_full().flex().items_center().justify_center().text_size(px(13.)).text_color(rgb(t.text_dim)).child(word))
            });
        div().group("overview-card").w(px(w)).flex().flex_col().font_family(crate::fonts::ui()).child(label).child(holder)
    }

    /// The overview, over the whole window: every session a card, in a
    /// grid as even as the window allows, in the sidebar's order.
    pub fn overview_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let o = self.overview.as_ref()?;
        let dark = crate::theme::is_dark();
        let rise = ease(o.opened.elapsed().as_secs_f32() / OVERVIEW_RISE.as_secs_f32());
        if rise < 1. {
            window.request_animation_frame();
        }
        let ids: Vec<TabId> = Pool::tabs(cx).iter().map(|t| t.id).collect();
        let n = ids.len().max(1);
        let vp = window.viewport_size();
        let (ww, wh) = (f32::from(vp.width), f32::from(vp.height));
        let l = &self.node.state.layout;
        let aspect = if l.r.x1 > 0 && l.r.y1 > 0 { l.r.x1 as f32 / l.r.y1 as f32 } else { ww / wh.max(1.) };
        // the grid with the largest cards that fit
        let (pad, gap) = (48f32, 28f32);
        let (aw, ah) = ((ww - 2. * pad).max(100.), (wh - 2. * pad).max(100.));
        let mut best = (1usize, 0f32, 0f32);
        for cols in 1..=n {
            let rows = n.div_ceil(cols);
            let cw = (aw - gap * (cols as f32 - 1.)) / cols as f32;
            let chh = (ah - gap * (rows as f32 - 1.)) / rows as f32 - LABEL;
            let w = cw.min(chh * aspect).min(ww * 0.45);
            if w > best.1 {
                best = (cols, w, w / aspect);
            }
        }
        let (cols, w, h) = best;
        let rows = n.div_ceil(cols);
        let (gw, gh) = (cols as f32 * w + (cols as f32 - 1.) * gap, rows as f32 * (h + LABEL) + (rows as f32 - 1.) * gap);
        let (x0, y0) = ((ww - gw) / 2., (wh - gh) / 2.);
        // the cards come up from a little smaller, fading in
        let grow = 0.94 + 0.06 * rise;
        let ground = if dark { 0x141414 } else { 0xEDEDED };
        let mut overlay = div()
            .id("overview")
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .bg(gpui::Hsla::from(rgb(ground)).opacity(0.96 * rise))
            // a click off the cards leaves things be
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.overview = None;
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            .child(self.overlay_mark());
        for (i, &id) in ids.iter().enumerate() {
            let (col, row) = ((i % cols) as f32, (i / cols) as f32);
            let (cx0, cy0) = (x0 + col * (w + gap), y0 + row * (h + LABEL + gap));
            let (cw, ch) = (w * grow, h * grow);
            let x = cx0 + (w - cw) / 2.;
            let y = cy0 + (h + LABEL - ch - LABEL) / 2.;
            let card = self.session_card(id, cw, ch, cx);
            overlay = overlay.child(
                div()
                    .id(("overview-card", i))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .opacity(rise)
                    .cursor_pointer()
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.overview_pick(id, window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        Some(overlay.into_any_element())
    }
}

impl Acme {
    /// The pointer on a session's row in the sidebar (not the one shown):
    /// its window, live, beside the row, as ctrl-tab's cards draw it.
    pub fn session_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // only while the sidebar is shown
        if !self.sidebar_shown() {
            return None;
        }
        let id = self.sidebar_hover.filter(|id| *id != self.tab)?;
        let row = self.sidebar_rows.borrow().get(&id).copied()?;
        let t = crate::theme::theme();
        let parked = Pool::parked_nodes(cx);
        let node = parked.iter().find(|(p, _, _)| *p == id).map(|(_, _, n)| *n)?;
        let mini = snapshot(node, &t);
        let l = &node.state.layout;
        let (cw, ch) = (l.r.x1.max(1) as f32, l.r.y1.max(1) as f32);
        let w = 320f32;
        let h = w * ch / cw;
        // beside the row
        let (x, y) = (f32::from(row.right()) + 10., f32::from(row.top()) - 8.);
        let y = y.max(8.);
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.25), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false };
        let body = gpui::canvas(|_, _, _| {}, move |b, _, window, cx| mini.paint(b, window, cx)).size_full();
        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(w))
                .h(px(h))
                .rounded(px(10.))
                .overflow_hidden()
                .bg(rgb(t.body_bg))
                .border(px(0.5))
                .border_color(rgb(t.panel_border))
                .shadow(vec![shadow])
                .child(body)
                .child(self.overlay_mark())
                .into_any_element(),
        )
    }
}
