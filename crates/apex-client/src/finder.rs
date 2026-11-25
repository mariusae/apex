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
//! its own as a name; windows under a lens are listed after it.
//!
//! Sections too: each open window's outline (`apex_server::outline`, as
//! its scrollbar shows it -- a file's symbols, a terminal's commands, a
//! page's headings or an agent's turns). With nothing typed, the window
//! the keys are in has its outline under it, and the list starts on the
//! entry it is showing, so ↑↩ goes to the section before and ↓↩ to the
//! one after; with a query, every window's entries are matched with the
//! windows (by their titles, or the window's name and the title). Picked,
//! the window is shown at the entry.
//!
//! The rows are ranked once a keystroke, not a frame, and only those in
//! view are made.
//!
//! ⌘⇧P is the same across every tab of the window: the windows open in
//! each session the tabs hold (this one's and the parked ones', from
//! each one's replica), and each one's files closed lately; each row
//! says which tab it is in, this one marked so, and a pick in another
//! tab switches there first.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

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

/// One of an open window's outline's entries: a section to go to.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub window: WindowId,
    pub level: u8,
    pub title: String,
    /// A command's when and how long.
    pub detail: Option<String>,
    pub failed: bool,
    pub at: apex_server::outline::At,
    /// The window, as a row names it: its file's last part, or its label.
    pub file: String,
}

/// A choice: a window or a file, or a section of a window.
#[derive(Clone, Debug)]
pub enum Pick {
    Entry(Entry),
    Section(Rc<Section>),
}

/// The rows shown at once.
pub const ROWS: usize = 10;

pub struct Finder {
    pub filter: crate::field::LineEdit,
    pub cursor: usize,
    /// The first row in view.
    pub top: usize,
    /// Open windows in layout order, then closed files by recency.
    entries: Vec<Entry>,
    /// The open windows' outlines' entries, a window's together, the
    /// windows in layout order.
    sections: Vec<Rc<Section>>,
    /// The window the keys were in, and the one of its sections it was
    /// showing (into `sections`).
    within: Option<WindowId>,
    current: Option<usize>,
    /// What the list shows, ranked for the query as it is.
    picked: Vec<Pick>,
    pub caret_since: std::time::Instant,
    /// Across every tab (⌘⇧P), not this session alone.
    pub all: bool,
    /// This window's session, whose windows come first at equal scores.
    here: Option<crate::pool::TabId>,
    /// The wheel's turn not yet a whole row.
    pub wheel: f32,
}

impl Finder {
    pub fn new(entries: Vec<Entry>, sections: Vec<Rc<Section>>, within: Option<WindowId>, current: Option<usize>, all: bool, here: Option<crate::pool::TabId>) -> Finder {
        let mut f = Finder { filter: crate::field::LineEdit::new(), cursor: 0, top: 0, entries, sections, within, current, picked: Vec::new(), caret_since: std::time::Instant::now(), all, here, wheel: 0. };
        f.changed();
        f
    }

    pub fn caret_visible(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / BLINK.as_millis()) % 2 == 0
    }

    /// What the list shows for the query, ranked (`changed`).
    pub fn picks(&self) -> &[Pick] {
        &self.picked
    }

    /// The query changed (or the finder opened): the list ranked anew,
    /// the cursor at its start -- with nothing typed, on the section the
    /// window the keys were in is showing, the rows before it in view.
    pub fn changed(&mut self) {
        self.picked = self.rank();
        self.cursor = 0;
        self.top = 0;
        if self.filter.trim().is_empty() {
            let at = self.current.and_then(|c| self.picked.iter().position(|p| matches!(p, Pick::Section(s) if Rc::ptr_eq(s, &self.sections[c]))));
            let at = at.or_else(|| self.picked.iter().position(|p| matches!(p, Pick::Entry(e) if e.window.is_some() && e.window == self.within)));
            if let Some(i) = at {
                self.cursor = i;
                self.top = i.saturating_sub(ROWS / 2);
            }
        }
        self.clamp_top();
    }

    /// With nothing typed, the open windows, the one the keys were in
    /// with its sections under it: the files closed lately are there to
    /// be found by name, and other windows' sections by title, not to be
    /// scrolled through. With a query, the windows, files and sections
    /// it matches, best first.
    fn rank(&self) -> Vec<Pick> {
        let q = self.filter.trim();
        if q.is_empty() {
            let mut out = Vec::new();
            for e in self.entries.iter().filter(|e| e.window.is_some()) {
                out.push(Pick::Entry(e.clone()));
                if e.window == self.within && e.tab.as_ref().is_none_or(|(id, _)| Some(*id) == self.here) {
                    out.extend(self.sections.iter().filter(|s| Some(s.window) == self.within).cloned().map(Pick::Section));
                }
            }
            return out;
        }
        let query = apex_core::fuzzy::Query::new(q);
        let mut scorer = apex_core::fuzzy::Scorer::default();
        // (score, a window's or file's, this tab's, open, order, pick)
        let mut scored: Vec<(f64, bool, bool, bool, usize, Pick)> = Vec::new();
        let here = |e: &Entry| e.tab.as_ref().map(|(id, _)| Some(*id) == self.here).unwrap_or(true);
        for (i, e) in self.entries.iter().enumerate() {
            if let Some(s) = e.score(q) {
                scored.push((s, true, here(e), e.window.is_some(), i, Pick::Entry(e.clone())));
            }
        }
        let mut both = String::new();
        for (i, s) in self.sections.iter().enumerate() {
            // the title, or the window's name and the title ("lib parse")
            let title = scorer.score(&query, &s.title);
            both.clear();
            both.push_str(&s.file);
            both.push(' ');
            both.push_str(&s.title);
            let with_file = scorer.score(&query, &both);
            if let Some(v) = [title, with_file].into_iter().flatten().reduce(f64::max) {
                scored.push((v, false, true, true, self.entries.len() + i, Pick::Section(s.clone())));
            }
        }
        // best first; windows and files before sections, open before
        // closed, this tab before others, at equal scores; then the order
        // they were in
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.cmp(&a.1))
                .then(b.3.cmp(&a.3))
                .then(b.2.cmp(&a.2))
                .then(a.4.cmp(&b.4))
        });
        scored.into_iter().map(|x| x.5).collect()
    }

    pub fn move_cursor(&mut self, by: i32) {
        let n = self.picked.len();
        if n == 0 {
            return;
        }
        self.cursor = (self.cursor as i32 + by).rem_euclid(n as i32) as usize;
        // the cursor kept in view
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + ROWS {
            self.top = self.cursor + 1 - ROWS;
        }
    }

    /// The rows in view moved by `by` (the wheel), the cursor where it is.
    pub fn scroll(&mut self, by: i32) {
        self.top = (self.top as i64 + by as i64).max(0) as usize;
        self.clamp_top();
    }

    fn clamp_top(&mut self) {
        self.top = self.top.min(self.picked.len().saturating_sub(ROWS));
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
    // each with the windows under it after it (`Lens`): going to one
    // shows their place
    for w in order.chain(orphans).flat_map(|w| l.chain(w)) {
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
        // this session's open windows' sections, in the windows' order,
        // and which the window the keys are in is showing
        let within = self.key_window().or(self.win_under_pointer);
        let mut sections = Vec::new();
        let mut current = None;
        let tab = self.tab;
        for e in entries.iter().filter(|e| e.tab.as_ref().is_none_or(|(id, _)| *id == tab)) {
            let Some(w) = e.window else { continue };
            // one under a lens is not where its sections would show
            if self.node.state.layout.lens_of(w).is_some() {
                continue;
            }
            let (name, _) = e.split();
            let file = e.label.clone().filter(|l| !l.is_empty() && name.is_empty()).unwrap_or(name);
            let Some((heads, now_at)) = self.sections_of(w) else { continue };
            if Some(w) == within {
                current = now_at.map(|i| sections.len() + i);
            }
            sections.extend(heads.into_iter().map(|(level, title, detail, failed, at)| Rc::new(Section { window: w, level, title, detail, failed, at, file: file.clone() })));
        }
        self.finder = Some(Finder::new(entries, sections, within, current, all, Some(self.tab)));
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
            "pageup" => {
                f.move_cursor(-(ROWS as i32));
                cx.notify();
            }
            "pagedown" => {
                f.move_cursor(ROWS as i32);
                cx.notify();
            }
            _ => match f.filter.key(key, ch, mods) {
                crate::field::Edited::Changed => {
                    f.changed();
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
        let Entry { name, tab, window: open, .. } = match p {
            Pick::Entry(e) => e,
            Pick::Section(s) => return self.go_to_section(&s, cx),
        };
        // an open window by its id: its path may be another's too (a
        // file's and its preview's)
        let name = open.map(|w| w.0.to_string()).unwrap_or(name);
        let loc = Loc { session: None, name, pos: Pos::Keep };
        self.go_in_tab(tab.map(|(id, _)| id), loc, window, cx);
    }

    /// Go to `loc` in tab `tab` (none, this one): that tab first, when it
    /// is another, and there once it is shown.
    pub fn go_in_tab(&mut self, tab: Option<crate::pool::TabId>, loc: Loc, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if let Some(id) = tab.filter(|id| *id != self.tab) {
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

    /// Window `w`'s outline, as the finder lists it -- (level, title,
    /// detail, failed, where) an entry -- and which entry it is showing.
    /// A page shown has its outline from the page itself (and so which
    /// entry it is at); one not shown, parsed from its HTML, once a
    /// version (`page_outlines`); anything else as `apex_server::outline`
    /// says.
    #[allow(clippy::type_complexity)]
    fn sections_of(&mut self, w: WindowId) -> Option<(Vec<(u8, String, Option<String>, bool, apex_server::outline::At)>, Option<usize>)> {
        use apex_server::outline::At;
        if let Some((toc, current)) = self.webs.toc(w) {
            let heads = toc.heads.iter().enumerate().map(|(i, h)| (h.level, h.title.clone(), h.detail.clone(), h.failed, At::Page(i as u32))).collect();
            return Some((heads, current));
        }
        let win = self.node.state.window(w).ok()?;
        if let Body::Page(src) = win.body {
            let Source::Buffer(b) = src else { return None };
            let buf = self.node.state.buffer(b).ok()?;
            let parsed = match self.page_outlines.get(&b) {
                Some((v, o)) if *v == buf.version => o.clone(),
                _ => {
                    let o = Rc::new(apex_server::outline::page(&buf.text.to_string()));
                    self.page_outlines.insert(b, (buf.version, o.clone()));
                    o
                }
            };
            let heads: Vec<_> = parsed.iter().enumerate().map(|(i, (l, t))| (*l, t.clone(), None, false, At::Page(i as u32))).collect();
            return (!heads.is_empty()).then_some((heads, None));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let entries = apex_server::outline::of(&self.node, w, now)?;
        let current = apex_server::outline::current(&self.node, w, &entries);
        Some((entries.into_iter().map(|e| (e.level, e.title, e.detail, e.failed, e.at)).collect(), current))
    }

    /// A section picked: its window shown (a jump, as any pick is), and
    /// shown at the section -- a text with its line at the top and dot
    /// there, a terminal scrolled to the command, a page to the entry.
    fn go_to_section(&mut self, s: &Section, cx: &mut Context<Self>) {
        use apex_server::outline::At;
        let loc = Loc { session: None, name: s.window.0.to_string(), pos: Pos::Keep };
        let _ = apex_server::proposal::apply(&mut self.node, &mut self.log, apex_server::Proposal::Goto { loc });
        match (self.node.state.window(s.window).map(|x| x.body), s.at) {
            (Ok(Body::Text(_)), At::Text(a)) => {
                let v = ViewId::Body(s.window);
                if let Some(t) = self.text_of(v) {
                    let a = a.min(t.len());
                    let start = t.line_start(t.line_of(a));
                    let _ = self.node.set_origin(&mut self.log, v, start);
                    let _ = self.node.select(&mut self.log, v, a, a);
                }
            }
            (Ok(Body::Term(t)), At::Term(line)) => {
                let top = self.node.state.terms.get(&t).map(|x| x.top).unwrap_or(0);
                self.term_scroll(t, line as isize - top as isize);
            }
            (Ok(Body::Page(_)), At::Page(i)) => {
                let _ = apex_server::proposal::apply(&mut self.node, &mut self.log, apex_server::Proposal::PageScroll { window: s.window, scroll: Some(Scroll::Outline(i)) });
            }
            _ => {}
        }
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
        let field = crate::shell::palette_field(crate::field::field_view(&f.filter, f.caret_visible(), if f.all { "Go to a window or section in any tab, or a file closed lately…" } else { "Go to a window, a section, or a file closed lately…" }, true));
        let typed = !f.filter.trim().is_empty();
        // the rows in view alone, however many there are; the wheel moves
        // them, the keys the cursor
        let mut list = div()
            .id("finder-list")
            .flex()
            .flex_col()
            .px(px(6.))
            .pb(px(6.))
            .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, _, cx| {
                let dy = match e.delta {
                    gpui::ScrollDelta::Lines(p) => -f32::from(p.y),
                    gpui::ScrollDelta::Pixels(p) => -f32::from(p.y) / 34.,
                };
                if let Some(f) = this.finder.as_mut() {
                    f.wheel += dy;
                    let rows = f.wheel.trunc() as i32;
                    if rows != 0 {
                        f.wheel -= rows as f32;
                        f.scroll(rows);
                        cx.notify();
                    }
                }
                cx.stop_propagation();
            }));
        for (i, pick) in picks.iter().enumerate().skip(f.top).take(ROWS) {
            let picked = i == f.cursor;
            let dim = crate::shell::palette_dim(picked, crate::shell::Act::Look);
            let e = match pick {
                Pick::Entry(e) => e,
                Pick::Section(s) => {
                    let p = pick.clone();
                    // a section: indented by depth under its window; its
                    // window named when found by a query
                    let row = crate::shell::palette_row(picked, crate::shell::Act::Look)
                        .id(("goto", i))
                        .cursor_default()
                        .child(div().flex_none().w(px(16.)).flex().justify_center().text_size(px(11.)).text_color(dim).child("›"))
                        .child(div().flex_none().w(px(12. * (s.level.saturating_sub(1)) as f32)))
                        .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(if s.failed { format!("✗ {}", s.title) } else { s.title.clone() }))
                        .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().text_size(px(12.)).text_color(dim).child(s.detail.clone().unwrap_or_default()))
                        .when(typed, |d| d.child(div().flex_none().max_w(px(220.)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(11.5)).text_color(dim).child(s.file.clone())))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.pick(p.clone(), window, cx);
                                cx.stop_propagation();
                            }),
                        );
                    list = list.child(row);
                    continue;
                }
            };
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
        let mut f = Finder::new(vec![
                Entry { name: "/x/closed.rs".into(), label: None, window: None, kind: WinKind::File, tab: None },
                Entry { name: "/x/open.rs".into(), label: None, window: Some(WindowId(1)), kind: WinKind::File, tab: None },
            ], Vec::new(), None, None, false, None);
        f.filter = "rs".into();
        f.changed();
        let picks = f.picks();
        assert!(matches!(&picks[0], Pick::Entry(e) if e.name == "/x/open.rs"), "{picks:?}");
        assert_eq!(picks.len(), 2);
    }

    #[test]
    fn a_window_is_found_by_its_label_and_shows_its_name_and_folder() {
        let e = |name: &str, label: Option<&str>, w: u64| Entry { name: name.into(), label: label.map(String::from), window: Some(WindowId(w)), kind: WinKind::File, tab: None };
        let mut f = Finder::new(vec![e("/src/apex/notes.md", None, 1), e("/src/apex/", Some("rust-analyzer"), 2), e("/src/apex/a/x.rs", None, 3)], Vec::new(), None, None, false, None);
        f.filter = "analyzer".into();
        f.changed();
        let picks = f.picks();
        assert_eq!(picks.len(), 1, "{picks:?}");
        assert!(matches!(&picks[0], Pick::Entry(e) if e.window == Some(WindowId(2))));
        // a folder's last part with its slash, and the folder it is in
        assert_eq!(e("/src/apex/", None, 1).split(), ("apex/".to_string(), "/src/".to_string()));
        assert_eq!(e("/src/apex/notes.md", None, 1).split(), ("notes.md".to_string(), "/src/apex/".to_string()));
        assert_eq!(e("/", None, 1).split(), ("/".to_string(), String::new()));
    }

    fn section(w: u64, level: u8, title: &str, at: usize, file: &str) -> Rc<Section> {
        Rc::new(Section { window: WindowId(w), level, title: title.into(), detail: None, failed: false, at: apex_server::outline::At::Text(at), file: file.into() })
    }

    /// Nothing typed: the windows, the one the keys are in with its
    /// sections under it, the cursor on the section it is showing -- so
    /// ↑↩ is the section before.
    #[test]
    fn it_starts_at_the_section_the_window_is_showing() {
        let e = |name: &str, w: u64| Entry { name: name.into(), label: None, window: Some(WindowId(w)), kind: WinKind::File, tab: None };
        let sections = vec![section(1, 1, "intro", 0, "a.md"), section(2, 1, "fn one", 0, "b.rs"), section(2, 1, "fn two", 10, "b.rs"), section(2, 2, "fn three", 20, "b.rs")];
        let mut f = Finder::new(vec![e("/x/a.md", 1), e("/x/b.rs", 2), e("/x/c.rs", 3)], sections, Some(WindowId(2)), Some(2), false, None);
        let names: Vec<String> = f.picks().iter().map(|p| match p {
            Pick::Entry(e) => e.name.clone(),
            Pick::Section(s) => format!("§{}", s.title),
        }).collect();
        // a.md's sections are not listed: it is not the window the keys are in
        assert_eq!(names, vec!["/x/a.md", "/x/b.rs", "§fn one", "§fn two", "§fn three", "/x/c.rs"]);
        assert!(matches!(&f.picks()[f.cursor], Pick::Section(s) if s.title == "fn two"));
        f.move_cursor(-1);
        assert!(matches!(&f.picks()[f.cursor], Pick::Section(s) if s.title == "fn one"));
    }

    /// Typed: sections of every window are found by title, or by the
    /// window's name and the title, a window first at equal scores.
    #[test]
    fn a_query_finds_any_windows_sections() {
        let e = |name: &str, w: u64| Entry { name: name.into(), label: None, window: Some(WindowId(w)), kind: WinKind::File, tab: None };
        let sections = vec![section(1, 1, "Install", 0, "README.md"), section(2, 1, "fn parse", 0, "lib.rs"), section(3, 1, "fn parse", 0, "main.rs")];
        let mut f = Finder::new(vec![e("/x/README.md", 1), e("/x/lib.rs", 2), e("/x/main.rs", 3)], sections, None, None, false, None);
        f.filter = "install".into();
        f.changed();
        assert!(matches!(&f.picks()[0], Pick::Section(s) if s.title == "Install"), "{:?}", f.picks());
        f.filter = "main parse".into();
        f.changed();
        assert!(matches!(&f.picks()[0], Pick::Section(s) if s.window == WindowId(3)), "{:?}", f.picks());
        assert_eq!(f.cursor, 0);
    }

    /// Fast: twenty thousand sections ranked for a keystroke.
    #[test]
    fn ranking_many_sections_is_quick() {
        let sections: Vec<Rc<Section>> = (0..20_000).map(|i| section(1 + i % 40, 1 + (i % 3) as u8, &format!("fn handle_event_{i}"), i as usize, "server.rs")).collect();
        let mut f = Finder::new(Vec::new(), sections, None, None, false, None);
        let t = std::time::Instant::now();
        f.filter = "hev12".into();
        f.changed();
        let took = t.elapsed();
        assert!(!f.picks().is_empty());
        eprintln!("ranked 20000 sections in {took:?}");
        assert!(took < std::time::Duration::from_millis(250), "{took:?}");
    }

    #[test]
    fn across_the_tabs_this_tab_comes_first_at_equal_scores() {
        let here = SessionUrl::local("main").with_id("aaaa");
        let there = SessionUrl::local("work").with_id("bbbb");
        let (t_here, t_there) = (crate::pool::TabId(1), crate::pool::TabId(2));
        let e = |name: &str, w: u64, id: crate::pool::TabId, l: &str| Entry { name: name.into(), label: None, window: Some(WindowId(w)), kind: WinKind::File, tab: Some((id, l.into())) };
        let mut f = Finder::new(vec![e("/w/src/lib.rs", 1, t_there, "work"), e("/m/src/lib.rs", 2, t_here, "main")], Vec::new(), None, None, true, Some(t_here));
        f.filter = "lib".into();
        f.changed();
        let picks = f.picks();
        assert!(matches!(&picks[0], Pick::Entry(e) if e.tab.as_ref().is_some_and(|(id, _)| *id == t_here)), "{picks:?}");
        assert_eq!(picks.len(), 2);
        // a remote tab is named with its host
        assert_eq!(super::tab_label(&here, Some("main")), "main");
        assert_eq!(super::tab_label(&there, None), "work");
    }
}
