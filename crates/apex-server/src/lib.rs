//! The apex server: everything with an effect on the world. It hosts
//! terminals (pinned term shards), reads and writes files, runs external
//! commands and pipes, and performs the execs that name it as handler.
//!
//! The server never writes a shard it does not lead: everything it wants
//! done to buffers, windows or the layout is a [`Proposal`] for the leader.
//! In-process the client applies proposals at once ([`proposal::apply`]);
//! over a socket ([`daemon`], [`remote`]) they travel as messages.

pub mod daemon;
pub mod proposal;
pub mod proto;
pub mod providers;
pub mod remote;
pub mod term;
pub mod term_loop;
pub mod watch;

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use alacritty_terminal::event::Event;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

use apex_core::node::ERRORS;
use apex_core::state::ExecStatus;
use apex_core::*;

pub use proposal::Proposal;
pub use term::{TermEvent, TermHost, TermKey};

/// Something that happened off the main thread and needs the server's
/// attention on it.
pub enum ServerEvent {
    Term(TermId, TermEvent),
    /// A shell command finished: `(ctx, exec seq, stdout, stderr, what to do
    /// with stdout)`, its name as shown in the top row, and how it ended
    /// (acme's wait message: empty for a clean exit).
    Shell { ctx: ExecCtx, exec: Seq, out: String, err: String, mode: ShellMode, name: String, exit: String },
    /// A watched directory reported this path.
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub enum ShellMode {
    /// Output goes to `dir/+Errors`.
    Errors { dir: Option<String> },
    /// Output replaces a range of a buffer (`|cmd`, `<cmd`).
    Replace { dir: Option<String>, buffer: BufferId, version: Version, q0: usize, q1: usize },
}

/// A command the server started and has not seen finish: `Kill name`
/// ends every one whose first word is `name`, as acme's does.
#[derive(Clone, Debug)]
pub struct Running {
    pub pid: u32,
    pub name: String,
}

/// The server. `node` is its replica as the `SERVER` attachment: it leads
/// the pinned terminal shards and nothing else. Reads of the rest of the
/// session go through a `view` the caller supplies (in-process, the
/// client's own node; in the daemon, a follower kept up to date).
pub struct Server {
    pub node: Node,
    terms: HashMap<TermId, TermHost>,
    tx: UnboundedSender<ServerEvent>,
    term_tx: UnboundedSender<(TermId, TermEvent)>,
    /// What every shell and command gets in its environment beyond acme's
    /// own: the session (`apexsession`) and the socket, so `apex` in a
    /// terminal works on the session it is in.
    pub env: Vec<(String, String)>,
    /// Execs already performed, so a scan does not repeat them.
    performed: BTreeSet<(ExecCtx, Seq)>,
    pub cwd: PathBuf,
    next_term: u64,
    /// Terminals whose window has been seen: once it goes, so do they.
    windowed: BTreeSet<TermId>,
    watches: watch::Watches,
    /// Stale buffers `Put` has refused once (acme: the second Put writes).
    put_warned: BTreeSet<BufferId>,
    running: std::sync::Arc<std::sync::Mutex<Vec<Running>>>,
    /// Proposals made while performing (a command's name for the top row).
    started: Vec<Proposal>,
}

impl Server {
    /// A server over `log`; returns the receiver the host must drain into
    /// [`Server::pump`].
    pub fn new(log: &Log) -> (Server, UnboundedReceiver<ServerEvent>) {
        let (tx, rx) = unbounded();
        let (term_tx, mut term_rx) = unbounded::<(TermId, TermEvent)>();
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
        let watches = {
            let tx = tx.clone();
            watch::Watches::new(move |p| {
                let _ = tx.unbounded_send(ServerEvent::File(p));
            })
        };
        let server = Server {
            node,
            terms: HashMap::new(),
            tx,
            term_tx,
            env: Vec::new(),
            performed: BTreeSet::new(),
            cwd,
            next_term: 1,
            windowed: BTreeSet::new(),
            watches,
            put_warned: BTreeSet::new(),
            running: Default::default(),
            started: Vec::new(),
        };
        (server, rx)
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

    /// Propose a window of `col` on a file (or a directory listing). The
    /// leader reuses a window already showing it.
    pub fn open_file(&self, col: ColumnId, from: Option<WindowId>, dir: &Path, name: &str, select_line: Option<usize>) -> Result<Proposal, String> {
        let path = resolve(dir, name);
        let (display, text) = self.read_path(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let hash = Text::new(&text).content_hash();
        Ok(Proposal::OpenWindow { col, from, name: display, text, hash, select_line })
    }

    fn put(&mut self, view: &Node, w: WindowId, arg: Option<&str>) -> Result<Vec<Proposal>, String> {
        let b = view.state.window(w).map_err(|e| e.to_string())?.body_buffer().ok_or("Put: not a text window")?;
        let buf = view.state.buffer(b).map_err(|e| e.to_string())?;
        let (buf_name, text, version) = (buf.name.clone(), buf.text.to_string(), buf.version);
        let dir = self.dir_of(view, ExecCtx::Window(w));
        let name = match arg {
            Some(a) => resolve(&dir, a).to_string_lossy().to_string(),
            None => buf_name,
        };
        if name.is_empty() || name.starts_with('+') || name.ends_with('/') {
            return Err("Put: no file name".into());
        }
        if buf.stale && arg.is_none() && self.put_warned.insert(b) {
            return Err(format!("{name}: modified since last read"));
        }
        self.put_warned.remove(&b);
        std::fs::write(&name, &text).map_err(|e| format!("{name}: {e}"))?;
        let hash = Text::new(&text).content_hash();
        self.watches.written.insert(PathBuf::from(&name), hash.clone());
        let mut out = Vec::new();
        if arg.is_some() {
            out.push(Proposal::Rename { buffer: b, window: w, name });
        }
        out.push(Proposal::Clean { buffer: b, version, hash });
        Ok(out)
    }

    fn get(&self, view: &Node, w: WindowId) -> Result<Proposal, String> {
        let b = view.state.window(w).map_err(|e| e.to_string())?.body_buffer().ok_or("Get: not a text window")?;
        let name = view.state.buffer(b).map(|b| b.name.clone()).map_err(|e| e.to_string())?;
        let (_, text) = self.read_path(Path::new(&name)).map_err(|e| format!("{name}: {e}"))?;
        let hash = Text::new(&text).content_hash();
        Ok(Proposal::SetContent { buffer: b, version: None, text, hash })
    }

    // ---- terminals ------------------------------------------------------------

    /// Start a shell in a new pinned terminal shard; propose its window.
    pub fn new_term(&mut self, log: &mut Log, col: ColumnId, dir: &Path) -> Result<Proposal, String> {
        let id = TermId(self.next_term);
        self.next_term += 1;
        let host = TermHost::spawn(id, dir, 80, 24, self.term_tx.clone(), &self.env)?;
        self.node.create_shard(log, Shard::Term(id)).map_err(|e| e.to_string())?;
        self.node
            .append(log, Shard::Term(id), Op::Term(TermOp::Create { cols: 80, rows: 24 }))
            .map_err(|e| e.to_string())?;
        self.terms.insert(id, host);
        // win's name: the directory, then `-` and the host (`awd` keeps it so)
        let name = format!("{}/-{}", dir.display().to_string().trim_end_matches('/'), term::sysname());
        Ok(Proposal::TermWindow { col, name, term: id })
    }

    /// Publish a terminal's current grid to its shard.
    pub fn publish_term(&mut self, log: &mut Log, id: TermId) {
        let Some(h) = self.terms.get(&id) else { return };
        let ops = h.snapshot_ops();
        for op in ops {
            let _ = self.node.append(log, Shard::Term(id), Op::Term(op));
        }
    }

    /// Keys go to the live screen: a terminal scrolled back comes back
    /// to the bottom first (its rows are republished by the next event).
    pub fn term_key(&mut self, log: &mut Log, id: TermId, key: &TermKey) {
        if let Some(h) = self.terms.get_mut(&id) {
            let scrolled = h.scroll_to_bottom();
            h.key(key);
            if scrolled {
                self.publish_term(log, id);
            }
        }
    }

    pub fn term_paste(&mut self, log: &mut Log, id: TermId, text: &str) {
        if let Some(h) = self.terms.get_mut(&id) {
            let scrolled = h.scroll_to_bottom();
            h.paste(text);
            if scrolled {
                self.publish_term(log, id);
            }
        }
    }

    /// A terminal selection's text (it may run into the scrollback only
    /// the server has), as a proposal that snarfs it.
    pub fn term_text(&self, id: TermId, p0: (u16, u64), p1: (u16, u64)) -> Option<Proposal> {
        let text = self.terms.get(&id)?.text(p0, p1);
        if text.is_empty() {
            return None;
        }
        Some(Proposal::Snarf { text })
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

    /// Close terminals whose windows are gone. A terminal whose window has
    /// not appeared yet (the proposal is in flight) is left alone.
    pub fn close_orphan_terms(&mut self, log: &mut Log, view: &Node) {
        let live: BTreeSet<TermId> = view
            .state
            .windows
            .values()
            .filter_map(|w| match w.body {
                Body::Term(t) => Some(t),
                _ => None,
            })
            .collect();
        for t in self.term_ids() {
            if live.contains(&t) {
                self.windowed.insert(t);
            } else if self.windowed.remove(&t) {
                self.close_term(log, t);
            }
        }
    }

    // ---- events and execs -------------------------------------------------------

    /// Handle one event from the receiver returned by [`Server::new`].
    /// Terminal output goes straight to the term shard; shell results
    /// become proposals.
    pub fn pump(&mut self, log: &mut Log, view: &Node, ev: ServerEvent) -> Vec<Proposal> {
        let mut props = Vec::new();
        match ev {
            ServerEvent::File(path) => props.extend(self.file_changed(view, &path)),
            ServerEvent::Term(id, ev) => {
                let Some(h) = self.terms.get_mut(&id) else { return props };
                // a new name for the window, from a label (acme's win)
                let mut name: Option<String> = None;
                match ev {
                    TermEvent::Alac(ev) => match ev {
                        Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::ResetTitle | Event::Bell => {}
                        Event::Title(t) => name = Some(term::labelled(&t, &h.label)),
                        Event::PtyWrite(s) => h.write(s.as_bytes()),
                        Event::ColorRequest(i, fmt) => h.write(fmt(term::default_color(i)).as_bytes()),
                        Event::TextAreaSizeRequest(fmt) => h.write(fmt(h.window_size()).as_bytes()),
                        Event::ClipboardStore(..) | Event::ClipboardLoad(..) => {}
                        Event::Exit | Event::ChildExit(_) => {
                            h.exited = true;
                            let _ = self.node.append(log, Shard::Term(id), Op::Term(TermOp::Exit { status: 0 }));
                        }
                    },
                    TermEvent::Name(t) => name = Some(term::labelled(&t, &h.label)),
                    TermEvent::Cwd(s) => {
                        if let Some(p) = term::cwd_path(&s) {
                            name = Some(format!("{}/-{}", p.display().to_string().trim_end_matches('/'), h.label));
                        }
                    }
                }
                if let Some(name) = name {
                    // the label's `-name` stays for later directory reports;
                    // its directory is where the shell now is
                    if let Some((dir, last)) = name.rsplit_once('/') {
                        if let Some(l) = last.strip_prefix('-') {
                            h.label = l.to_string();
                        }
                        let dir = if dir.is_empty() { "/" } else { dir };
                        if Path::new(dir).is_dir() {
                            h.dir = PathBuf::from(dir);
                        }
                    }
                    if let Some(w) = view.state.windows.values().find(|w| w.body == Body::Term(id)).map(|w| w.id) {
                        props.push(Proposal::TermName { window: w, name });
                    }
                }
                self.publish_term(log, id);
            }
            ServerEvent::Shell { ctx, exec, out, err, mode, name, exit } => {
                let dir = mode_dir(&mode);
                // acme's waitthread: the name leaves the top row, then any
                // exit message is reported
                props.push(Proposal::CommandExit { name: name.clone() });
                if !exit.is_empty() {
                    props.push(Proposal::Errors { dir: dir.clone(), text: format!("{name}: exit {exit}\n") });
                }
                match mode {
                    ShellMode::Errors { dir } => {
                        if !out.is_empty() {
                            props.push(Proposal::Errors { dir, text: out });
                        }
                    }
                    ShellMode::Replace { dir, buffer, version, q0, q1 } => {
                        props.push(Proposal::ReplaceRange { dir, buffer, version, q0, q1, text: out });
                    }
                }
                if !err.is_empty() {
                    props.push(Proposal::Errors { dir, text: err });
                }
                props.push(Proposal::Status { ctx, exec, status: ExecStatusOp::Done });
            }
        }
        props
    }

    /// Keep the directory watches in step with the files open in `view`.
    pub fn sync_watches(&mut self, view: &Node) {
        let files: Vec<PathBuf> = view
            .state
            .buffers
            .values()
            .filter(|b| !b.name.is_empty() && !b.name.starts_with('+') && !b.name.ends_with('/') && b.name.starts_with('/'))
            .map(|b| PathBuf::from(&b.name))
            .collect();
        self.watches.sync(files.iter().map(|p| p.as_path()));
    }

    /// A watched path changed. A clean buffer follows the disk; a dirty one
    /// is flagged stale so `Get` appears in its tag (§9).
    fn file_changed(&mut self, view: &Node, path: &Path) -> Vec<Proposal> {
        let path = &self.watches.as_named(path);
        let name = path.to_string_lossy().to_string();
        let Some(buf) = view.state.buffers.values().find(|b| b.name == name) else { return Vec::new() };
        let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
        let text = String::from_utf8_lossy(&bytes).to_string();
        let hash = Text::new(&text).content_hash();
        if self.watches.written.get(path) == Some(&hash) || buf.disk_hash.as_deref() == Some(hash.as_str()) {
            return Vec::new(); // our own write, or nothing new
        }
        if buf.dirty() {
            if buf.stale {
                return Vec::new();
            }
            vec![Proposal::Stale { buffer: buf.id, hash }]
        } else {
            // valid at the version this replica saw: if the leader has
            // typed since, it flags the buffer stale instead
            vec![Proposal::SetContent { buffer: buf.id, version: Some(buf.version), text, hash }]
        }
    }

    /// Perform every pending exec addressed to the server, as seen in
    /// `view`. Returns the proposals that carry the results.
    pub fn poll_execs(&mut self, log: &mut Log, view: &Node) -> Vec<Proposal> {
        self.sync_watches(view);
        let mut todo: Vec<(ExecCtx, Seq, String, ExecAt)> = Vec::new();
        for (w, win) in &view.state.windows {
            for (seq, e) in &win.execs {
                if e.handler == Handler::Server && e.status == ExecStatus::Pending && !self.performed.contains(&(ExecCtx::Window(*w), *seq)) {
                    todo.push((ExecCtx::Window(*w), *seq, e.text.clone(), e.at));
                }
            }
        }
        for (seq, (ctx, e)) in &view.state.layout.execs {
            if e.handler == Handler::Server && e.status == ExecStatus::Pending && !self.performed.contains(&(*ctx, *seq)) {
                todo.push((*ctx, *seq, e.text.clone(), e.at));
            }
        }
        let mut props = Vec::new();
        for (ctx, seq, text, at) in todo {
            self.performed.insert((ctx, seq));
            let r = self.perform(log, view, ctx, seq, &text, at);
            props.append(&mut self.started);
            match r {
                Ok(Some(mut p)) => {
                    props.append(&mut p);
                    props.push(Proposal::Status { ctx, exec: seq, status: ExecStatusOp::Done });
                }
                Ok(None) => {} // asynchronous; status comes with the result
                Err(reason) => {
                    let dir = self.dir_of(view, ctx).to_string_lossy().to_string();
                    props.push(Proposal::Errors { dir: Some(dir), text: format!("{reason}\n") });
                    props.push(Proposal::Status { ctx, exec: seq, status: ExecStatusOp::Failed(reason) });
                }
            }
        }
        props
    }

    /// `Ok(Some(proposals))` if finished now, `Ok(None)` if the result
    /// arrives later through [`Server::pump`].
    fn perform(&mut self, log: &mut Log, view: &Node, ctx: ExecCtx, seq: Seq, text: &str, at: ExecAt) -> Result<Option<Vec<Proposal>>, String> {
        let dir = self.dir_of(view, ctx);
        let col = column_of(view, ctx)?;
        let errdir = Some(dir.to_string_lossy().to_string());
        let win = match ctx {
            ExecCtx::Window(w) => Some(w),
            _ => None,
        };
        // pipes
        if let Some(rest) = text.strip_prefix('|').or_else(|| text.strip_prefix('<')).or_else(|| text.strip_prefix('>')) {
            let kind = text.as_bytes()[0];
            let input = at
                .buffer
                .and_then(|b| view.state.buffer(b).ok())
                .map(|b| b.text.slice(at.q0, at.q1))
                .unwrap_or_default();
            let mode = match (kind, at.buffer) {
                (b'>', _) | (_, None) => ShellMode::Errors { dir: errdir },
                (_, Some(b)) => ShellMode::Replace { dir: errdir, buffer: b, version: at.version, q0: at.q0, q1: at.q1 },
            };
            let stdin = if kind == b'<' { None } else { Some(input) };
            let env = self.command_env(view, ctx);
            self.spawn_shell(ctx, seq, rest.trim().to_string(), dir, stdin, mode, env);
            return Ok(None);
        }
        let mut words = text.split_whitespace();
        let cmd = words.next().unwrap_or("");
        let arg = words.clone().next();
        let mut props = Vec::new();
        match cmd {
            "Put" => {
                let w = win.ok_or("Put needs a window")?;
                props.append(&mut self.put(view, w, arg)?);
            }
            "Putall" => {
                for (w, x) in &view.state.windows {
                    let dirty = x
                        .body_buffer()
                        .and_then(|b| view.state.buffer(b).ok())
                        .is_some_and(|b| b.dirty() && !b.name.is_empty() && !b.name.starts_with('+') && !b.name.ends_with('/'));
                    if dirty {
                        props.append(&mut self.put(view, *w, None)?);
                    }
                }
            }
            "Get" => {
                let w = win.ok_or("Get needs a window")?;
                props.push(self.get(view, w)?);
            }
            "New" => {
                let name = arg.ok_or("New needs a name here")?;
                let path = resolve(&dir, name);
                if path.exists() {
                    props.push(self.open_file(col, win, &dir, name, None)?);
                } else {
                    props.push(Proposal::NewWindow { col, name: path.to_string_lossy().to_string() });
                }
            }
            "Newterm" => {
                props.push(self.new_term(log, col, &dir)?);
            }
            "Kill" => {
                // acme's xkill: every running command whose name is given
                let names: Vec<&str> = words.collect();
                let running = self.running.lock().unwrap().clone();
                for r in running {
                    if names.iter().any(|n| *n == r.name) {
                        // the command runs in its own process group (rc and
                        // what it started): end all of it
                        // SAFETY: a plain signal to a group we made
                        unsafe {
                            libc::kill(-(r.pid as i32), libc::SIGTERM);
                        }
                    }
                }
            }
            "Send" => {
                // acme's sendx for a terminal: the snarf buffer, with a
                // newline, typed into the shell
                let w = win.ok_or("Send needs a window")?;
                let Some(Body::Term(t)) = view.state.window(w).ok().map(|x| x.body) else { return Err("Send: not a terminal".into()) };
                let mut text = view.state.layout.snarf.clone();
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                self.term_paste(log, t, &text);
            }
            "Newweb" => return Err(format!("{cmd}: not implemented")),
            _ => {
                // anything else is a shell command; output goes to +Errors
                let env = self.command_env(view, ctx);
                self.spawn_shell(ctx, seq, text.to_string(), dir, None, ShellMode::Errors { dir: errdir }, env);
                return Ok(None);
            }
        }
        Ok(Some(props))
    }

    fn spawn_shell(&mut self, ctx: ExecCtx, exec: Seq, cmd: String, dir: PathBuf, stdin: Option<String>, mode: ShellMode, env: Vec<(String, String)>) {
        let tx = self.tx.clone();
        let running = self.running.clone();
        let name = command_name(&cmd);
        // acme's waitthread: the name goes into the top row while it runs
        self.started.push(Proposal::CommandStart { name: name.clone() });
        std::thread::spawn(move || {
            let (out, err, exit) = shell_in(&cmd, &dir, stdin, Some(running), &env);
            let _ = tx.unbounded_send(ServerEvent::Shell { ctx, exec, out, err, mode, name, exit });
        });
    }

    /// acme's `textcomplete`, the file-system half: what to insert after
    /// `prefix` (a path fragment typed at `at` in `view`, relative to
    /// `dir`), or the candidates in `+Errors` when it is not decided.
    pub fn complete(&self, view: ViewId, at: usize, dir: &Path, prefix: &str) -> Proposal {
        let (dirpart, base) = match prefix.rsplit_once('/') {
            Some((d, b)) => (if d.is_empty() { "/".to_string() } else { d.to_string() }, b.to_string()),
            None => (String::new(), prefix.to_string()),
        };
        let where_ = if dirpart.is_empty() { dir.to_path_buf() } else { resolve(dir, &dirpart) };
        let errdir = Some(dir.to_string_lossy().to_string());
        let mut names: Vec<(String, bool)> = match std::fs::read_dir(&where_) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| (e.file_name().to_string_lossy().to_string(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
                .filter(|(n, _)| n.starts_with(&base))
                .collect(),
            Err(e) => return Proposal::Errors { dir: errdir, text: format!("{}: {e}\n", where_.display()) },
        };
        names.sort();
        if names.is_empty() {
            return Proposal::Errors { dir: errdir, text: format!("{}{}*: no matches\n", if dirpart.is_empty() { String::new() } else { format!("{dirpart}/") }, base) };
        }
        // the longest common extension of the candidates past what is typed
        let first: Vec<char> = names[0].0.chars().collect();
        let mut common = first.len();
        for (n, _) in &names[1..] {
            let c: Vec<char> = n.chars().collect();
            common = common.min(first.iter().zip(c.iter()).take_while(|(a, b)| a == b).count());
        }
        let typed = base.chars().count();
        let mut extension: String = first[typed..common.max(typed)].iter().collect();
        if names.len() == 1 {
            extension.push(if names[0].1 { '/' } else { ' ' });
        }
        if extension.is_empty() {
            let list: String = names.iter().map(|(n, d)| format!("{n}{}\n", if *d { "/" } else { "" })).collect();
            return Proposal::Errors { dir: errdir, text: list };
        }
        Proposal::Complete { view, at, text: extension }
    }

    /// B3: open `text` as a file (with optional `:line`) if it names one;
    /// otherwise the leader searches for it.
    pub fn plumb(&self, view: &Node, ctx: ExecCtx, text: &str) -> Proposal {
        let look = Proposal::Look { ctx, text: text.to_string() };
        let dir = self.dir_of(view, ctx);
        let Ok(col) = column_of(view, ctx) else { return look };
        let candidates = [text.to_string(), text.trim_end_matches(['.', ',', ';', ':', ')']).to_string()];
        for cand in &candidates {
            let (path, line) = match cand.rsplit_once(':') {
                Some((p, l)) if !p.is_empty() && !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => {
                    (p.to_string(), l.parse::<usize>().ok())
                }
                _ => (cand.clone(), None),
            };
            if std::fs::metadata(resolve(&dir, &path)).is_ok() {
                let from = match ctx {
                    ExecCtx::Window(w) => Some(w),
                    _ => None,
                };
                if let Ok(p) = self.open_file(col, from, &dir, &path, line) {
                    return p;
                }
            }
        }
        look
    }
}

/// In-process convenience: apply proposals through the leader, reporting
/// failures to the column's `+Errors`.
pub fn perform(node: &mut Node, log: &mut Log, props: Vec<Proposal>) -> Option<WindowId> {
    let mut made = None;
    for p in props {
        match proposal::apply(node, log, p) {
            Ok(w) => made = w.or(made),
            Err(e) => eprintln!("proposal: {e}"),
        }
    }
    made
}

fn mode_dir(m: &ShellMode) -> Option<String> {
    match m {
        ShellMode::Errors { dir } | ShellMode::Replace { dir, .. } => dir.clone(),
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
    let (out, err, _) = shell_tracked(cmd, dir, input, None);
    (out, err)
}

/// The shell commands run with, as acme's `runproc`: `$acmeshell` if
/// set, else `rc` (ours, beside us, in `~/.apex/bin`, or on the PATH),
/// else `sh`.
pub fn command_shell() -> String {
    if let Ok(s) = std::env::var("acmeshell") {
        if !s.is_empty() {
            return s;
        }
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("rc"));
            // a dev tree: target/rc-host/bin/rc beside target/release or target/debug/deps
            for up in 1..=3 {
                let mut d = dir.to_path_buf();
                for _ in 0..up {
                    d = match d.parent() {
                        Some(p) => p.to_path_buf(),
                        None => break,
                    };
                }
                candidates.push(d.join("rc-host/bin/rc"));
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join(".apex/bin/rc"));
    }
    if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
        return p.to_string_lossy().to_string();
    }
    if let Some(path) = std::env::var_os("PATH") {
        if std::env::split_paths(&path).any(|d| d.join("rc").is_file()) {
            return "rc".into();
        }
    }
    "sh".into()
}

/// What acme's `runproc` puts in a command's environment: `winid`, and
/// for a window on a file, `%` and `samfile` naming it.
impl Server {
    /// acme's environment for a command, plus this server's (`apexsession`).
    fn command_env(&self, view: &Node, ctx: ExecCtx) -> Vec<(String, String)> {
        let mut env = command_env(view, ctx);
        env.extend(self.env.iter().cloned());
        env
    }
}

pub fn command_env(view: &Node, ctx: ExecCtx) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let w = match ctx {
        ExecCtx::Window(w) => Some(w),
        _ => view.seltext.and_then(|v| v.window()),
    };
    env.push(("winid".into(), w.map(|w| w.0.to_string()).unwrap_or_else(|| "0".into())));
    if let Some(w) = w {
        let name = view.window_name(w);
        if !name.is_empty() {
            env.push(("%".into(), name.clone()));
            env.push(("samfile".into(), name));
        }
    }
    env
}

/// acme's name for a command (`runproc`): the first word, without any
/// directory, as it appears in the top row and as `Kill` knows it.
pub fn command_name(cmd: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("");
    first.rsplit('/').next().unwrap_or(first).to_string()
}

/// `shell`, registering the child in `running` (by acme's name for it)
/// for `Kill` while it lives. The third result is how it ended: empty for
/// a clean exit, else the status or signal, as acme's wait message.
pub fn shell_tracked(cmd: &str, dir: &Path, input: Option<String>, running: Option<std::sync::Arc<std::sync::Mutex<Vec<Running>>>>) -> (String, String, String) {
    shell_in(cmd, dir, input, running, &[])
}

/// `shell_tracked` with acme's environment for the command (`winid`,
/// `%`, `samfile`), run by `rc -c` as acme's `runproc` does.
pub fn shell_in(cmd: &str, dir: &Path, input: Option<String>, running: Option<std::sync::Arc<std::sync::Mutex<Vec<Running>>>>, env: &[(String, String)]) -> (String, String, String) {
    let mut command = Command::new(command_shell());
    // acme's runproc clears these before setting its own
    for k in ["acmeaddr", "winid", "%", "samfile"] {
        command.env_remove(k);
    }
    for (k, v) in env {
        command.env(k, v);
    }
    use std::os::unix::process::CommandExt;
    let child = command
        .arg("-c")
        .arg(cmd)
        .process_group(0) // its own group, so Kill reaches what the shell started
        .current_dir(dir)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (String::new(), format!("{cmd}: {e}\n"), String::new()),
    };
    let pid = child.id();
    let name = command_name(cmd);
    if let Some(r) = &running {
        r.lock().unwrap().push(Running { pid, name });
    }
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let out = match child.wait_with_output() {
        Ok(o) => {
            use std::os::unix::process::ExitStatusExt;
            let exit = match (o.status.code(), o.status.signal()) {
                (Some(0), _) => String::new(),
                (Some(n), _) => n.to_string(),
                (None, Some(sig)) => format!("signal {sig}"),
                _ => "?".to_string(),
            };
            (String::from_utf8_lossy(&o.stdout).to_string(), String::from_utf8_lossy(&o.stderr).to_string(), exit)
        }
        Err(e) => (String::new(), format!("{cmd}: {e}\n"), String::new()),
    };
    if let Some(r) = &running {
        r.lock().unwrap().retain(|x| x.pid != pid);
    }
    out
}

impl Drop for Server {
    fn drop(&mut self) {
        self.terms.clear();
    }
}

/// The `+Errors` name, re-exported for clients.
pub const ERRORS_NAME: &str = ERRORS;
