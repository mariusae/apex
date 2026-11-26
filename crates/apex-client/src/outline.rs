//! A window's outline in its scrollbar: a page's headings, or the
//! outline a tool gives a text (`Buffer::outline`, a language server's
//! symbols), shown alike, a text's lines standing for a page's height.
//!
//! The scrollbar down a page's left
//! (a preview's, a diff's) is acme's, as a text's, and carries the
//! page's headings (`web::Toc`) as stops down it, dots on the thumb's
//! axis where each heading is, over the thumb as over the lane -- the
//! heading the view is in in the accent, the others dark, bigger the
//! higher the heading. The bar is for scrolling first: the pointer in it
//! brings out only a pill by the accent's stop, naming the part being
//! read; the pointer onto the pill opens it into the whole outline, that heading level with where the pill
//! was, and a heading chosen there is where the page goes. Back in the
//! bar, it is a pill again. The bar's buttons are acme's as ever.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Bounds, Context, MouseButton, Pixels, Point};

use apex_core::WindowId;

use crate::app::Acme;
use crate::web::{Toc, TocHead};

/// A row of the outline's list: a popover's.
const ROW: f32 = crate::shell::POPOVER_ROW_H;

/// The list's padding, its border's included.
const PAD: f32 = crate::shell::POPOVER_PAD + crate::shell::HAIRLINE;

/// How big a stop is across, by its heading's level: a first nearly as
/// wide as the thumb (5), smaller down to a fourth. Whole points, an odd
/// number short of the thumb's, so a stop is centred on it to the pixel.
const STOP: [f32; 5] = [0., 4., 3., 2., 2.];

/// How tall the pill naming the part being read is: a popover row's, a
/// target the pointer finds.
const PILL: f32 = crate::shell::POPOVER_ROW_H;

/// The outline's state in the app: which page's is out, its bar (as the
/// pointer last found it: the bars are laid out after the panel is made),
/// whether it is the whole list or the pill, the list's top, where the
/// pill or the list was drawn, and the list's scroll.
#[derive(Default)]
pub struct Outline {
    pub open: Option<WindowId>,
    /// The item whose stop is nearest the pointer in the bar: what the
    /// pill names, and what the list opens level with.
    pub near: Option<usize>,
    pub bar: Option<Bounds<Pixels>>,
    pub whole: bool,
    pub top: Pixels,
    pub panel_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub scroll: gpui::ScrollHandle,
    /// The list's row under the pointer: what ⌘C copies, a command's.
    pub hover: Option<usize>,
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
    /// Window `w`'s outline: a page's headings (two or more), or the
    /// outline a tool gave its text, each item at its line; with where
    /// the view is (top, height, how much shows: a page's pixels, a
    /// text's lines) and the item it is in.
    pub fn outline_of(&self, w: WindowId) -> Option<(Toc, Option<(f64, f64, f64)>, Option<usize>)> {
        if let Some((toc, current)) = self.webs.toc(w) {
            return Some((toc.clone(), self.webs.page_scroll(w), current));
        }
        // a terminal's or a text's, as the session says it
        // (`apex_server::outline`): a terminal's commands, each at its
        // prompt's line, with when it ran and how it went; the outline a
        // tool gave a text, each item at its line
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let entries = apex_server::outline::of(&self.node, w, now)?;
        let (heads, height, scroll): (Vec<TocHead>, f64, (f64, f64, f64)) = match self.node.state.window(w).ok()?.body {
            apex_core::Body::Term(t) => {
                let term = self.node.state.terms.get(&t)?;
                if entries.len() < 2 {
                    return None;
                }
                let heads = entries.into_iter().map(|e| TocHead { level: e.level, title: e.title, top: match e.at { apex_server::outline::At::Term(l) => l as f64, _ => 0. }, path: false, detail: e.detail, failed: e.failed }).collect();
                let height = term.total.max(1) as f64;
                (heads, height, (term.top as f64, height, term.rows as f64))
            }
            apex_core::Body::Text(b) => {
                let buf = self.node.state.buffer(b).ok()?;
                let n = buf.text.line_count().max(1);
                let heads = entries.into_iter().map(|e| TocHead { level: e.level, title: e.title, top: match e.at { apex_server::outline::At::Text(a) => buf.text.line_of(a.min(buf.text.len())) as f64, _ => 0. }, path: false, detail: e.detail, failed: e.failed }).collect();
                let scroll = match self.layouts.get(&apex_core::ViewId::Body(w)) {
                    Some(l) => (l.first_line as f64, l.total_lines.max(1) as f64, l.lines.len() as f64),
                    None => {
                        let origin = buf.views.get(&apex_core::ViewId::Body(w)).map(|v| v.origin).unwrap_or(0);
                        (buf.text.line_of(origin.min(buf.text.len())) as f64, n as f64, 1.)
                    }
                };
                (heads, n as f64, scroll)
            }
            // a page's is the client's own, positions and all (above)
            apex_core::Body::Page(_) => return None,
        };
        let toc = Toc { height, heads };
        let current = toc.current(Some(scroll));
        Some((toc, Some(scroll), current))
    }

    /// ⌘↑ ⌘↓ in window `w`: to the outline's entry before or after the
    /// line at the top -- ⌘↑ to the start of the entry the view is in when
    /// that is scrolled past -- and past the first, the top; past the
    /// last, the end (a terminal's screen, a text's end as End shows it).
    /// A terminal's commands are its prompts' lines; a text's entries a
    /// tool gave it: win's commands, scrolled to, the typing left where it
    /// is; a file's symbols, scrolled to and dot put there. Whether `w`
    /// has an outline to go by (else the keys are what they were).
    pub fn outline_jump(&mut self, w: WindowId, up: bool) -> bool {
        let Ok(body) = self.node.state.window(w).map(|x| x.body) else { return false };
        match body {
            apex_core::Body::Term(t) => {
                let Some(term) = self.node.state.terms.get(&t) else { return false };
                let (top, last) = (term.top, term.total.saturating_sub(term.rows as u64));
                let to = if up {
                    term.marks.iter().rev().map(|m| m.prompt).find(|&p| p < top).unwrap_or(0)
                } else {
                    term.marks.iter().map(|m| m.prompt).find(|&p| p > top).unwrap_or(last).min(last.max(top))
                };
                if to != top {
                    self.term_scroll(t, to as isize - top as isize);
                }
                true
            }
            apex_core::Body::Text(_) => {
                let Some(entries) = apex_server::outline::of(&self.node, w, 0) else { return false };
                let commands = self.node.window_outline(w).is_some_and(|o| o.items.iter().any(|it| it.of.is_none() && (it.started.is_some() || it.output.is_some())));
                let v = apex_core::ViewId::Body(w);
                let Some(t) = self.text_of(v) else { return false };
                let origin = self.node.view_buffer(v).ok().and_then(|b| self.node.state.buffer(b).ok()).map(|b| b.view(v).origin).unwrap_or(0).min(t.len());
                let top = t.line_of(origin);
                let ats: Vec<usize> = entries.iter().filter_map(|e| match e.at {
                    apex_server::outline::At::Text(a) => Some(a.min(t.len())),
                    _ => None,
                }).collect();
                let to = if up { ats.iter().rev().copied().find(|&a| t.line_of(a) < top) } else { ats.iter().copied().find(|&a| t.line_of(a) > top) };
                match to {
                    Some(a) => {
                        let _ = self.node.set_origin(&mut self.log, v, t.line_start(t.line_of(a)));
                        if !commands {
                            let _ = self.node.select(&mut self.log, v, a, a);
                        }
                    }
                    None if up => {
                        if origin > 0 {
                            let _ = self.node.set_origin(&mut self.log, v, 0);
                        }
                    }
                    // the end, as End shows it: a live window's with room
                    // under it for what comes
                    None => {
                        let quarters = if self.node.window_live(w) { 3 } else { 1 };
                        self.show_at.insert(v, (t.len(), quarters));
                    }
                }
                self.after();
                true
            }
            apex_core::Body::Page(_) => false,
        }
    }

    /// Window `w` at its outline's item `i`: a page scrolled to the
    /// heading; a text with the item's line at its top, dot there.
    pub fn outline_go(&mut self, w: WindowId, i: usize) {
        if self.webs.toc(w).is_some() {
            self.webs.toc_go(w, i);
            return;
        }
        // a terminal: its view to the command's prompt
        if let Some(apex_core::Body::Term(t)) = self.node.state.window(w).ok().map(|x| x.body) {
            let Some((toc, _, _)) = self.outline_of(w) else { return };
            let (Some(head), Some(term)) = (toc.heads.get(i), self.node.state.terms.get(&t)) else { return };
            let delta = head.top as isize - term.top as isize;
            self.term_scroll(t, delta);
            return;
        }
        let Some(at) = self.node.window_outline(w).and_then(|o| o.items.get(i)).map(|it| it.at) else { return };
        let v = apex_core::ViewId::Body(w);
        let Some(start) = self.text_of(v).map(|t| t.line_start(t.line_of(at.min(t.len())))) else { return };
        let _ = self.node.set_origin(&mut self.log, v, start);
        let _ = self.node.select(&mut self.log, v, at, at);
        self.after();
    }

    /// The scrollbar a pointer at `pos` is in, of a window with an
    /// outline: a page's, or a text body's.
    fn outline_bar_at(&self, pos: Point<Pixels>) -> Option<(WindowId, Bounds<Pixels>)> {
        if let Some((w, b)) = self.web_bars.iter().find(|(w, b)| b.contains(&pos) && self.webs.toc(**w).is_some()) {
            return Some((*w, *b));
        }
        if let Some(found) = self.layouts.iter().find_map(|(v, l)| match (v, l.scrollbar) {
            (apex_core::ViewId::Body(w), Some(bar)) if bar.contains(&pos) && self.node.window_outline(*w).is_some_and(|o| !o.items.is_empty()) => Some((*w, Bounds::new(bar.origin, gpui::size(bar.size.width, l.bounds.size.height)))),
            _ => None,
        }) {
            return Some(found);
        }
        // a terminal's
        self.term_layouts.iter().find_map(|(w, l)| (l.scrollbar.contains(&pos) && self.outline_of(*w).is_some()).then(|| (*w, Bounds::new(l.scrollbar.origin, gpui::size(l.scrollbar.size.width, l.bounds.size.height)))))
    }

    /// The pointer moved: the outline's pill out while it is in a page's
    /// bar (one with an outline); onto the pill, the whole outline from
    /// it, staying while the pointer is on it; back in the bar, the pill
    /// again. Whether anything changed.
    pub fn outline_hover(&mut self, pos: Point<Pixels>) -> bool {
        let in_bar = self.outline_bar_at(pos);
        let on_it = self.outline.panel_at.get().is_some_and(|b| b.dilate(px(4.)).contains(&pos));
        let was = (self.outline.open, self.outline.whole, self.outline.near);
        match in_bar {
            Some((w, bar)) => {
                self.outline.bar = Some(bar);
                (self.outline.open, self.outline.whole) = (Some(w), false);
                // the item under the pointer, not the one being read
                self.outline.near = self.outline_of(w).and_then(|(toc, scroll, _)| {
                    (0..toc.heads.len()).min_by(|&a, &b| {
                        let d = |i: usize| (stop_y(&toc, scroll, bar, i) - pos.y).abs();
                        d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
                    })
                });
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
            self.outline.near = None;
            self.outline.hover = None;
        }
        was != (self.outline.open, self.outline.whole, self.outline.near)
    }

    /// The whole outline placed where the pill was: the heading being
    /// read level with its stop (the list scrolled to it where the panel
    /// can go no higher, or no lower).
    fn place_outline(&mut self) {
        let (Some(w), Some(bar)) = (self.outline.open, self.outline.bar) else { return };
        let Some((toc, scroll, current)) = self.outline_of(w) else { return };
        let toc = &toc;
        let i = self.outline.near.or(current).unwrap_or(0);
        let y = stop_y(toc, scroll, bar, i);
        let (n, tall) = (toc.heads.len(), tall(toc.heads.len(), bar));
        let row = px(PAD + ROW * (i as f32 + 0.5));
        let top = (y - row).min(bar.bottom() - px(8.) - tall).max(bar.top() + px(8.));
        let off = (top + row - y).max(px(0.)).min(px(ROW * n as f32 + 2. * PAD) - tall);
        self.outline.top = top;
        self.outline.scroll.set_offset(point(px(0.), -off));
    }

    /// The outline beside the bar of the page whose it is: its pill
    /// (`outline_pill`), or the whole of it, a heading a row, indented by
    /// level, the view's with a menu's chosen bar at its left; B1 on one, the page there.
    pub fn outline_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = self.outline.open?;
        let bar = self.outline.bar?;
        let (toc, _, current) = self.outline_of(w)?;
        if !self.outline.whole {
            return self.outline_pill(w, bar);
        }
        let t = crate::theme::theme();
        let at = self.outline.panel_at.clone();
        // a first level is a title over what is under it, bold, only
        // when there is something under it: a flat list (an agent's
        // turns, a terminal's commands) is all of a kind, none bold
        let nested = toc.heads.iter().any(|h| h.level > 1);
        let rows = toc.heads.iter().enumerate().map(|(i, h)| {
            let here = current == Some(i);
            let row = div()
                .id(("toc", i))
                .relative()
                .flex_none()
                .h(px(ROW))
                .flex()
                .items_center()
                .pl(px(12.))
                .pr(px(8.))
                .rounded(px(crate::shell::POPOVER_ROW_R))
                .whitespace_nowrap()
                .overflow_hidden()
                .cursor_default()
                .when(h.level == 1 && nested, |d| d.font_weight(crate::fonts::weight(gpui::FontWeight::SEMIBOLD)))
                // the heading being read: the bar a menu's chosen row
                // has at its left, B3's, as going there is
                .when(here, |d| d.child(div().absolute().left(px(3.)).top(px(crate::shell::BAR_INSET)).bottom(px(crate::shell::BAR_INSET)).w(px(2.5)).rounded(px(1.25)).bg(rgb(t.look_hl))))
                .child(div().flex_none().w(px(12. * (h.level.saturating_sub(1)) as f32)))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(title(h)))
                // what more it says (a command's run), quieter, at the right
                .when_some(h.detail.clone(), |d, detail| d.child(div().flex_none().pl(px(10.)).text_size(px(12.)).font_weight(crate::fonts::weight(gpui::FontWeight::NORMAL)).text_color(rgb(if h.failed { t.text } else { t.panel_dim })).child(detail)))
                // a command's dot: the command's own buttons, as by its prompt
                .children(self.dot_view(w, i, cx).map(|d| d.ml(px(6.))))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |acme, _, _, cx| {
                        acme.outline_go(w, i);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                // a row is a place: B3 goes there too, as the lens has it
                // (a command's own buttons are its dot's, at the row's end)
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |acme, _, _, cx| {
                        acme.outline_go(w, i);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_hover(cx.listener(move |acme, over: &bool, _, _| {
                    if *over {
                        acme.outline.hover = Some(i);
                    } else if acme.outline.hover == Some(i) {
                        acme.outline.hover = None;
                    }
                }));
            // going to a heading is B3's: its colours under the pointer
            crate::shell::chosen_under_pointer(row, crate::shell::Act::Look, crate::shell::BAR_INSET, "toc-row")
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
            .p(px(crate::shell::POPOVER_PAD))
            .children(rows);
        let panel = crate::shell::popover()
            .relative()
            .flex()
            .flex_col()
            .min_w(px(170.))
            .max_w(px(if toc.heads.iter().any(|h| h.detail.is_some()) { 520. } else { 320. }))
            .h(tall)
            .overflow_hidden()
            .child(self.overlay_mark_by(px(0.)))
            .child(gpui::canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {}).absolute().top(px(0.)).left(px(0.)).size_full())
            .child(list);
        Some(deferred(anchored().anchor(gpui::Anchor::TopLeft).position(point(bar.right() + px(2.), self.outline.top)).child(crate::shell::fade_in("toc-panel", panel))).with_priority(2).into_any_element())
    }

    /// The outline's pill: the part being read, by its stop; the pointer
    /// onto it opens the whole outline (`outline_hover`).
    fn outline_pill(&self, w: WindowId, bar: Bounds<Pixels>) -> Option<AnyElement> {
        let (toc, scroll, current) = self.outline_of(w)?;
        let i = self.outline.near.or(current).unwrap_or(0);
        let head = toc.heads.get(i)?;
        let t = crate::theme::theme();
        let at = self.outline.panel_at.clone();
        let y = stop_y(&toc, scroll, bar, i);
        let top = (y - px(PILL / 2.)).min(bar.bottom() - px(PILL)).max(bar.top());
        let pill = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(PILL))
            .max_w(px(320.))
            .px(px(10.))
            .rounded(px(PILL / 2.))
            .bg(rgb(t.tag_bg))
            .border_1()
            .border_color(rgb(t.body_border))
            .font_family(crate::fonts::ui())
            .text_size(px(13.))
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
