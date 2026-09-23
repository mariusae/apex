//! Unified diffs as a page. What `diff -u` or `git diff` writes, laid
//! out side by side the way rsc's review lays out a change -- the old
//! file on the left and the new on the right, each with its line
//! numbers, a band between hunks, a pale colour for a changed line and a
//! strong one for the part of it that changed -- in acme's colours, and
//! with every file name, line number and line a link to its place in
//! the file on disk: `apexfile://localhost/path?line=N`, which a page in
//! apex opens in a text window at that line.
//!
//! The colours are the page's theme (`--apex-*`, which apex gives every
//! page and rewrites when the theme changes), with acme's light ones for
//! a page seen anywhere else. Added and removed are blue and orange, not
//! green and red: on acme's yellow paper Gerrit's greens and reds (which
//! review keeps, as chosen to stay legible on white) run together for a
//! reader with deuteranopia, and blue against orange does not.
//!
//! Nothing here touches the filesystem: paths in the diff are resolved
//! against the base directory given, as they are written.

use std::fmt::Write;
use std::path::Path;

/// One file's part of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct File {
    /// The path before, as the diff names it (`a/` taken off a git diff's);
    /// none for a file the diff makes.
    pub old: Option<String>,
    /// The path after; none for a file the diff deletes.
    pub new: Option<String>,
    /// What the diff says about the file besides its lines: a mode, a
    /// rename, "Binary files … differ".
    pub notes: Vec<String>,
    pub hunks: Vec<Hunk>,
}

impl File {
    /// The name to show: the new path, else the old.
    pub fn name(&self) -> &str {
        self.new.as_deref().or(self.old.as_deref()).unwrap_or("?")
    }

    /// Lines added and removed.
    pub fn counts(&self) -> (usize, usize) {
        let mut add = 0;
        let mut del = 0;
        for h in &self.hunks {
            for l in &h.lines {
                match l {
                    Line::Add(_) => add += 1,
                    Line::Del(_) => del += 1,
                    _ => {}
                }
            }
        }
        (add, del)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    /// What follows the second `@@`: the function the hunk is in, as
    /// `diff -p` and git name it.
    pub section: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Same(String),
    Del(String),
    Add(String),
    /// `\ No newline at end of file`, after the line it is about.
    NoNewline,
}

/// The files of a unified diff, in order. Text before the first file
/// (a commit message, `git show`'s header) and lines no diff writes are
/// passed over; a hunk ends when it has as many lines as its header
/// says, so a `--- ` starting the next file is never taken for a removal.
pub fn parse(text: &str) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();
    // this file's diff --git line said it was git's: a/ and b/ go
    let mut git = false;
    // lines of the open hunk still to come, old side and new
    let (mut old_left, mut new_left) = (0usize, 0usize);
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if old_left > 0 || new_left > 0 {
            let Some(h) = files.last_mut().and_then(|f| f.hunks.last_mut()) else {
                old_left = 0;
                new_left = 0;
                continue;
            };
            // a blank context line whose space an editor took is still one
            let (tag, rest) = match line.chars().next() {
                Some(c) => (c, &line[c.len_utf8()..]),
                None => (' ', ""),
            };
            match tag {
                ' ' => {
                    h.lines.push(Line::Same(rest.to_string()));
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    continue;
                }
                '-' => {
                    h.lines.push(Line::Del(rest.to_string()));
                    old_left = old_left.saturating_sub(1);
                    continue;
                }
                '+' => {
                    h.lines.push(Line::Add(rest.to_string()));
                    new_left = new_left.saturating_sub(1);
                    continue;
                }
                '\\' => {
                    h.lines.push(Line::NoNewline);
                    continue;
                }
                // not a hunk's line: the hunk was shorter than it said
                _ => {
                    old_left = 0;
                    new_left = 0;
                }
            }
        }
        // said of the last line, which ended its hunk
        if line.starts_with("\\ ") {
            if let Some(h) = files.last_mut().and_then(|f| f.hunks.last_mut()) {
                h.lines.push(Line::NoNewline);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            git = true;
            let mut f = File::default();
            // the names, for a diff with no --- and +++ (a mode change,
            // a rename, a binary file): "a/x b/x", unquoted
            if let Some((a, b)) = split_git_names(rest) {
                f.old = clean(&a, true);
                f.new = clean(&b, true);
            }
            files.push(f);
            continue;
        }
        if line.starts_with("--- ") && lines.get(i).is_some_and(|n| n.starts_with("+++ ")) {
            let (old, new) = (&line[4..], &lines[i][4..]);
            i += 1;
            // a git diff's file began at its diff --git line; any other
            // diff's begins here
            let continuing = git && files.last().is_some_and(|f| f.hunks.is_empty());
            if !continuing {
                git = false;
                files.push(File::default());
            }
            let git_names = git || (old.starts_with("a/") && new.starts_with("b/"));
            let f = files.last_mut().unwrap();
            f.old = clean(old, git_names);
            f.new = clean(new, git_names);
            continue;
        }
        if let Some(h) = hunk_header(line) {
            if let Some(f) = files.last_mut() {
                old_left = h.1;
                new_left = h.3;
                f.hunks.push(Hunk { old_start: h.0, new_start: h.2, section: h.4, lines: Vec::new() });
            }
            continue;
        }
        // what a diff says about a file besides its lines
        if let Some(f) = files.last_mut().filter(|f| f.hunks.is_empty()) {
            let note = line.trim();
            let said = ["new file mode", "deleted file mode", "old mode", "new mode", "rename from", "rename to", "copy from", "copy to", "similarity index", "Binary files"];
            if said.iter().any(|s| note.starts_with(s)) {
                if note.starts_with("new file mode") {
                    f.old = None;
                }
                if note.starts_with("deleted file mode") {
                    f.new = None;
                }
                f.notes.push(note.to_string());
            }
        }
    }
    files
}

/// `@@ -a,b +c,d @@ section`: the starts, the lengths (1 when a length
/// is not written), and what follows.
fn hunk_header(line: &str) -> Option<(usize, usize, usize, usize, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@").unwrap_or((rest, ""));
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(usize, usize)> {
        match r.split_once(',') {
            Some((s, n)) => Some((s.parse().ok()?, n.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (a, b) = range(old)?;
    let (c, d) = range(new)?;
    Some((a, b, c, d, section.trim().to_string()))
}

/// `a/x b/x` as a diff --git line writes two unquoted names: split where
/// the second name starts, which is where ` b/` does.
fn split_git_names(s: &str) -> Option<(String, String)> {
    let at = s.find(" b/")?;
    Some((s[..at].to_string(), s[at + 1..].to_string()))
}

/// A name as a diff line gives it: the timestamp after a tab dropped,
/// git's quoting undone, `a/` or `b/` taken off a git diff's, and
/// `/dev/null` (a file made or deleted) as none.
fn clean(name: &str, git: bool) -> Option<String> {
    let name = name.split('\t').next().unwrap_or("").trim_end();
    let name = unquote(name);
    if name == "/dev/null" || name.is_empty() {
        return None;
    }
    if git {
        if let Some(rest) = name.strip_prefix("a/").or_else(|| name.strip_prefix("b/")) {
            return Some(rest.to_string());
        }
    }
    Some(name)
}

/// git's C-style quoting of an odd name ("a/with\ttab"), undone.
fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else { return s.to_string() };
    let mut out = Vec::new();
    let mut b = inner.bytes();
    while let Some(c) = b.next() {
        if c != b'\\' {
            out.push(c);
            continue;
        }
        match b.next() {
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(d @ b'0'..=b'7') => {
                // three octal digits: a byte of the name's UTF-8
                let mut v = (d - b'0') as u32;
                for _ in 0..2 {
                    if let Some(e @ b'0'..=b'7') = b.clone().next() {
                        b.next();
                        v = v * 8 + (e - b'0') as u32;
                    }
                }
                out.push(v as u8);
            }
            Some(o) => out.push(o),
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A row of the side-by-side table: the old file's line on the left, the
/// new file's on the right, either side absent where the other has a
/// line of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
    pub kind: Kind,
    /// The line of the new file this row is at: what a click on any of it
    /// opens. A removed line is at the line of the new file where it was.
    pub at: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The same on both sides.
    Same,
    /// A line removed and one added in its place: the part that changed
    /// is marked on each.
    Changed,
    /// Removed, with nothing in its place.
    Removed,
    /// Added, with nothing it replaced.
    Added,
    /// `\ No newline at end of file`.
    NoNewline,
}

/// A hunk's rows: context on both sides; a run of removed lines and the
/// run of added lines after it paired off row by row, the longer run's
/// rest against nothing.
pub fn rows(h: &Hunk) -> Vec<Row> {
    let mut out = Vec::new();
    let (mut o, mut n) = (h.old_start, h.new_start);
    let mut i = 0;
    while i < h.lines.len() {
        match &h.lines[i] {
            Line::Same(t) => {
                out.push(Row { left: Some((o, t.clone())), right: Some((n, t.clone())), kind: Kind::Same, at: n.max(1) });
                o += 1;
                n += 1;
                i += 1;
            }
            Line::NoNewline => {
                out.push(Row { left: None, right: None, kind: Kind::NoNewline, at: n.saturating_sub(1).max(1) });
                i += 1;
            }
            Line::Del(_) | Line::Add(_) => {
                let mut dels = Vec::new();
                while let Some(Line::Del(t)) = h.lines.get(i) {
                    dels.push(t.clone());
                    i += 1;
                }
                let mut adds = Vec::new();
                while let Some(Line::Add(t)) = h.lines.get(i) {
                    adds.push(t.clone());
                    i += 1;
                }
                for k in 0..dels.len().max(adds.len()) {
                    let left = dels.get(k).map(|t| (o + k, t.clone()));
                    let right = adds.get(k).map(|t| (n + k, t.clone()));
                    let kind = match (&left, &right) {
                        (Some(_), Some(_)) => Kind::Changed,
                        (Some(_), None) => Kind::Removed,
                        _ => Kind::Added,
                    };
                    // a removed line is where the new file carries on
                    let at = if right.is_some() { n + k } else { n + adds.len() };
                    out.push(Row { left, right, kind, at: at.max(1) });
                }
                o += dels.len();
                n += adds.len();
            }
        }
    }
    out
}

/// The part of two lines that differs, as char ranges into each: what is
/// left once their common start and end are taken away. Gerrit marks a
/// changed line's changed words with a real diff; the common ends are
/// the whole of it for the one edit a line usually has, and never mark
/// less than changed.
pub fn changed_part(a: &str, b: &str) -> ((usize, usize), (usize, usize)) {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    ((pre, a.len() - suf), (pre, b.len() - suf))
}

/// A link to the file at `path` (under `base` unless absolute) at `line`:
/// what a page in apex opens in a text window there. Through apex's own
/// scheme, `apexfile://`, not `file://`: a page from a buffer has no
/// origin WebKit lets reach a `file:` URL, and it refuses the navigation
/// before apex could hear of it.
pub fn file_url(base: &Path, path: &str, line: usize) -> String {
    let p = if Path::new(path).is_absolute() { Path::new(path).to_path_buf() } else { base.join(path) };
    let mut out = String::from("apexfile://localhost");
    for c in p.to_string_lossy().bytes() {
        if c.is_ascii_alphanumeric() || b"/-._~".contains(&c) {
            out.push(c as char);
        } else {
            let _ = write!(out, "%{c:02X}");
        }
    }
    let _ = write!(out, "?line={line}");
    out
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// A line's text, the chars `[q0, q1)` marked as the part that changed.
fn marked(s: &str, (q0, q1): (usize, usize)) -> String {
    if q0 >= q1 {
        return esc(s);
    }
    let cs: Vec<char> = s.chars().collect();
    let part = |a: usize, b: usize| esc(&cs[a.min(cs.len())..b.min(cs.len())].iter().collect::<String>());
    format!("{}<span class=\"i\">{}</span>{}", part(0, q0), part(q0, q1), part(q1, cs.len()))
}

/// The page: a summary, then each file -- its name (a link to the file,
/// at its first hunk), how much it changed, what the diff said of it,
/// and its hunks side by side.
pub fn page(files: &[File], base: &Path) -> String {
    let mut out = String::new();
    let (adds, dels) = files.iter().map(File::counts).fold((0, 0), |a, c| (a.0 + c.0, a.1 + c.1));
    let _ = write!(out, "<!doctype html><html><head><meta charset=\"utf-8\"><title>Diff</title><style>{STYLE}</style></head><body>");
    let _ = write!(
        out,
        "<div class=\"summary\">{} file{} changed, <span class=\"plus\">+{adds}</span> <span class=\"minus\">\u{2212}{dels}</span></div>",
        files.len(),
        if files.len() == 1 { "" } else { "s" }
    );
    if files.is_empty() {
        out.push_str("<p class=\"empty\">No differences.</p>");
    }
    for f in files {
        // lines are links into the new file; a deleted file has none
        let link = |line: usize| f.new.as_deref().map(|p| file_url(base, p, line));
        let first = f.hunks.first().map(|h| h.new_start.max(1)).unwrap_or(1);
        let (a, d) = f.counts();
        out.push_str("<section class=\"file\"><h2>");
        match link(first) {
            Some(u) => {
                let _ = write!(out, "<a class=\"name\" href=\"{}\">{}</a>", esc(&u), esc(f.name()));
            }
            None => {
                let _ = write!(out, "<span class=\"name\">{}</span>", esc(f.name()));
            }
        }
        if let (Some(o), Some(n)) = (&f.old, &f.new) {
            if o != n {
                let _ = write!(out, " <span class=\"from\">from {}</span>", esc(o));
            }
        }
        let _ = write!(out, " <span class=\"delta\"><span class=\"plus\">+{a}</span> <span class=\"minus\">\u{2212}{d}</span></span>");
        for n in &f.notes {
            let _ = write!(out, " <span class=\"note\">{}</span>", esc(n));
        }
        out.push_str("</h2>");
        if f.hunks.is_empty() {
            out.push_str("</section>");
            continue;
        }
        // both layouts, and the page's width chooses (STYLE): side by
        // side where each side has room for a line, one above the other
        // where it has not
        let mut split = String::from("<table class=\"diff split\"><colgroup><col class=\"numcol\"><col><col class=\"numcol\"><col></colgroup><tbody>");
        let mut inline = String::from("<table class=\"diff inline\"><colgroup><col class=\"numcol\"><col class=\"numcol\"><col></colgroup><tbody>");
        for h in &f.hunks {
            let href = link(h.new_start.max(1)).map(|u| format!(" data-href=\"{}\"", esc(&u))).unwrap_or_default();
            let band = format!("@@ \u{2212}{} +{} @@ <span class=\"section\">{}</span>", h.old_start, h.new_start, esc(&h.section));
            let _ = write!(split, "<tr class=\"hunk\"{href}><td colspan=\"4\">{band}</td></tr>");
            let _ = write!(inline, "<tr class=\"hunk\"{href}><td colspan=\"3\">{band}</td></tr>");
            let cells: Vec<Cells> = rows(h).iter().map(|r| cells(r, link(r.at))).collect();
            split_rows(&mut split, &cells);
            inline_rows(&mut inline, &cells);
        }
        split.push_str("</tbody></table>");
        inline.push_str("</tbody></table>");
        out.push_str(&split);
        out.push_str(&inline);
        out.push_str("</section>");
    }
    let _ = write!(out, "<script>{SCRIPT}</script></body></html>");
    out
}

/// A row's parts as both layouts draw them: each side's number (a link
/// when the file is there to open), text (its changed part marked) and
/// colour, or none where that side has no line; and where a click goes.
struct Cells {
    href: String,
    kind: Kind,
    left: Option<(String, String, &'static str)>,
    right: Option<(String, String, &'static str)>,
}

fn cells(r: &Row, u: Option<String>) -> Cells {
    let href = u.as_ref().map(|u| format!(" data-href=\"{}\"", esc(u))).unwrap_or_default();
    let num = |n: usize| match &u {
        Some(u) => format!("<a href=\"{}\">{n}</a>", esc(u)),
        None => n.to_string(),
    };
    let (lclass, rclass) = match r.kind {
        Kind::Changed => ("del", "add"),
        Kind::Removed => ("del total", ""),
        Kind::Added => ("", "add total"),
        Kind::Same | Kind::NoNewline => ("", ""),
    };
    let (lpart, rpart) = match (&r.left, &r.right) {
        (Some((_, a)), Some((_, b))) if r.kind == Kind::Changed => changed_part(a, b),
        _ => ((0, 0), (0, 0)),
    };
    Cells {
        href,
        kind: r.kind,
        left: r.left.as_ref().map(|(n, t)| (num(*n), marked(t, lpart), lclass)),
        right: r.right.as_ref().map(|(n, t)| (num(*n), marked(t, rpart), rclass)),
    }
}

/// Side by side: the old line and the new on one row, a side with no
/// line on it blank.
fn split_rows(out: &mut String, rows: &[Cells]) {
    for c in rows {
        let href = &c.href;
        if c.kind == Kind::NoNewline {
            let _ = write!(out, "<tr class=\"nonl\"{href}><td></td><td colspan=\"3\">No newline at end of file</td></tr>");
            continue;
        }
        let side = |s: &Option<(String, String, &'static str)>| match s {
            Some((n, t, class)) => format!("<td class=\"num {class}\">{n}</td><td class=\"code {class}\">{t}</td>"),
            None => "<td class=\"num blank\"></td><td class=\"code blank\"></td>".to_string(),
        };
        let _ = write!(out, "<tr{href}>{}{}</tr>", side(&c.left), side(&c.right));
    }
}

/// One above the other, as `diff -u` writes it: a line both sides share
/// once, with both its numbers; a changed run's removed lines, then its
/// added ones, each with the number of the side it is on.
fn inline_rows(out: &mut String, rows: &[Cells]) {
    let mut i = 0;
    while i < rows.len() {
        let c = &rows[i];
        match c.kind {
            Kind::NoNewline => {
                let _ = write!(out, "<tr class=\"nonl\"{}><td colspan=\"2\"></td><td>No newline at end of file</td></tr>", c.href);
                i += 1;
            }
            Kind::Same => {
                let (ln, lt, _) = c.left.as_ref().expect("a shared line has both sides");
                let (rn, _, _) = c.right.as_ref().expect("a shared line has both sides");
                let _ = write!(out, "<tr{}><td class=\"num\">{ln}</td><td class=\"num\">{rn}</td><td class=\"code\">{lt}</td></tr>", c.href);
                i += 1;
            }
            _ => {
                let run = rows[i..].iter().take_while(|c| matches!(c.kind, Kind::Changed | Kind::Removed | Kind::Added)).count();
                for c in &rows[i..i + run] {
                    if let Some((n, t, class)) = &c.left {
                        let _ = write!(out, "<tr{}><td class=\"num {class}\">{n}</td><td class=\"num {class}\"></td><td class=\"code {class}\">{t}</td></tr>", c.href);
                    }
                }
                for c in &rows[i..i + run] {
                    if let Some((n, t, class)) = &c.right {
                        let _ = write!(out, "<tr{}><td class=\"num {class}\"></td><td class=\"num {class}\">{n}</td><td class=\"code {class}\">{t}</td></tr>", c.href);
                    }
                }
                i += run;
            }
        }
    }
}

/// A diff's text as its page, its paths under `base`.
pub fn render(text: &str, base: &Path) -> String {
    page(&parse(text), base)
}

/// review's layout -- a fixed table of number, code, number, code, the
/// code wrapping where it must, a band between hunks -- in acme's
/// colours. Each is the page's theme variable, with acme's light colour
/// for a page seen outside apex.
const STYLE: &str = r#"
:root {
  --bg: var(--apex-bg, #FFFFEA); --fg: var(--apex-fg, #000);
  --tag: var(--apex-tag-bg, #EAFFFF); --band: var(--apex-code-bg, #E8E8DC);
  --rule: var(--apex-rule, #C8C8B8); --dim: var(--apex-dim, #6F6F60); --border: var(--apex-border, #99994C);
  --add: var(--apex-add, #C8E4FF); --add-strong: var(--apex-add-strong, #B0C4FF);
  --del: var(--apex-del, #FFE6B6); --del-strong: var(--apex-del-strong, #FFC080);
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 13px "Lucida Grande", -apple-system, sans-serif; }
a { color: inherit; text-decoration: none; }
a:hover { text-decoration: underline; }
.summary { padding: 8px 12px; color: var(--dim); }
.plus, .minus { font: 12px Menlo, monospace; }
.empty { padding: 0 12px; color: var(--dim); }
section.file { margin: 0 0 16px; }
/* a file's name as acme's tags are: the pale blue, a line under it,
   and there at the top while its lines go by */
h2 { position: sticky; top: 0; z-index: 1; margin: 0; padding: 4px 12px; font: 13px "Lucida Grande", -apple-system, sans-serif;
     background: var(--tag); border-top: 1px solid var(--border); border-bottom: 1px solid var(--border); }
h2 .name { font-weight: bold; }
h2 .from, h2 .delta, h2 .note { color: var(--dim); margin-left: 6px; }
table.diff { width: 100%; border-collapse: collapse; table-layout: fixed; font: 12px/16px Menlo, monospace; tab-size: 4; }
col.numcol { width: 6ch; }
/* side by side while each side has room for a line of code, about 70
   columns of it beside its number; one above the other below that, as
   the page narrows with its column -- no render, the page decides */
table.inline { display: none; }
@media (max-width: 1100px) {
  table.split { display: none; }
  table.inline { display: table; }
}
td { padding: 0; vertical-align: top; }
td.num { text-align: right; padding-right: 6px; color: var(--dim); background: var(--band); border-right: 1px solid var(--rule); user-select: none; }
td.num a { display: block; cursor: pointer; }
td.num a:hover { color: var(--fg); }
/* review breaks a long line anywhere, which suits a browser's whole
   width; a page in apex is half a column, where that splits every other
   word, so a line breaks where it has room to (a space), and a token is
   split only when it will not fit on a line of its own */
td.code { white-space: pre-wrap; overflow-wrap: anywhere; padding: 0 6px; }
/* Gerrit's rule: the pale colour for a changed line, the strong one for
   the part of it that changed, and for the whole of a line that is only
   added or only removed */
td.code.add { background: var(--add); } td.code.add .i { background: var(--add-strong); }
td.code.del { background: var(--del); } td.code.del .i { background: var(--del-strong); }
td.code.add.total { background: var(--add-strong); } td.code.del.total { background: var(--del-strong); }
td.num.add, td.num.del { color: var(--fg); }
td.blank { background: var(--band); }
tr.hunk td { background: var(--band); color: var(--dim); padding: 2px 8px; border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule);
             font: 11px Menlo, monospace; cursor: pointer; }
tr.hunk .section { color: var(--fg); }
tr.nonl td { color: var(--dim); font-style: italic; }
"#;

/// A click on a line -- anywhere in it, not only its number -- opens its
/// file there. A drag that selects text is a selection, not a click.
const SCRIPT: &str = r#"
document.addEventListener('click', function (e) {
  if (e.target.closest('a')) return;
  var s = window.getSelection();
  if (s && !s.isCollapsed) return;
  var tr = e.target.closest('tr[data-href]');
  if (tr) location.href = tr.dataset.href;
});
"#;

#[cfg(test)]
mod tests {
    use super::*;

    const GIT: &str = "\
commit 1234
Author: someone

    a message, with a line that starts
    --- like this, which is not a file

diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,5 @@ fn main() {
 use std::io;
-let x = 1;
+let x = 2;
+let y = 3;
 fn f() {}
 fn g() {}
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
\\ No newline at end of file
";

    #[test]
    fn a_git_diff_is_its_files_with_their_names_as_the_tree_has_them() {
        let files = parse(GIT);
        assert_eq!(files.len(), 3, "the commit message is no file: {files:#?}");
        assert_eq!(files[0].old.as_deref(), Some("src/main.rs"));
        assert_eq!(files[0].new.as_deref(), Some("src/main.rs"), "a/ and b/ come off");
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].section, "fn main() {");
        assert_eq!(files[0].counts(), (2, 1));
        assert_eq!(files[1].old, None, "a new file was nothing before");
        assert_eq!(files[1].new.as_deref(), Some("new.txt"));
        assert!(files[1].notes.iter().any(|n| n.starts_with("new file mode")));
        assert_eq!(files[2].new, None, "a deleted file is nothing after");
        assert_eq!(files[2].hunks[0].lines, vec![Line::Del("bye".into()), Line::NoNewline]);
    }

    #[test]
    fn a_plain_diff_u_keeps_its_names_and_drops_their_timestamps() {
        let text = "--- old/a.c\t2026-01-01 10:00:00.000000000 +0000\n+++ new/a.c\t2026-01-02 10:00:00.000000000 +0000\n@@ -2 +2 @@\n-a\n+b\n";
        let files = parse(text);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old.as_deref(), Some("old/a.c"), "no a/ prefix, so nothing comes off");
        assert_eq!(files[0].new.as_deref(), Some("new/a.c"));
        assert_eq!(files[0].hunks[0].old_start, 2, "a hunk length of one need not be written");
        // and a --- inside a hunk is a removed line, not the next file
        let text = "--- a\n+++ b\n@@ -1,2 +1 @@\n--- x\n same\n";
        let files = parse(text);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks[0].lines, vec![Line::Del("-- x".into()), Line::Same("same".into())]);
    }

    #[test]
    fn removed_and_added_runs_pair_off_side_by_side() {
        let h = Hunk {
            old_start: 10,
            new_start: 20,
            section: String::new(),
            lines: vec![Line::Same("a".into()), Line::Del("b".into()), Line::Del("c".into()), Line::Add("B".into()), Line::Same("d".into()), Line::Add("e".into())],
        };
        let r = rows(&h);
        let shape: Vec<(Option<usize>, Option<usize>, Kind, usize)> = r.iter().map(|r| (r.left.as_ref().map(|l| l.0), r.right.as_ref().map(|l| l.0), r.kind, r.at)).collect();
        assert_eq!(
            shape,
            vec![
                (Some(10), Some(20), Kind::Same, 20),
                (Some(11), Some(21), Kind::Changed, 21),
                // removed with nothing in its place: where the new file carries on
                (Some(12), None, Kind::Removed, 22),
                (Some(13), Some(22), Kind::Same, 22),
                (None, Some(23), Kind::Added, 23),
            ]
        );
    }

    #[test]
    fn the_part_of_a_changed_line_that_changed_is_marked() {
        assert_eq!(changed_part("let x = 1;", "let x = 22;"), ((8, 9), (8, 10)));
        assert_eq!(changed_part("same", "same"), ((4, 4), (4, 4)));
        // an insertion marks nothing on the side that had nothing there
        assert_eq!(changed_part("ab", "aXb"), ((1, 1), (1, 2)));
        assert_eq!(marked("let x = 1;", (8, 9)), "let x = <span class=\"i\">1</span>;");
    }

    #[test]
    fn wide_it_is_side_by_side_and_narrow_one_above_the_other() {
        let text = "--- a\n+++ b\n@@ -1,4 +1,3 @@\n top\n-red\n-tan\n+blue\n end\n";
        let html = render(text, Path::new("/w"));
        let (s, i) = (html.find("<table class=\"diff split\">").expect("side by side"), html.find("<table class=\"diff inline\">").expect("inline"));
        let (split, inline) = (&html[s..i], &html[i..]);
        // side by side, red and blue share a row: the change pairs off
        let row = &split[split.find("red").unwrap()..];
        assert!(row.find("blue").unwrap() < row.find("</tr>").unwrap(), "{split}");
        // one above the other, as diff -u writes it: the removed run, then the added
        let at = |w: &str| inline.find(w).unwrap();
        assert!(at("top") < at("red") && at("red") < at("tan") && at("tan") < at("blue") && at("blue") < at("end"), "{inline}");
        // a shared line once, with both its numbers
        assert!(inline.contains("<td class=\"num\"><a href=\"apexfile://localhost/w/b?line=1\">1</a></td><td class=\"num\"><a href=\"apexfile://localhost/w/b?line=1\">1</a></td>"), "{inline}");
        // and the page's width chooses between them
        assert!(html.contains("@media (max-width: 1100px)"));
    }

    #[test]
    fn every_line_and_name_links_to_its_place_in_the_file() {
        let html = render(GIT, Path::new("/work/repo"));
        // the name, at its first hunk; each line, at its line of the new file
        assert!(html.contains("href=\"apexfile://localhost/work/repo/src/main.rs?line=1\""), "{html}");
        assert!(html.contains("data-href=\"apexfile://localhost/work/repo/src/main.rs?line=3\""), "the added y");
        // a deleted file has nowhere to go
        assert!(!html.contains("gone.txt?line"), "no links into a file that is gone");
        // what the diff holds is text, whatever it says
        let html = render("--- a\n+++ b\n@@ -1 +1 @@\n-<b>&\n+<i>&\n", Path::new("/w"));
        assert!(html.contains("&lt;<span class=\"i\">b</span>&gt;&amp;"), "{html}");
        // names with room in them are escaped for a URL
        assert_eq!(file_url(Path::new("/w"), "a b/c#d.rs", 3), "apexfile://localhost/w/a%20b/c%23d.rs?line=3");
        assert_eq!(file_url(Path::new("/w"), "/abs/x.rs", 1), "apexfile://localhost/abs/x.rs?line=1");
    }
}
