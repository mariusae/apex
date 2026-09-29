//! Paints a terminal window from the term shard's grid in the core state.

use gpui::{
    fill, outline, point, px, relative, size, App, BorderStyle, Bounds, ContentMask, Element, ElementId, Entity,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, SharedString, Style, TextRun,
    Window,
};

use apex_core::{Cell, TermId, ViewId, WindowId};
use apex_server::term::{FLAG_BOLD, FLAG_UNDERLINE};

use crate::app::Acme;
use crate::text_element::{font_for, rgb, FontSpec, BODY_MARGIN, LANE_HIT, SCROLLWID};

pub struct TermLayout {
    pub bounds: Bounds<Pixels>,
    pub text_origin: Point<Pixels>,
    pub cell_w: Pixels,
    pub line_height: Pixels,
    pub rows: Vec<String>,
    pub cols: u16,
    pub scrollbar: Bounds<Pixels>,
}

impl TermLayout {
    pub fn cell_at(&self, pos: Point<Pixels>) -> (usize, usize) {
        let x = ((pos.x - self.text_origin.x) / self.cell_w).max(0.) as usize;
        let y = ((pos.y - self.text_origin.y) / self.line_height).max(0.) as usize;
        (x.min(self.cols.saturating_sub(1) as usize), y.min(self.rows.len().saturating_sub(1)))
    }
}

/// A cell's colour as the theme has it (entry.rs on the packing).
pub(crate) fn color_rgb(packed: u32, th: &crate::theme::Theme) -> u32 {
    match packed >> 24 {
        0xfe => th.ansi[(packed & 0xf) as usize],
        0xfd => if packed & 1 == 0 { th.text } else { th.body_bg },
        _ => packed & 0x00ff_ffff,
    }
}

struct RowDraw {
    text: SharedString,
    runs: Vec<TextRun>,
    bgs: Vec<(u16, u16, Hsla)>,
    /// The column each byte of `text` belongs to: a grid is drawn cell
    /// by cell, so every glyph is put where its cell is and no glyph's
    /// own width can move the ones after it.
    cols: Vec<u16>,
    /// The ink of each column.
    inks: Vec<Hsla>,
    /// The underlined stretches: first column, how many, in what ink.
    uls: Vec<(u16, u16, Hsla)>,
}

pub struct Prepaint {
    fontspec: FontSpec,
    cell_w: Pixels,
    rows: Vec<RowDraw>,
    row_text: Vec<String>,
    cols: u16,
    cursor: Option<(u16, u16)>,
    /// The keys go here: the cursor is the accent's caret, and whether it
    /// shows just now (it blinks with the text caret). None when they go
    /// elsewhere, and it is the plain dark caret.
    keys: Option<bool>,
    exited: bool,
    /// What the scrollbar shows: the viewport's first row and how many
    /// rows it holds, out of the whole screen's.
    view: (u64, u64, u64),
    /// A program at work (OSC 9;4), as far along as it says: the bar
    /// across the top. Full width while it does not say.
    progress: Option<Option<u8>>,
    /// The rows on screen where a command the shell marked (OSC 133)
    /// has its prompt, having failed.
    failed: Vec<usize>,
    /// How much of the scroller's thumb shows, and whether that is
    /// changing (an overlay scroller, `Acme::scroller`).
    scroller: (f32, bool),
    /// The pointer in the scroller's lane: it is open.
    lane: bool,
}

pub struct TermElement {
    pub acme: Entity<Acme>,
    pub window: WindowId,
    pub term: TermId,
}

impl IntoElement for TermElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TermElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepaint>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        // acme's tiling decides the rectangle; fill it
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Prepaint> {
        let fontspec = font_for(true);
        let font = fontspec.font.clone();
        let bold_font = gpui::Font { weight: gpui::FontWeight::BOLD, ..fontspec.font.clone() };
        let run = move |len: usize, color: Hsla| TextRun { len, font: font.clone(), color, background_color: None, underline: None, strikethrough: None };
        let cell_w = window.text_system().shape_line("M".into(), fontspec.size, &[run(1, gpui::black())], None).width;
        let lh = fontspec.line_height;
        let cols = (((bounds.size.width - px(BODY_MARGIN) - px(4.)) / cell_w).floor() as u16).max(2);
        let rows_n = ((bounds.size.height / lh).floor() as u16).max(1);
        let term = self.term;
        let win = self.window;
        self.acme.update(cx, |acme, _| {
            acme.term_resize(term, cols, rows_n);
            let at = acme.node.state.terms.get(&term).map(|t| t.top)?;
            let scroller = acme.scroller(ViewId::Body(win), at);
            let lane = acme.lane_open(ViewId::Body(win));
            let t = acme.node.state.terms.get(&term)?;
            let th = crate::theme::theme();
            let correct = crate::theme::contrast();
            let cursor = if t.cursor_visible { Some(t.cursor) } else { None };
            let keys = (acme.caret_term == Some(term)).then_some(acme.caret_on);
            // the selection, if it is in this terminal: acme's yellow
            let order = |a: (usize, u64), b: (usize, u64)| if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) };
            let sel = acme.term_sel.filter(|(sw, _, _)| *sw == self.window).map(|(_, a, b)| order(a, b));
            // a B2/B3 sweep shows in the button's colour, over the selection
            let hl = acme.term_hl.filter(|(sw, ..)| *sw == self.window).map(|(_, b, p0, p1)| (b, order(p0, p1)));
            let top = t.top;
            let total = t.total.max(t.rows as u64).max(1);
            let within = |x: usize, y: usize, (p0, p1): ((usize, u64), (usize, u64))| {
                let line = top + y as u64;
                (line, x) >= (p0.1, p0.0) && (line, x) < (p1.1, p1.0)
            };
            let highlight = |x: usize, y: usize| -> Option<(Hsla, Hsla)> {
                if let Some((b, r)) = hl {
                    if within(x, y, r) {
                        let bg = if b == gpui::MouseButton::Middle { th.exec_hl } else { th.look_hl };
                        return Some((rgb(bg), rgb(th.sweep_text)));
                    }
                }
                if sel.is_some_and(|r| within(x, y, r)) {
                    return Some((rgb(th.body_sel), rgb(th.text)));
                }
                None
            };
            let mut rows = Vec::with_capacity(t.grid.len());
            let mut row_text = Vec::with_capacity(t.grid.len());
            for (y, row) in t.grid.iter().enumerate() {
                let mut line = String::with_capacity(row.len());
                let mut runs: Vec<TextRun> = Vec::new();
                let mut bgs: Vec<(u16, u16, Hsla)> = Vec::new();
                let mut cols: Vec<u16> = Vec::with_capacity(row.len());
                let mut inks: Vec<Hsla> = Vec::with_capacity(row.len());
                let mut uls: Vec<(u16, u16, Hsla)> = Vec::new();
                for (x, cell) in row.iter().enumerate() {
                    let Cell { ch, fg, bg, flags, link } = *cell;
                    let fg_rgb = if fg == 0 { th.text } else { color_rgb(fg, th) };
                    let bg_rgb = if bg == 0 { None } else { Some(color_rgb(bg, th)) };
                    // the ink as it reads on its paper (contrast.rs):
                    // the theme's own on the theme's own needs no asking
                    let fg_rgb = if correct && (fg != 0 || bg != 0) { crate::contrast::correct(fg_rgb, bg_rgb.unwrap_or(th.body_bg), th) } else { fg_rgb };
                    let mut fgc = rgb(fg_rgb);
                    let mut bgc = bg_rgb.map(rgb);
                    if let Some((b, f)) = highlight(x, y) {
                        bgc = Some(b);
                        fgc = f;
                    }
                    if let Some(b) = bgc {
                        match bgs.last_mut() {
                            Some((sx, n, c)) if *c == b && (*sx + *n) as usize == x => *n += 1,
                            _ => bgs.push((x as u16, 1, b)),
                        }
                    }
                    let start = line.len();
                    line.push(if ch == '\0' { ' ' } else { ch });
                    let len = line.len() - start;
                    cols.resize(line.len(), x as u16);
                    inks.push(fgc);
                    let bold = flags & FLAG_BOLD != 0;
                    // underlined text, and OSC 8 links (B3 on one plumbs it)
                    let ul = flags & FLAG_UNDERLINE != 0 || link != 0;
                    if ul {
                        match uls.last_mut() {
                            Some((sx, n, c)) if *c == fgc && (*sx + *n) as usize == x => *n += 1,
                            _ => uls.push((x as u16, 1, fgc)),
                        }
                    }
                    match runs.last_mut() {
                        Some(r) if r.color == fgc && (r.font.weight == gpui::FontWeight::BOLD) == bold => r.len += len,
                        _ => {
                            let mut r = run(len, fgc);
                            if bold {
                                r.font = bold_font.clone();
                            }
                            runs.push(r);
                        }
                    }
                }
                row_text.push(line.clone());
                rows.push(RowDraw { text: line.into(), runs, bgs, cols, inks, uls });
            }
            let failed = t.marks.iter().filter(|m| m.exit.is_some_and(|e| e != 0) && m.prompt >= top && m.prompt < top + t.rows as u64).map(|m| (m.prompt - top) as usize).collect();
            Some(Prepaint { fontspec, cell_w, rows, row_text, cols: t.cols, cursor, keys, exited: t.exit.is_some(), view: (top, t.rows as u64, total), progress: t.working.then_some(t.progress), failed, scroller, lane })
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Option<Prepaint>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(pp) = prepaint.take() else { return };
        let lh = pp.fontspec.line_height;
        let origin = point(bounds.left() + px(BODY_MARGIN), bounds.top());
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let th = crate::theme::theme();
            window.paint_quad(fill(bounds, rgb(th.body_bg)));
            // acme's scrollbar, as a text window draws one: the bar dark,
            // and the part of it the viewport takes of the whole screen
            // (the history and the viewport together) in the paper
            // the lane to the pointer: the text's inset while shut, the
            // whole lane once open (drawn over the text, after it)
            let w = px(if pp.lane { SCROLLWID } else { LANE_HIT });
            let sb = Bounds::new(point(bounds.right() - w, bounds.top()), size(w, bounds.size.height));
            let (top, shown, total) = pp.view;
            let total = total.max(1);
            // an overlay scroller: seen while the view moves or the pointer
            // is in the lane, fading after
            let (shows, fading) = pp.scroller;
            if fading {
                window.request_animation_frame();
            }
            // a command that failed: a mark in the gutter by its prompt,
            // in the terminal's own red
            for &row in &pp.failed {
                let y = origin.y + lh * row as f32;
                let bar = Bounds::new(point(origin.x - px(3.5), y + px(2.)), size(px(2.5), lh - px(4.)));
                window.paint_quad(fill(bar, rgb(th.ansi[1])).corner_radii(px(1.)));
            }
            for (i, row) in pp.rows.iter().enumerate() {
                let y = origin.y + lh * i as f32;
                for &(x, n, c) in &row.bgs {
                    window.paint_quad(fill(Bounds::new(point(origin.x + pp.cell_w * x as f32, y), size(pp.cell_w * n as f32, lh)), c));
                }
                if !row.runs.is_empty() {
                    // shaped as a line, so a font's own choices still hold
                    // (and the symbols font can stand in where the mono
                    // one has nothing), but painted cell by cell: a glyph
                    // wider than a cell -- an icon from a Nerd Font is a
                    // whole em wide, a CJK ideograph two cells -- would
                    // otherwise push the rest of the row along with it
                    let line = window.text_system().shape_line(row.text.clone(), pp.fontspec.size, &row.runs, None);
                    let base = y + (lh - line.ascent - line.descent) / 2. + line.ascent;
                    for run in &line.runs {
                        for g in &run.glyphs {
                            let col = row.cols.get(g.index).copied().unwrap_or_default();
                            let at = point(origin.x + pp.cell_w * col as f32, base);
                            let _ = if g.is_emoji {
                                window.paint_emoji(at, run.font_id, g.id, pp.fontspec.size)
                            } else {
                                let ink = row.inks.get(col as usize).copied().unwrap_or(rgb(th.text));
                                window.paint_glyph(at, run.font_id, g.id, pp.fontspec.size, ink)
                            };
                        }
                    }
                    // and the underlines under their own cells, so a link
                    // is underlined as far as it reaches and no further
                    for &(x0, n, ink) in &row.uls {
                        let at = point(origin.x + pp.cell_w * x0 as f32, base + line.descent * 0.618);
                        window.paint_underline(at, pp.cell_w * n as f32, &gpui::UnderlineStyle { thickness: px(1.), color: Some(ink), wavy: false });
                    }
                }
            }
            // a program at work: the bar other terminals draw, across the
            // top of the text, as far along as it says (all of it while
            // it does not), over the text's first line and no taller
            if let Some(at) = pp.progress {
                let w = bounds.size.width;
                let part = at.map(|p| f32::from(p.min(100)) / 100.).unwrap_or(1.);
                let bar = Bounds::new(point(bounds.left(), bounds.top()), size((w * part).max(px(1.)), px(2.)));
                window.paint_quad(fill(bar, rgb(th.progress)));
            }
            // the cursor as a text window's caret: where the keys go, the
            // accent's, a little wider, blinking with the text caret; where
            // they do not, the plain dark one. A program that has ended
            // leaves a hollow box where its cursor was
            if let Some((cx_, cy)) = pp.cursor {
                let x = origin.x + pp.cell_w * cx_ as f32;
                let y = origin.y + lh * cy as f32;
                if pp.exited {
                    window.paint_quad(outline(Bounds::new(point(x, y), size(pp.cell_w, lh)), rgb(th.text), BorderStyle::Solid));
                } else {
                    match pp.keys {
                        Some(true) => {
                            // gliding there, with Smooth Cursor on
                            let frame = crate::text_element::caret_frame(bounds, pp.view.0, px(0.));
                            let at = crate::text_element::glide_caret(&self.acme, crate::text_element::CaretKey::Term(self.window), frame, point(x, y), window, cx);
                            window.paint_quad(fill(Bounds::new(point(at.x - px(0.5), at.y + px(1.)), size(px(2.), lh - px(2.))), rgb(th.accent)).corner_radii(px(1.)))
                        }
                        Some(false) => {}
                        None => window.paint_quad(fill(Bounds::new(point(x, y + px(2.)), size(px(1.5), lh - px(4.))), rgb(th.text)).corner_radii(px(0.75))),
                    }
                }
            }
            // the scroller, over the text
            crate::text_element::paint_overlay_scroller(window, bounds, top.min(total) as f32 / total as f32, (top + shown).min(total) as f32 / total as f32, shows, pp.lane);
            let layout = TermLayout {
                bounds,
                text_origin: origin,
                cell_w: pp.cell_w,
                line_height: lh,
                rows: pp.row_text,
                cols: pp.cols,
                scrollbar: sb,
            };
            let w = self.window;
            self.acme.update(cx, |acme, _| {
                acme.term_layouts.insert(w, layout);
            });
        });
    }
}
