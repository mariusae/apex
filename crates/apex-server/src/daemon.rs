//! The daemon: sessions, each with its authoritative `Log`, a [`Server`],
//! and a follower replica of the whole session; and the attach and control
//! protocols to clients over a Unix socket.
//!
//! One thread owns all state. Connection readers and terminal events feed
//! it through a channel; each connection has a writer thread draining its
//! own outbound queue, so a slow client never stalls the core.
//!
//! A session with no UI attached is led by the daemon itself (the replica
//! runs as the `SERVER` attachment), so tools can work on a headless
//! session and a UI that attaches later takes it over.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufReader, BufWriter};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use apex_core::*;

use crate::proto::{read_frame, write_frame, ClientMsg, ServerMsg, SessionInit};
use crate::{proposal, Proposal, Server, ServerEvent};

/// Start a daemon on `socket` from the `apex` binary at `exe`, detached
/// from whoever asked: its own session, so that the end of an ssh
/// session or a bridge's process group does not take it along, and no
/// terminal. Returns once it answers on the socket.
pub fn spawn_server(exe: &Path, socket: &Path, session: &str) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    if let Some(d) = socket.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--socket", &socket.to_string_lossy(), "--session", session, "server"])
        .current_dir(&home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid in the child before exec; it only touches the child.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err(io::Error::other("the daemon did not start"))
}

/// Where `apexd` listens by default: `$TMPDIR/apex-$USER/main.sock`,
/// with the uid standing in where the environment names no user (a
/// container's `exec` often sets neither USER nor LOGNAME).
pub fn default_socket() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("APEX_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let who = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|u| !u.is_empty())
        // SAFETY: getuid cannot fail
        .unwrap_or_else(|| unsafe { libc::getuid() }.to_string());
    std::path::PathBuf::from(base).join(format!("apex-{who}")).join("main.sock")
}

/// The session's commands (and its shells) find our own command and
/// `rc`: `~/.apex/bin`, where a remote install puts them, and the
/// directory we run from (the app bundle's) go on the PATH.
pub fn put_apex_on_path() {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(std::path::Path::new(&home).join(".apex/bin"));
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        dirs.push(d);
    }
    let mut path = std::env::var("PATH").unwrap_or_default();
    for d in dirs.into_iter().rev() {
        if d.is_dir() && !std::env::split_paths(&path).any(|p| p == d) {
            path = format!("{}:{path}", d.display());
        }
    }
    std::env::set_var("PATH", path);
}

enum Event {
    Accept(UnixStream),
    Msg(u64, ClientMsg),
    Gone(u64),
    /// From a session's server, by the session's id (names can change).
    Server(u64, ServerEvent),
}

struct Conn {
    /// The id of the session attached to.
    session: Option<u64>,
    attachment: Option<AttachmentId>,
    kind: AttachmentKind,
    out: Sender<ServerMsg>,
    /// How far each shard has been forwarded to this connection.
    sent: HashMap<Shard, Seq>,
}

struct Session {
    /// Stable across renames; what connections and events refer to.
    id: u64,
    log: Log,
    server: Server,
    /// Follower replica of every shard, and the leader when no UI is
    /// attached.
    view: Node,
    /// The UI connection currently leading (single-player mode).
    leader: Option<u64>,
}

/// A tool's proposal in flight at the leader: (tool connection, tool's id).
struct Pending {
    conn: u64,
    id: u64,
}

pub struct Daemon {
    socket: PathBuf,
    /// The host's init file for new sessions (`~/.apex/init`).
    host_init: Option<PathBuf>,
    sessions: BTreeMap<String, Session>,
    next_session: u64,
    conns: HashMap<u64, Conn>,
    pending: HashMap<u64, Pending>,
    next_pending: u64,
    rx: Receiver<Event>,
    tx: Sender<Event>,
}

impl Daemon {
    /// Listen on `path` (removed first if stale) and run until the process
    /// ends, with one session `session` to begin with. Returns only on a
    /// listener error.
    pub fn run(path: &Path, session: &str) -> io::Result<()> {
        let host_init = std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".apex/init"));
        Self::run_with(path, session, host_init)
    }

    /// `run`, with the host's init file given (tests keep it out of `$HOME`).
    pub fn run_with(path: &Path, session: &str, host_init: Option<PathBuf>) -> io::Result<()> {
        put_apex_on_path();
        // whoever started us may go (an ssh session, a terminal): we stay
        // SAFETY: setting a signal disposition.
        unsafe {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
        }
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        let (tx, rx) = channel();
        {
            let tx = tx.clone();
            thread::spawn(move || {
                for s in listener.incoming().flatten() {
                    if tx.send(Event::Accept(s)).is_err() {
                        break;
                    }
                }
            });
        }
        let mut d = Daemon { socket: path.to_path_buf(), host_init, sessions: BTreeMap::new(), next_session: 1, conns: HashMap::new(), pending: HashMap::new(), next_pending: 1, rx, tx };
        // the daemon's own session is made from this host: its file is
        // both the host's and the creator's, so it runs once
        d.new_session(session, None);
        let mut next_id = 1u64;
        while let Ok(ev) = d.rx.recv() {
            match ev {
                Event::Accept(s) => {
                    let id = next_id;
                    next_id += 1;
                    d.accept(id, s);
                }
                // `apex stop`: done, sessions and all
                Event::Msg(_, ClientMsg::Stop) => {
                    d.conns.clear(); // every connection ends with us
                    break;
                }
                Event::Msg(id, m) => d.handle(id, m),
                Event::Gone(id) => d.gone(id),
                Event::Server(sid, ev) => {
                    if let Some(name) = d.name_of(sid) {
                        let s = d.sessions.get_mut(&name).unwrap();
                        let props = s.server.pump(&mut s.log, &s.view, ev);
                        d.after(&name, props);
                    }
                }
            }
        }
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    fn new_session(&mut self, name: &str, init: Option<SessionInit>) -> bool {
        if self.sessions.contains_key(name) {
            return false;
        }
        let log = Log::new();
        let (mut server, mut srx) = Server::new(&log);
        // shells and commands in this session know it, and the daemon
        server.env = vec![("apexsession".into(), name.to_string()), ("APEX_SOCKET".into(), self.socket.display().to_string())];
        if let Some(i) = &init {
            server.env.push(("apexclient".into(), i.client.clone()));
        }
        let sid = self.next_session;
        self.next_session += 1;
        {
            let tx = self.tx.clone();
            thread::spawn(move || {
                use futures::StreamExt;
                futures::executor::block_on(async move {
                    while let Some(ev) = srx.next().await {
                        if tx.send(Event::Server(sid, ev)).is_err() {
                            break;
                        }
                    }
                });
            });
        }
        let mut view = Node::new(SERVER);
        view.catch_up(&log).expect("fresh log");
        let mut log = log;
        // the daemon lays the session out (one column, the top tag) so a
        // tool can work before any UI attaches
        view.init_session(&mut log).expect("fresh session");
        self.sessions.insert(name.to_string(), Session { id: sid, log, server, view, leader: None });
        // its init runs now, as a command of the session
        let s = self.sessions.get_mut(name).unwrap();
        s.server.run_init(&s.view, self.host_init.as_deref(), init.as_ref());
        self.after(name, Vec::new());
        true
    }

    fn name_of(&self, sid: u64) -> Option<String> {
        self.sessions.iter().find(|(_, s)| s.id == sid).map(|(n, _)| n.clone())
    }

    /// Rename a session; everything attached stays attached.
    fn rename_session(&mut self, from: &str, to: &str) -> Result<(), String> {
        if to.is_empty() || to.contains('/') {
            return Err(format!("bad session name {to:?}"));
        }
        if self.sessions.contains_key(to) {
            return Err(format!("session {to} exists"));
        }
        let s = self.sessions.remove(from).ok_or_else(|| format!("no session {from}"))?;
        self.sessions.insert(to.to_string(), s);
        Ok(())
    }

    fn accept(&mut self, id: u64, s: UnixStream) {
        let (out, orx) = channel::<ServerMsg>();
        // the first word: which apex this is
        let _ = out.send(ServerMsg::Build { id: crate::BUILD_ID.to_string() });
        let reader = match s.try_clone() {
            Ok(r) => r,
            Err(_) => return,
        };
        let tx = self.tx.clone();
        thread::spawn(move || {
            let mut r = BufReader::new(reader);
            while let Ok(Some(m)) = read_frame::<_, ClientMsg>(&mut r) {
                if tx.send(Event::Msg(id, m)).is_err() {
                    break;
                }
            }
            let _ = tx.send(Event::Gone(id));
        });
        thread::spawn(move || {
            let mut w = BufWriter::new(s);
            while let Ok(m) = orx.recv() {
                if write_frame(&mut w, &m).is_err() {
                    break;
                }
                // coalesce whatever else is queued before flushing
                while let Ok(m) = orx.try_recv() {
                    if write_frame(&mut w, &m).is_err() {
                        return;
                    }
                }
                if std::io::Write::flush(&mut w).is_err() {
                    break;
                }
            }
        });
        self.conns.insert(id, Conn { session: None, attachment: None, kind: AttachmentKind::Tool, out, sent: HashMap::new() });
    }

    fn send(&self, id: u64, m: ServerMsg) {
        if let Some(c) = self.conns.get(&id) {
            let _ = c.out.send(m);
        }
    }

    fn gone(&mut self, id: u64) {
        let Some(c) = self.conns.remove(&id) else { return };
        let (Some(sid), Some(a)) = (c.session, c.attachment) else { return };
        let Some(name) = self.name_of(sid) else { return };
        let Some(s) = self.sessions.get_mut(&name) else { return };
        let e = s.log.detach(a);
        let _ = s.view.state.apply(Shard::Meta, &e);
        if s.leader == Some(id) {
            s.leader = None;
            // the attachment is gone for good: its leases return to the
            // server, which leads until the next UI attaches
            for shard in s.log.shards().collect::<Vec<_>>() {
                let held = s.log.lease(shard).is_some_and(|l| l.holder == a);
                if held {
                    if let Ok(e) = s.log.reclaim(shard) {
                        let _ = s.view.state.apply(Shard::Meta, &e);
                    }
                }
            }
            let _ = s.view.catch_up(&s.log);
        }
        self.pending.retain(|_, p| p.conn != id);
        self.after(&name, Vec::new());
    }

    fn handle(&mut self, id: u64, m: ClientMsg) {
        match m {
            ClientMsg::Hello { session, name, kind } => self.hello(id, session, name, kind),
            ClientMsg::NewSession { name, init } => {
                // making a session that exists is fine: it is there
                if name.is_empty() || name.contains('/') {
                    self.send(id, ServerMsg::Error { text: format!("bad session name {name:?}") });
                } else {
                    self.new_session(&name, init);
                    self.send(id, ServerMsg::Sessions { names: self.sessions.keys().cloned().collect() });
                }
            }
            ClientMsg::ListSessions => {
                self.send(id, ServerMsg::Sessions { names: self.sessions.keys().cloned().collect() });
            }
            ClientMsg::RenameSession { from, to } => match self.rename_session(&from, &to) {
                Ok(()) => self.send(id, ServerMsg::Sessions { names: self.sessions.keys().cloned().collect() }),
                Err(text) => self.send(id, ServerMsg::Error { text }),
            },
            ClientMsg::Ping { t } => self.send(id, ServerMsg::Pong { t }),
            ClientMsg::Stop => {}
            other => {
                let Some(name) = self.conns.get(&id).and_then(|c| c.session).and_then(|sid| self.name_of(sid)) else {
                    self.send(id, ServerMsg::Error { text: "not attached".into() });
                    return;
                };
                self.in_session(id, &name, other);
            }
        }
    }

    fn hello(&mut self, id: u64, session: String, name: String, kind: AttachmentKind) {
        let Some(s) = self.sessions.get_mut(&session) else {
            self.send(id, ServerMsg::Error { text: format!("no session {session}") });
            return;
        };
        let (a, e) = s.log.attach(kind, &name);
        let _ = s.view.state.apply(Shard::Meta, &e);
        if kind == AttachmentKind::Ui {
            // transfer else reclaim: single-player, so reclaim now
            for shard in s.log.shards().collect::<Vec<_>>() {
                if shard.is_pinned() {
                    continue;
                }
                let l = s.log.lease(shard).expect("lease");
                if l.holder != SERVER && l.released.is_none() {
                    if let Ok(e) = s.log.reclaim(shard) {
                        let _ = s.view.state.apply(Shard::Meta, &e);
                    }
                }
                if let Ok(e) = s.log.grant(shard, a) {
                    let _ = s.view.state.apply(Shard::Meta, &e);
                }
            }
            s.leader = Some(id);
        }
        let _ = s.view.catch_up(&s.log);
        let snapshot = s.view.state.to_snapshot();
        let marks: HashMap<Shard, Seq> = s.log.shards().map(|sh| (sh, s.log.last_seq(sh))).collect();
        let sid = s.id;
        if let Some(c) = self.conns.get_mut(&id) {
            c.session = Some(sid);
            c.attachment = Some(a);
            c.kind = kind;
            c.sent = marks;
        }
        self.send(id, ServerMsg::Welcome { attachment: a, snapshot });
        // the others learn from the metalog that the leases moved
        self.after(&session, Vec::new());
    }

    fn in_session(&mut self, id: u64, name: &str, m: ClientMsg) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        let is_leader = s.leader == Some(id);
        let mut props = Vec::new();
        match m {
            ClientMsg::Append { shard, entries } => {
                let mut last = 0;
                let mut failed = None;
                for e in entries {
                    let seq = e.seq;
                    match s.log.append_entry(shard, e) {
                        Ok(()) => last = seq,
                        Err(err) => {
                            failed = Some(format!("{shard} seq {seq}: {err}"));
                            break;
                        }
                    }
                }
                let _ = s.view.catch_up(&s.log);
                // the appender's own entries need no forwarding
                let mark = s.log.last_seq(shard);
                if let Some(c) = self.conns.get_mut(&id) {
                    c.sent.insert(shard, mark);
                }
                if let Some(text) = failed {
                    self.send(id, ServerMsg::Error { text });
                }
                if last > 0 {
                    self.send(id, ServerMsg::Ack { shard, seq: last });
                }
                let s = self.sessions.get_mut(name).unwrap();
                props = s.server.poll_execs(&mut s.log, &s.view);
            }
            ClientMsg::CreateShard { shard } => {
                let Some(a) = self.conns.get(&id).and_then(|c| c.attachment) else { return };
                match s.log.create_shard(shard, a) {
                    Ok(_) => {
                        let _ = s.view.catch_up(&s.log);
                        if let Some(c) = self.conns.get_mut(&id) {
                            c.sent.insert(shard, 0);
                        }
                        self.send(id, ServerMsg::ShardReady { shard });
                    }
                    Err(e) => self.send(id, ServerMsg::Error { text: format!("create {shard}: {e}") }),
                }
            }
            ClientMsg::DeleteShard { shard } => {
                let _ = s.log.delete_shard(shard);
                let _ = s.view.catch_up(&s.log);
            }
            ClientMsg::TermKey { term, key } => s.server.term_key(&mut s.log, term, &key),
            ClientMsg::TermPaste { term, text } => s.server.term_paste(&mut s.log, term, &text),
            ClientMsg::TermText { term, p0, p1 } => {
                if let Some(p) = s.server.term_text(term, p0, p1) {
                    props.push(p);
                }
            }
            ClientMsg::TermResize { term, cols, rows } => s.server.term_resize(&mut s.log, term, cols, rows),
            ClientMsg::TermScroll { term, delta } => s.server.term_scroll(&mut s.log, term, delta as isize),
            ClientMsg::Env { set } => {
                for (k, v) in &set {
                    s.server.set_env(k, v);
                }
                let vars = s.server.env.clone();
                self.send(id, ServerMsg::Env { vars });
            }
            ClientMsg::OpenFile { col, ctx, name: file } => {
                let dir = s.server.dir_of(&s.view, ctx);
                let from = match ctx {
                    ExecCtx::Window(w) => Some(w),
                    _ => None,
                };
                props.push(match s.server.open_file(col, from, &dir, &file, None) {
                    Ok(p) => p,
                    Err(e) => Proposal::Errors { dir: Some(dir.to_string_lossy().to_string()), text: format!("{e}\n") },
                });
            }
            ClientMsg::Plumb { ctx, text } => props.push(s.server.plumb(&s.view, ctx, &text)),
            ClientMsg::Complete { view, ctx, at, prefix } => {
                let dir = s.server.dir_of(&s.view, ctx);
                props.push(s.server.complete(view, at, &dir, &prefix));
            }
            ClientMsg::Propose { id: tool_id, proposal } => {
                // a tool's proposal: hand it to the leader, remembering who
                // waits for the answer
                let pid = self.next_pending;
                self.next_pending += 1;
                self.pending.insert(pid, Pending { conn: id, id: tool_id });
                self.propose(name, pid, proposal);
            }
            ClientMsg::Applied { id: pid, result } => {
                if is_leader {
                    if let Some(p) = self.pending.remove(&pid) {
                        self.send(p.conn, ServerMsg::Applied { id: p.id, result });
                    }
                }
            }
            ClientMsg::Hello { .. } | ClientMsg::NewSession { .. } | ClientMsg::ListSessions | ClientMsg::RenameSession { .. } | ClientMsg::Ping { .. } | ClientMsg::Stop => {}
        }
        self.after(name, props);
    }

    /// Route one proposal to the session's leader: the UI, or the daemon
    /// itself when none is attached.
    fn propose(&mut self, name: &str, pid: u64, p: Proposal) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        match s.leader {
            Some(leader) => self.send(leader, ServerMsg::Propose { id: pid, proposal: p }),
            None => {
                let result = proposal::apply(&mut s.view, &mut s.log, p).map_err(|e| e.to_string());
                if let Some(w) = self.pending.remove(&pid) {
                    self.send(w.conn, ServerMsg::Applied { id: w.id, result });
                }
            }
        }
    }

    /// Forward new entries to every connection of the session, and the
    /// server's proposals to its leader.
    fn after(&mut self, name: &str, mut props: Vec<Proposal>) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        s.server.close_orphan_terms(&mut s.log, &s.view);
        s.server.sync_watches(&s.view);
        let _ = s.view.catch_up(&s.log);
        // the daemon leads: its own proposals apply here and now
        if s.leader.is_none() {
            loop {
                for p in std::mem::take(&mut props) {
                    if let Err(e) = proposal::apply(&mut s.view, &mut s.log, p) {
                        eprintln!("apexd: {name}: proposal: {e}");
                    }
                }
                let _ = s.view.update_tags(&mut s.log);
                // what the daemon just did may have handed the server more
                props = s.server.poll_execs(&mut s.log, &s.view);
                s.server.close_orphan_terms(&mut s.log, &s.view);
                let _ = s.view.catch_up(&s.log);
                if props.is_empty() {
                    break;
                }
            }
        }
        // the metalog first: it announces shards before their entries
        let mut shards: Vec<Shard> = s.log.shards().collect();
        shards.sort_by_key(|sh| *sh != Shard::Meta);
        let sid = s.id;
        let members: Vec<u64> = self.conns.iter().filter(|(_, c)| c.session == Some(sid)).map(|(id, _)| *id).collect();
        for id in members {
            let c = self.conns.get_mut(&id).unwrap();
            for &shard in &shards {
                let from = c.sent.get(&shard).copied().unwrap_or(0);
                let new = s.log.since(shard, from);
                if !new.is_empty() {
                    let entries = new.to_vec();
                    c.sent.insert(shard, entries.last().unwrap().seq);
                    let _ = c.out.send(ServerMsg::Entries { shard, entries });
                }
            }
        }
        let s = self.sessions.get_mut(name).unwrap();
        if let Some(leader) = s.leader {
            for p in props {
                self.send(leader, ServerMsg::Propose { id: 0, proposal: p });
            }
        }
    }
}
