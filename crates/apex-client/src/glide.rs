//! Windows glide to their places. The tiling moves a window at once --
//! a grow, a stash or a recall, a drag, a close -- and the layout is the
//! core's; what is drawn is a copy of it in which each window and column
//! that has just moved is part of the way from where it was drawn to
//! where it now is, for a sixth of a second, easing out. A window that
//! appears (made, recalled) opens down from its top. The OS window
//! resizing, or another session shown, is no move: everything is where
//! it is at once. While a window glides its terminal keeps its size and
//! its text keeps its scroll, both settled once it lands.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use apex_core::state::Layout;
use apex_core::tiling::Rect;
use apex_core::{ColumnId, WindowId};

use crate::app::Acme;

const GLIDE: Duration = Duration::from_millis(160);
/// A glide begun while the last was still on its way: the user is ahead
/// of it (a second click on a handle), so it jumps to where the last was
/// going and goes on from there quicker.
const HURRY: Duration = Duration::from_millis(80);

/// A rectangle part of the way (`k`) from `a` to `b`.
fn lerp(a: Rect, b: Rect, k: f32) -> Rect {
    let l = |x: i32, y: i32| x + ((y - x) as f32 * k).round() as i32;
    Rect::new(l(a.x0, b.x0), l(a.y0, b.y0), l(a.x1, b.x1), l(a.y1, b.y1))
}

pub fn ease(k: f32) -> f32 {
    1. - (1. - k.clamp(0., 1.)).powi(3)
}

/// A window's two rectangles: all of it, and its body.
type Place = (Rect, Rect);

#[derive(Default)]
pub struct Glide {
    /// Where each window and column was last put by the tiling.
    wins: HashMap<WindowId, Place>,
    cols: HashMap<ColumnId, Rect>,
    /// Those on their way: from, to, since, taking how long.
    moving_w: HashMap<WindowId, (Place, Place, Instant, Duration)>,
    moving_c: HashMap<ColumnId, (Rect, Rect, Instant, Duration)>,
    /// The row and the tab last drawn: when either changes, nothing glides.
    row: Option<(Rect, crate::pool::TabId)>,
}

impl Glide {
    fn now_w(&self, w: WindowId) -> Option<Place> {
        let (a, b, at, d) = self.moving_w.get(&w)?;
        let k = ease(at.elapsed().as_secs_f32() / d.as_secs_f32());
        Some((lerp(a.0, b.0, k), lerp(a.1, b.1, k)))
    }

    fn now_c(&self, c: ColumnId) -> Option<Rect> {
        let (a, b, at, d) = self.moving_c.get(&c)?;
        let k = ease(at.elapsed().as_secs_f32() / d.as_secs_f32());
        Some(lerp(*a, *b, k))
    }

    /// Where window `w` is drawn this instant, while it is on its way.
    pub fn drawn_at(&self, w: WindowId) -> Option<Rect> {
        self.now_w(w).map(|(r, _)| r)
    }

    /// Is window `w` on its way somewhere?
    pub fn gliding(&self, w: WindowId) -> bool {
        self.moving_w.contains_key(&w)
    }

    pub fn any(&self) -> bool {
        !self.moving_w.is_empty() || !self.moving_c.is_empty()
    }
}

impl Acme {
    /// The layout to draw this frame: the tiling's, with whatever has
    /// just moved part of the way there.
    pub fn glided_layout(&mut self) -> Layout {
        let mut l = self.node.state.layout.clone();
        let g = &mut self.glide;
        // what has landed is done
        g.moving_w.retain(|_, (_, _, at, d)| at.elapsed() < *d);
        g.moving_c.retain(|_, (_, _, at, d)| at.elapsed() < *d);
        let here = (l.r, self.tab);
        let snap = g.row != Some(here);
        g.row = Some(here);
        if snap {
            g.moving_w.clear();
            g.moving_c.clear();
        }
        let before = !g.wins.is_empty() && !snap;
        let mut wins = HashMap::new();
        let mut cols = HashMap::new();
        for c in &l.cols {
            if let Some(&was) = g.cols.get(&c.id) {
                if was != c.r && !snap {
                    // still on its way: from where it was going, quicker
                    let (from, d) = if g.moving_c.contains_key(&c.id) { (was, HURRY) } else { (was, GLIDE) };
                    g.moving_c.insert(c.id, (from, c.r, Instant::now(), d));
                }
            }
            cols.insert(c.id, c.r);
            for s in &c.wins {
                let to = (s.r, s.body);
                match g.wins.get(&s.window) {
                    Some(&was) if was != to && !snap => {
                        // still on its way (a second click before it
                        // landed): from where it was going, quicker
                        let (from, d) = if g.moving_w.contains_key(&s.window) { (was, HURRY) } else { (was, GLIDE) };
                        g.moving_w.insert(s.window, (from, to, Instant::now(), d));
                    }
                    // appearing among windows that were there: opening down
                    // from its top
                    None if before => {
                        let flat = |r: Rect| Rect::new(r.x0, r.y0, r.x1, r.y0);
                        g.moving_w.insert(s.window, ((flat(s.r), flat(s.body)), to, Instant::now(), GLIDE));
                    }
                    _ => {}
                }
                wins.insert(s.window, to);
            }
        }
        g.wins = wins;
        g.cols = cols;
        // the copy drawn: each on its way where it is now
        for c in l.cols.iter_mut() {
            if let Some(r) = g.now_c(c.id) {
                c.r = r;
            }
            for s in c.wins.iter_mut() {
                if let Some((r, body)) = g.now_w(s.window) {
                    s.r = r;
                    s.body = body;
                }
            }
        }
        l
    }
}
