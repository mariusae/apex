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
use crate::state::{Column, Full, Layout, Slot, Stashed};

/// acme's `Border`: between columns and between windows.
pub const BORDER: i32 = 2;
/// acme's `Scrollwid`: the layout box and scrollbar width.
pub const SCROLLWID: i32 = 12;
/// A column squeezed as far as it goes (minimized by B2 on another
/// column's box, or stashed by B3 on its own): its box, and its windows'
/// boxes down it, with no room for any text. The
/// width acme's windows squeezed to their tags have, turned on its side.
pub const STRIP: i32 = SCROLLWID + BORDER;

/// Whether a column (or a window in one) is a strip: too narrow for text.
pub fn is_strip(r: Rect) -> bool {
    r.dx() <= STRIP
}

/// The column nearest `ci` that is no strip, `ci` itself if it is none;
/// nearer on the left first at equal distance. None if every column is.
pub fn nearest_open(l: &Layout, ci: usize) -> Option<usize> {
    let n = l.cols.len();
    (0..n).flat_map(|d| [ci.checked_sub(d), Some(ci + d)]).flatten().filter(|&i| i < n).find(|&i| !is_strip(l.cols[i].r))
}

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
    /// height of column tags and the top row. A one-line tag's.
    fn font_height(&self) -> i32;
    /// How much each line after its first adds to a tag: the font's
    /// height, as acme has it -- or less, where a tag's line has a pad
    /// over and under it that is the tag's, not each line's (the client's:
    /// a body's line). See `tag_height`.
    fn tag_row(&self) -> i32 {
        self.font_height()
    }
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

/// A tag of `n` lines: its first a whole tag line (`font_height`, its
/// pad over and under it), each after that a row (`tag_row`) -- the pad
/// is the tag's, not each line's. acme's `n * font->height` where the two
/// are one.
pub fn tag_height(info: &dyn Info, n: i32) -> i32 {
    if n <= 0 {
        0
    } else {
        info.font_height() + (n - 1) * info.tag_row().max(1)
    }
}

/// How many tag lines fit in `dy` (`tag_height`'s inverse, rounded down).
pub fn tag_lines_fit(info: &dyn Info, dy: i32) -> i32 {
    let font = info.font_height().max(1);
    if dy < font {
        0
    } else {
        1 + (dy - font) / info.tag_row().max(1)
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
    /// The bottom of the tag (`w->tag.all.max.y`): where the body
    /// begins, less the line between them when there is a body.
    pub fn tag_y1(&self) -> i32 {
        if self.body.dy() > 0 {
            self.body.y0 - 1
        } else {
            self.body.y0
        }
    }
    /// `w->body.fr.maxlines`: whole body lines that fit (zero while
    /// obscured).
    pub fn fr_maxlines(&self, _body_font: i32) -> i32 {
        self.frmax
    }
    /// `Dy(w->body.all)`: from the tag's bottom to the window's bottom.
    pub fn body_all_dy(&self) -> i32 {
        self.r.y1 - self.tag_y1()
    }
}

/// acme's `winresize` (geometry only): lay `w` out in `r`. The tag takes
/// as many lines as it needs, the body the rest, trimmed to whole lines
/// unless `keepextra`. Returns the window's new bottom.
pub fn winresize(l: &mut Layout, ci: usize, wi: usize, r: Rect, keepextra: bool, info: &dyn Info) -> i32 {
    // every caller but colresize is the user's doing: the shares are
    // read anew from the rectangles at the next resize
    let y = winresize_in(l, ci, wi, r, keepextra, info);
    l.cols[ci].wins[wi].share = 0;
    y
}

/// Parts per million: a column's window space, shared out.
const SHARE_UNIT: i64 = 1_000_000;

/// The shares read off the windows' allocations (their rectangles and
/// the remainders they gave up to whole lines).
fn sync_shares(l: &mut Layout, ci: usize) {
    let allocs: Vec<i64> = l.cols[ci].wins.iter().map(|s| (s.r.dy() + s.extra).max(1) as i64).collect();
    let total: i64 = allocs.iter().sum::<i64>().max(1);
    let n = allocs.len();
    let mut given = 0i64;
    for (i, s) in l.cols[ci].wins.iter_mut().enumerate() {
        let share = if i == n - 1 { SHARE_UNIT - given } else { allocs[i] * SHARE_UNIT / total };
        s.share = share as i32;
        given += share;
    }
}

fn winresize_in(l: &mut Layout, ci: usize, wi: usize, r: Rect, keepextra: bool, info: &dyn Info) -> i32 {
    let id = l.cols[ci].wins[wi].window;
    // wintaglines: the tag laid out in all of r tells how many lines fit
    let tag_maxlines = tag_lines_fit(info, r.dy()).max(0);
    // in a strip the tag is its box: one line, whatever its text would
    // wrap to (the text is not drawn there to be measured)
    let taglines = if is_strip(r) { tag_maxlines.min(1) } else { info.taglines(id, r.dx(), tag_maxlines).max(0) };
    let y = (r.y0 + tag_height(info, taglines)).min(r.y1.max(r.y0));
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
    let mut extra = 0;
    if body.dy() <= 0 {
        body.y1 = body.y0;
    } else if !keepextra {
        extra = body.dy() % bf;
        body.y1 -= extra;
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
    s.extra = extra;
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
    unfull(l, ci, info);
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
            if v.fr_maxlines(bf) > 3 && v.body_all_dy() > minht {
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
        y = y.min(ymax - (tag_height(info, v.taglines) + bf + BORDER + 1));
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
            Slot { window: id, r, body: Rect::new(r.x0, (r.y0 + font + 1).min(r.y1), r.x1, r.y1), taglines: 1, nlines: 0, frmax: body_dy / bf, maxlines: body_dy / bf, extra: 0, share: 0, premax: 0 }
        }
        Adding::Existing(s) => s,
    };
    l.cols[ci].wins.insert(i, slot);
    // a new window's tag is set right after (winsettag → winresize); a
    // moved one is resized into its place: both come to this
    winresize(l, ci, i, r, true, info);
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
    unfull(l, ci, info);
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
    winresize(l, ci, idx, r, true, info);
    (s, if up { Some(id) } else { None })
}

/// acme's `colresize`: the column gets rectangle `r`; its windows keep
/// their proportions. A window grown to the whole column stays so.
pub fn colresize(l: &mut Layout, ci: usize, r: Rect, info: &dyn Info) {
    let font = info.font_height().max(1);
    if let Some(f) = l.cols[ci].full {
        if let Some(wi) = l.cols[ci].wins.iter().position(|s| s.window == f.window) {
            lay_full(l, ci, wi, r, info);
            l.cols[ci].r = r;
            return;
        }
        l.cols[ci].full = None;
    }
    let n = l.cols[ci].wins.len();
    let mut r1 = r;
    r1.y1 = r1.y0 + font; // the column tag
    r1.y0 = r1.y1;
    r1.y1 += BORDER;
    r1.y1 = r.y1;
    // the windows sized by their shares of the column's window space
    // (acme scales the last heights, which, trimmed to whole lines each
    // time, hand their remainders down the column resize after resize)
    let stale = n > 0 && (l.cols[ci].wins.iter().any(|s| s.share <= 0) || l.cols[ci].wins.iter().map(|s| s.share as i64).sum::<i64>() != SHARE_UNIT);
    if stale {
        sync_shares(l, ci);
    }
    let bottom = r.y1;
    let space = (bottom - r.y0 - font - n as i32 * BORDER).max(0) as i64;
    for i in 0..n {
        l.cols[ci].wins[i].maxlines = 0;
        if i == n - 1 {
            r1.y1 = bottom;
        } else {
            let alloc = (l.cols[ci].wins[i].share as i64 * space + SHARE_UNIT / 2) / SHARE_UNIT;
            r1.y1 = r1.y0 + BORDER + alloc as i32;
        }
        r1.y1 = r1.y1.max(r1.y0 + BORDER + font);
        let mut r2 = r1;
        r2.y1 = r2.y0 + BORDER;
        r1.y0 = r2.y1;
        r1.y0 = winresize_in(l, ci, i, r1, i == n - 1, info);
    }
    l.cols[ci].r = r;
}

/// acme's `colgrow`: button 1 makes window `wi` a bit bigger, 2 as big as
/// can be, 3 the whole column; -1 just refits it in its own space.
pub fn colgrow(l: &mut Layout, ci: usize, wi: usize, but: i32, info: &dyn Info) {
    let font = info.font_height().max(1);
    let n = l.cols[ci].wins.len();
    let mut cr = l.cols[ci].r;
    if but < 0 && l.cols[ci].full.is_some() {
        // the whole column's, or hidden and nowhere to fit
        if !l.cols[ci].hides(l.cols[ci].wins[wi].window) {
            lay_full(l, ci, wi, cr, info);
        }
        return;
    }
    if but == 3 {
        colfull(l, ci, wi, info);
        return;
    }
    unfull(l, ci, info);
    let bf = |l: &Layout, j: usize| info.body_font_height(l.cols[ci].wins[j].window).max(1);
    if but < 0 {
        // make sure window fills its own space properly
        let mut r = l.cols[ci].wins[wi].r;
        if wi == n - 1 {
            r.y1 = cr.y1;
        } else {
            r.y1 = l.cols[ci].wins[wi + 1].r.y0 - BORDER;
        }
        winresize(l, ci, wi, r, true, info);
        return;
    }
    cr.y0 = l.cols[ci].wins[0].r.y0;
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
    let mut y2 = cr.y1;
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
}

// ---- the whole column --------------------------------------------------------------------

/// Window `wi` laid out in the whole of column rectangle `cr`, below its
/// tag; the others obscured, as in acme (no lines fit; their rectangles
/// go stale).
fn lay_full(l: &mut Layout, ci: usize, wi: usize, cr: Rect, info: &dyn Info) {
    let font = info.font_height().max(1);
    let r = Rect::new(cr.x0, cr.y0 + font + BORDER, cr.x1, cr.y1);
    winresize_in(l, ci, wi, r, true, info);
    for (j, s) in l.cols[ci].wins.iter_mut().enumerate() {
        if j != wi {
            s.frmax = 0;
        }
    }
}

/// B3 on a window's box (acme's): grown to the whole column, keeping
/// its place in it, the others hidden behind it until B3 again (or B1)
/// gives them back. On the window already grown so, gives them back.
pub fn colfull(l: &mut Layout, ci: usize, wi: usize, info: &dyn Info) {
    let w = l.cols[ci].wins[wi].window;
    if l.cols[ci].full.is_some_and(|f| f.window == w) {
        unfull(l, ci, info);
        return;
    }
    unfull(l, ci, info);
    sync_shares(l, ci);
    let s = l.cols[ci].wins[wi];
    l.cols[ci].full = Some(Full { window: w, share: s.share, r: s.r, extra: s.extra, col: l.cols[ci].r });
    let cr = l.cols[ci].r;
    lay_full(l, ci, wi, cr, info);
}

/// Column `ci` laid out again with every window in it, if one was grown
/// to the whole of it: each where it was, or, the column resized
/// meanwhile, at the share it had. Whether one was.
pub fn unfull(l: &mut Layout, ci: usize, info: &dyn Info) -> bool {
    let Some(f) = l.cols[ci].full.take() else { return false };
    let Some(wi) = l.cols[ci].wins.iter().position(|s| s.window == f.window) else {
        let cr = l.cols[ci].r;
        colresize(l, ci, cr, info);
        return true;
    };
    l.cols[ci].wins[wi].share = f.share;
    if l.cols[ci].r != f.col {
        whole_shares(l, ci);
        let cr = l.cols[ci].r;
        colresize(l, ci, cr, info);
        return true;
    }
    // the others' rectangles went stale untouched: each back as it was
    for j in 0..l.cols[ci].wins.len() {
        let (r, extra) = if j == wi { (f.r, f.extra) } else { (l.cols[ci].wins[j].r, l.cols[ci].wins[j].extra) };
        winresize_in(l, ci, j, r, true, info);
        l.cols[ci].wins[j].extra = extra;
    }
    true
}

/// The shares made whole again (a window may have come or gone): each
/// in proportion to what it had, summing to the unit.
fn whole_shares(l: &mut Layout, ci: usize) {
    let c = &mut l.cols[ci];
    let total: i64 = c.wins.iter().map(|s| s.share.max(1) as i64).sum();
    let n = c.wins.len();
    let mut given = 0i64;
    for (k, s) in c.wins.iter_mut().enumerate() {
        let share = if k == n - 1 { SHARE_UNIT - given } else { s.share.max(1) as i64 * SHARE_UNIT / total.max(1) };
        s.share = share.max(1) as i32;
        given += share;
    }
}

/// Is window `wi` the one B2 gave its column: every other window laid
/// out down to its tag, and the shares they had kept to go back to.
pub fn is_maximized_win(c: &Column, wi: usize) -> bool {
    c.wins.len() > 1 && c.wins.iter().enumerate().all(|(j, s)| j == wi || s.body.dy() <= 0) && c.wins.iter().any(|s| s.premax > 0)
}

/// Shift-B1 on a window's box: minimized -- down to its tag, where it
/// stands in the column, as B2 on another's box leaves it -- its body's
/// room to the window under it (over it, the last). A window alone in
/// its column, or down to its tag already, stays as it is. B1 on its box
/// grows it again (`colgrow`).
pub fn colminimize(l: &mut Layout, ci: usize, wi: usize, info: &dyn Info) {
    unfull(l, ci, info);
    let n = l.cols[ci].wins.len();
    if n < 2 || wi >= n || l.cols[ci].wins[wi].body.dy() <= 0 {
        return;
    }
    let s = l.cols[ci].wins[wi];
    let tag = tag_height(info, s.taglines.max(1));
    if wi + 1 < n {
        // the one under it comes up to its tag
        let mine = Rect::new(s.r.x0, s.r.y0, s.r.x1, s.r.y0 + tag);
        winresize(l, ci, wi, mine, true, info);
        let below = l.cols[ci].wins[wi + 1].r;
        let r = Rect::new(below.x0, mine.y1 + BORDER, below.x1, below.y1);
        winresize(l, ci, wi + 1, r, wi + 1 == n - 1, info);
    } else {
        // the last: its tag at the column's foot, the one over it down to it
        let foot = l.cols[ci].r.y1;
        let mine = Rect::new(s.r.x0, foot - tag, s.r.x1, foot);
        let above = l.cols[ci].wins[wi - 1].r;
        let r = Rect::new(above.x0, above.y0, above.x1, mine.y0 - BORDER);
        winresize(l, ci, wi - 1, r, true, info);
        winresize(l, ci, wi, mine, true, info);
    }
}

/// B2 on a window's box: maximized -- as big as it can be, the others in
/// the column down to their tags, as acme's B2: the share each had kept,
/// for B1 on its box to give back.
pub fn colmaximize(l: &mut Layout, ci: usize, wi: usize, info: &dyn Info) {
    unfull(l, ci, info);
    if !is_maximized_win(&l.cols[ci], wi) {
        sync_shares(l, ci);
        for s in l.cols[ci].wins.iter_mut() {
            s.premax = s.share.max(1);
        }
    }
    colgrow(l, ci, wi, 2, info);
}

/// B1 on the box of the window B2 maximized: every window in the column
/// back at the share it had before.
pub fn colunmaximize(l: &mut Layout, ci: usize, info: &dyn Info) {
    unfull(l, ci, info);
    for s in l.cols[ci].wins.iter_mut() {
        if s.premax > 0 {
            s.share = s.premax;
        }
        s.premax = 0;
    }
    whole_shares(l, ci);
    let cr = l.cols[ci].r;
    colresize(l, ci, cr, info);
}

// ---- the stash --------------------------------------------------------------------------

/// Fewer body lines than this, a window comes back from the stash with an
/// even share of its column rather than the little it had.
const FEW_LINES: i32 = 5;

/// Column `ci`'s windows in its order with those stashed from it put
/// back where they were: (window, stashed). A stashed window goes under
/// the one it was under (laid out or stashed); one whose window is gone,
/// which leaving hands on (`left`) and so should not be, at the end.
pub fn stash_order(l: &Layout, ci: usize) -> Vec<(WindowId, bool)> {
    let c = &l.cols[ci];
    let mut order: Vec<(WindowId, bool)> = c.wins.iter().map(|s| (s.window, false)).collect();
    let mut rest: Vec<&Stashed> = l.stash.iter().filter(|s| s.col == c.id).collect();
    loop {
        let before = rest.len();
        rest.retain(|s| {
            let at = match s.above {
                None => Some(0),
                Some(a) => order.iter().position(|&(w, _)| w == a).map(|i| i + 1),
            };
            match at {
                Some(i) => {
                    order.insert(i, (s.slot.window, true));
                    false
                }
                None => true,
            }
        });
        if rest.is_empty() || rest.len() == before {
            break;
        }
    }
    order.extend(rest.iter().map(|s| (s.slot.window, true)));
    order
}

/// The window just above `w` in column `ci`'s order, stashed or not.
pub fn stash_above(l: &Layout, ci: usize, w: WindowId) -> Option<WindowId> {
    let order = stash_order(l, ci);
    let i = order.iter().position(|&(x, _)| x == w)?;
    i.checked_sub(1).map(|j| order[j].0)
}

/// The shares read off the rectangles unless they are current.
fn fresh_shares(l: &mut Layout, ci: usize) {
    let c = &l.cols[ci];
    if !c.wins.is_empty() && (c.wins.iter().any(|s| s.share <= 0) || c.wins.iter().map(|s| s.share as i64).sum::<i64>() != SHARE_UNIT) {
        sync_shares(l, ci);
    }
}

/// Window `wi` of column `ci` put in the stash (⌘M, `Stash`), its space
/// going to a neighbour as a closed window's does.
pub fn stash(l: &mut Layout, ci: usize, wi: usize, info: &dyn Info) {
    unfull(l, ci, info);
    fresh_shares(l, ci);
    let w = l.cols[ci].wins[wi].window;
    let above = stash_above(l, ci, w);
    let (slot, _) = colclose(l, ci, wi, info);
    l.stash.push(Stashed { slot, col: l.cols[ci].id, above });
}

/// Stashed window `si` brought back: under the window it was under in
/// its column, with the share of the column it had, the others giving
/// it up in proportion -- or, if it had only a few lines (stashed as it
/// was made, as an +Errors window is), an even share. A window whose column is gone comes back at the
/// foot of column `or` instead. Returns the column it went to; None
/// (and it stays stashed) when there is none.
pub fn recall(l: &mut Layout, si: usize, or: Option<usize>, info: &dyn Info) -> Option<usize> {
    let home = l.column_index(l.stash[si].col);
    let ci = home.or(or).filter(|&ci| ci < l.cols.len())?;
    unfull(l, ci, info);
    fresh_shares(l, ci);
    let Stashed { mut slot, above, .. } = l.stash.remove(si);
    let at = if home.is_some() {
        // under the nearest laid-out window above it in the column's order
        let id = l.cols[ci].id;
        let mut up = above;
        loop {
            match up {
                None => break 0,
                Some(a) => {
                    if let Some(i) = l.cols[ci].wins.iter().position(|s| s.window == a) {
                        break i + 1;
                    }
                    match l.stash.iter().find(|s| s.slot.window == a && s.col == id) {
                        Some(s) => up = s.above,
                        None => break l.cols[ci].wins.len(),
                    }
                }
            }
        }
    } else {
        l.cols[ci].wins.len()
    };
    let n = l.cols[ci].wins.len() as i64;
    let even = SHARE_UNIT / (n + 1);
    let mine = if slot.share > 0 && n > 0 { (slot.share as i64).min(SHARE_UNIT - n) } else { even };
    let mine = if slot.frmax < FEW_LINES { mine.max(even) } else { mine };
    let mut given = 0i64;
    for s in l.cols[ci].wins.iter_mut() {
        s.share = ((s.share as i64 * (SHARE_UNIT - mine)) / SHARE_UNIT).max(1) as i32;
        given += s.share as i64;
    }
    slot.share = (SHARE_UNIT - given) as i32;
    slot.premax = 0;
    l.cols[ci].wins.insert(at, slot);
    let cr = l.cols[ci].r;
    colresize(l, ci, cr, info);
    Some(ci)
}

/// Window `w`, which was under `above`, has left column `ci` (closed, or
/// moved to another): the windows stashed under it go under `above`
/// instead.
pub fn left(l: &mut Layout, ci: usize, w: WindowId, above: Option<WindowId>) {
    let id = l.cols[ci].id;
    for s in l.stash.iter_mut() {
        if s.col == id && s.above == Some(w) {
            s.above = above;
        }
    }
}

/// Stashed window `si` made the latest put away, as though stashed just
/// now (worked in where it is shown): first among the stash's cards.
pub fn restash(l: &mut Layout, si: usize) {
    let s = l.stash.remove(si);
    l.stash.push(s);
}

/// A stashed window taken out of the stash for good (closed): its slot.
pub fn unstash(l: &mut Layout, si: usize) -> Slot {
    let s = l.stash.remove(si);
    if let Some(ci) = l.column_index(s.col) {
        left(l, ci, s.slot.window, s.above);
    }
    s.slot
}

/// acme's `colsort`: windows in name order, keeping their heights.
pub fn colsort(l: &mut Layout, ci: usize, name: impl Fn(WindowId) -> String, info: &dyn Info) {
    unfull(l, ci, info);
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
/// Which side of a column a new one goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// A window's box (window `wi` of column `ci`), pressed at `op`, let go at
/// `p` near a column's left or right edge: a new column there, the window
/// in it (as VS Code's and Zed's editors split) -- which column, which
/// side. Near is the outer eighth of the column (16 to 48 across). The
/// window's own column, whose box is at its left: its left edge only with
/// the pointer pushed onto the edge itself, so a drag up or down drifting
/// left does not split it; its right only after a move clearly right; and
/// not at all for its only window, which would only move it. A column too
/// narrow to halve is not split.
pub fn split_at(l: &Layout, ci: usize, wi: usize, op: (i32, i32), p: (i32, i32)) -> Option<(usize, Side)> {
    if (p.0 - op.0).abs() < 5 && (p.1 - op.1).abs() < 5 {
        return None;
    }
    let tc = rowwhichcol(l, p)?;
    let r = l.cols[tc].r;
    if r.dx() < 200 || is_strip(r) {
        return None;
    }
    let own = tc == ci;
    if own && l.cols[ci].wins.len() == 1 {
        return None;
    }
    let _ = wi;
    let near = (r.dx() / 8).clamp(16, 48);
    if p.0 >= r.x1 - near && (!own || p.0 - op.0 > 30) {
        return Some((tc, Side::Right));
    }
    if (own && p.0 <= r.x0 + 2) || (!own && p.0 <= r.x0 + near) {
        return Some((tc, Side::Left));
    }
    None
}

/// The split `split_at` found: column `tc` halved, `new` the half on
/// `side`, and window `wi` of column `ci` moved into it.
pub fn coldragsplit(l: &mut Layout, ci: usize, wi: usize, tc: usize, side: Side, new: AddingCol, info: &dyn Info) -> Option<Warp> {
    unfull(l, ci, info);
    let w = l.cols[ci].wins[wi].window;
    let r = l.cols[tc].r;
    // the new column on the right half (rowadd adds after the column it
    // splits); on the left, it and the old one change places
    let mut ni = rowadd(l, new, Some(r.x0 + r.dx() / 2), info)?;
    if side == Side::Left {
        let oi = ni - 1;
        let (ra, rb) = (l.cols[oi].r, l.cols[ni].r);
        l.cols.swap(oi, ni);
        colresize(l, oi, ra, info);
        colresize(l, ni, rb, info);
        ni = oi;
    }
    // the window, from wherever its column now is, into the new one
    let ci = l.cols.iter().position(|c| c.wins.iter().any(|s| s.window == w))?;
    let wi = l.cols[ci].wins.iter().position(|s| s.window == w)?;
    let above = stash_above(l, ci, w);
    let (slot, _) = colclose(l, ci, wi, info);
    left(l, ci, w, above);
    coladd(l, ni, Adding::Existing(slot), None, info);
    close_if_left_empty(l, ci, info);
    Some(Warp::WinButton(w))
}

/// Column `ci`, which a window was just dragged out of: gone, its room
/// to its neighbour, if that left it with none -- an empty column made
/// so (Newcol) stays, but one emptied by a drag is no one's. Not the
/// last column of all.
fn close_if_left_empty(l: &mut Layout, ci: usize, info: &dyn Info) {
    if ci < l.cols.len() && l.cols[ci].wins.is_empty() && l.cols.len() > 1 {
        rowclose(l, ci, info);
    }
}

pub fn coldragwin(l: &mut Layout, ci: usize, wi: usize, but: i32, op: (i32, i32), p: (i32, i32), info: &dyn Info) -> Option<Warp> {
    let font = info.font_height().max(1);
    let n = l.cols[ci].wins.len();
    let w = l.cols[ci].wins[wi].window;
    let (mut px, py) = p;
    if (px - op.0).abs() < 5 && (py - op.1).abs() < 5 {
        // a window in a strip: the first click is the column's box's, which
        // brings the column back; the window grows from the next one, once
        // it can be seen
        if is_strip(l.cols[ci].r) {
            rowgrow(l, ci, 1, info);
            return Some(Warp::WinButton(w));
        }
        // B2 maximizes it (the others down to their tags), and B1 on it
        // then gives them back their sizes; B3 grows it to the whole
        // column, and B3 again or B1 gives the others back; B1 otherwise
        // grows it a little
        match but {
            2 => colmaximize(l, ci, wi, info),
            3 => colfull(l, ci, wi, info),
            1 if l.cols[ci].full.is_some() => {
                unfull(l, ci, info);
            }
            1 if is_maximized_win(&l.cols[ci], wi) => colunmaximize(l, ci, info),
            _ => colgrow(l, ci, wi, but, info),
        }
        return Some(Warp::WinButton(w));
    }
    // a drag works on the column as it is laid out, every window in it
    unfull(l, ci, info);
    // is it a flick to the right?
    if (py - op.1).abs() < 10 && px > op.0 + 30 && rowwhichcol(l, (px, py)) == Some(ci) {
        px = op.0 + l.cols[ci].wins[wi].r.dx(); // yes: toss to next column
    }
    let nc = rowwhichcol(l, (px, py));
    if let Some(nc) = nc {
        if nc != ci {
            let above = stash_above(l, ci, w);
            let (slot, _) = colclose(l, ci, wi, info);
            left(l, ci, w, above);
            coladd(l, nc, Adding::Existing(slot), Some(py), info);
            close_if_left_empty(l, ci, info);
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
    // a hidden column's rectangle is stale, and may lie under the full one
    (0..l.cols.len()).find(|&i| l.shows(i) && l.cols[i].r.contains(p.0, p.1))
}

/// A column being added.
pub enum AddingCol {
    New { id: ColumnId, tag: BufferId },
    Existing(Column),
}

/// acme's `rowadd`: a column at `x`, or with no `x`, taking 40% of the
/// last column. `None` if the column it would split is too narrow.
pub fn rowadd(l: &mut Layout, c: AddingCol, x: Option<i32>, info: &dyn Info) -> Option<usize> {
    reveal(l, info);
    settle(l);
    let font = info.font_height().max(1);
    let mut r = l.r;
    r.y0 = l.r.y0 + font + BORDER;
    let mut x = x.unwrap_or(r.x0 - 1);
    let n = l.cols.len();
    if x < r.x0 && n > 0 {
        // steal 40% of last column by default; a strip has nothing to
        // give, and there the widest column gives instead
        let mut di = n - 1;
        if l.cols[di].r.dx() < 100 {
            di = (0..n).max_by_key(|&j| l.cols[j].r.dx()).unwrap_or(di);
        }
        let d = &l.cols[di];
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
            l.cols.insert(i, Column { id, tag, r, wins: Vec::new(), full: None, restore: 0, stashed: false, after: None });
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
    if let Some(fi) = l.full_index() {
        // the full column is the row; the hidden ones are laid out afresh
        // when they come back, so their stale rectangles can stay so
        l.r = r;
        let mut cr = r;
        cr.y0 += font + BORDER;
        colresize(l, fi, cr, info);
        return;
    }
    let or = l.r;
    let deltax = r.x0 - or.x0;
    l.r = r;
    let mut r = r;
    r.y0 += font + BORDER; // the top row's tag, then a border
    let n = l.cols.len();
    // strips stay strips: a column put away keeps a strip's width, and
    // the columns with room share the rest as they shared it before
    // (scaled with the rest, a strip would come out of it, and the last
    // column -- a strip, at the row's right -- would take what is over)
    let strips = l.cols.iter().filter(|c| is_strip(c.r)).count();
    let open: i32 = l.cols.iter().filter(|c| !is_strip(c.r)).map(|c| c.r.dx().max(0)).sum();
    if strips > 0 && strips < n && open > 0 && or.dx() > 0 {
        let total = (r.dx() - (n as i32 - 1) * BORDER).max(0);
        let room = (total - strips as i32 * STRIP).max(0);
        let last_open = (0..n).rev().find(|&j| !is_strip(l.cols[j].r)).unwrap_or(n - 1);
        let mut given = 0;
        let w: Vec<i32> = (0..n)
            .map(|j| {
                if is_strip(l.cols[j].r) {
                    STRIP
                } else if j == last_open {
                    (room - given).max(STRIP)
                } else {
                    let x = (l.cols[j].r.dx().max(0) as i64 * room as i64 / open as i64) as i32;
                    given += x;
                    x.max(STRIP)
                }
            })
            .collect();
        let mut x = r.x0;
        for (j, &width) in w.iter().enumerate() {
            let mut r1 = r;
            r1.x0 = x;
            r1.x1 = if j == n - 1 { r.x1 } else { x + width };
            colresize(l, j, r1, info);
            x = r1.x1 + BORDER;
        }
        return;
    }
    let mut r1 = r;
    r1.x1 = r1.x0;
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
    let ci = revealed(l, ci, info);
    settle(l);
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

/// `colgrow` turned on its side: a click on column `ci`'s box does to the
/// row what a click on a window's box does to its column (acme has no
/// such). Button 1 widens it by half again, or a fifth of the row if that
/// is more, taking from the columns beside it, nearest first, right then
/// left, each giving at most half of what it has beyond a strip. Button 2
/// makes it as wide as can be, every other column a strip. Button 3 gives
/// it the whole row and hides the others (`Layout::full`), until a click
/// on its box lays the row out again with the others back as strips --
/// as acme's windows come back as tags after a window took the column.
pub fn rowgrow(l: &mut Layout, mut ci: usize, but: i32, info: &dyn Info) {
    let n = l.cols.len();
    if ci >= n {
        return;
    }
    let row = l.r;
    // B1 or B3 on the box of a column given the row: the others back
    // where they were; anything else works on the row laid out
    if l.full.is_some() {
        if but == 1 || but == 3 {
            reveal(l, info);
            return;
        }
        ci = revealed(l, ci, info);
    }
    // as a window's box does. B1 on a strip brings it back where it
    // stands (one an older apex put away at the right, where it stood),
    // and on the column B2 maximized, every minimized one; B2 maximizes
    // it, the others minimized; B3 gives it the whole row, the others
    // hidden. B4 has nothing to do on a column
    if but == 4 {
        return;
    }
    if but == 1 && is_strip(l.cols[ci].r) {
        rowbringback(l, ci, info);
        return;
    }
    if but == 1 && is_maximized_col(l, ci) {
        rowunmaximize(l, ci, info);
        return;
    }
    if but == 3 {
        rowfull(l, ci, info);
        return;
    }
    if but == 2 {
        rowmaximize(l, ci, info);
        return;
    }
    // the width the columns share, the borders between them taken out
    let total = (row.dx() - (n as i32 - 1) * BORDER).max(0);
    // each column's width now
    let mut w: Vec<i32> = l.cols.iter().map(|c| c.r.dx().max(0)).collect();
    let most = (total - (n as i32 - 1) * STRIP).max(STRIP);
    // a step, not a leap: columns are few and wide, and a window's
    // growth (half again) would take most of a neighbour at a click.
    // A fifth of its width or a twelfth of the row, whichever is
    // more; a neighbour gives at most a third of what it can spare
    let mine = w[ci];
    let mut dw = (mine / 5).max(row.dx() / 12).min(most - mine).max(0);
    let give = |w: &mut Vec<i32>, j: usize, dw: &mut i32| {
        let spare = (w[j] - STRIP).max(0);
        let take = (*dw).min((spare + 2) / 3);
        w[j] -= take;
        w[ci] += take;
        *dw -= take;
    };
    for k in 1..n {
        if ci + k < n {
            give(&mut w, ci + k, &mut dw);
        }
        if k <= ci {
            give(&mut w, ci - k, &mut dw);
        }
    }
    // no column narrower than a strip; what one lacks, the grown one pays
    for j in 0..n {
        if j != ci && w[j] < STRIP {
            w[ci] -= STRIP - w[j];
            w[j] = STRIP;
        }
    }
    w[ci] = w[ci].max(STRIP);
    rowpack(l, &w, info);
    settle(l);
}

/// Where a put-away column comes back: right of the column it stood
/// right of, when that one has room; right of where that one would come
/// back, when it is put away too; at the left when it stood there.
fn back_at(l: &Layout, c: &Column) -> usize {
    // the columns in their order (with room or minimized) are those
    // before the stashed ones
    let open = |j: usize| !l.cols[j].stashed;
    let wide_end = (0..l.cols.len()).find(|&j| !open(j)).unwrap_or(l.cols.len());
    let mut after = c.after;
    let mut seen = 0;
    loop {
        match after {
            None => return 0,
            Some(a) => match l.column_index(a) {
                Some(i) if i < wide_end && open(i) => return i + 1,
                Some(i) if seen < l.cols.len() => {
                    after = l.cols[i].after;
                    seen += 1;
                }
                _ => return wide_end,
            },
        }
    }
}

/// B1 on a strip at the row's right: the column back where it stood, at
/// the width it had, taken from the columns with room nearest it.
pub fn rowbringback(l: &mut Layout, ci: usize, info: &dyn Info) {
    if ci >= l.cols.len() || !is_strip(l.cols[ci].r) {
        return;
    }
    // minimized (or left where it stood by an older apex): back where
    // it stands
    if !l.cols[ci].stashed {
        restore_one(l, ci, info);
        l.cols[ci].after = None;
        return;
    }
    let mut w: Vec<i32> = l.cols.iter().map(|c| c.r.dx().max(0)).collect();
    let c = l.cols.remove(ci);
    w.remove(ci);
    let at = back_at(l, &c).min(l.cols.len());
    l.cols.insert(at, c);
    w.insert(at, STRIP);
    rowpack(l, &w, info);
    restore_one(l, at, info);
    l.cols[at].after = None;
    l.cols[at].stashed = false;
}

/// Every minimized strip back at the width it had at once (a fifth of
/// the row when it has none), column `ci` keeping the rest, the stashed
/// left as they are: B1 on the column B2 maximized.
fn rowrestore_all(l: &mut Layout, ci: usize, info: &dyn Info) {
    let n = l.cols.len();
    let row = l.r.dx() as i64;
    let total = (l.r.dx() - (n as i32 - 1) * BORDER).max(0);
    let had = |c: &Column| match c.restore {
        s if s > 0 => ((s as i64 * row + SHARE_UNIT / 2) / SHARE_UNIT) as i32,
        _ => (row / 5) as i32,
    };
    let mut w: Vec<i32> = l.cols.iter().map(|c| if c.stashed { STRIP } else if is_strip(c.r) || c.restore > 0 { had(c).max(STRIP) } else { c.r.dx() }).collect();
    let others: i32 = (0..n).filter(|&j| j != ci).map(|j| w[j]).sum();
    // what is left is ci's; too little, and the others give in proportion
    let room = total - others;
    if room < MINCOL {
        let scale = (total - MINCOL).max(0) as f64 / others.max(1) as f64;
        for j in 0..n {
            if j != ci {
                w[j] = ((w[j] as f64 * scale) as i32).max(STRIP);
            }
        }
    }
    w[ci] = total - (0..n).filter(|&j| j != ci).map(|j| w[j]).sum::<i32>();
    for c in l.cols.iter_mut().filter(|c| !c.stashed) {
        c.restore = 0;
    }
    rowpack(l, &w, info);
}

/// Is column `ci` the one B2 maximized: the only one with room, the
/// others minimized (the stashed apart).
pub fn is_maximized_col(l: &Layout, ci: usize) -> bool {
    !is_strip(l.cols[ci].r) && (0..l.cols.len()).any(|j| j != ci && !l.cols[j].stashed) && (0..l.cols.len()).all(|j| j == ci || l.cols[j].stashed || is_strip(l.cols[j].r))
}

/// B2 on a column's box: maximized -- as wide as it can be, the others
/// minimized where they stand, as a window's B2 leaves the others their
/// tags -- and not stashed. Each keeps the width it had (the maximized
/// one too) for B1 on its box to give back. A strip is brought back
/// first.
pub fn rowmaximize(l: &mut Layout, ci: usize, info: &dyn Info) {
    let n = l.cols.len();
    if n < 2 || l.full.is_some() {
        return;
    }
    let id = l.cols[ci].id;
    if is_strip(l.cols[ci].r) {
        rowbringback(l, ci, info);
    }
    let Some(ci) = l.column_index(id) else { return };
    if is_maximized_col(l, ci) {
        return;
    }
    let mut w: Vec<i32> = l.cols.iter().map(|c| c.r.dx().max(0)).collect();
    for j in 0..n {
        if l.cols[j].stashed || is_strip(l.cols[j].r) {
            continue;
        }
        if j == ci {
            if l.cols[j].restore == 0 {
                remember(l, j, w[j]);
            }
        } else {
            remember(l, j, w[j]);
            w[j] = STRIP;
        }
    }
    let total = (l.r.dx() - (n as i32 - 1) * BORDER).max(0);
    w[ci] = total - (0..n).filter(|&j| j != ci).map(|j| w[j]).sum::<i32>();
    rowpack(l, &w, info);
}

/// Shift-B1 on a column's box: minimized where it stands -- a strip, as
/// B2 on another's box leaves it -- its width to the nearest column with
/// room, right of it first, and kept for B1 on the strip to give back.
/// Not the last column with room. A column given the whole row (B3) is
/// laid out as it was first.
pub fn rowminimize(l: &mut Layout, ci: usize, info: &dyn Info) {
    if ci >= l.cols.len() {
        return;
    }
    let id = l.cols[ci].id;
    reveal(l, info);
    let Some(ci) = l.column_index(id) else { return };
    if is_strip(l.cols[ci].r) || l.cols[ci].stashed {
        return;
    }
    let roomy = |j: usize| !l.cols[j].stashed && !is_strip(l.cols[j].r);
    let n = l.cols.len();
    let Some(to) = (ci + 1..n).find(|&j| roomy(j)).or_else(|| (0..ci).rev().find(|&j| roomy(j))) else { return };
    let mut w: Vec<i32> = l.cols.iter().map(|c| c.r.dx().max(0)).collect();
    remember(l, ci, w[ci]);
    w[to] += w[ci] - STRIP;
    w[ci] = STRIP;
    rowpack(l, &w, info);
}

/// B1 on the box of the column B2 maximized: the minimized ones back at
/// the widths they had, it at the rest.
pub fn rowunmaximize(l: &mut Layout, ci: usize, info: &dyn Info) {
    rowrestore_all(l, ci, info);
}

/// B3 on a column's box, as on a window's: column `ci` given the whole
/// row, the others hidden behind it (`Layout::full`), their rectangles
/// left as they were, until B3 again or B1 on its box gives them back
/// (`reveal`). The width it had is kept as its share of the row
/// (`Column::restore`), to go back to.
pub fn rowfull(l: &mut Layout, ci: usize, info: &dyn Info) {
    let row = l.r;
    // another given the row already: the row as it was first (every
    // column where it stood, so `ci` is still this one's)
    reveal(l, info);
    if l.cols.len() < 2 {
        return;
    }
    let width = l.cols[ci].r.dx();
    remember(l, ci, width);
    l.full = Some(l.cols[ci].id);
    let mut r = l.cols[ci].r;
    r.x0 = row.x0;
    r.x1 = row.x1;
    colresize(l, ci, r, info);
}

/// The narrowest a column keeps for its text when it gives width to
/// another, as a drag leaves it (`rowdragcol`).
const MINCOL: i32 = 80 + SCROLLWID;

/// The columns laid out left to right at widths `w`, a border between;
/// the last ends at the row's edge. What the widths leave over or ask
/// beyond the row is the last column with room's (the strips put away
/// at the right keep theirs).
fn rowpack(l: &mut Layout, w: &[i32], info: &dyn Info) {
    let n = l.cols.len();
    let row = l.r;
    let mut w: Vec<i32> = w.iter().take(n).copied().collect();
    let total = row.dx() - (n as i32 - 1).max(0) * BORDER;
    if let Some(k) = (0..w.len()).rev().find(|&j| w[j] > STRIP) {
        let diff = total - w.iter().sum::<i32>();
        w[k] = (w[k] + diff).max(STRIP);
    }
    let mut x = row.x0;
    for (j, width) in w.iter().enumerate().take(n) {
        let mut r = l.cols[j].r;
        r.x0 = x;
        r.x1 = if j == n - 1 { row.x1 } else { x + width };
        colresize(l, j, r, info);
        x = r.x1 + BORDER;
    }
}

/// Column `j`, about to become a strip, `width` wide: remembered as a
/// share of the row, so bringing it back gives it this width again (a
/// share, so a resized window gives it the same part of the row).
/// The width column `j` has in keeping: a strip's, the width it comes
/// back at; one with room holding a strip's width, the width it had
/// before it took it (and goes back to as the strip comes back).
fn natural(l: &Layout, j: usize) -> Option<i32> {
    let row = l.r.dx() as i64;
    match l.cols[j].restore {
        s if s > 0 => Some(((s as i64 * row + SHARE_UNIT / 2) / SHARE_UNIT) as i32),
        _ => None,
    }
}

/// The columns with room have been given widths by hand (a drag, a
/// click, a column added or taken out): those are their widths now, and
/// nothing is held for the strips.
fn settle(l: &mut Layout) {
    for c in l.cols.iter_mut() {
        if !is_strip(c.r) {
            c.restore = 0;
        }
    }
}

fn remember(l: &mut Layout, j: usize, width: i32) {
    let row = l.r.dx() as i64;
    if width > STRIP && row > 0 {
        l.cols[j].restore = ((width as i64 * SHARE_UNIT + row / 2) / row) as i32;
    }
}

/// One strip back at its remembered width (a fifth of the row when it
/// has none), taken from the columns with room nearest it, each keeping
/// enough for its text.
fn restore_one(l: &mut Layout, j: usize, info: &dyn Info) -> bool {
    let n = l.cols.len();
    let mut givers: Vec<usize> = (0..n).filter(|&d| d != j && !is_strip(l.cols[d].r)).collect();
    givers.sort_by_key(|&d| d.abs_diff(j));
    let mut w: Vec<i32> = l.cols.iter().map(|c| c.r.dx().max(0)).collect();
    let had = natural(l, j).unwrap_or((l.r.dx() / 5) as i32);
    let want = (had - w[j]).max(0);
    let spare: i32 = givers.iter().map(|&d| (w[d] - MINCOL).max(0)).sum();
    let give = want.min(spare);
    if give <= 0 {
        return false;
    }
    w[j] += give;
    let mut owe = give;
    // first from those holding width for strips, back to what they had;
    // then from the nearest
    let naturals: Vec<Option<i32>> = (0..n).map(|d| natural(l, d)).collect();
    for &d in &givers {
        if let Some(h) = naturals[d] {
            let take = owe.min((w[d] - h.max(MINCOL)).max(0));
            w[d] -= take;
            owe -= take;
            if w[d] <= h {
                l.cols[d].restore = 0;
            }
        }
    }
    for &d in &givers {
        let take = owe.min((w[d] - MINCOL).max(0));
        w[d] -= take;
        owe -= take;
    }
    l.cols[j].restore = 0;
    rowpack(l, &w, info);
    true
}

/// Column `ci` made something the user can see, because they are being
/// taken to a window in it (a warp): out from behind a column given the
/// row, and back from a strip at the width it had, as a click on its box
/// would bring it.
pub fn uncover(l: &mut Layout, ci: usize, info: &dyn Info) {
    if ci >= l.cols.len() {
        return;
    }
    let ci = if l.full.is_some() && !l.shows(ci) { revealed(l, ci, info) } else { ci };
    if l.full.is_none() && is_strip(l.cols[ci].r) {
        rowbringback(l, ci, info);
    }
}

/// A row with a column grown to the whole of it laid out again, every
/// column where it was: the hidden ones' rectangles were left as they
/// stood, and the grown one goes back to the width it had (its share of
/// the row, `Column::restore`). What a click on its box does, and what
/// anything else that changes the row does first, so it never works on
/// the hidden columns' stale rectangles.
fn reveal(l: &mut Layout, info: &dyn Info) {
    let Some(fi) = l.full_index() else {
        l.full = None;
        return;
    };
    l.full = None;
    let had = natural(l, fi);
    l.cols[fi].restore = 0;
    let mut w: Vec<i32> = l.cols.iter().map(|c| c.r.dx().max(0)).collect();
    // the others as they stood; the grown one at its width, or what the
    // others leave when that is unknown
    let others: i32 = (0..w.len()).filter(|&j| j != fi).map(|j| w[j]).sum();
    let total = (l.r.dx() - (w.len() as i32 - 1) * BORDER).max(0);
    w[fi] = had.unwrap_or(total - others).clamp(STRIP, total.max(STRIP));
    // what the widths leave over or ask beyond the row goes to the widest
    // of the others, not the last (which may be a strip)
    let diff = total - w.iter().sum::<i32>();
    if diff != 0 {
        if let Some(k) = (0..w.len()).filter(|&j| j != fi && w[j] > STRIP).max_by_key(|&j| w[j]).or(Some(fi)) {
            w[k] = (w[k] + diff).max(STRIP);
        }
    }
    rowpack(l, &w, info);
}

/// `reveal`, and where column `ci` stands after it (where it stood).
fn revealed(l: &mut Layout, ci: usize, info: &dyn Info) -> usize {
    let id = l.cols[ci].id;
    reveal(l, info);
    l.column_index(id).unwrap_or(ci)
}

/// acme's `rowdragcol`: the layout box of column `ci` was dragged from
/// `op` to `p`: move it past its neighbours, or resize against the
/// column to its left.
pub fn rowdragcol(l: &mut Layout, ci: usize, but: i32, op: (i32, i32), p: (i32, i32), info: &dyn Info) -> Option<Warp> {
    let n = l.cols.len();
    let id = l.cols[ci].id;
    if (p.0 - op.0).abs() < 5 && (p.1 - op.1).abs() < 5 {
        // a click, not a drag: the column grows, as a window's box does
        rowgrow(l, ci, but, info);
        // put away: the pointer stays where it is -- taken to the strip,
        // it would bring the column straight out again
        if l.column(id).is_some_and(|c| is_strip(c.r)) {
            return None;
        }
        return Some(Warp::ColButton(id));
    }
    // dragged out of a hidden row: the row comes back first
    let ci = revealed(l, ci, info);
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
    rowmovecol(l, ci, p.0, info);
    Some(Warp::ColButton(id))
}

/// The line between column `ci` and the one to its left moved to `x`,
/// neither made narrower than acme allows: `rowdragcol`'s resize, and
/// what a drag of the line itself does.
pub fn rowmovecol(l: &mut Layout, ci: usize, x: i32, info: &dyn Info) {
    if ci == 0 || ci >= l.cols.len() {
        return;
    }
    let cr = l.cols[ci].r;
    let d = l.cols[ci - 1].r;
    let mut x = x;
    // each side at least the least a column may be; taken past half of
    // that, a column is minimized (a strip where it stands, remembering
    // its width), as a window dragged over goes down to its tag
    if x < d.x0 + MINCOL {
        x = if x < d.x0 + MINCOL / 2 {
            if !is_strip(d) {
                remember(l, ci - 1, d.dx());
            }
            d.x0 + STRIP
        } else {
            d.x0 + MINCOL
        };
    }
    if x > cr.x1 - BORDER - MINCOL {
        x = if x > cr.x1 - BORDER - MINCOL / 2 {
            if !is_strip(cr) {
                remember(l, ci, cr.dx());
            }
            cr.x1 - BORDER - STRIP
        } else {
            cr.x1 - BORDER - MINCOL
        };
    }
    let mut r = d;
    r.x1 = x;
    colresize(l, ci - 1, r, info);
    let mut r = cr;
    r.x0 = x + BORDER;
    colresize(l, ci, r, info);
    settle(l);
}

/// A failed `rowadd` of an existing column leaves it out of the row;
/// the caller keeps trying, so hand it back.
fn c_back(_l: &mut Layout, _ci: usize) -> Column {
    unreachable!("rowadd of an existing column into a row that held it cannot fail")
}
