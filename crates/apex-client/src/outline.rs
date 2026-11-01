//! A page's outline in its scrollbar: the scrollbar down a page's left
//! (a preview's, a diff's) is acme's, as a text's, and carries the
//! page's headings (`web::Toc`) as stops down it, dots on the thumb's
//! axis where each heading is, holes in the thumb under it -- the heading
//! the view is in in the accent, the others faint, bigger the higher the
//! heading. The pointer in the bar brings the outline out beside it,
//! over the page, the heading whose stop is nearest the pointer marked; a heading chosen there is
//! where the page goes. The bar's buttons are acme's as ever.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Bounds, Context, MouseButton, Pixels, Point};

use apex_core::WindowId;

use crate::app::Acme;
use crate::web::Toc;

/// A row of the outline's list.
const ROW: f32 = 23.;

/// The list's padding, its border's included.
const PAD: f32 = 7.;

/// How big a stop is across, by its heading's level: a first as wide as
/// the thumb (5), smaller down to a fourth.
const STOP: [f32; 5] = [0., 4.5, 3.5, 2.5, 2.];

/// The outline's state in the app: which page's is out, its bar (as the
/// pointer last found it: the bars are laid out after the panel is made),
/// the panel's top, the heading near the pointer, where the panel was
/// drawn, and its list's scroll.
#[derive(Default)]
pub struct Outline {
    pub open: Option<WindowId>,
    pub bar: Option<Bounds<Pixels>>,
    pub top: Pixels,
    pub near: Option<usize>,
    pub panel_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub scroll: gpui::ScrollHandle,
}

/// The stops down the bar: each heading where it is in the page
/// (`toc.at`), as big as its level says, the current one in the accent,
/// the others faint (less so with the lane open); as `paint_lane` has
/// them.
pub fn stops(toc: &Toc, scroll: Option<(f64, f64, f64)>, current: Option<usize>, open: bool) -> Vec<(f32, f32, u32, f32)> {
    let t = crate::theme::theme();
    (0..toc.heads.len())
        .map(|i| {
            let d = STOP[toc.heads[i].level.min(4) as usize];
            let (rgb, alpha) = if current == Some(i) { (t.accent, 1.) } else { (t.text_dim, if open { 0.75 } else { 0.4 }) };
            (toc.at(i, scroll), d, rgb, alpha)
        })
        .collect()
}

/// How tall the panel of `n` headings is by `bar`: as its rows, no
/// taller than the bar; a list that scrolls has no height of its own it
/// would take from its rows.
fn tall(n: usize, bar: Bounds<Pixels>) -> Pixels {
    px(ROW * n as f32 + 2. * PAD).min(bar.size.height - px(16.))
}

/// The heading whose stop is nearest `y` in `bar`.
pub fn nearest(toc: &Toc, scroll: Option<(f64, f64, f64)>, bar: Bounds<Pixels>, y: Pixels) -> Option<usize> {
    let at = ((y - bar.top()) / bar.size.height).clamp(0., 1.);
    (0..toc.heads.len()).min_by(|a, b| (toc.at(*a, scroll) - at).abs().total_cmp(&(toc.at(*b, scroll) - at).abs()))
}

impl Acme {
    /// The pointer moved: the outline out while it is in a page's bar
    /// (one with an outline), or on the outline itself. In the bar, the
    /// outline follows it, the heading nearest it marked and level with
    /// it (the list scrolled to it where the panel can go no higher); on
    /// the outline, the outline stays where it is, to be chosen from.
    /// Whether anything changed.
    pub fn outline_hover(&mut self, pos: Point<Pixels>) -> bool {
        let in_bar = self.web_bars.iter().find(|(w, b)| b.contains(&pos) && self.webs.toc(**w).is_some()).map(|(w, b)| (*w, *b));
        let on_panel = self.outline.panel_at.get().is_some_and(|b| b.dilate(px(4.)).contains(&pos));
        let mut moved = false;
        let (open, near) = match in_bar {
            Some((w, bar)) => {
                self.outline.bar = Some(bar);
                let (toc, _) = self.webs.toc(w).unwrap();
                let near = nearest(toc, self.webs.page_scroll(w), bar, pos.y);
                let (n, tall) = (toc.heads.len(), tall(toc.heads.len(), bar));
                let row = px(PAD + ROW * (near.unwrap_or(0) as f32 + 0.5));
                let top = (pos.y - row).min(bar.bottom() - px(8.) - tall).max(bar.top() + px(8.));
                let off = (top + row - pos.y).max(px(0.)).min(px(ROW * n as f32 + 2. * PAD) - tall);
                if top != self.outline.top || off != -self.outline.scroll.offset().y {
                    self.outline.top = top;
                    self.outline.scroll.set_offset(point(px(0.), -off));
                    moved = true;
                }
                (Some(w), near)
            }
            None if on_panel => (self.outline.open, None),
            None => (None, None),
        };
        let changed = moved || open != self.outline.open || near != self.outline.near;
        if open.is_none() {
            self.outline.panel_at.set(None);
        }
        (self.outline.open, self.outline.near) = (open, near);
        changed
    }

    /// The outline beside the bar of the page whose it is, by the
    /// pointer: a heading a row, indented by level, the view's in the
    /// accent, the pointer's nearest washed; B1 on one, the page there.
    pub fn outline_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = self.outline.open?;
        let bar = self.outline.bar?;
        let (toc, current) = self.webs.toc(w)?;
        let t = crate::theme::theme();
        let at = self.outline.panel_at.clone();
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.16), offset: point(px(0.), px(8.)), blur_radius: px(28.), spread_radius: px(0.), inset: false };
        let rows = toc.heads.iter().enumerate().map(|(i, h)| {
            let title = if h.path && h.title.chars().count() > 44 {
                // a path: its end says most
                format!("…{}", h.title.chars().skip(h.title.chars().count() - 43).collect::<String>())
            } else {
                h.title.clone()
            };
            let here = current == Some(i);
            let near = self.outline.near == Some(i);
            div()
                .id(("toc", i))
                .flex_none()
                .h(px(ROW))
                .flex()
                .items_center()
                .px(px(8.))
                .pl(px(8. + 12. * (h.level.saturating_sub(1)) as f32))
                .rounded(px(6.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .cursor_pointer()
                .text_color(rgb(if here { t.accent } else { t.panel_text }))
                .when(h.level == 1 || here, |d| d.font_weight(crate::fonts::weight(gpui::FontWeight::SEMIBOLD)))
                .when(near, |d| d.bg(rgb(t.tag_bg)))
                .hover(|d| d.bg(rgb(t.tag_bg)))
                .child(title)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |acme, _, _, cx| {
                        acme.webs.toc_go(w, i);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
        });
        let tall = tall(toc.heads.len(), bar);
        // the list scrolls in the panel; what marks where the panel is
        // (its hole in the page, `panel_at`) is the panel's, and stays
        let list = div()
            .id("toc-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.outline.scroll)
            .p(px(PAD - 1.))
            .children(rows);
        let panel = div()
            .relative()
            .flex()
            .flex_col()
            .min_w(px(170.))
            .max_w(px(320.))
            .h(tall)
            .overflow_hidden()
            .rounded(px(10.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .font_family(crate::fonts::ui())
            .text_size(px(12.5))
            .child(self.overlay_mark_by(px(0.)))
            .child(gpui::canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full())
            .child(list);
        Some(deferred(anchored().anchor(gpui::Anchor::TopLeft).position(point(bar.right() + px(2.), self.outline.top)).child(panel)).with_priority(2).into_any_element())
    }
}
