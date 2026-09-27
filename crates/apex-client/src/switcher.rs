//! ctrl-tab: the connected sessions, the title bar's tabs, as Manifold's
//! ⌘E lays out a stack -- side by side across the whole window as iOS
//! lays out apps, each a live card of its session's window, the chosen
//! one in the middle with its neighbours running off the sides. Each
//! press moves the choice on, most recently shown first (this one, then
//! the parked ones by when they were parked), ctrl-shift-tab back; the
//! order is the one when control was pressed and holds while it is
//! held, so the presses walk the list rather than bouncing between the
//! last two. Letting go of control switches to the one chosen (a click
//! on a card does too); escape leaves things be. The cards come up only
//! once control has been held a moment (`HOLD`): a quick ctrl-tab, let go
//! at once, goes to the last session without them flashing up, as the
//! system's app switcher does.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, rgb, AnyElement, Context, FontWeight, MouseButton, Window};

use crate::app::Acme;
use crate::miniature::{snapshot, Mini, Tilt};
use crate::pool::{Pool, TabId};

/// How long the cards take to slide to a new choice.
const SLIDE: Duration = Duration::from_millis(220);
/// The label over a card: its session's initial and name.
const LABEL: f32 = 26.;
/// How long control is held before the cards come up.
const HOLD: Duration = Duration::from_millis(150);

pub struct Switcher {
    /// The connected sessions (the tabs), most recently shown first as
    /// they were when control was pressed: this window's, then the
    /// parked ones by when they were parked.
    pub entries: Vec<TabId>,
    pub index: usize,
    /// Where the cards were when the choice last moved, and when: they
    /// slide from there.
    from: (f32, Instant),
    /// When ctrl-tab was first pressed: the cards show `HOLD` after.
    opened: Instant,
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
            let now = Instant::now();
            self.switcher = Some(Switcher { entries, index: 0, from: (0., now), opened: now });
            // drawn again when the cards are to come up, if control is
            // still held then
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HOLD).await;
                let _ = this.update(cx, |a, cx| {
                    if a.switcher.is_some() {
                        cx.notify();
                    }
                });
            })
            .detach();
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
        // not until control has been held a moment
        if s.opened.elapsed() < HOLD {
            return None;
        }
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

/// ⌘E: a column's stash as a stack of cards leaning back within the
/// column, each a live preview, as Safari once showed its tabs: what the
/// column shows now at the front, the stashed windows behind it, most
/// recently put away first. The column tilts back into the front card
/// and slides down as the stash rises behind it, the first stashed
/// window chosen; more E's choose further back (⇧E forward again), the
/// cards before the choice sliding down to the foot and gathering there
/// as their tags. Letting go of ⌘ brings the chosen window back where it
/// was, its card settling flat onto where it lands (a click on a card
/// does too); escape settles the front card back as the column.
pub struct StashWalk {
    pub col: apex_core::ColumnId,
    /// The stashed windows, the most recently put away first; card 0 is
    /// the column as it shows, card k the stashed window k - 1.
    pub entries: Vec<apex_core::WindowId>,
    /// The card chosen.
    pub index: usize,
    from: (f32, Instant),
    /// When the stack came up.
    opened: Instant,
    /// Letting go: the card settling, since when, and the rectangle it
    /// settles on (in the column's window space).
    closing: Option<(usize, Instant, (f32, f32, f32, f32))>,
    /// Brought up with ⌘ down (⌘E), so ⌘ coming up ends it; from the
    /// menu, a click or escape does.
    pub held: bool,
}

/// How long the stack takes to come up, and a card to settle.
const RISE: Duration = Duration::from_millis(320);
const SETTLE: Duration = Duration::from_millis(260);

fn ease(k: f32) -> f32 {
    let k = k.clamp(0., 1.);
    1. - (1. - k).powi(3)
}

impl StashWalk {
    fn pos(&self) -> f32 {
        let (from, at) = self.from;
        let e = ease(at.elapsed().as_secs_f32() / SLIDE.as_secs_f32());
        from + (self.index as f32 - from) * e
    }

    /// How far the stack is up: 0 flat as the column, 1 standing.
    fn up(&self) -> f32 {
        let rise = ease(self.opened.elapsed().as_secs_f32() / RISE.as_secs_f32());
        match self.closing {
            Some((_, at, _)) => rise * (1. - ease(at.elapsed().as_secs_f32() / SETTLE.as_secs_f32())),
            None => rise,
        }
    }

    fn moving(&self) -> bool {
        self.from.1.elapsed() < SLIDE || self.opened.elapsed() < RISE || self.closing.is_some()
    }

    /// Settled: the walk is over.
    pub fn done(&self) -> bool {
        self.closing.is_some_and(|(_, at, _)| at.elapsed() >= SETTLE)
    }

    pub fn settling(&self) -> bool {
        self.closing.is_some()
    }
}

/// Card `r` cards behind the choice (in front when negative), standing:
/// its tilt in a space `w` by `h`, the tag line `font` high.
fn standing(r: f32, w: f32, h: f32, font: f32) -> Tilt {
    let margin = 0.055 * w;
    let width = w - 2. * margin;
    let vs = 0.9;
    let card_h = h * width / w * vs;
    // the choice's top, a tag's height between those behind it, and in
    // front of it two places down the column: part way, then at the
    // foot with only its tag showing
    let (sel, gap) = (0.15 * h, 0.047 * h);
    let (near, foot) = (0.66 * h, h - font * 1.1);
    let top = if r >= 0. {
        sel - gap * r.min(3.) - 4. * (r - 3.).max(0.)
    } else {
        let f = -r;
        if f <= 1. {
            sel + (near - sel) * f
        } else if f <= 2. {
            near + (foot - near) * (f - 1.)
        } else {
            foot + 6. * (f - 2.)
        }
    };
    Tilt { left: margin, top, width, slope: 0.12, vs, height: card_h }
}

impl Acme {
    /// The column ⌘E is about: the active one -- the column of the window
    /// the keys go to, else the last one worked in -- and only when it
    /// has windows put away; never another column's.
    fn stash_walk_column(&self) -> Option<apex_core::ColumnId> {
        let l = &self.node.state.layout;
        let keys = self.key_window().and_then(|w| l.column_of(w));
        keys.or(self.node.activecol).filter(|&c| l.column(c).is_some_and(|c| !c.stash.is_empty()))
    }

    /// ⌥⌘E: the active column's whole stash back where it was, as ⌘E's
    /// walk would bring back one.
    pub fn unstash_all(&mut self, cx: &mut Context<Self>) {
        if self.stash_walk.is_some() {
            return;
        }
        let Some(col) = self.stash_walk_column() else { return };
        let _ = self.node.unstash_all(&mut self.log, col);
        self.after();
        cx.notify();
    }

    /// ⌘E (`back`: ⇧⌘E): the stack brought up, the first stashed window
    /// chosen, or the choice one on through it.
    pub fn stash_walk_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.stash_walk.as_ref().is_some_and(|s| s.settling()) {
            return;
        }
        if self.stash_walk.is_none() {
            let Some(col) = self.stash_walk_column() else { return };
            let Some(c) = self.node.state.layout.column(col) else { return };
            let entries: Vec<apex_core::WindowId> = c.stash.iter().rev().map(|s| s.slot.window).collect();
            self.stash_open = None;
            let now = Instant::now();
            let held = crate::web::modifiers_down().0;
            self.stash_walk = Some(StashWalk { col, entries, index: 1, from: (0., now), opened: now, closing: None, held });
            cx.notify();
            return;
        }
        let Some(s) = self.stash_walk.as_mut() else { return };
        let n = s.entries.len() + 1;
        s.from = (s.pos(), Instant::now());
        s.index = if back { (s.index + n - 1) % n } else { (s.index + 1) % n };
        cx.notify();
    }

    /// ⌘ let go (or a card clicked): the chosen window back where it was,
    /// its card settling onto where it lands; the front card, the column
    /// as it is, settles back as the column.
    pub fn stash_walk_commit(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.stash_walk.as_ref() else { return };
        if s.settling() {
            return;
        }
        let (col, index) = (s.col, s.index);
        let w = if index == 0 { None } else { s.entries.get(index - 1).copied() };
        if let Some(w) = w.filter(|&w| self.node.state.layout.is_stashed(w)) {
            self.reveal_window(w, cx);
        }
        // where the card settles: the window's place now, in the column's
        // window space; the whole of it for the front card
        let font = f32::from(crate::text_element::tag_line_height());
        let l = &self.node.state.layout;
        let target = l.column(col).map(|c| {
            let (x0, y0) = (c.r.x0 as f32, c.r.y0 as f32 + font);
            let whole = (0., 0., c.r.dx() as f32, c.r.y1 as f32 - y0);
            match w.and_then(|w| c.wins.iter().find(|s| s.window == w)) {
                Some(s) => (s.r.x0 as f32 - x0, s.r.y0 as f32 - y0, s.r.dx() as f32, s.r.dy() as f32),
                None => whole,
            }
        });
        if let (Some(s), Some(target)) = (self.stash_walk.as_mut(), target) {
            s.closing = Some((index, Instant::now(), target));
        } else {
            self.stash_walk = None;
        }
        cx.notify();
    }

    /// Escape: the front card chosen, settling back as the column.
    pub fn stash_walk_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.stash_walk.as_mut() {
            if !s.settling() {
                s.from = (s.pos(), Instant::now());
                s.index = 0;
            }
        }
        self.stash_walk_commit(cx);
    }

    /// The stack, over the column, while ⌘ is held; `l` the layout drawn.
    pub fn stash_walk_overlay(&self, l: &apex_core::state::Layout, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.stash_walk.as_ref()?;
        let ci = l.column_index(s.col)?;
        let c = &l.cols[ci];
        let t = crate::theme::theme();
        let dark = crate::theme::is_dark();
        let font = f32::from(crate::text_element::tag_line_height());
        let (x0, y0) = (c.r.x0 as f32, c.r.y0 as f32 + font);
        let (cw, ch) = (c.r.dx() as f32, (c.r.y1 as f32 - y0).max(1.));
        if s.moving() {
            window.request_animation_frame();
        }
        let pos = s.pos();
        let up = s.up();
        // the cards, each a live preview: the column as it shows, then
        // the stashed windows as each would stand filling it
        let mut minis: Vec<Mini> = Vec::new();
        minis.push(crate::miniature::snapshot_column(&self.node, ci, &t).map(|(m, _)| m)?);
        for &w in &s.entries {
            minis.push(crate::miniature::snapshot_window(&self.node, w, cw, ch, &t));
        }
        let flat = Tilt { left: 0., top: 0., width: cw, slope: 0., vs: 1., height: ch };
        let closing = s.closing;
        let settle_k = closing.map(|(_, at, _)| ease(at.elapsed().as_secs_f32() / SETTLE.as_secs_f32())).unwrap_or(0.);
        // back to front: the furthest behind first, then nearer; those in
        // front of the choice last, the nearest the viewer on top
        let n = minis.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| (a as f32 - pos).total_cmp(&(b as f32 - pos)).reverse());
        let mut tilts: Vec<(usize, Tilt, f32)> = Vec::new();
        for k in order {
            let r = k as f32 - pos;
            let stand = standing(r, cw, ch, font);
            let mut tilt = Tilt::lerp(flat, stand, up);
            let mut alpha = 1.;
            if let Some((chosen, _, target)) = closing {
                if k == chosen {
                    // the chosen card settles flat onto its window's place
                    let onto = Tilt { left: target.0, top: target.1, width: target.2, slope: 0., vs: 1., height: target.3 };
                    tilt = Tilt::lerp(stand, onto, settle_k);
                } else {
                    alpha = 1. - settle_k;
                }
            } else if r > 3.5 {
                alpha = (4.5 - r).clamp(0., 1.);
            }
            tilts.push((k, tilt, alpha));
        }
        // the chosen card last while settling, over everything
        if let Some((chosen, _, _)) = closing {
            if let Some(i) = tilts.iter().position(|(k, _, _)| *k == chosen) {
                let c = tilts.remove(i);
                tilts.push(c);
            }
        }
        let ground_top = if dark { 0x454545 } else { 0xE9E9EB };
        let ground_bottom = if dark { 0x3B4350 } else { 0xD9DEE6 };
        let shade_alpha = if dark { 0.3 } else { 0.1 };
        let border = t.panel_border;
        let ground_alpha = if closing.is_some() { 1. - settle_k } else { up.min(1.) };
        let hit: Vec<(usize, f32, f32)> = tilts.iter().map(|(k, t, _)| (*k, t.top, t.top + t.height)).collect();
        let canvas = gpui::canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let (bx, by) = (f32::from(b.left()), f32::from(b.top()));
                let bg = gpui::linear_gradient(
                    180.,
                    gpui::linear_color_stop(gpui::Hsla::from(rgb(ground_top)).opacity(ground_alpha), 0.),
                    gpui::linear_color_stop(gpui::Hsla::from(rgb(ground_bottom)).opacity(ground_alpha), 1.),
                );
                window.paint_quad(gpui::fill(b, bg));
                window.with_content_mask(Some(gpui::ContentMask { bounds: b }), |window| {
                    for (k, tilt, alpha) in &tilts {
                        let tilt = Tilt { left: tilt.left + bx, top: tilt.top + by, ..*tilt };
                        // a soft shade just above the card's top edge, parting
                        // it from the one behind, and a hairline round it
                        let lean = (tilt.slope / 0.12).clamp(0., 1.);
                        if lean > 0.01 {
                            let shade = gpui::linear_gradient(
                                180.,
                                gpui::linear_color_stop(gpui::hsla(0., 0., 0., 0.), 0.),
                                gpui::linear_color_stop(gpui::hsla(0., 0., 0., shade_alpha * lean * alpha), 1.),
                            );
                            window.paint_quad(gpui::fill(gpui::Bounds::new(gpui::point(px(tilt.left), px(tilt.top - 6.)), gpui::size(px(tilt.width), px(6.))), shade));
                            let c = tilt.corners();
                            let o = 0.75;
                            let edge = [(c[0].0 - o, c[0].1 - o), (c[1].0 + o, c[1].1 - o), (c[2].0 + o, c[2].1), (c[3].0 - o, c[3].1)];
                            crate::miniature::quad(window, edge, gpui::Hsla::from(rgb(border)).opacity(lean * alpha));
                        }
                        minis[*k].paint_tilted(tilt, *alpha, window, cx);
                    }
                });
            },
        )
        .size_full();
        let col_top = y0;
        let overlay = div()
            .id("stash-walk")
            .absolute()
            .left(px(x0))
            .top(px(col_top))
            .w(px(cw))
            .h(px(ch))
            .overflow_hidden()
            .child(canvas)
            .child(self.overlay_mark())
            // a click on a card chooses it: the nearest the viewer whose
            // showing part is under the pointer
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    let (_, y) = this.row_pt(e.position);
                    let y = y as f32 - col_top;
                    if let Some(&(k, _, _)) = hit.iter().rev().find(|(_, top, bottom)| y >= *top && y <= *bottom) {
                        if let Some(s) = this.stash_walk.as_mut() {
                            s.index = k;
                        }
                        this.stash_walk_commit(cx);
                    }
                    cx.stop_propagation();
                }),
            );
        Some(overlay.into_any_element())
    }
}

impl Acme {
    /// The pointer on a session's row in the sidebar (not the one shown):
    /// its window, live, beside the row, as ctrl-tab's cards draw it.
    pub fn session_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
        // beside the row; on glass the row is in the panel's window (the
        // card, INSET in from the main one's corner), and the preview,
        // in the main window, is under the panel: past its edge
        let (x, y) = if self.on_glass() {
            let inset = crate::sidebar::INSET;
            (crate::shell::SIDEBAR_W - inset + 10., f32::from(row.top()) + inset - 8.)
        } else {
            (f32::from(row.right()) + 10., f32::from(row.top()) - 8.)
        };
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
