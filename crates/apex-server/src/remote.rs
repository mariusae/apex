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

use crate::proto::{FileFrame, IoFrame, read_frame, write_frame, ClientMsg, ServerMsg, Script, SessionInfo};
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
    pub kind: AttachmentKind,
    out: Outbound,
    /// Messages from the server, delivered by the reader thread.
    pub rx: Receiver<ServerMsg>,
    /// How far each led shard has been shipped.
    sent: HashMap<Shard, Seq>,
    /// Latest `Ack` per shard.
    pub acked: HashMap<Shard, Seq>,
    /// Windows made by proposals since the last `take_made`.
    made: Vec<WindowId>,
    /// Text a program put into a buffer through a proposal since the last
    /// `take_outputs`: `(buffer, at, end)`, for acme's scrolling rule.
    outputs: Vec<(BufferId, usize, usize)>,
    /// Answers to this tool's proposals, by its ids.
    pub applied: HashMap<u64, Result<Option<WindowId>, String>>,
    /// The last session listing received.
    pub sessions: Option<Vec<SessionInfo>>,
    /// The session environment, after an `Env`.
    pub env: Option<Vec<(String, String)>>,
    /// A dry-run plumb's report, after a `Plumb{dry}`.
    pub trace: Option<Vec<String>>,
    /// Plumbs handed to this tool by rules naming it, to answer with
    /// `PlumbAck`.
    pub plumbs: Vec<ToolPlumb>,
    /// The id of the rule last added.
    pub rule_added: Option<RuleId>,
    /// What rules asked this client to do (`ClientDo`): (id, verb, args),
    /// for the owner to carry out and answer with `Applied{id}`. Only a
    /// UI is asked; anything else refuses at once.
    pub client_asks: Vec<(u64, String, String)>,
    /// Frames that arrived on the I/O plane, by stream, in order (those
    /// no thread waits for through `io_plane`).
    pub io: Vec<(u32, IoFrame)>,
    /// Stream ids (odd; the server opens none), shared with threads.
    ids: crate::plane::IoIds,
    /// Streams whose frames go to a thread rather than `io`.
    sinks: crate::plane::IoSinks,
    /// The running commands, after a `Ps` or `Kill`.
    pub ps: Option<Vec<crate::Running>>,
    /// Terminal text read with `TermRead`.
    pub term_lines: Vec<(TermId, String)>,
    /// When the last `Pong` arrived (the owner's heartbeat).
    pub last_pong: Option<std::time::Instant>,
    /// The session was ended under us (`Ended`): the link closes next.
    pub ended: Option<String>,
    /// Where the last edit by another attachment ended, per buffer: in a
    /// win's window, the output point, where the prompt is.
    pub foreign_end: HashMap<BufferId, usize>,
    /// The last entry flushed on each shard and when, until its `Ack`.
    pending_ack: HashMap<Shard, (Seq, std::time::Instant)>,
    /// How long the last `Ack` took to come back, in milliseconds.
    pub ack_ms: Option<u64>,
    next_id: u64,
    /// Closes the transport on drop, so the reader thread ends and the
    /// server sees the attachment go.
    closer: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}

impl Link {
    /// End the transport now: the reader thread ends, and a bridge behind
    /// it is ended with its process group.
    pub fn close(&mut self) {
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
    /// `over`, making the session when the daemon has none named
    /// `session` (its id, or its label): then one labelled `label` is
    /// made and attached to instead.
    pub fn over_streams_creating(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        closer: Option<Box<dyn FnOnce() + Send>>,
        session: &str,
        label: &str,
        name: &str,
        kind: AttachmentKind,
        wake: Option<Wake>,
    ) -> io::Result<(Link, Log, Node)> {
        let attach = if kind == AttachmentKind::Ui { local_attach() } else { None };
        Self::over_streams_inner(reader, writer, closer, session, name, kind, wake, Some(label.to_string()), attach)
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
        let attach = if kind == AttachmentKind::Ui { local_attach() } else { None };
        Self::over_streams_inner(reader, writer, closer, session, name, kind, wake, None, attach)
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
        create: Option<String>,
        attach: Option<Script>,
    ) -> io::Result<(Link, Log, Node)> {
        let out = Outbound(Arc::new(Mutex::new(BufWriter::new(writer))));
        let (tx, rx) = channel::<ServerMsg>();
        let sinks = crate::plane::IoSinks::new();
        spawn_reader(reader, tx, wake, sinks.clone());
        // a session named by its label alone is made if it is not there;
        // one named by its id is attached to as it is, and only when the
        // daemon has none (it was ended, or the daemon is new) is one of
        // the label made instead
        let mut create = create;
        if let Some(label) = create.as_deref() {
            if label == session {
                out.send(&ClientMsg::NewSession { name: label.to_string() })?;
                create = None;
            }
        }
        out.send(&ClientMsg::Hello { session: session.to_string(), name: name.to_string(), kind, attach: attach.clone() })?;
        // a daemon that never answers must not hold a client forever
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let (attachment, snapshot) = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let m = match rx.recv_timeout(left) {
                Ok(m) => m,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(io::Error::new(io::ErrorKind::TimedOut, "no welcome from the daemon within a minute")),
                Err(_) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "closed before welcome")),
            };
            match m {
                ServerMsg::Build { protocol, id } => check_build(protocol, &id)?,
                ServerMsg::Welcome { attachment, snapshot } => break (attachment, snapshot),
                ServerMsg::Error { text } if text.starts_with("no session") && create.is_some() => {
                    let label = create.take().unwrap();
                    out.send(&ClientMsg::NewSession { name: label.clone() })?;
                    out.send(&ClientMsg::Hello { session: label, name: name.to_string(), kind, attach: attach.clone() })?;
                }
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
        Ok((Link { attachment, kind, out, rx, sent, acked: HashMap::new(), made: Vec::new(), outputs: Vec::new(), applied: HashMap::new(), sessions: None, env: None, trace: None, plumbs: Vec::new(), rule_added: None, client_asks: Vec::new(), io: Vec::new(), ids: crate::plane::IoIds::new(), sinks, ps: None, term_lines: Vec::new(), last_pong: None, ended: None, foreign_end: HashMap::new(), pending_ack: HashMap::new(), ack_ms: None, next_id: 1, closer }, log, node))
    }

    pub fn send(&self, m: &ClientMsg) {
        let _ = self.out.send(m);
    }

    /// The outbound side, for a thread of the owner's that sends too.
    pub fn outbound(&self) -> Outbound {
        self.out.clone()
    }

    /// Open a stream on the I/O plane with a request; its id, for the
    /// frames that come back in `io`.
    pub fn io_open(&mut self, method: &str, url: &str, headers: &[(&str, &str)]) -> u32 {
        let stream = self.ids.next();
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        self.send(&ClientMsg::Io { stream, frame: IoFrame::Request { method: method.to_string(), url: url.to_string(), headers } });
        stream
    }

    /// The plane for threads: streams they open get their frames from the
    /// reader directly (WEB.md §2.3: the web view's proxy, `apexfile://`).
    pub fn io_plane(&self) -> crate::plane::IoPlane {
        crate::plane::IoPlane::new(self.out.clone(), self.ids.clone(), self.sinks.clone())
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
                let last = entries.last().unwrap().seq;
                self.sent.insert(shard, last);
                // timed until acked, unless an earlier one still waits
                self.pending_ack.entry(shard).or_insert((last, std::time::Instant::now()));
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
            ServerMsg::Build { .. } => {} // checked before the welcome
            ServerMsg::Entries { shard, entries } => {
                if let Shard::Buffer(b) = shard {
                    for e in &entries {
                        if e.attachment != self.attachment {
                            if let Op::Buffer(BufferOp::Edit { q0, text, .. }) = &e.op {
                                self.foreign_end.insert(b, q0 + text.chars().count());
                            }
                        }
                    }
                }
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
                self.compact_mirror(node, log, shard);
                let mark = self.sent.entry(shard).or_insert(0);
                if *mark < last && !leads {
                    *mark = last;
                }
            }
            ServerMsg::Propose { id, proposal } => {
                // a program's output into a text window (win's Insert),
                // applied here as the lead: where it ended is the prompt
                match &proposal {
                    Proposal::Insert { buffer, at, text, .. } => {
                        let end = at + text.chars().count();
                        self.foreign_end.insert(*buffer, end);
                        self.outputs.push((*buffer, *at, end));
                    }
                    Proposal::ReplaceRange { buffer, q0, text, .. } if !text.is_empty() => {
                        let end = q0 + text.chars().count();
                        self.foreign_end.insert(*buffer, end);
                        self.outputs.push((*buffer, *q0, end));
                    }
                    _ => {}
                }
                let result = match proposal {
                    // a rule asks this client for something only it can
                    // do: the owner answers when it has done it
                    Proposal::ClientDo { verb, args } if self.kind == AttachmentKind::Ui => {
                        self.client_asks.push((id, verb, args));
                        return true;
                    }
                    Proposal::ClientDo { verb, .. } => Err(format!("this client cannot {verb}")),
                    p => proposal::apply(node, log, p).map_err(|e| e.to_string()),
                };
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
            ServerMsg::Sessions { sessions } => {
                self.sessions = Some(sessions);
            }
            ServerMsg::Env { vars } => {
                self.env = Some(vars);
            }
            ServerMsg::PlumbTrace { lines } => self.trace = Some(lines),
            ServerMsg::Plumb { id, rule, ctx, verb, text, dir, groups, at, sel } => self.plumbs.push(ToolPlumb { id, rule, ctx, verb, text, dir, groups, at, sel }),
            ServerMsg::RuleAdded { id } => self.rule_added = Some(id),
            ServerMsg::Io { stream, frame } => self.io.push((stream, frame)),
            ServerMsg::Ps { procs } => self.ps = Some(procs),
            ServerMsg::TermLines { term, text } => self.term_lines.push((term, text)),
            ServerMsg::Ack { shard, seq } => {
                self.acked.insert(shard, seq);
                self.compact_mirror(node, log, shard);
                if let Some((want, at)) = self.pending_ack.get(&shard).copied() {
                    if seq >= want {
                        self.ack_ms = Some(at.elapsed().as_millis() as u64);
                        self.pending_ack.remove(&shard);
                    }
                }
            }
            ServerMsg::ShardReady { .. } => {}
            ServerMsg::Welcome { .. } => {}
            ServerMsg::Error { text } => eprintln!("remote: server: {text}"),
            ServerMsg::Pong { .. } => {
                self.last_pong = Some(std::time::Instant::now());
            }
            ServerMsg::Ended { label, .. } => self.ended = Some(label),
        }
        true
    }

    /// Forget the mirror's entries that are behind us: applied here, and
    /// (for a shard we lead) shipped and acked by the server. A mirror
    /// holds what is outstanding, not the session's whole history.
    fn compact_mirror(&mut self, node: &Node, log: &mut Log, shard: Shard) {
        let mut low = node.state.applied(shard);
        if self.leads(log, shard) {
            low = low.min(self.sent.get(&shard).copied().unwrap_or(0)).min(self.acked.get(&shard).copied().unwrap_or(0));
        }
        log.compact(shard, low);
    }

    fn leads(&self, log: &Log, shard: Shard) -> bool {
        log.lease(shard).is_some_and(|l| l.holder == self.attachment && l.released.is_none())
    }

    /// Windows opened or looked in by proposals since the last call.
    pub fn take_made(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.made)
    }

    /// What programs wrote into buffers since the last call.
    pub fn take_outputs(&mut self) -> Vec<(BufferId, usize, usize)> {
        std::mem::take(&mut self.outputs)
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
pub fn list_sessions(path: &Path) -> io::Result<Vec<SessionInfo>> {
    let mut s = UnixStream::connect(path)?;
    write_frame(&mut s, &ClientMsg::ListSessions)?;
    let mut r = BufReader::new(s);
    loop {
        match read_frame::<_, ServerMsg>(&mut r)? {
            Some(ServerMsg::Sessions { sessions }) => return Ok(sessions),
            Some(ServerMsg::Error { text }) => return Err(io::Error::other(text)),
            Some(ServerMsg::Build { protocol, id }) => check_build(protocol, &id)?,
            Some(_) => {}
            None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no answer")),
        }
    }
}

/// The bridge command (`ssh host apex attach --stdio`, a provider's
/// equivalent) as a child with piped stdin and stdout, in a process group
/// of its own, and the closer that ends that whole group: the shell, the
/// provider, whatever it ran. A bridge left behind keeps its far end
/// open, and some providers only tear down cleanly when it goes.
pub fn bridge_child(cmd: &str) -> io::Result<(std::process::ChildStdin, std::process::ChildStdout, Box<dyn FnOnce() + Send>)> {
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .process_group(0)
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let pid = child.id() as libc::pid_t;
    let closer = Box::new(move || {
        // SAFETY: a signal to the group we made for this child.
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
        let _ = child.kill();
        let _ = child.wait();
    });
    Ok((stdin, stdout, closer))
}

/// A plumb a rule handed to this tool; answer with `Remote::plumb_ack`.
#[derive(Clone, Debug)]
pub struct ToolPlumb {
    pub id: u64,
    /// The rule of ours that matched.
    pub rule: RuleId,
    pub ctx: ExecCtx,
    pub verb: String,
    pub text: String,
    pub dir: String,
    pub groups: Vec<String>,
    /// Where the pointer or dot was, and what was expanded or swept.
    pub at: Option<Span>,
    pub sel: Option<Span>,
}

/// A daemon speaking another protocol version is not ours to talk to:
/// the error (kind `Unsupported`) says what to do about it. Another
/// build with the same protocol is fine.
pub fn check_build(protocol: u32, id: &str) -> io::Result<()> {
    if protocol == crate::proto::PROTOCOL {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "the daemon speaks apex protocol {protocol} (build {id}), this is protocol {} (build {}); when its sessions can be let go, stop it (`apex stop` on its machine) and attach again",
            crate::proto::PROTOCOL,
            crate::BUILD_ID
        ),
    ))
}

/// Stop the daemon at `path`: it exits, its sessions with it.
pub fn stop(path: &Path) -> io::Result<()> {
    let mut s = UnixStream::connect(path)?;
    write_frame(&mut s, &ClientMsg::Stop)?;
    // it goes on its way out; give it a moment to take the message
    let _ = s.shutdown(std::net::Shutdown::Write);
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let mut r = BufReader::new(s);
    while let Ok(Some(_)) = read_frame::<_, ServerMsg>(&mut r) {}
    Ok(())
}

/// This machine's `~/.apex/NAME` and its name, as a script for a host.
pub fn local_script(name: &str) -> Option<Script> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    let text = std::fs::read_to_string(Path::new(&home).join(".apex").join(name)).ok()?;
    Some(Script { client: crate::term::sysname(), text })
}

/// `~/.apex/attach`: run on the host every time this machine attaches.
pub fn local_attach() -> Option<Script> {
    local_script("attach")
}

/// Create a session on a daemon; fine if it already exists.
pub fn new_session(path: &Path, name: &str) -> io::Result<()> {
    let mut s = UnixStream::connect(path)?;
    write_frame(&mut s, &ClientMsg::NewSession { name: name.to_string() })?;
    let mut r = BufReader::new(s);
    loop {
        match read_frame::<_, ServerMsg>(&mut r)? {
            Some(ServerMsg::Sessions { .. }) => return Ok(()),
            Some(ServerMsg::Error { text }) if text.contains("exists") => return Ok(()),
            Some(ServerMsg::Error { text }) => return Err(io::Error::other(text)),
            Some(ServerMsg::Build { protocol, id }) => check_build(protocol, &id)?,
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
            Some(ServerMsg::Build { protocol, id }) => check_build(protocol, &id)?,
            Some(_) => {}
            None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no answer")),
        }
    }
}

/// End the session `name` on the daemon at `path`: refused while a
/// window there is dirty unless `force`.
pub fn end_session(path: &Path, name: &str, force: bool) -> io::Result<()> {
    let mut s = UnixStream::connect(path)?;
    write_frame(&mut s, &ClientMsg::EndSession { name: name.to_string(), force })?;
    let mut r = BufReader::new(s);
    loop {
        match read_frame::<_, ServerMsg>(&mut r)? {
            Some(ServerMsg::Sessions { .. }) => return Ok(()),
            Some(ServerMsg::Error { text }) => return Err(io::Error::other(text)),
            Some(ServerMsg::Build { protocol, id }) => check_build(protocol, &id)?,
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
        let (stdin, stdout, closer) = bridge_child(cmd)?;
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

    /// Block until `ready` says the link has what we wait for.
    fn wait_for<T>(&mut self, timeout: std::time::Duration, mut ready: impl FnMut(&mut Link) -> Option<T>) -> Result<T, String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(v) = ready(&mut self.link) {
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

    /// Install a plumbing rule and block for its id.
    pub fn rule_add(&mut self, rule: PlumbRule, priority: i32, mine: bool, timeout: std::time::Duration) -> Result<RuleId, String> {
        self.link.rule_added = None;
        self.send(&ClientMsg::RuleAdd { rule, priority, mine });
        self.wait_for(timeout, |l| l.rule_added.take())
    }

    /// What a plumb would do, rule by rule.
    pub fn plumb_dry(&mut self, ctx: ExecCtx, text: &str, dir: Option<String>, edit_only: bool, timeout: std::time::Duration) -> Result<Vec<String>, String> {
        self.link.trace = None;
        self.send(&ClientMsg::Plumb { ctx, text: text.to_string(), dir, edit_only, dry: true, at: None, sel: None, alt: None, reverse: false, verb: None });
        self.wait_for(timeout, |l| l.trace.take())
    }

    /// The bytes of a file on the host: `GET file://path` on the I/O plane.
    pub fn read_file(&mut self, path: &str, timeout: std::time::Duration) -> Result<Vec<u8>, String> {
        let stream = self.io_open("GET", &file_url(path), &[]);
        let (status, body) = self.io_collect(stream, timeout)?;
        io_result(status, body)
    }

    /// Open a stream on the I/O plane with a request; its id, for the
    /// frames that come back in `link.io`.
    pub fn io_open(&mut self, method: &str, url: &str, headers: &[(&str, &str)]) -> u32 {
        self.link.io_open(method, url, headers)
    }

    /// The plane for threads (see `Link::io_plane`).
    pub fn io_plane(&self) -> crate::plane::IoPlane {
        self.link.io_plane()
    }

    /// Send body bytes, then the end, on a stream we opened (a PUT).
    pub fn io_send(&self, stream: u32, body: &[u8]) {
        for chunk in body.chunks(256 * 1024) {
            self.send(&ClientMsg::Io { stream, frame: IoFrame::Body(chunk.to_vec()) });
        }
    }

    pub fn io_end(&self, stream: u32) {
        self.send(&ClientMsg::Io { stream, frame: IoFrame::End });
    }

    /// The frames that arrived on `stream`, taken out of `link.io`.
    pub fn io_take(&mut self, stream: u32) -> Vec<IoFrame> {
        let mut out = Vec::new();
        self.link.io.retain(|(s, f)| {
            if *s == stream {
                out.push(f.clone());
                false
            } else {
                true
            }
        });
        out
    }

    /// Wait for a stream's response head: the status.
    pub fn io_response(&mut self, stream: u32, timeout: std::time::Duration) -> Result<u16, String> {
        self.wait_for(timeout, |l| {
            let i = l.io.iter().position(|(s, f)| *s == stream && matches!(f, IoFrame::Response { .. }))?;
            match l.io.remove(i).1 {
                IoFrame::Response { status, .. } => Some(status),
                _ => None,
            }
        })
    }

    /// Wait for a whole answer on `stream`: the status and the body up to
    /// its end.
    pub fn io_collect(&mut self, stream: u32, timeout: std::time::Duration) -> Result<(u16, Vec<u8>), String> {
        let status = self.io_response(stream, timeout)?;
        let mut body = Vec::new();
        self.wait_for(timeout, |l| {
            let mut done = false;
            l.io.retain(|(s, f)| {
                if *s != stream || done {
                    return true;
                }
                match f {
                    IoFrame::Body(b) => body.extend_from_slice(b),
                    IoFrame::End => done = true,
                    IoFrame::Reset { .. } => done = true,
                    _ => {}
                }
                false
            });
            done.then_some(())
        })?;
        Ok((status, body))
    }

    /// The next body on a watch stream, as a `FileFrame`, if one is in.
    pub fn io_next_file(&mut self, stream: u32) -> Option<FileFrame> {
        let i = self.link.io.iter().position(|(s, f)| *s == stream && matches!(f, IoFrame::Body(_)))?;
        match self.link.io.remove(i).1 {
            IoFrame::Body(b) => FileFrame::decode(&b),
            _ => None,
        }
    }

    /// The commands the server runs now.
    pub fn ps(&mut self, timeout: std::time::Duration) -> Result<Vec<crate::Running>, String> {
        self.link.ps = None;
        self.send(&ClientMsg::Ps);
        self.wait_for(timeout, |l| l.ps.take())
    }

    /// Say what this program is called: its entry in the top row, `ps`
    /// and `Kill` takes `name` (the command of our process group), or a
    /// new one is made for us if none was started by the server.
    pub fn announce(&self, name: &str) {
        // SAFETY: plain libc queries
        let group = unsafe { libc::getpgrp() } as u32;
        let cmd = std::env::args().collect::<Vec<_>>().join(" ");
        self.send(&ClientMsg::Named { name: name.to_string(), group, pid: std::process::id(), cmd });
    }

    /// End running commands by name or pid; what is left.
    pub fn kill(&mut self, targets: Vec<String>, timeout: std::time::Duration) -> Result<Vec<crate::Running>, String> {
        self.link.ps = None;
        self.send(&ClientMsg::Kill { targets });
        self.wait_for(timeout, |l| l.ps.take())
    }

    /// Subscribe to a file on the host: `GET file://path` with `Watch`.
    /// The stream and the bytes now; after each change `io_next_file` on
    /// the stream has the file again, until `unwatch`.
    pub fn watch(&mut self, path: &str, timeout: std::time::Duration) -> Result<(u32, Vec<u8>), String> {
        let stream = self.io_open("GET", &file_url(path), &[("Watch", "1")]);
        let status = self.io_response(stream, timeout)?;
        if status != 200 {
            let (_, body) = self.io_collect_body(stream, timeout)?;
            return Err(String::from_utf8_lossy(&body).to_string());
        }
        let first = self.wait_for(timeout, |l| {
            let i = l.io.iter().position(|(s, f)| *s == stream && matches!(f, IoFrame::Body(_)))?;
            match l.io.remove(i).1 {
                IoFrame::Body(b) => FileFrame::decode(&b).map(|f| f.bytes),
                _ => None,
            }
        })?;
        Ok((stream, first))
    }

    /// The body of a stream whose response head was already taken.
    fn io_collect_body(&mut self, stream: u32, timeout: std::time::Duration) -> Result<(u16, Vec<u8>), String> {
        let mut body = Vec::new();
        self.wait_for(timeout, |l| {
            let mut done = false;
            l.io.retain(|(s, f)| {
                if *s != stream || done {
                    return true;
                }
                match f {
                    IoFrame::Body(b) => body.extend_from_slice(b),
                    IoFrame::End | IoFrame::Reset { .. } => done = true,
                    _ => {}
                }
                false
            });
            done.then_some(())
        })?;
        Ok((0, body))
    }

    /// End a watch stream.
    pub fn unwatch(&self, stream: u32) {
        self.io_end(stream);
    }

    /// The outbound side, for a thread that sends too.
    pub fn outbound(&self) -> Outbound {
        self.link.outbound()
    }

    /// The body of a stream whose response head was already taken.
    pub fn io_collect_body_pub(&mut self, stream: u32, timeout: std::time::Duration) -> Result<(u16, Vec<u8>), String> {
        self.io_collect_body(stream, timeout)
    }

    /// Did the server end (or reset) `stream`? The frame is consumed.
    pub fn io_take_ended(&mut self, stream: u32) -> bool {
        let before = self.link.io.len();
        self.link.io.retain(|(s, f)| !(*s == stream && matches!(f, IoFrame::End | IoFrame::Reset { .. })));
        self.link.io.len() != before
    }

    /// Attach with an explicit attach script (tests; a UI sends its
    /// `~/.apex/attach` on its own).
    pub fn connect_with(path: &Path, session: &str, name: &str, kind: AttachmentKind, attach: Option<Script>) -> io::Result<Remote> {
        let s = UnixStream::connect(path)?;
        let w = s.try_clone()?;
        let closer = s.try_clone()?;
        let (link, log, node) = Link::over_streams_inner(
            Box::new(s),
            Box::new(w),
            Some(Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            })),
            session,
            name,
            kind,
            None,
            None,
            attach,
        )?;
        Ok(Remote { log, node, link })
    }

    /// Answer a plumb a rule handed to this tool.
    pub fn plumb_ack(&self, id: u64, ok: bool) {
        self.send(&ClientMsg::PlumbAck { id, ok });
    }

    /// Set session variables (none: just ask) and block for the
    /// environment that results.
    pub fn env(&mut self, set: Vec<(String, String)>, timeout: std::time::Duration) -> Result<Vec<(String, String)>, String> {
        self.env_msg(ClientMsg::Env { set }, timeout)
    }

    /// `EnvImport`: this environment's changes become the session's.
    pub fn env_import(&mut self, vars: Vec<(String, String)>, timeout: std::time::Duration) -> Result<Vec<(String, String)>, String> {
        self.env_msg(ClientMsg::EnvImport { vars }, timeout)
    }

    fn env_msg(&mut self, m: ClientMsg, timeout: std::time::Duration) -> Result<Vec<(String, String)>, String> {
        self.link.env = None;
        self.send(&m);
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

fn spawn_reader(stream: Box<dyn Read + Send>, tx: Sender<ServerMsg>, wake: Option<Wake>, sinks: crate::plane::IoSinks) {
    thread::spawn(move || {
        let mut r = BufReader::new(stream);
        loop {
            match read_frame::<_, ServerMsg>(&mut r) {
                Ok(Some(m)) => {
                    // a stream a thread waits on: straight to it
                    if let ServerMsg::Io { stream, frame } = &m {
                        if sinks.deliver(*stream, frame) {
                            continue;
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_protocol_is_refused_with_advice() {
        // the same protocol from any build is fine; another protocol is not
        assert!(check_build(crate::proto::PROTOCOL, crate::BUILD_ID).is_ok());
        assert!(check_build(crate::proto::PROTOCOL, "000000000000").is_ok());
        let e = check_build(crate::proto::PROTOCOL + 1, "000000000000").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().contains("apex stop"), "{e}");
        assert!(e.to_string().contains(&format!("protocol {}", crate::proto::PROTOCOL)), "{e}");
    }
}

/// `file://` for a path on the host, percent-encoding what a URL cannot
/// carry bare.
pub fn file_url(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b'+' | b'@' | b':' | b',' | b'=' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A GET's answer as a result: the body, or its message as the error.
pub fn io_result(status: u16, body: Vec<u8>) -> Result<Vec<u8>, String> {
    if status == 200 {
        Ok(body)
    } else {
        Err(String::from_utf8_lossy(&body).to_string())
    }
}
