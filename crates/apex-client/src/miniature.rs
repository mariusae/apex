//! A session drawn small, for the ctrl-tab switcher's cards: its columns
//! and windows where the tiling has them, their tags and the text their
//! bodies show (a terminal's screen, a text's lines from where it is
//! scrolled to), from the session's own replica -- so a card is live,
//! drawn afresh each frame from what the session is now. Pages, whose
//! native views are the shown session's alone, are their paper and name.

use apex_core::*;
use gpui::{point, px, size, App, Bounds, ContentMask, Pixels, Window};

use crate::text_element::{font_for, rgb, BODY_MARGIN, MARGIN};
use crate::theme::Theme;

/// What a card paints, in the session's own coordinates (its area's,
/// `w` wide): taken from a replica during the render, painted scaled.
pub struct Mini {
    pub w: f32,
    /// Rectangles in a colour, painted in order.
    fills: Vec<(f32, f32, f32, f32, u32)>,
    /// Lines of text, each in its window's clip.
    lines: Vec<Line>,
}

struct Line {
    x: f32,
    y: f32,
    clip: (f32, f32, f32, f32),
    mono: bool,
    runs: Vec<(String, u32)>,
}

/// Tabs as the text's tab stop sets them, for a line drawn plainly.
fn untab(s: &str, tabstop: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0;
    for c in s.chars() {
        if c == '\t' {
            let n = tabstop.max(1) - col % tabstop.max(1);
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else if c != '\n' {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// Session `node` as it is, to be drawn small.
pub fn snapshot(node: &Node, t: &Theme) -> Mini {
    let l = &node.state.layout;
    let (w, h) = (l.r.x1.max(1) as f32, l.r.y1.max(1) as f32);
    let font = f32::from(font_for(false).line_height);
    let mut m = Mini {
        w,
        fills: vec![(0., 0., w, h, t.border)],
        lines: Vec::new(),
    };
    let tag = |m: &mut Mini, b: BufferId, r: (f32, f32, f32, f32)| tag_into(m, node, None, b, r, t);
    if let Some(top) = l.top {
        tag(&mut m, top, (0., 0., w, font));
    }
    for (ci, col) in l.cols.iter().enumerate() {
        if !l.shows(ci) {
            continue;
        }
        let cr = col.r;
        m.fills.push((cr.x0 as f32, cr.y0 as f32, cr.dx() as f32, cr.dy() as f32, t.body_bg));
        tag(&mut m, col.tag, (cr.x0 as f32, cr.y0 as f32, cr.dx() as f32, font));
        for s in col.wins.iter().filter(|s| !col.hides(s.window)) {
            let r = s.r;
            let tag_h = if s.body.dy() > 0 { s.body.y0 - r.y0 } else { r.dy() };
            // the hairline where it meets the one above
            m.fills.push((r.x0 as f32, r.y0 as f32 - 1., r.dx() as f32, 1., t.body_border));
            let body = (s.body.dy() > 0).then(|| (s.body.x0 as f32, s.body.y0 as f32, s.body.dx() as f32, s.body.dy() as f32));
            window_into(&mut m, node, s.window, (r.x0 as f32, r.y0 as f32, r.dx() as f32, tag_h as f32), body, t);
        }
    }
    m
}

/// Window `w` drawn into `m`: its tag in `tag_r`, and in `body_r` (when
/// it has one) what its body shows from where it is scrolled to.
fn window_into(m: &mut Mini, node: &Node, w: WindowId, tag_r: (f32, f32, f32, f32), body_r: Option<(f32, f32, f32, f32)>, t: &Theme) {
    let Ok(win) = node.state.window(w) else { return };
    let prop = font_for(false);
    let mono = font_for(true);
    let font = f32::from(prop.line_height);
    tag_into(m, node, Some(w), win.tag, tag_r, t);
    let Some(clip) = body_r else { return };
    let b = (clip.0, clip.1, clip.2, clip.3);
    match win.body {
        Body::Text(buf) => {
            let Ok(bb) = node.state.buffer(buf) else { return };
            let lh = f32::from(if win.mono { mono.line_height } else { prop.line_height });
            let first = bb.text.line_of(bb.view(ViewId::Body(w)).origin);
            let n = (b.3 / lh).ceil() as usize;
            for i in 0..n {
                if first + i >= bb.text.line_count() {
                    break;
                }
                let line = untab(&bb.text.line(first + i), win.tabstop as usize);
                m.lines.push(Line {
                    x: b.0 + BODY_MARGIN,
                    y: b.1 + i as f32 * lh,
                    clip,
                    mono: win.mono,
                    runs: vec![(line, t.text)],
                });
            }
        }
        Body::Term(tid) => {
            let Some(term) = node.state.terms.get(&tid) else { return };
            let lh = f32::from(mono.line_height);
            for (i, row) in term.grid.iter().enumerate() {
                // runs of one colour, the cells' own
                let mut runs: Vec<(String, u32)> = Vec::new();
                for c in row {
                    let ink = if c.fg == 0 { t.text } else { crate::term_element::color_rgb(c.fg, t) };
                    match runs.last_mut() {
                        Some((s, k)) if *k == ink => s.push(c.ch),
                        _ => runs.push((c.ch.to_string(), ink)),
                    }
                }
                m.lines.push(Line {
                    x: b.0 + BODY_MARGIN,
                    y: b.1 + i as f32 * lh,
                    clip,
                    mono: true,
                    runs,
                });
            }
        }
        Body::Web | Body::Html(_) => {
            // the page's paper, and its name in the middle
            let paper = if matches!(win.body, Body::Html(_)) { t.body_bg } else { 0xffffff };
            m.fills.push((clip.0, clip.1, clip.2, clip.3, paper));
            let name = node.window_path(w);
            m.lines.push(Line {
                x: b.0 + MARGIN,
                y: b.1 + (b.3 - font) / 2.,
                clip,
                mono: false,
                runs: vec![(name, t.text_dim)],
            });
        }
    }
}

/// A tag's first line into `r`: a window's path in the ink and its
/// label, then the words in the tag dim.
fn tag_into(m: &mut Mini, node: &Node, w: Option<WindowId>, b: BufferId, r: (f32, f32, f32, f32), t: &Theme) {
    m.fills.push((r.0, r.1, r.2, r.3, t.tag_bg));
    let Ok(buf) = node.state.buffer(b) else { return };
    let line = untab(&buf.text.line(0), 4);
    let mut runs = Vec::new();
    if let Some(w) = w {
        runs.push((node.window_path(w), t.text));
        if let Some(l) = node.window_label(w) {
            runs.push((format!("  {l}"), t.text_dim));
        }
        runs.push(("  ".to_string(), t.text_dim));
    }
    runs.push((line, t.text_dim));
    m.lines.push(Line {
        x: r.0 + MARGIN,
        y: r.1,
        clip: r,
        mono: false,
        runs,
    });
}

/// Window `w` alone, drawn small as it would stand filling a space `w`
/// by `h`: its tag a line at the top, its body the rest.
pub fn snapshot_window(node: &Node, w: WindowId, width: f32, height: f32, t: &Theme) -> Mini {
    let font = f32::from(font_for(false).line_height);
    let mut m = Mini {
        w: width,
        fills: vec![(0., 0., width, height, t.body_bg)],
        lines: Vec::new(),
    };
    window_into(&mut m, node, w, (0., 0., width, font), Some((0., font + 1., width, height - font - 1.)), t);
    m
}

/// Where a card is drawn, leaning back as a sheet in a stack does: its
/// top edge at (`left`, `top`) and `width` across, its sides drawing in
/// by `slope` for each pixel down, its rows `vs` as tall as they are
/// wide (a card seen at an angle is shorter), and nothing below
/// `height`. Flat is a slope of 0 and a `vs` of 1.
#[derive(Clone, Copy, Debug)]
pub struct Tilt {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub slope: f32,
    pub vs: f32,
    pub height: f32,
}

impl Tilt {
    /// How far down the card (on the screen) content row `y` lands, for
    /// content `w` wide.
    fn down(&self, w: f32, y: f32) -> f32 {
        y * self.width / w * self.vs
    }

    /// Content point (`x`, `y`) on the screen, for content `w` wide.
    fn at(&self, w: f32, x: f32, y: f32) -> (f32, f32) {
        let d = self.down(w, y);
        let inset = self.slope * d;
        (self.left + inset + x * (self.width - 2. * inset) / w, self.top + d)
    }

    /// Screen pixels per content pixel across, at content row `y`.
    fn across(&self, w: f32, y: f32) -> f32 {
        (self.width - 2. * self.slope * self.down(w, y)) / w
    }
}

pub fn quad(window: &mut Window, c: [(f32, f32); 4], color: gpui::Hsla) {
    let mut p = gpui::PathBuilder::fill();
    p.move_to(point(px(c[0].0), px(c[0].1)));
    for &(x, y) in &c[1..] {
        p.line_to(point(px(x), px(y)));
    }
    p.close();
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

impl Mini {
    /// Painted into `b`, scaled to fit its width.
    pub fn paint(&self, b: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let t = Tilt { left: f32::from(b.left()), top: f32::from(b.top()), width: f32::from(b.size.width), slope: 0., vs: 1., height: f32::from(b.size.height) };
        self.paint_tilted(t, 1., window, cx);
    }

    /// Painted as `t` has it, at `alpha`.
    pub fn paint_tilted(&self, t: Tilt, alpha: f32, window: &mut Window, cx: &mut App) {
        let w = self.w;
        // the content rows that show: those above the card's `height`
        let ymax = t.height / (t.width / w * t.vs).max(1e-3);
        let ink = |c: u32| rgb(c).opacity(alpha);
        for &(x, y, fw, fh, c) in &self.fills {
            if y >= ymax {
                continue;
            }
            let y1 = (y + fh).min(ymax);
            let (a, b, cc, d) = (t.at(w, x, y), t.at(w, x + fw, y), t.at(w, x + fw, y1), t.at(w, x, y1));
            quad(window, [a, b, cc, d], ink(c));
        }
        let prop = font_for(false);
        let mono = font_for(true);
        for l in &self.lines {
            if l.y >= ymax {
                continue;
            }
            let fs = if l.mono { &mono } else { &prop };
            let text: String = l.runs.iter().map(|(s, _)| s.as_str()).collect();
            if text.trim().is_empty() {
                continue;
            }
            let runs: Vec<gpui::TextRun> = l
                .runs
                .iter()
                .filter(|(s, _)| !s.is_empty())
                .map(|(s, c)| gpui::TextRun { len: s.len(), font: fs.font.clone(), color: ink(*c), background_color: None, underline: None, strikethrough: None })
                .collect();
            let lh = f32::from(fs.line_height);
            // the row's own scale: narrower further down a leaning card
            let k = t.across(w, l.y + lh / 2.);
            let (x0, y0) = t.at(w, l.x, l.y);
            // the window's clip, as the rectangle round where it lands
            let (c0, c1) = (t.at(w, l.clip.0, l.clip.1), t.at(w, l.clip.0 + l.clip.2, (l.clip.1 + l.clip.3).min(ymax)));
            let clip = Bounds::new(point(px(c0.0), px(c0.1)), size(px((c1.0 - c0.0).max(0.)), px((c1.1 - c0.1).max(0.))));
            let shaped = window.text_system().shape_line(text.into(), fs.size * k, &runs, None);
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                let _ = shaped.paint(point(px(x0), px(y0)), px(lh * t.width / w * t.vs), gpui::TextAlign::Left, None, window, cx);
            });
        }
    }
}


/// Column `ci`'s windows as they stand, drawn small: its window space
/// (below its tag), `width` wide when given -- a strip's, as it would
/// stand brought back.
pub fn snapshot_column_at(node: &Node, ci: usize, width: Option<f32>, t: &Theme) -> Option<(Mini, f32)> {
    let c = node.state.layout.cols.get(ci)?;
    let font = f32::from(crate::text_element::tag_line_height());
    let (x0, y0) = (c.r.x0 as f32, c.r.y0 as f32 + font);
    let (w, h) = (width.unwrap_or(c.r.dx() as f32), (c.r.y1 as f32 - y0).max(1.));
    let mut m = Mini { w, fills: vec![(0., 0., w, h, t.body_bg)], lines: Vec::new() };
    for s in c.wins.iter().filter(|s| !c.hides(s.window)) {
        let r = s.r;
        let tag_h = if s.body.dy() > 0 { s.body.y0 - r.y0 } else { r.dy() };
        m.fills.push((0., r.y0 as f32 - y0 - 1., w, 1., t.body_border));
        // across the whole width asked for: a strip's windows are its width
        let body = (s.body.dy() > 0).then(|| (0., s.body.y0 as f32 - y0, w, s.body.dy() as f32));
        let _ = x0;
        window_into(&mut m, node, s.window, (0., r.y0 as f32 - y0, w, tag_h as f32), body, t);
    }
    Some((m, h))
}
