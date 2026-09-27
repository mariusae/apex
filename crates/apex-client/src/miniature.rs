//! A session drawn small, for the ctrl-tab switcher's cards: its columns
//! and windows where the tiling has them, their tags and the text their
//! bodies show (a terminal's screen, a text's lines from where it is
//! scrolled to), from the session's own replica -- so a card is live,
//! drawn afresh each frame from what the session is now. Pages, whose
//! native views are the shown session's alone, are their paper and name.

use apex_core::tiling;
use apex_core::*;
use gpui::{point, px, size, App, Bounds, ContentMask, Pixels, Window};

use crate::text_element::{font_for, rgb, MARGIN};
use crate::theme::Theme;

/// What a card paints, in the session's own coordinates (its area's,
/// `w` by `h`): taken from a replica during the render, painted scaled.
pub struct Mini {
    pub w: f32,
    pub h: f32,
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
    let prop = font_for(false);
    let mono = font_for(true);
    let font = f32::from(prop.line_height);
    let mut m = Mini { w, h, fills: vec![(0., 0., w, h, t.border)], lines: Vec::new() };
    let text_of = |b: BufferId| node.state.buffer(b).ok().map(|b| b.text.clone());
    // a tag: its first line, the name in the ink and the rest dim
    let tag = |m: &mut Mini, b: BufferId, r: (f32, f32, f32, f32)| {
        m.fills.push((r.0, r.1, r.2, r.3, t.tag_bg));
        let Some(text) = text_of(b) else { return };
        let line = untab(&text.line(0), 4);
        let (name, rest) = match line.find(' ') {
            Some(i) => (line[..i].to_string(), line[i..].to_string()),
            None => (line, String::new()),
        };
        m.lines.push(Line { x: r.0 + MARGIN, y: r.1, clip: r, mono: false, runs: vec![(name, t.text), (rest, t.text_dim)] });
    };
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
        for s in &col.wins {
            let Ok(win) = node.state.window(s.window) else { continue };
            let r = s.r;
            let tag_h = if s.body.dy() > 0 { s.body.y0 - r.y0 } else { r.dy() };
            // the hairline where it meets the one above
            m.fills.push((r.x0 as f32, r.y0 as f32 - 1., r.dx() as f32, 1., t.body_border));
            tag(&mut m, win.tag, (r.x0 as f32, r.y0 as f32, r.dx() as f32, tag_h as f32));
            let b = s.body;
            if b.dy() <= 0 {
                continue;
            }
            let clip = (b.x0 as f32, b.y0 as f32, b.dx() as f32, b.dy() as f32);
            match win.body {
                Body::Text(buf) => {
                    let Ok(bb) = node.state.buffer(buf) else { continue };
                    let lh = f32::from(if win.mono { mono.line_height } else { prop.line_height });
                    let first = bb.text.line_of(bb.view(ViewId::Body(s.window)).origin);
                    let n = (b.dy() as f32 / lh).ceil() as usize;
                    for i in 0..n {
                        if first + i >= bb.text.line_count() {
                            break;
                        }
                        let line = untab(&bb.text.line(first + i), win.tabstop as usize);
                        m.lines.push(Line { x: b.x0 as f32 + MARGIN, y: b.y0 as f32 + i as f32 * lh, clip, mono: win.mono, runs: vec![(line, t.text)] });
                    }
                }
                Body::Term(tid) => {
                    let Some(term) = node.state.terms.get(&tid) else { continue };
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
                        m.lines.push(Line { x: b.x0 as f32 + MARGIN, y: b.y0 as f32 + i as f32 * lh, clip, mono: true, runs });
                    }
                }
                Body::Web | Body::Html(_) => {
                    // the page's paper, and its name in the middle
                    let paper = if matches!(win.body, Body::Html(_)) { t.body_bg } else { 0xffffff };
                    m.fills.push((clip.0, clip.1, clip.2, clip.3, paper));
                    let name = node.window_name(s.window);
                    m.lines.push(Line { x: b.x0 as f32 + MARGIN, y: b.y0 as f32 + (b.dy() as f32 - font) / 2., clip, mono: false, runs: vec![(name, t.text_dim)] });
                }
            }
        }
        // a stash: its band, the colour of its sheets
        let band = tiling::stash_band(col);
        if band > 0 {
            m.fills.push((cr.x0 as f32 + 2., (cr.y1 - band + tiling::BORDER) as f32, cr.dx() as f32 - 4., (band - tiling::BORDER) as f32, t.tag_bg));
        }
    }
    m
}

impl Mini {
    /// Painted into `b`, scaled to fit its width.
    pub fn paint(&self, b: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let s = f32::from(b.size.width) / self.w;
        let at = |x: f32, y: f32| point(b.left() + px(x * s), b.top() + px(y * s));
        window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
            for &(x, y, w, h, c) in &self.fills {
                window.paint_quad(gpui::fill(Bounds::new(at(x, y), size(px(w * s), px(h * s))), rgb(c)));
            }
            let prop = font_for(false);
            let mono = font_for(true);
            for l in &self.lines {
                let fs = if l.mono { &mono } else { &prop };
                let text: String = l.runs.iter().map(|(s, _)| s.as_str()).collect();
                if text.trim().is_empty() {
                    continue;
                }
                let runs: Vec<gpui::TextRun> = l
                    .runs
                    .iter()
                    .filter(|(s, _)| !s.is_empty())
                    .map(|(s, c)| gpui::TextRun { len: s.len(), font: fs.font.clone(), color: rgb(*c), background_color: None, underline: None, strikethrough: None })
                    .collect();
                let clip = Bounds::new(at(l.clip.0, l.clip.1), size(px(l.clip.2 * s), px(l.clip.3 * s)));
                let shaped = window.text_system().shape_line(text.into(), fs.size * s, &runs, None);
                window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                    let _ = shaped.paint(at(l.x, l.y), fs.line_height * s, gpui::TextAlign::Left, None, window, cx);
                });
            }
        });
    }
}
