//! ⌘P: go to anything. Every open window of the session (files,
//! directories, terminals, win, +Errors) and the files closed lately
//! (the last fifty, each once, kept per session on this machine), ranked
//! the way Zed's file finder ranks: with nothing typed, what is open in
//! layout order, then the closed files by recency; with a query, a fuzzy
//! score over the whole path that likes the file name best, matches at
//! word starts and after `/` next, runs of consecutive matches, and pays
//! for gaps and for the wrong case; open windows before closed files at
//! equal scores. A path that matches nothing can still be opened as
//! typed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::{anchored, deferred, div, point, prelude::*, px, rgb, Context, MouseButton};

use apex_core::*;
use apex_server::providers::SessionUrl;

use crate::app::Acme;
use crate::shell::{BLINK, UI_FONT};

/// How many closed files a session remembers.
const KEEP: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The window's name (a file's path, `dir/`, `dir/-host`, `dir/+Errors`).
    pub name: String,
    /// The window, when one is open on it.
    pub window: Option<WindowId>,
    pub kind: WinKind,
}

/// A choice: an entry.
#[derive(Clone, Debug)]
pub enum Pick {
    Entry(Entry),
}

pub struct Finder {
    pub filter: crate::field::LineEdit,
    pub cursor: usize,
    /// Open windows in layout order, then closed files by recency.
    entries: Vec<Entry>,
    pub caret_since: std::time::Instant,
}

impl Finder {
    pub fn caret_visible(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / BLINK.as_millis()) % 2 == 0
    }

    /// What the list shows for the query, ranked. With nothing typed,
    /// the open windows alone: the files closed lately are there to be
    /// found by name, not to be scrolled through.
    pub fn picks(&self) -> Vec<Pick> {
        let q = self.filter.trim();
        if q.is_empty() {
            self.entries.iter().filter(|e| e.window.is_some()).cloned().map(Pick::Entry).collect()
        } else {
            let mut scored: Vec<(f64, usize, &Entry)> = self.entries.iter().enumerate().filter_map(|(i, e)| score(q, &e.name).map(|s| (s, i, e))).collect();
            // best first; open before closed; then the order we had
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.2.window.is_some().cmp(&a.2.window.is_some())).then(a.1.cmp(&b.1)));
            scored.into_iter().map(|(_, _, e)| Pick::Entry(e.clone())).collect()
        }
    }

    pub fn move_cursor(&mut self, by: i32) {
        let n = self.picks().len();
        if n == 0 {
            return;
        }
        self.cursor = (self.cursor as i32 + by).rem_euclid(n as i32) as usize;
    }
}

/// A fuzzy score for `query` against `path`, in Zed's spirit: each query
/// character must appear in order; a character scores 1.0 when it starts
/// the file name, 0.9 right after `/`, 0.8 at a word start (after `-`,
/// `_`, `.`, a digit, or a case change), 1.0 when it continues the
/// previous match, 0.55 otherwise; a case mismatch costs half. The best
/// alignment is taken, and the score is the mean per query character,
/// nudged up for matches inside the file name.
pub fn score(query: &str, path: &str) -> Option<f64> {
    let q: Vec<char> = query.chars().collect();
    let p: Vec<char> = path.chars().collect();
    if q.is_empty() || q.len() > p.len() {
        return None;
    }
    let name_at = path.rfind('/').map(|i| path[..i].chars().count() + 1).unwrap_or(0);
    // memo over (query index, path index): the best score from there
    let mut memo: BTreeMap<(usize, usize, bool), Option<f64>> = BTreeMap::new();
    fn best(q: &[char], p: &[char], qi: usize, pi: usize, prev_matched: bool, name_at: usize, memo: &mut BTreeMap<(usize, usize, bool), Option<f64>>) -> Option<f64> {
        if qi == q.len() {
            return Some(0.0);
        }
        if let Some(m) = memo.get(&(qi, pi, prev_matched)) {
            return *m;
        }
        let mut out: Option<f64> = None;
        let remaining = q.len() - qi;
        for j in pi..=p.len().saturating_sub(remaining) {
            let pc = p[j];
            let qc = q[qi];
            if pc.to_lowercase().ne(qc.to_lowercase()) {
                continue;
            }
            let mut s = if j == name_at {
                1.0
            } else if j > 0 && p[j - 1] == '/' {
                0.9
            } else if prev_matched && j == pi {
                1.0
            } else if j > 0 && (matches!(p[j - 1], '-' | '_' | '.' | ' ') || p[j - 1].is_numeric() || (p[j - 1].is_lowercase() && pc.is_uppercase())) {
                0.8
            } else {
                0.55
            };
            if pc != qc && !(pc.is_lowercase() == qc.is_lowercase()) {
                s *= 0.5;
            }
            if j >= name_at {
                s += 0.15; // in the file name
            }
            if let Some(rest) = best(q, p, qi + 1, j + 1, true, name_at, memo) {
                let total = s + rest;
                if out.is_none_or(|o| total > o) {
                    out = Some(total);
                }
            }
            // a gap: the next tries are not consecutive
            if qi < q.len() && !(prev_matched && j == pi) {
                // nothing: the loop itself explores later positions
            }
        }
        memo.insert((qi, pi, prev_matched), out);
        out
    }
    let total = best(&q, &p, 0, 0, false, name_at, &mut memo)?;
    // per query character, and a little less for long paths, as Zed does
    Some(total / q.len() as f64 - (p.len() as f64) * 0.0005)
}

// ---- the closed files, per session, on this machine ------------------------

fn closed_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(home).join("Library/Application Support/apex/recent-files")
}

/// The files closed lately in `url`'s session, latest first.
pub fn recently_closed(url: &SessionUrl) -> Vec<String> {
    let key = url.to_string();
    std::fs::read_to_string(closed_file())
        .map(|s| s.lines().filter_map(|l| l.split_once('\t')).filter(|(u, _)| *u == key).map(|(_, p)| p.to_string()).collect())
        .unwrap_or_default()
}

/// A file window closed: remember it, first, once.
pub fn note_closed(url: &SessionUrl, path: &str) {
    let key = url.to_string();
    let all: Vec<(String, String)> = std::fs::read_to_string(closed_file())
        .map(|s| s.lines().filter_map(|l| l.split_once('\t').map(|(u, p)| (u.to_string(), p.to_string()))).collect())
        .unwrap_or_default();
    let mut ours: Vec<String> = all.iter().filter(|(u, _)| *u == key).map(|(_, p)| p.clone()).collect();
    ours.retain(|p| p != path);
    ours.insert(0, path.to_string());
    ours.truncate(KEEP);
    let others = all.iter().filter(|(u, _)| *u != key);
    let text: String = ours.iter().map(|p| format!("{key}\t{p}\n")).chain(others.map(|(u, p)| format!("{u}\t{p}\n"))).collect();
    if let Some(d) = closed_file().parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(closed_file(), text);
}

// ---- the app side -------------------------------------------------------------

impl Acme {
    /// The entries as things stand: open windows in layout order, then
    /// the closed files not open now.
    fn finder_entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for col in &self.node.state.layout.cols {
            for slot in &col.wins {
                let w = slot.window;
                let name = self.node.window_name(w);
                if name.is_empty() {
                    continue;
                }
                out.push(Entry { name, window: Some(w), kind: self.node.window_kind(w) });
            }
        }
        for p in recently_closed(&self.url) {
            if !out.iter().any(|e| e.name == p) {
                out.push(Entry { name: p, window: None, kind: WinKind::File });
            }
        }
        out
    }

    pub fn open_finder(&mut self, cx: &mut Context<Self>) {
        self.selector = None;
        let entries = self.finder_entries();
        self.finder = Some(Finder { filter: crate::field::LineEdit::new(), cursor: 0, entries, caret_since: std::time::Instant::now() });
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BLINK).await;
            let open = cx
                .update(|cx| {
                    this.update(cx, |acme, cx| {
                        let open = acme.finder.is_some();
                        if open {
                            cx.notify();
                        }
                        open
                    })
                    .unwrap_or(false)
                });
            if !open {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    pub fn close_finder(&mut self, cx: &mut Context<Self>) {
        self.finder = None;
        cx.notify();
    }

    /// Keys while the finder is open.
    pub fn finder_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(f) = self.finder.as_mut() else { return };
        f.caret_since = std::time::Instant::now();
        match key {
            "escape" => self.close_finder(cx),
            "enter" => {
                let pick = f.picks().get(f.cursor).cloned();
                if let Some(p) = pick {
                    self.pick(p, cx);
                }
            }
            "up" => {
                f.move_cursor(-1);
                cx.notify();
            }
            "down" => {
                f.move_cursor(1);
                cx.notify();
            }
            _ => match f.filter.key(key, ch, mods) {
                crate::field::Edited::Changed => {
                    f.cursor = 0;
                    cx.notify();
                }
                crate::field::Edited::Moved => cx.notify(),
                crate::field::Edited::No => {}
            },
        }
    }

    /// Go there: an open window is shown and the pointer warped to it (as
    /// acme warps to what it opens); a closed file is opened in the first
    /// column, which warps to the new window.
    pub fn pick(&mut self, p: Pick, cx: &mut Context<Self>) {
        self.finder = None;
        // a jump: the origin goes on the back stack, and we land there
        let Pick::Entry(Entry { name, .. }) = p;
        let loc = Loc { name, pos: Pos::Keep };
        let _ = apex_server::proposal::apply(&mut self.node, &mut self.log, apex_server::Proposal::Goto { loc });
        self.sync();
        self.after();
        cx.notify();
    }

    /// Windows that went since the last look: a file's goes on the list.
    pub fn track_closed(&mut self) {
        let now: BTreeMap<WindowId, String> = self.node.state.windows.keys().map(|w| (*w, self.node.window_name(*w))).collect();
        for (w, name) in &self.last_windows {
            if !now.contains_key(w) && name.starts_with('/') && !name.ends_with('/') && !name.contains("/+") && !name.rsplit('/').next().is_some_and(|n| n.starts_with('-')) {
                note_closed(&self.url, name);
            }
        }
        self.last_windows = now;
    }

    /// The finder, when open: Zed's file finder in acme's colours.
    pub fn finder_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let f = self.finder.as_ref()?;
        let picks = f.picks();
        let field = div().px(px(14.)).py(px(10.)).border_b_1().border_color(rgb(0xdddddd)).text_size(px(14.)).font_family(UI_FONT).child(crate::field::field_view(&f.filter, f.caret_visible(), "Go to a window, or a file closed lately…", true));
        let mut list = div().flex().flex_col().py(px(6.)).px(px(6.));
        for (i, pick) in picks.iter().enumerate().take(24) {
            let picked = i == f.cursor;
            let Pick::Entry(e) = pick;
            let (dir, name) = match e.name.rfind('/') {
                Some(k) if k + 1 < e.name.len() => (e.name[..=k].to_string(), e.name[k + 1..].to_string()),
                _ => (String::new(), e.name.clone()),
            };
            let open = e.window.is_some();
            let (mark, mark_color) = match (open, e.kind) {
                (true, WinKind::Term) => ("▶", 0x990099),
                (true, _) => ("●", 0x000099),
                (false, _) => ("○", 0x8a8a8a),
            };
            let p = pick.clone();
            let row = div()
                .id(("goto", i))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(14.))
                .font_family(UI_FONT)
                .cursor_pointer()
                .when(picked, |d| d.bg(rgb(0x9eeeee)))
                .when(!picked, |d| d.hover(|s| s.bg(rgb(0xe4e4e4))))
                .child(div().w(px(14.)).text_color(rgb(mark_color)).child(mark))
                .child(div().text_color(rgb(if open { 0x111111 } else { 0x555555 })).child(name))
                .child(div().flex_1().text_size(px(12.)).text_color(rgb(0x8a8a8a)).overflow_hidden().child(dir))
                .when(!open, |d| d.child(div().text_size(px(11.)).text_color(rgb(0x8a8a8a)).child("closed")))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.pick(p.clone(), cx);
                        cx.stop_propagation();
                    }),
                );
            list = list.child(row);
        }
        if picks.is_empty() {
            list = list.child(div().px(px(10.)).py(px(8.)).text_size(px(13.)).font_family(UI_FONT).text_color(rgb(0x8a8a8a)).child("Nothing matches"));
        }
        let panel = div()
            .w(px(620.))
            .bg(rgb(0xf4f4f4))
            .border_1()
            .border_color(rgb(0xc8c8c8))
            .rounded(px(8.))
            .shadow_lg()
            .flex()
            .flex_col()
            .child(field)
            .child(list)
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()));
        Some(deferred(anchored().position(point(px(72.), px(self.top() + 6.))).child(panel)).with_priority(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_name_beats_the_directory_and_open_beats_closed() {
        let a = score("main", "/src/apex/crates/apex-client/src/main.rs").unwrap();
        let b = score("main", "/src/main-things/other/file.rs").unwrap();
        assert!(a > b, "{a} vs {b}");
        assert!(score("zzz", "/src/main.rs").is_none());
        // consecutive letters in the name beat scattered ones
        let c = score("app", "/x/app.rs").unwrap();
        let d = score("app", "/a/p/p.rs").unwrap();
        assert!(c > d, "{c} vs {d}");
        let f = Finder {
            filter: "rs".into(),
            cursor: 0,
            entries: vec![
                Entry { name: "/x/closed.rs".into(), window: None, kind: WinKind::File },
                Entry { name: "/x/open.rs".into(), window: Some(WindowId(1)), kind: WinKind::File },
            ],
            caret_since: std::time::Instant::now(),
        };
        let picks = f.picks();
        assert!(matches!(&picks[0], Pick::Entry(e) if e.name == "/x/open.rs"), "{picks:?}");
        assert_eq!(picks.len(), 2);
    }
}
