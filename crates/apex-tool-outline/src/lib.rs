//! `apex tool outline`: Outline, in a window's tools menu (shift-B1) or
//! written in its tag, makes the window's outline a text lens on it
//! (DESIGN.md "Lenses"), and Outline (or ⌘↩, View) on that lens takes it
//! away. The outline is whatever the window's scrollbar shows: a file's
//! symbols (as lsp gave them), a terminal's commands with when they ran
//! and how they went, a page's headings or the entries its maker marked
//! (an agent's turns).
//!
//! The lens is text, an entry a line, indented by its depth: B1 on a
//! line, or B3, goes there -- the window under the lens shown at the
//! entry, the lens gone. Its input strip filters it: what is typed there
//! keeps the entries whose titles have its letters in order, the first
//! selected, and ↩ (or ⌘↩, Send) goes to the selected one. It is kept as
//! the outline changes (a command run, a turn taken, a symbol added).
//!
//! A tool like any other, using only the tool API (`apex_tool`): it
//! reads the outline (`Tool::outline`), shows it, and goes to an entry
//! (`Tool::reveal`). A tool composing outlines of its own -- several
//! windows', or one's filtered -- does it the same way. Started by the
//! session's rule when Outline is first used; gone when no lens has been
//! open a while.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use apex_tool::{Event, OutlineEntry, Plumb, Rule, RuleId, Tool, WindowId};

/// How long the tool stays with no lens open.
const IDLE: Duration = Duration::from_secs(60);
/// How often the outlines are looked at again.
const TICK: Duration = Duration::from_millis(250);

/// An outline lens: on `base`, showing `shown` (indices into `entries`)
/// as `text`, the line of `selected` dot.
struct Lens {
    base: WindowId,
    entries: Vec<OutlineEntry>,
    shown: Vec<usize>,
    selected: Option<usize>,
    /// The filter the lines were made for.
    query: String,
    /// The text as last written, and dot as last set: dot anywhere else,
    /// the text the same, is the user's click.
    text: String,
    dot: (usize, usize),
    rules: Vec<RuleId>,
}

pub fn run(socket: &Path, session: &str) -> Result<(), String> {
    let mut t = Tool::attach_to(socket, session, "outline").map_err(|e| e.to_string())?;
    let mut lenses: BTreeMap<WindowId, Lens> = BTreeMap::new();
    let mut idle_since = Some(Instant::now());
    loop {
        match t.next_event(Some(TICK)) {
            Ok(Some(Event::Plumb(p))) => plumbed(&mut t, &mut lenses, &p),
            Ok(Some(Event::Deleted { window })) => {
                if let Some(l) = lenses.remove(&window) {
                    for r in l.rules {
                        t.withdraw(r);
                    }
                }
            }
            Err(e) if e.is_closed() => return Ok(()),
            _ => {}
        }
        let ws: Vec<WindowId> = lenses.keys().copied().collect();
        for w in ws {
            if let Err(e) = keep(&mut t, &mut lenses, w) {
                eprintln!("outline: {e}");
            }
        }
        // gone when no lens is open for a while: the session's rule starts
        // it again (ARCHITECTURE.md §5)
        match (lenses.is_empty(), idle_since) {
            (true, None) => idle_since = Some(Instant::now()),
            (true, Some(at)) if at.elapsed() >= IDLE => return Ok(()),
            (true, Some(_)) => {}
            (false, _) => idle_since = None,
        }
    }
}

/// A rule of ours used: Outline on a window, or View, Send or B3 on a
/// lens of ours.
fn plumbed(t: &mut Tool, lenses: &mut BTreeMap<WindowId, Lens>, p: &Plumb) {
    let Some(w) = p.window else {
        let _ = t.answer(p, false);
        return;
    };
    let _ = t.answer(p, true);
    let r = match (p.verb.as_str(), lenses.contains_key(&w)) {
        // on the lens: taken away, the window as it was
        ("Outline" | "View", true) => close(t, lenses, w),
        ("Outline", false) => open(t, lenses, w),
        ("Send", true) => go_selected(t, lenses, w),
        // B3 on a line: that entry
        ("plumb", true) => match p.at.or(p.sel) {
            Some(r) => go_line(t, lenses, w, r.q0),
            None => Ok(()),
        },
        _ => Ok(()),
    };
    if let Err(e) = r {
        let _ = t.errors(Some(&p.dir), &format!("{}: {e}\n", if p.verb == "plumb" { "Outline" } else { &p.verb }));
    }
}

/// Window `base`'s outline, a lens on it.
fn open(t: &mut Tool, lenses: &mut BTreeMap<WindowId, Lens>, base: WindowId) -> Result<(), String> {
    let entries = t.outline(base).ok_or("this window has no outline")?;
    let path = t.window(base).map(|i| i.path).unwrap_or_default();
    let current = t.outline_current(base, &entries);
    let mut l = Lens { base, entries, shown: Vec::new(), selected: None, query: String::new(), text: String::new(), dot: (0, 0), rules: Vec::new() };
    l.filter(current);
    let w = t.lens_text(base, &format!("{path}+Outline"), &l.text).map_err(|e| e.to_string())?;
    for rule in [Rule::verb("View").window(w).unlisted(), Rule::verb("Send").window(w).unlisted(), Rule::plumb().window(w)] {
        if let Ok(id) = t.offer(rule) {
            l.rules.push(id);
        }
    }
    // the filter, where typing goes
    let _ = t.set_input(w, Some(""));
    let _ = t.set_clean(w);
    l.select(t, w);
    lenses.insert(w, l);
    Ok(())
}

fn close(t: &mut Tool, lenses: &mut BTreeMap<WindowId, Lens>, w: WindowId) -> Result<(), String> {
    if let Some(l) = lenses.remove(&w) {
        for r in l.rules {
            t.withdraw(r);
        }
    }
    t.delete(w).map_err(|e| e.to_string())
}

/// To entry `i` of lens `w`'s: the lens gone, its window shown there.
fn go(t: &mut Tool, lenses: &mut BTreeMap<WindowId, Lens>, w: WindowId, i: usize) -> Result<(), String> {
    let Some(l) = lenses.get(&w) else { return Ok(()) };
    let (base, at) = (l.base, l.entries[i].at);
    close(t, lenses, w)?;
    t.reveal(base, at).map_err(|e| e.to_string())
}

fn go_selected(t: &mut Tool, lenses: &mut BTreeMap<WindowId, Lens>, w: WindowId) -> Result<(), String> {
    match lenses.get(&w).and_then(|l| l.selected) {
        Some(i) => go(t, lenses, w, i),
        None => Err("nothing matches".into()),
    }
}

/// To the entry on the line at `q` in lens `w`.
fn go_line(t: &mut Tool, lenses: &mut BTreeMap<WindowId, Lens>, w: WindowId, q: usize) -> Result<(), String> {
    let Some(l) = lenses.get(&w) else { return Ok(()) };
    let text = t.read(w).map_err(|e| e.to_string())?;
    let line = text.chars().take(q).filter(|&c| c == '\n').count();
    match l.shown.get(line).copied() {
        Some(i) => go(t, lenses, w, i),
        None => Ok(()),
    }
}

/// Lens `w` kept: written again when the outline or the filter changed;
/// gone to where the user clicked, or the entry ↩ was typed for.
fn keep(t: &mut Tool, lenses: &mut BTreeMap<WindowId, Lens>, w: WindowId) -> Result<(), String> {
    let Some(l) = lenses.get_mut(&w) else { return Ok(()) };
    if t.window(w).is_none() {
        lenses.remove(&w);
        return Ok(());
    }
    // ↩ in the filter: the selected entry
    let input = t.input(w).unwrap_or_default();
    if input.contains('\n') {
        let _ = t.take_input(w);
        return go_selected(t, lenses, w);
    }
    // a click on a line, the text as we wrote it: that entry
    if let (Ok(text), Ok(dot)) = (t.read(w), t.selection(w)) {
        if text == l.text && dot.q0 == dot.q1 && (dot.q0, dot.q1) != l.dot {
            return go_line(t, lenses, w, dot.q0);
        }
    }
    let entries = t.outline(l.base).unwrap_or_default();
    if entries == l.entries && input == l.query {
        return Ok(());
    }
    let keep_at = l.selected.and_then(|i| l.entries.get(i)).map(|e| e.at);
    let query_changed = input != l.query;
    l.entries = entries;
    l.query = input;
    let current = match keep_at {
        // the same entry selected, if it is still there
        Some(at) if !query_changed => l.entries.iter().position(|e| e.at == at),
        _ if l.query.is_empty() => t.outline_current(l.base, &l.entries),
        _ => None,
    };
    l.filter(current);
    let len = t.read(w).map(|s| s.chars().count()).unwrap_or(0);
    t.replace(w, 0, len, &l.text).map_err(|e| e.to_string())?;
    let _ = t.set_clean(w);
    l.select(t, w);
    Ok(())
}

impl Lens {
    /// The entries the filter keeps, as lines; `prefer` selected if it is
    /// among them, else the first.
    fn filter(&mut self, prefer: Option<usize>) {
        let q = self.query.trim().to_lowercase();
        self.shown = (0..self.entries.len()).filter(|&i| matches(&q, &self.entries[i].title)).collect();
        self.selected = prefer.filter(|i| self.shown.contains(i)).or(self.shown.first().copied());
        // indented by depth from the shallowest shown, so a flat list is
        // flush and a filtered one keeps its shape
        let top = self.shown.iter().map(|&i| self.entries[i].level).min().unwrap_or(1);
        let mut text = String::new();
        for &i in &self.shown {
            let e = &self.entries[i];
            text.push_str(&"    ".repeat(e.level.saturating_sub(top) as usize));
            if e.failed {
                text.push_str("✗ ");
            }
            text.push_str(&e.title);
            if let Some(d) = &e.detail {
                text.push_str("    ");
                text.push_str(d);
            }
            text.push('\n');
        }
        self.text = text;
    }

    /// Dot on the selected entry's line, shown.
    fn select(&mut self, t: &mut Tool, w: WindowId) {
        let Some(line) = self.selected.and_then(|i| self.shown.iter().position(|&s| s == i)) else {
            self.dot = (0, 0);
            let _ = t.select(w, 0, 0);
            return;
        };
        let mut q0 = 0;
        for (n, l) in self.text.split('\n').enumerate() {
            let len = l.chars().count();
            if n == line {
                self.dot = (q0, q0 + len);
                let _ = t.select(w, q0, q0 + len);
                let _ = t.show(w, q0);
                return;
            }
            q0 += len + 1;
        }
    }
}

/// `title` has `q`'s letters in order (`q` lowercase), ignoring case.
fn matches(q: &str, title: &str) -> bool {
    let mut want = q.chars().filter(|c| !c.is_whitespace()).peekable();
    for c in title.chars().flat_map(char::to_lowercase) {
        if want.peek() == Some(&c) {
            want.next();
        }
    }
    want.peek().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_tool::OutlineAt;

    fn entry(level: u8, title: &str, detail: Option<&str>, failed: bool, at: usize) -> OutlineEntry {
        OutlineEntry { level, title: title.into(), detail: detail.map(String::from), failed, at: OutlineAt::Text(at) }
    }

    fn lens(entries: Vec<OutlineEntry>, query: &str) -> Lens {
        Lens { base: WindowId(1), entries, shown: Vec::new(), selected: None, query: query.into(), text: String::new(), dot: (0, 0), rules: Vec::new() }
    }

    #[test]
    fn letters_in_order() {
        assert!(matches("", "anything"));
        assert!(matches("ft", "fn filter"));
        assert!(matches("fl t", "Filter"));
        assert!(!matches("tf", "fn filter"));
    }

    #[test]
    fn lines_indented_with_detail_and_failure() {
        let mut l = lens(vec![entry(1, "mod a", None, false, 0), entry(2, "fn b", None, false, 5), entry(1, "cargo test", Some("2m ago › failed after 3.0s (exit 101)"), true, 9)], "");
        l.filter(Some(1));
        assert_eq!(l.text, "mod a\n    fn b\n✗ cargo test    2m ago › failed after 3.0s (exit 101)\n");
        assert_eq!(l.selected, Some(1));
    }

    #[test]
    fn filtered_flush_from_the_shallowest_first_selected() {
        let mut l = lens(vec![entry(1, "mod a", None, false, 0), entry(2, "fn bar", None, false, 5), entry(3, "fn baz", None, false, 9)], "ba");
        l.filter(Some(0));
        assert_eq!(l.text, "fn bar\n    fn baz\n");
        assert_eq!(l.shown, vec![1, 2]);
        assert_eq!(l.selected, Some(1));
    }
}
