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
//! ⌘⇧\ (or ⌘'): all the sessions at once, as Mission Control shows the
//! windows -- a grid of cards over the window, each its session's window
//! drawn small and live. The window shrinks into its card as the grid
//! comes up; one ring (the accent's) is on the card chosen, moving to
//! the one under the pointer or the arrows' next; a click or return goes
//! to it, its card growing to fill the window. Escape, ⌘⇧\ again or a
//! click off the cards goes back to this one the same way.

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
/// How long the overview takes to come up, to go (a card growing to the
/// window), and its ring to move from card to card.
const OVERVIEW_RISE: Duration = Duration::from_millis(260);
const OVERVIEW_PICK: Duration = Duration::from_millis(240);
const OVERVIEW_RING: Duration = Duration::from_millis(140);
/// Round a card, and how far out of it the ring stands.
const CARD_R: f32 = 12.;
const RING_OUT: f32 = 4.;

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

/// The overview (⌘⇧\): when it came up, the card chosen (the ring's)
/// and the one the ring moved from and when, and the card gone to and
/// when (growing to fill the window).
pub struct Overview {
    opened: Instant,
    focus: usize,
    moved: Option<(usize, Instant)>,
    picked: Option<(TabId, Instant)>,
}

/// A rectangle, left, top, width, height.
type Box4 = (f32, f32, f32, f32);

fn mix(a: Box4, b: Box4, k: f32) -> Box4 {
    (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k, a.2 + (b.2 - a.2) * k, a.3 + (b.3 - a.3) * k)
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

    /// ⌘⇧\: the overview up, or away (back to this session, as escape).
    pub fn toggle_overview(&mut self, cx: &mut Context<Self>) {
        match &self.overview {
            Some(_) => self.overview_pick(self.tab, cx),
            None => {
                let focus = Pool::tabs(cx).iter().position(|t| t.id == self.tab).unwrap_or(0);
                self.overview = Some(Overview { opened: Instant::now(), focus, moved: None, picked: None });
            }
        }
        cx.notify();
    }

    /// Session `id` chosen in the overview: its card grows to fill the
    /// window, and then it is gone to (`overview_tick`).
    fn overview_pick(&mut self, id: TabId, cx: &mut Context<Self>) {
        let ids: Vec<TabId> = Pool::tabs(cx).iter().map(|t| t.id).collect();
        let Some(o) = self.overview.as_mut() else { return };
        if o.picked.is_some() {
            return;
        }
        if let Some(i) = ids.iter().position(|&x| x == id) {
            if i != o.focus {
                o.moved = Some((o.focus, Instant::now()));
                o.focus = i;
            }
        }
        o.picked = Some((id, Instant::now()));
        cx.notify();
    }

    /// The ring to card `i` (the pointer on it, an arrow).
    fn overview_focus(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(o) = self.overview.as_mut() else { return };
        if o.picked.is_some() || o.focus == i {
            return;
        }
        o.moved = Some((o.focus, Instant::now()));
        o.focus = i;
        cx.notify();
    }

    /// A key while the overview is up: the arrows move the ring through
    /// the grid, return goes to its card, escape back to this one.
    pub fn overview_key(&mut self, key: &str, window: &Window, cx: &mut Context<Self>) {
        let n = Pool::tabs(cx).len();
        let Some(o) = self.overview.as_ref() else { return };
        if n == 0 || o.picked.is_some() {
            return;
        }
        let (cols, ..) = self.overview_grid(window, n);
        let f = o.focus;
        let to = match key {
            "left" => f.checked_sub(1),
            "right" | "tab" => (f + 1 < n).then_some(f + 1),
            "up" => f.checked_sub(cols),
            "down" => (f + cols < n).then_some(f + cols),
            "enter" => {
                if let Some(t) = Pool::tabs(cx).get(f) {
                    let id = t.id;
                    self.overview_pick(id, cx);
                }
                return;
            }
            "escape" => {
                self.overview_pick(self.tab, cx);
                return;
            }
            _ => None,
        };
        if let Some(i) = to {
            self.overview_focus(i, cx);
        }
    }

    /// Each frame: a card gone to that has grown to fill the window is
    /// the session shown now, and the overview goes.
    pub fn overview_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, at)) = self.overview.as_ref().and_then(|o| o.picked) else { return };
        if at.elapsed() < OVERVIEW_PICK {
            return;
        }
        self.overview = None;
        if id != self.tab {
            self.switch_to(id, window, cx);
        }
        Pool::note_settled(cx, self.tab);
        cx.notify();
    }

    /// The grid for `n` cards in the window: its columns, a card's width
    /// and height (below its label), and the grid's top left.
    fn overview_grid(&self, window: &Window, n: usize) -> (usize, f32, f32, f32, f32) {
        let n = n.max(1);
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
        (cols, w, h, (ww - gw) / 2., (wh - gh) / 2.)
    }

    /// Session `id`'s card, `w` by `h` (its window, small and live) with
    /// its label over it; the shown one's edge in the accent.
    fn session_card(&self, id: TabId, w: f32, h: f32, label_alpha: f32, cx: &mut Context<Self>) -> gpui::Div {
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
            .rounded(px(CARD_R))
            .overflow_hidden()
            .bg(rgb(t.body_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .child(body)
            .when(!word.is_empty(), |d| {
                d.child(div().absolute().top(px(0.)).left(px(0.)).size_full().flex().items_center().justify_center().text_size(px(13.)).text_color(rgb(t.text_dim)).child(word))
            });
        div().w(px(w)).flex().flex_col().font_family(crate::fonts::ui()).child(label.opacity(label_alpha)).child(holder)
    }

    /// The overview, over the whole window: every session a card, in a
    /// grid as even as the window allows, in the sidebar's order, the
    /// ring on the one chosen.
    pub fn overview_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let o = self.overview.as_ref()?;
        let t = crate::theme::theme();
        let dark = crate::theme::is_dark();
        let ids: Vec<TabId> = Pool::tabs(cx).iter().map(|t| t.id).collect();
        let n = ids.len().max(1);
        let vp = window.viewport_size();
        let (ww, wh) = (f32::from(vp.width), f32::from(vp.height));
        let (cols, w, h, x0, y0) = self.overview_grid(window, n);
        let gap = 28f32;
        // coming up, going (a card growing to the window), the ring moving
        let rise = ease(o.opened.elapsed().as_secs_f32() / OVERVIEW_RISE.as_secs_f32());
        let (gone, go) = match o.picked {
            Some((id, at)) => (ids.iter().position(|&x| x == id), ease(at.elapsed().as_secs_f32() / OVERVIEW_PICK.as_secs_f32())),
            None => (None, 0.),
        };
        let ring_k = o.moved.map(|(_, at)| ease(at.elapsed().as_secs_f32() / OVERVIEW_RING.as_secs_f32())).unwrap_or(1.);
        // (going: until the tick has gone there, which a frame asks)
        if rise < 1. || gone.is_some() || ring_k < 1. {
            window.request_animation_frame();
        }
        let here = ids.iter().position(|&x| x == self.tab);
        // the window's own place: where this session's card comes from,
        // and where the one gone to grows to
        let full: Box4 = (self.left(), self.top(), ww - self.left(), wh - self.top());
        // card `i`'s window part (below its label) as it stands this frame
        let grid = |i: usize| -> Box4 {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            (x0 + c * (w + gap), y0 + r * (h + LABEL + gap) + LABEL, w, h)
        };
        let place = |i: usize| -> (Box4, f32) {
            let g = grid(i);
            if gone == Some(i) {
                return (mix(g, full, go), 1.);
            }
            if gone.is_some() {
                return (g, 1. - go);
            }
            if here == Some(i) {
                return (mix(full, g, rise), 1.);
            }
            // the others come up from a little smaller, fading in
            let k = 0.92 + 0.08 * rise;
            ((g.0 + g.2 * (1. - k) / 2., g.1 + g.3 * (1. - k) / 2., g.2 * k, g.3 * k), rise)
        };
        let ground = if dark { 0x141414 } else { 0xEDEDED };
        // opaque once up: nothing of the window behind shows through
        let scrim = if gone.is_some() { 1. - go } else { rise };
        let mut overlay = div()
            .id("overview")
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .bg(gpui::Hsla::from(rgb(ground)).opacity(scrim))
            // a click off the cards goes back to this session
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.overview_pick(this.tab, cx);
                    cx.stop_propagation();
                }),
            )
            .child(self.overlay_mark());
        // the card growing or shrinking to the window drawn last, over the rest
        let mut order: Vec<usize> = (0..ids.len()).collect();
        let front = gone.or(if rise < 1. { here } else { None });
        if let Some(f) = front {
            order.retain(|&i| i != f);
            order.push(f);
        }
        for i in order {
            let id = ids[i];
            let ((x, y, cw, ch), alpha) = place(i);
            let zooming = front == Some(i);
            // its label goes as it becomes the window
            let label_alpha = if gone == Some(i) { 1. - go } else if zooming { rise } else { 1. };
            let card = self.session_card(id, cw, ch, label_alpha, cx);
            overlay = overlay.child(
                div()
                    .id(("overview-card", i))
                    .absolute()
                    .left(px(x))
                    .top(px(y - LABEL))
                    .opacity(alpha)
                    .cursor_pointer()
                    .child(card)
                    .hover_listener_mode(gpui::HoverListenerMode::InputModalityAware)
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                        if *on {
                            this.overview_focus(i, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.overview_pick(id, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        // the ring: the accent's, round the card chosen, moving from the
        // last one to it; with its card as that grows to the window
        if !ids.is_empty() {
            let to = place(o.focus.min(ids.len() - 1)).0;
            let (x, y, rw, rh) = match o.moved {
                Some((from, _)) if ring_k < 1. && from < ids.len() => mix(place(from).0, to, ring_k),
                _ => to,
            };
            let alpha = if gone.is_some() { 1. - go } else { rise };
            overlay = overlay.child(
                div()
                    .absolute()
                    .left(px(x - RING_OUT))
                    .top(px(y - RING_OUT))
                    .w(px(rw + 2. * RING_OUT))
                    .h(px(rh + 2. * RING_OUT))
                    .rounded(px(CARD_R + RING_OUT))
                    .border(px(2.5))
                    .border_color(gpui::Hsla::from(rgb(t.accent)).opacity(0.9 * alpha)),
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
