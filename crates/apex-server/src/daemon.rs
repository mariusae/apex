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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{self, BufReader, BufWriter};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use apex_core::*;

use crate::proto::{read_frame, write_frame, ClientMsg, ServerMsg, Script};
use crate::{PlumbReq, PlumbStep, proposal, Proposal, Server, ServerEvent};

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
    cmd.args([&format!("-socket={}", socket.to_string_lossy()), &format!("-session={session}"), "server"])
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
    /// A tool asked to plumb has not answered in time.
    PlumbTimeout(u64),
}

struct Conn {
    /// The id of the session attached to.
    session: Option<u64>,
    attachment: Option<AttachmentId>,
    kind: AttachmentKind,
    out: Sender<ServerMsg>,
    /// How far each shard has been forwarded to this connection.
    sent: HashMap<Shard, Seq>,
    /// Files this connection subscribed to (`Watch`).
    watched: BTreeSet<String>,
    /// Programs adopted at this connection's word (`Named`), forgotten
    /// when it goes.
    adopted: Vec<u32>,
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

/// Something in flight at the leader, by our id for it.
enum Pending {
    /// A tool's proposal: (tool connection, tool's id).
    Tool { conn: u64, id: u64 },
    /// A plumb walk's `Ask`: (session id, plumb id, who asked).
    Plumb { session: u64, plumb: u64, asker: u64 },
}

pub struct Daemon {
    socket: PathBuf,
    /// The host's init file for new sessions (`~/.apex/init`).
    host_profile: Option<PathBuf>,
    sessions: BTreeMap<String, Session>,
    next_session: u64,
    conns: HashMap<u64, Conn>,
    pending: HashMap<u64, Pending>,
    next_pending: u64,
    /// Plumbs handed to tools: our id → (session id, plumb id, asker).
    tool_plumbs: HashMap<u64, (u64, u64, u64)>,
    next_tool_plumb: u64,
    rx: Receiver<Event>,
    tx: Sender<Event>,
}

impl Daemon {
    /// Listen on `path` (removed first if stale) and run until the process
    /// ends, with one session `session` to begin with. Returns only on a
    /// listener error.
    pub fn run(path: &Path, session: &str) -> io::Result<()> {
        let host_profile = std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".apex/profile"));
        Self::run_with(path, session, host_profile)
    }

    /// `run`, with the host's init file given (tests keep it out of `$HOME`).
    pub fn run_with(path: &Path, session: &str, host_profile: Option<PathBuf>) -> io::Result<()> {
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
        let mut d = Daemon { socket: path.to_path_buf(), host_profile, sessions: BTreeMap::new(), next_session: 1, conns: HashMap::new(), pending: HashMap::new(), next_pending: 1, tool_plumbs: HashMap::new(), next_tool_plumb: 1, rx, tx };
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
                        let changed = s.server.take_changed();
                        d.after(&name, props);
                        for p in changed {
                            d.file_changed(sid, &p);
                        }
                    }
                }
                Event::PlumbTimeout(tid) => d.tool_answered(tid, Err("no answer in time".into())),
            }
        }
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    fn new_session(&mut self, name: &str, profile: Option<Script>) -> bool {
        if self.sessions.contains_key(name) {
            return false;
        }
        let log = Log::new();
        let (mut server, mut srx) = Server::new(&log);
        // shells and commands in this session know it, and the daemon
        server.env = vec![("apexsession".into(), name.to_string()), ("APEX_SOCKET".into(), self.socket.display().to_string())];
        // $EDITOR opens in the session and returns when the window goes
        if let Some(editor) = editor_command() {
            server.env.push(("EDITOR".into(), editor));
        }
        if let Some(i) = &profile {
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
        let mut log = log;
        server.install_default_rules(&mut log);
        let mut view = Node::new(SERVER);
        view.catch_up(&log).expect("fresh log");
        // the daemon lays the session out (one column, the top tag) so a
        // tool can work before any UI attaches
        view.init_session(&mut log).expect("fresh session");
        self.sessions.insert(name.to_string(), Session { id: sid, log, server, view, leader: None });
        // its init runs now, as a command of the session
        let s = self.sessions.get_mut(name).unwrap();
        s.server.run_profile(&s.view, self.host_profile.as_deref(), profile.as_ref());
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
        let _ = out.send(ServerMsg::Build { protocol: crate::proto::PROTOCOL, id: crate::BUILD_ID.to_string() });
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
        self.conns.insert(id, Conn { session: None, attachment: None, kind: AttachmentKind::Tool, out, sent: HashMap::new(), watched: BTreeSet::new(), adopted: Vec::new() });
    }

    fn send(&self, id: u64, m: ServerMsg) {
        if let Some(c) = self.conns.get(&id) {
            let _ = c.out.send(m);
        }
    }

    fn gone(&mut self, id: u64) {
        if let Some(name) = self.conns.get(&id).and_then(|c| c.session).and_then(|sid| self.name_of(sid)) {
            self.drop_watches(id, &name);
        }
        let Some(c) = self.conns.remove(&id) else { return };
        let (Some(sid), Some(a)) = (c.session, c.attachment) else { return };
        let Some(name) = self.name_of(sid) else { return };
        let Some(s) = self.sessions.get_mut(&name) else { return };
        for pid in &c.adopted {
            s.server.forget_process(*pid);
        }
        for rid in apex_core::plumb::owned_by(&s.view.state.meta.rules, a) {
            let e = s.log.remove_rule(rid);
            let _ = s.view.state.apply(Shard::Meta, &e);
        }
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
        self.pending.retain(|_, p| !matches!(p, Pending::Tool { conn, .. } if *conn == id));
        self.after(&name, Vec::new());
    }

    fn handle(&mut self, id: u64, m: ClientMsg) {
        match m {
            ClientMsg::Hello { session, name, kind, attach } => self.hello(id, session, name, kind, attach),
            ClientMsg::NewSession { name, profile } => {
                // making a session that exists is fine: it is there
                if name.is_empty() || name.contains('/') {
                    self.send(id, ServerMsg::Error { text: format!("bad session name {name:?}") });
                } else {
                    self.new_session(&name, profile);
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

    fn hello(&mut self, id: u64, session: String, name: String, kind: AttachmentKind, attach: Option<Script>) {
        let Some(s) = self.sessions.get_mut(&session) else {
            self.send(id, ServerMsg::Error { text: format!("no session {session}") });
            return;
        };
        let (a, e) = s.log.attach(kind, &name);
        let _ = s.view.state.apply(Shard::Meta, &e);
        let mut fenced_old = None;
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
            fenced_old = s.leader.replace(id).filter(|old| *old != id);
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
        if let Some(old) = fenced_old {
            // the UI that led is fenced now: what it watched ends
            self.drop_watches(old, &session);
        }
        // the client's attach script runs now, as the attachment's own
        if let Some(script) = attach {
            let s = self.sessions.get_mut(&session).unwrap();
            s.server.run_attach(&s.view, a, &script);
        }
        // the others learn from the metalog that the leases moved
        self.after(&session, Vec::new());
    }

    fn in_session(&mut self, id: u64, name: &str, m: ClientMsg) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        let is_leader = s.leader == Some(id);
        let mut props = Vec::new();
        let mut verbs = false;
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
                verbs = true;
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
            ClientMsg::TermRead { term, from, to } => {
                let text = s.server.term(term).map(|h| h.text((0, from), (h.cols, to))).unwrap_or_default();
                self.send(id, ServerMsg::TermLines { term, text });
                return;
            }
            ClientMsg::TermResize { term, cols, rows } => s.server.term_resize(&mut s.log, term, cols, rows),
            ClientMsg::TermScroll { term, delta, at } => s.server.term_wheel(&mut s.log, term, delta as isize, at),
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
            ClientMsg::Plumb { ctx, text, dir, edit_only, dry, at, sel, alt, reverse } => {
                let req = PlumbReq { ctx, text, dir: dir.map(PathBuf::from), verb: "plumb".into(), edit_only, dry, exec: None, at, sel, alt, reverse };
                let (pid, step) = s.server.plumb_start(&s.view, req);
                self.drive(name, pid, step, id);
                return;
            }
            ClientMsg::PlumbAck { id: tid, ok } => {
                self.tool_answered(tid, if ok { Ok(()) } else { Err("refused".into()) });
                return;
            }
            ClientMsg::RuleAdd { rule, priority, mine } => {
                if let Err(e) = rule.check() {
                    self.send(id, ServerMsg::Error { text: format!("rule: {e}") });
                    return;
                }
                let owner = if mine { self.conns.get(&id).and_then(|c| c.attachment).unwrap_or(SERVER) } else { SERVER };
                let (rid, e) = s.log.install_rule(owner, priority, rule);
                let _ = s.view.state.apply(Shard::Meta, &e);
                self.send(id, ServerMsg::RuleAdded { id: rid });
            }
            ClientMsg::RuleRm { id: rid } => {
                let e = s.log.remove_rule(rid);
                let _ = s.view.state.apply(Shard::Meta, &e);
            }
            ClientMsg::Set { key, value, attachment } => {
                let owner = match attachment {
                    Some(a) if s.view.state.meta.attachments.contains_key(&a) => a,
                    Some(a) => {
                        self.send(id, ServerMsg::Error { text: format!("set: no attachment {a}") });
                        return;
                    }
                    None => SERVER,
                };
                let e = s.log.set(owner, &key, &value);
                let _ = s.view.state.apply(Shard::Meta, &e);
            }
            ClientMsg::ReadFile { path } => {
                let bytes = std::fs::read(&path).map_err(|e| e.to_string());
                self.send(id, ServerMsg::File { path, bytes });
                return;
            }
            ClientMsg::Ps => {
                let procs = s.server.processes();
                self.send(id, ServerMsg::Ps { procs });
                return;
            }
            ClientMsg::Named { name: pname, group, pid, cmd } => {
                if let Some(pid) = s.server.name_process(&pname, group, pid, &cmd) {
                    if let Some(c) = self.conns.get_mut(&id) {
                        c.adopted.push(pid);
                    }
                }
            }
            ClientMsg::Kill { targets } => {
                for t in &targets {
                    s.server.kill(t);
                }
                // a moment for the groups to go, then what is left
                std::thread::sleep(std::time::Duration::from_millis(100));
                let procs = s.server.processes();
                self.send(id, ServerMsg::Ps { procs });
                return;
            }
            ClientMsg::Watch { path } => {
                s.server.subscribe(&s.view, Path::new(&path));
                if let Some(c) = self.conns.get_mut(&id) {
                    c.watched.insert(path.clone());
                }
                let bytes = std::fs::read(&path).map_err(|e| e.to_string());
                self.send(id, ServerMsg::File { path, bytes });
                return;
            }
            ClientMsg::Unwatch { path } => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.watched.remove(&path);
                }
                self.unwatch_unused(name, &path);
                return;
            }
            ClientMsg::Complete { view, ctx, at, prefix } => {
                let dir = s.server.dir_of(&s.view, ctx);
                props.push(s.server.complete(view, at, &dir, &prefix));
            }
            ClientMsg::Propose { id: tool_id, proposal } => {
                // a tool's proposal: hand it to the leader, remembering who
                // waits for the answer
                let pid = self.next_pending;
                self.next_pending += 1;
                self.pending.insert(pid, Pending::Tool { conn: id, id: tool_id });
                self.propose(name, pid, proposal);
            }
            ClientMsg::Applied { id: pid, result } => {
                if is_leader {
                    self.answered(pid, result);
                }
            }
            ClientMsg::Hello { .. } | ClientMsg::NewSession { .. } | ClientMsg::ListSessions | ClientMsg::RenameSession { .. } | ClientMsg::Ping { .. } | ClientMsg::Stop => {}
        }
        self.after(name, props);
        if verbs {
            self.start_verbs(name);
        }
    }

    /// Route one proposal to the session's leader: the UI, or the daemon
    /// itself when none is attached.
    fn propose(&mut self, name: &str, pid: u64, p: Proposal) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        match s.leader {
            Some(leader) => self.send(leader, ServerMsg::Propose { id: pid, proposal: p }),
            None => {
                let result = proposal::apply(&mut s.view, &mut s.log, p).map_err(|e| e.to_string());
                // what it did reaches everyone before the answer does, so a
                // tool's replica has the window its proposal made
                self.after(name, Vec::new());
                self.answered(pid, result);
            }
        }
    }

    /// A subscribed file changed: every connection of the session that
    /// watches it gets the bytes.
    fn file_changed(&mut self, sid: u64, path: &Path) {
        let p = path.to_string_lossy().to_string();
        let watchers: Vec<u64> = self.conns.iter().filter(|(_, c)| c.session == Some(sid) && c.watched.contains(&p)).map(|(id, _)| *id).collect();
        if watchers.is_empty() {
            return;
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string());
        for id in watchers {
            self.send(id, ServerMsg::File { path: p.clone(), bytes: bytes.clone() });
        }
    }

    /// Stop watching `path` for the session unless another connection
    /// still wants it.
    fn unwatch_unused(&mut self, name: &str, path: &str) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        let sid = s.id;
        let wanted = self.conns.values().any(|c| c.session == Some(sid) && c.watched.contains(path));
        if !wanted {
            s.server.unsubscribe(&s.view, Path::new(path));
        }
    }

    /// Drop every subscription of a connection (it is gone, or fenced).
    fn drop_watches(&mut self, id: u64, name: &str) {
        let paths: Vec<String> = self.conns.get_mut(&id).map(|c| std::mem::take(&mut c.watched).into_iter().collect()).unwrap_or_default();
        for p in paths {
            self.unwatch_unused(name, &p);
        }
    }

    /// The leader's answer to something in flight.
    fn answered(&mut self, pid: u64, result: Result<Option<WindowId>, String>) {
        match self.pending.remove(&pid) {
            Some(Pending::Tool { conn, id }) => self.send(conn, ServerMsg::Applied { id, result }),
            Some(Pending::Plumb { session, plumb, asker }) => {
                let Some(name) = self.name_of(session) else { return };
                let s = self.sessions.get_mut(&name).unwrap();
                let step = s.server.plumb_next(&s.view, plumb, result.map(|_| ()));
                self.drive(&name, plumb, step, asker);
            }
            None => {}
        }
    }

    /// A tool's answer (or its silence) to a plumb handed to it.
    fn tool_answered(&mut self, tid: u64, outcome: Result<(), String>) {
        let Some((sid, plumb, asker)) = self.tool_plumbs.remove(&tid) else { return };
        let Some(name) = self.name_of(sid) else { return };
        let s = self.sessions.get_mut(&name).unwrap();
        let step = s.server.plumb_next(&s.view, plumb, outcome);
        self.drive(&name, plumb, step, asker);
    }

    /// Carry out one step of a plumb walk, and whatever follows from it.
    fn drive(&mut self, name: &str, plumb: u64, step: PlumbStep, asker: u64) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        let sid = s.id;
        match step {
            PlumbStep::Done(props) => self.after(name, props),
            PlumbStep::Trace(lines) => self.send(asker, ServerMsg::PlumbTrace { lines }),
            PlumbStep::Ask(proposal) => {
                let pid = self.next_pending;
                self.next_pending += 1;
                self.pending.insert(pid, Pending::Plumb { session: sid, plumb, asker });
                self.propose(name, pid, proposal);
            }
            PlumbStep::AskTool { tool, ctx, verb, text, dir, groups, at, sel } => {
                // the tool attached under that name, in this session
                let found = self.conns.iter().find(|(_, c)| c.session == Some(sid) && c.attachment.is_some_and(|a| s.view.state.meta.attachments.get(&a).is_some_and(|x| x.name == tool))).map(|(id, _)| *id);
                match found {
                    Some(cid) => {
                        let tid = self.next_tool_plumb;
                        self.next_tool_plumb += 1;
                        self.tool_plumbs.insert(tid, (sid, plumb, asker));
                        self.send(cid, ServerMsg::Plumb { id: tid, ctx, verb, text, dir, groups, at, sel });
                        // a second, then it is taken as refused
                        let tx = self.tx.clone();
                        thread::spawn(move || {
                            thread::sleep(std::time::Duration::from_secs(1));
                            let _ = tx.send(Event::PlumbTimeout(tid));
                        });
                    }
                    None => {
                        let step = s.server.plumb_next(&s.view, plumb, Err(format!("no tool {tool} attached")));
                        self.drive(name, plumb, step, asker);
                    }
                }
            }
        }
    }

    /// Verb execs the server found while polling: each starts a walk.
    fn start_verbs(&mut self, name: &str) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        let starts = s.server.take_plumb_starts();
        for req in starts {
            let s = self.sessions.get_mut(name).unwrap();
            let (pid, step) = s.server.plumb_start(&s.view, req);
            self.drive(name, pid, step, 0);
        }
    }

    /// Forward new entries to every connection of the session, and the
    /// server's proposals to its leader.
    /// (see `editor_command`)
    fn after(&mut self, name: &str, props: Vec<Proposal>) {
        let Some(s) = self.sessions.get_mut(name) else { return };
        // starts first: a command started by what we just did (the attach
        // script, a rename) is named before anything reports its end
        let mut props = { let mut all = s.server.take_started(); all.extend(props); all };
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
                // places to go whose windows are not open: open them, land
                for loc in s.view.take_gotos() {
                    let col = s.view.state.layout.cols.first().map(|c| c.id);
                    let dir = PathBuf::from(&loc.name).parent().map(|d| d.to_path_buf()).unwrap_or_default();
                    if let Some(col) = col {
                        if let Ok(p) = s.server.open_file(col, None, &dir, &loc.name, None) {
                            let _ = proposal::apply(&mut s.view, &mut s.log, p);
                            let _ = s.view.land(&mut s.log, &loc);
                        }
                    }
                }
                // what the daemon just did may have handed the server more
                props = s.server.poll_execs(&mut s.log, &s.view);
                s.server.close_orphan_terms(&mut s.log, &s.view);
                let _ = s.view.catch_up(&s.log);
                if props.is_empty() {
                    break;
                }
            }
        }
        let verbs = !s.server.peek_plumb_starts().is_empty();
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
        if verbs {
            self.start_verbs(name);
        }
    }
}

/// What `$EDITOR` is in a session: one word, since `$EDITOR file` at a
/// zsh or rc prompt is not split into words (only sh and bash do that,
/// and git runs it under `sh -c`). So it is `apex-editor`, a link to the
/// binary beside it that the CLI recognises by its name and runs as
/// `apex editor`; the daemon makes the link when it is missing and the
/// directory allows. Failing that, `PATH/apex editor`, which still
/// works under `sh -c`.
fn editor_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let two_words = format!("{} editor", exe.display());
    if exe.file_name().and_then(|n| n.to_str()) != Some("apex") {
        return Some(two_words); // a test binary: leave its directory alone
    }
    let link = exe.with_file_name("apex-editor");
    if !link.exists() {
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink("apex", &link);
    }
    if link.exists() {
        Some(link.display().to_string())
    } else {
        Some(two_words)
    }
}
