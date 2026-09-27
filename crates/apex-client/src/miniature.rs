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
    let tag = |m: &mut Mini, b: BufferId, r: (f32, f32, f32, f32)| tag_into(m, node, b, r, t);
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
            let r = s.r;
            let tag_h = if s.body.dy() > 0 { s.body.y0 - r.y0 } else { r.dy() };
            // the hairline where it meets the one above
            m.fills.push((r.x0 as f32, r.y0 as f32 - 1., r.dx() as f32, 1., t.body_border));
            let body = (s.body.dy() > 0).then(|| (s.body.x0 as f32, s.body.y0 as f32, s.body.dx() as f32, s.body.dy() as f32));
            window_into(&mut m, node, s.window, (r.x0 as f32, r.y0 as f32, r.dx() as f32, tag_h as f32), body, t);
        }
        // a stash: its band, the colour of its sheets
        let band = tiling::stash_band(col);
        if band > 0 {
            m.fills.push((cr.x0 as f32 + 2., (cr.y1 - band + tiling::BORDER) as f32, cr.dx() as f32 - 4., (band - tiling::BORDER) as f32, t.tag_bg));
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
    tag_into(m, node, win.tag, tag_r, t);
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
                    x: b.0 + MARGIN,
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
                    x: b.0 + MARGIN,
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
            let name = node.window_name(w);
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

/// A tag's first line into `r`: the name in the ink, the rest dim.
fn tag_into(m: &mut Mini, node: &Node, b: BufferId, r: (f32, f32, f32, f32), t: &Theme) {
    m.fills.push((r.0, r.1, r.2, r.3, t.tag_bg));
    let Ok(buf) = node.state.buffer(b) else { return };
    let line = untab(&buf.text.line(0), 4);
    let (name, rest) = match line.find(' ') {
        Some(i) => (line[..i].to_string(), line[i..].to_string()),
        None => (line, String::new()),
    };
    m.lines.push(Line {
        x: r.0 + MARGIN,
        y: r.1,
        clip: r,
        mono: false,
        runs: vec![(name, t.text), (rest, t.text_dim)],
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
                    .map(|(s, c)| gpui::TextRun {
                        len: s.len(),
                        font: fs.font.clone(),
                        color: rgb(*c),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    })
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
