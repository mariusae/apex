//! ⌘⇧P: a palette of commands to run in the window under the pointer
//! (where it was when the palette came up) -- the words in its tag, the
//! tools that apply to it (the B4 menu's), the commands run lately
//! anywhere in the session, and apex's own -- fuzzy-matched as typed.
//! Return runs the one chosen as B2 would run it there; with nothing
//! matching, what was typed.

use std::time::Instant;

use gpui::prelude::*;
use gpui::{deferred, div, px, AnyElement, Context, MouseButton};

use apex_core::*;

use crate::app::Acme;
use crate::field::{Edited, LineEdit};

/// Where a command in the palette comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Tag,
    Tool,
    Recent,
    Apex,
}

impl Origin {
    fn glyph(self) -> &'static str {
        match self {
            Origin::Tag => "›",
            Origin::Tool => "⚙",
            Origin::Recent => "↺",
            Origin::Apex => "▸",
        }
    }
    fn word(self) -> &'static str {
        match self {
            Origin::Tag => "in its tag",
            Origin::Tool => "tool",
            Origin::Recent => "run lately",
            Origin::Apex => "apex",
        }
    }
}

pub struct Commands {
    /// Where the command runs.
    pub ctx: ExecCtx,
    /// What that is called, for the field's hint.
    pub target: String,
    pub items: Vec<(String, Origin)>,
    pub filter: LineEdit,
    pub cursor: usize,
    pub caret_since: Instant,
}

/// How well `text` matches `q` as a subsequence, case aside: None when
/// it does not; higher for matches at the start, at word starts and in
/// runs.
pub fn score(text: &str, q: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut at = 0;
    let mut last: Option<usize> = None;
    for qc in q.to_lowercase().chars() {
        let i = (at..t.len()).find(|&i| t[i] == qc)?;
        score += 1;
        if i == 0 {
            score += 8;
        } else if !t[i - 1].is_alphanumeric() {
            score += 4;
        }
        if last == Some(i.wrapping_sub(1)) {
            score += 5;
        }
        last = Some(i);
        at = i + 1;
    }
    Some(score * 10 - t.len() as i32 / 4)
}

impl Commands {
    /// The commands matching what is typed, best first (ties in the
    /// order they were gathered: the tag's, tools, recent, apex's).
    pub fn matches(&self) -> Vec<(String, Origin)> {
        let q = self.filter.trim();
        let mut m: Vec<(i32, usize, &(String, Origin))> = self.items.iter().enumerate().filter_map(|(i, it)| score(&it.0, q).map(|s| (s, i, it))).collect();
        m.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        m.into_iter().take(12).map(|(_, _, it)| it.clone()).collect()
    }

    fn caret_on(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / 530) % 2 == 0
    }
}

impl Acme {
    /// ⌘⇧P: the palette, for the window under the pointer.
    pub fn open_commands(&mut self, cx: &mut Context<Self>) {
        if self.commands.is_some() {
            self.commands = None;
            cx.notify();
            return;
        }
        let w = self.command_target();
        let ctx = w.map(ExecCtx::Window).unwrap_or(ExecCtx::Top);
        let target = w.map(|w| self.node.window_name(w)).filter(|n| !n.is_empty()).unwrap_or_else(|| "the session".into());
        let mut items: Vec<(String, Origin)> = Vec::new();
        let add = |items: &mut Vec<(String, Origin)>, s: &str, f: Origin| {
            let s = s.trim();
            if !s.is_empty() && s != "|" && !items.iter().any(|(t, _)| t == s) {
                items.push((s.to_string(), f));
            }
        };
        if let Some(w) = w {
            // its tag's words, past its name
            if let Ok(win) = self.node.state.window(w) {
                if let Ok(tag) = self.node.state.buffer(win.tag) {
                    for word in tag.text.to_string().split_whitespace().skip(1) {
                        add(&mut items, word, Origin::Tag);
                    }
                }
            }
            for v in apex_core::plumb::verbs_for(&self.node.state.meta.rules, &self.node.window_name(w), self.node.window_kind(w), Some(w), self.node.window_owner(w)) {
                add(&mut items, &v, Origin::Tool);
            }
        }
        // what was run lately, anywhere: the newest first
        let mut recent: Vec<(Seq, String)> = self.node.state.windows.values().flat_map(|win| win.execs.iter().map(|(s, e)| (*s, e.text.clone()))).collect();
        recent.extend(self.node.state.layout.execs.iter().map(|(s, (_, e))| (*s, e.text.clone())));
        recent.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, t) in recent.into_iter().take(40) {
            add(&mut items, &t, Origin::Recent);
        }
        for b in apex_core::plumb::BUILTINS {
            add(&mut items, b, Origin::Apex);
        }
        self.commands = Some(Commands { ctx, target, items, filter: LineEdit::new(), cursor: 0, caret_since: Instant::now() });
        cx.notify();
    }

    /// The window under the pointer, else the one last selected in.
    fn command_target(&self) -> Option<WindowId> {
        let (x, y) = self.row_pt(self.last_mouse);
        let l = &self.node.state.layout;
        l.cols.iter().enumerate().filter(|(ci, _)| l.shows(*ci)).flat_map(|(_, c)| c.wins.iter()).find(|s| s.r.contains(x, y)).map(|s| s.window).or_else(|| self.node.seltext.and_then(|v| v.window()))
    }

    pub fn commands_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(c) = self.commands.as_mut() else { return };
        c.caret_since = Instant::now();
        match key {
            "escape" => self.commands = None,
            "up" => c.cursor = c.cursor.saturating_sub(1),
            "down" => c.cursor = (c.cursor + 1).min(c.matches().len().saturating_sub(1)),
            "enter" => {
                let m = c.matches();
                let text = m.get(c.cursor).map(|(t, _)| t.clone()).unwrap_or_else(|| c.filter.trim().to_string());
                self.run_command(text, cx);
                return;
            }
            _ => match c.filter.key(key, ch, mods) {
                Edited::Changed => c.cursor = 0,
                Edited::Moved => {}
                Edited::No => return,
            },
        }
        cx.notify();
    }

    fn run_command(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(c) = self.commands.take() else { return };
        if !text.is_empty() {
            self.execute(c.ctx, &text, cx);
            self.after();
        }
        cx.notify();
    }

    /// The palette, centred over the window as the picker is.
    pub fn commands_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = self.commands.as_ref()?;
        let hint = format!("Run a command in {}…", c.target);
        let field = crate::shell::palette_field(crate::field::field_view(&c.filter, c.caret_on(), &hint, true));
        let mut list = div().id("command-rows").flex().flex_col().px(px(6.)).pb(px(6.));
        let matches = c.matches();
        for (i, (text, from)) in matches.iter().enumerate() {
            let picked = i == c.cursor;
            let dim = crate::shell::palette_dim(picked);
            let t = text.clone();
            list = list.child(
                crate::shell::palette_row(picked)
                    .id(("command", i))
                    .cursor_default()
                    .child(div().flex_none().w(px(16.)).flex().justify_center().text_size(px(12.)).text_color(dim).child(from.glyph()))
                    .child(div().flex_none().max_w(px(360.)).truncate().child(text.clone()))
                    .child(div().flex_none().text_size(px(12.)).text_color(dim).child(from.word()))
                    .child(div().flex_1())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.run_command(t.clone(), cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        let panel = crate::shell::palette_panel().child(self.overlay_mark()).child(field).when(!matches.is_empty(), |d| d.child(list));
        Some(deferred(crate::shell::palette_place(panel)).with_priority(2).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn fuzzy_scores() {
        assert!(score("Putall", "pa").is_some());
        assert!(score("Put", "x").is_none());
        // the start of the name beats a match inside it
        assert!(score("Put", "p").unwrap() > score("Snarf|Up", "p").unwrap());
        // a run beats scattered letters
        assert!(score("Newcol", "new").unwrap() > score("N e w", "new").unwrap());
    }
}
