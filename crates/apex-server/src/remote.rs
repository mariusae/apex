//! The client side of the attach protocol. A [`Link`] is the connection:
//! the owner drives its `Node` over a mirror `Log` exactly as in-process,
//! calls [`Link::flush`] to ship what it sequenced, and feeds messages from
//! [`Link::rx`] to [`Link::handle`] to keep up with the server. [`Remote`]
//! bundles a link with its log and node for headless clients.

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use apex_core::log::MirrorHook;
use apex_core::*;

use crate::proto::{read_frame, write_frame, ClientMsg, ServerMsg};
use crate::proposal;

/// A shared, buffered writer: the mirror hook and the owner both send.
#[derive(Clone)]
pub struct Outbound(Arc<Mutex<BufWriter<UnixStream>>>);

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
}

/// Called by the reader thread after queuing a message, so an event loop
/// that cannot block on `rx` knows to poll.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

impl Link {
    /// Connect to the daemon at `path` and attach to `session`. Blocks for
    /// the welcome; returns the link and the mirror log and node to drive.
    pub fn connect(path: &Path, session: &str, name: &str, wake: Option<Wake>) -> io::Result<(Link, Log, Node)> {
        let stream = UnixStream::connect(path)?;
        Self::over(stream, session, name, wake)
    }

    pub fn over(stream: UnixStream, session: &str, name: &str, wake: Option<Wake>) -> io::Result<(Link, Log, Node)> {
        let out = Outbound(Arc::new(Mutex::new(BufWriter::new(stream.try_clone()?))));
        let (tx, rx) = channel::<ServerMsg>();
        spawn_reader(stream, tx, wake);
        out.send(&ClientMsg::Hello { session: session.to_string(), name: name.to_string() })?;
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
        Ok((Link { attachment, out, rx, sent, acked: HashMap::new(), made: Vec::new() }, log, node))
    }

    pub fn send(&self, m: &ClientMsg) {
        let _ = self.out.send(m);
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
            ServerMsg::Propose(p) => {
                match proposal::apply(node, log, p) {
                    Ok(Some(w)) => self.made.push(w),
                    Ok(None) => {}
                    Err(e) => eprintln!("remote: proposal: {e}"),
                }
                self.flush(log);
            }
            ServerMsg::Ack { shard, seq } => {
                self.acked.insert(shard, seq);
            }
            ServerMsg::ShardReady { .. } => {}
            ServerMsg::Welcome { .. } => {}
            ServerMsg::Error { text } => eprintln!("remote: server: {text}"),
            ServerMsg::Pong { .. } => {}
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

/// A link with its own log and node: a headless client.
pub struct Remote {
    pub log: Log,
    pub node: Node,
    pub link: Link,
}

impl Remote {
    pub fn connect(path: &Path, session: &str, name: &str) -> io::Result<Remote> {
        let (link, log, node) = Link::connect(path, session, name, None)?;
        Ok(Remote { log, node, link })
    }

    pub fn attachment(&self) -> AttachmentId {
        self.link.attachment
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

fn spawn_reader(stream: UnixStream, tx: Sender<ServerMsg>, wake: Option<Wake>) {
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
