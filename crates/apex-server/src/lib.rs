//! The apex server: everything with an effect on the world. It hosts
//! terminals (pinned term shards), reads and writes files, runs external
//! commands and pipes, and performs the execs that name it as handler.
//!
//! The server never writes a shard it does not lead: everything it wants
//! done to buffers, windows or the layout is a [`Proposal`] for the leader.
//! In-process the client applies proposals at once ([`proposal::apply`]);
//! over a socket ([`daemon`], [`remote`]) they travel as messages.

/// This apex's build id (see `build.rs`): a daemon says its own first
/// thing on every connection, and a client of another build stops there.
pub const BUILD_ID: &str = env!("APEX_BUILD_ID");

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
use apex_core::plumb::{expand, Bindings};
use apex_core::*;

pub use proposal::Proposal;
pub use term::{TermEvent, TermHost, TermKey};
pub use proto::Script;

/// Something that happened off the main thread and needs the server's
/// attention on it.
pub enum ServerEvent {
    Term(TermId, TermEvent),
    /// A shell command finished: `(ctx, exec seq, stdout, stderr, what to do
    /// with stdout)`, its name as shown in the top row, and how it ended
    /// (acme's wait message: empty for a clean exit).
    Shell { ctx: ExecCtx, exec: Option<Seq>, out: String, err: String, mode: ShellMode, name: String, exit: String },
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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Running {
    pub pid: u32,
    /// As the top row and `Kill` know it: the command's first word.
    pub name: String,
    /// The whole command line, as the shell got it.
    pub cmd: String,
    pub dir: String,
    /// Where it was started from.
    pub ctx: ExecCtx,
    /// Seconds since the epoch.
    pub started: u64,
    /// Not started by us: a program that announced itself (`Named`),
    /// ended by its pid rather than a group, gone with its connection.
    #[serde(default)]
    pub adopted: bool,
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
    /// Plumb walks in progress, by id.
    plumbs: HashMap<u64, Plumb>,
    next_plumb: u64,
    /// Verb execs seen by `poll_execs`, for the host to start.
    plumb_starts: Vec<PlumbReq>,
    /// Files clients subscribed to (`Watch`), beyond the buffers' own.
    subscribed: BTreeSet<PathBuf>,
    /// Subscribed files that changed, for the host to report.
    changed: Vec<PathBuf>,
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
            plumbs: HashMap::new(),
            next_plumb: 1,
            plumb_starts: Vec::new(),
            subscribed: BTreeSet::new(),
            changed: Vec::new(),
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
        // a name typed into the tag may be relative: it is resolved where
        // the window is, and the buffer takes the absolute name
        let name = match arg {
            Some(a) => resolve(&dir, a).to_string_lossy().to_string(),
            None if !buf_name.is_empty() && !buf_name.starts_with('/') && !buf_name.starts_with('+') => resolve(&dir, &buf_name).to_string_lossy().to_string(),
            None => buf_name.clone(),
        };
        if name.is_empty() || name.starts_with('+') || name.ends_with('/') {
            return Err("Put: no file name".into());
        }
        let renamed = name != buf_name;
        if buf.stale && arg.is_none() && self.put_warned.insert(b) {
            return Err(format!("{name}: modified since last read"));
        }
        self.put_warned.remove(&b);
        std::fs::write(&name, &text).map_err(|e| format!("{name}: {e}"))?;
        let hash = Text::new(&text).content_hash();
        self.watches.written.insert(PathBuf::from(&name), hash.clone());
        let mut out = Vec::new();
        if renamed {
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
    /// A terminal window in `col`: the user's shell, or `cmd` run by it
    /// (acme's `win cmd`), named `dir/-host` or `dir/-cmd`.
    pub fn new_term(&mut self, log: &mut Log, col: ColumnId, dir: &Path, cmd: Option<&str>) -> Result<Proposal, String> {
        let id = TermId(self.next_term);
        self.next_term += 1;
        let host = TermHost::spawn(id, dir, 80, 24, self.term_tx.clone(), &self.env, cmd)?;
        self.node.create_shard(log, Shard::Term(id)).map_err(|e| e.to_string())?;
        self.node
            .append(log, Shard::Term(id), Op::Term(TermOp::Create { cols: 80, rows: 24 }))
            .map_err(|e| e.to_string())?;
        self.terms.insert(id, host);
        // win's name: the directory, then `-` and the host (`awd` keeps it
        // so), or the command
        let name = format!("{}/-{}", dir.display().to_string().trim_end_matches('/'), self.terms[&id].label);
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
            ServerEvent::File(path) => {
                let named = self.watches.as_named(&path);
                if self.subscribed.contains(&named) {
                    self.changed.push(named);
                }
                props.extend(self.file_changed(view, &path));
            }
            ServerEvent::Term(id, ev) => {
                let Some(h) = self.terms.get_mut(&id) else { return props };
                // a new name for the window, from a label (acme's win)
                let mut name: Option<String> = None;
                match ev {
                    TermEvent::Alac(ev) => match ev {
                        Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::ResetTitle | Event::Bell => {}
                        Event::Title(t) => name = Some(term::labelled(&term::expand_tilde(&t), &h.label)),
                        Event::PtyWrite(s) => h.write(s.as_bytes()),
                        Event::ColorRequest(i, fmt) => h.write(fmt(term::default_color(i)).as_bytes()),
                        Event::TextAreaSizeRequest(fmt) => h.write(fmt(h.window_size()).as_bytes()),
                        Event::ClipboardStore(..) | Event::ClipboardLoad(..) => {}
                        Event::Exit | Event::ChildExit(_) => {
                            h.exited = true;
                            let _ = self.node.append(log, Shard::Term(id), Op::Term(TermOp::Exit { status: 0 }));
                        }
                    },
                    TermEvent::Name(t) => name = Some(term::labelled(&term::expand_tilde(&t), &h.label)),
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
                if let Some(exec) = exec {
                    props.push(Proposal::Status { ctx, exec, status: ExecStatusOp::Done });
                }
            }
        }
        props
    }

    /// Keep the directory watches in step with the files open in `view`.
    /// Watch `path` for a subscriber (beyond the buffers).
    pub fn subscribe(&mut self, view: &Node, path: &Path) {
        self.subscribed.insert(path.to_path_buf());
        self.sync_watches(view);
    }

    pub fn unsubscribe(&mut self, view: &Node, path: &Path) {
        self.subscribed.remove(path);
        self.sync_watches(view);
    }

    /// Subscribed files that changed since the last call.
    pub fn take_changed(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.changed)
    }

    pub fn sync_watches(&mut self, view: &Node) {
        let mut files: Vec<PathBuf> = view
            .state
            .buffers
            .values()
            .filter(|b| !b.name.is_empty() && !b.name.starts_with('+') && !b.name.ends_with('/') && b.name.starts_with('/'))
            .map(|b| PathBuf::from(&b.name))
            .collect();
        files.extend(self.subscribed.iter().cloned());
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

    /// The commands running now (what the top row names and `Kill` ends),
    /// and the terminals' shells while they run.
    pub fn processes(&self) -> Vec<Running> {
        let mut out = self.running.lock().unwrap().clone();
        for (id, h) in &self.terms {
            if h.exited {
                continue;
            }
            let ctx = self.node.state.windows.values().find(|w| w.body == Body::Term(*id)).map(|w| ExecCtx::Window(w.id)).unwrap_or(ExecCtx::Top);
            out.push(Running { pid: h.pid, name: h.name.clone(), cmd: h.cmd.clone(), dir: h.dir.display().to_string(), ctx, started: h.started, adopted: false });
        }
        out.sort_by_key(|r| r.started);
        out
    }

    /// A program said what it is called (`Named`): the entry of its group
    /// takes the name (the top row follows); a program of no known group
    /// is adopted. Returns the pid to forget when the announcer goes, for
    /// an adoption.
    pub fn name_process(&mut self, name: &str, group: u32, pid: u32, cmd: &str) -> Option<u32> {
        let mut running = self.running.lock().unwrap();
        if let Some(r) = running.iter_mut().find(|r| r.pid == group) {
            // a name the server gave on purpose (Win, attach) stays; the
            // default one, the command's first word (apex), gives way
            if r.name != name && r.name == command_name(&r.cmd) {
                self.started.push(Proposal::CommandExit { name: r.name.clone() });
                self.started.push(Proposal::CommandStart { name: name.to_string() });
                r.name = name.to_string();
            }
            return None;
        }
        if running.iter().any(|r| r.pid == pid) {
            return None;
        }
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        running.push(Running { pid, name: name.to_string(), cmd: cmd.to_string(), dir: String::new(), ctx: ExecCtx::Top, started, adopted: true });
        self.started.push(Proposal::CommandStart { name: name.to_string() });
        Some(pid)
    }

    /// The top row's starts and ends waiting to be applied (commands
    /// started outside an exec: scripts, adoptions, renames). The daemon
    /// takes them after everything it does, so a start never trails its
    /// own exit.
    pub fn take_started(&mut self) -> Vec<Proposal> {
        std::mem::take(&mut self.started)
    }

    /// An adopted program's announcer went: the entry goes too.
    pub fn forget_process(&mut self, pid: u32) {
        let mut running = self.running.lock().unwrap();
        if let Some(i) = running.iter().position(|r| r.pid == pid && r.adopted) {
            let r = running.remove(i);
            self.started.push(Proposal::CommandExit { name: r.name });
        }
    }

    /// acme's xkill: end every running command whose name (or pid) is
    /// `target`, with its process group (rc and what it started). How
    /// many were signalled.
    pub fn kill(&self, target: &str) -> usize {
        let running = self.running.lock().unwrap().clone();
        let mut n = 0;
        for r in running {
            if r.name == target || r.pid.to_string() == target {
                // SAFETY: a plain signal to a group we made, or to a
                // program that announced itself
                unsafe {
                    libc::kill(if r.adopted { r.pid as i32 } else { -(r.pid as i32) }, libc::SIGTERM);
                }
                n += 1;
            }
        }
        // a terminal's shell: hung up, as closing its window would
        for h in self.terms.values() {
            if !h.exited && (h.name == target || h.pid.to_string() == target) {
                // SAFETY: a signal to the shell we started
                unsafe {
                    libc::kill(h.pid as i32, libc::SIGHUP);
                }
                n += 1;
            }
        }
        n
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
                // `Newterm cmd args`: the terminal runs that instead of a shell
                let rest = text[cmd.len()..].trim();
                props.push(self.new_term(log, col, &dir, if rest.is_empty() { None } else { Some(rest) })?);
            }
            "Win" => {
                // acme's win: the tool, run as a command named Win (so Kill
                // Win ends it), with $acmeshell or the command given
                let rest = text[cmd.len()..].trim();
                let apex = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_else(|_| "apex".into());
                let command = format!("{} tool win {rest}", shell_quote(&apex));
                let env = self.command_env(view, ctx);
                self.spawn_shell_as("Win".into(), ctx, Some(seq), command, dir, None, ShellMode::Errors { dir: errdir }, env);
                return Ok(None);
            }
            "Kill" => {
                // acme's xkill: every running command whose name is given
                for name in words {
                    self.kill(name);
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
                // a rule's verb offered in this window: the rules take it
                if let Some(req) = self.verb_request(view, ctx, seq, text) {
                    self.plumb_starts.push(req);
                    return Ok(None);
                }
                // anything else is a shell command; output goes to +Errors
                let env = self.command_env(view, ctx);
                self.spawn_shell(ctx, seq, text.to_string(), dir, None, ShellMode::Errors { dir: errdir }, env);
                return Ok(None);
            }
        }
        Ok(Some(props))
    }

    fn spawn_shell(&mut self, ctx: ExecCtx, exec: Seq, cmd: String, dir: PathBuf, stdin: Option<String>, mode: ShellMode, env: Vec<(String, String)>) {
        let name = command_name(&cmd);
        self.spawn_shell_as(name, ctx, Some(exec), cmd, dir, stdin, mode, env);
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_shell_as(&mut self, name: String, ctx: ExecCtx, exec: Option<Seq>, cmd: String, dir: PathBuf, stdin: Option<String>, mode: ShellMode, env: Vec<(String, String)>) {
        let tx = self.tx.clone();
        let running = self.running.clone();
        // acme's waitthread: the name goes into the top row while it runs
        self.started.push(Proposal::CommandStart { name: name.clone() });
        std::thread::spawn(move || {
            // the name as it is at the end: the program may have renamed itself
            let (out, err, exit, name) = shell_in_named(&name, ctx, &cmd, &dir, stdin, Some(running), &env);
            let _ = tx.unbounded_send(ServerEvent::Shell { ctx, exec, out, err, mode, name, exit });
        });
    }

    /// Set a variable in the session's environment: what terminals and
    /// commands started from now on get.
    pub fn set_env(&mut self, k: &str, v: &str) {
        match self.env.iter_mut().find(|(n, _)| n == k) {
            Some(e) => e.1 = v.to_string(),
            None => self.env.push((k.to_string(), v.to_string())),
        }
    }

    /// A new session's init: the host's file (`host_profile`, normally
    /// `~/.apex/init`) sourced, then what the creator brought, unless it
    /// is that same file; one shell reading both, run like any command,
    /// named `init` in the top row, its output in `+Errors`.
    pub fn run_profile(&mut self, view: &Node, host_profile: Option<&Path>, init: Option<&proto::Script>) {
        let host_text = host_profile.and_then(|p| std::fs::read_to_string(p).ok());
        let mut script = String::new();
        if let (Some(p), Some(_)) = (host_profile, &host_text) {
            script.push_str(&format!(". {}\n", shell_quote(&p.display().to_string())));
        }
        if let Some(i) = init {
            if !i.text.trim().is_empty() && host_text.as_deref() != Some(i.text.as_str()) {
                script.push_str(&i.text);
                if !script.ends_with('\n') {
                    script.push('\n');
                }
            }
        }
        if script.is_empty() {
            return;
        }
        let dir = self.cwd.clone();
        let env = self.command_env(view, ExecCtx::Top);
        self.spawn_shell_as("profile".into(), ExecCtx::Top, None, script, dir, None, ShellMode::Errors { dir: None }, env);
    }

    /// A client's attach script (`~/.apex/attach` where it runs), run on
    /// this host every time it attaches, with `apexattachment` naming it
    /// so `apex set` there is the attachment's own; output in `+Errors`.
    pub fn run_attach(&mut self, view: &Node, attachment: AttachmentId, script: &Script) {
        if script.text.trim().is_empty() {
            return;
        }
        let dir = self.cwd.clone();
        let mut env = self.command_env(view, ExecCtx::Top);
        env.push(("apexattachment".into(), attachment.0.to_string()));
        env.push(("apexclient".into(), script.client.clone()));
        let mut text = script.text.clone();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        self.spawn_shell_as("attach".into(), ExecCtx::Top, None, text, dir, None, ShellMode::Errors { dir: None }, env);
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

    /// The plumbing rules a session starts with, owned by the session at
    /// a low priority so that anything installed later wins: what B3 did
    /// before there were rules. `.,;:)` after a name are forgiven.
    pub fn install_default_rules(&mut self, log: &mut Log) {
        let r = |text: &str, isfile: Option<&str>, isdir: Option<&str>, edit: &str| PlumbRule {
            verb: "plumb".into(),
            text: Some(text.into()),
            file: None,
            kind: None,
            isfile: isfile.map(String::from),
            isdir: isdir.map(String::from),
            action: RuleAction::Edit(edit.into()),
            to: None,
        };
        let defaults = [
            r(r"(\S+?):(\d+)[.,;:)]*", Some("$1"), None, "$1:$2"),
            r(r"(\S+?)[.,;:)]*", Some("$1"), None, "$1"),
            r(r"(\S+?)[.,;:)]*", None, Some("$1"), "$1"),
        ];
        for rule in defaults {
            let (_, e) = log.install_rule(SERVER, -100, rule);
            let _ = self.node.state.apply(Shard::Meta, &e);
        }
    }

    /// Start a plumb: the first step of walking the rules. The id names
    /// the walk to `plumb_next` when a step's answer comes from elsewhere.
    pub fn plumb_start(&mut self, view: &Node, req: PlumbReq) -> (u64, PlumbStep) {
        let id = self.next_plumb;
        self.next_plumb += 1;
        let win = match req.ctx {
            ExecCtx::Window(w) => Some(w),
            _ => None,
        };
        let dir = req.dir.clone().unwrap_or_else(|| self.dir_of(view, req.ctx));
        let (name, kind) = match win {
            Some(w) => (view.window_name(w), view.window_kind(w)),
            None => (String::new(), WinKind::File),
        };
        let sel = view.seltext.and_then(|v| view.selected_text(v).ok()).unwrap_or_default();
        let base = Bindings {
            groups: Vec::new(),
            file: name.clone(),
            dir: dir.display().to_string(),
            win: win.map(|w| w.0.to_string()).unwrap_or_default(),
            line: String::new(),
            sel,
        };
        let remaining: Vec<(RuleId, apex_core::state::Rule)> = apex_core::plumb::ordered(&view.state.meta.rules).into_iter().map(|(i, r)| (i, r.clone())).collect();
        self.plumbs.insert(id, Plumb { req, dir, name, kind, base, remaining, trace: Vec::new() });
        let step = self.plumb_advance(view, id);
        (id, step)
    }

    /// The answer to an `Ask`/`AskTool` step: taken (`Ok`) or refused,
    /// after which the walk goes on.
    pub fn plumb_next(&mut self, view: &Node, id: u64, outcome: Result<(), String>) -> PlumbStep {
        if let Some(p) = self.plumbs.get_mut(&id) {
            match outcome {
                Ok(()) => {
                    p.trace.push("taken".into());
                    return self.plumb_finish(id, Vec::new());
                }
                Err(e) => p.trace.push(format!("refused: {e}")),
            }
        }
        self.plumb_advance(view, id)
    }

    /// Verb execs (a rule's word in a tag, B2'd) found by `poll_execs`,
    /// to be started by the host.
    pub fn take_plumb_starts(&mut self) -> Vec<PlumbReq> {
        std::mem::take(&mut self.plumb_starts)
    }

    pub fn peek_plumb_starts(&self) -> &[PlumbReq] {
        &self.plumb_starts
    }

    fn plumb_finish(&mut self, id: u64, mut props: Vec<Proposal>) -> PlumbStep {
        let Some(p) = self.plumbs.remove(&id) else { return PlumbStep::Done(props) };
        if let Some(exec) = p.req.exec {
            if !p.req.dry {
                props.push(Proposal::Status { ctx: p.req.ctx, exec, status: ExecStatusOp::Done });
            }
        }
        PlumbStep::Done(props)
    }

    fn plumb_advance(&mut self, view: &Node, id: u64) -> PlumbStep {
        loop {
            let Some(p) = self.plumbs.get_mut(&id) else { return PlumbStep::Done(Vec::new()) };
            if p.remaining.is_empty() {
                break;
            }
            let (rid, rule) = p.remaining.remove(0);
            let r = &rule.rule;
            if r.verb != p.req.verb {
                continue;
            }
            if p.req.edit_only && !matches!(r.action, RuleAction::Edit(_)) {
                continue;
            }
            let owner = if rule.attachment == SERVER { "session".to_string() } else { view.state.meta.attachments.get(&rule.attachment).map(|a| a.name.clone()).unwrap_or_else(|| rule.attachment.to_string()) };
            let who = format!("{rid} ({owner}, p{})", rule.priority);
            if !r.applies_to(&p.name, p.kind) {
                p.trace.push(format!("{who}: not this window"));
                continue;
            }
            let Some(groups) = r.match_text(&p.req.text) else {
                p.trace.push(format!("{who}: text does not match"));
                continue;
            };
            let mut b = p.base.clone();
            b.groups = groups;
            if let Some(t) = &r.isfile {
                let path = resolve(&p.dir, &expand(t, &b));
                if !path.is_file() {
                    p.trace.push(format!("{who}: {} is not a file", path.display()));
                    continue;
                }
            }
            if let Some(t) = &r.isdir {
                let path = resolve(&p.dir, &expand(t, &b));
                if !path.is_dir() {
                    p.trace.push(format!("{who}: {} is not a directory", path.display()));
                    continue;
                }
            }
            let dry = p.req.dry;
            let (ctx, dir) = (p.req.ctx, p.dir.clone());
            match &r.action {
                RuleAction::Edit(t) => {
                    let target = expand(t, &b);
                    if dry {
                        p.trace.push(format!("{who}: would open {target}"));
                        return self.plumb_trace(id);
                    }
                    let (path, line) = split_line(&target);
                    let full = resolve(&dir, &path);
                    if !full.exists() {
                        p.trace.push(format!("{who}: {target}: no such file"));
                        continue;
                    }
                    p.trace.push(format!("{who}: opened {target}"));
                    let loc = Loc { name: full.display().to_string(), pos: line.map(Pos::Line).unwrap_or(Pos::Keep) };
                    return self.plumb_finish(id, vec![Proposal::Goto { loc }]);
                }
                RuleAction::Run(t) => {
                    let cmd = expand(t, &b);
                    if dry {
                        p.trace.push(format!("{who}: would run {cmd}"));
                        return self.plumb_trace(id);
                    }
                    p.trace.push(format!("{who}: ran {cmd}"));
                    let stdin = if b.sel.is_empty() { None } else { Some(b.sel.clone()) };
                    let exec = p.req.exec;
                    let errdir = Some(dir.display().to_string());
                    let env = self.command_env(view, ctx);
                    let name = command_name(&cmd);
                    self.spawn_shell_as(name, ctx, exec, cmd, dir, stdin, ShellMode::Errors { dir: errdir }, env);
                    // the shell reports the status when it is done
                    self.plumbs.remove(&id);
                    return PlumbStep::Done(Vec::new());
                }
                RuleAction::Client { verb, args } => {
                    let args = expand(args, &b);
                    if dry {
                        p.trace.push(format!("{who}: would ask the client to {verb} {args}"));
                        return self.plumb_trace(id);
                    }
                    p.trace.push(format!("{who}: asked the client to {verb} {args}"));
                    return PlumbStep::Ask(Proposal::ClientDo { verb: verb.clone(), args });
                }
                RuleAction::Tool(name) => {
                    if dry {
                        p.trace.push(format!("{who}: would ask {name}"));
                        return self.plumb_trace(id);
                    }
                    p.trace.push(format!("{who}: asked {name}"));
                    let (verb, text, at, sel) = (p.req.verb.clone(), p.req.text.clone(), p.req.at, p.req.sel);
                    return PlumbStep::AskTool { tool: name.clone(), ctx, verb, text, dir: dir.display().to_string(), groups: b.groups.clone(), at, sel };
                }
            }
        }
        // no rule took it
        let Some(p) = self.plumbs.get_mut(&id) else { return PlumbStep::Done(Vec::new()) };
        if let Some((word, span)) = p.req.alt.take() {
            // acme's expand: the word, now that the longer text found nothing
            p.trace.push(format!("nothing took {:?}: as the word {word:?}", p.req.text));
            p.req.text = word;
            p.req.sel = Some(span);
            p.remaining = apex_core::plumb::ordered(&view.state.meta.rules).into_iter().map(|(i, r)| (i, r.clone())).collect();
            return self.plumb_advance(view, id);
        }
        if p.req.dry {
            p.trace.push(if p.req.verb == "plumb" { "no rule: Look".into() } else { format!("no rule takes {}", p.req.verb) });
            return self.plumb_trace(id);
        }
        let (ctx, text, verb, edit_only, dir) = (p.req.ctx, p.req.text.clone(), p.req.verb.clone(), p.req.edit_only, p.dir.clone());
        let exec = p.req.exec;
        if edit_only {
            // B: the text as a path, then
            let (path, line) = split_line(&text);
            let full = resolve(&dir, &path);
            let prop = if full.exists() {
                Proposal::Goto { loc: Loc { name: full.display().to_string(), pos: line.map(Pos::Line).unwrap_or(Pos::Keep) } }
            } else {
                Proposal::Errors { dir: Some(dir.display().to_string()), text: format!("{text}: no such file\n") }
            };
            return self.plumb_finish(id, vec![prop]);
        }
        if verb == "plumb" {
            let reverse = self.plumbs.get(&id).is_some_and(|p| p.req.reverse);
            return self.plumb_finish(id, vec![Proposal::Look { ctx, text, reverse }]);
        }
        let mut props = vec![Proposal::Errors { dir: Some(dir.display().to_string()), text: format!("{verb}: no rule takes it here\n") }];
        if let Some(exec) = exec {
            props.push(Proposal::Status { ctx, exec, status: ExecStatusOp::Failed(format!("{verb}: no rule")) });
        }
        self.plumbs.remove(&id);
        PlumbStep::Done(props)
    }

    fn plumb_trace(&mut self, id: u64) -> PlumbStep {
        let lines = self.plumbs.remove(&id).map(|p| p.trace).unwrap_or_default();
        PlumbStep::Trace(lines)
    }

    /// A rule's verb, B2'd in a window: the request that walks the rules
    /// with that verb, if any rule offers it there.
    fn verb_request(&self, view: &Node, ctx: ExecCtx, seq: Seq, text: &str) -> Option<PlumbReq> {
        let mut words = text.split_whitespace();
        let verb = words.next()?;
        let (name, kind) = match ctx {
            ExecCtx::Window(w) => (view.window_name(w), view.window_kind(w)),
            _ => (String::new(), WinKind::File),
        };
        let offered = view.state.meta.rules.values().any(|r| r.rule.verb == verb && r.rule.applies_to(&name, kind));
        // no rule offers the word: a rule for every command here (win's
        // exec) takes the whole line
        let (verb, rest) = if offered {
            (verb, text[verb.len()..].trim().to_string())
        } else if view.state.meta.rules.values().any(|r| r.rule.verb == apex_core::plumb::EXEC && r.rule.applies_to(&name, kind)) {
            (apex_core::plumb::EXEC, text.trim().to_string())
        } else {
            return None;
        };
        // a verb acts on the window's dot
        let at = match ctx {
            ExecCtx::Window(w) => view.view_buffer(ViewId::Body(w)).ok().and_then(|b| view.selection(ViewId::Body(w)).ok().map(|(q0, q1)| Span { buffer: b, q0, q1 })),
            _ => None,
        };
        Some(PlumbReq { ctx, text: rest, dir: None, verb: verb.to_string(), edit_only: false, dry: false, exec: Some(seq), at, sel: None, alt: None, reverse: false })
    }
}

/// `name:line` split, when the tail is a number.
fn split_line(target: &str) -> (String, Option<usize>) {
    match target.rsplit_once(':') {
        Some((p, l)) if !p.is_empty() && !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => (p.to_string(), l.parse().ok()),
        _ => (target.to_string(), None),
    }
}

/// A plumb, or a verb, to walk the rules with.
#[derive(Clone, Debug)]
pub struct PlumbReq {
    pub ctx: ExecCtx,
    /// The plumbed text, or a verb's arguments.
    pub text: String,
    /// The directory, when the context has none of its own to trust (a
    /// terminal's cwd, `apex plumb` from a shell).
    pub dir: Option<PathBuf>,
    /// `plumb` for B3; a rule's word otherwise.
    pub verb: String,
    /// Plan 9's `B`: only rules that open in the session.
    pub edit_only: bool,
    /// Only say what would happen.
    pub dry: bool,
    /// The exec entry a verb came from, for its status.
    pub exec: Option<Seq>,
    /// Where the pointer or dot was, and what was expanded or swept.
    pub at: Option<Span>,
    pub sel: Option<Span>,
    /// The word within `text`: acme's `expand` tries the file-name
    /// expansion first and, should nothing take it, the word.
    pub alt: Option<(String, Span)>,
    /// shift-B3: the Look at the end runs backwards.
    pub reverse: bool,
}

/// One step of a plumb walk, for the host to carry out.
#[derive(Debug)]
pub enum PlumbStep {
    /// Finished: these proposals carry the outcome.
    Done(Vec<Proposal>),
    /// Ask the leader (a UI) to do this; `plumb_next` with the answer.
    Ask(Proposal),
    /// Ask this tool; `plumb_next` with its answer, or refusal after a
    /// second of silence.
    AskTool { tool: String, ctx: ExecCtx, verb: String, text: String, dir: String, groups: Vec<String>, at: Option<Span>, sel: Option<Span> },
    /// A dry run's report.
    Trace(Vec<String>),
}

struct Plumb {
    req: PlumbReq,
    dir: PathBuf,
    name: String,
    kind: WinKind,
    base: Bindings,
    remaining: Vec<(RuleId, apex_core::state::Rule)>,
    trace: Vec<String>,
}

impl Server {
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
    shell_in_as(&command_name(cmd), cmd, dir, input, running, env)
}

/// A single-quoted word for rc or sh.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `shell_in`, the command known (to `Kill` and the top row) as `name`.
pub fn shell_in_as(name: &str, cmd: &str, dir: &Path, input: Option<String>, running: Option<std::sync::Arc<std::sync::Mutex<Vec<Running>>>>, env: &[(String, String)]) -> (String, String, String) {
    shell_in_ctx(name, ExecCtx::Top, cmd, dir, input, running, env)
}

/// `shell_in_as`, recording where it was started from.
pub fn shell_in_ctx(name: &str, ctx: ExecCtx, cmd: &str, dir: &Path, input: Option<String>, running: Option<std::sync::Arc<std::sync::Mutex<Vec<Running>>>>, env: &[(String, String)]) -> (String, String, String) {
    let (out, err, exit, _) = shell_in_named(name, ctx, cmd, dir, input, running, env);
    (out, err, exit)
}

/// `shell_in_ctx`, returning as well the command's name as it was when
/// it ended (a program may have said what it is called meanwhile).
pub fn shell_in_named(name: &str, ctx: ExecCtx, cmd: &str, dir: &Path, input: Option<String>, running: Option<std::sync::Arc<std::sync::Mutex<Vec<Running>>>>, env: &[(String, String)]) -> (String, String, String, String) {
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
        Err(e) => return (String::new(), format!("{cmd}: {e}\n"), String::new(), name.to_string()),
    };
    let pid = child.id();
    if let Some(r) = &running {
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        r.lock().unwrap().push(Running { pid, name: name.to_string(), cmd: cmd.to_string(), dir: dir.display().to_string(), ctx, started, adopted: false });
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
    let mut final_name = name.to_string();
    if let Some(r) = &running {
        let mut r = r.lock().unwrap();
        if let Some(x) = r.iter().find(|x| x.pid == pid) {
            final_name = x.name.clone();
        }
        r.retain(|x| x.pid != pid);
    }
    (out.0, out.1, out.2, final_name)
}

impl Drop for Server {
    fn drop(&mut self) {
        self.terms.clear();
    }
}

/// The `+Errors` name, re-exported for clients.
pub const ERRORS_NAME: &str = ERRORS;
