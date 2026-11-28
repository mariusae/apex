//! A session sketched, for the sidebar's preview of another session (the
//! pointer on its row): not its text drawn small -- a screenshot in all
//! but name, too fine to read -- but what is on its screen, as Manifold's
//! space switcher draws a space: its windows where the tiling has them,
//! at their sizes, each a tile with its handle's dot, what kind of window
//! it is and its name, the one last worked in ringed. From the session's
//! own replica, so it is the session as it is now.

use apex_core::{Body, Node, WinKind, WindowId};
use gpui::{div, prelude::*, px, AnyElement};

use crate::text_element::{icon_index, rgb, Dot};
use crate::theme::Theme;

/// What a tile is, by its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Dir,
    Term,
    Errors,
    Page,
    /// A page with an input strip: an agent's conversation.
    Agent,
}

impl Kind {
    /// Its icon's name in `VERB_ICONS`.
    fn icon(self) -> &'static str {
        match self {
            Kind::Text => "kind:text",
            Kind::Dir => "kind:dir",
            Kind::Term => "kind:term",
            Kind::Errors => "kind:errors",
            Kind::Page => "kind:page",
            Kind::Agent => "kind:agent",
        }
    }
}

/// A window, as the sketch has it: where it is, in the session's area
/// taken as the unit square (y down), and what it shows.
#[derive(Clone, Debug)]
pub struct Tile {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Its tag's share of its height: the whole of it for a window shrunk
    /// to its tag.
    pub tag: f32,
    pub kind: Kind,
    pub name: String,
    pub dot: Dot,
    /// The window last worked in (the session's selected text's).
    pub focused: bool,
}

/// A session's screen, sketched.
#[derive(Clone, Debug)]
pub struct Sketch {
    /// Its area's width over its height.
    pub aspect: f32,
    pub tiles: Vec<Tile>,
    /// The windows put away in the stash.
    pub stashed: usize,
}

/// What kind of window `w` is, by its icon.
pub fn kind(node: &Node, w: WindowId) -> Kind {
    let Ok(win) = node.state.window(w) else { return Kind::Text };
    match win.body {
        Body::Term(_) => Kind::Term,
        Body::Page(_) if win.input.is_some() => Kind::Agent,
        Body::Page(_) => Kind::Page,
        _ => match node.window_kind(w) {
            WinKind::Dir => Kind::Dir,
            WinKind::Errors => Kind::Errors,
            WinKind::Term => Kind::Term,
            WinKind::Page => Kind::Page,
            WinKind::File => Kind::Text,
        },
    }
}

/// Session `node` as it is now, sketched: every window the tiling shows,
/// column by column (one hidden behind a window grown to its column, not).
pub fn sketch(node: &Node) -> Sketch {
    let l = &node.state.layout;
    let (x0, y0) = (l.r.x0 as f32, l.r.y0 as f32);
    let (aw, ah) = (((l.r.x1 - l.r.x0) as f32).max(1.), ((l.r.y1 - l.r.y0) as f32).max(1.));
    let focused = node.seltext.and_then(|v| v.window()).map(|w| l.top_of(w));
    let mut tiles = Vec::new();
    for c in &l.cols {
        for s in &c.wins {
            let w = s.window;
            if c.hides(w) {
                continue;
            }
            let (h, tag) = ((s.r.y1 - s.r.y0) as f32, (s.body.y0 - s.r.y0) as f32);
            if h <= 0. || s.r.x1 <= s.r.x0 {
                continue;
            }
            tiles.push(Tile {
                x: (s.r.x0 as f32 - x0) / aw,
                y: (s.r.y0 as f32 - y0) / ah,
                w: (s.r.x1 - s.r.x0) as f32 / aw,
                h: h / ah,
                tag: (tag / h).clamp(0., 1.),
                kind: kind(node, w),
                name: crate::sidebar::names(node, w).0,
                dot: crate::strips::dot_in(node, w, None),
                focused: focused == Some(w),
            });
        }
    }
    Sketch { aspect: aw / ah, tiles, stashed: l.stash.len() }
}

/// The bar across a tile's top: its tag, as tall as a line of the
/// sketch's type.
const BAR: f32 = 14.;
/// The space between tiles, and round them.
const GAP: f32 = 2.;

impl Sketch {
    /// The sketch drawn `w` wide (its height its area's for that width),
    /// in `t`'s colours: the ground the columns stand on, a tile a window
    /// -- its bar its dot, its icon and its name, its body plain paper
    /// with its icon large in it where there is room -- the window last
    /// worked in ringed in the accent; under it, the stash's count.
    pub fn element(&self, w: f32, t: &Theme) -> AnyElement {
        let h = w / self.aspect.max(0.1);
        let mut map = div().relative().w(px(w)).h(px(h)).bg(rgb(t.border));
        if self.tiles.is_empty() {
            map = map.flex().items_center().justify_center().text_size(px(11.)).text_color(rgb(t.text_dim)).child("No windows");
        }
        for tile in &self.tiles {
            let (x, y) = (tile.x * w + GAP / 2., tile.y * h + GAP / 2.);
            let (tw, th) = ((tile.w * w - GAP).max(0.), (tile.h * h - GAP).max(0.));
            if tw < 6. || th < 3. {
                continue;
            }
            let bar = BAR.min(th).max(tile.tag * th).min(th);
            let icon = icon_index(tile.kind.icon());
            let mut head = div().flex_none().h(px(bar)).w_full().flex().items_center().gap(px(4.)).px(px(4.)).bg(rgb(t.tag_bg)).overflow_hidden();
            if bar >= 8. {
                if tw > 24. {
                    head = head.child(div().flex_none().child(crate::sidebar::dot_element(&tile.dot)));
                }
                if tw > 40. {
                    head = head.child(crate::sidebar::icon_element(icon, 9., t.text_dim));
                }
                if tw > 56. {
                    head = head.child(div().flex_1().min_w_0().truncate().text_size(px(9.)).line_height(px(bar)).text_color(rgb(t.text)).child(tile.name.clone()));
                }
            }
            let mut body = div().flex_1().min_h_0().w_full().bg(rgb(t.body_bg)).flex().items_center().justify_center();
            if th - bar > 30. && tw > 30. {
                body = body.child(crate::sidebar::icon_element(icon, 16., theme_faint(t)));
            }
            let mut el = div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(tw))
                .h(px(th))
                .rounded(px(3.))
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(head)
                .child(body);
            if tile.focused && self.tiles.len() > 1 {
                el = el.border(px(1.5)).border_color(rgb(t.accent));
            }
            map = map.child(el);
        }
        let mut card = div().flex().flex_col().child(map);
        if self.stashed > 0 {
            let words = if self.stashed == 1 { "1 stashed".to_string() } else { format!("{} stashed", self.stashed) };
            card = card.child(div().px(px(8.)).py(px(4.)).text_size(px(11.)).text_color(rgb(t.text_dim)).child(words));
        }
        card.into_any_element()
    }
}

/// The ink of a tile's large icon: the body's, faint.
fn theme_faint(t: &Theme) -> u32 {
    crate::theme::step(t.body_bg, 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_core::{AttachmentKind, Log, Rect};

    fn session() -> (Log, Node, apex_core::ColumnId) {
        let mut log = Log::new();
        let (a, _) = log.attach(AttachmentKind::Ui, "ui");
        let mut node = Node::new(a);
        node.catch_up(&log).unwrap();
        let col = node.init_session(&mut log).unwrap();
        node.resize_layout(&mut log, Rect::new(0, 0, 1000, 600)).unwrap();
        (log, node, col)
    }

    #[test]
    fn a_session_is_sketched_as_its_windows_where_they_are() {
        let (mut log, mut node, col) = session();
        let a = node.new_window(&mut log, col, "/tmp/a.rs", "fn main() {}\n").unwrap();
        let dir = apex_core::node::Spec { kind: WinKind::Dir, ..Default::default() };
        let b = node.new_window_as(&mut log, col, "/tmp/", "a.rs\n", &dir).unwrap();
        node.select(&mut log, apex_core::ViewId::Body(a), 0, 0).unwrap();
        let s = sketch(&node);
        assert_eq!(s.tiles.len(), 2);
        assert!((s.aspect - 1000. / 600.).abs() < 0.01);
        let (ta, tb) = (&s.tiles[0], &s.tiles[1]);
        assert_eq!((ta.name.as_str(), ta.kind, ta.focused), ("a.rs", Kind::Text, true));
        assert_eq!((tb.kind, tb.focused), (Kind::Dir, false));
        // in the unit square, one over the other, the column's whole width
        for t in &s.tiles {
            assert!(t.x >= 0. && t.y >= 0. && t.x + t.w <= 1.001 && t.y + t.h <= 1.001, "{t:?}");
        }
        assert!(ta.y + ta.h <= tb.y + 0.001 || tb.y + tb.h <= ta.y + 0.001, "they overlap: {ta:?} {tb:?}");
        let _ = b;
    }
}
