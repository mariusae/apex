//! `apex tool preview FILE` (WEB.md §3): the file's buffer, piped through
//! the converter its extension names in the settings (`Preview.md`),
//! shown as a page in a window named `FILE+Preview` beside it, and kept
//! so as the buffer changes: live from the text, not the file, so
//! unsaved edits show. The tool ends with either window.
//!
//! Unprivileged: it attaches like anything else, reads the buffer from
//! the entry stream, and writes the page through proposals.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::proto::ServerMsg;
use apex_server::remote::Remote;
use apex_server::Proposal;

const TIMEOUT: Duration = Duration::from_secs(10);
/// How long the source must be quiet before a render.
const SETTLE: Duration = Duration::from_millis(250);

fn debug() -> bool {
    std::env::var_os("APEX_PREVIEW_DEBUG").is_some()
}

pub fn run(socket: &Path, session: &str, file: &str) -> Result<(), String> {
    let file = std::path::absolute(file).map_err(|e| format!("{file}: {e}"))?.display().to_string();
    if debug() {
        eprintln!("preview: {file}");
    }
    let ext = Path::new(&file).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let remote = Remote::connect_as(socket, session, "preview", AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
    remote.announce("preview");
    let Some(converter) = apex_core::preview::converter(&remote.node.state.meta, &ext) else {
        return Err(format!("Preview: no converter for .{ext} files: apex set Preview.{ext} CMD (a command reading the file on stdin, writing HTML)"));
    };
    let mut t = Tool { remote, file, converter, source: None, page: None, dirty: true, last_edit: Instant::now(), rendered: String::new() };
    t.start()?;
    t.main_loop()
}

struct Tool {
    remote: Remote,
    file: String,
    converter: String,
    /// The source window and its buffer, once found.
    source: Option<(WindowId, BufferId)>,
    /// The preview window and its buffer.
    page: Option<(WindowId, BufferId)>,
    dirty: bool,
    last_edit: Instant,
    /// The HTML last written, to diff the next against.
    rendered: String,
}

impl Tool {
    fn window_named(&self, name: &str) -> Option<WindowId> {
        self.remote.node.state.windows.keys().copied().find(|w| self.remote.node.window_name(*w) == name)
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
        // the file's window, opened when it is not
        let src = match self.window_named(&self.file) {
            Some(w) => w,
            None => {
                let loc = Loc { name: self.file.clone(), pos: Pos::Keep };
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
        let name = apex_core::preview::preview_name(&self.file);
        if let Some(w) = self.window_named(&name) {
            if self.remote.node.window_live(w) {
                let loc = Loc { name: name.clone(), pos: Pos::Keep };
                let _ = self.propose(Proposal::Goto { loc }, TIMEOUT);
                return Err("shown".into());
            }
        }
        // ours: beside the source (the next column, else its own)
        let cols = &self.remote.node.state.layout.cols;
        let ci = self.remote.node.column_of(src).ok().and_then(|c| cols.iter().position(|x| x.id == c)).unwrap_or(0);
        let col = cols.get(ci + 1).or(cols.get(ci)).map(|c| c.id).ok_or("no column")?;
        let page = match self.window_named(&name) {
            Some(w) => w,
            None => self.propose(Proposal::OpenHtml { col, name: name.clone(), text: String::new() }, TIMEOUT)?.ok_or("no preview window")?,
        };
        let page_buf = self.remote.node.state.window(page).map_err(|e| e.to_string())?.body_buffer().ok_or("not a text window")?;
        if debug() {
            eprintln!("preview: page window {page}");
        }
        self.page = Some((page, page_buf));
        self.rendered = self.remote.node.state.buffer(page_buf).map(|b| b.text.to_string()).unwrap_or_default();
        let me = self.remote.attachment();
        let _ = self.propose(Proposal::Live { window: page, by: Some(me) }, TIMEOUT);
        self.render()?;
        Ok(())
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
                self.render()?;
            }
        }
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
            if self.propose(Proposal::ReplaceRange { dir: None, buffer: page_buf, version, q0, q1, text: middle.clone() }, TIMEOUT).is_ok() {
                self.rendered = html;
                return Ok(());
            }
            // the page moved under us (someone edited it): the whole thing then
            let Some(version) = self.remote.node.state.buffer(page_buf).ok().map(|b| b.version) else { return Ok(()) };
            if self.propose(Proposal::ReplaceRange { dir: None, buffer: page_buf, version, q0: 0, q1: whole, text: html.clone() }, TIMEOUT).is_ok() {
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
