//! acme's tiling, ported from plan9port's `cols.c`, `rows.c` and the
//! geometry half of `wind.c` (`winresize`), with the drawing left out.
//!
//! Everything is integer pixels in the row's coordinate space, as in acme:
//! a column is a rectangle; a window is a rectangle whose top line is its
//! tag and whose bottom is trimmed to whole body lines unless it is the
//! last in its column. The algorithms need to know how many lines a tag
//! wraps to, the body font's height, and how many lines of text a body
//! shows; the [`Info`] trait supplies those (the client from its
//! layouts, a headless leader from [`Headless`]).
//!
//! Function and variable names follow acme's so the two can be read side
//! by side.

use serde::{Deserialize, Serialize};

use crate::ids::*;
use crate::state::{Column, Layout, Slot};

/// acme's `Border`: between columns and between windows.
pub const BORDER: i32 = 2;
/// acme's `Scrollwid`: the layout box and scrollbar width.
pub const SCROLLWID: i32 = 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, Hash)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
        Rect { x0, y0, x1, y1 }
    }
    pub fn dx(&self) -> i32 {
        self.x1 - self.x0
    }
    pub fn dy(&self) -> i32 {
        self.y1 - self.y0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.x0 <= x && x < self.x1 && self.y0 <= y && y < self.y1
    }
}

/// What the algorithms ask about text: acme reads these off its frames.
pub trait Info {
    /// The tag font's height (acme's global `font->height`); also the
    /// height of column tags and the top row.
    fn font_height(&self) -> i32;
    /// acme's `wintaglines`: the lines a window's tag needs at `width`,
    /// never more than `maxlines` (how many fit the window).
    fn taglines(&self, w: WindowId, width: i32, maxlines: i32) -> i32;
    fn body_font_height(&self, w: WindowId) -> i32;
    /// acme's `fr.nlines`: lines of text the body shows at `width` when
    /// `maxlines` lines fit.
    fn body_nlines(&self, w: WindowId, width: i32, maxlines: i32) -> i32;
}

/// For a leader with no screen (the daemon): every tag is one line, every
/// body is full.
#[derive(Clone, Copy, Debug)]
pub struct Headless {
    pub font: i32,
    pub body_font: i32,
}

impl Default for Headless {
    fn default() -> Self {
        Headless { font: 17, body_font: 17 }
    }
}

impl Info for Headless {
    fn font_height(&self) -> i32 {
        self.font
    }
    fn taglines(&self, _w: WindowId, _width: i32, maxlines: i32) -> i32 {
        taglines_rule(1, false, maxlines)
    }
    fn body_font_height(&self, _w: WindowId) -> i32 {
        self.body_font
    }
    fn body_nlines(&self, _w: WindowId, _width: i32, maxlines: i32) -> i32 {
        maxlines
    }
}

/// The tail of acme's `wintaglines` (with `tagexpand` on, as by default):
/// `nlines` wrapped lines of tag text, `trailing_newline` if the tag ends
/// with one, `maxlines` fitting the window.
pub fn taglines_rule(nlines: i32, trailing_newline: bool, maxlines: i32) -> i32 {
    let maxlines = maxlines.max(0);
    if nlines >= maxlines {
        return maxlines;
    }
    let mut n = nlines;
    if trailing_newline {
        n += 1;
    }
    if n == 0 {
        n = 1;
    }
    n
}

/// Where acme moves the mouse after a layout change.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Warp {
    /// `coladd`: "near the button, but in the body" of a new window.
    NewWindow(WindowId),
    /// `winmousebut`: the middle of a window's layout box.
    WinButton(WindowId),
    /// `colclose`: `window` went away; if the next window down took its
    /// space, acme moves the mouse onto that window's `Del` (unless the
    /// mouse is restored to where it was before `window` was made).
    Closed { window: WindowId, next: Option<WindowId> },
    /// `colmousebut`: the middle of a column's layout box.
    ColButton(ColumnId),
    /// `openfile` with `jump`, or a `Look` that found something: the start
    /// of the selection in that text.
    Sel(ViewId),
}

// ---- per-window geometry helpers ---------------------------------------------------

impl Slot {
    /// `w->tagtop.max.y`, the bottom of the tag's first line.
    pub fn tagtop_y1(&self, font: i32) -> i32 {
        self.r.y0 + font
    }
    /// The bottom of the tag (`w->tag.all.max.y`).
    pub fn tag_y1(&self, font: i32) -> i32 {
        (self.r.y0 + self.taglines * font).min(self.r.y1.max(self.r.y0))
    }
    /// `w->body.fr.maxlines`: whole body lines that fit (zero while
    /// obscured).
    pub fn fr_maxlines(&self, _body_font: i32) -> i32 {
        self.frmax
    }
    /// `Dy(w->body.all)`: from the tag's bottom to the window's bottom.
    pub fn body_all_dy(&self, font: i32) -> i32 {
        self.r.y1 - self.tag_y1(font)
    }
}

/// acme's `winresize` (geometry only): lay `w` out in `r`. The tag takes
/// as many lines as it needs, the body the rest, trimmed to whole lines
/// unless `keepextra`. Returns the window's new bottom.
pub fn winresize(l: &mut Layout, ci: usize, wi: usize, r: Rect, keepextra: bool, info: &dyn Info) -> i32 {
    let font = info.font_height().max(1);
    let id = l.cols[ci].wins[wi].window;
    // wintaglines: the tag laid out in all of r tells how many lines fit
    let tag_maxlines = (r.dy() / font).max(0);
    let taglines = info.taglines(id, r.dx(), tag_maxlines).max(0);
    let y = (r.y0 + taglines * font).min(r.y1.max(r.y0));
    let bf = info.body_font_height(id).max(1);
    let mut body = r;
    if y + 1 + bf <= r.y1 {
        // room for one line: a 1-pixel line, then the body
        body.y0 = (y + 1).min(r.y1);
        body.y1 = r.y1;
    } else {
        body.y0 = y;
        body.y1 = y;
    }
    // textresize
    if body.dy() <= 0 {
        body.y1 = body.y0;
    } else if !keepextra {
        body.y1 -= body.dy() % bf;
    }
    let fr_maxlines = body.dy() / bf;
    let nlines = info.body_nlines(id, r.dx(), fr_maxlines).clamp(0, fr_maxlines);
    let s = &mut l.cols[ci].wins[wi];
    s.taglines = taglines;
    s.r = Rect::new(r.x0, r.y0, r.x1, body.y1);
    s.body = body;
    s.nlines = nlines;
    s.frmax = fr_maxlines;
    s.maxlines = nlines.min(s.maxlines.max(fr_maxlines));
    s.r.y1
}

// ---- columns ---------------------------------------------------------------------------

/// A window being added: brand new (`wininit`) or one that already has a
/// history (`coldragwin` moving it).
pub enum Adding {
    New(WindowId),
    Existing(Slot),
}

/// acme's `coladd`: put a window into column `ci` at height `y`, or with
/// no `y`, by stealing the lower half of the last window. Returns the
/// window's index in the column.
pub fn coladd(l: &mut Layout, ci: usize, w: Adding, y: Option<i32>, info: &dyn Info) -> usize {
    let font = info.font_height().max(1);
    let mut r = l.cols[ci].r;
    r.y0 = l.cols[ci].r.y0 + font + BORDER;
    let mut y = y.unwrap_or(r.y0 - 1);
    let n = l.cols[ci].wins.len();
    if y < r.y0 && n > 0 {
        // steal half of last window by default
        let v = &l.cols[ci].wins[n - 1];
        y = v.body.y0 + v.body.dy() / 2;
    }
    // look for window we'll land on
    let mut i = 0;
    while i < n {
        if y < l.cols[ci].wins[i].r.y1 {
            break;
        }
        i += 1;
    }
    let mut buggered = false;
    if n > 0 {
        let vi = if i < n { i } else { n - 1 };
        if i < n {
            i += 1; // new window will go after v
        }
        // if landing window (v) is too small, grow it first
        let minht = font + BORDER + 1;
        let mut j = 0;
        loop {
            let c = &l.cols[ci];
            let v = &c.wins[vi];
            let bf = info.body_font_height(v.window).max(1);
            if c.safe && v.fr_maxlines(bf) > 3 && v.body_all_dy(font) > minht {
                break;
            }
            j += 1;
            if j > 10 {
                buggered = true; // too many windows in column
                break;
            }
            colgrow(l, ci, vi, 1, info);
        }
        // figure out where to split v to make room for w
        let c = &l.cols[ci];
        let ymax = if i < n { c.wins[i].r.y0 - BORDER } else { c.r.y1 };
        let v = &c.wins[vi];
        let bf = info.body_font_height(v.window).max(1);
        // new window must start after v's tag ends
        y = y.max(v.tagtop_y1(font) + BORDER);
        // new window must start early enough to end before ymax
        y = y.min(ymax - minht);
        // if y is too small, too many windows in column
        if y < v.tagtop_y1(font) + BORDER {
            buggered = true;
        }
        // resize v
        r = v.r;
        r.y1 = ymax;
        let mut r1 = r;
        y = y.min(ymax - (font * v.taglines + bf + BORDER + 1));
        r1.y1 = y.min(v.body.y0 + v.nlines * bf);
        r1.y0 = winresize(l, ci, vi, r1, false, info);
        r1.y1 = r1.y0 + BORDER;
        // leave r with w's coordinates
        r.y0 = r1.y1;
    }
    let slot = match w {
        Adding::New(id) => {
            // wininit: one tag line, the body below a 1-pixel line, and
            // maxlines from what fits
            let bf = info.body_font_height(id).max(1);
            let body_dy = (r.dy() - font - 1).max(0);
            Slot { window: id, r, body: Rect::new(r.x0, (r.y0 + font + 1).min(r.y1), r.x1, r.y1), taglines: 1, nlines: 0, frmax: body_dy / bf, maxlines: body_dy / bf }
        }
        Adding::Existing(s) => s,
    };
    l.cols[ci].wins.insert(i, slot);
    // a new window's tag is set right after (winsettag → winresize); a
    // moved one is resized into its place: both come to this
    winresize(l, ci, i, r, true, info);
    l.cols[ci].safe = true;
    // if there were too many windows, redo the whole column
    if buggered {
        let cr = l.cols[ci].r;
        colresize(l, ci, cr, info);
    }
    i
}

/// acme's `colclose` without the freeing: take window `wi` out of column
/// `ci`, giving its space to a neighbour. Returns the slot removed and,
/// when the *next* window took the space ("extend next window up"), that
/// window: acme moves the mouse onto its `Del`.
pub fn colclose(l: &mut Layout, ci: usize, wi: usize, info: &dyn Info) -> (Slot, Option<WindowId>) {
    if !l.cols[ci].safe {
        colgrow(l, ci, wi, 1, info);
    }
    let s = l.cols[ci].wins.remove(wi);
    let mut r = s.r;
    let n = l.cols[ci].wins.len();
    if n == 0 {
        return (s, None);
    }
    let (idx, up) = if wi == n {
        // extend last window down
        let w = &l.cols[ci].wins[wi - 1];
        r.y0 = w.r.y0;
        r.y1 = l.cols[ci].r.y1;
        (wi - 1, false)
    } else {
        // extend next window up
        let w = &l.cols[ci].wins[wi];
        r.y1 = w.r.y1;
        (wi, true)
    };
    let id = l.cols[ci].wins[idx].window;
    if l.cols[ci].safe {
        winresize(l, ci, idx, r, true, info);
    }
    (s, if up { Some(id) } else { None })
}

/// acme's `colresize`: the column gets rectangle `r`; its windows keep
/// their proportions.
pub fn colresize(l: &mut Layout, ci: usize, r: Rect, info: &dyn Info) {
    let font = info.font_height().max(1);
    let n = l.cols[ci].wins.len();
    let mut r1 = r;
    r1.y1 = r1.y0 + font; // the column tag
    r1.y0 = r1.y1;
    r1.y1 += BORDER;
    r1.y1 = r.y1;
    let new = r.dy() - n as i32 * (BORDER + font);
    let old = l.cols[ci].r.dy() - n as i32 * (BORDER + font);
    for i in 0..n {
        let wdy = l.cols[ci].wins[i].r.dy();
        l.cols[ci].wins[i].maxlines = 0;
        if i == n - 1 {
            r1.y1 = r.y1;
        } else {
            r1.y1 = r1.y0;
            if new > 0 && old > 0 && wdy > BORDER + font {
                r1.y1 += (wdy - BORDER - font) * new / old + BORDER + font;
            }
        }
        r1.y1 = r1.y1.max(r1.y0 + BORDER + font);
        let mut r2 = r1;
        r2.y1 = r2.y0 + BORDER;
        r1.y0 = r2.y1;
        r1.y0 = winresize(l, ci, i, r1, i == n - 1, info);
    }
    l.cols[ci].r = r;
}

/// acme's `colgrow`: button 1 makes window `wi` a bit bigger, 2 as big as
/// can be, 3 the whole column; -1 just refits it in its own space.
pub fn colgrow(l: &mut Layout, ci: usize, wi: usize, but: i32, info: &dyn Info) {
    let font = info.font_height().max(1);
    let n = l.cols[ci].wins.len();
    let mut cr = l.cols[ci].r;
    let bf = |l: &Layout, j: usize| info.body_font_height(l.cols[ci].wins[j].window).max(1);
    if but < 0 {
        // make sure window fills its own space properly
        let mut r = l.cols[ci].wins[wi].r;
        if wi == n - 1 || !l.cols[ci].safe {
            r.y1 = cr.y1;
        } else {
            r.y1 = l.cols[ci].wins[wi + 1].r.y0 - BORDER;
        }
        winresize(l, ci, wi, r, true, info);
        return;
    }
    cr.y0 = l.cols[ci].wins[0].r.y0;
    if but == 3 {
        // full size
        if wi != 0 {
            l.cols[ci].wins.swap(0, wi);
        }
        winresize(l, ci, 0, cr, true, info);
        for j in 1..n {
            // obscured: no lines fit; the rectangles go stale, as in acme
            l.cols[ci].wins[j].frmax = 0;
        }
        l.cols[ci].safe = false;
        return;
    }
    // store old #lines for each window
    let onl = l.cols[ci].wins[wi].fr_maxlines(bf(l, wi));
    let mut nl: Vec<i32> = (0..n).map(|j| l.cols[ci].wins[j].taglines - 1 + l.cols[ci].wins[j].fr_maxlines(bf(l, j))).collect();
    let tot: i32 = nl.iter().sum();
    // approximate new #lines for this window
    if but == 2 {
        // as big as can be
        nl.iter_mut().for_each(|x| *x = 0);
    } else {
        let w = &l.cols[ci].wins[wi];
        let mine = w.taglines - 1 + w.maxlines;
        let mut nnl = (onl + (5.min(mine)).max(onl / 2)).min(tot);
        if nnl < mine {
            nnl = (mine + nnl) / 2;
        }
        if nnl == 0 {
            nnl = 2;
        }
        let mut dnl = nnl - onl;
        // compute new #lines for each window
        for k in 1..n {
            // prune from later window
            let j = wi + k;
            if j < n && nl[j] != 0 {
                let take = dnl.min(1.max(nl[j] / 2));
                nl[j] -= take;
                nl[wi] += take;
                dnl -= take;
            }
            // prune from earlier window
            if k <= wi {
                let j = wi - k;
                if nl[j] != 0 {
                    let take = dnl.min(1.max(nl[j] / 2));
                    nl[j] -= take;
                    nl[wi] += take;
                    dnl -= take;
                }
            }
        }
    }
    // Pack: pack everyone above
    let mut y1 = cr.y0;
    for j in 0..wi {
        let mut r = l.cols[ci].wins[j].r;
        r.y0 = y1;
        r.y1 = y1 + font;
        if nl[j] != 0 {
            r.y1 += 1 + nl[j] * bf(l, j);
        }
        r.y0 = winresize(l, ci, j, r, false, info);
        r.y1 = r.y0 + BORDER;
        y1 = r.y1;
    }
    // scan to see new size of everyone below
    let mut y2 = l.cols[ci].r.y1;
    for j in (wi + 1..n).rev() {
        let mut r = l.cols[ci].wins[j].r;
        r.y0 = y2 - font;
        if nl[j] != 0 {
            r.y0 -= 1 + nl[j] * bf(l, j);
        }
        r.y0 -= BORDER;
        y2 = r.y0;
    }
    // compute new size of window
    let mut r = l.cols[ci].wins[wi].r;
    r.y0 = y1;
    r.y1 = y2;
    let h = bf(l, wi);
    if r.dy() < font + 1 + h + BORDER {
        r.y1 = r.y0 + font + 1 + h + BORDER;
    }
    r.y1 = winresize(l, ci, wi, r, true, info);
    if wi < n - 1 {
        r.y0 = r.y1;
        r.y1 += BORDER;
    }
    // pack everyone below
    let mut y1 = r.y1;
    for j in wi + 1..n {
        let mut r = l.cols[ci].wins[j].r;
        r.y0 = y1;
        r.y1 = y1 + font;
        if nl[j] != 0 {
            r.y1 += 1 + nl[j] * bf(l, j);
        }
        y1 = winresize(l, ci, j, r, j == n - 1, info);
        if j < n - 1 {
            // no border on last window
            r.y0 = y1;
            r.y1 += BORDER;
            y1 = r.y1;
        }
    }
    l.cols[ci].safe = true;
}

/// acme's `colsort`: windows in name order, keeping their heights.
pub fn colsort(l: &mut Layout, ci: usize, name: impl Fn(WindowId) -> String, info: &dyn Info) {
    let font = info.font_height().max(1);
    let n = l.cols[ci].wins.len();
    if n == 0 {
        return;
    }
    let mut wp: Vec<Slot> = l.cols[ci].wins.clone();
    wp.sort_by(|a, b| name(a.window).cmp(&name(b.window)));
    l.cols[ci].wins = wp;
    let mut r = l.cols[ci].r;
    r.y0 = l.cols[ci].r.y0 + font; // c->tag.fr.r.max.y
    let mut y = r.y0;
    for i in 0..n {
        r.y0 = y;
        if i == n - 1 {
            r.y1 = l.cols[ci].r.y1;
        } else {
            r.y1 = r.y0 + l.cols[ci].wins[i].r.dy() + BORDER;
        }
        let mut r1 = r;
        r1.y1 = r1.y0 + BORDER;
        r.y0 = r1.y1;
        y = winresize(l, ci, i, r, i == n - 1, info);
    }
}

/// acme's `coldragwin`: the layout box of window `wi` was pressed with
/// `but` at `op` and released at `p`. A click grows; a drag moves the
/// window within its column, to another column, or resizes against the
/// window above. Returns where the mouse goes.
pub fn coldragwin(l: &mut Layout, ci: usize, wi: usize, but: i32, op: (i32, i32), p: (i32, i32), info: &dyn Info) -> Option<Warp> {
    let font = info.font_height().max(1);
    let n = l.cols[ci].wins.len();
    let w = l.cols[ci].wins[wi].window;
    let (mut px, py) = p;
    if (px - op.0).abs() < 5 && (py - op.1).abs() < 5 {
        colgrow(l, ci, wi, but, info);
        return Some(Warp::WinButton(w));
    }
    // is it a flick to the right?
    if (py - op.1).abs() < 10 && px > op.0 + 30 && rowwhichcol(l, (px, py)) == Some(ci) {
        px = op.0 + l.cols[ci].wins[wi].r.dx(); // yes: toss to next column
    }
    let nc = rowwhichcol(l, (px, py));
    if let Some(nc) = nc {
        if nc != ci {
            let (slot, _) = colclose(l, ci, wi, info);
            coladd(l, nc, Adding::Existing(slot), Some(py), info);
            return Some(Warp::WinButton(w));
        }
    }
    if wi == 0 && n == 1 {
        return None; // can't do it
    }
    let wr = l.cols[ci].wins[wi].r;
    let shuffle = (wi > 0 && py < l.cols[ci].wins[wi - 1].r.y0) || (wi < n - 1 && py > wr.y1) || (wi == 0 && py > wr.y1);
    if shuffle {
        let (slot, _) = colclose(l, ci, wi, info);
        coladd(l, ci, Adding::Existing(slot), Some(py), info);
        return Some(Warp::WinButton(w));
    }
    if wi == 0 {
        return None;
    }
    let v = l.cols[ci].wins[wi - 1].clone();
    let mut py = py;
    if py < v.tagtop_y1(font) {
        py = v.tagtop_y1(font);
    }
    if py > wr.y1 - font - BORDER {
        py = wr.y1 - font - BORDER;
    }
    let vbf = info.body_font_height(v.window).max(1);
    let mut r = v.r;
    r.y1 = py;
    if r.y1 > v.body.y0 {
        r.y1 -= (r.y1 - v.body.y0) % vbf;
        if v.body.y0 == v.body.y1 {
            r.y1 += 1;
        }
    }
    r.y0 = winresize(l, ci, wi - 1, r, false, info);
    r.y1 = r.y0 + BORDER;
    r.y0 = r.y1;
    r.y1 = if wi == n - 1 { l.cols[ci].r.y1 } else { l.cols[ci].wins[wi + 1].r.y0 - BORDER };
    winresize(l, ci, wi, r, true, info);
    l.cols[ci].safe = true;
    Some(Warp::WinButton(w))
}

/// acme's `makenewwindow`, the placement half: where in column `ci` a
/// window made from window `from` goes. `None` means "steal half of the
/// last window".
pub fn newwindow_y(l: &Layout, ci: usize, from: Option<WindowId>, info: &dyn Info) -> Option<i32> {
    let font = info.font_height().max(1);
    let c = &l.cols[ci];
    let from = from?;
    if c.wins.is_empty() {
        return None;
    }
    let bf = |s: &Slot| info.body_font_height(s.window).max(1);
    // find biggest window and biggest blank spot
    let mut emptyw = &c.wins[0];
    let mut bigw = emptyw;
    for w in &c.wins[1..] {
        // use >= to choose one near bottom of screen
        if w.fr_maxlines(bf(w)) >= bigw.fr_maxlines(bf(bigw)) {
            bigw = w;
        }
        if w.fr_maxlines(bf(w)) - w.nlines >= emptyw.fr_maxlines(bf(emptyw)) - emptyw.nlines {
            emptyw = w;
        }
    }
    let el = emptyw.fr_maxlines(bf(emptyw)) - emptyw.nlines;
    // if empty space is big, use it
    if el > 15 || (el > 3 && el > (bigw.fr_maxlines(bf(bigw)) - 1) / 2) {
        return Some(emptyw.body.y0 + emptyw.nlines * font);
    }
    // if this window is in column and isn't much smaller, split it
    let mut big = bigw;
    if let Some(t) = c.wins.iter().find(|s| s.window == from) {
        if t.r.dy() > 2 * bigw.r.dy() / 3 {
            big = t;
        }
    }
    Some((big.r.y0 + big.r.y1) / 2)
}

// ---- the row -----------------------------------------------------------------------------

/// The column under a point, if any.
pub fn rowwhichcol(l: &Layout, p: (i32, i32)) -> Option<usize> {
    l.cols.iter().position(|c| c.r.contains(p.0, p.1))
}

/// A column being added.
pub enum AddingCol {
    New { id: ColumnId, tag: BufferId },
    Existing(Column),
}

/// acme's `rowadd`: a column at `x`, or with no `x`, taking 40% of the
/// last column. `None` if the column it would split is too narrow.
pub fn rowadd(l: &mut Layout, c: AddingCol, x: Option<i32>, info: &dyn Info) -> Option<usize> {
    let font = info.font_height().max(1);
    let mut r = l.r;
    r.y0 = l.r.y0 + font + BORDER;
    let mut x = x.unwrap_or(r.x0 - 1);
    let n = l.cols.len();
    if x < r.x0 && n > 0 {
        // steal 40% of last column by default
        let d = &l.cols[n - 1];
        x = d.r.x0 + 3 * d.r.dx() / 5;
    }
    // look for column we'll land on
    let mut i = 0;
    while i < n {
        if x < l.cols[i].r.x1 {
            break;
        }
        i += 1;
    }
    if n > 0 {
        let di = if i < n { i } else { n - 1 };
        if i < n {
            i += 1; // new column will go after d
        }
        r = l.cols[di].r;
        if r.dx() < 100 {
            return None;
        }
        let mut r1 = r;
        r1.x1 = (x - BORDER).min(r.x1 - 50);
        if r1.dx() < 50 {
            r1.x1 = r1.x0 + 50;
        }
        colresize(l, di, r1, info);
        r1.x0 = r1.x1;
        r1.x1 = r1.x0 + BORDER;
        r.x0 = r1.x1;
    }
    match c {
        AddingCol::New { id, tag } => {
            // colinit
            l.cols.insert(i, Column { id, tag, r, safe: true, wins: Vec::new() });
        }
        AddingCol::Existing(c) => {
            l.cols.insert(i, c);
            colresize(l, i, r, info);
        }
    }
    Some(i)
}

/// acme's `rowresize`: the row gets rectangle `r`; columns keep their
/// proportions.
pub fn rowresize(l: &mut Layout, r: Rect, info: &dyn Info) {
    let font = info.font_height().max(1);
    let or = l.r;
    let deltax = r.x0 - or.x0;
    l.r = r;
    let mut r = r;
    r.y0 += font + BORDER; // the top row's tag, then a border
    let mut r1 = r;
    r1.x1 = r1.x0;
    let n = l.cols.len();
    for i in 0..n {
        r1.x0 = r1.x1;
        // the test should not be necessary, but guarantee we don't lose a pixel
        if i == n - 1 {
            r1.x1 = r.x1;
        } else if or.dx() > 0 {
            r1.x1 = (l.cols[i].r.x1 - or.x0) * r.dx() / or.dx() + deltax;
        } else {
            r1.x1 = r.x1;
        }
        if i > 0 {
            r1.x0 += BORDER;
        }
        colresize(l, i, r1, info);
    }
}

/// acme's `rowclose` without the freeing: take column `ci` out, giving
/// its width to a neighbour.
pub fn rowclose(l: &mut Layout, ci: usize, info: &dyn Info) -> Column {
    let c = l.cols.remove(ci);
    let mut r = c.r;
    let n = l.cols.len();
    if n == 0 {
        return c;
    }
    let idx = if ci == n {
        // extend last column right
        let d = &l.cols[ci - 1];
        r.x0 = d.r.x0;
        r.x1 = l.r.x1;
        ci - 1
    } else {
        // extend next column left
        r.x1 = l.cols[ci].r.x1;
        ci
    };
    colresize(l, idx, r, info);
    c
}

/// acme's `rowdragcol`: the layout box of column `ci` was dragged from
/// `op` to `p`: move it past its neighbours, or resize against the
/// column to its left.
pub fn rowdragcol(l: &mut Layout, ci: usize, op: (i32, i32), p: (i32, i32), info: &dyn Info) -> Option<Warp> {
    let n = l.cols.len();
    let id = l.cols[ci].id;
    if (p.0 - op.0).abs() < 5 && (p.1 - op.1).abs() < 5 {
        return None;
    }
    let cr = l.cols[ci].r;
    if (ci > 0 && p.0 < l.cols[ci - 1].r.x0) || (ci < n - 1 && p.0 > cr.x1) {
        // shuffle
        let x = cr.x0;
        let c = rowclose(l, ci, info);
        let c = match rowadd(l, AddingCol::Existing(c), Some(p.0), info) {
            Some(_) => return Some(Warp::ColButton(id)),
            None => c_back(l, ci),
        };
        let c = match rowadd(l, AddingCol::Existing(c), Some(x), info) {
            Some(_) => return Some(Warp::ColButton(id)),
            None => c_back(l, ci),
        };
        if rowadd(l, AddingCol::Existing(c), None, info).is_some() {
            return Some(Warp::ColButton(id));
        }
        return None; // acme gives up and drops the column; we cannot lose it here
    }
    if ci == 0 {
        return None;
    }
    let d = l.cols[ci - 1].r;
    let mut x = p.0;
    if x < d.x0 + 80 + SCROLLWID {
        x = d.x0 + 80 + SCROLLWID;
    }
    if x > cr.x1 - 80 - SCROLLWID {
        x = cr.x1 - 80 - SCROLLWID;
    }
    let mut r = d;
    r.x1 = x;
    colresize(l, ci - 1, r, info);
    let mut r = cr;
    r.x0 = x + BORDER;
    colresize(l, ci, r, info);
    Some(Warp::ColButton(id))
}

/// A failed `rowadd` of an existing column leaves it out of the row;
/// the caller keeps trying, so hand it back.
fn c_back(_l: &mut Layout, _ci: usize) -> Column {
    unreachable!("rowadd of an existing column into a row that held it cannot fail")
}
