//! The client side of the attach protocol. A [`Link`] is the connection:
//! the owner drives its `Node` over a mirror `Log` exactly as in-process,
//! calls [`Link::flush`] to ship what it sequenced, and feeds messages from
//! [`Link::rx`] to [`Link::handle`] to keep up with the server. [`Remote`]
//! bundles a link with its log and node for headless clients.

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use apex_core::log::MirrorHook;
use apex_core::*;

use crate::proto::{read_frame, write_frame, ClientMsg, ServerMsg, SessionInit};
use crate::{proposal, Proposal};

/// A shared, buffered writer: the mirror hook and the owner both send.
#[derive(Clone)]
pub struct Outbound(Arc<Mutex<BufWriter<Box<dyn Write + Send>>>>);

impl Outbound {
    pub fn send(&self, m: &ClientMsg) -> io::Result<()> {
        let mut w = self.0.lock().unwrap();
        write_frame(&mut *w, m)?;
        w.flush()
    }
}

struct Hook(Outbound);

impl MirrorHook for Hook {
    fn create_shard(&mut self, shard: Shard, _creator: AttachmentId) {
        let _ = self.0.send(&ClientMsg::CreateShard { shard });
    }
    fn delete_shard(&mut self, shard: Shard) {
        let _ = self.0.send(&ClientMsg::DeleteShard { shard });
    }
}

pub struct Link {
    pub attachment: AttachmentId,
    out: Outbound,
    /// Messages from the server, delivered by the reader thread.
    pub rx: Receiver<ServerMsg>,
    /// How far each led shard has been shipped.
    sent: HashMap<Shard, Seq>,
    /// Latest `Ack` per shard.
    pub acked: HashMap<Shard, Seq>,
    /// Windows made by proposals since the last `take_made`.
    made: Vec<WindowId>,
    /// Answers to this tool's proposals, by its ids.
    pub applied: HashMap<u64, Result<Option<WindowId>, String>>,
    /// The last session listing received.
    pub sessions: Option<Vec<String>>,
    /// The session environment, after an `Env`.
    pub env: Option<Vec<(String, String)>>,
    /// When the last `Pong` arrived (the owner's heartbeat).
    pub last_pong: Option<std::time::Instant>,
    next_id: u64,
    /// Closes the transport on drop, so the reader thread ends and the
    /// server sees the attachment go.
    closer: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Some(c) = self.closer.take() {
            c();
        }
    }
}

/// Called by the reader thread after queuing a message, so an event loop
/// that cannot block on `rx` knows to poll.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

impl Link {
    /// Connect to the daemon at `path` and attach to `session`. Blocks for
    /// the welcome; returns the link and the mirror log and node to drive.
    pub fn connect(path: &Path, session: &str, name: &str, kind: AttachmentKind, wake: Option<Wake>) -> io::Result<(Link, Log, Node)> {
        let stream = UnixStream::connect(path)?;
        Self::over(stream, session, name, kind, wake)
    }

    pub fn over(stream: UnixStream, session: &str, name: &str, kind: AttachmentKind, wake: Option<Wake>) -> io::Result<(Link, Log, Node)> {
        let w = stream.try_clone()?;
        let closer = stream.try_clone()?;
        Self::over_streams(
            Box::new(stream),
            Box::new(w),
            Some(Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            })),
            session,
            name,
            kind,
            wake,
        )
    }

    /// `over_streams`, making the session first if the daemon has none
    /// of that name (a UI opening a session it was told to).
    pub fn over_streams_creating(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        closer: Option<Box<dyn FnOnce() + Send>>,
        session: &str,
        name: &str,
        kind: AttachmentKind,
        wake: Option<Wake>,
        init: Option<SessionInit>,
    ) -> io::Result<(Link, Log, Node)> {
        Self::over_streams_inner(reader, writer, closer, session, name, kind, wake, Some(init))
    }

    /// Attach over any byte stream pair: a child's stdout and stdin, say,
    /// with `ssh host apex attach --stdio session` as the child.
    pub fn over_streams(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        closer: Option<Box<dyn FnOnce() + Send>>,
        session: &str,
        name: &str,
        kind: AttachmentKind,
        wake: Option<Wake>,
    ) -> io::Result<(Link, Log, Node)> {
        Self::over_streams_inner(reader, writer, closer, session, name, kind, wake, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn over_streams_inner(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        closer: Option<Box<dyn FnOnce() + Send>>,
        session: &str,
        name: &str,
        kind: AttachmentKind,
        wake: Option<Wake>,
        create: Option<Option<SessionInit>>,
    ) -> io::Result<(Link, Log, Node)> {
        let out = Outbound(Arc::new(Mutex::new(BufWriter::new(writer))));
        let (tx, rx) = channel::<ServerMsg>();
        spawn_reader(reader, tx, wake);
        if let Some(init) = create {
            out.send(&ClientMsg::NewSession { name: session.to_string(), init })?;
        }
        out.send(&ClientMsg::Hello { session: session.to_string(), name: name.to_string(), kind })?;
        let (attachment, snapshot) = loop {
            match rx.recv().map_err(|_| io::Error::new(io::ErrorKind::ConnectionAborted, "closed before welcome"))? {
                ServerMsg::Welcome { attachment, snapshot } => break (attachment, snapshot),
                ServerMsg::Error { text } => return Err(io::Error::other(text)),
                _ => {}
            }
        };
        let state = State::from_snapshot(&snapshot).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let log = Log::mirror(&state, Box::new(Hook(out.clone())));
        let mut node = Node::new(attachment);
        node.state = state;
        node.catch_up(&log).map_err(io::Error::other)?;
        let mut sent = HashMap::new();
        for shard in log.shards() {
            sent.insert(shard, log.last_seq(shard));
        }
        Ok((Link { attachment, out, rx, sent, acked: HashMap::new(), made: Vec::new(), applied: HashMap::new(), sessions: None, env: None, last_pong: None, next_id: 1, closer }, log, node))
    }

    pub fn send(&self, m: &ClientMsg) {
        let _ = self.out.send(m);
    }

    /// Send a proposal to the leader; the answer arrives in `applied`
    /// under the returned id.
    pub fn propose(&mut self, p: Proposal) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ClientMsg::Propose { id, proposal: p });
        id
    }

    /// Ship every entry the node sequenced since the last flush.
    pub fn flush(&mut self, log: &Log) {
        let mut batch = Vec::new();
        for shard in log.shards().collect::<Vec<_>>() {
            let from = self.sent.get(&shard).copied().unwrap_or(0);
            let new = log.since(shard, from);
            if !new.is_empty() {
                let entries = new.to_vec();
                self.sent.insert(shard, entries.last().unwrap().seq);
                batch.push(ClientMsg::Append { shard, entries });
            }
        }
        if batch.is_empty() {
            return;
        }
        let mut w = self.out.0.lock().unwrap();
        for m in &batch {
            let _ = write_frame(&mut *w, m);
        }
        let _ = w.flush();
    }

    /// Apply one message from the server. Returns `false` when the
    /// connection is finished.
    pub fn handle(&mut self, node: &mut Node, log: &mut Log, m: ServerMsg) -> bool {
        match m {
            ServerMsg::Entries { shard, entries } => {
                for e in entries {
                    if let Err(err) = log.append_entry(shard, e) {
                        eprintln!("remote: {shard}: {err}");
                        break;
                    }
                }
                if let Err(e) = node.catch_up(log) {
                    eprintln!("remote: catch up: {e}");
                }
                // whatever the server just handed us is its own; keep the
                // shipping mark past it so we never echo a follower's entries
                let last = log.last_seq(shard);
                let leads = self.leads(log, shard);
                let mark = self.sent.entry(shard).or_insert(0);
                if *mark < last && !leads {
                    *mark = last;
                }
            }
            ServerMsg::Propose { id, proposal } => {
                let result = proposal::apply(node, log, proposal).map_err(|e| e.to_string());
                match &result {
                    Ok(Some(w)) => self.made.push(*w),
                    Ok(None) => {}
                    Err(e) => eprintln!("remote: proposal: {e}"),
                }
                self.flush(log);
                if id != 0 {
                    self.send(&ClientMsg::Applied { id, result });
                }
            }
            ServerMsg::Applied { id, result } => {
                self.applied.insert(id, result);
            }
            ServerMsg::Sessions { names } => {
                self.sessions = Some(names);
            }
            ServerMsg::Env { vars } => {
                self.env = Some(vars);
            }
            ServerMsg::Ack { shard, seq } => {
                self.acked.insert(shard, seq);
            }
            ServerMsg::ShardReady { .. } => {}
            ServerMsg::Welcome { .. } => {}
            ServerMsg::Error { text } => eprintln!("remote: server: {text}"),
            ServerMsg::Pong { .. } => {
                self.last_pong = Some(std::time::Instant::now());
            }
        }
        true
    }

    fn leads(&self, log: &Log, shard: Shard) -> bool {
        log.lease(shard).is_some_and(|l| l.holder == self.attachment && l.released.is_none())
    }

    /// Windows opened or looked in by proposals since the last call.
    pub fn take_made(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.made)
    }

    /// Drain everything queued from the server without blocking.
    pub fn poll(&mut self, node: &mut Node, log: &mut Log) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(m) => {
                    if !self.handle(node, log, m) {
                        return false;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return false,
            }
        }
    }
}

/// Ask a daemon for its sessions without attaching.
pub fn list_sessions(path: &Path) -> io::Result<Vec<String>> {
    let mut s = UnixStream::connect(path)?;
    write_frame(&mut s, &ClientMsg::ListSessions)?;
    let mut r = BufReader::new(s);
    loop {
        match read_frame::<_, ServerMsg>(&mut r)? {
            Some(ServerMsg::Sessions { names }) => return Ok(names),
            Some(ServerMsg::Error { text }) => return Err(io::Error::other(text)),
            Some(_) => {}
            None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no answer")),
        }
    }
}

/// This machine's `~/.apex/init` and name, for a session made from here.
pub fn local_init() -> Option<SessionInit> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    let script = std::fs::read_to_string(Path::new(&home).join(".apex/init")).ok()?;
    Some(SessionInit { client: crate::term::sysname(), script })
}

/// Create a session on a daemon, with its creator's init; fine if it
/// already exists.
pub fn new_session(path: &Path, name: &str, init: Option<SessionInit>) -> io::Result<()> {
    let mut s = UnixStream::connect(path)?;
    write_frame(&mut s, &ClientMsg::NewSession { name: name.to_string(), init })?;
    let mut r = BufReader::new(s);
    loop {
        match read_frame::<_, ServerMsg>(&mut r)? {
            Some(ServerMsg::Sessions { .. }) => return Ok(()),
            Some(ServerMsg::Error { text }) if text.contains("exists") => return Ok(()),
            Some(ServerMsg::Error { text }) => return Err(io::Error::other(text)),
            Some(_) => {}
            None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no answer")),
        }
    }
}

/// Rename a session on a daemon, without attaching.
pub fn rename_session(path: &Path, from: &str, to: &str) -> io::Result<()> {
    let mut s = UnixStream::connect(path)?;
    write_frame(&mut s, &ClientMsg::RenameSession { from: from.to_string(), to: to.to_string() })?;
    let mut r = BufReader::new(s);
    loop {
        match read_frame::<_, ServerMsg>(&mut r)? {
            Some(ServerMsg::Sessions { .. }) => return Ok(()),
            Some(ServerMsg::Error { text }) => return Err(io::Error::other(text)),
            Some(_) => {}
            None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no answer")),
        }
    }
}

/// A link with its own log and node: a headless client.
pub struct Remote {
    pub log: Log,
    pub node: Node,
    pub link: Link,
}

impl Remote {
    pub fn connect(path: &Path, session: &str, name: &str) -> io::Result<Remote> {
        Self::connect_as(path, session, name, AttachmentKind::Ui)
    }

    pub fn connect_as(path: &Path, session: &str, name: &str, kind: AttachmentKind) -> io::Result<Remote> {
        let (link, log, node) = Link::connect(path, session, name, kind, None)?;
        Ok(Remote { log, node, link })
    }

    /// Attach through a command's stdin/stdout (`ssh host apex attach
    /// --stdio`, or the same bridge run locally).
    pub fn via(cmd: &str, session: &str, name: &str, kind: AttachmentKind) -> io::Result<Remote> {
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let closer = Box::new(move || {
            let _ = child.kill();
        });
        let (link, log, node) = Link::over_streams(Box::new(stdout), Box::new(stdin), Some(closer), session, name, kind, None)?;
        Ok(Remote { log, node, link })
    }

    /// Propose and block for the answer.
    pub fn propose(&mut self, p: Proposal, timeout: std::time::Duration) -> Result<Option<WindowId>, String> {
        let id = self.link.propose(p);
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(r) = self.link.applied.remove(&id) {
                return r;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("timed out waiting for the leader".into());
            }
            match self.step(left) {
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("connection closed".into()),
            }
        }
    }

    pub fn attachment(&self) -> AttachmentId {
        self.link.attachment
    }

    /// Set session variables (none: just ask) and block for the
    /// environment that results.
    pub fn env(&mut self, set: Vec<(String, String)>, timeout: std::time::Duration) -> Result<Vec<(String, String)>, String> {
        self.link.env = None;
        self.send(&ClientMsg::Env { set });
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(v) = self.link.env.take() {
                return Ok(v);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("timed out waiting for the server".into());
            }
            match self.step(left) {
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("connection closed".into()),
            }
        }
    }

    pub fn send(&self, m: &ClientMsg) {
        self.link.send(m)
    }

    pub fn flush(&mut self) {
        self.link.flush(&self.log)
    }

    pub fn handle(&mut self, m: ServerMsg) -> bool {
        self.link.handle(&mut self.node, &mut self.log, m)
    }

    /// Block for the next message (up to `timeout`) and handle it.
    pub fn step(&mut self, timeout: std::time::Duration) -> Result<bool, std::sync::mpsc::RecvTimeoutError> {
        let m = self.link.rx.recv_timeout(timeout)?;
        Ok(self.handle(m))
    }

    pub fn acked(&self, shard: Shard) -> Seq {
        self.link.acked.get(&shard).copied().unwrap_or(0)
    }
}

fn spawn_reader(stream: Box<dyn Read + Send>, tx: Sender<ServerMsg>, wake: Option<Wake>) {
    thread::spawn(move || {
        let mut r = BufReader::new(stream);
        loop {
            match read_frame::<_, ServerMsg>(&mut r) {
                Ok(Some(m)) => {
                    if tx.send(m).is_err() {
                        break;
                    }
                    if let Some(w) = &wake {
                        w();
                    }
                }
                _ => break,
            }
        }
        if let Some(w) = &wake {
            w();
        }
    });
}
