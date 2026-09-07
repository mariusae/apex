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

// acme's colours, from plan9port src/cmd/acme/acme.c (iconinit) and
// include/draw.h. The two backgrounds are allocimagemix(DPaleyellow, DWhite)
// and allocimagemix(DPalebluegreen, DWhite): a 63/255 blend of the colour
// into white, which libdraw's MUL rounding makes exactly FFFFEA and EAFFFF.
pub const PALEYELLOW: u32 = 0xFFFFEA; // textcols[BACK]
pub const DARKYELLOW: u32 = 0xEEEE9E; // textcols[HIGH]  DDarkyellow
pub const YELLOWGREEN: u32 = 0x99994C; // textcols[BORD] DYellowgreen
pub const PALEBLUEGREEN: u32 = 0xEAFFFF; // tagcols[BACK]
pub const PALEGREYGREEN: u32 = 0x9EEEEE; // tagcols[HIGH] DPalegreygreen
pub const PURPLEBLUE: u32 = 0x8888CC; // tagcols[BORD] DPurpleblue; also colbutton
pub const MEDBLUE: u32 = 0x000099; // modbutton fill, DMedblue
/// `a` towards `b` by `t` (0..1), per channel.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let x = ((a >> shift) & 0xff) as f32;
        let y = ((b >> shift) & 0xff) as f32;
        ((x + (y - x) * t).round().clamp(0.0, 255.0) as u32) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// A live window's handle: a process is behind it. Dark magenta with a
/// quarter of yellow in it (a raspberry): unlike the dirty blue, the
/// fenced red, the unsynced green, and the scrollbar's dark yellow.
pub const LIVE: u32 = 0xB24073;
pub const BUT2COL: u32 = 0xAA0000; // but2col, text drawn white
pub const BUT3COL: u32 = 0x006600; // but3col, text drawn white
pub const BUTTON_BORDER: f32 = 2.; // ButtonBorder
/// Not acme's: the unsynced signal in the tag box (DMedgreen).
pub const MEDGREEN: u32 = 0x88CC88;

pub fn rgb(hex: u32) -> Hsla {
    Rgba::from(gpui::rgb(hex)).into()
}

pub struct Palette {
    pub bg: Hsla,
    pub sel: Hsla,
    pub border: Hsla,
}

pub fn palette(kind: Kind) -> Palette {
    match kind {
        Kind::Body => Palette { bg: rgb(PALEYELLOW), sel: rgb(DARKYELLOW), border: rgb(YELLOWGREEN) },
        _ => Palette { bg: rgb(PALEBLUEGREEN), sel: rgb(PALEGREYGREEN), border: rgb(PURPLEBLUE) },
    }
}

pub struct FontSpec {
    pub font: Font,
    pub size: Pixels,
    pub line_height: Pixels,
}

pub fn font_for(mono: bool) -> FontSpec {
    if mono {
        FontSpec { font: font("Menlo"), size: px(12.), line_height: px(16.) }
    } else {
        FontSpec { font: font("Lucida Grande"), size: px(13.), line_height: px(17.) }
    }
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
}

pub struct TextLayout {
    pub bounds: Bounds<Pixels>,
    pub text_origin: Point<Pixels>,
    pub line_height: Pixels,
    pub lines: Vec<LineInfo>,
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
    /// glyph, in window coordinates.
    pub fn point_of(&self, off: usize) -> Option<Point<Pixels>> {
        let lh = self.line_height;
        let line = self.lines.iter().find(|l| l.start <= off && (off < l.end || (off == l.end && !l.has_newline)))?;
        let d = line.to_disp(off);
        let p = line.layout.position_for_index(d, lh)?;
        Some(point(self.text_origin.x + p.x, self.text_origin.y + line.y + p.y))
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
    pub mono: bool,
    pub dirty: bool,
    /// A process is behind the window (a terminal's, a win's): neither
    /// clean nor dirty.
    pub live: bool,
    /// A page loading: how far (0..1) the handle is from live towards
    /// pale this instant.
    pub pulse: Option<f32>,
    pub unsynced: bool,
    /// This client no longer leads (its leases went elsewhere): the top
    /// row's square says so.
    pub fenced: bool,
    pub text: Text,
    pub sel: (usize, usize),
    pub origin: usize,
    pub hl: Option<(usize, usize, HlKind)>,
    pub want_visible: bool,
    /// Bring this position on screen, a quarter of the window down when
    /// it is not (acme's `textshow` for new `+Errors` text).
    pub show_at: Option<usize>,
}

pub struct TextElement {
    pub acme: Entity<Acme>,
    pub view: ViewId,
}

pub struct Prepaint {
    kind: Kind,
    fontspec: FontSpec,
    lines: Vec<LineInfo>,
    text_len: usize,
    total_lines: usize,
    first_line: usize,
    sel: (usize, usize),
    hl: Option<(usize, usize, HlKind)>,
    dirty: bool,
    live: bool,
    pulse: Option<f32>,
    unsynced: bool,
    fenced: bool,
}

#[allow(clippy::too_many_arguments)]
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
) -> LineInfo {
    let (disp, map) = expand(line_text, start);
    let disp: SharedString = disp.into();
    let black = gpui::black();
    let white = gpui::white();
    let run = |len: usize, color: Hsla| TextRun {
        len,
        font: fontspec.font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
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
    let mut runs = Vec::new();
    let mut colors = Vec::new();
    match hl {
        Some((lo, hi, _)) if lo < end && hi > start && lo < hi => {
            let dlo = info.to_disp(lo.max(start));
            let dhi = info.to_disp(hi.min(end));
            if dlo > 0 {
                runs.push(run(dlo, black));
                colors.push((0, dlo, black));
            }
            if dhi > dlo {
                runs.push(run(dhi - dlo, white));
                colors.push((dlo, dhi, white));
            }
            if disp.len() > dhi {
                runs.push(run(disp.len() - dhi, black));
                colors.push((dhi, disp.len(), black));
            }
        }
        _ => {
            runs.push(run(disp.len(), black));
            colors.push((0, disp.len(), black));
        }
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
            let fit = ((height / lh) as usize).max(1);

            let mut lines = Vec::new();
            let mut first = 0;
            if kind != Kind::Body {
                // what acme's wintaglines asks: how many lines the tag wraps to
                let mut y = px(0.);
                let mut n = 0;
                let mut wrapped = 0usize;
                while let Some((s, e)) = text.line_range(n) {
                    let li = shape(window, &text.slice(s, e), s, e, e < text_len, &fontspec, src.hl, wrap, y);
                    wrapped += li.subs.len().max(1);
                    y += li.height(lh);
                    lines.push(li);
                    n += 1;
                }
                let trailing = text_len > 0 && text.char_at(text_len - 1) == '\n';
                acme.tag_need.insert(view, (wrapped, trailing));
            } else if kind == Kind::Body {
                first = text.line_of(src.origin).min(total.saturating_sub(1));
                for _pass in 0..2 {
                    lines.clear();
                    let mut y = px(0.);
                    let mut n = first;
                    while y < height {
                        let Some((s, e)) = text.line_range(n) else { break };
                        let nl = e < text_len;
                        let li = shape(window, &text.slice(s, e), s, e, nl, &fontspec, src.hl, wrap, y);
                        y += li.height(lh);
                        lines.push(li);
                        n += 1;
                    }
                    let last_full = if y <= height { n } else { n.saturating_sub(1) };
                    if let Some(q) = src.show_at {
                        // textshow: the start of the new text, maxlines/4 from the top
                        let cl = text.line_of(q.min(text_len));
                        if cl < first || cl >= last_full {
                            first = cl.saturating_sub(fit / 4);
                            continue;
                        }
                        break;
                    }
                    if !src.want_visible {
                        break;
                    }
                    let cl = text.line_of(src.sel.1);
                    if cl < first {
                        first = cl;
                    } else if cl >= last_full {
                        first = cl.saturating_sub(fit / 2);
                    } else {
                        break;
                    }
                }
                let origin = text.line_start(first);
                if origin != src.origin || src.want_visible || src.show_at.is_some() {
                    acme.set_origin(view, origin);
                }
            } else {
                unreachable!()
            }
            Some(Prepaint {
                kind,
                fontspec,
                lines,
                text_len,
                total_lines: total,
                first_line: first,
                sel: src.sel,
                hl: src.hl,
                dirty: src.dirty,
                live: src.live,
                pulse: src.pulse,
                unsynced: src.unsynced,
                fenced: src.fenced,
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

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, pal.bg));

            let mut scrollbar = None;
            let mut layout_box = None;
            match pp.kind {
                Kind::Body => {
                    let sb = Bounds::new(bounds.origin, size(px(SCROLLWID), bounds.size.height));
                    window.paint_quad(fill(sb, pal.border));
                    let total = pp.total_lines.max(1) as f32;
                    let h = bounds.size.height;
                    let t0 = h * (pp.first_line as f32 / total);
                    let t1 = h * (((pp.first_line + pp.lines.len()) as f32).min(total) / total);
                    let thumb = Bounds::new(point(bounds.left(), bounds.top() + t0), size(px(SCROLLWID - 1.), (t1 - t0).max(px(2.))));
                    window.paint_quad(fill(thumb, pal.bg));
                    scrollbar = Some(sb);
                }
                Kind::WinTag => {
                    let b = Bounds::new(bounds.origin, size(px(SCROLLWID), lh));
                    window.paint_quad(fill(b, pal.border));
                    let bb = px(BUTTON_BORDER);
                    let inner = Bounds::new(point(b.left() + bb, b.top() + bb), size(b.size.width - bb * 2., b.size.height - bb * 2.));
                    let fillc = if pp.unsynced {
                        rgb(MEDGREEN)
                    } else if let (true, Some(t)) = (pp.live, pp.pulse) {
                        rgb(mix(LIVE, 0xFFFFEA, t * 0.85))
                    } else if pp.live {
                        rgb(LIVE)
                    } else if pp.dirty {
                        rgb(MEDBLUE)
                    } else {
                        pal.bg
                    };
                    window.paint_quad(fill(inner, fillc));
                    window.paint_quad(fill(
                        Bounds::new(point(bounds.left(), bounds.bottom() - px(1.)), size(bounds.size.width, px(1.))),
                        pal.border,
                    ));
                    layout_box = Some(b);
                }
                Kind::ColTag => {
                    let b = Bounds::new(bounds.origin, size(px(SCROLLWID), lh));
                    window.paint_quad(fill(b, pal.border));
                    layout_box = Some(b);
                }
                Kind::Top => {
                    // the upper-left square: filled when this client has
                    // lost its leases and only watches
                    let b = Bounds::new(bounds.origin, size(px(SCROLLWID), lh));
                    window.paint_quad(fill(b, pal.border));
                    let bb = px(BUTTON_BORDER);
                    let inner = Bounds::new(point(b.left() + bb, b.top() + bb), size(b.size.width - bb * 2., b.size.height - bb * 2.));
                    window.paint_quad(fill(inner, if pp.fenced { gpui::rgb(0xaa0000).into() } else { pal.bg }));
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
                        Some((lo, hi, HlKind::Exec)) => (lo, hi, rgb(BUT2COL)),
                        Some((lo, hi, HlKind::Look)) => (lo, hi, rgb(BUT3COL)),
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

                // the tick
                if q0 == q1 && q0 >= line.start && q0 <= line.end {
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
                    let black = gpui::black();
                    window.paint_quad(fill(Bounds::new(point(cx_, ty), size(px(1.), lh)), black));
                    window.paint_quad(fill(Bounds::new(point(cx_ - px(1.), ty), size(px(3.), px(3.))), black));
                    window.paint_quad(fill(Bounds::new(point(cx_ - px(1.), ty + lh - px(3.)), size(px(3.), px(3.))), black));
                }
            }

            let layout = TextLayout {
                bounds,
                text_origin: origin,
                line_height: lh,
                lines: pp.lines,
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
