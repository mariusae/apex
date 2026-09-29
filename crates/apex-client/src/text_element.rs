//! A gpui element that paints one acme text (a tag or a body) straight from
//! the core's state: only the visible lines are shaped, and the geometry
//! is recorded on the app so mouse events map to rune offsets.

use std::cell::RefCell;

use gpui::{
    fill, font, point, px, relative, size, App, AvailableSpace, Bounds, ContentMask, Element, ElementId,
    Entity, Font, FontId, GlobalElementId, GlyphId, Hsla, InspectorElementId, IntoElement, LayoutId,
    LineLayout, Pixels, Point, Rgba, SharedString, Size, Style, TextRun, Window, WrappedLine,
};

use apex_core::{Text, ViewId};

use crate::app::{Acme, HlKind, Kind};

pub const SCROLLWID: f32 = 12.;
pub const MARGIN: f32 = 16.; // Scrollwid + Scrollgap
/// A body's text (a text's, a terminal's) starts this far in. Its
/// scroller keeps no lane of its own: it lays over the body's right edge,
/// as macOS's overlay scrollers do (`LANE_HIT`, `SCROLLWID`).
pub const BODY_MARGIN: f32 = 8.;
/// How wide a body's scroller lane, at its right edge, is to the pointer
/// while it is shut. Once the pointer is in it, the lane opens to
/// `SCROLLWID`, its gutter drawn over what is there (which does not
/// move), and B1 B2 B3 in it are acme's scrollbar.
pub const LANE_HIT: f32 = 6.;

/// The body's scroller, at its right edge over its text: shut, the thumb
/// alone while the text moves (`shows`); open (the pointer in the lane),
/// a gutter the lane's width, drawn over what is there, and the thumb in
/// it.
pub fn paint_overlay_scroller(window: &mut Window, bounds: Bounds<Pixels>, s0: f32, s1: f32, shows: f32, open: bool) {
    let th = crate::theme::theme();
    // a pixel in from the card's edge, where the key window's ring lies
    // over it: centred in what shows of the gutter
    let lane = Bounds::new(point(bounds.right() - px(SCROLLWID + 1.), bounds.top()), size(px(SCROLLWID), bounds.size.height));
    if open {
        window.paint_quad(fill(lane, rgb(mix(th.body_bg, th.body_border, 0.45))));
        paint_scroller(window, lane, s0, s1, rgb(th.text_dim).opacity(0.75));
    } else if shows > 0. {
        paint_scroller(window, lane, s0, s1, rgb(th.text_dim).opacity(0.55 * shows));
    }
}
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
}

/// The ground the windows stand on: what shows between them (they are
/// cards on it), under the column tags and the top row. A step below the
/// tags in light, below the paper in dark.
/// A tag's line, a little taller than a body's: the text's ink centred
/// in it (`ink_lift`), as much air over the ascenders as under the
/// descenders, and enough of it that a folded window's card (the line
/// less the card's inset top and bottom) still has some round its text.
/// The tiling's font height (a tag's, a column tag's, the top row's) is
/// this.
pub const TAG_PAD: f32 = 4.;

pub fn tag_line_height() -> Pixels {
    font_for(false).line_height + px(TAG_PAD)
}

/// The radius of a card's corners (a window on the ground).
pub const CARD_RADIUS: f32 = 7.;


/// apex's verbs in a window's tag (the words before its `|`), drawn as
/// icons: each word laid out as one em space, an icon painted over it.
/// Only a synonym, drawn: the text is the word, and a click, a sweep, B2
/// and B3 take it as the word.
pub const VERB_ICONS: &[(&str, &str)] = &[
    ("Del", r#"<path d="M7 7l10 10M17 7L7 17"/>"#),
    ("Snarf", r#"<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>"#),
    ("Undo", r#"<path d="M9 14L4 9l5-5M4 9h11a5 5 0 0 1 0 10h-3"/>"#),
    ("Redo", r#"<path d="M15 14l5-5-5-5M20 9H9a5 5 0 0 0 0 10h3"/>"#),
    ("Put", r#"<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>"#),
    ("Get", r#"<path d="M20 11a8 8 0 0 0-14.9-4M4 4v4h4M4 13a8 8 0 0 0 14.9 4M20 20v-4h-4"/>"#),
    ("Send", r#"<path d="M21 3L10 14M21 3l-7 18-4-7-7-4z"/>"#),
    ("Back", r#"<path d="M15 6l-6 6 6 6"/>"#),
    ("Fwd", r#"<path d="M9 6l6 6-6 6"/>"#),
];

/// The icon of verb `i` as an SVG document, stroked, for `paint_svg`
/// (which draws its shape in the ink it is given).
fn verb_svg(i: usize) -> &'static [u8] {
    thread_local! {
        static SVGS: Vec<&'static [u8]> = VERB_ICONS
            .iter()
            .map(|(_, body)| &*Box::leak(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#).into_bytes().into_boxed_slice()))
            .collect();
    }
    SVGS.with(|v| v[i])
}

/// The em space a verb's icon stands on.
const ICON_CELL: char = '\u{2003}';

pub fn ground(t: &crate::theme::Theme) -> u32 {
    if crate::theme::is_dark() {
        mix(t.body_bg, 0x000000, 0.28)
    } else {
        mix(t.tag_bg, t.text, 0.07)
    }
}

pub fn palette(kind: Kind) -> Palette {
    let t = crate::theme::theme();
    match kind {
        Kind::Body => Palette { bg: rgb(t.body_bg), sel: rgb(t.body_sel) },
        _ => Palette { bg: rgb(t.tag_bg), sel: rgb(t.tag_sel) },
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
    /// Grown to the whole column, others hidden behind it: square, not
    /// round.
    pub square: bool,
}

impl Dot {
    pub fn squared(self, square: bool) -> Dot {
        Dot { square, ..self }
    }
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
        square: false,
    }
}

/// The circle's radius, and the ring's round a filled one, and the arc's.
const DOT_R: f32 = 3.75;
const RING_R: f32 = 5.;
const SPIN_R: f32 = 5.25;

pub fn paint_dot(window: &mut Window, d: &Dot, c: Point<Pixels>) {
    let circle = |r: f32| Bounds::new(point(c.x - px(r), c.y - px(r)), size(px(2. * r), px(2. * r)));
    // square: the same size, its corners barely rounded
    let round = |r: f32| if d.square { px(r.min(1.25)) } else { px(r) };
    if let Some(f) = d.fill {
        window.paint_quad(fill(circle(DOT_R), rgb(f)).corner_radii(round(DOT_R)));
    }
    if let Some(ring) = d.ring {
        let r = if d.fill.is_some() { RING_R } else { DOT_R };
        window.paint_quad(gpui::quad(circle(r), round(r), gpui::transparent_black(), px(1.25), rgb(ring), gpui::BorderStyle::Solid));
    }
    if let Some(core) = d.core {
        window.paint_quad(fill(circle(1.75), rgb(core)).corner_radii(round(1.75)));
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
    // in the middle of the lane
    let thumb = Bounds::new(point(lane.left() + (lane.size.width - px(5.)) / 2., top), size(px(5.), len));
    window.paint_quad(fill(thumb, ink).corner_radii(px(2.5)));
}

/// A drag grip, two columns of three dots: a column's box, and the
/// session's.
pub fn paint_grip(window: &mut Window, b: Bounds<Pixels>, ink: Hsla) {
    paint_grip_as(window, b, ink, false);
}

/// The grip, and `hiding` (the column given the whole row, the others
/// hidden behind it, as a window's square handle says of its column):
/// its dots square, and a square round them.
pub fn paint_grip_as(window: &mut Window, b: Bounds<Pixels>, ink: Hsla, hiding: bool) {
    let cx = b.left() + b.size.width / 2.;
    let cy = b.top() + b.size.height / 2.;
    for (dx, dy) in [(-2., -4.), (2., -4.), (-2., 0.), (2., 0.), (-2., 4.), (2., 4.)] {
        let r = if hiding { 1. } else { 0.9 };
        window.paint_quad(fill(Bounds::new(point(cx + px(dx - r), cy + px(dy - r)), size(px(2. * r), px(2. * r))), ink).corner_radii(px(if hiding { 0. } else { r })));
    }
    if hiding {
        let frame = Bounds::new(point(cx - px(5.), cy - px(7.)), size(px(10.), px(14.)));
        window.paint_quad(gpui::quad(frame, px(1.5), gpui::transparent_black(), px(1.), ink, gpui::BorderStyle::Solid));
    }
}

pub struct FontSpec {
    pub font: Font,
    pub size: Pixels,
    pub line_height: Pixels,
}

/// The text faces, as View ▸ Font has them (`fonts::text` for text
/// windows and tags, `fonts::mono` for mono windows and terminals).
pub fn font_for(mono: bool) -> FontSpec {
    let spec = if mono { crate::fonts::mono() } else { crate::fonts::text() };
    let f = Font { weight: spec.weight, ..unjoined(with_symbols(font(spec.family)), spec.features) };
    FontSpec { font: f, size: spec.size, line_height: spec.line_height }
}

/// No ligatures from the font: every character its own glyph. Where a
/// character is -- for a click, the cursor, a selection's edge, a sweep --
/// is known only from where its glyph starts (gpui's `x_for_index` and
/// `closest_index_for_x`), and the letters a ligature joins have no glyph
/// of their own, so there was no putting the cursor between the f's of an
/// `ff`. Lucida Grande joins ff, fi, fl, ffi and ffl by default (`liga`);
/// `clig` and `calt` are the other ways a font joins letters. All three
/// are named: gpui's `disable_ligatures` turns off `calt` alone.
///
/// Then the set's own (`more`), which may turn one of those back on where
/// it substitutes glyph for glyph and joins nothing (Monaspace's `calt`).
fn unjoined(f: Font, more: &[(&str, u32)]) -> Font {
    let mut features: Vec<(String, u32)> = UNJOINED.iter().map(|t| (t.to_string(), 0)).collect();
    // and a zero with a slash through it, where the face has one (SF
    // Pro and SF Mono do: `zero`), so it is not an O
    features.push(("zero".to_string(), 1));
    for (tag, v) in more {
        features.retain(|(t, _)| t != tag);
        features.push((tag.to_string(), *v));
    }
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


thread_local! {
    /// Each font's zero and slashed zero, by the font gpui draws with
    /// (none for most): looked for once per font.
    static ZEROS: RefCell<std::collections::HashMap<FontId, Option<(GlyphId, GlyphId)>>> = RefCell::new(std::collections::HashMap::new());
    static LIFT: RefCell<std::collections::HashMap<(String, u32), (f32, f32)>> = RefCell::new(std::collections::HashMap::new());
}

/// How far a line's glyphs are lifted so their ink sits in the middle of
/// the line. gpui centres a face's ascent and descent in the line, and a
/// face keeps more room over its ascenders (for accents) than under its
/// descenders, so text centred so sits low: System at 14 points, 1.4
/// pixels. Measured once per face and size, from the tallest ascender
/// and deepest descender among a few letters; to the device's pixel.
fn ink_lift(window: &Window, fs: &FontSpec) -> Pixels {
    ink(window, fs).0
}

/// `ink_lift`, and how tall the ink is, from the tallest ascender to the
/// deepest descender: what a caret spans (a little more), not the line.
fn ink(window: &Window, fs: &FontSpec) -> (Pixels, Pixels) {
    let key = (fs.font.family.to_string(), f32::from(fs.size).to_bits());
    let (lift, tall) = LIFT.with(|m| m.borrow().get(&key).copied()).unwrap_or_else(|| {
        let ts = window.text_system();
        let id = ts.resolve_font(&fs.font);
        let (asc, desc) = (f32::from(ts.ascent(id, fs.size)), f32::from(ts.descent(id, fs.size)).abs());
        // typographic bounds are y-up from the baseline
        let bounds = |cs: &str| cs.chars().filter_map(|c| ts.typographic_bounds(id, fs.size, c).ok()).collect::<Vec<_>>();
        let top = bounds("bdfhklI").iter().map(|b| f32::from(b.origin.y + b.size.height)).fold(0., f32::max);
        let bottom = bounds("gjpqy").iter().map(|b| -f32::from(b.origin.y)).fold(0., f32::max);
        let lift = if top > 0. && bottom > 0. { (((asc - top) - (desc - bottom)) / 2.).clamp(0., f32::from(fs.line_height) / 4.) } else { 0. };
        let tall = if top > 0. && bottom > 0. { top + bottom } else { asc + desc };
        LIFT.with(|m| m.borrow_mut().insert(key, (lift, tall)));
        (lift, tall)
    });
    let scale = window.scale_factor();
    let snap = |v: f32| px((v * scale).round() / scale);
    (snap(lift), snap(tall))
}

/// Font `id`'s zero, and the slashed zero to draw in its place, where the
/// face has one by name and no feature for it (Lucida Grande's
/// `zeroslash`): asked of the font gpui draws the run with, whatever it
/// is -- not of one font resolved once, which a font resolved anew (its
/// fallbacks, a setting come from the server) would no longer match.
fn zero_for(window: &Window, id: FontId) -> Option<(GlyphId, GlyphId)> {
    if let Some(z) = ZEROS.with(|m| m.borrow().get(&id).copied()) {
        return z;
    }
    let z = (|| {
        let font = window.text_system().get_font_for_id(id)?;
        // the system's faces have their slashed zero as a feature
        // (`zero`, set in `unjoined`), and CoreText will not open them by
        // name
        if font.family.starts_with('.') {
            return None;
        }
        let ct = core_text::font::new_from_name(&font.family, 12.).ok()?;
        let slashed = ct.get_glyph_with_name("zeroslash");
        if slashed == 0 {
            return None;
        }
        let mut zero: u16 = 0;
        let ch: u16 = '0' as u16;
        // SAFETY: one character in, one glyph out, both on the stack
        let found = unsafe { ct.get_glyphs_for_characters(&ch, &mut zero, 1) };
        if !found || zero == 0 {
            return None;
        }
        // SAFETY: gpui::GlyphId is `#[repr(C)] struct GlyphId(u32)` with a
        // crate-private field and no public constructor.
        let glyph = |g: u16| unsafe { std::mem::transmute::<u32, GlyphId>(g as u32) };
        Some((glyph(zero), glyph(slashed)))
    })();
    ZEROS.with(|m| m.borrow_mut().insert(id, z));
    z
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
    lift: Pixels,
) {
    let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
    let baseline = padding_top + layout.ascent - lift;
    let mut sub = 0usize;
    let mut sub_x = px(0.);
    let mut ci = 0usize;
    let line_bounds = Bounds::new(origin, size(layout.width, line_height * subs.len().max(1) as f32));
    window.paint_layer(line_bounds, |window| {
        for run in &layout.runs {
            // the run's face's slashed zero, where it has one to swap in
            let zero = zero_for(window, run.font_id);
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
                let id = match zero {
                    Some((from, to)) if glyph.id == from => to,
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
    /// A window's tag: where its `|` is, drawn as a hairline, and where its
    /// verbs' icons stand (`VERB_ICONS`) -- display offsets; the text is
    /// the tag's as ever.
    pub bar: Option<usize>,
    pub icons: Vec<(usize, usize)>,
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
    /// glyph, in window coordinates -- or, at a line's end (before its
    /// newline, where a caret sits after the last word), just past its
    /// last. (The first line's rows above the top one are laid out too,
    /// and are not on screen.)
    pub fn point_of(&self, off: usize) -> Option<Point<Pixels>> {
        let lh = self.line_height;
        let line = self.lines.iter().find(|l| l.start <= off && off <= l.end)?;
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
/// `expand`, and each verb in `icons` (its runes `ws..we`, which icon)
/// laid out as one em space: every byte of it the word's start, the byte
/// after it the word's end, so an offset in the word is at the icon's
/// start or end. Where each icon stands, in display bytes.
fn expand_icons(src: &str, start: usize, icons: &[(usize, usize, usize)]) -> (String, Vec<usize>, Vec<(usize, usize)>) {
    if icons.is_empty() {
        let (d, m) = expand(src, start);
        return (d, m, Vec::new());
    }
    let (mut out, mut map, mut at) = (String::new(), Vec::new(), Vec::new());
    let chars: Vec<char> = src.chars().collect();
    let mut k = 0;
    let mut col = 0;
    while k < chars.len() {
        let r = start + k;
        if let Some(&(_, we, i)) = icons.iter().find(|&&(ws, we, _)| ws == r && we <= start + chars.len()) {
            at.push((out.len(), i));
            out.push(ICON_CELL);
            for _ in 0..ICON_CELL.len_utf8() {
                map.push(r);
            }
            col += 1;
            k = we - start;
            continue;
        }
        let c = chars[k];
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
        k += 1;
    }
    map.push(start + chars.len());
    (out, map, at)
}

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
    /// The pointer is on it: a tag's commands at their full secondary ink,
    /// faint otherwise.
    pub hovered: bool,
    /// Its outer corners rounded, as the top (a tag) or the foot (a body)
    /// of a card: (top, bottom).
    pub round: (bool, bool),
    /// A body's scroller: how much of its thumb shows, and whether that
    /// is changing (drawn again soon).
    pub scroller: (f32, bool),
    /// A window grown to the whole column, others hidden behind it: its
    /// handle square.
    pub hiding: bool,
    /// The keys go here: its caret is the blue one, and whether it shows
    /// just now (it blinks). None for any other view, whose caret is the
    /// plain one.
    pub key_caret: Option<bool>,
    pub text: Text,
    pub sel: (usize, usize),
    pub origin: usize,
    pub hl: Option<(usize, usize, HlKind)>,
    /// What a click would take here with the modifier held (⌘: B3's,
    /// ⌥: B2's), on a pill.
    pub hint: Option<(usize, usize, HlKind)>,
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
    hint: Option<(usize, usize, HlKind)>,
    dirty: bool,
    stale: bool,
    live: bool,
    pulse: Option<f32>,
    fenced: bool,
    notified: bool,
    round: (bool, bool),
    scroller: (f32, bool),
    /// The pointer in the scroller's lane: it is open.
    lane: bool,
    hiding: bool,
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
    let line = |n: usize| text.line_range(n).map(|(s, e)| shape(window, &text.slice(s, e), s, e, e < text_len, fontspec, None, wrap, px(0.), None, &[]));
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

/// How a tag's text is inked: its name's directory (up to `dir_end`) in
/// the secondary ink, its last part (up to `name_end`) in the primary and
/// a weight heavier, and the commands after it in `rest` -- faint until
/// the pointer is on the tag. Offsets are the text's, in characters.
#[derive(Clone, Copy)]
pub struct Tint {
    pub dir_end: usize,
    pub name_end: usize,
    pub rest: Hsla,
    /// A window's tag: its first `|` after the name, which parts apex's
    /// words from the user's, and the ink of the user's words after it.
    pub bar: Option<usize>,
    pub yours: Hsla,
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
    tint: Option<Tint>,
    icons: &[(usize, usize, usize)],
) -> LineInfo {
    let (disp, map, icon_at) = expand_icons(line_text, start, icons);
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
        bar: None,
        icons: Vec::new(),
    };
    // the line cut where its ink or face changes: the sweep, and a tag's
    // parts -- its name's directory, its last part, the commands after
    let n = disp.len();
    let at = |q: usize| if q <= start { 0 } else { info.to_disp(q.min(end)) };
    let dir = tint.map(|t| at(t.dir_end));
    let dim = tint.map(|t| at(t.name_end));
    // a window's tag: its `|`, when it is on this line, a glyph left clear
    // for the hairline drawn in its place
    let bar = tint.and_then(|t| t.bar).filter(|&q| q >= start && q < end).map(|q| (info.to_disp(q), info.to_disp(q + 1)));
    // the space after its name set in the mono face, whose space is
    // wider, for air between the path and the first word (a real
    // advance, so clicks and the caret agree with what is drawn)
    let name_at = tint.filter(|t| start == 0 && t.name_end > 0).map(|t| at(t.name_end));
    let gap = name_at.filter(|&p| disp.as_bytes().get(p) == Some(&b' ')).map(|p| (p, p + 1));
    let wide = font_for(true).font;
    let sweep = match hl {
        Some((lo, hi, _)) if lo < end && hi > start && lo < hi => Some((info.to_disp(lo.max(start)), info.to_disp(hi.min(end)))),
        _ => None,
    };
    let mut cuts = vec![0, n];
    cuts.extend(dim);
    cuts.extend(dir);
    if let Some((a, b)) = bar {
        cuts.extend([a, b]);
    }
    if let Some((a, b)) = gap {
        cuts.extend([a, b]);
    }
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
        let command = dim.is_some_and(|d| a >= d);
        let folder = dir.is_some_and(|d| a < d) && !command;
        let is_bar = bar.is_some_and(|(p, q)| a >= p && b <= q);
        let yours = bar.is_some_and(|(_, q)| a >= q) || tint.and_then(|t| t.bar).is_some_and(|q| q < start);
        let color = if is_bar {
            gpui::transparent_black()
        } else if swept {
            white
        } else if yours && command {
            tint.map(|t| t.yours).unwrap_or(dimmed)
        } else if command {
            tint.map(|t| t.rest).unwrap_or(dimmed)
        } else if folder {
            dimmed
        } else {
            black
        };
        let face = if gap.is_some_and(|(p, q)| a >= p && b <= q) {
            wide.clone()
        } else if dim.is_some_and(|d| d > 0 && a < d) && !folder {
            strong.clone()
        } else {
            fontspec.font.clone()
        };
        runs.push(TextRun { len: b - a, font: face, color, background_color: None, underline: None, strikethrough: None });
        colors.push((a, b, color));
    }
    if runs.is_empty() {
        runs.push(TextRun { len: 0, font: fontspec.font.clone(), color: black, background_color: None, underline: None, strikethrough: None });
        colors.push((0, 0, black));
    }
    info.colors = colors;
    info.bar = bar.map(|(p, _)| p);
    info.icons = icon_at;
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
            let mut fontspec = font_for(false);
            fontspec.line_height = tag_line_height();
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
            let mut fontspec = font_for(src.mono && kind == Kind::Body);
            // a tag's line a little taller than a body's
            if kind != Kind::Body {
                fontspec.line_height = tag_line_height();
            }
            let lh = fontspec.line_height;
            let margin = if kind == Kind::Body { px(BODY_MARGIN) } else { px(MARGIN) };
            let wrap = Some((bounds.size.width - margin - px(4.)).max(px(10.)));
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
                // a window's name: its directory in the secondary ink, its
                // last part in the primary; the commands after it (and a
                // column's tag and the top row, all commands) faint until
                // the pointer is on the tag
                let th = crate::theme::theme();
                let under = if kind == Kind::WinTag { th.tag_bg } else { ground(&th) };
                let rest = if src.hovered { rgb(mix(th.text_dim, under, 0.2)) } else { rgb(mix(th.text_dim, under, 0.5)) };
                // the user's words, after the `|`: a step above apex's
                let yours = rgb(th.text_dim);
                let mut icons: Vec<(usize, usize, usize)> = Vec::new();
                let tint = match kind {
                    Kind::WinTag => {
                        let whole = text.to_string();
                        let name = &whole[..whole.find([' ', '\t']).unwrap_or(whole.len())];
                        let name_end = name.chars().count();
                        let trimmed = name.trim_end_matches('/');
                        let dir_end = trimmed.rfind('/').map(|i| trimmed[..=i].chars().count()).unwrap_or(0);
                        let bar = whole[name.len()..].find('|').map(|i| whole[..name.len() + i].chars().count());
                        // apex's verbs, between the name and the `|` (or the
                        // end): drawn as icons
                        let till = bar.unwrap_or(whole.chars().count());
                        let chars: Vec<char> = whole.chars().collect();
                        let mut q = name_end;
                        while q < till {
                            while q < till && chars[q].is_whitespace() {
                                q += 1;
                            }
                            let ws = q;
                            while q < till && !chars[q].is_whitespace() {
                                q += 1;
                            }
                            let word: String = chars[ws..q].iter().collect();
                            if let Some(i) = VERB_ICONS.iter().position(|(v, _)| *v == word) {
                                icons.push((ws, q, i));
                            }
                        }
                        Some(Tint { dir_end, name_end, rest, bar, yours })
                    }
                    _ => Some(Tint { dir_end: 0, name_end: 0, rest, bar: None, yours }),
                };
                while let Some((s, e)) = text.line_range(n) {
                    let li = shape(window, &text.slice(s, e), s, e, e < text_len, &fontspec, src.hl.or(src.hint), wrap, y, tint, &icons);
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
                let line = |n: usize, y: Pixels, hl| text.line_range(n).map(|(s, e)| shape(window, &text.slice(s, e), s, e, e < text_len, &fontspec, hl, wrap, y, None, &[]));
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
                        let Some(mut li) = line(n, y, src.hl.or(src.hint)) else { break };
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
                    // (a selection by its start: a line plumbed or looked
                    // to ends at the next line's start, and shown by its end
                    // from below it came to the top as the line under it,
                    // the line itself just out of sight above)
                    let want = match (src.show_at, src.want_visible) {
                        (Some((q, quarters)), _) => Some((q, Some(height * (quarters as f32 / 4.)))),
                        (None, true) => Some((src.sel.0, None)),
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
                hint: src.hint,
                dirty: src.dirty,
                stale: src.stale,
                live: src.live,
                pulse: src.pulse,
                fenced: src.fenced,
                notified: src.notified,
                round: src.round,
                scroller: src.scroller,
                lane: acme.lane_open(view),
                hiding: src.hiding,
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
        let pal = palette(pp.kind);
        let lh = pp.fontspec.line_height;
        let lift = ink_lift(window, &pp.fontspec);
        // a folded window's tag is its card, a little shorter than the
        // line: the line centred in it, clipped as much top as bottom
        let shift = if pp.kind != Kind::Body { (bounds.size.height - lh).min(px(0.)) / 2. } else { px(0.) };
        let margin = if pp.kind == Kind::Body { px(BODY_MARGIN) } else { px(MARGIN) };
        let origin = point(bounds.left() + margin, bounds.top() + shift);

        // a notified window's header in a pale tint of the accent, as
        // Mail tints a flagged row: the whole bar says it wants the user.
        // Pale enough that a selection in the tag still shows on it: with
        // the GitHub palettes' accents, 6 to 9 in CIELAB from the plain
        // header and from the tag's selection, light and dark, under a
        // deuteranopia simulation too
        let header_bg = if pp.kind == Kind::WinTag && pp.notified {
            let th = crate::theme::theme();
            rgb(mix(th.tag_bg, th.accent, 0.10))
        } else {
            pal.bg
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // a card's corners where it has them; the column tags and the
            // top row on the ground, with no card of their own
            let r = px(CARD_RADIUS);
            let radii = gpui::Corners { top_left: if pp.round.0 { r } else { px(0.) }, top_right: if pp.round.0 { r } else { px(0.) }, bottom_left: if pp.round.1 { r } else { px(0.) }, bottom_right: if pp.round.1 { r } else { px(0.) } };
            let bg = if matches!(pp.kind, Kind::ColTag | Kind::Top) { rgb(ground(&crate::theme::theme())) } else { header_bg };
            window.paint_quad(fill(bounds, bg).corner_radii(radii));

            let mut scrollbar = None;
            let mut overlay = None;
            let mut layout_box = None;
            match pp.kind {
                Kind::Body => {
                    // the lane is acme's, B1, B2 and B3 all as ever; what
                    // shows in it is a Mac scroller's thumb, slim and
                    // rounded, with no track
                    // the lane to the pointer: the text's inset while shut,
                    // the whole lane once open (drawn over the text below)
                    let w = px(if pp.lane { SCROLLWID } else { LANE_HIT });
                    let sb = Bounds::new(point(bounds.right() - w, bounds.top()), size(w, bounds.size.height));
                    // acme's: the runes shown, of all of them
                    let total = pp.text_len.max(1) as f32;
                    let (s0, s1) = if pp.text_len == 0 { (0., 1.) } else { (pp.shown.0 as f32 / total, (pp.shown.1 as f32 / total).min(1.)) };
                    // an overlay scroller: seen while the text moves or the
                    // pointer is in the lane, fading after -- over the text,
                    // once it is drawn
                    let (shows, fading) = pp.scroller;
                    overlay = Some((s0, s1, shows));
                    if fading {
                        window.request_animation_frame();
                    }
                    scrollbar = Some(sb);
                }
                Kind::WinTag => {
                    let th = crate::theme::theme();
                    let b = Bounds::new(point(bounds.left(), origin.y), size(px(SCROLLWID), lh));
                    let d = dot(&th, pp.stale, pp.dirty, pp.live, pp.pulse.is_some(), pp.notified).squared(pp.hiding);
                    // in from the card's rounded corner
                    paint_dot(window, &d, point(b.left() + px(7.5), b.top() + lh / 2.));
                    layout_box = Some(b);
                }
                Kind::ColTag => {
                    let b = Bounds::new(point(bounds.left(), origin.y), size(px(SCROLLWID), lh));
                    paint_grip_as(window, b, rgb(crate::theme::theme().text_dim), pp.hiding);
                    layout_box = Some(b);
                }
                Kind::Top => {
                    // the session's own square: nothing to drag, so no grip;
                    // red when this client has lost its leases and only
                    // watches, and a click takes the oldest notification
                    let b = Bounds::new(point(bounds.left(), origin.y), size(px(SCROLLWID), lh));
                    if pp.fenced {
                        let th = crate::theme::theme();
                        let r = Bounds::new(point(b.left() + px(1.), b.top() + (lh - px(10.)) / 2.), size(px(10.), px(10.)));
                        window.paint_quad(fill(r, rgb(th.fenced)).corner_radii(px(3.)));
                    }
                    layout_box = Some(b);
                }
            }

            let (q0, q1) = pp.sel;
            let right = bounds.right();
            for line in &pp.lines {
                let ly = origin.y + line.y;
                let x = |d: usize| line.layout.unwrapped_layout.x_for_index(d);
                // the selection a ⌘- or ⌥-click would take (the pointer
                // on it): shown as the pill, not under it as the selection
                let sel_is_pill = q0 < q1 && pp.hint.is_some_and(|(a, b, _)| (a, b) == (q0, q1));
                let ranges: [(usize, usize, Hsla); 2] = [
                    if sel_is_pill { (0, 0, pal.sel) } else { (q0, q1, pal.sel) },
                    match pp.hl {
                        Some((lo, hi, HlKind::Exec)) => (lo, hi, rgb(crate::theme::theme().exec_hl)),
                        Some((lo, hi, HlKind::Look)) => (lo, hi, rgb(crate::theme::theme().look_hl)),
                        None => (0, 0, pal.sel),
                    },
                ];
                // a pill under what a click with ⌘ (B3) or ⌥ (B2) held
                // would take, in that sweep's own colour, its text in the
                // sweep's ink: what the click would drag, before it does
                {
                    let th = crate::theme::theme();
                    let pill = match pp.hint {
                        Some((a, b, HlKind::Look)) => Some((a, b, th.look_hl)),
                        Some((a, b, HlKind::Exec)) => Some((a, b, th.exec_hl)),
                        None => None,
                    };
                    if let Some((a, b, color)) = pill {
                        let (lo, hi) = (a.max(line.start), b.min(line.end));
                        if lo < hi {
                            let (dlo, dhi) = (line.to_disp(lo), line.to_disp(hi));
                            for (i, &(ds, de)) in line.subs.iter().enumerate() {
                                let (s, e) = (dlo.max(ds), dhi.min(de));
                                if s < e {
                                    let sy = ly + lh * i as f32;
                                    let r = Bounds::from_corners(point(origin.x + x(s) - x(ds) - px(3.), sy + px(1.)), point(origin.x + x(e) - x(ds) + px(3.), sy + lh - px(1.)));
                                    window.paint_quad(fill(r, rgb(color)).corner_radii(px(5.)));
                                }
                            }
                        }
                    }
                }
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
                            // softly rounded, as a modern editor's selection
                            // is: the whole of it one shape, its rows joined
                            // square -- only the top row's top corners and
                            // the bottom row's bottom ones rounded, or each
                            // row's corners would notch its edges
                            let top = a >= line.start && dlo >= ds && (dlo < de || last);
                            let bottom = if incl_nl { last && b == line.end + 1 } else { dhi > ds && dhi <= de };
                            let r = px(3.);
                            let radii = gpui::Corners {
                                top_left: if top { r } else { px(0.) },
                                top_right: if top { r } else { px(0.) },
                                bottom_left: if bottom { r } else { px(0.) },
                                bottom_right: if bottom { r } else { px(0.) },
                            };
                            window.paint_quad(fill(Bounds::from_corners(point(origin.x + x0, sy), point(origin.x + x1, sy + lh)), color).corner_radii(radii));
                        }
                    }
                }

                paint_glyphs(window, &line.layout.unwrapped_layout, &line.subs, point(origin.x, ly), lh, &line.colors, lift);
                // apex's verbs, drawn as their icons on their em spaces, in
                // the ink the word would have (faint, swept, hinted)
                for &(d, i) in &line.icons {
                    let sub = line.subs.iter().position(|&(ds, de)| d >= ds && d < de).unwrap_or(0);
                    let (ds, _) = line.subs[sub];
                    let (x0, x1) = (origin.x + x(d) - x(ds), origin.x + x(d + ICON_CELL.len_utf8()) - x(ds));
                    let side = pp.fontspec.size.min(lh - px(4.));
                    let c = point((x0 + x1) / 2., ly + lh * sub as f32 + lh / 2.);
                    let ink = line.colors.iter().find(|&&(a, b, _)| d >= a && d < b).map(|c| c.2).unwrap_or_else(|| rgb(crate::theme::theme().text_dim));
                    let name: SharedString = format!("apex-verb-{i}.svg").into();
                    let _ = window.paint_svg(Bounds::new(point(c.x - side / 2., c.y - side / 2.), size(side, side)), name, Some(verb_svg(i)), gpui::TransformationMatrix::unit(), ink, cx);
                }
                // the tag's `|`, drawn as a hairline as tall as the ink
                if let Some(d) = line.bar {
                    let sub = line.subs.iter().position(|&(ds, de)| d >= ds && d < de).unwrap_or(0);
                    let (ds, _) = line.subs[sub];
                    let mid = origin.x + (x(d) + x(d + 1)) / 2. - x(ds);
                    let tall = ink(window, &pp.fontspec).1;
                    let sy = ly + lh * sub as f32 + (lh - tall) / 2.;
                    let th = crate::theme::theme();
                    window.paint_quad(fill(Bounds::new(point(mid - px(0.5), sy), size(px(1.), tall)), rgb(th.body_border)));
                }

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
                    // as tall as the ink (ascender to descender) and a pixel
                    // over each way, centred on the line as the ink is --
                    // not the line's height, which a tag's air makes taller
                    let tall = (ink(window, &pp.fontspec).1 + px(2.)).min(lh - px(2.));
                    let cy = ty + (lh - tall) / 2.;
                    if keys {
                        window.paint_quad(fill(Bounds::new(point(cx_ - px(0.5), cy), size(px(2.), tall)), rgb(th.accent)).corner_radii(px(1.)));
                    } else {
                        window.paint_quad(fill(Bounds::new(point(cx_, cy + px(0.5)), size(px(1.5), tall - px(1.))), rgb(th.text)).corner_radii(px(0.75)));
                    }
                }
            }

            // a body's scroller, over its text
            if let Some((s0, s1, shows)) = overlay {
                paint_overlay_scroller(window, bounds, s0, s1, shows, pp.lane);
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

#[cfg(test)]
mod icon_tests {
    use super::{expand_icons, ICON_CELL};

    #[test]
    fn verbs_laid_out_as_icons_keep_their_words() {
        // "/a Del Snarf | Look": Del (runes 3..6) and Snarf (7..12) as icons
        let (disp, map, at) = expand_icons("/a Del Snarf | Look", 0, &[(3, 6, 0), (7, 12, 1)]);
        let cell = ICON_CELL.len_utf8();
        assert_eq!(disp, format!("/a {ICON_CELL} {ICON_CELL} | Look"));
        assert_eq!(at, vec![(3, 0), (3 + cell + 1, 1)]);
        // every byte of an icon is its word's start, the byte after it the
        // word's end: an offset in the word is at the icon's start or end
        assert!(map[3..3 + cell].iter().all(|&r| r == 3));
        assert_eq!(map[3 + cell], 6);
        let snarf = 3 + cell + 1;
        assert!(map[snarf..snarf + cell].iter().all(|&r| r == 7));
        assert_eq!(map[snarf + cell], 12);
        assert_eq!(*map.last().unwrap(), 19);
        assert_eq!(map.len(), disp.len() + 1);
    }
}
