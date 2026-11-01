//! A page's outline in its scrollbar: the scrollbar down a page's left
//! (a preview's, a diff's) is acme's, as a text's, and carries the
//! page's headings (`web::Toc`) as stops down it, dots on the thumb's
//! axis where each heading is, over the thumb as over the lane -- the
//! heading the view is in in the accent, the others dark, bigger the
//! higher the heading. The bar is for scrolling first: the pointer in it
//! brings out only a pill by the accent's stop, naming the part being
//! read, as a text's bar says its lines; the pointer onto the pill opens
//! it into the whole outline, that heading level with where the pill
//! was, and a heading chosen there is where the page goes. Back in the
//! bar, it is a pill again. The bar's buttons are acme's as ever.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Bounds, Context, MouseButton, Pixels, Point};

use apex_core::WindowId;

use crate::app::Acme;
use crate::web::{Toc, TocHead};

/// A row of the outline's list.
const ROW: f32 = 23.;

/// The list's padding, its border's included.
const PAD: f32 = 7.;

/// How big a stop is across, by its heading's level: a first nearly as
/// wide as the thumb (5), smaller down to a fourth. Whole points, an odd
/// number short of the thumb's, so a stop is centred on it to the pixel.
const STOP: [f32; 5] = [0., 4., 3., 2., 2.];

/// How tall the pill naming the part being read is.
const PILL: f32 = 20.;

/// The outline's state in the app: which page's is out, its bar (as the
/// pointer last found it: the bars are laid out after the panel is made),
/// whether it is the whole list or the pill, the list's top, where the
/// pill or the list was drawn, and the list's scroll.
#[derive(Default)]
pub struct Outline {
    pub open: Option<WindowId>,
    pub bar: Option<Bounds<Pixels>>,
    pub whole: bool,
    pub top: Pixels,
    pub panel_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub scroll: gpui::ScrollHandle,
}

/// The stops down the bar: each heading where it is in the page
/// (`toc.at`), as big as its level says, the current one in the accent,
/// the others darker than the thumb at its darkest, whatever they are
/// over; as `paint_lane` has them.
pub fn stops(toc: &Toc, scroll: Option<(f64, f64, f64)>, current: Option<usize>) -> Vec<(f32, f32, u32, f32)> {
    let t = crate::theme::theme();
    (0..toc.heads.len())
        .map(|i| {
            let d = STOP[toc.heads[i].level.min(4) as usize];
            let (rgb, alpha) = if current == Some(i) { (t.accent, 1.) } else { (crate::text_element::mix(t.text_dim, t.text, 0.5), 1.) };
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

/// Where heading `i`'s stop is down `bar`, as `paint_lane` draws it.
fn stop_y(toc: &Toc, scroll: Option<(f64, f64, f64)>, bar: Bounds<Pixels>, i: usize) -> Pixels {
    bar.top() + px(3.) + (bar.size.height - px(6.)) * toc.at(i, scroll)
}

/// A heading's title as the outline shows it: a path by its end, which
/// says most, when it is long.
fn title(h: &TocHead) -> String {
    if h.path && h.title.chars().count() > 44 {
        format!("…{}", h.title.chars().skip(h.title.chars().count() - 43).collect::<String>())
    } else {
        h.title.clone()
    }
}

impl Acme {
    /// The pointer moved: the outline's pill out while it is in a page's
    /// bar (one with an outline); onto the pill, the whole outline from
    /// it, staying while the pointer is on it; back in the bar, the pill
    /// again. Whether anything changed.
    pub fn outline_hover(&mut self, pos: Point<Pixels>) -> bool {
        let in_bar = self.web_bars.iter().find(|(w, b)| b.contains(&pos) && self.webs.toc(**w).is_some()).map(|(w, b)| (*w, *b));
        let on_it = self.outline.panel_at.get().is_some_and(|b| b.dilate(px(4.)).contains(&pos));
        let was = (self.outline.open, self.outline.whole);
        match in_bar {
            Some((w, bar)) => {
                self.outline.bar = Some(bar);
                (self.outline.open, self.outline.whole) = (Some(w), false);
            }
            None if on_it && self.outline.open.is_some() => {
                if !self.outline.whole {
                    self.outline.whole = true;
                    self.place_outline();
                }
            }
            None => (self.outline.open, self.outline.whole) = (None, false),
        }
        if self.outline.open.is_none() {
            self.outline.panel_at.set(None);
        }
        was != (self.outline.open, self.outline.whole)
    }

    /// The whole outline placed where the pill was: the heading being
    /// read level with its stop (the list scrolled to it where the panel
    /// can go no higher, or no lower).
    fn place_outline(&mut self) {
        let (Some(w), Some(bar)) = (self.outline.open, self.outline.bar) else { return };
        let Some((toc, current)) = self.webs.toc(w) else { return };
        let i = current.unwrap_or(0);
        let y = stop_y(toc, self.webs.page_scroll(w), bar, i);
        let (n, tall) = (toc.heads.len(), tall(toc.heads.len(), bar));
        let row = px(PAD + ROW * (i as f32 + 0.5));
        let top = (y - row).min(bar.bottom() - px(8.) - tall).max(bar.top() + px(8.));
        let off = (top + row - y).max(px(0.)).min(px(ROW * n as f32 + 2. * PAD) - tall);
        self.outline.top = top;
        self.outline.scroll.set_offset(point(px(0.), -off));
    }

    /// The outline beside the bar of the page whose it is: its pill
    /// (`outline_pill`), or the whole of it, a heading a row, indented by
    /// level, the view's in the accent; B1 on one, the page there.
    pub fn outline_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = self.outline.open?;
        let bar = self.outline.bar?;
        let (toc, current) = self.webs.toc(w)?;
        if !self.outline.whole {
            return self.outline_pill(w, bar);
        }
        let t = crate::theme::theme();
        let at = self.outline.panel_at.clone();
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.16), offset: point(px(0.), px(8.)), blur_radius: px(28.), spread_radius: px(0.), inset: false };
        let rows = toc.heads.iter().enumerate().map(|(i, h)| {
            let here = current == Some(i);
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
                .hover(|d| d.bg(rgb(t.tag_bg)))
                .child(title(h))
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

    /// The outline's pill: the part being read, by its stop, as a text's
    /// bar has its lines on pills by its thumb; the pointer onto it opens
    /// the whole outline (`outline_hover`).
    fn outline_pill(&self, w: WindowId, bar: Bounds<Pixels>) -> Option<AnyElement> {
        let (toc, current) = self.webs.toc(w)?;
        let i = current.unwrap_or(0);
        let head = toc.heads.get(i)?;
        let t = crate::theme::theme();
        let at = self.outline.panel_at.clone();
        let y = stop_y(toc, self.webs.page_scroll(w), bar, i);
        let top = (y - px(PILL / 2.)).min(bar.bottom() - px(PILL)).max(bar.top());
        let pill = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(PILL))
            .max_w(px(320.))
            .px(px(8.))
            .rounded(px(PILL / 2.))
            .bg(rgb(t.tag_bg))
            .border_1()
            .border_color(rgb(t.body_border))
            .font_family(crate::fonts::ui())
            .text_size(px(11.5))
            .text_color(rgb(t.text_dim))
            .whitespace_nowrap()
            .child(self.overlay_mark_by(px(0.)))
            .child(gpui::canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full())
            // the accent's mark: the stop it names
            .child(div().flex_none().text_color(rgb(t.accent)).child("§"))
            .child(div().min_w_0().overflow_hidden().text_ellipsis().child(title(head)))
            .child(div().flex_none().child("›"));
        Some(deferred(anchored().anchor(gpui::Anchor::TopLeft).position(point(bar.right() + px(2.), top)).child(pill)).with_priority(2).into_any_element())
    }
}
