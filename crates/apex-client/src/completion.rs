//! ^F: completion inline, at the caret. The path fragment before the
//! caret goes to the server (which has the file system); what all its
//! candidates share is typed in at once, and when more than one is left
//! they are listed under the caret. Typing goes on into the text as ever
//! and narrows the list; ↑ ↓ choose; return or tab takes the one chosen;
//! escape, a click elsewhere, or a slash typed puts the list away. One
//! candidate is simply typed in: a directory with its slash, a file with
//! a space after, as acme's ^F does. A completion done is done: the next
//! list, a directory's names say, comes only with the next ^F.

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Context, MouseButton};

use apex_core::ViewId;
use apex_server::proto::Candidates;

use crate::app::Acme;

/// The most rows the list shows at once.
const ROWS: usize = 8;

pub struct Completion {
    pub view: ViewId,
    /// Where the name being completed (the fragment's last part) starts.
    pub start: usize,
    pub names: Vec<(String, bool)>,
    /// The one chosen, among those still matching.
    pub cursor: usize,
    /// Nothing matched: said a moment, then gone.
    pub none: Option<std::time::Instant>,
    /// Where the name starts on the screen, and its line's height: taken
    /// from the last frame's layout, before a frame lays it out anew.
    pub anchor: Option<(gpui::Point<gpui::Pixels>, gpui::Pixels)>,
    /// Frames drawn without finding where it goes: the names came before
    /// the text they follow was laid out (a directory's, asked for as its
    /// name was typed in). A few more are asked for; then it goes.
    pub unplaced: u8,
}

/// How many frames a list waits to be placed.
const UNPLACED: u8 = 4;

impl Completion {
    /// The names matching what is typed of the name so far.
    pub fn matching(&self, typed: &str) -> Vec<(String, bool)> {
        self.names.iter().filter(|(n, _)| n.starts_with(typed)).cloned().collect()
    }
}

/// The longest start all of `names` share.
fn common(names: &[(String, bool)]) -> String {
    let Some((first, _)) = names.first() else { return String::new() };
    let mut n = first.chars().count();
    for (other, _) in &names[1..] {
        n = n.min(first.chars().zip(other.chars()).take_while(|(a, b)| a == b).count());
    }
    first.chars().take(n).collect()
}

fn is_file_char(c: char) -> bool {
    !c.is_whitespace() && !"\"'`()[]{}<>|;".contains(c)
}

impl Acme {
    /// The names completing `c.prefix` arrived: what they share typed in,
    /// and the rest listed under the caret.
    pub fn got_candidates(&mut self, c: Candidates) {
        // the caret must still be where it was asked from
        if self.node.selection(c.view).ok() != Some((c.at, c.at)) {
            return;
        }
        let base = c.prefix.rsplit_once('/').map(|(_, b)| b).unwrap_or(&c.prefix).to_string();
        let start = c.at - base.chars().count();
        let names = c.names.unwrap_or_default();
        if names.is_empty() {
            self.completion = Some(Completion { view: c.view, start, names, cursor: 0, none: Some(std::time::Instant::now()), anchor: None, unplaced: 0 });
            return;
        }
        if names.len() == 1 {
            let (name, dir) = names[0].clone();
            self.complete_with(c.view, start, c.at, &name, dir);
            return;
        }
        // what they all share, typed in now
        let shared = common(&names);
        let typed = base.chars().count();
        let more: String = shared.chars().skip(typed).collect();
        if !more.is_empty() {
            let _ = self.node.insert(&mut self.log, c.view, &more);
            self.after();
        }
        self.completion = Some(Completion { view: c.view, start, names, cursor: 0, none: None, anchor: None, unplaced: 0 });
    }

    /// The name `name` in place of what is typed of it (from `start` to
    /// `at`): a directory with its slash, a file with a space after, and
    /// the list put away.
    fn complete_with(&mut self, view: ViewId, start: usize, at: usize, name: &str, dir: bool) {
        // done: a directory's names wait for the next ^F
        let text = if dir { format!("{name}/") } else { format!("{name} ") };
        let _ = self.node.select(&mut self.log, view, start, at);
        let _ = self.node.replace_selection(&mut self.log, view, &text);
        // the caret after it, nothing left selected
        let end = start + text.chars().count();
        let _ = self.node.select(&mut self.log, view, end, end);
        self.completion = None;
        self.after();
    }

    /// What is typed of the name so far, while the caret is after it and
    /// nothing but a name's characters lie between: else None.
    fn completion_typed(&self) -> Option<(String, usize)> {
        let c = self.completion.as_ref()?;
        let (q0, q1) = self.node.selection(c.view).ok()?;
        if q0 != q1 || q0 < c.start {
            return None;
        }
        let t = self.text_of(c.view)?;
        let typed = t.slice(c.start, q0);
        typed.chars().all(|ch| is_file_char(ch) && ch != '/').then_some((typed, q0))
    }

    /// A key while the list is up: ↑ ↓ choose, return or tab takes, escape
    /// puts it away. True when the key was the list's.
    pub fn completion_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(c) = self.completion.as_ref() else { return false };
        // a list not drawn (not placed yet) takes no keys: return is the
        // text's, not a name nobody saw chosen
        if c.anchor.is_none() {
            self.completion = None;
            cx.notify();
            return false;
        }
        if c.none.is_some() {
            self.completion = None;
            cx.notify();
            return key == "escape";
        }
        let Some((typed, at)) = self.completion_typed() else {
            self.completion = None;
            return false;
        };
        let matching = c.matching(&typed);
        match key {
            "up" | "down" => {
                let n = matching.len().max(1);
                if let Some(c) = self.completion.as_mut() {
                    c.cursor = if key == "up" { (c.cursor + n - 1) % n } else { (c.cursor + 1) % n };
                }
            }
            "enter" | "tab" => {
                let (view, start, cursor) = (c.view, c.start, c.cursor);
                match matching.get(cursor.min(matching.len().saturating_sub(1))) {
                    Some((name, dir)) => {
                        let (name, dir) = (name.clone(), *dir);
                        self.complete_with(view, start, at, &name, dir);
                    }
                    None => self.completion = None,
                }
            }
            "escape" => self.completion = None,
            _ => return false,
        }
        cx.notify();
        true
    }

    /// After a key went into the text: the list follows what is typed,
    /// and goes when the name is left (a space, the caret moved away) or
    /// a slash starts another (whose names are asked for).
    pub fn completion_follow(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.completion.as_ref() else { return };
        let view = c.view;
        let Ok((q0, q1)) = self.node.selection(view) else { return };
        // a slash typed: the name is done, and the list goes (the next
        // directory's names wait for the next ^F)
        if q0 == q1 && q0 > c.start && self.text_of(view).is_some_and(|t| t.char_at(q0 - 1) == '/') {
            self.completion = None;
            cx.notify();
            return;
        }
        match self.completion_typed() {
            Some((typed, _)) => {
                let n = c.matching(&typed).len();
                if n == 0 {
                    self.completion = None;
                } else if let Some(c) = self.completion.as_mut() {
                    c.cursor = c.cursor.min(n - 1);
                }
            }
            None => self.completion = None,
        }
        cx.notify();
    }

    /// Where the list goes, from the last frame's layout of its text:
    /// called before the frame clears the layouts to draw them again.
    /// True when it could not be placed yet and another frame is wanted.
    pub fn completion_anchor(&mut self) -> bool {
        let Some(c) = self.completion.as_ref() else { return false };
        // at the name's start; failing that, just past the rune before it
        let anchor = self.layouts.get(&c.view).and_then(|l| {
            let p = l.point_of(c.start).or_else(|| c.start.checked_sub(1).and_then(|q| l.point_of(q)).map(|p| gpui::point(p.x + px(8.), p.y)))?;
            Some((p, l.line_height))
        });
        let Some(c) = self.completion.as_mut() else { return false };
        match anchor {
            Some(a) => {
                c.anchor = Some(a);
                false
            }
            None if c.anchor.is_none() => {
                c.unplaced += 1;
                if c.unplaced > UNPLACED {
                    self.completion = None;
                    return false;
                }
                true
            }
            None => false,
        }
    }

    /// The list under the caret.
    pub fn completion_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.completion.as_ref()?;
        if c.none.is_some_and(|t| t.elapsed() > std::time::Duration::from_millis(1500)) {
            self.completion = None;
            return None;
        }
        let c = self.completion.as_ref()?;
        let (at, lh) = c.anchor?;
        let t = crate::theme::theme();
        let mono = self.node.state.window(c.view.window()?).is_ok_and(|w| w.mono);
        let font = crate::text_element::font_for(mono).font.family.clone();
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.2), offset: gpui::point(px(0.), px(4.)), blur_radius: px(14.), spread_radius: px(0.), inset: false };
        // under the name's line, or over it where the window has no room
        // below (at the window's foot): `anchored` turns it about the
        // line's top, and keeps it inside the window
        let place = |panel: gpui::Div| deferred(anchored().position(point(at.x - px(6.), at.y - px(2.))).offset(point(px(0.), lh + px(4.))).child(panel)).with_priority(2).into_any_element();
        let panel = div()
            .min_w(px(160.))
            .max_w(px(420.))
            .p(px(4.))
            .rounded(px(7.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .font_family(font)
            .text_size(px(13.))
            .flex()
            .flex_col()
            .child(self.overlay_mark());
        if c.none.is_some() {
            return Some(place(panel.child(div().px(px(8.)).py(px(3.)).text_color(rgb(t.panel_dim)).child("No matches"))));
        }
        let (typed, _) = self.completion_typed()?;
        let matching = c.matching(&typed);
        let cursor = c.cursor.min(matching.len().saturating_sub(1));
        // a window of the rows round the one chosen
        let first = cursor.saturating_sub(ROWS - 1).min(matching.len().saturating_sub(ROWS));
        let mut panel = panel;
        for (i, (name, dir)) in matching.iter().enumerate().skip(first).take(ROWS) {
            let picked = i == cursor;
            let (view, start, name2, dir2) = (c.view, c.start, name.clone(), *dir);
            let row = div()
                .id(("completion", i))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .flex()
                .flex_row()
                .gap(px(1.))
                .cursor_default()
                .relative()
                .pl(px(10.))
                // what is typed already, then the rest
                .child(div().flex_none().when(!picked, |d| d.text_color(rgb(t.panel_dim))).child(typed.clone()))
                .child(div().flex_none().child(name.chars().skip(typed.chars().count()).collect::<String>()))
                .when(*dir, |d| d.child(div().flex_none().when(!picked, |d| d.text_color(rgb(t.panel_dim))).child("/")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some((_, at)) = this.completion_typed() {
                            this.complete_with(view, start, at, &name2, dir2);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                );
            panel = panel.child(crate::shell::chosen(row, picked, crate::shell::Act::Look, 3.));
        }
        if matching.len() > ROWS {
            panel = panel.child(div().px(px(8.)).text_size(px(11.)).text_color(rgb(t.panel_dim)).child(format!("{} more", matching.len() - ROWS)));
        }
        Some(place(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::common;

    #[test]
    fn what_candidates_share() {
        let n = |v: &[&str]| v.iter().map(|s| (s.to_string(), false)).collect::<Vec<_>>();
        assert_eq!(common(&n(&["alpha.txt", "alpine.txt"])), "alp");
        assert_eq!(common(&n(&["one"])), "one");
        assert_eq!(common(&n(&["a", "b"])), "");
        assert_eq!(common(&[]), "");
    }
}
