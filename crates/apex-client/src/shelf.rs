//! The stash, in the title bar: the windows put away (⌘M, `Stash`) as
//! their tags made small, bunched at the bar's right end like a hand of
//! cards, the latest on top. The pointer on them, or a scroll over
//! them, fans them out, and the one under the pointer (or scrolled to)
//! shows below the bar -- the window itself, live, at the size it had:
//! the pointer can go onto it and work in it as in any window (select,
//! B2, B3, type, scroll) and it stays stashed; the fan closes a moment
//! after the pointer has left both. A click on a card, or B1 on the
//! handle in the preview, brings the window back where it was. The sidebar lists them too. Going to a stashed window any other
//! way (Look, the plumber, a notification) brings it back as well.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{canvas, div, px, rgb, AnyElement, Bounds, Context, HoverListenerMode, MouseButton, Pixels, ScrollWheelEvent, Window};

use apex_core::WindowId;

use crate::app::Acme;
use crate::glide::ease;

/// A card as the bar shows it: as wide as the widest name needs (the
/// dot, its gap and the card's padding besides), up to a limit; and how
/// much of each one under the top card peeks out while they are bunched
/// (a few at most).
const CARD_MAX: f32 = 240.;
const CARD_PAD: f32 = 7. + 9. + 6. + 7. + 2. + 2.;
const CARD_H: f32 = 22.;
const CARD_TEXT: f32 = 12.;
const PEEK: f32 = 5.;
const PEEKS: usize = 4;
/// How far a notified card under the top one -- or one whose window is
/// working (a language server indexing) -- is drawn out of the bunch,
/// leftward: its handle and the start of its name, each further one as
/// far again past it; and how long a notified one takes to come out.
const PULL: f32 = 34.;
const PULL_IN: f32 = 0.3;
/// Between the cards fanned out, and the narrowest they get to fit.
const GAP: f32 = 4.;
const MIN_W: f32 = 72.;
/// How long the fan takes to open or close, and how long it waits for
/// the pointer to come back (on its way from a card to its preview).
const FAN: Duration = Duration::from_millis(160);
const GRACE: Duration = Duration::from_millis(250);
/// A scroll this far moves the choice one card.
const NOTCH: f32 = 24.;
/// The least the preview is, whatever size the window had (one stashed
/// as it was made, an errors window, may have had a few lines): this
/// wide, and this tall or this much of the window's height.
const PREVIEW_W: f32 = 480.;
const PREVIEW_H: f32 = 320.;
const PREVIEW_OF_H: f32 = 0.4;

#[derive(Default)]
pub struct Shelf {
    /// Fanned out (or on its way): the pointer is on the cards or the
    /// preview, or left them a moment ago.
    pub hovered: bool,
    on_stack: bool,
    on_preview: bool,
    /// When the pointer left both.
    left_at: Option<Instant>,
    /// Where the preview was drawn last: the pointer there is on it and
    /// nothing under it. And where the cards were.
    pub preview_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub stack_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// When the fan last began to open or close.
    since: Option<Instant>,
    /// The card chosen: under the pointer, or scrolled to.
    pub pick: Option<WindowId>,
    /// Scrolled, not yet a whole notch.
    scroll: f32,
    /// Stashed windows worked in (clicked or typed in, in the preview;
    /// a toast's Show All), in that order: brought forward among the
    /// cards when the fan closes -- not while it is open, where a card
    /// moving would take the preview out from under the pointer.
    pub touched: Vec<WindowId>,
}

impl Shelf {
    /// How far fanned out (0 bunched, 1 open) this instant, and whether
    /// that is still changing.
    fn fan(&self) -> (f32, bool) {
        let k = self.since.map(|t| t.elapsed().as_secs_f32() / FAN.as_secs_f32()).unwrap_or(1.).min(1.);
        let k = ease(k);
        if self.hovered {
            (k, k < 1.)
        } else {
            (1. - k, k < 1.)
        }
    }

    /// The window shown in the preview.
    pub fn peeking(&self) -> Option<WindowId> {
        self.pick.filter(|_| self.hovered && self.fan().0 > 0.5)
    }

    /// Fanned out at once on stashed window `w`, its preview up (a toast's
    /// Show All): the pointer is taken onto it.
    pub fn open_on(&mut self, w: WindowId) {
        self.hovered = true;
        self.since = Some(Instant::now() - FAN);
        self.pick = Some(w);
        self.scroll = 0.;
        self.on_preview = true;
        self.left_at = None;
    }

    /// The pointer moved to `p`: on the cards or the preview, or off both
    /// (the hover listeners hear no warp, and a move straight off after
    /// one is no leaving to them). True when that changed anything.
    pub fn pointer_at(&mut self, p: gpui::Point<Pixels>) -> bool {
        if !self.hovered {
            return false;
        }
        let on = [&self.stack_at, &self.preview_at].iter().any(|c| c.get().is_some_and(|b| b.contains(&p)));
        match (on, self.left_at) {
            (true, Some(_)) => {
                self.left_at = None;
                true
            }
            (false, None) => {
                self.on_stack = false;
                self.on_preview = false;
                self.left_at = Some(Instant::now());
                true
            }
            _ => false,
        }
    }

    /// The pointer came onto or left the cards (`stack`) or the preview.
    fn touch(&mut self, stack: bool, on: bool) {
        if stack {
            self.on_stack = on;
        } else {
            self.on_preview = on;
        }
        if self.on_stack || self.on_preview {
            self.left_at = None;
            self.hover(true);
        } else if self.hovered {
            self.left_at = Some(Instant::now());
        }
    }

    fn hover(&mut self, on: bool) {
        if on == self.hovered {
            return;
        }
        // turning round midway: from where it is, not from the end
        let (k, _) = self.fan();
        self.hovered = on;
        let done = if on { k } else { 1. - k };
        self.since = Some(Instant::now() - FAN.mul_f32(done.clamp(0., 1.)));
        if !on {
            self.pick = None;
            self.scroll = 0.;
        }
    }
}

impl Acme {
    /// The stashed windows as the bar shows them, the latest first.
    fn shelved(&self) -> Vec<WindowId> {
        self.node.state.layout.stash.iter().rev().map(|s| s.slot.window).collect()
    }

    /// Stashed window `w`'s card's name: its label or the last part of
    /// its path, or what it is when it has neither.
    fn shelf_label(&self, w: WindowId) -> String {
        crate::sidebar::names(&self.node, w).0
    }

    #[allow(dead_code)]
    fn shelf_kind_label(&self, w: WindowId) -> String {
        match self.node.window_kind(w) {
            apex_core::WinKind::Term => "Terminal".into(),
            apex_core::WinKind::Web => "New page".into(),
            _ => "Untitled".into(),
        }
    }

    /// How wide the cards are: the widest name's, all of them alike.
    fn shelf_card_w(&self, cx: &gpui::App) -> f32 {
        let ts = cx.text_system();
        let id = ts.resolve_font(&gpui::font(crate::fonts::ui()));
        let width = |s: &str| s.chars().map(|c| ts.advance(id, px(CARD_TEXT), c).map(|a| f32::from(a.width)).unwrap_or(7.)).sum::<f32>();
        let widest = self.node.state.layout.stash.iter().map(|s| width(&self.shelf_label(s.slot.window))).fold(0., f32::max);
        (widest.ceil() + CARD_PAD).clamp(MIN_W, CARD_MAX)
    }

    /// ⌘M: the window the keys go to (else the one under the pointer)
    /// put in the stash.
    pub fn stash_key(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(w) = self.key_window().or_else(|| self.window_at_pointer(window)) else { return };
        let _ = self.node.stash_window(&mut self.log, w);
        self.after();
        cx.notify();
    }

    /// The stashed windows worked in while the fan was open, brought
    /// forward: the last worked in rightmost, the first card met.
    fn restack(&mut self) {
        let touched = std::mem::take(&mut self.shelf.touched);
        if touched.is_empty() {
            return;
        }
        for w in touched {
            let _ = self.node.restash_window(&mut self.log, w);
        }
        self.after();
    }

    /// A stashed window worked in: brought forward when the fan closes.
    pub fn touch_stashed(&mut self, w: WindowId) {
        if self.node.state.layout.is_stashed(w) {
            self.shelf.touched.retain(|&x| x != w);
            self.shelf.touched.push(w);
        }
    }

    /// A toast's Show All: its stashed window shown in the stash's
    /// preview, left stashed, the pointer taken to the start of what the
    /// toast said (its first line selected).
    pub fn peek_errors(&mut self, w: WindowId, said: &str, cx: &mut Context<Self>) {
        let v = apex_core::ViewId::Body(w);
        let Some(body) = self.text_of(v).map(|t| t.to_string()) else { return };
        let first = said.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        // the toast's text is the tail of the window's (what was written
        // since it came), else where its first line last is
        let at = if !said.is_empty() && body.ends_with(said) {
            body.len() - said.len() + said.find(first).unwrap_or(0)
        } else {
            body.rfind(first).unwrap_or(body.len())
        };
        let q0 = body[..at].chars().count();
        let q1 = q0 + first.chars().count();
        self.shelf.open_on(w);
        self.touch_stashed(w);
        let _ = self.node.select(&mut self.log, v, q0, q1);
        self.node.seltext = Some(v);
        self.show_at.insert(v, (q0, 1));
        self.node.warp = Some(apex_core::Warp::Sel(v));
        self.after();
        cx.notify();
    }

    /// A stashed window brought back where it was, from the bar or the
    /// sidebar.
    pub fn unstash(&mut self, w: WindowId, cx: &mut Context<Self>) {
        let _ = self.node.unstash_window(&mut self.log, w);
        if self.shelf.pick == Some(w) {
            self.shelf.pick = None;
        }
        self.after();
        cx.notify();
    }

    /// How much room the bunched cards want at the bar's right end
    /// (nothing without a stash).
    pub fn shelf_room(&self, cx: &gpui::App) -> f32 {
        let n = self.node.state.layout.stash.len();
        if n == 0 {
            0.
        } else {
            let wins = self.shelved();
            let pulls = self.shelf_pulls(&wins);
            let out = (0..n).map(|i| PEEK * i.min(PEEKS) as f32 + pulls[i]).fold(0., f32::max);
            self.shelf_card_w(cx) + out + 20.
        }
    }

    /// Each stashed window's card out of the bunch or not -- working, or
    /// notified -- and since when, kept as that changes (each frame), so
    /// it slides out as the work or the notification comes and back as
    /// it goes.
    pub fn sync_pulls(&mut self) {
        let stashed: Vec<WindowId> = self.shelved();
        self.pulled.retain(|w, _| stashed.contains(w));
        for w in stashed {
            let out = self.node.window_working(w) || self.window_notified(w);
            match self.pulled.get(&w) {
                Some(&(was, _)) if was == out => {}
                // first seen in, as it always was: nothing to slide
                None if !out => {}
                _ => {
                    self.pulled.insert(w, (out, Instant::now()));
                }
            }
        }
    }

    /// Is a card on its way out of the bunch or back?
    fn pulling(&self) -> bool {
        self.pulled.values().any(|(_, t)| t.elapsed().as_secs_f32() < PULL_IN)
    }

    /// How far each card (latest first) is drawn out of the bunch: a
    /// notified or working one under the top card, so its handle shows
    /// (`PULL`), its handle turning or filling -- sliding as
    /// that comes and goes (`sync_pulls`).
    fn shelf_pulls(&self, wins: &[WindowId]) -> Vec<f32> {
        let mut out = 0.;
        wins.iter()
            .enumerate()
            .map(|(i, &w)| {
                let k = match self.pulled.get(&w) {
                    _ if i == 0 => 0.,
                    Some(&(on, at)) => {
                        let e = ease((at.elapsed().as_secs_f32() / PULL_IN).clamp(0., 1.));
                        if on {
                            e
                        } else {
                            1. - e
                        }
                    }
                    None => 0.,
                };
                out += PULL * k;
                out
            })
            .collect()
    }

    /// The cards, at the right end of a bar `h` high and `bar_w` wide:
    /// over what is under them when fanned out, with the chosen one's
    /// preview below the bar.
    pub fn shelf(&self, h: f32, bar_w: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let wins = self.shelved();
        if wins.is_empty() {
            return None;
        }
        let t = crate::theme::theme();
        let n = wins.len();
        let (k, moving) = self.shelf.fan();
        // fanned out: side by side leftward from the right end, narrower
        // if they would take more than half the bar
        let full_w = self.shelf_card_w(cx);
        let fan_w = (((bar_w / 2.) / n as f32) - GAP).clamp(MIN_W.min(full_w), full_w);
        let card_w = full_w + (fan_w - full_w) * k;
        let pulls = self.shelf_pulls(&wins);
        let right_of = |i: usize| {
            let bunched = PEEK * i.min(PEEKS) as f32 + pulls[i];
            let fanned = (fan_w + GAP) * i as f32;
            bunched + (fanned - bunched) * k
        };
        let width = (0..n).map(right_of).fold(0., f32::max) + card_w;
        let top = (h - CARD_H) / 2.;
        let mut stack = div()
            .id("shelf")
            .absolute()
            .top(px(top - 4.))
            .right(px(8.))
            .w(px(width + 8.))
            .h(px(CARD_H + 8.))
            // gpui takes a key typed as the pointer gone until it moves;
            // the pointer is where it was, on the cards or the preview
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                this.shelf.touch(true, *on);
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, _, cx| {
                let d = e.delta.pixel_delta(px(CARD_H));
                let d = f32::from(if d.y.abs() >= d.x.abs() { d.y } else { -d.x });
                this.shelf.hover(true);
                this.shelf.scroll += d;
                let wins = this.shelved();
                let at = this.shelf.pick.and_then(|p| wins.iter().position(|&w| w == p));
                while this.shelf.scroll.abs() >= NOTCH {
                    let step = this.shelf.scroll.signum();
                    this.shelf.scroll -= step * NOTCH;
                    // down (or right) goes further back, to older ones
                    let i = match at {
                        None => 0,
                        Some(i) if step < 0. => (i + 1).min(wins.len() - 1),
                        Some(i) => i.saturating_sub(1),
                    };
                    this.shelf.pick = wins.get(i).copied();
                }
                cx.notify();
                cx.stop_propagation();
            }));
        let at = self.shelf.stack_at.clone();
        stack = stack.child(canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full());
        // the ground behind them as they fan over the bar
        if k > 0. {
            let ground = crate::text_element::ground(&t);
            stack = stack.child(div().absolute().top(px(0.)).right(px(0.)).size_full().rounded(px(8.)).bg(gpui::Hsla::from(rgb(ground)).opacity(k)));
        }
        // the oldest first, so the latest lies on top
        for (i, &w) in wins.iter().enumerate().rev() {
            let picked = self.shelf.pick == Some(w) && self.shelf.hovered;
            stack = stack.child(self.shelf_card(w, i, right_of(i) + 4., card_w, picked, k, cx));
        }
        // keep the fan moving until it settles, and close it once the
        // pointer has been off it a moment (not while a button is held:
        // a selection swept out of the preview)
        let me = cx.entity();
        let left = self.shelf.left_at;
        let pulling = self.pulling();
        let tick = canvas(
            move |_, window, cx| {
                if let Some(at) = left {
                    if at.elapsed() >= GRACE {
                        me.update(cx, |this, cx| {
                            if this.shelf.left_at == Some(at) && !this.held_any() {
                                this.shelf.left_at = None;
                                this.shelf.hover(false);
                                this.restack();
                                cx.notify();
                            }
                        });
                    }
                    window.request_animation_frame();
                }
            },
            move |_, _, window, _| {
                if moving || pulling {
                    window.request_animation_frame();
                }
            },
        )
        .size(px(0.));
        let mut out = div().absolute().top(px(0.)).right(px(0.)).size_full().child(stack).child(tick);
        if let Some(p) = self.shelf.pick.filter(|_| self.shelf.hovered && k > 0.5) {
            if let Some(i) = wins.iter().position(|&w| w == p) {
                let centre = bar_w - 8. - 4. - right_of(i) - card_w / 2.;
                out = out.child(self.shelf_preview(p, centre, h, bar_w, cx));
            }
        }
        Some(out.into_any_element())
    }

    /// Stashed window `w`'s card: its handle's dot and its name, `right`
    /// in from the stack's right end, `w_px` wide.
    fn shelf_card(&self, w: WindowId, i: usize, right: f32, w_px: f32, picked: bool, fan: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        // (notified, its handle says so)
        let bg = if picked {
            crate::theme::step(t.tag_bg, 1)
        } else {
            t.tag_bg
        };
        let label = self.shelf_label(w);
        let d = self.window_dot(w);
        // one shadow for the hand, not one a card: the top card's drops
        // onto the bar; each under it shows only its edge, a hairline of
        // shade where it peeks out from under the one over it -- a card's
        // own drop shadow coming back as the fan opens and they part
        let drop = |a: f32| gpui::BoxShadow { color: gpui::hsla(0., 0., 0., a), offset: gpui::point(px(0.), px(1.)), blur_radius: px(3.), spread_radius: px(0.), inset: false };
        let edge = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.10 * (1. - fan)), offset: gpui::point(px(-1.), px(0.)), blur_radius: px(1.5), spread_radius: px(0.), inset: false };
        let shadow = if i == 0 { vec![drop(0.14)] } else { vec![edge, drop(0.14 * fan)] };
        div()
            .id(("shelf-card", i))
            .absolute()
            .top(px(4.))
            .right(px(right))
            .w(px(w_px))
            .h(px(CARD_H))
            .rounded(px(6.))
            .bg(rgb(bg))
            .border_1()
            .border_color(rgb(if picked { t.accent } else { t.body_border }))
            .shadow(shadow)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .px(px(7.))
            .overflow_hidden()
            .font_family(crate::fonts::ui())
            .text_size(px(CARD_TEXT))
            .text_color(rgb(t.text))
            .cursor_default()
            .child(crate::sidebar::dot_element(&d))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                if *on {
                    this.shelf.pick = Some(w);
                    this.shelf.scroll = 0.;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.unstash(w, cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    /// Window `w` below the bar, at the size it had in its column
    /// (smaller only when the window has not the room), under its card
    /// at `centre`: its tag and body themselves, live, to work in as in
    /// any window -- a page's drawn from its replica.
    fn shelf_preview(&self, w: WindowId, centre: f32, h: f32, bar_w: f32, cx: &mut Context<Self>) -> AnyElement {
        use apex_core::{Body, ViewId};
        let t = crate::theme::theme();
        let l = &self.node.state.layout;
        let slot = l.stash.iter().find(|s| s.slot.window == w).map(|s| s.slot);
        let (room_w, room_h) = ((bar_w - 16.).max(80.), (l.r.dy() as f32 - 12.).max(80.));
        // as large as it stood, and never less than a useful size
        let (cw, ch) = slot.map(|s| (s.r.dx() as f32, s.r.dy() as f32)).unwrap_or((0., 0.));
        let (cw, ch) = (cw.max(PREVIEW_W.min(room_w)), ch.max(PREVIEW_H.max(l.r.dy() as f32 * PREVIEW_OF_H).min(room_h)));
        // unless the window below the bar is smaller
        let scale = (room_w / cw).min(room_h / ch).min(1.);
        let (pw, ph) = (cw * scale, ch * scale);
        let left = (centre - pw / 2.).clamp(8., (bar_w - pw - 8.).max(8.));
        let me = cx.entity();
        let font = f32::from(crate::text_element::tag_line_height());
        // the tag as many rows as it wraps to here, as it was last drawn
        // at this width (`tag_need`) -- not as it stood in its column, which
        // was another width, or, for a window made stashed (a diagnostic
        // one), never was; until it has been drawn, as it stood
        let most = (1. + ((ph / 2. - font) / f32::from(crate::text_element::tag_row_height())).floor()).max(1.) as i32;
        let need = self.tag_need.get(&ViewId::Tag(w)).map(|&(n, nl)| apex_core::tiling::taglines_rule(n as i32, nl, most));
        let stood = slot.filter(|s| s.body.dy() > 0).map(|s| (s.body.y0 - s.r.y0) as f32).unwrap_or(font + 1.);
        let row = f32::from(crate::text_element::tag_row_height());
        let tag_h = need.map_or(stood, |n| font + (n - 1).max(0) as f32 * row + 1.).clamp(font, (ph / 2.).max(font));
        // drawn at this width for the first time, or wrapping anew: once
        // more, at the height it now says
        let me_ = cx.entity();
        let settle = canvas(|_, _, _| {}, move |_, _, window, cx| {
            let now = me_.read(cx).tag_need.get(&ViewId::Tag(w)).map(|&(n, nl)| apex_core::tiling::taglines_rule(n as i32, nl, most));
            if now != need {
                window.refresh();
            }
        })
        .absolute()
        .size(px(0.));
        let body = self.node.state.window(w).map(|x| x.body).ok();
        let content: AnyElement = match body {
            Some(Body::Text(_)) | Some(Body::Term(_)) => {
                let tag = div().flex_none().w_full().h(px(tag_h - 1.)).child(crate::text_element::TextElement { acme: me.clone(), view: ViewId::Tag(w) });
                let inner: AnyElement = match body {
                    Some(Body::Term(term)) => crate::term_element::TermElement { acme: me.clone(), window: w, term }.into_any_element(),
                    _ => crate::text_element::TextElement { acme: me.clone(), view: ViewId::Body(w) }.into_any_element(),
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(tag)
                    .child(div().flex_none().w_full().h(px(1.)).bg(rgb(t.body_border)))
                    .child(div().flex_1().min_h_0().w_full().child(inner))
                    .into_any_element()
            }
            _ => {
                let mini = crate::miniature::snapshot_window(&self.node, w, cw, ch, &t);
                canvas(|_, _, _| {}, move |b, _, window, cx| mini.paint(b, window, cx)).size_full().into_any_element()
            }
        };
        let at = self.shelf.preview_at.clone();
        let mark = canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full();
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false };
        gpui::deferred(
            div()
                .id("shelf-preview")
                .absolute()
                .top(px(h + 6.))
                .left(px(left))
                .w(px(pw))
                .h(px(ph))
                .rounded(px(crate::text_element::CARD_RADIUS))
                .overflow_hidden()
                .bg(rgb(t.body_bg))
                .border_1()
                .border_color(rgb(t.body_border))
                .shadow(vec![shadow])
                .cursor(gpui::CursorStyle::Arrow)
                .child(self.overlay_mark())
                .child(mark)
                .child(content)
                .child(settle)
                // typing into it is no leaving it
                .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
                .on_hover(cx.listener(|this, on: &bool, _, cx| {
                    this.shelf.touch(false, *on);
                    cx.notify();
                })),
        )
        .with_priority(2)
        .into_any_element()
    }
}
