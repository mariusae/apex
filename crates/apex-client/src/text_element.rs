//! A gpui element that paints one acme text (a tag or a body) straight from
//! the core's state: only the visible lines are shaped, and the geometry
//! is recorded on the app so mouse events map to rune offsets.

use std::cell::Cell;

use gpui::{
    fill, font, point, px, relative, size, App, AvailableSpace, Bounds, ContentMask, Element, ElementId,
    Entity, Font, FontId, GlobalElementId, GlyphId, Hsla, InspectorElementId, IntoElement, LayoutId,
    LineLayout, Pixels, Point, Rgba, SharedString, Size, Style, TextRun, Window, WrappedLine,
};

use apex_core::{Text, ViewId};

use crate::app::{Acme, HlKind, Kind};

pub const SCROLLWID: f32 = 12.;
pub const MARGIN: f32 = 16.; // Scrollwid + Scrollgap
pub const TABSTOP: usize = 4;

// acme's colours live in theme.rs (the light theme), with the dark
// theme beside them.
/// `a` towards `b` by `t` (0..1), per channel.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let x = ((a >> shift) & 0xff) as f32;
        let y = ((b >> shift) & 0xff) as f32;
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << shift
    };
    ch(16) | ch(8) | ch(0)
}

pub fn rgb(hex: u32) -> Hsla {
    Rgba::from(gpui::rgb(hex)).into()
}

pub struct Palette {
    pub bg: Hsla,
    pub sel: Hsla,
    pub border: Hsla,
}

pub fn palette(kind: Kind) -> Palette {
    let t = crate::theme::theme();
    match kind {
        Kind::Body => Palette { bg: rgb(t.body_bg), sel: rgb(t.body_sel), border: rgb(t.body_border) },
        _ => Palette { bg: rgb(t.tag_bg), sel: rgb(t.tag_sel), border: rgb(t.tag_border) },
    }
}

/// A window's handle, as a Mac app marks a document: a small circle,
/// hollow when the window is clean and filled when it is not (gold when
/// the file has also changed on disk since); a process behind it rings
/// it in the accent and lights its middle; work going on turns an arc
/// round it, the system's spinner; a tool wanting the user tints the
/// header and puts pjw's face at its far end. Any mark goes with any other.
/// The handle is acme's layout box all the same: B1, B2 and B3 on it do
/// what they always have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dot {
    /// Dirty or stale: the circle filled.
    pub fill: Option<u32>,
    /// The circle's ring: the secondary ink round a clean one, the accent
    /// round a live one (outside the fill, when there is one).
    pub ring: Option<u32>,
    /// Live and clean: the accent in the middle.
    pub core: Option<u32>,
    /// Working: the arc turning.
    pub spin: Option<u32>,
    /// Notified: the header tinted, pjw's face at its end.
    pub badge: bool,
}

pub fn dot(th: &crate::theme::Theme, stale: bool, dirty: bool, live: bool, working: bool, notified: bool) -> Dot {
    let fill = if stale {
        Some(th.stale)
    } else if dirty {
        Some(th.dirty)
    } else {
        None
    };
    Dot {
        fill,
        ring: if live { Some(th.accent) } else if fill.is_none() { Some(th.text_dim) } else { None },
        core: (live && fill.is_none()).then_some(th.accent),
        spin: working.then_some(th.accent),
        badge: notified,
    }
}

/// The circle's radius, and the ring's round a filled one, and the arc's.
const DOT_R: f32 = 3.75;
const RING_R: f32 = 5.;
const SPIN_R: f32 = 5.25;

fn paint_dot(window: &mut Window, d: &Dot, c: Point<Pixels>) {
    let circle = |r: f32| Bounds::new(point(c.x - px(r), c.y - px(r)), size(px(2. * r), px(2. * r)));
    if let Some(f) = d.fill {
        window.paint_quad(fill(circle(DOT_R), rgb(f)).corner_radii(px(DOT_R)));
    }
    if let Some(ring) = d.ring {
        let r = if d.fill.is_some() { RING_R } else { DOT_R };
        window.paint_quad(gpui::quad(circle(r), px(r), gpui::transparent_black(), px(1.25), rgb(ring), gpui::BorderStyle::Solid));
    }
    if let Some(core) = d.core {
        window.paint_quad(fill(circle(1.75), rgb(core)).corner_radii(px(1.75)));
    }
    if let Some(ink) = d.spin {
        paint_spinner(window, c, SPIN_R, 1.5, rgb(ink));
    }
}

/// The system's spinner, as an arc: a quarter and a bit of a circle of
/// radius `r` round `c`, once round in 0.9 s (the window is drawn again
/// each tick while anything spins).
pub fn paint_spinner(window: &mut Window, c: Point<Pixels>, r: f32, width: f32, ink: Hsla) {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_millis() % 900).unwrap_or(0);
    let a0 = ms as f32 / 900. * std::f32::consts::TAU;
    let a1 = a0 + 1.8;
    let at = |a: f32| point(c.x + px(r * a.cos()), c.y + px(r * a.sin()));
    let mut p = gpui::PathBuilder::stroke(px(width));
    p.move_to(at(a0));
    p.arc_to(point(px(r), px(r)), px(0.), false, true, at(a1));
    if let Ok(path) = p.build() {
        window.paint_path(path, ink);
    }
}

/// A Mac scroller's thumb in acme's scrollbar lane: slim and rounded,
/// no track, `s0` to `s1` of the way down, and nothing at all when the
/// whole of it shows. The lane is acme's whatever is drawn in it: B1,
/// B2 and B3 anywhere down it.
pub fn paint_scroller(window: &mut Window, lane: Bounds<Pixels>, s0: f32, s1: f32, ink: Hsla) {
    if s0 <= 0. && s1 >= 1. {
        return;
    }
    let h = lane.size.height - px(6.);
    let (t0, t1) = (h * s0.clamp(0., 1.), h * s1.clamp(0., 1.));
    let len = (t1 - t0).max(px(14.)).min(h);
    let top = (lane.top() + px(3.) + t0).min(lane.bottom() - px(3.) - len);
    let thumb = Bounds::new(point(lane.left() + px(4.), top), size(px(5.), len));
    window.paint_quad(fill(thumb, ink).corner_radii(px(2.5)));
}

/// A drag grip, two columns of three dots: a column's box, and the
/// session's.
fn paint_grip(window: &mut Window, b: Bounds<Pixels>, ink: Hsla) {
    let cx = b.left() + b.size.width / 2.;
    let cy = b.top() + b.size.height / 2.;
    for (dx, dy) in [(-2., -4.), (2., -4.), (-2., 0.), (2., 0.), (-2., 4.), (2., 4.)] {
        let r = 0.9;
        window.paint_quad(fill(Bounds::new(point(cx + px(dx - r), cy + px(dy - r)), size(px(2. * r), px(2. * r))), ink).corner_radii(px(r)));
    }
}

pub struct FontSpec {
    pub font: Font,
    pub size: Pixels,
    pub line_height: Pixels,
}

/// The text faces: the system's own, SF Pro, for text, and SF Mono for
/// a window set mono -- or Menlo, SF Mono's forebear, where SF Mono
/// could not be loaded (`install_mono`).
pub fn font_for(mono: bool) -> FontSpec {
    if mono {
        let family = if SF_MONO.with(|c| c.get()) { SF_MONO_FAMILY } else { "Menlo" };
        FontSpec { font: unjoined(with_symbols(font(family))), size: px(12.), line_height: px(16.) }
    } else {
        FontSpec { font: legible(unjoined(with_symbols(font(".SystemUIFont")))), size: px(14.), line_height: px(20.) }
    }
}

thread_local! {
    static SF_MONO: Cell<bool> = const { Cell::new(false) };
}

/// SF Mono's family, as its file names it. The system keeps it out of
/// reach by name (CoreText hands out Helvetica for "SF Mono" to anyone
/// but Terminal and Xcode), so gpui is given the file itself.
const SF_MONO_FAMILY: &str = ".SF NS Mono";

/// Give gpui SF Mono from the system's own copy, for this process.
pub fn install_mono(cx: &mut App) {
    let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNSMono.ttf") else { return };
    if cx.text_system().add_fonts(vec![std::borrow::Cow::Owned(bytes)]).is_ok() {
        SF_MONO.with(|c| c.set(true));
    }
}

/// No ligatures from the font: every character its own glyph. Where a
/// character is -- for a click, the cursor, a selection's edge, a sweep --
/// is known only from where its glyph starts (gpui's `x_for_index` and
/// `closest_index_for_x`), and the letters a ligature joins have no glyph
/// of their own, so there was no putting the cursor between the f's of an
/// `ff`. Lucida Grande joins ff, fi, fl, ffi and ffl by default (`liga`);
/// `clig` and `calt` are the other ways a font joins letters. All three
/// are named: gpui's `disable_ligatures` turns off `calt` alone.
fn unjoined(f: Font) -> Font {
    let mut features: Vec<(String, u32)> = UNJOINED.iter().map(|t| (t.to_string(), 0)).collect();
    // and a zero with a slash through it, where the face has one (SF
    // Pro and SF Mono do: `zero`), so it is not an O
    features.push(("zero".to_string(), 1));
    Font { features: gpui::FontFeatures(std::sync::Arc::new(features)), ..f }
}

/// SF Pro set for code, as it is set in an editor for it: its high
/// legibility set (`ss06`: an I with bars, an l with a tail, a 1 with a
/// flag, so none is taken for another) and tabular figures (`tnum`: every
/// digit as wide as the next, so numbers line up down a column).
fn legible(f: Font) -> Font {
    let mut features: Vec<(String, u32)> = f.features.0.as_ref().clone();
    features.extend([("ss06".to_string(), 1), ("tnum".to_string(), 1)]);
    Font { features: gpui::FontFeatures(std::sync::Arc::new(features)), ..f }
}

/// The features that join letters into one glyph, all off.
const UNJOINED: [&str; 3] = ["liga", "clig", "calt"];

/// The symbols font apex carries (`install_symbols`), behind whatever
/// font is asked for: the glyphs a program means when it prints one of
/// the private-use characters the Nerd Fonts agreed on -- `exa --icons`,
/// a shell prompt's powerline arrows -- which no font of the system's
/// has.
fn with_symbols(f: Font) -> Font {
    Font { fallbacks: Some(gpui::FontFallbacks::from_fonts(vec![SYMBOLS.to_string()])), ..f }
}

/// The family name of the font in `assets/`, as its `name` table has it.
pub const SYMBOLS: &str = "Symbols Nerd Font Mono";

/// Give the symbols font to CoreText, for this process alone: it is in
/// the binary, not on the machine, so nothing the user has installed (or
/// has not) decides whether a terminal can draw what a program prints.
///
/// It goes to CoreText rather than to gpui's own font source because a
/// fallback is named to CoreText, which resolves the cascade list, and
/// because gpui will not load a family with no `m` in it -- which a font
/// of symbols has no business having.
pub fn install_symbols() {
    const TTF: &[u8] = include_bytes!("../assets/SymbolsNerdFontMono-Regular.ttf");
    // SAFETY: the bytes are static, so the provider needs no release
    // callback and may outlive this call; the font and the provider are
    // CoreFoundation objects we own and hand to the font manager.
    unsafe {
        let provider = CGDataProviderCreateWithData(std::ptr::null_mut(), TTF.as_ptr() as *const _, TTF.len(), std::ptr::null());
        if provider.is_null() {
            eprintln!("apex-ui: the symbols font: no data provider");
            return;
        }
        let font = CGFontCreateWithDataProvider(provider);
        CFRelease(provider);
        if font.is_null() {
            eprintln!("apex-ui: the symbols font is not a font CoreGraphics knows");
            return;
        }
        let mut err: *const std::ffi::c_void = std::ptr::null();
        let ok = CTFontManagerRegisterGraphicsFont(font, &mut err);
        CFRelease(font);
        if !ok {
            eprintln!("apex-ui: the symbols font was not registered");
            if !err.is_null() {
                CFRelease(err);
            }
        }
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDataProviderCreateWithData(info: *mut std::ffi::c_void, data: *const std::ffi::c_void, size: usize, release: *const std::ffi::c_void) -> *const std::ffi::c_void;
    fn CGFontCreateWithDataProvider(provider: *const std::ffi::c_void) -> *const std::ffi::c_void;
}

#[link(name = "CoreText", kind = "framework")]
extern "C" {
    /// Registers a font with CoreText for this process, as a font in an
    /// app bundle's Resources would be. Not in the `core-text` crate.
    fn CTFontManagerRegisterGraphicsFont(font: *const std::ffi::c_void, error: *mut *const std::ffi::c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(v: *const std::ffi::c_void);
}

/// Glyph substitution: Lucida Grande ships a slashed zero (glyph
/// `zeroslash`) that no OpenType feature exposes, so after shaping we swap
/// the glyph id of `zero` for it. Resolved once per process.
#[derive(Clone, Copy)]
struct Subst {
    font_id: FontId,
    from: GlyphId,
    to: GlyphId,
}

thread_local! {
    static SUBST: Cell<Option<Option<Subst>>> = const { Cell::new(None) };
}

fn slashed_zero(window: &Window, cx: &App) -> Option<Subst> {
    if let Some(cached) = SUBST.with(|c| c.get()) {
        return cached;
    }
    let fs = font_for(false);
    // the system's faces have their slashed zero as a feature (`zero`,
    // set in `unjoined`), and CoreText will not open them by name
    if fs.font.family.starts_with('.') {
        SUBST.with(|c| c.set(Some(None)));
        return None;
    }
    let run = TextRun { len: 1, font: fs.font.clone(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    let shaped = window.text_system().shape_line("0".into(), fs.size, &[run], None);
    let lucida = cx.text_system().resolve_font(&fs.font);
    let subst = shaped.runs.first().and_then(|r| {
        let glyph = r.glyphs.first()?;
        if r.font_id != lucida {
            return None;
        }
        let ct = core_text::font::new_from_name(&fs.font.family, f32::from(fs.size) as f64).ok()?;
        let slashed = ct.get_glyph_with_name("zeroslash");
        if slashed == 0 {
            return None;
        }
        // SAFETY: gpui::GlyphId is `#[repr(C)] struct GlyphId(u32)` with a
        // crate-private field and no public constructor.
        let to: GlyphId = unsafe { std::mem::transmute::<u32, GlyphId>(slashed as u32) };
        Some(Subst { font_id: r.font_id, from: glyph.id, to })
    });
    SUBST.with(|c| c.set(Some(subst)));
    subst
}

/// Paint a shaped (possibly wrapped) line glyph by glyph, like gpui's own
/// line painter but with per-glyph colour from `colors` (display byte
/// ranges) and the zero substitution applied.
fn paint_glyphs(
    window: &mut Window,
    layout: &LineLayout,
    subs: &[(usize, usize)],
    origin: Point<Pixels>,
    line_height: Pixels,
    colors: &[(usize, usize, Hsla)],
    subst: Option<Subst>,
) {
    let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
    let baseline = padding_top + layout.ascent;
    let mut sub = 0usize;
    let mut sub_x = px(0.);
    let mut ci = 0usize;
    let line_bounds = Bounds::new(origin, size(layout.width, line_height * subs.len().max(1) as f32));
    window.paint_layer(line_bounds, |window| {
        for run in &layout.runs {
            for glyph in &run.glyphs {
                while sub + 1 < subs.len() && glyph.index >= subs[sub + 1].0 {
                    sub += 1;
                    sub_x = layout.x_for_index(subs[sub].0);
                }
                while ci + 1 < colors.len() && glyph.index >= colors[ci].1 {
                    ci += 1;
                }
                let color = colors.get(ci).map(|c| c.2).unwrap_or_else(gpui::black);
                let at = point(origin.x + glyph.position.x - sub_x, origin.y + line_height * sub as f32 + baseline);
                let id = match subst {
                    Some(s) if s.font_id == run.font_id && glyph.id == s.from => s.to,
                    _ => glyph.id,
                };
                if glyph.is_emoji {
                    window.paint_emoji(at, run.font_id, id, layout.font_size).ok();
                } else {
                    window.paint_glyph(at, run.font_id, id, layout.font_size, color).ok();
                }
            }
        }
    });
}

/// One shaped source line. `start`/`end` are rune offsets; display offsets
/// are bytes of the tab-expanded display string, as gpui shapes them.
pub struct LineInfo {
    pub start: usize,
    pub end: usize,
    pub has_newline: bool,
    pub disp: SharedString,
    /// display byte -> rune offset, length disp.len()+1
    map: Vec<usize>,
    pub layout: WrappedLine,
    pub y: Pixels,
    pub subs: Vec<(usize, usize)>,
    pub colors: Vec<(usize, usize, Hsla)>,
}

impl LineInfo {
    pub fn to_disp(&self, src: usize) -> usize {
        self.map.partition_point(|&s| s < src).min(self.disp.len())
    }
    pub fn to_src(&self, d: usize) -> usize {
        self.map[d.min(self.map.len() - 1)]
    }
    pub fn height(&self, lh: Pixels) -> Pixels {
        lh * self.subs.len() as f32
    }
    /// Where each of the rows it wraps to starts, as rune offsets.
    pub fn row_starts(&self) -> Vec<usize> {
        self.subs.iter().map(|&(ds, _)| self.to_src(ds)).collect()
    }
    /// The row a rune is on: the last to start at or before it.
    pub fn row_of(&self, q: usize) -> usize {
        let d = self.to_disp(q);
        self.subs.iter().rposition(|&(ds, _)| ds <= d).unwrap_or(0)
    }
}

pub struct TextLayout {
    pub bounds: Bounds<Pixels>,
    pub text_origin: Point<Pixels>,
    pub line_height: Pixels,
    pub lines: Vec<LineInfo>,
    /// Where the rows start, in order: a screen of them above the top one,
    /// the top one, and those laid out below it -- the rows
    /// a scroll steps across, as acme's scrolling steps across the rows of
    /// its frame rather than the text's lines. The first is 0 when they
    /// reach back to the start of the text.
    pub rows: Vec<usize>,
    /// The last of `rows` is the text's last.
    pub rows_end: bool,
    pub text_len: usize,
    pub total_lines: usize,
    pub first_line: usize,
    pub scrollbar: Option<Bounds<Pixels>>,
    pub layout_box: Option<Bounds<Pixels>>,
}

impl TextLayout {
    /// Map a window position to a rune offset.
    pub fn offset_at(&self, pos: Point<Pixels>) -> usize {
        let Some(first) = self.lines.first() else { return self.text_len };
        let x = pos.x - self.text_origin.x;
        let y = pos.y - self.text_origin.y;
        if y < first.y {
            return first.start;
        }
        for line in &self.lines {
            if y < line.y + line.height(self.line_height) {
                let rel = point(x.max(px(0.)), y - line.y);
                let d = match line.layout.closest_index_for_position(rel, self.line_height) {
                    Ok(i) | Err(i) => i,
                };
                return line.to_src(d);
            }
        }
        let last = self.lines.last().unwrap();
        if last.has_newline && self.first_line + self.lines.len() < self.total_lines {
            last.end
        } else {
            self.text_len
        }
    }

    /// Where a rune is drawn, if it is on screen: the top-left of its
    /// glyph, in window coordinates. (The first line's rows above the top
    /// one are laid out too, and are not on screen.)
    pub fn point_of(&self, off: usize) -> Option<Point<Pixels>> {
        let lh = self.line_height;
        let line = self.lines.iter().find(|l| l.start <= off && (off < l.end || (off == l.end && !l.has_newline)))?;
        let d = line.to_disp(off);
        let p = line.layout.position_for_index(d, lh)?;
        if line.y + p.y + lh <= px(0.) {
            return None;
        }
        Some(point(self.text_origin.x + p.x, self.text_origin.y + line.y + p.y))
    }

    /// The row `n` rows below the one `origin` is on (above when `n` is
    /// negative), as far as the rows laid out reach: where the top of the
    /// view goes when scrolled by `n` rows.
    pub fn row_from(&self, origin: usize, n: i64) -> usize {
        if n == 0 || self.rows.is_empty() {
            return origin;
        }
        let at = self.rows.partition_point(|&s| s <= origin).saturating_sub(1);
        let to = (at as i64 + n).clamp(0, self.rows.len() as i64 - 1) as usize;
        self.rows[to]
    }

    pub fn lines_that_fit(&self) -> usize {
        ((self.bounds.size.height / self.line_height) as usize).max(1)
    }
}

/// Expand tabs and build the display-byte to rune map.
fn expand(src: &str, start: usize) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(src.len() + 8);
    let mut map = Vec::with_capacity(src.len() + 8);
    let mut col = 0;
    let mut r = start;
    for c in src.chars() {
        if c == '\t' {
            let n = TABSTOP - col % TABSTOP;
            for _ in 0..n {
                out.push(' ');
                map.push(r);
            }
            col += n;
        } else {
            out.push(c);
            for _ in 0..c.len_utf8() {
                map.push(r);
            }
            col += 1;
        }
        r += 1;
    }
    map.push(r);
    (out, map)
}

/// What the element needs from the app for one view.
pub struct Source {
    pub kind: Kind,
    /// A body scrolled by the pixel (the trackpad): how far its text is
    /// moved up from the top of the row its origin is on, negative when
    /// pulled down past the start.
    pub shift: f32,
    pub mono: bool,
    pub dirty: bool,
    /// Dirty, and the file (or directory) changed on disk since.
    pub stale: bool,
    /// A process is behind the window (a terminal's, a win's): neither
    /// clean nor dirty.
    pub live: bool,
    /// Work with nothing to show (a page loading, a tool thinking): how
    /// far (0..1) the handle is from its colour towards pale this
    /// instant.
    pub pulse: Option<f32>,
    /// This client no longer leads (its leases went elsewhere): the top
    /// row's square says so.
    pub fenced: bool,
    /// Notified: for the top row, some window in the session is (its
    /// square says so, and a click on it takes the oldest); for a window's
    /// tag, that window is (its handle says so).
    pub notified: bool,
    /// A window's tag with no body under it (acme collapsed it, or the
    /// column has no room): drawn as the edge of a sheet in a stack.
    pub sheet: bool,
    /// The keys go here: its caret is the blue one, and whether it shows
    /// just now (it blinks). None for any other view, whose caret is the
    /// plain one.
    pub key_caret: Option<bool>,
    pub text: Text,
    pub sel: (usize, usize),
    pub origin: usize,
    pub hl: Option<(usize, usize, HlKind)>,
    pub want_visible: bool,
    /// Bring this position on screen when it is not: acme's `textshow`,
    /// the position `quarters` quarters of the window down (one for new
    /// `+Errors` text, three for a program's output into a win).
    pub show_at: Option<(usize, usize)>,
}

pub struct TextElement {
    pub acme: Entity<Acme>,
    pub view: ViewId,
}

pub struct Prepaint {
    kind: Kind,
    fontspec: FontSpec,
    lines: Vec<LineInfo>,
    rows: Vec<usize>,
    rows_end: bool,
    /// The text from the top row to the end of the last row in view: what
    /// the scrollbar's thumb covers, as acme's covers the runes its frame
    /// shows.
    shown: (usize, usize),
    text_len: usize,
    total_lines: usize,
    first_line: usize,
    sel: (usize, usize),
    hl: Option<(usize, usize, HlKind)>,
    dirty: bool,
    stale: bool,
    live: bool,
    pulse: Option<f32>,
    fenced: bool,
    notified: bool,
    sheet: bool,
    key_caret: Option<bool>,
}

/// The row to have at the top so that the row `q` is on starts `room`
/// down the view (or as near as whole rows come without going over),
/// counting rows as the lines wrap: a long line of output is many rows,
/// and counting it as one leaves what was to be shown below the bottom.
/// The rest of `q`'s line is kept in the `height` when it can be. The
/// top row may be any row of a line, as acme's origin may be anywhere.
fn top_for(window: &Window, text: &apex_core::text::Text, fontspec: &FontSpec, wrap: Option<Pixels>, q: usize, room: Pixels, height: Pixels) -> usize {
    let lh = fontspec.line_height;
    let text_len = text.len();
    let line = |n: usize| text.line_range(n).map(|(s, e)| shape(window, &text.slice(s, e), s, e, e < text_len, fontspec, None, wrap, px(0.), None));
    let cl = text.line_of(q.min(text_len));
    let Some(li) = line(cl) else { return 0 };
    let r = li.row_of(q);
    // (a line taller than the view cannot be kept in it: `q` goes where
    // it was to go)
    let rest = lh * (li.subs.len() - r) as f32;
    let room = if rest <= height { room.min(height - rest) } else { room }.max(px(0.));
    let mut up = (room / lh).floor() as usize;
    let starts = li.row_starts();
    if up <= r {
        return starts[r - up];
    }
    up -= r;
    let mut n = cl;
    while n > 0 {
        n -= 1;
        let Some(li) = line(n) else { break };
        let starts = li.row_starts();
        if up <= starts.len() {
            return starts[starts.len() - up];
        }
        up -= starts.len();
    }
    0
}

fn shape(
    window: &Window,
    line_text: &str,
    start: usize,
    end: usize,
    has_newline: bool,
    fontspec: &FontSpec,
    hl: Option<(usize, usize, HlKind)>,
    wrap_width: Option<Pixels>,
    y: Pixels,
    dim_from: Option<usize>,
) -> LineInfo {
    let (disp, map) = expand(line_text, start);
    let disp: SharedString = disp.into();
    let black = rgb(crate::theme::theme().text);
    let white = rgb(crate::theme::theme().sweep_text);
    let dimmed = rgb(crate::theme::theme().text_dim);
    // a tag's name is set a weight heavier than the commands after it,
    // as a title is over a toolbar's
    let strong = Font { weight: gpui::FontWeight::MEDIUM, ..fontspec.font.clone() };
    let mut info = LineInfo {
        start,
        end,
        has_newline,
        disp: disp.clone(),
        map,
        layout: WrappedLine::default(),
        y,
        subs: Vec::new(),
        colors: Vec::new(),
    };
    // the line cut where its ink or face changes: the sweep, and the
    // secondary ink from `dim_from` on (a tag's commands after its name)
    let n = disp.len();
    let dim = dim_from.map(|q| if q <= start { 0 } else { info.to_disp(q.min(end)) });
    let sweep = match hl {
        Some((lo, hi, _)) if lo < end && hi > start && lo < hi => Some((info.to_disp(lo.max(start)), info.to_disp(hi.min(end)))),
        _ => None,
    };
    let mut cuts = vec![0, n];
    cuts.extend(dim);
    if let Some((a, b)) = sweep {
        cuts.extend([a, b]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::new();
    let mut colors = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        let swept = sweep.is_some_and(|(lo, hi)| a >= lo && b <= hi);
        let secondary = dim.is_some_and(|d| a >= d);
        let color = if swept {
            white
        } else if secondary {
            dimmed
        } else {
            black
        };
        let face = if dim.is_some_and(|d| d > 0 && a < d) { strong.clone() } else { fontspec.font.clone() };
        runs.push(TextRun { len: b - a, font: face, color, background_color: None, underline: None, strikethrough: None });
        colors.push((a, b, color));
    }
    if runs.is_empty() {
        runs.push(TextRun { len: 0, font: fontspec.font.clone(), color: black, background_color: None, underline: None, strikethrough: None });
        colors.push((0, 0, black));
    }
    info.colors = colors;
    let shaped = window
        .text_system()
        .shape_text(disp.clone(), fontspec.size, &runs, wrap_width, None)
        .ok()
        .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) })
        .unwrap_or_default();
    let mut subs = Vec::new();
    let mut s = 0;
    for b in shaped.wrap_boundaries() {
        let ix = shaped.runs()[b.run_ix].glyphs[b.glyph_ix].index;
        subs.push((s, ix));
        s = ix;
    }
    subs.push((s, disp.len()));
    info.layout = shaped;
    info.subs = subs;
    info
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepaint>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // acme's tiling decides every rectangle; the element fills the
        // box it is given
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        let kind = Kind::of(self.view);
        if kind != Kind::Top && kind != Kind::Top {
            (window.request_layout(style, [], cx), ())
        } else {
            let text: SharedString = self.acme.read(cx).view_text(self.view).into();
            let fontspec = font_for(false);
            let id = window.request_measured_layout(
                style,
                move |known: Size<Option<Pixels>>, avail: Size<AvailableSpace>, window, _cx| {
                    let width = known.width.or(match avail.width {
                        AvailableSpace::Definite(w) => Some(w),
                        _ => None,
                    });
                    let wrap = width.map(|w| (w - px(MARGIN) - px(4.)).max(px(10.)));
                    let run = TextRun {
                        len: text.len(),
                        font: fontspec.font.clone(),
                        color: gpui::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let n: usize = window
                        .text_system()
                        .shape_text(text.clone(), fontspec.size, &[run], wrap, None)
                        .map(|ls| ls.iter().map(|l| l.wrap_boundaries().len() + 1).sum())
                        .unwrap_or(1);
                    let extra = if kind == Kind::WinTag { px(1.) } else { px(0.) };
                    size(width.unwrap_or(px(100.)), fontspec.line_height * n.max(1) as f32 + extra)
                },
            );
            (id, ())
        }
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Prepaint> {
        let view = self.view;
        self.acme.update(cx, |acme, _cx| {
            let src = acme.source(view)?;
            let kind = src.kind;
            let fontspec = font_for(src.mono && kind == Kind::Body);
            let lh = fontspec.line_height;
            let wrap = Some((bounds.size.width - px(MARGIN) - px(4.)).max(px(10.)));
            let height = bounds.size.height;
            let text = &src.text;
            let text_len = text.len();
            let total = text.line_count();

            let mut lines = Vec::new();
            let mut rows = Vec::new();
            let (mut rows_end, mut shown) = (true, (0, text_len));
            let mut first = 0;
            if kind != Kind::Body {
                // what acme's wintaglines asks: how many lines the tag wraps to
                let mut y = px(0.);
                let mut n = 0;
                let mut wrapped = 0usize;
                // a window's name in the primary ink and the rest of its
                // tag in the secondary; a column's tag and the top row
                // are all commands
                let dim_from = match kind {
                    Kind::WinTag => text.to_string().find([' ', '\t']).map(|b| text.to_string()[..b].chars().count()),
                    _ => Some(0),
                };
                while let Some((s, e)) = text.line_range(n) {
                    let li = shape(window, &text.slice(s, e), s, e, e < text_len, &fontspec, src.hl, wrap, y, dim_from);
                    wrapped += li.subs.len().max(1);
                    y += li.height(lh);
                    lines.push(li);
                    n += 1;
                }
                let trailing = text_len > 0 && text.char_at(text_len - 1) == '\n';
                acme.tag_need.insert(view, (wrapped, trailing));
            } else if kind == Kind::Body {
                // acme's frame: from the origin, which may be anywhere in a
                // line -- the view starts at the row it is on, the line
                // wrapped from its own start whatever row is at the top --
                // and down the rows to the bottom
                let line = |n: usize, y: Pixels, hl| text.line_range(n).map(|(s, e)| shape(window, &text.slice(s, e), s, e, e < text_len, &fontspec, hl, wrap, y, None));
                let mut top = src.origin.min(text_len);
                // a view being brought somewhere is at its row, not between;
                // one whose selection is in view already (typing) stays
                // where it is scrolled, or it would jump there and back
                let mut shift = if src.show_at.is_some() { px(0.) } else { px(src.shift) };
                for pass in 0..2 {
                    lines.clear();
                    first = text.line_of(top).min(total.saturating_sub(1));
                    let mut y = -shift;
                    let mut n = first;
                    while y < height {
                        let Some(mut li) = line(n, y, src.hl) else { break };
                        if n == first {
                            // the rows of the first line above the top one
                            // are above the view
                            li.y -= lh * li.row_of(top) as f32;
                            y = li.y;
                        }
                        y += li.height(lh);
                        lines.push(li);
                        n += 1;
                    }
                    if pass == 1 {
                        break;
                    }
                    // what is to be shown: textshow's position, `quarters`
                    // quarters of the window down (one for new `+Errors`
                    // text, three for a program's output), or the selection
                    let want = match (src.show_at, src.want_visible) {
                        (Some((q, quarters)), _) => Some((q, Some(height * (quarters as f32 / 4.)))),
                        (None, true) => Some((src.sel.1, None)),
                        _ => None,
                    };
                    let Some((q, room)) = want else { break };
                    let q = q.min(text_len);
                    let cl = text.line_of(q);
                    // in view: its row wholly on screen (the top one while
                    // scrolled into it counts)
                    let row_y = cl.checked_sub(first).and_then(|i| lines.get(i)).map(|l| l.y + lh * l.row_of(q) as f32);
                    if row_y.is_some_and(|ry| ry + lh > px(0.) && ry + lh <= height) {
                        break;
                    }
                    let above_top = cl < first || row_y.is_some_and(|ry| ry < px(0.));
                    // the selection above: its row at the top; else half
                    // way down, as acme's textshow
                    let room = room.unwrap_or(if above_top { px(0.) } else { height / 2. });
                    top = top_for(window, text, &fontspec, wrap, q, room, height);
                    shift = px(0.);
                }
                if top != src.origin || src.want_visible || src.show_at.is_some() {
                    acme.set_origin(view, top);
                }
                // brought to a row: the scroll between rows is gone with it
                if shift == px(0.) && src.shift != 0. {
                    acme.forget_smooth(view);
                }
                // the rows: a screen of them above the top, for scrolling
                // back across, and those laid out from it down
                let mut up = Vec::new();
                if let Some(l) = lines.first() {
                    let starts = l.row_starts();
                    let r0 = l.row_of(top);
                    up.extend(starts[..r0].iter().rev());
                    let mut n = first;
                    while n > 0 && lh * up.len() as f32 <= height {
                        n -= 1;
                        let Some(li) = line(n, px(0.), None) else { break };
                        up.extend(li.row_starts().iter().rev());
                    }
                    up.reverse();
                    up.extend(starts[r0..].iter());
                    for li in &lines[1..] {
                        up.extend(li.row_starts());
                    }
                    rows_end = first + lines.len() >= total;
                    // what shows, from the top row to the end of the last
                    // row whose top is in view
                    let mut end = l.start;
                    for li in &lines {
                        let starts = li.row_starts();
                        for (i, _) in starts.iter().enumerate() {
                            if li.y + lh * i as f32 >= height {
                                break;
                            }
                            end = starts.get(i + 1).copied().unwrap_or(li.end + usize::from(li.has_newline));
                        }
                    }
                    shown = (starts[r0], end.max(starts[r0]));
                }
                rows = up;
            } else {
                unreachable!()
            }
            Some(Prepaint {
                kind,
                fontspec,
                lines,
                rows,
                rows_end,
                shown,
                text_len,
                total_lines: total,
                first_line: first,
                sel: src.sel,
                hl: src.hl,
                dirty: src.dirty,
                stale: src.stale,
                live: src.live,
                pulse: src.pulse,
                fenced: src.fenced,
                notified: src.notified,
                sheet: src.sheet,
                key_caret: src.key_caret,
            })
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Option<Prepaint>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(pp) = prepaint.take() else { return };
        let subst = if pp.fontspec.font.family == font_for(false).font.family { slashed_zero(window, cx) } else { None };
        let pal = palette(pp.kind);
        let lh = pp.fontspec.line_height;
        let origin = point(bounds.left() + px(MARGIN), bounds.top());

        // a notified window's header in a pale tint of the accent, as
        // Mail tints a flagged row: the whole bar says it wants the user.
        // Pale enough that a selection in the tag still shows on it (under
        // a deuteranopia simulation, 8 in CIELAB from the plain header and
        // 6 from the tag's selection on light, rsms's vivid blue being
        // near the selection's hue; 10 and 16 on dark, the pink)
        let header_bg = if pp.kind == Kind::WinTag && pp.notified {
            let th = crate::theme::theme();
            rgb(mix(th.tag_bg, th.accent, if crate::theme::is_dark() { 0.14 } else { 0.06 }))
        } else {
            pal.bg
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if pp.sheet {
                // the edge of a sheet in a stack of them, as Manifold
                // draws what lies beneath the top one: in from the
                // column's sides, its top corners rounded, a hairline
                // round it, on the column's own ground
                let th = crate::theme::theme();
                window.paint_quad(fill(bounds, rgb(th.body_bg)));
                let card = Bounds::new(point(bounds.left() + px(2.), bounds.top() + px(1.)), size(bounds.size.width - px(4.), bounds.size.height + px(1.)));
                let radii = gpui::Corners { top_left: px(7.), top_right: px(7.), bottom_left: px(0.), bottom_right: px(0.) };
                window.paint_quad(gpui::quad(card, radii, header_bg, gpui::Edges { top: px(1.), left: px(1.), right: px(1.), bottom: px(0.) }, rgb(th.tab_outline), gpui::BorderStyle::Solid));
            } else {
                window.paint_quad(fill(bounds, header_bg));
            }

            let mut scrollbar = None;
            let mut layout_box = None;
            let mut badge = false;
            match pp.kind {
                Kind::Body => {
                    // the lane is acme's, B1, B2 and B3 all as ever; what
                    // shows in it is a Mac scroller's thumb, slim and
                    // rounded, with no track
                    let sb = Bounds::new(bounds.origin, size(px(SCROLLWID), bounds.size.height));
                    // acme's: the runes shown, of all of them
                    let total = pp.text_len.max(1) as f32;
                    let (s0, s1) = if pp.text_len == 0 { (0., 1.) } else { (pp.shown.0 as f32 / total, (pp.shown.1 as f32 / total).min(1.)) };
                    paint_scroller(window, sb, s0, s1, pal.border);
                    scrollbar = Some(sb);
                }
                Kind::WinTag => {
                    let th = crate::theme::theme();
                    let b = Bounds::new(bounds.origin, size(px(SCROLLWID), lh));
                    let d = dot(&th, pp.stale, pp.dirty, pp.live, pp.pulse.is_some(), pp.notified);
                    // on a sheet's edge, in from the sheet's rounded corner
                    let dx = if pp.sheet { px(7.5) } else { px(6.) };
                    paint_dot(window, &d, point(b.left() + dx, b.top() + lh / 2.));
                    badge = d.badge;
                    window.paint_quad(fill(
                        // a hairline under the header, one device pixel
                        Bounds::new(point(bounds.left(), bounds.bottom() - px(1.) / window.scale_factor()), size(bounds.size.width, px(1.) / window.scale_factor())),
                        pal.border,
                    ));
                    layout_box = Some(b);
                }
                Kind::ColTag => {
                    let b = Bounds::new(bounds.origin, size(px(SCROLLWID), lh));
                    paint_grip(window, b, rgb(crate::theme::theme().text_dim));
                    layout_box = Some(b);
                }
                Kind::Top => {
                    // the session's own square, a grip like a column's; red
                    // when this client has lost its leases and only watches
                    // (a click still takes the oldest notification)
                    let b = Bounds::new(bounds.origin, size(px(SCROLLWID), lh));
                    let th = crate::theme::theme();
                    if pp.fenced {
                        let r = Bounds::new(point(b.left() + px(1.), b.top() + (lh - px(10.)) / 2.), size(px(10.), px(10.)));
                        window.paint_quad(fill(r, rgb(th.fenced)).corner_radii(px(3.)));
                    } else {
                        paint_grip(window, b, rgb(th.text_dim));
                    }
                    layout_box = Some(b);
                }
            }

            let (q0, q1) = pp.sel;
            let right = bounds.right();
            for line in &pp.lines {
                let ly = origin.y + line.y;
                let x = |d: usize| line.layout.unwrapped_layout.x_for_index(d);
                let ranges: [(usize, usize, Hsla); 2] = [
                    (q0, q1, pal.sel),
                    match pp.hl {
                        Some((lo, hi, HlKind::Exec)) => (lo, hi, rgb(crate::theme::theme().exec_hl)),
                        Some((lo, hi, HlKind::Look)) => (lo, hi, rgb(crate::theme::theme().look_hl)),
                        None => (0, 0, pal.sel),
                    },
                ];
                for (a, b, color) in ranges {
                    if a >= b {
                        continue;
                    }
                    let lo = a.max(line.start);
                    let hi = b.min(line.end + usize::from(line.has_newline));
                    if lo > line.end || hi < line.start || (lo >= hi && !(line.has_newline && a <= line.end && b > line.end)) {
                        continue;
                    }
                    let incl_nl = line.has_newline && b > line.end && a <= line.end;
                    let dlo = line.to_disp(lo.min(line.end));
                    let dhi = line.to_disp(hi.min(line.end));
                    for (i, &(ds, de)) in line.subs.iter().enumerate() {
                        let last = i + 1 == line.subs.len();
                        let s = dlo.max(ds);
                        let e = dhi.min(de);
                        let mut x0 = None;
                        let mut x1 = None;
                        if s < e {
                            x0 = Some(x(s) - x(ds));
                            x1 = Some(x(e) - x(ds));
                        }
                        if last && incl_nl && dlo <= de {
                            x0 = Some(x0.unwrap_or(x(dlo.max(ds)) - x(ds)));
                            x1 = Some(right - origin.x);
                        }
                        if let (Some(x0), Some(x1)) = (x0, x1) {
                            let sy = ly + lh * i as f32;
                            window.paint_quad(fill(Bounds::from_corners(point(origin.x + x0, sy), point(origin.x + x1, sy + lh)), color));
                        }
                    }
                }

                paint_glyphs(window, &line.layout.unwrapped_layout, &line.subs, point(origin.x, ly), lh, &line.colors, subst);

                // the tick, as a Mac text view's caret: a plain line, a
                // little in from the row's top and bottom. A header's is
                // left out where it only sits at its start, as every
                // one's does until it is typed in or clicked in
                // The keys' view has the blue one, a little wider, which
                // blinks as iOS's does -- and which, being the one, says
                // where typing goes; a header's shows even at its start
                // then, since that is where a key would land
                let header = pp.kind != Kind::Body;
                let keys = pp.key_caret.is_some();
                let shows = pp.key_caret.unwrap_or(true);
                if q0 == q1 && q0 >= line.start && q0 <= line.end && !(header && q0 == 0 && !keys) && shows {
                    let d = line.to_disp(q0);
                    let mut sub = line.subs.len() - 1;
                    for (i, &(ds, de)) in line.subs.iter().enumerate() {
                        if d >= ds && d < de {
                            sub = i;
                            break;
                        }
                    }
                    let (ds, _) = line.subs[sub];
                    let cx_ = origin.x + x(d) - x(ds);
                    let ty = ly + lh * sub as f32;
                    let th = crate::theme::theme();
                    if keys {
                        window.paint_quad(fill(Bounds::new(point(cx_ - px(0.5), ty + px(1.)), size(px(2.), lh - px(2.))), rgb(th.accent)).corner_radii(px(1.)));
                    } else {
                        window.paint_quad(fill(Bounds::new(point(cx_, ty + px(2.)), size(px(1.5), lh - px(4.))), rgb(th.text)).corner_radii(px(0.75)));
                    }
                }
            }

            // notified: pjw's face at the header's far end, in the accent,
            // over the tag's text on a patch of the header -- the face the
            // session's row wears in the sidebar, here on its window
            if badge {
                let th = crate::theme::theme();
                const PJW: &[u8] = include_bytes!("../assets/pjw.svg");
                let fh = (lh - px(2.)).min(px(15.));
                let fw = fh * (201. / 259.);
                let at = point(bounds.right() - px(8.) - fw, bounds.top() + (lh - fh) / 2.);
                // the patch stops short of the right edge, a sheet's line
                window.paint_quad(fill(Bounds::new(point(at.x - px(5.), bounds.top() + px(2.)), size(fw + px(9.), lh - px(2.))), header_bg));
                let _ = window.paint_svg(Bounds::new(at, size(fw, fh)), "pjw.svg".into(), Some(PJW), gpui::TransformationMatrix::unit(), rgb(th.accent), cx);
            }
            let layout = TextLayout {
                bounds,
                text_origin: origin,
                line_height: lh,
                lines: pp.lines,
                rows: pp.rows,
                rows_end: pp.rows_end,
                text_len: pp.text_len,
                total_lines: pp.total_lines,
                first_line: pp.first_line,
                scrollbar,
                layout_box,
            };
            let view = self.view;
            self.acme.update(cx, |acme, _| {
                acme.layouts.insert(view, layout);
            });
        });
    }
}

#[cfg(test)]
mod row_tests {
    use super::TextLayout;
    use gpui::{point, px, Bounds};

    fn laid(rows: Vec<usize>) -> TextLayout {
        TextLayout {
            bounds: Bounds::default(),
            text_origin: point(px(0.), px(0.)),
            line_height: px(16.),
            lines: Vec::new(),
            rows,
            rows_end: true,
            text_len: 1000,
            total_lines: 1,
            first_line: 0,
            scrollbar: None,
            layout_box: None,
        }
    }

    #[test]
    fn scrolling_steps_across_rows_from_wherever_the_origin_is() {
        // one line wrapped every 80 runes
        let l = laid((0..10).map(|r| r * 80).collect());
        assert_eq!(l.row_from(0, 3), 240, "three rows into the line");
        assert_eq!(l.row_from(250, -1), 160, "from a row's middle, a row back");
        assert_eq!(l.row_from(250, 0), 250, "no scroll, no move");
        assert_eq!(l.row_from(160, 100), 720, "the last row can come to the top");
        assert_eq!(l.row_from(160, -100), 0);
    }
}

#[cfg(test)]
mod dot_tests {
    use super::dot;

    #[test]
    fn a_handle_is_a_circle_with_its_marks() {
        let th = crate::theme::theme();
        // clean: a hollow circle; dirty and stale: filled
        let clean = dot(&th, false, false, false, false, false);
        assert_eq!((clean.fill, clean.ring, clean.core, clean.spin, clean.badge), (None, Some(th.text_dim), None, None, false));
        assert_eq!(dot(&th, false, true, false, false, false).fill, Some(th.dirty));
        assert_eq!(dot(&th, true, true, false, false, false).fill, Some(th.stale));
        // live: the accent round it, and in its middle when clean
        let live = dot(&th, false, false, true, false, false);
        assert_eq!((live.ring, live.core), (Some(th.accent), Some(th.accent)));
        let dirty_live = dot(&th, false, true, true, false, false);
        assert_eq!((dirty_live.fill, dirty_live.ring, dirty_live.core), (Some(th.dirty), Some(th.accent), None));
        // all of it at once: each mark still there
        let all = dot(&th, false, true, true, true, true);
        assert!(all.fill.is_some() && all.ring.is_some() && all.spin.is_some() && all.badge, "{all:?}");
    }
}

#[cfg(test)]
mod ligature_tests {
    use super::font_for;

    #[test]
    fn text_fonts_join_no_letters_into_one_glyph() {
        // with liga, clig and calt off CoreText gives each of ff, fi, fl,
        // ffi and ffl a glyph a letter in Lucida Grande (and Menlo), so a
        // click or the cursor can land between any two of them
        for mono in [false, true] {
            let f = font_for(mono).font;
            let off: Vec<(String, u32)> = f.features.tag_value_list().to_vec();
            for tag in ["liga", "clig", "calt"] {
                assert!(off.contains(&(tag.to_string(), 0)), "{tag} off in {} ({off:?})", f.family);
            }
        }
    }
}
