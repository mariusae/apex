//! `apex tool preview FILE` (WEB.md §3): the file's buffer, piped through
//! the converter its extension names in the settings (`Preview.md`),
//! shown as a page in a preview window at FILE beside it, and kept
//! so as the buffer changes: live from the text, not the file, so
//! unsaved edits show; and scrolled to follow the file's caret, by the
//! page's scroll in the log (the converter marks its blocks with their
//! source lines, `data-line`). The tool ends with either window.
//!
//! The page is the tool's, and so are its links: one to a file Preview
//! converts opens that file's preview (at the link's line), one to any
//! other file plumbs it, as B3 on its name would; others are the
//! client's.
//!
//! Unprivileged: it attaches like anything else, reads the buffer from
//! the entry stream, and writes the page through proposals.
//!
//! `apex tool preview`, with no file, is the resident tool a session's
//! rules start when Preview is first used (ARCHITECTURE.md §5): it runs
//! a preview, as above, for each file the verb is used on, adds the
//! session a rule for each extension a setting gives a converter beyond
//! the default rule's, and goes when no preview has been open a while.

pub mod converters;
pub mod markdown;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::proto::{Answer, ClientMsg, NavAnswer, Request, ServerMsg};
use apex_server::remote::Remote;
use apex_server::Proposal;

const TIMEOUT: Duration = Duration::from_secs(10);
/// How long the source must be quiet before a render.
const SETTLE: Duration = Duration::from_millis(250);

fn debug() -> bool {
    std::env::var_os("APEX_PREVIEW_DEBUG").is_some()
}

pub fn run(socket: &Path, session: &str, file: &str) -> Result<(), String> {
    run_at(socket, session, file, None)
}

/// `run`, the source's caret put at `line` (a link's place in it).
pub fn run_at(socket: &Path, session: &str, file: &str, line: Option<usize>) -> Result<(), String> {
    let file = std::path::absolute(file).map_err(|e| format!("{file}: {e}"))?.display().to_string();
    if debug() {
        eprintln!("preview: {file}");
    }
    let ext = Path::new(&file).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // its own attachment, not the resident tool's name: the Preview verb's
    // rules name that one, and a plumb must go to it alone
    let remote = Remote::connect_as(socket, session, "preview-file", AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
    remote.announce("preview");
    let Some(converter) = converters::converter(&remote.node.state.meta, &ext) else {
        return Err(format!("Preview: no converter for .{ext} files: apex set Preview.{ext} CMD (a command reading the file on stdin, writing HTML)"));
    };
    let mut t = Tool { remote, socket: socket.to_path_buf(), session: session.to_string(), file, line, converter, source: None, page: None, dirty: true, last_edit: Instant::now(), rendered: String::new(), followed: None, linked: Vec::new() };
    t.start()?;
    let r = t.main_loop();
    // the previews its links opened go on: this one is not done till they are
    for h in std::mem::take(&mut t.linked) {
        let _ = h.join();
    }
    r
}

struct Tool {
    remote: Remote,
    socket: std::path::PathBuf,
    session: String,
    file: String,
    /// Where the source's caret goes when it opens (a link's line).
    line: Option<usize>,
    converter: String,
    /// The source window and its buffer, once found.
    source: Option<(WindowId, BufferId)>,
    /// The preview window and its buffer.
    page: Option<(WindowId, BufferId)>,
    dirty: bool,
    last_edit: Instant,
    /// The HTML last written, to diff the next against.
    rendered: String,
    /// The source line the page was last scrolled to.
    followed: Option<u32>,
    /// The previews the page's links opened, on threads of their own.
    linked: Vec<std::thread::JoinHandle<()>>,
}

impl Tool {
    /// The file's own window at `path` (not its preview).
    fn window_named(&self, path: &str) -> Option<WindowId> {
        self.remote.node.window_named(path).filter(|w| self.remote.node.window_kind(*w) != WinKind::Page)
    }

    /// The file's preview, if it has one.
    fn preview_window(&self) -> Option<WindowId> {
        self.remote.node.window_of(&self.file, WinKind::Page)
    }

    /// One message, through `before` first; false when the link ended.
    fn step(&mut self, timeout: Duration) -> bool {
        match self.remote.link.rx.recv_timeout(timeout) {
            Ok(m) => {
                self.before(&m);
                self.remote.handle(m)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => true,
            Err(_) => false,
        }
    }

    fn propose(&mut self, p: Proposal, timeout: Duration) -> Result<Option<WindowId>, String> {
        let id = self.remote.link.propose(p);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(r) = self.remote.link.applied.remove(&id) {
                return r;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("timed out waiting for the leader".into());
            }
            if !self.step(left.min(Duration::from_millis(50))) {
                return Err("connection closed".into());
            }
        }
    }

    /// Wait for a window by name, up to `timeout`.
    fn await_window(&mut self, name: &str, timeout: Duration) -> Result<WindowId, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(w) = self.window_named(name) {
                return Ok(w);
            }
            if Instant::now() >= deadline {
                return Err(format!("{name}: not open"));
            }
            if !self.step(Duration::from_millis(50)) {
                return Err("connection closed".into());
            }
        }
    }

    /// The source (opened if need be), the page (found or made), the
    /// first render.
    fn start(&mut self) -> Result<(), String> {
        // the file's window, opened when it is not (at the line asked for)
        let at = self.line.map(Pos::Line).unwrap_or(Pos::Keep);
        if let (Some(_), Some(_)) = (self.line, self.window_named(&self.file)) {
            let _ = self.propose(Proposal::Goto { loc: Loc { session: None, name: self.file.clone(), pos: at.clone() } }, TIMEOUT);
        }
        let src = match self.window_named(&self.file) {
            Some(w) => w,
            None => {
                let loc = Loc { session: None, name: self.file.clone(), pos: at };
                let r = self.propose(Proposal::Goto { loc }, TIMEOUT);
                if debug() {
                    eprintln!("preview: goto answered {r:?}");
                }
                r?;
                let w = self.await_window(&self.file.clone(), TIMEOUT)?;
                if debug() {
                    eprintln!("preview: source window {w}");
                }
                w
            }
        };
        let src_buf = self.remote.node.state.window(src).map_err(|e| e.to_string())?.body_buffer().ok_or("not a text window")?;
        self.source = Some((src, src_buf));
        // a preview someone else keeps: show it and be done
        if let Some(w) = self.preview_window() {
            if self.remote.node.window_live(w) {
                let loc = Loc { session: None, name: w.0.to_string(), pos: Pos::Keep };
                let _ = self.propose(Proposal::Goto { loc }, TIMEOUT);
                return Err("shown".into());
            }
        }
        // ours: beside the source (the next column, else its own)
        let cols = &self.remote.node.state.layout.cols;
        let ci = self.remote.node.column_of(src).ok().and_then(|c| cols.iter().position(|x| x.id == c)).unwrap_or(0);
        let col = cols.get(ci + 1).or(cols.get(ci)).map(|c| c.id).ok_or("no column")?;
        let page = match self.preview_window() {
            Some(w) => w,
            None => self.propose(Proposal::open_html(col, &self.file, "", None), TIMEOUT)?.ok_or("no preview window")?,
        };
        let page_buf = self.remote.node.state.window(page).map_err(|e| e.to_string())?.body_buffer().ok_or("not a text window")?;
        if debug() {
            eprintln!("preview: page window {page}");
        }
        self.page = Some((page, page_buf));
        self.rendered = self.remote.node.state.buffer(page_buf).map(|b| b.text.to_string()).unwrap_or_default();
        let me = self.remote.attachment();
        let _ = self.propose(Proposal::Live { window: page, by: Some(me) }, TIMEOUT);
        // the page ours, and where its links go ours to say
        let _ = self.propose(Proposal::Own { window: page, by: Some(me) }, TIMEOUT);
        self.remote.link.answers_navigation = true;
        self.render_or_say();
        Ok(())
    }

    /// Where the page's links go: a file Preview converts, its preview,
    /// on a thread of this process (the page's link a place in it: the
    /// source's caret there, the preview following); any other file,
    /// plumbed, as B3 on its name; anything else, the client's to follow.
    fn answer_links(&mut self) {
        let Some((page, _)) = self.page else { return };
        for (id, request) in std::mem::take(&mut self.remote.link.asks) {
            let Request::Navigate { window, url } = request else { continue };
            let answer = match apex_server::plane::host_file(&url).filter(|_| window == page) {
                Some((path, line)) => {
                    let ext = Path::new(&path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    if converters::converter(&self.remote.node.state.meta, &ext).is_some() {
                        let (socket, session) = (self.socket.clone(), self.session.clone());
                        self.linked.push(std::thread::spawn(move || {
                            if let Err(e) = run_at(&socket, &session, &path, line) {
                                if e != "shown" {
                                    eprintln!("preview: {path}: {e}");
                                }
                            }
                        }));
                    } else {
                        let text = match line {
                            Some(n) => format!("{path}:{n}"),
                            None => path,
                        };
                        let dir = Path::new(&self.file).parent().map(|d| d.display().to_string());
                        self.remote.send(&ClientMsg::Plumb { ctx: ExecCtx::Window(page), text, dir, edit_only: false, dry: false, at: None, sel: None, alt: None, reverse: false, verb: None });
                    }
                    NavAnswer::Handled
                }
                None => NavAnswer::Default,
            };
            self.remote.answer(id, Answer::Navigate(answer));
        }
    }

    fn main_loop(&mut self) -> Result<(), String> {
        loop {
            if !self.step(Duration::from_millis(50)) {
                return Ok(());
            }
            while let Ok(m) = self.remote.link.rx.try_recv() {
                self.before(&m);
                if !self.remote.handle(m) {
                    return Ok(());
                }
            }
            let (Some((src, _)), Some((page, _))) = (self.source, self.page) else { return Ok(()) };
            if self.remote.node.state.window(src).is_err() || self.remote.node.state.window(page).is_err() {
                // either window went: so do we (the page's live mark with us)
                if self.remote.node.state.window(page).is_ok() {
                    let _ = self.propose(Proposal::Live { window: page, by: None }, TIMEOUT);
                }
                return Ok(());
            }
            if self.dirty && self.last_edit.elapsed() >= SETTLE {
                self.render_or_say();
            }
            self.answer_links();
            self.follow();
        }
    }

    /// The page scrolled to the line the file's caret is on, when it has
    /// moved to another: a proposal, so every client showing the page
    /// scrolls it there.
    fn follow(&mut self) {
        let (Some((src, src_buf)), Some((page, _))) = (self.source, self.page) else { return };
        let node = &self.remote.node;
        let Some(line) = node.selection(ViewId::Body(src)).ok().and_then(|(q0, _)| node.state.buffer(src_buf).ok().map(|b| b.text.line_of(q0.min(b.text.len())) as u32 + 1)) else { return };
        if self.followed == Some(line) {
            return;
        }
        self.followed = Some(line);
        let _ = self.propose(Proposal::PageScroll { window: page, scroll: Some(Scroll::Line(line)) }, TIMEOUT);
    }

    /// The source's entries: any change means a render once it settles.
    fn before(&mut self, m: &ServerMsg) {
        let Some((_, src_buf)) = self.source else { return };
        if let ServerMsg::Entries { shard: Shard::Buffer(b), .. } = m {
            if *b == src_buf {
                self.dirty = true;
                self.last_edit = Instant::now();
            }
        }
    }

    /// A render, or what kept it from being one said in the file's
    /// directory's errors window (a diagnostic one: a toast) -- the page
    /// left as it was, and the preview going on, the next edit another try.
    fn render_or_say(&mut self) {
        if let Err(e) = self.render() {
            let dir = Path::new(&self.file).parent().map(|d| d.display().to_string());
            let text = format!("preview: {}: {}\n", self.file, e.trim_end());
            let _ = self.propose(Proposal::Errors { dir, text }, TIMEOUT);
        }
    }

    /// The source through the converter, into the page as a minimal diff.
    fn render(&mut self) -> Result<(), String> {
        self.dirty = false;
        let (Some((_, src_buf)), Some((_, page_buf))) = (self.source, self.page) else { return Ok(()) };
        let text = self.remote.node.state.buffer(src_buf).map(|b| b.text.to_string()).unwrap_or_default();
        let dir = Path::new(&self.file).parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let html = convert(&self.converter, &dir, &text)?;
        if html == self.rendered {
            return Ok(());
        }
        // the part that changed, as chars
        let old: Vec<char> = self.rendered.chars().collect();
        let new: Vec<char> = html.chars().collect();
        let prefix = old.iter().zip(new.iter()).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
        let (q0, q1) = (prefix, old.len() - suffix);
        let middle: String = new[prefix..new.len() - suffix].iter().collect();
        for _ in 0..5 {
            let Some((version, whole)) = self.remote.node.state.buffer(page_buf).ok().map(|b| (b.version, b.text.len())) else { return Ok(()) };
            if self.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: page_buf, version, q0, q1, text: middle.clone() }, TIMEOUT).is_ok() {
                self.rendered = html;
                return Ok(());
            }
            // the page moved under us (someone edited it): the whole thing then
            let Some(version) = self.remote.node.state.buffer(page_buf).ok().map(|b| b.version) else { return Ok(()) };
            if self.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: page_buf, version, q0: 0, q1: whole, text: html.clone() }, TIMEOUT).is_ok() {
                self.rendered = html;
                return Ok(());
            }
        }
        Err("the page would not take the render".into())
    }
}

/// `text` through `converter` (a command for the shell commands run
/// with) in `dir`: its HTML, or its complaint.
fn convert(converter: &str, dir: &Path, text: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new(apex_server::command_shell())
        .arg("-c")
        .arg(converter)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{converter}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let text = text.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    let out = child.wait_with_output().map_err(|e| format!("{converter}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{converter}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The resident tool (`apex tool preview`): the Preview verb offered on
/// each extension a converter exists for, kept in step with the
/// settings, and a preview run for each file it is used on -- each its
/// own attachment (`preview-file`), on a thread of this process.
pub fn run_resident(socket: &Path, session: &str) -> Result<(), String> {
    use apex_tool::Event;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut t = apex_tool::Tool::attach_to(socket, session, "preview").map_err(|e| e.to_string())?;
    let open = std::sync::Arc::new(AtomicUsize::new(0));
    let mut idle_since = Some(Instant::now());
    let mut offered = std::collections::BTreeSet::new();
    loop {
        lasting_rules(&mut t, &mut offered);
        match t.next_event(Some(Duration::from_millis(500))) {
            Ok(Some(Event::Plumb(p))) => {
                let file = p.window.and_then(|w| t.window(w)).map(|w| w.path).unwrap_or_default();
                let ext = Path::new(&file).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                if file.is_empty() || converters::converter(t.meta(), &ext).is_none() {
                    let _ = t.answer(&p, false);
                    continue;
                }
                let _ = t.answer(&p, true);
                let (socket, session, open) = (socket.to_path_buf(), session.to_string(), open.clone());
                open.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    if let Err(e) = run(&socket, &session, &file) {
                        if e != "shown" {
                            eprintln!("preview: {file}: {e}");
                        }
                    }
                    open.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(e) if e.is_closed() => return Ok(()),
            _ => {}
        }
        // gone when no preview is open for a while: the session's rule
        // starts it again (ARCHITECTURE.md §5)
        match (open.load(Ordering::SeqCst), idle_since) {
            (0, None) => idle_since = Some(Instant::now()),
            (0, Some(at)) if at.elapsed() >= IDLE => return Ok(()),
            (0, Some(_)) => {}
            _ => idle_since = None,
        }
    }
}

/// How long the resident Preview stays with no preview open.
const IDLE: Duration = Duration::from_secs(60);

/// The session's Preview rules for what a setting adds (`Preview.EXT`)
/// beyond the default rule's formats: installed as the session's, with
/// the default's `start`, so the verb is there whether or not Preview
/// runs. Being the session's, they stay when the setting goes (Preview
/// then declines the file), for `apex plumb rule rm` to take out: no
/// tool removes a rule not its own. `offered` are those asked for here,
/// not to be asked for twice before the log has them.
fn lasting_rules(t: &mut apex_tool::Tool, offered: &mut std::collections::BTreeSet<String>) {
    use apex_core::entry::RuleAction;
    let wanted: Vec<String> = converters::exts(t.meta())
        .into_iter()
        .filter(|e| !converters::DEFAULTS.iter().any(|(d, _)| d == e))
        .collect();
    let mut have = offered.clone();
    for r in t.meta().rules.values() {
        let r = &r.rule;
        if r.verb != "Preview" || r.action != RuleAction::Tool("preview".into()) || r.start.is_none() {
            continue;
        }
        if let Some(e) = r.file.as_deref().and_then(converters::ext_of_pattern) {
            have.insert(e);
        }
    }
    for ext in wanted.into_iter().filter(|e| !have.contains(e)) {
        let rule = apex_tool::Rule::verb("Preview")
            .file(&converters::pattern_of_ext(&ext))
            .kind(WinKind::File)
            .priority(-10)
            .start("apex tool preview");
        if t.offer_lasting(rule).is_ok() {
            offered.insert(ext);
        }
    }
}
