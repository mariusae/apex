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
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use apex_core::*;

use crate::proto::{read_frame, write_frame, ClientMsg, ServerMsg};
use crate::{proposal, Proposal, Server, ServerEvent};

/// Where `apexd` listens by default: `$TMPDIR/apex-$USER/main.sock`.
pub fn default_socket() -> std::path::PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(base).join(format!("apex-{}", std::env::var("USER").unwrap_or_default())).join("main.sock")
}

enum Event {
    Accept(UnixStream),
    Msg(u64, ClientMsg),
    Gone(u64),
    Server(String, ServerEvent),
}

struct Conn {
    session: Option<String>,
    attachment: Option<AttachmentId>,
    kind: AttachmentKind,
    out: Sender<ServerMsg>,
    /// How far each shard has been forwarded to this connection.
    sent: HashMap<Shard, Seq>,
}

struct Session {
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
    sessions: BTreeMap<String, Session>,
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
        let mut d = Daemon { sessions: BTreeMap::new(), conns: HashMap::new(), pending: HashMap::new(), next_pending: 1, rx, tx };
        d.new_session(session);
        let mut next_id = 1u64;
        while let Ok(ev) = d.rx.recv() {
            match ev {
                Event::Accept(s) => {
                    let id = next_id;
                    next_id += 1;
                    d.accept(id, s);
                }
                Event::Msg(id, m) => d.handle(id, m),
                Event::Gone(id) => d.gone(id),
                Event::Server(name, ev) => {
                    if let Some(s) = d.sessions.get_mut(&name) {
                        let props = s.server.pump(&mut s.log, &s.view, ev);
                        d.after(&name, props);
                    }
                }
            }
        }
        Ok(())
    }

    fn new_session(&mut self, name: &str) -> bool {
        if self.sessions.contains_key(name) {
            return false;
        }
        let log = Log::new();
        let (server, mut srx) = Server::new(&log);
        {
            let tx = self.tx.clone();
            let name = name.to_string();
            thread::spawn(move || {
                use futures::StreamExt;
                futures::executor::block_on(async move {
                    while let Some(ev) = srx.next().await {
                        if tx.send(Event::Server(name.clone(), ev)).is_err() {
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
        self.sessions.insert(name.to_string(), Session { log, server, view, leader: None });
        true
    }

    fn accept(&mut self, id: u64, s: UnixStream) {
        let (out, orx) = channel::<ServerMsg>();
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
        let (Some(name), Some(a)) = (c.session, c.attachment) else { return };
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
            ClientMsg::NewSession { name } => {
                if self.new_session(&name) {
                    self.send(id, ServerMsg::Sessions { names: self.sessions.keys().cloned().collect() });
                } else {
                    self.send(id, ServerMsg::Error { text: format!("session {name} exists") });
                }
            }
            ClientMsg::ListSessions => {
                self.send(id, ServerMsg::Sessions { names: self.sessions.keys().cloned().collect() });
            }
            ClientMsg::Ping { t } => self.send(id, ServerMsg::Pong { t }),
            other => {
                let Some(name) = self.conns.get(&id).and_then(|c| c.session.clone()) else {
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
        if let Some(c) = self.conns.get_mut(&id) {
            c.session = Some(session);
            c.attachment = Some(a);
            c.kind = kind;
            c.sent = marks;
        }
        self.send(id, ServerMsg::Welcome { attachment: a, snapshot });
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
            ClientMsg::TermKey { term, key } => s.server.term_key(term, &key),
            ClientMsg::TermPaste { term, text } => s.server.term_paste(term, &text),
            ClientMsg::TermResize { term, cols, rows } => s.server.term_resize(&mut s.log, term, cols, rows),
            ClientMsg::TermScroll { term, delta } => s.server.term_scroll(&mut s.log, term, delta as isize),
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
            ClientMsg::Hello { .. } | ClientMsg::NewSession { .. } | ClientMsg::ListSessions | ClientMsg::Ping { .. } => {}
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
        let members: Vec<u64> = self.conns.iter().filter(|(_, c)| c.session.as_deref() == Some(name)).map(|(id, _)| *id).collect();
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
