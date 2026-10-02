//! A column is a strip two ways, as a window is shown two ways when it
//! has not its room. Stashed (B3 on its box) it stands at the row's
//! right, as a window stashed goes to its column's foot: drawn as the
//! edges of sheets stood on their sides, the whole of it its column's
//! box -- B1 brings it back where it stood, B2 back and maximized, a drag
//! moves it. The pointer on it brings out a slice of the column, live,
//! as wide as it would come back, beside the strip; a click there brings
//! it back. Minimized (B2 on another's box, a drag, a neighbour's growth)
//! it stands where it is, among the others in their order, as a folded
//! window keeps its tag: a slim card on its side, its outline rounded,
//! the column's grip at its top where the column's tag would be (it is
//! the column's box, and says it is a column), each window's handle down
//! it where the window stands. A click on a
//! handle brings the column back and lands on that window; anywhere else
//! on it is the column's box.

use gpui::prelude::*;
use gpui::{canvas, div, px, rgb, AnyElement, Context, MouseButton, Pixels, Point};

use apex_core::state::Layout;
use apex_core::{tiling, ColumnId};

use crate::app::Acme;

impl Acme {
    /// The strip under the pointer (`p`, in the window's coordinates), its
    /// slice out at once, and put away a moment after the pointer leaves
    /// both. True when it came or went.
    pub fn strip_tick(&mut self, p: Point<Pixels>, held: bool) -> bool {
        let (x, y) = self.row_pt(p);
        let l = &self.node.state.layout;
        // stashed ones only: a minimized one shows its handles
        let on_strip = (0..l.cols.len()).find(|&ci| l.shows(ci) && l.cols[ci].stashed && tiling::is_strip(l.cols[ci].r) && l.cols[ci].r.contains(x, y)).map(|ci| l.cols[ci].id);
        if let Some(open) = self.strip_open {
            let over_slice = self.strip_slice_rect(open).is_some_and(|r| r.contains(x, y));
            if on_strip == Some(open) || over_slice || held {
                self.strip_leaving = None;
                return false;
            }
            if on_strip.is_some() {
                self.strip_open = on_strip;
                self.strip_leaving = None;
                return true;
            }
            return match self.strip_leaving {
                None => {
                    self.strip_leaving = Some(std::time::Instant::now());
                    false
                }
                Some(t) if t.elapsed() >= std::time::Duration::from_millis(150) => {
                    self.strip_open = None;
                    self.strip_leaving = None;
                    true
                }
                Some(_) => false,
            };
        }
        if held || on_strip.is_none() {
            return false;
        }
        self.strip_open = on_strip;
        true
    }

    /// Where strip `c`'s slice goes, in the area's coordinates: as wide as
    /// the column would come back, beside the strip on the side with
    /// more room.
    pub fn strip_slice_rect(&self, c: ColumnId) -> Option<tiling::Rect> {
        let l = &self.node.state.layout;
        let col = l.column(c)?;
        let row = l.r.dx();
        let had = if col.restore > 0 { (col.restore as i64 * row as i64 / 1_000_000) as i32 } else { row / 3 };
        let w = had.clamp(240, (row / 2).max(240));
        let right = col.r.x0 < l.r.x0 + row / 2;
        let x0 = if right { col.r.x1 + 4 } else { col.r.x0 - 4 - w };
        Some(tiling::Rect::new(x0, col.r.y0, x0 + w, col.r.y1))
    }

    /// Strip `ci`'s drawing: the edges of sheets on their sides, the whole
    /// of it its column's box.
    pub fn strip_element(&self, ci: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let c = self.node.state.layout.cols[ci].id;
        let open = self.strip_open == Some(c);
        let notified = self.node.state.layout.cols[ci].wins.iter().any(|s| self.window_notified(s.window));
        let edge = if notified { crate::text_element::mix(t.tag_bg, t.accent, 0.12) } else if open { crate::theme::step(t.tag_bg, 1) } else { t.tag_bg };
        let line = t.body_border;
        let sheets = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                // three edges, the nearest widest, the others behind it
                for i in (0..3).rev() {
                    let inset = px(1. + 2. * i as f32);
                    let r = gpui::Bounds::new(gpui::point(b.left() + inset, b.top() + px(2. + 3. * i as f32)), gpui::size(b.size.width - inset * 2., b.size.height - px(4. + 6. * i as f32)));
                    window.paint_quad(gpui::quad(r, px(4.), gpui::rgb(edge), px(1.), gpui::rgb(line), gpui::BorderStyle::Solid));
                }
            },
        )
        .size_full();
        let press = |b: MouseButton| cx.listener(move |this: &mut Acme, e: &gpui::MouseDownEvent, _, cx| this.press_col_box(c, b, e.position, e.modifiers.shift, cx));
        div()
            .id(("strip", ci))
            .size_full()
            .cursor(gpui::CursorStyle::OpenHand)
            .child(sheets)
            .on_mouse_down(MouseButton::Left, press(MouseButton::Left))
            .on_mouse_down(MouseButton::Middle, press(MouseButton::Middle))
            .on_mouse_down(MouseButton::Right, press(MouseButton::Right))
            .into_any_element()
    }

    /// Window `w`'s handle as its tag shows it.
    pub(crate) fn window_dot(&self, w: apex_core::WindowId) -> crate::text_element::Dot {
        let t = crate::theme::theme();
        let live = self.node.window_live(w) || self.node.state.window(w).is_ok_and(|x| x.body == apex_core::Body::Web);
        crate::text_element::dot(&t, false, self.node.window_unsaved(w), live, self.node.window_working(w), self.note_age(w)).at(self.node.window_progress(w))
    }

    /// Minimized column `ci`'s drawing: a slim card on its side, where it
    /// stands, each window's handle down it where the window is.
    pub fn minimized_element(&self, ci: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let col = &self.node.state.layout.cols[ci];
        let c = col.id;
        // (a notified window's handle down it says so)
        let card = t.tag_bg;
        let line = t.body_border;
        let grip = crate::text_element::rgb(t.text_dim);
        let font = f32::from(crate::text_element::tag_line_height());
        let handles: Vec<(apex_core::WindowId, f32)> = col.wins.iter().map(|s| (s.window, (s.r.y0 - col.r.y0) as f32)).collect();
        let dots: Vec<(f32, crate::text_element::Dot)> = handles.iter().map(|&(w, y)| (y + font / 2., self.window_dot(w))).collect();
        let drawn = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let r = gpui::Bounds::new(gpui::point(b.left() + px(1.), b.top() + px(1.)), gpui::size(b.size.width - px(2.), b.size.height - px(2.)));
                window.paint_quad(gpui::quad(r, px(5.), gpui::rgb(card), px(1.), gpui::rgb(line), gpui::BorderStyle::Solid));
                // the column's grip on its tag's row, as a column tag has it
                crate::text_element::paint_grip(window, gpui::Bounds::new(b.origin, gpui::size(b.size.width, px(font))), grip);
                for (y, d) in &dots {
                    crate::text_element::paint_dot(window, d, gpui::point(b.left() + b.size.width / 2., b.top() + px(*y)));
                }
            },
        )
        .size_full();
        let press = |b: MouseButton| cx.listener(move |this: &mut Acme, e: &gpui::MouseDownEvent, _, cx| this.press_col_box(c, b, e.position, e.modifiers.shift, cx));
        let mut el = div()
            .id(("minimized", ci))
            .relative()
            .size_full()
            .cursor(gpui::CursorStyle::OpenHand)
            .child(drawn)
            .on_mouse_down(MouseButton::Left, press(MouseButton::Left))
            .on_mouse_down(MouseButton::Middle, press(MouseButton::Middle))
            .on_mouse_down(MouseButton::Right, press(MouseButton::Right));
        // each handle its window's box, as a folded window's is
        for (k, &(w, y)) in handles.iter().enumerate() {
            let hit = |b: MouseButton| cx.listener(move |this: &mut Acme, e: &gpui::MouseDownEvent, _, cx| this.press_handle(w, b, e.position, e.modifiers.shift, cx));
            el = el.child(
                div()
                    .id(("minimized-handle", ci * 1000 + k))
                    .absolute()
                    .left(px(0.))
                    .top(px(y))
                    .w_full()
                    .h(px(font))
                    .on_mouse_down(MouseButton::Left, hit(MouseButton::Left))
                    .on_mouse_down(MouseButton::Middle, hit(MouseButton::Middle))
                    .on_mouse_down(MouseButton::Right, hit(MouseButton::Right)),
            );
        }
        el.into_any_element()
    }

    /// The slice of the strip under the pointer: the column, live, as it
    /// would stand brought back; a click brings it back.
    pub fn strip_slice(&self, l: &Layout, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.strip_open?;
        let ci = l.column_index(c)?;
        let r = self.strip_slice_rect(c)?;
        let t = crate::theme::theme();
        let (mini, _) = crate::miniature::snapshot_column_at(&self.node, ci, Some(r.dx() as f32), &t)?;
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false };
        let body = canvas(|_, _, _| {}, move |b, _, window, cx| mini.paint(b, window, cx)).size_full();
        let slice = div()
            .id(("strip-slice", ci))
            .absolute()
            .left(px(r.x0 as f32))
            .top(px(r.y0 as f32))
            .w(px(r.dx() as f32))
            .h(px(r.dy() as f32))
            .rounded(px(crate::text_element::CARD_RADIUS))
            .overflow_hidden()
            .bg(rgb(t.body_bg))
            .border(px(0.5))
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .flex()
            .flex_col()
            .cursor(gpui::CursorStyle::PointingHand)
            .child(div().flex_none().px(px(10.)).py(px(4.)).text_size(px(12.)).text_color(rgb(t.text_dim)).bg(rgb(t.tag_bg)).truncate().font_family(crate::fonts::ui()).child("Put away — click to bring back"))
            .child(div().flex_1().relative().child(body))
            .child(self.overlay_mark())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    this.strip_open = None;
                    this.bring_back_column(c, e.position, cx);
                    cx.stop_propagation();
                }),
            );
        Some(slice.into_any_element())
    }
}
