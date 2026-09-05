//! The session daemon: owns the authoritative `Log`, hosts the [`Server`],
//! keeps a follower replica of the whole session, and speaks the attach
//! protocol to clients over a Unix socket.
//!
//! One thread owns all state. Connection readers and terminal events feed
//! it through a channel; each connection has a writer thread draining its
//! own outbound queue, so a slow client never stalls the core.

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use apex_core::*;

use crate::proto::{read_frame, write_frame, ClientMsg, ServerMsg};
use crate::{Proposal, Server, ServerEvent};

enum Event {
    Accept(UnixStream),
    Msg(u64, ClientMsg),
    Gone(u64),
    Server(ServerEvent),
}

struct Conn {
    attachment: Option<AttachmentId>,
    out: Sender<ServerMsg>,
}

pub struct Daemon {
    log: Log,
    server: Server,
    /// Follower replica of every shard: what the server performs execs
    /// against and what new attachments get as their snapshot.
    view: Node,
    conns: HashMap<u64, Conn>,
    /// The UI attachment currently leading (single-player mode).
    leader: Option<u64>,
    /// How far each shard has been forwarded to the leader.
    sent: HashMap<Shard, Seq>,
    session: String,
    rx: Receiver<Event>,
    tx: Sender<Event>,
}

/// Where `apexd` listens by default: `$TMPDIR/apex-$USER/main.sock`.
pub fn default_socket() -> std::path::PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(base).join(format!("apex-{}", std::env::var("USER").unwrap_or_default())).join("main.sock")
}

impl Daemon {
    /// Listen on `path` (removed first if stale) and run until the process
    /// ends. Returns only on a listener error.
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
        let mut log = Log::new();
        let (server, mut srx) = Server::new(&log);
        {
            let tx = tx.clone();
            thread::spawn(move || {
                use futures::StreamExt;
                futures::executor::block_on(async move {
                    while let Some(ev) = srx.next().await {
                        if tx.send(Event::Server(ev)).is_err() {
                            break;
                        }
                    }
                });
            });
        }
        let mut view = Node::new(AttachmentId(u64::MAX));
        view.catch_up(&mut log).expect("fresh log");
        let mut d = Daemon {
            log,
            server,
            view,
            conns: HashMap::new(),
            leader: None,
            sent: HashMap::new(),
            session: session.to_string(),
            rx,
            tx,
        };
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
                Event::Server(ev) => {
                    let props = d.server.pump(&mut d.log, ev);
                    d.after(props);
                }
            }
        }
        Ok(())
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
            loop {
                match read_frame::<_, ClientMsg>(&mut r) {
                    Ok(Some(m)) => {
                        if tx.send(Event::Msg(id, m)).is_err() {
                            break;
                        }
                    }
                    _ => break,
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
        self.conns.insert(id, Conn { attachment: None, out });
    }

    fn send(&self, id: u64, m: ServerMsg) {
        if let Some(c) = self.conns.get(&id) {
            let _ = c.out.send(m);
        }
    }

    fn gone(&mut self, id: u64) {
        if let Some(c) = self.conns.remove(&id) {
            if let Some(a) = c.attachment {
                let e = self.log.detach(a);
                let _ = self.view.state.apply(Shard::Meta, &e);
            }
            if self.leader == Some(id) {
                self.leader = None;
                // leases stay with the departed attachment until the next
                // attach reclaims them (§4: nothing is lost)
            }
        }
    }

    fn handle(&mut self, id: u64, m: ClientMsg) {
        match m {
            ClientMsg::Hello { session, name } => {
                if session != self.session {
                    self.send(id, ServerMsg::Error { text: format!("no session {session}") });
                    return;
                }
                let (a, e) = self.log.attach(AttachmentKind::Ui, &name);
                let _ = self.view.state.apply(Shard::Meta, &e);
                // transfer else reclaim: single-player, so reclaim now
                for shard in self.log.shards().collect::<Vec<_>>() {
                    if shard.is_pinned() {
                        continue;
                    }
                    let l = self.log.lease(shard).expect("lease");
                    if l.holder != SERVER && l.released.is_none() {
                        if let Ok(e) = self.log.reclaim(shard) {
                            let _ = self.view.state.apply(Shard::Meta, &e);
                        }
                    }
                    if let Ok(e) = self.log.grant(shard, a) {
                        let _ = self.view.state.apply(Shard::Meta, &e);
                    }
                }
                let _ = self.view.catch_up(&self.log);
                let snapshot = self.view.state.to_snapshot();
                if let Some(c) = self.conns.get_mut(&id) {
                    c.attachment = Some(a);
                }
                self.leader = Some(id);
                self.sent.clear();
                for shard in self.log.shards().collect::<Vec<_>>() {
                    self.sent.insert(shard, self.log.last_seq(shard));
                }
                self.send(id, ServerMsg::Welcome { attachment: a, snapshot });
                // an empty session: hand the first column to the leader by
                // letting it run init itself (it leads Layout)
            }
            ClientMsg::Append { shard, entries } => {
                let mut last = 0;
                for e in entries {
                    let seq = e.seq;
                    match self.log.append_entry(shard, e) {
                        Ok(()) => last = seq,
                        Err(err) => {
                            self.send(id, ServerMsg::Error { text: format!("{shard} seq {seq}: {err}") });
                            break;
                        }
                    }
                }
                let _ = self.view.catch_up(&self.log);
                // the leader's own entries need no forwarding
                self.sent.insert(shard, self.log.last_seq(shard));
                if last > 0 {
                    self.send(id, ServerMsg::Ack { shard, seq: last });
                }
                let props = self.server.poll_execs(&mut self.log, &self.view);
                self.after(props);
            }
            ClientMsg::CreateShard { shard } => {
                let Some(a) = self.conns.get(&id).and_then(|c| c.attachment) else { return };
                match self.log.create_shard(shard, a) {
                    Ok(_) => {
                        // the shard's own log is empty; nothing to forward
                        self.sent.insert(shard, 0);
                        let _ = self.view.catch_up(&self.log);
                        self.send(id, ServerMsg::ShardReady { shard });
                    }
                    Err(e) => self.send(id, ServerMsg::Error { text: format!("create {shard}: {e}") }),
                }
                self.after(Vec::new());
            }
            ClientMsg::DeleteShard { shard } => {
                let _ = self.log.delete_shard(shard);
                self.sent.remove(&shard);
                let _ = self.view.catch_up(&self.log);
                self.after(Vec::new());
            }
            ClientMsg::TermKey { term, key } => self.server.term_key(term, &key),
            ClientMsg::TermPaste { term, text } => self.server.term_paste(term, &text),
            ClientMsg::TermResize { term, cols, rows } => {
                self.server.term_resize(&mut self.log, term, cols, rows);
                self.after(Vec::new());
            }
            ClientMsg::TermScroll { term, delta } => {
                self.server.term_scroll(&mut self.log, term, delta as isize);
                self.after(Vec::new());
            }
            ClientMsg::OpenFile { col, ctx, name } => {
                let dir = self.server.dir_of(&self.view, ctx);
                let p = match self.server.open_file(col, &dir, &name, None) {
                    Ok(p) => p,
                    Err(e) => Proposal::Errors { col, text: format!("{e}\n") },
                };
                self.after(vec![p]);
            }
            ClientMsg::Plumb { ctx, text } => {
                let p = self.server.plumb(&self.view, ctx, &text);
                self.after(vec![p]);
            }
            ClientMsg::Ping { t } => self.send(id, ServerMsg::Pong { t }),
        }
    }

    /// Forward new server-side entries and proposals to the leader.
    fn after(&mut self, props: Vec<Proposal>) {
        self.server.close_orphan_terms(&mut self.log, &self.view);
        let _ = self.view.catch_up(&self.log);
        let Some(leader) = self.leader else { return };
        // the metalog first: it announces shards before their entries
        let mut shards: Vec<Shard> = self.log.shards().collect();
        shards.sort_by_key(|s| *s != Shard::Meta);
        for shard in shards {
            let from = self.sent.get(&shard).copied().unwrap_or(0);
            let new = self.log.since(shard, from);
            if !new.is_empty() {
                let entries = new.to_vec();
                self.sent.insert(shard, entries.last().unwrap().seq);
                self.send(leader, ServerMsg::Entries { shard, entries });
            }
        }
        for p in props {
            self.send(leader, ServerMsg::Propose(p));
        }
    }
}
