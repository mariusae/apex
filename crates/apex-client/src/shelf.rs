//! The stash, in the title bar: the windows put away (⌘M, `Stash`) as
//! their tags made small, bunched at the bar's right end like a hand of
//! cards, the latest on top. The pointer on them, or a scroll over
//! them, fans them out, and the one under the pointer (or scrolled to)
//! shows live below the bar; a click on one brings it back where it
//! was. The sidebar lists them too. Going to a stashed window any other
//! way (Look, the plumber, a notification) brings it back as well.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{canvas, div, px, rgb, AnyElement, Context, MouseButton, ScrollWheelEvent, Window};

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
/// Between the cards fanned out, and the narrowest they get to fit.
const GAP: f32 = 4.;
const MIN_W: f32 = 72.;
/// How long the fan takes to open or close.
const FAN: Duration = Duration::from_millis(160);
/// A scroll this far moves the choice one card.
const NOTCH: f32 = 24.;
/// The preview's size when the window's own is not known.
const PREVIEW_W: f32 = 440.;
const PREVIEW_H: f32 = 320.;

#[derive(Default)]
pub struct Shelf {
    /// The pointer is on the cards.
    pub hovered: bool,
    /// When the fan last began to open or close.
    since: Option<Instant>,
    /// The card chosen: under the pointer, or scrolled to.
    pub pick: Option<WindowId>,
    /// Scrolled, not yet a whole notch.
    scroll: f32,
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

    /// Stashed window `w`'s card's name: the last part of its name, or
    /// what it is when it has none.
    fn shelf_label(&self, w: WindowId) -> String {
        let name = self.node.window_name(w);
        if !name.is_empty() {
            return crate::sidebar::split_name(&name).0;
        }
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
            self.shelf_card_w(cx) + PEEK * (n - 1).min(PEEKS) as f32 + 20.
        }
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
        let right_of = |i: usize| {
            let bunched = PEEK * i.min(PEEKS) as f32;
            let fanned = (fan_w + GAP) * i as f32;
            bunched + (fanned - bunched) * k
        };
        let width = right_of(n - 1) + card_w;
        let top = (h - CARD_H) / 2.;
        let mut stack = div()
            .id("shelf")
            .absolute()
            .top(px(top - 4.))
            .right(px(8.))
            .w(px(width + 8.))
            .h(px(CARD_H + 8.))
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                this.shelf.hover(*on);
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
        // the ground behind them as they fan over the bar
        if k > 0. {
            let ground = crate::text_element::ground(&t);
            stack = stack.child(div().absolute().top(px(0.)).right(px(0.)).size_full().rounded(px(8.)).bg(gpui::Hsla::from(rgb(ground)).opacity(k)));
        }
        // the oldest first, so the latest lies on top
        for (i, &w) in wins.iter().enumerate().rev() {
            let picked = self.shelf.pick == Some(w) && self.shelf.hovered;
            stack = stack.child(self.shelf_card(w, i, right_of(i) + 4., card_w, picked, cx));
        }
        // keep the fan moving until it settles
        let tick = canvas(|_, _, _| {}, move |_, _, window, _| {
            if moving {
                window.request_animation_frame();
            }
        })
        .size(px(0.));
        let mut out = div().absolute().top(px(0.)).right(px(0.)).size_full().child(stack).child(tick);
        if let Some(p) = self.shelf.pick.filter(|_| self.shelf.hovered && k > 0.5) {
            if let Some(i) = wins.iter().position(|&w| w == p) {
                let centre = bar_w - 8. - 4. - right_of(i) - card_w / 2.;
                out = out.child(self.shelf_preview(p, centre, h, bar_w));
            }
        }
        Some(out.into_any_element())
    }

    /// Stashed window `w`'s card: its handle's dot and its name, `right`
    /// in from the stack's right end, `w_px` wide.
    fn shelf_card(&self, w: WindowId, i: usize, right: f32, w_px: f32, picked: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let notified = self.window_notified(w);
        let bg = if notified {
            crate::text_element::mix(t.tag_bg, t.accent, 0.10)
        } else if picked {
            crate::theme::step(t.tag_bg, 1)
        } else {
            t.tag_bg
        };
        let label = self.shelf_label(w);
        let d = self.window_dot(w);
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.14), offset: gpui::point(px(0.), px(1.)), blur_radius: px(3.), spread_radius: px(0.), inset: false };
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
            .shadow(vec![shadow])
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

    /// Window `w` drawn live below the bar, at the size it had in its
    /// column (smaller only when the window has not the room), under its
    /// card at `centre`.
    fn shelf_preview(&self, w: WindowId, centre: f32, h: f32, bar_w: f32) -> AnyElement {
        let t = crate::theme::theme();
        let l = &self.node.state.layout;
        let (cw, ch) = l.stash.iter().find(|s| s.slot.window == w).map(|s| (s.slot.r.dx() as f32, s.slot.r.dy() as f32)).filter(|&(x, y)| x > 40. && y > 40.).unwrap_or((PREVIEW_W, PREVIEW_H));
        // as large as it stood, unless the window below the bar is smaller
        let (room_w, room_h) = ((bar_w - 16.).max(80.), (l.r.dy() as f32 - 12.).max(80.));
        let scale = (room_w / cw).min(room_h / ch).min(1.);
        let (pw, ph) = (cw * scale, ch * scale);
        let left = (centre - pw / 2.).clamp(8., (bar_w - pw - 8.).max(8.));
        let mini = crate::miniature::snapshot_window(&self.node, w, cw, ch, &t);
        let body = canvas(|_, _, _| {}, move |b, _, window, cx| mini.paint(b, window, cx)).size_full();
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false };
        gpui::deferred(
            div()
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
                .child(self.overlay_mark())
                .child(body),
        )
        .with_priority(2)
        .into_any_element()
    }
}
