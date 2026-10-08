//! ⌘P: go to anything. Every open window of the session (files,
//! directories, terminals, win, errors) and the files closed lately
//! (the last fifty, each once, kept per session on this machine), ranked
//! the way Zed's file finder ranks: with nothing typed, what is open in
//! layout order, then the closed files by recency; with a query, a fuzzy
//! score over the whole path that likes the file name best, matches at
//! word starts and after `/` next, runs of consecutive matches, and pays
//! for gaps and for the wrong case; open windows before closed files at
//! equal scores. A path that matches nothing can still be opened as
//! typed. A window's label (its own, or Errors and Preview, as its tag
//! has it) is beside its name in its row, a chip, and is matched too, on
//! its own as a name; windows covered by another are listed after it.
//!
//! ⌘⇧P is the same across every tab of the window: the windows open in
//! each session the tabs hold (this one's and the parked ones', from
//! each one's replica), and each one's files closed lately; each row
//! says which tab it is in, this one marked so, and a pick in another
//! tab switches there first.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::{deferred, div, prelude::*, px, rgb, Context, MouseButton};

use apex_core::*;
use apex_server::providers::SessionUrl;

use crate::app::Acme;
use crate::shell::BLINK;

/// How many closed files a session remembers.
const KEEP: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The window's path (a file's, `dir/`, a terminal's directory, a URL).
    pub name: String,
    /// Its label, shown for it where it has one (a terminal's, a tool's
    /// pane's).
    pub label: Option<String>,
    /// The window, when one is open on it.
    pub window: Option<WindowId>,
    pub kind: WinKind,
    /// The tab it is in, across the tabs (⌘⇧P): the tab and its label;
    /// none for this session in the plain finder.
    pub tab: Option<(crate::pool::TabId, String)>,
}

impl Entry {
    /// `query`'s score against the entry: the best of its path, its label
    /// alone (a name of its own: `rust-analyzer`, `$EDITOR for git`), and
    /// the two together (a query that is some of each).
    fn score(&self, query: &str) -> Option<f64> {
        let path = score(query, &self.name);
        let Some(l) = self.label.as_deref().filter(|l| !l.is_empty()) else { return path };
        [path, score(query, l), score(query, &format!("{} {l}", self.name))].into_iter().flatten().reduce(f64::max)
    }

    /// The path as a row shows it: its last part (a folder's with its
    /// slash) and the folder it is in.
    fn split(&self) -> (String, String) {
        let trimmed = self.name.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(k) if k + 1 < trimmed.len() => (self.name[k + 1..].to_string(), self.name[..=k].to_string()),
            _ => (self.name.clone(), String::new()),
        }
    }
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
    /// Across every tab (⌘⇧P), not this session alone.
    pub all: bool,
    /// This window's session, whose windows come first at equal scores.
    here: Option<crate::pool::TabId>,
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
            let mut scored: Vec<(f64, usize, &Entry)> = self.entries.iter().enumerate().filter_map(|(i, e)| e.score(q).map(|s| (s, i, e))).collect();
            // best first; open before closed; this tab before others; then
            // the order we had
            let here = |e: &Entry| e.tab.as_ref().map(|(id, _)| Some(*id) == self.here).unwrap_or(true);
            scored.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.2.window.is_some().cmp(&a.2.window.is_some()))
                    .then(here(b.2).cmp(&here(a.2)))
                    .then(a.1.cmp(&b.1))
            });
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
    // the core's, which ⌘O's listing on the host uses too
    apex_core::fuzzy::score(query, path)
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

/// A session's entries: its open windows in layout order, then its files
/// closed lately that are not open now; each said to be in `tab` when
/// given (the label it goes by in the strip).
fn session_entries(node: &Node, url: &SessionUrl, id: crate::pool::TabId, label: Option<String>) -> Vec<Entry> {
    let tab = label.map(|l| (id, l));
    let mut out = Vec::new();
    let l = &node.state.layout;
    // stashed ones as well, where they stood (those whose column is gone
    // after): going to one brings it back
    let order = (0..l.cols.len()).flat_map(|ci| apex_core::tiling::stash_order(l, ci)).map(|(w, _)| w);
    let orphans = l.stash.iter().filter(|s| l.column(s.col).is_none()).map(|s| s.slot.window);
    // each with the windows it covers after it (`Cover`): going to one
    // brings it up
    for w in order.chain(orphans).flat_map(|w| l.stack(w)) {
        let (name, kind) = (node.window_path(w), node.window_kind(w));
        // the label its tag shows: its own, or what its kind says
        let label = node.window_label(w).or_else(|| match kind {
            WinKind::Errors => Some("Errors".into()),
            WinKind::Page if node.is_buffer_page(w) => Some("Preview".into()),
            _ => None,
        });
        if name.is_empty() && label.is_none() {
            continue;
        }
        out.push(Entry { name, label, window: Some(w), kind, tab: tab.clone() });
    }
    for p in recently_closed(url) {
        if !out.iter().any(|e| e.name == p && e.kind == WinKind::File) {
            out.push(Entry { name: p, label: None, window: None, kind: WinKind::File, tab: tab.clone() });
        }
    }
    out
}

/// A tab's name as the strip gives it: the session's label (this
/// window's as it knows it, `here`), and the host for one elsewhere.
fn tab_label(u: &SessionUrl, here: Option<&str>) -> String {
    let name = here.filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| u.session.clone());
    if u.is_local() { name } else { format!("{name} @ {}", u.arg) }
}

impl Acme {
    /// The entries as things stand: open windows in layout order, then
    /// the closed files not open now; across every tab, each tab's in the
    /// tabs' order, all of them said to be in it.
    fn finder_entries(&self, all: bool, cx: &Context<Self>) -> Vec<Entry> {
        if !all {
            return session_entries(&self.node, &self.url, self.tab, None);
        }
        let mut out = Vec::new();
        let parked = crate::pool::Pool::parked_nodes(cx);
        for t in crate::pool::Pool::tabs(cx) {
            let label = tab_label(&t.url, (t.id == self.tab).then_some(self.session.as_str()));
            if t.id == self.tab {
                out.extend(session_entries(&self.node, &t.url, t.id, Some(label)));
            } else if let Some((_, _, node)) = parked.iter().find(|(id, _, _)| *id == t.id) {
                out.extend(session_entries(node, &t.url, t.id, Some(label)));
            }
        }
        out
    }

    pub fn open_finder(&mut self, all: bool, cx: &mut Context<Self>) {
        self.selector = None;
        let entries = self.finder_entries(all, cx);
        self.finder = Some(Finder { filter: crate::field::LineEdit::new(), cursor: 0, entries, caret_since: std::time::Instant::now(), all, here: Some(self.tab) });
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
    pub fn finder_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(f) = self.finder.as_mut() else { return };
        f.caret_since = std::time::Instant::now();
        match key {
            "escape" => self.close_finder(cx),
            "enter" => {
                let pick = f.picks().get(f.cursor).cloned();
                if let Some(p) = pick {
                    self.pick(p, window, cx);
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
    /// column, which warps to the new window. In another tab: that tab
    /// first, and there once it is shown.
    pub fn pick(&mut self, p: Pick, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.finder = None;
        let Pick::Entry(Entry { name, tab, window: open, .. }) = p;
        // an open window by its id: its path may be another's too (a
        // file's and its preview's)
        let name = open.map(|w| w.0.to_string()).unwrap_or(name);
        let loc = Loc { session: None, name, pos: Pos::Keep };
        if let Some((id, _)) = tab.filter(|(id, _)| *id != self.tab) {
            self.switch_to(id, window, cx);
            // a parked tab is here at once, and the pick is made in it as
            // in any; one still attaching lands there once its window is
            if !self.connected || self.node.state.meta.id.is_empty() {
                self.pending_goto = Some(loc);
                self.sync();
                self.after();
                cx.notify();
                return;
            }
        }
        // a jump: the origin goes on the back stack, and we land there
        let _ = apex_server::proposal::apply(&mut self.node, &mut self.log, apex_server::Proposal::Goto { loc });
        self.sync();
        self.after();
        cx.notify();
    }

    /// Windows that went since the last look: a file's goes on the list.
    /// Only within one session: when the window has switched to another,
    /// the windows of the one left are not gone, and not this one's.
    pub fn track_closed(&mut self) {
        // a file's window, not a scratch one: the files to go back to
        let now: BTreeMap<WindowId, String> = self
            .node
            .state
            .windows
            .keys()
            .filter(|w| self.node.window_kind(**w) == WinKind::File && !self.node.window_scratch(**w))
            .map(|w| (*w, self.node.window_path(*w)))
            .collect();
        if self.last_windows_of.as_ref() != Some(&self.url) {
            self.last_windows = now;
            self.last_windows_of = Some(self.url.clone());
            return;
        }
        for (w, name) in &self.last_windows {
            if !now.contains_key(w) && name.starts_with('/') && !name.ends_with('/') && self.node.state.windows.get(w).is_none() {
                note_closed(&self.url, name);
            }
        }
        self.last_windows = now;
    }

    /// The finder, when open: Zed's file finder in acme's colours.
    pub fn finder_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let f = self.finder.as_ref()?;
        let picks = f.picks();
        let t = crate::theme::theme();
        let field = crate::shell::palette_field(crate::field::field_view(&f.filter, f.caret_visible(), if f.all { "Go to a window in any tab, or a file closed lately…" } else { "Go to a window, or a file closed lately…" }, true));
        let mut list = div().id("finder-list").flex().flex_col().px(px(6.)).pb(px(6.)).max_h(px(10. * 34.)).overflow_y_scroll();
        for (i, pick) in picks.iter().enumerate().take(24) {
            let picked = i == f.cursor;
            let dim = crate::shell::palette_dim(picked, crate::shell::Act::Look);
            let Pick::Entry(e) = pick;
            // the name, its label beside it as the tag has it, the folder
            let (name, dir) = e.split();
            let open = e.window.is_some();
            let mark = match (open, e.kind) {
                (true, WinKind::Term) => "▶",
                (true, _) => "●",
                (false, _) => "○",
            };
            let p = pick.clone();
            let row = crate::shell::palette_row(picked, crate::shell::Act::Look)
                .id(("goto", i))
                .cursor_default()
                .child(div().flex_none().w(px(16.)).flex().justify_center().text_size(px(11.)).text_color(dim).child(mark))
                // one line, whatever the lengths: the name and the badges
                // keep theirs, the directory gives way and is cut short
                .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().when(!open && !picked, |d| d.text_color(dim)).child(name))
                .when_some(e.label.clone().filter(|l| !l.is_empty()), |d, l| {
                    d.child(
                        div()
                            .flex_none()
                            .max_w(px(220.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .px(px(6.))
                            .rounded(px(4.))
                            .text_size(px(11.5))
                            .bg(rgb(crate::theme::step(t.panel_bg, 1)))
                            .text_color(rgb(t.panel_text))
                            .child(l),
                    )
                })
                .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().text_size(px(12.)).text_color(dim).child(dir))
                .when(!open, |d| d.child(div().flex_none().whitespace_nowrap().text_size(px(11.)).text_color(dim).child("closed")))
                // the tab it is in: this one's marked so, the others named
                .when_some(e.tab.clone(), |d, (id, label)| {
                    let here = id == self.tab;
                    let badge = div()
                        .flex_none()
                        .max_w(px(200.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .px(px(6.))
                        .rounded(px(4.))
                        .border_1()
                        .text_size(px(11.))
                        .whitespace_nowrap()
                        .border_color(if here { rgb(t.panel_accent) } else { rgb(t.panel_border) })
                        .text_color(if here { rgb(t.panel_accent).into() } else { dim })
                        .child(if here { format!("{label} · here") } else { label });
                    d.child(badge)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.pick(p.clone(), window, cx);
                        cx.stop_propagation();
                    }),
                );
            list = list.child(row);
        }
        if picks.is_empty() {
            list = list.child(div().px(px(10.)).py(px(8.)).text_size(px(13.)).font_family(crate::fonts::ui()).text_color(rgb(t.panel_dim)).child("Nothing matches"));
        }
        let panel = crate::shell::palette_panel()
            .child(self.overlay_mark())
            .child(field)
            .child(list)
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()));
        // Manifold's palette, as the picker is
        Some(deferred(crate::shell::palette_place(panel)).with_priority(1))
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
                Entry { name: "/x/closed.rs".into(), label: None, window: None, kind: WinKind::File, tab: None },
                Entry { name: "/x/open.rs".into(), label: None, window: Some(WindowId(1)), kind: WinKind::File, tab: None },
            ],
            caret_since: std::time::Instant::now(),
            all: false,
            here: None,
        };
        let picks = f.picks();
        assert!(matches!(&picks[0], Pick::Entry(e) if e.name == "/x/open.rs"), "{picks:?}");
        assert_eq!(picks.len(), 2);
    }

    #[test]
    fn a_window_is_found_by_its_label_and_shows_its_name_and_folder() {
        let e = |name: &str, label: Option<&str>, w: u64| Entry { name: name.into(), label: label.map(String::from), window: Some(WindowId(w)), kind: WinKind::File, tab: None };
        let f = Finder {
            filter: "analyzer".into(),
            cursor: 0,
            entries: vec![e("/src/apex/notes.md", None, 1), e("/src/apex/", Some("rust-analyzer"), 2), e("/src/apex/a/x.rs", None, 3)],
            caret_since: std::time::Instant::now(),
            all: false,
            here: None,
        };
        let picks = f.picks();
        assert_eq!(picks.len(), 1, "{picks:?}");
        assert!(matches!(&picks[0], Pick::Entry(e) if e.window == Some(WindowId(2))));
        // a folder's last part with its slash, and the folder it is in
        assert_eq!(e("/src/apex/", None, 1).split(), ("apex/".to_string(), "/src/".to_string()));
        assert_eq!(e("/src/apex/notes.md", None, 1).split(), ("notes.md".to_string(), "/src/apex/".to_string()));
        assert_eq!(e("/", None, 1).split(), ("/".to_string(), String::new()));
    }

    #[test]
    fn across_the_tabs_this_tab_comes_first_at_equal_scores() {
        let here = SessionUrl::local("main").with_id("aaaa");
        let there = SessionUrl::local("work").with_id("bbbb");
        let (t_here, t_there) = (crate::pool::TabId(1), crate::pool::TabId(2));
        let e = |name: &str, w: u64, id: crate::pool::TabId, l: &str| Entry { name: name.into(), label: None, window: Some(WindowId(w)), kind: WinKind::File, tab: Some((id, l.into())) };
        let f = Finder {
            filter: "lib".into(),
            cursor: 0,
            // the other tab's listed first, as the strip might have it
            entries: vec![e("/w/src/lib.rs", 1, t_there, "work"), e("/m/src/lib.rs", 2, t_here, "main")],
            caret_since: std::time::Instant::now(),
            all: true,
            here: Some(t_here),
        };
        let picks = f.picks();
        assert!(matches!(&picks[0], Pick::Entry(e) if e.tab.as_ref().is_some_and(|(id, _)| *id == t_here)), "{picks:?}");
        assert_eq!(picks.len(), 2);
        // a remote tab is named with its host
        assert_eq!(super::tab_label(&here, Some("main")), "main");
        assert_eq!(super::tab_label(&there, None), "work");
    }
}
