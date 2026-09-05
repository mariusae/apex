//! The apex server: everything with an effect on the world. It hosts
//! terminals (pinned term shards), reads and writes files, runs external
//! commands and pipes, and performs the execs that name it as handler.
//!
//! In-process mode: the server shares the `Log` with the client and applies
//! results through the client's leader node directly. Over a socket the
//! same calls become proposals.

pub mod term;

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use alacritty_terminal::event::Event;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

use apex_core::node::ERRORS;
use apex_core::state::ExecStatus;
use apex_core::*;

pub use term::{TermHost, TermKey};

/// Something that happened off the main thread and needs the server's
/// attention on it.
pub enum ServerEvent {
    Term(TermId, Event),
    /// A shell command finished: `(ctx, exec seq, stdout, stderr, what to do with stdout)`.
    Shell { ctx: ExecCtx, exec: Seq, out: String, err: String, mode: ShellMode },
}

#[derive(Clone, Debug)]
pub enum ShellMode {
    /// Output goes to `+Errors`.
    Errors { col: ColumnId },
    /// Output replaces a range of a buffer (`|cmd`, `<cmd`).
    Replace { col: ColumnId, buffer: BufferId, version: Version, q0: usize, q1: usize },
}

pub struct Server {
    pub node: Node,
    terms: HashMap<TermId, TermHost>,
    tx: UnboundedSender<ServerEvent>,
    term_tx: UnboundedSender<(TermId, Event)>,
    /// Execs already performed, so a scan does not repeat them.
    performed: BTreeSet<(ExecCtx, Seq)>,
    pub cwd: PathBuf,
    next_term: u64,
}

impl Server {
    /// A server over `log`; returns the receiver the host must drain into
    /// [`Server::pump`].
    pub fn new(log: &Log) -> (Server, UnboundedReceiver<ServerEvent>) {
        let (tx, rx) = unbounded();
        let (term_tx, mut term_rx) = unbounded::<(TermId, Event)>();
        // forward terminal events into the one server channel
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                use futures::StreamExt;
                futures::executor::block_on(async move {
                    while let Some((id, ev)) = term_rx.next().await {
                        if tx.unbounded_send(ServerEvent::Term(id, ev)).is_err() {
                            break;
                        }
                    }
                });
            });
        }
        let mut node = Node::new(SERVER);
        node.catch_up(log).expect("fresh log");
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        (Server { node, terms: HashMap::new(), tx, term_tx, performed: BTreeSet::new(), cwd, next_term: 1 }, rx)
    }

    pub fn term(&self, id: TermId) -> Option<&TermHost> {
        self.terms.get(&id)
    }

    pub fn term_mut(&mut self, id: TermId) -> Option<&mut TermHost> {
        self.terms.get_mut(&id)
    }

    pub fn term_ids(&self) -> Vec<TermId> {
        self.terms.keys().copied().collect()
    }

    /// Directory a window works in: its file's directory, or the cwd.
    pub fn dir_of(&self, leader: &Node, ctx: ExecCtx) -> PathBuf {
        if let ExecCtx::Window(w) = ctx {
            if let Ok(win) = leader.state.window(w) {
                if let Body::Term(t) = win.body {
                    if let Some(h) = self.terms.get(&t) {
                        return h.dir.clone();
                    }
                }
            }
            let name = leader.window_name(w);
            let p = Path::new(&name);
            if name.ends_with('/') && p.is_dir() {
                return p.to_path_buf();
            }
            if let Some(parent) = p.parent() {
                if parent.is_dir() {
                    return parent.to_path_buf();
                }
            }
        }
        self.cwd.clone()
    }

    // ---- files -------------------------------------------------------------

    /// Read a file or directory listing; returns (display name, text).
    pub fn read_path(&self, path: &Path) -> std::io::Result<(String, String)> {
        let meta = std::fs::metadata(path)?;
        if meta.is_dir() {
            let mut entries: Vec<String> = std::fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .map(|e| {
                    let mut n = e.file_name().to_string_lossy().to_string();
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        n.push('/');
                    }
                    n
                })
                .collect();
            entries.sort();
            let mut d = path.to_string_lossy().to_string();
            if !d.ends_with('/') {
                d.push('/');
            }
            Ok((d, entries.join("\n") + "\n"))
        } else {
            let bytes = std::fs::read(path)?;
            Ok((path.to_string_lossy().to_string(), String::from_utf8_lossy(&bytes).to_string()))
        }
    }

    /// Open a file in a window of `col` through the leader, or return the
    /// window already showing it.
    pub fn open_file(&mut self, log: &mut Log, leader: &mut Node, col: ColumnId, dir: &Path, name: &str) -> Result<WindowId, String> {
        let path = resolve(dir, name);
        let (display, text) = self.read_path(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(w) = leader
            .state
            .windows
            .keys()
            .copied()
            .find(|w| leader.window_name(*w) == display)
        {
            return Ok(w);
        }
        let hash = Text::new(&text).content_hash();
        let b = leader.create_buffer(log, &display, &text, Some(hash)).map_err(|e| e.to_string())?;
        leader.open_window(log, col, b).map_err(|e| e.to_string())
    }

    fn put(&mut self, log: &mut Log, leader: &mut Node, w: WindowId, arg: Option<&str>) -> Result<(), String> {
        let (b, tag) = {
            let win = leader.state.window(w).map_err(|e| e.to_string())?;
            (win.body_buffer().ok_or("Put: not a text window")?, win.tag)
        };
        let (buf_name, text, version) = {
            let buf = leader.state.buffer(b).map_err(|e| e.to_string())?;
            (buf.name.clone(), buf.text.to_string(), buf.version)
        };
        let dir = self.dir_of(leader, ExecCtx::Window(w));
        let name = match arg {
            Some(a) => resolve(&dir, a).to_string_lossy().to_string(),
            None => buf_name,
        };
        if name.is_empty() || name.starts_with('+') || name.ends_with('/') {
            return Err("Put: no file name".into());
        }
        std::fs::write(&name, &text).map_err(|e| format!("{name}: {e}"))?;
        let hash = Text::new(&text).content_hash();
        if arg.is_some() {
            leader
                .append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Rename { name: name.clone() }))
                .map_err(|e| e.to_string())?;
            let rest = leader.state.buffer(tag).map(|t| t.text.to_string()).unwrap_or_default();
            let rest = rest.split_once(' ').map(|(_, r)| r.to_string()).unwrap_or_default();
            leader.set_content(log, tag, &format!("{name} {rest}")).map_err(|e| e.to_string())?;
        }
        leader
            .append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Clean { version, disk_hash: Some(hash) }))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn get(&mut self, log: &mut Log, leader: &mut Node, w: WindowId) -> Result<(), String> {
        let b = leader.state.window(w).map_err(|e| e.to_string())?.body_buffer().ok_or("Get: not a text window")?;
        let name = leader.state.buffer(b).map(|b| b.name.clone()).map_err(|e| e.to_string())?;
        let (_, text) = self.read_path(Path::new(&name)).map_err(|e| format!("{name}: {e}"))?;
        leader.set_content(log, b, &text).map_err(|e| e.to_string())?;
        let version = leader.state.buffer(b).map(|b| b.version).unwrap_or(0);
        let hash = Text::new(&text).content_hash();
        leader
            .append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Clean { version, disk_hash: Some(hash) }))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---- terminals ------------------------------------------------------------

    pub fn new_term(&mut self, log: &mut Log, leader: &mut Node, col: ColumnId, dir: &Path) -> Result<WindowId, String> {
        let id = TermId(self.next_term);
        self.next_term += 1;
        let host = TermHost::spawn(id, dir, 80, 24, self.term_tx.clone())?;
        self.node.create_shard(log, Shard::Term(id)).map_err(|e| e.to_string())?;
        self.node
            .append(log, Shard::Term(id), Op::Term(TermOp::Create { cols: 80, rows: 24 }))
            .map_err(|e| e.to_string())?;
        self.terms.insert(id, host);
        let name = format!("{}/-term", dir.display().to_string().trim_end_matches('/'));
        leader.catch_up(log).map_err(|e| e.to_string())?;
        leader.open_term_window(log, col, &name, id).map_err(|e| e.to_string())
    }

    /// Publish a terminal's current grid to its shard.
    pub fn publish_term(&mut self, log: &mut Log, id: TermId) {
        let Some(h) = self.terms.get(&id) else { return };
        let ops = h.snapshot_ops();
        for op in ops {
            let _ = self.node.append(log, Shard::Term(id), Op::Term(op));
        }
    }

    pub fn term_key(&mut self, id: TermId, key: &TermKey) {
        if let Some(h) = self.terms.get(&id) {
            h.key(key);
        }
    }

    pub fn term_paste(&mut self, id: TermId, text: &str) {
        if let Some(h) = self.terms.get(&id) {
            h.paste(text);
        }
    }

    pub fn term_resize(&mut self, log: &mut Log, id: TermId, cols: u16, rows: u16) {
        if let Some(h) = self.terms.get_mut(&id) {
            if cols != h.cols || rows != h.rows {
                h.resize(cols, rows);
                let _ = self.node.append(log, Shard::Term(id), Op::Term(TermOp::Resize { cols, rows }));
                self.publish_term(log, id);
            }
        }
    }

    pub fn term_scroll(&mut self, log: &mut Log, id: TermId, delta: isize) {
        if let Some(h) = self.terms.get_mut(&id) {
            h.scroll(delta);
            self.publish_term(log, id);
        }
    }

    /// Kill a terminal (its window was deleted).
    pub fn close_term(&mut self, log: &mut Log, id: TermId) {
        self.terms.remove(&id);
        let _ = self.node.delete_shard(log, Shard::Term(id));
    }

    // ---- events and execs -------------------------------------------------------

    /// Handle one event from the receiver returned by [`Server::new`].
    pub fn pump(&mut self, log: &mut Log, leader: &mut Node, ev: ServerEvent) {
        match ev {
            ServerEvent::Term(id, ev) => {
                let Some(h) = self.terms.get_mut(&id) else { return };
                match ev {
                    Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Title(_) | Event::ResetTitle | Event::Bell => {}
                    Event::PtyWrite(s) => h.write(s.as_bytes()),
                    Event::ColorRequest(i, fmt) => h.write(fmt(term::default_color(i)).as_bytes()),
                    Event::TextAreaSizeRequest(fmt) => h.write(fmt(h.window_size()).as_bytes()),
                    Event::ClipboardStore(..) | Event::ClipboardLoad(..) => {}
                    Event::Exit | Event::ChildExit(_) => {
                        h.exited = true;
                        let _ = self.node.append(log, Shard::Term(id), Op::Term(TermOp::Exit { status: 0 }));
                    }
                }
                self.publish_term(log, id);
            }
            ServerEvent::Shell { ctx, exec, out, err, mode } => {
                let _ = leader.catch_up(log);
                match mode {
                    ShellMode::Errors { col } => {
                        if !out.is_empty() {
                            let _ = leader.errors(log, col, &out);
                        }
                    }
                    ShellMode::Replace { col, buffer, version, q0, q1 } => {
                        let ok = leader.state.buffer(buffer).map(|b| b.version == version).unwrap_or(false);
                        if ok {
                            let group_view = leader.state.buffer(buffer).ok().and_then(|b| b.views.keys().next().copied());
                            if let Some(v) = group_view {
                                let _ = leader.select(log, v, q0, q1);
                                let _ = leader.replace_selection(log, v, &out);
                            }
                        } else {
                            let _ = leader.errors(log, col, &format!("pipe output not applied: buffer changed meanwhile\n{out}"));
                        }
                    }
                }
                if !err.is_empty() {
                    let col = match mode_col(&mode) {
                        Some(c) => c,
                        None => return,
                    };
                    let _ = leader.errors(log, col, &err);
                }
                let _ = leader.append_status(log, ctx, exec, ExecStatusOp::Done);
            }
        }
        let _ = leader.catch_up(log);
    }

    /// Perform every pending exec addressed to the server, through the
    /// leader. Returns how many were started.
    pub fn poll_execs(&mut self, log: &mut Log, leader: &mut Node) -> usize {
        let mut todo: Vec<(ExecCtx, Seq, String, ExecAt)> = Vec::new();
        for (w, win) in &leader.state.windows {
            for (seq, e) in &win.execs {
                if e.handler == Handler::Server && e.status == ExecStatus::Pending && !self.performed.contains(&(ExecCtx::Window(*w), *seq)) {
                    todo.push((ExecCtx::Window(*w), *seq, e.text.clone(), e.at));
                }
            }
        }
        for (seq, (ctx, e)) in &leader.state.layout.execs {
            if e.handler == Handler::Server && e.status == ExecStatus::Pending && !self.performed.contains(&(*ctx, *seq)) {
                todo.push((*ctx, *seq, e.text.clone(), e.at));
            }
        }
        let n = todo.len();
        for (ctx, seq, text, at) in todo {
            self.performed.insert((ctx, seq));
            match self.perform(log, leader, ctx, seq, &text, at) {
                Ok(true) => {
                    let _ = leader.append_status(log, ctx, seq, ExecStatusOp::Done);
                }
                Ok(false) => {} // asynchronous; status comes with the result
                Err(reason) => {
                    if let Ok(col) = column_of(leader, ctx) {
                        let _ = leader.errors(log, col, &format!("{reason}\n"));
                    }
                    let _ = leader.append_status(log, ctx, seq, ExecStatusOp::Failed(reason));
                }
            }
        }
        n
    }

    /// `Ok(true)` if finished now, `Ok(false)` if the result arrives later.
    fn perform(&mut self, log: &mut Log, leader: &mut Node, ctx: ExecCtx, seq: Seq, text: &str, at: ExecAt) -> Result<bool, String> {
        let dir = self.dir_of(leader, ctx);
        let col = column_of(leader, ctx)?;
        let win = match ctx {
            ExecCtx::Window(w) => Some(w),
            _ => None,
        };
        // pipes
        if let Some(rest) = text.strip_prefix('|').or_else(|| text.strip_prefix('<')).or_else(|| text.strip_prefix('>')) {
            let kind = text.as_bytes()[0];
            let input = at
                .buffer
                .and_then(|b| leader.state.buffer(b).ok())
                .map(|b| b.text.slice(at.q0, at.q1))
                .unwrap_or_default();
            let mode = match (kind, at.buffer) {
                (b'>', _) | (_, None) => ShellMode::Errors { col },
                (_, Some(b)) => ShellMode::Replace { col, buffer: b, version: at.version, q0: at.q0, q1: at.q1 },
            };
            let stdin = if kind == b'<' { None } else { Some(input) };
            self.spawn_shell(ctx, seq, rest.trim().to_string(), dir, stdin, mode);
            return Ok(false);
        }
        let mut words = text.split_whitespace();
        let cmd = words.next().unwrap_or("");
        let arg = words.next();
        match cmd {
            "Put" => {
                let w = win.ok_or("Put needs a window")?;
                self.put(log, leader, w, arg)?;
            }
            "Putall" => {
                let wins: Vec<WindowId> = leader.state.windows.keys().copied().collect();
                for w in wins {
                    let dirty = leader
                        .state
                        .window(w)
                        .ok()
                        .and_then(|x| x.body_buffer())
                        .and_then(|b| leader.state.buffer(b).ok())
                        .is_some_and(|b| b.dirty() && !b.name.is_empty() && !b.name.starts_with('+') && !b.name.ends_with('/'));
                    if dirty {
                        self.put(log, leader, w, None)?;
                    }
                }
            }
            "Get" => {
                let w = win.ok_or("Get needs a window")?;
                self.get(log, leader, w)?;
            }
            "New" => {
                let name = arg.ok_or("New needs a name here")?;
                let path = resolve(&dir, name);
                if path.exists() {
                    self.open_file(log, leader, col, &dir, name)?;
                } else {
                    leader.new_window(log, col, &path.to_string_lossy(), "").map_err(|e| e.to_string())?;
                }
            }
            "Newterm" => {
                self.new_term(log, leader, col, &dir)?;
            }
            "Kill" | "Dump" | "Load" | "Newweb" => return Err(format!("{cmd}: not implemented")),
            _ => {
                // anything else is a shell command; output goes to +Errors
                self.spawn_shell(ctx, seq, text.to_string(), dir, None, ShellMode::Errors { col });
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn spawn_shell(&self, ctx: ExecCtx, exec: Seq, cmd: String, dir: PathBuf, stdin: Option<String>, mode: ShellMode) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let (out, err) = shell(&cmd, &dir, stdin);
            let _ = tx.unbounded_send(ServerEvent::Shell { ctx, exec, out, err, mode });
        });
    }

    /// B3: open `text` as a file (with optional `:line`) if it names one.
    pub fn plumb_file(&mut self, log: &mut Log, leader: &mut Node, ctx: ExecCtx, text: &str) -> Option<WindowId> {
        let dir = self.dir_of(leader, ctx);
        let col = column_of(leader, ctx).ok()?;
        let candidates = [text.to_string(), text.trim_end_matches(['.', ',', ';', ':', ')']).to_string()];
        for cand in &candidates {
            let (path, line) = match cand.rsplit_once(':') {
                Some((p, l)) if !p.is_empty() && !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => {
                    (p.to_string(), l.parse::<usize>().ok())
                }
                _ => (cand.clone(), None),
            };
            if std::fs::metadata(resolve(&dir, &path)).is_ok() {
                let w = self.open_file(log, leader, col, &dir, &path).ok()?;
                if let Some(n) = line {
                    if let Ok(b) = leader.view_buffer(ViewId::Body(w)) {
                        if let Some((s, e)) = leader.state.buffer(b).ok().and_then(|b| b.text.line_range(n.saturating_sub(1))) {
                            let e = (e + 1).min(leader.state.buffer(b).map(|b| b.text.len()).unwrap_or(e));
                            let _ = leader.select(log, ViewId::Body(w), s, e);
                        }
                    }
                }
                return Some(w);
            }
        }
        None
    }
}

fn mode_col(m: &ShellMode) -> Option<ColumnId> {
    match m {
        ShellMode::Errors { col } | ShellMode::Replace { col, .. } => Some(*col),
    }
}

pub fn column_of(leader: &Node, ctx: ExecCtx) -> Result<ColumnId, String> {
    match ctx {
        ExecCtx::Window(w) => leader.column_of(w).map_err(|e| e.to_string()),
        ExecCtx::Column(c) => Ok(c),
        ExecCtx::Top => leader.state.layout.cols.first().map(|c| c.id).ok_or_else(|| "no column".to_string()),
    }
}

/// Resolve a user-typed path against a directory, expanding `~`.
pub fn resolve(dir: &Path, name: &str) -> PathBuf {
    let p = if let Some(rest) = name.strip_prefix("~/") {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest)
    } else {
        PathBuf::from(name)
    };
    let p = if p.is_absolute() { p } else { dir.join(p) };
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Run `sh -c cmd` in `dir`, feeding `input` on stdin. Returns (stdout, stderr).
pub fn shell(cmd: &str, dir: &Path, input: Option<String>) -> (String, String) {
    let child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (String::new(), format!("{cmd}: {e}\n")),
    };
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    match child.wait_with_output() {
        Ok(o) => (String::from_utf8_lossy(&o.stdout).to_string(), String::from_utf8_lossy(&o.stderr).to_string()),
        Err(e) => (String::new(), format!("{cmd}: {e}\n")),
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.terms.clear();
    }
}

/// The `+Errors` name, re-exported for clients.
pub const ERRORS_NAME: &str = ERRORS;
