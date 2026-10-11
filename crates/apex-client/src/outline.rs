//! A page's outline in its scrollbar: the scrollbar over a page from a
//! buffer's right edge (a preview, a diff) is acme's, as a text's, and carries the
//! page's headings (`web::Toc`) as ticks down it -- the heading the view
//! is in in the accent, the others faint, longer the higher the heading.
//! The pointer in the bar brings the outline out beside it, the heading
//! whose tick is nearest the pointer marked; a heading chosen there is
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

/// How far a tick reaches across the bar, by its heading's level.
const TICK: [f32; 5] = [0., 9., 7., 5., 3.];

/// The outline's state in the app: which page's is out, the heading near
/// the pointer, where the panel was drawn, and its list's scroll.
#[derive(Default)]
pub struct Outline {
    pub open: Option<WindowId>,
    pub near: Option<usize>,
    pub panel_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub scroll: gpui::ScrollHandle,
}

/// The ticks down the bar: each heading where it is in the page
/// (`toc.at`), as long as its level says, the current one in the accent,
/// the others faint (less so with the lane open); as `web::BarLook` has
/// them.
pub fn ticks(toc: &Toc, scroll: Option<(f64, f64, f64)>, current: Option<usize>, open: bool) -> Vec<(f32, f32, u32, f32)> {
    let t = crate::theme::theme();
    (0..toc.heads.len())
        .map(|i| {
            let len = TICK[toc.heads[i].level.min(4) as usize];
            let (rgb, alpha) = if current == Some(i) { (t.accent, 1.) } else { (t.text_dim, if open { 0.75 } else { 0.4 }) };
            (toc.at(i, scroll), len, rgb, alpha)
        })
        .collect()
}

/// The heading whose tick is nearest `y` in `bar`.
pub fn nearest(toc: &Toc, scroll: Option<(f64, f64, f64)>, bar: Bounds<Pixels>, y: Pixels) -> Option<usize> {
    let at = ((y - bar.top()) / bar.size.height).clamp(0., 1.);
    (0..toc.heads.len()).min_by(|a, b| (toc.at(*a, scroll) - at).abs().total_cmp(&(toc.at(*b, scroll) - at).abs()))
}

impl Acme {
    /// The pointer moved: the outline out while it is in a page's bar
    /// (one with an outline), or on the outline itself; the heading near
    /// it marked. Whether anything changed.
    pub fn outline_hover(&mut self, pos: Point<Pixels>) -> bool {
        let in_bar = self.web_bars.iter().find(|(w, b)| b.contains(&pos) && self.webs.toc(**w).is_some()).map(|(w, b)| (*w, *b));
        let on_panel = self.outline.panel_at.get().is_some_and(|b| b.dilate(px(4.)).contains(&pos));
        let (open, near) = match in_bar {
            Some((w, bar)) => {
                let (toc, _) = self.webs.toc(w).unwrap();
                (Some(w), nearest(toc, self.webs.page_scroll(w), bar, pos.y))
            }
            None if on_panel => (self.outline.open, self.outline.near),
            None => (None, None),
        };
        let changed = open != self.outline.open || near != self.outline.near;
        if open != self.outline.open {
            crate::shell::log_line(&format!("outline: {}", open.map_or("in".to_string(), |w| format!("out on page {w}"))));
        }
        if open.is_some() && open != self.outline.open {
            // out afresh: its list where the view is
            if let Some((_, Some(c))) = open.and_then(|w| self.webs.toc(w)) {
                self.outline.scroll.scroll_to_item(c);
            }
        }
        if open.is_none() {
            self.outline.panel_at.set(None);
        }
        (self.outline.open, self.outline.near) = (open, near);
        changed
    }

    /// The outline beside the bar of the page whose it is: a heading a
    /// row, indented by level, the view's in the accent, the pointer's
    /// nearest washed; B1 on one, the page there.
    pub fn outline_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = self.outline.open?;
        let bar = *self.web_bars.get(&w)?;
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
        // a height of its own, as tall as its rows and no taller than the
        // bar: a list that scrolls has none it would take from its rows
        let tall = (px(ROW * toc.heads.len() as f32 + 14.)).min(bar.size.height - px(16.));
        let panel = div()
            .id("toc-panel")
            .relative()
            .flex()
            .flex_col()
            .min_w(px(170.))
            .max_w(px(320.))
            .h(tall)
            .overflow_y_scroll()
            .track_scroll(&self.outline.scroll)
            .p(px(6.))
            .rounded(px(10.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .font_family(crate::fonts::ui())
            .text_size(px(12.5))
            .child(self.overlay_mark_by(px(0.)))
            .child(gpui::canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full())
            .children(rows);
        // left of the bar, in the middle of it, as the rail's was
        let top = (bar.top() + (bar.size.height - tall) / 2.).max(bar.top() + px(8.));
        Some(deferred(anchored().anchor(gpui::Anchor::TopRight).position(point(bar.left() - px(4.), top)).child(panel)).with_priority(2).into_any_element())
    }
}
