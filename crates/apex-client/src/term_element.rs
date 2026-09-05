//! Paints a terminal window from the term shard's grid in the core state.

use gpui::{
    fill, outline, point, px, relative, size, App, BorderStyle, Bounds, ContentMask, Element, ElementId, Entity,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, SharedString, Style, TextRun,
    Window,
};

use apex_core::{Cell, TermId, WindowId};
use apex_server::term::FLAG_BOLD;

use crate::app::Acme;
use crate::text_element::{font_for, rgb, FontSpec, MARGIN, PALEYELLOW, SCROLLWID, YELLOWGREEN};

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

fn color(packed: u32) -> Hsla {
    rgb(packed & 0x00ff_ffff)
}

struct RowDraw {
    text: SharedString,
    runs: Vec<TextRun>,
    bgs: Vec<(u16, u16, Hsla)>,
}

pub struct Prepaint {
    fontspec: FontSpec,
    cell_w: Pixels,
    rows: Vec<RowDraw>,
    row_text: Vec<String>,
    cols: u16,
    cursor: Option<(u16, u16)>,
    exited: bool,
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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.flex_grow = 1.;
        style.flex_shrink = 1.;
        style.flex_basis = px(0.).into();
        style.min_size.height = px(0.).into();
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
        let run = move |len: usize, color: Hsla| TextRun { len, font: font.clone(), color, background_color: None, underline: None, strikethrough: None };
        let cell_w = window.text_system().shape_line("M".into(), fontspec.size, &[run(1, gpui::black())], None).width;
        let lh = fontspec.line_height;
        let cols = (((bounds.size.width - px(MARGIN) - px(4.)) / cell_w).floor() as u16).max(2);
        let rows_n = ((bounds.size.height / lh).floor() as u16).max(1);
        let term = self.term;
        self.acme.update(cx, |acme, _| {
            acme.term_resize(term, cols, rows_n);
            let t = acme.node.state.terms.get(&term)?;
            let fg_default = gpui::black();
            let bg_default = rgb(PALEYELLOW);
            let cursor = if t.cursor_visible { Some(t.cursor) } else { None };
            let mut rows = Vec::with_capacity(t.grid.len());
            let mut row_text = Vec::with_capacity(t.grid.len());
            for (y, row) in t.grid.iter().enumerate() {
                let mut line = String::with_capacity(row.len());
                let mut runs: Vec<TextRun> = Vec::new();
                let mut bgs: Vec<(u16, u16, Hsla)> = Vec::new();
                for (x, cell) in row.iter().enumerate() {
                    let Cell { ch, fg, bg, flags } = *cell;
                    let mut fgc = if fg == 0 { fg_default } else { color(fg) };
                    let mut bgc = if bg == 0 { None } else { Some(color(bg)) };
                    if let Some((cx_, cy)) = cursor {
                        if cx_ as usize == x && cy as usize == y {
                            bgc = Some(fgc);
                            fgc = bg_default;
                        }
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
                    let _bold = flags & FLAG_BOLD != 0;
                    match runs.last_mut() {
                        Some(r) if r.color == fgc => r.len += len,
                        _ => runs.push(run(len, fgc)),
                    }
                }
                row_text.push(line.clone());
                rows.push(RowDraw { text: line.into(), runs, bgs });
            }
            Some(Prepaint { fontspec, cell_w, rows, row_text, cols: t.cols, cursor, exited: t.exit.is_some() })
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
        let origin = point(bounds.left() + px(MARGIN), bounds.top());
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, rgb(PALEYELLOW)));
            let sb = Bounds::new(bounds.origin, size(px(SCROLLWID), bounds.size.height));
            window.paint_quad(fill(sb, rgb(YELLOWGREEN)));
            for (i, row) in pp.rows.iter().enumerate() {
                let y = origin.y + lh * i as f32;
                for &(x, n, c) in &row.bgs {
                    window.paint_quad(fill(Bounds::new(point(origin.x + pp.cell_w * x as f32, y), size(pp.cell_w * n as f32, lh)), c));
                }
                if !row.runs.is_empty() {
                    let line = window.text_system().shape_line(row.text.clone(), pp.fontspec.size, &row.runs, None);
                    line.paint(point(origin.x, y), lh, window, cx).ok();
                }
            }
            if pp.exited {
                if let Some((cx_, cy)) = pp.cursor {
                    let x = origin.x + pp.cell_w * cx_ as f32;
                    let y = origin.y + lh * cy as f32;
                    window.paint_quad(outline(Bounds::new(point(x, y), size(pp.cell_w, lh)), gpui::black(), BorderStyle::Solid));
                }
            }
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
