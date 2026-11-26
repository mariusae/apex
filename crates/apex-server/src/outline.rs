//! A window's outline, whatever its body: what the scrollbar's outline
//! shows and what a tool reads (`apex_tool::Tool::outline`) to compose
//! outlines of its own -- the outline lens (`apex tool outline`), say.
//!
//! - A text's is what a tool gave it (`Buffer::outline`: lsp's symbols),
//!   while that tool is attached.
//! - A terminal's is the commands its shell marked (OSC 133), each with
//!   when it ran and how it went.
//! - A page's is parsed from its HTML, as the client's `TOC_SCRIPT` finds
//!   it in the page: the elements marked `data-apex-outline="LEVEL"`
//!   (titled by `data-apex-title`, else their text), when there are any
//!   -- how a page's maker says what its outline is -- else its headings,
//!   h1 to h4. A page at an address has none here (only the client sees
//!   its document).
//!
//! Each entry says where it is in its own terms (`At`), which
//! `reveal` takes back to: a place in the text, which moves with the
//! edits; a history line of the terminal; the page's nth entry.

use apex_core::{Body, Node, Source, ViewId, WindowId};

/// Where an outline's entry is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum At {
    /// A place in the text (chars).
    Text(usize),
    /// A history line of the terminal, the oldest kept being 0.
    Term(u64),
    /// The page's nth entry, as its outline counts them.
    Page(u32),
}

/// One of a window's outline's entries.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// How deep: 1 the top.
    pub level: u8,
    pub title: String,
    /// What else there is to say about it: a command's when and how
    /// long ("18m ago › took 2m 52s").
    pub detail: Option<String>,
    /// It went wrong: a command that failed.
    pub failed: bool,
    pub at: At,
}

/// Window `w`'s outline, as it is at `now_ms` (milliseconds since the
/// epoch, for a command's "18m ago"); `None` when it has none.
pub fn of(node: &Node, w: WindowId, now_ms: u64) -> Option<Vec<Entry>> {
    let win = node.state.window(w).ok()?;
    let entries: Vec<Entry> = match win.body {
        Body::Text(b) => {
            let o = node.window_outline(w)?;
            let len = node.state.buffer(b).ok()?.text.len();
            // a command among them (win's) says how it ran, as a terminal's;
            // a lens's lines standing for its window's entries are not
            // an outline of their own
            o.items
                .iter()
                .filter(|it| it.of.is_none())
                .map(|it| Entry { level: it.level.clamp(1, 4), title: it.title.clone(), detail: crate::run_said_of(it.started, it.ended, it.exit, now_ms), failed: it.exit.is_some_and(|x| x != 0), at: At::Text(it.at.min(len)) })
                .collect()
        }
        Body::Term(t) => {
            let term = node.state.terms.get(&t)?;
            term.marks
                .iter()
                .filter(|m| m.started.is_some() || m.cmd.is_some())
                .map(|m| Entry { level: 1, title: m.cmd.clone().unwrap_or_else(|| "a command".into()), detail: crate::run_said(m, now_ms), failed: m.exit.is_some_and(|x| x != 0), at: At::Term(m.prompt) })
                .collect()
        }
        Body::Page(Source::Buffer(b)) => {
            let html = node.state.buffer(b).ok()?.text.to_string();
            page(&html).into_iter().enumerate().map(|(i, (level, title))| Entry { level, title, detail: None, failed: false, at: At::Page(i as u32) }).collect()
        }
        Body::Page(Source::Url) => return None,
    };
    (!entries.is_empty()).then_some(entries)
}

/// A command in a window's outline (a terminal's, or a text's that a
/// shell's tool keeps: win's): what was run, and where its output is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Run {
    pub cmd: String,
    pub output: RunOutput,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunOutput {
    /// A text's: the output as the text has it now.
    Text(String),
    /// A terminal's: its history lines `from` to `to`, both in, which
    /// only the host has (`TermHost::text`); none when it printed nothing.
    Rows { term: apex_core::TermId, from: u64, to: u64 },
    None,
}

/// The `i`th of window `w`'s outline's entries (as `of` lists them), if
/// it is a command: a terminal's prompt mark, or a text's item with a
/// run. Its output runs to its end, or, still running, to where the
/// output stands now.
pub fn run_of(node: &Node, w: WindowId, i: usize) -> Option<Run> {
    let win = node.state.window(w).ok()?;
    match win.body {
        Body::Text(b) => {
            let it = node.window_outline(w)?.items.get(i)?;
            // a lens's line: the entry under it it stands for
            if let (Some(j), Some(base)) = (it.of, node.state.layout.base_of(w)) {
                return run_of(node, base, j);
            }
            it.started?;
            let text = &node.state.buffer(b).ok()?.text;
            let len = text.len();
            let output = match it.output {
                Some((a, end)) => {
                    let a = a.min(len);
                    let z = end.unwrap_or(len).clamp(a, len);
                    RunOutput::Text(text.slice(a, z))
                }
                None => RunOutput::None,
            };
            Some(Run { cmd: it.title.clone(), output })
        }
        Body::Term(t) => {
            let term = node.state.terms.get(&t)?;
            let m = term.marks.iter().filter(|m| m.started.is_some() || m.cmd.is_some()).nth(i)?;
            let from = m.output.unwrap_or(m.prompt + 1);
            // to the line before its end (where the next prompt is), else,
            // still running, to the last line there is
            let to = match m.end {
                Some(e) => e.checked_sub(1),
                None => term.total.checked_sub(1),
            };
            let output = match to {
                Some(to) if to >= from => RunOutput::Rows { term: t, from, to },
                _ => RunOutput::None,
            };
            Some(Run { cmd: m.cmd.clone().unwrap_or_default(), output })
        }
        Body::Page(_) => None,
    }
}

/// A command and its output as copied: `$ ` and the command, a line,
/// then the output, its trailing blank lines left off, ending in a
/// newline (whatever the shell's prompt was).
pub fn transcript(cmd: &str, output: &str) -> String {
    let out = output.trim_end_matches(|c: char| c == '\n' || c == '\r' || c == ' ' || c == '\t');
    if out.is_empty() {
        format!("$ {cmd}\n")
    } else {
        format!("$ {cmd}\n{out}\n")
    }
}

/// Which of window `w`'s `entries` it is showing, as best the state
/// says: the last at or before the top of a text's or terminal's view.
/// A page's scroll is the client's alone, so `None` for a page.
pub fn current(node: &Node, w: WindowId, entries: &[Entry]) -> Option<usize> {
    let win = node.state.window(w).ok()?;
    match win.body {
        Body::Text(b) => {
            let buf = node.state.buffer(b).ok()?;
            let origin = buf.views.get(&ViewId::Body(w)).map(|v| v.origin).unwrap_or(0).min(buf.text.len());
            let top = buf.text.line_of(origin);
            // the first entry on the line at the top counts as shown
            let first = entries.iter().position(|e| matches!(e.at, At::Text(a) if buf.text.line_of(a.min(buf.text.len())) >= top));
            match first {
                Some(i) if matches!(entries[i].at, At::Text(a) if buf.text.line_of(a.min(buf.text.len())) == top) => Some(i),
                Some(0) => Some(0),
                Some(i) => Some(i - 1),
                None => entries.len().checked_sub(1),
            }
        }
        Body::Term(t) => {
            let term = node.state.terms.get(&t)?;
            // the screen's end: the last command
            if term.top + term.rows as u64 >= term.total {
                return entries.len().checked_sub(1);
            }
            entries.iter().rposition(|e| matches!(e.at, At::Term(l) if l <= term.top)).or(Some(0))
        }
        Body::Page(_) => None,
    }
}

/// A page's outline from its HTML, as `TOC_SCRIPT` finds it in the
/// document: the marked elements' (level, title), else the headings'.
pub fn page(html: &str) -> Vec<(u8, String)> {
    let mut open: Vec<String> = Vec::new();
    let mut caps: Vec<Capture> = Vec::new();
    // (the order it opened in, level, title), of each kind
    let mut marked: Vec<(usize, u8, String)> = Vec::new();
    let mut heads: Vec<(usize, u8, String)> = Vec::new();
    let mut opened = 0;
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < html.len() {
        let Some(lt) = html[i..].find('<').map(|k| i + k) else {
            text_into(&html[i..], &mut caps);
            break;
        };
        text_into(&html[i..lt], &mut caps);
        let rest = &html[lt..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map(|k| lt + k + 3).unwrap_or(html.len());
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            i = rest.find('>').map(|k| lt + k + 1).unwrap_or(html.len());
            continue;
        }
        let closing = bytes.get(lt + 1) == Some(&b'/');
        let name_start = lt + 1 + closing as usize;
        let name_len = html[name_start..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).unwrap_or(html.len() - name_start);
        if name_len == 0 {
            // a `<` that starts no tag: text
            text_into("<", &mut caps);
            i = lt + 1;
            continue;
        }
        let name = html[name_start..name_start + name_len].to_ascii_lowercase();
        let end = tag_end(html, name_start + name_len);
        let inner = &html[name_start + name_len..end.saturating_sub(1).max(name_start + name_len)];
        i = end;
        if closing {
            if let Some(k) = open.iter().rposition(|n| *n == name) {
                open.truncate(k);
                while caps.last().is_some_and(|c| c.depth > open.len()) {
                    caps.pop().unwrap().finish(&mut marked, &mut heads);
                }
                for c in caps.iter_mut() {
                    if c.name_at.is_some_and(|d| d > open.len()) {
                        c.name_at = None;
                    }
                }
            }
            continue;
        }
        let attrs = attrs(inner);
        let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let void = matches!(name.as_str(), "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input" | "link" | "meta" | "source" | "track" | "wbr") || inner.trim_end().ends_with('/');
        if matches!(name.as_str(), "script" | "style" | "template" | "textarea" | "title") && !void {
            // their text is no heading's
            let close = format!("</{name}");
            i = html[i..].to_ascii_lowercase().find(&close).map(|k| i + k).unwrap_or(html.len());
            i = html[i..].find('>').map(|k| i + k + 1).unwrap_or(html.len());
            continue;
        }
        if void {
            continue;
        }
        open.push(name.clone());
        let depth = open.len();
        if get("class").is_some_and(|c| c.split_whitespace().any(|c| c == "name")) {
            for c in caps.iter_mut().filter(|c| c.name.is_none()) {
                c.name_at = Some(depth);
                c.name = Some(String::new());
            }
        }
        let new = |level, marked, title| Capture { order: opened, depth, level, marked, title, text: String::new(), name_at: None, name: None };
        if let Some(l) = get("data-apex-outline") {
            let level = l.trim().parse::<u8>().unwrap_or(1).clamp(1, 4);
            caps.push(new(level, true, get("data-apex-title").map(str::to_string)));
            opened += 1;
        } else if let Some(level) = name.strip_prefix('h').and_then(|d| d.parse::<u8>().ok()).filter(|l| (1..=4).contains(l)) {
            caps.push(new(level, false, None));
            opened += 1;
        }
    }
    while let Some(c) = caps.pop() {
        c.finish(&mut marked, &mut heads);
    }
    // in the document's order: one inside another finished first
    let mut out = if marked.is_empty() { heads } else { marked };
    out.sort_by_key(|(o, _, _)| *o);
    out.into_iter().map(|(_, l, t)| (l, t)).collect()
}

/// An element of the outline's, open: what it says so far.
struct Capture {
    order: usize,
    /// How many elements were open with it: it closes when fewer are.
    depth: usize,
    level: u8,
    marked: bool,
    title: Option<String>,
    text: String,
    /// Inside it, an element of class `name` (a diff's file's name),
    /// open at this depth, and what it said.
    name_at: Option<usize>,
    name: Option<String>,
}

impl Capture {
    fn finish(self, marked: &mut Vec<(usize, u8, String)>, heads: &mut Vec<(usize, u8, String)>) {
        if self.marked {
            let title = squash(self.title.as_deref().unwrap_or(""));
            let title = if title.is_empty() { squash(&self.text) } else { title };
            if !title.is_empty() {
                marked.push((self.order, self.level, title));
            }
        } else {
            let label = squash(self.name.as_deref().unwrap_or(&self.text));
            if !label.is_empty() {
                heads.push((self.order, self.level, label));
            }
        }
    }
}

/// Text between tags, to every capture open, and to the name inside one.
fn text_into(raw: &str, caps: &mut [Capture]) {
    if raw.is_empty() || caps.is_empty() {
        return;
    }
    let t = unescape(raw);
    for c in caps.iter_mut() {
        c.text.push_str(&t);
        if let (Some(_), Some(n)) = (c.name_at, c.name.as_mut()) {
            n.push_str(&t);
        }
    }
}

/// Where the tag that starts at `from` ends (past its `>`), quotes kept
/// together.
fn tag_end(html: &str, from: usize) -> usize {
    let mut quote = None;
    for (k, c) in html[from..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return from + k + 1,
            _ => {}
        }
    }
    html.len()
}

/// A tag's attributes, names lowercased, values unquoted and unescaped.
fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    let b = s;
    while let Some(&(k, c)) = it.peek() {
        if c.is_whitespace() || c == '/' {
            it.next();
            continue;
        }
        let start = k;
        while it.peek().is_some_and(|&(_, c)| !c.is_whitespace() && c != '=' && c != '/') {
            it.next();
        }
        let end = it.peek().map(|&(k, _)| k).unwrap_or(b.len());
        let name = b[start..end].to_ascii_lowercase();
        while it.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
            it.next();
        }
        if it.peek().is_some_and(|&(_, c)| c == '=') {
            it.next();
            while it.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
                it.next();
            }
            let value = match it.peek().copied() {
                Some((k, q)) if q == '"' || q == '\'' => {
                    it.next();
                    let mut e = b.len();
                    while let Some((j, c)) = it.next() {
                        if c == q {
                            e = j;
                            break;
                        }
                    }
                    &b[k + 1..e.max(k + 1)]
                }
                Some((k, _)) => {
                    while it.peek().is_some_and(|&(_, c)| !c.is_whitespace()) {
                        it.next();
                    }
                    &b[k..it.peek().map(|&(j, _)| j).unwrap_or(b.len())]
                }
                None => "",
            };
            out.push((name, unescape(value)));
        } else {
            out.push((name, String::new()));
        }
    }
    out
}

/// HTML's character references, the usual named ones and numbers.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(k) = rest.find('&') {
        out.push_str(&rest[..k]);
        rest = &rest[k..];
        let semi = rest[..rest.len().min(12)].find(';');
        let decoded = semi.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            }?;
            Some((c, e + 1))
        });
        match decoded {
            Some((c, n)) => {
                out.push(c);
                rest = &rest[n..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whitespace runs one space, the ends trimmed: as `textContent` reads
/// once `TOC_SCRIPT` has squashed it.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transcript_is_dollar_the_command_then_its_output() {
        assert_eq!(transcript("make; ls", "built\nok\n\n  \n"), "$ make; ls\nbuilt\nok\n");
        assert_eq!(transcript("true", ""), "$ true\n");
        assert_eq!(transcript("echo", "no newline"), "$ echo\nno newline\n");
    }

    #[test]
    fn headings() {
        let html = "<h1 id=a>Title &amp; more</h1><p>x</p><h2>Sub <code>part</code></h2><h5>deep</h5><h3>  </h3><h4>Four</h4>";
        assert_eq!(page(html), vec![(1, "Title & more".into()), (2, "Sub part".into()), (4, "Four".into())]);
    }

    #[test]
    fn marked_elements_win() {
        let html = r#"<h1>Head</h1><section class="ax-turn" data-apex-outline="1" data-apex-title="fix the &quot;bug&quot;"><h2>inner</h2></section><div data-apex-outline="2">untitled  text</div>"#;
        assert_eq!(page(html), vec![(1, "fix the \"bug\"".into()), (2, "untitled text".into())]);
    }

    #[test]
    fn a_diffs_file_by_its_name() {
        let html = r#"<section class="file"><h3><span class="name">src/a.rs</span> <span class="stat">+3 −1</span></h3></section>"#;
        assert_eq!(page(html), vec![(3, "src/a.rs".into())]);
    }

    #[test]
    fn scripts_and_comments_are_no_headings() {
        let html = "<script>var s = '<h1>no</h1>';</script><!-- <h2>no</h2> --><style>h1{}</style><h2 class='x'>yes</h2><br/><img src=a.png>";
        assert_eq!(page(html), vec![(2, "yes".into())]);
    }

    #[test]
    fn nested_in_document_order() {
        let html = r#"<section data-apex-outline="1" data-apex-title="outer"><div data-apex-outline="2" data-apex-title="inner"></div></section><p data-apex-outline=1 data-apex-title=last></p>"#;
        assert_eq!(page(html), vec![(1, "outer".into()), (2, "inner".into()), (1, "last".into())]);
    }

    #[test]
    fn unclosed_at_the_end() {
        assert_eq!(page("<h2>open"), vec![(2, "open".into())]);
    }
}
