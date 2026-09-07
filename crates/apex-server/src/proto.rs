//! The attach protocol: postcard-encoded messages in length-prefixed frames
//! over a Unix socket (or any byte stream, e.g. ssh's stdio).

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use apex_core::*;

use crate::proposal::Proposal;
use crate::term::TermKey;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Attach to a session as a new attachment. A UI attachment takes the
    /// leases; a tool follows and proposes.
    Hello { session: String, name: String, kind: AttachmentKind, attach: Option<Script> },
    /// Make a session (fine if it exists), with what its creator brings
    /// for its init.
    NewSession { name: String, profile: Option<Script> },
    ListSessions,
    /// Rename a session; attachments to it stay attached.
    RenameSession { from: String, to: String },
    /// Entries this client sequenced as leader.
    Append { shard: Shard, entries: Vec<Entry> },
    /// The client allocated a shard; the server records it and grants the
    /// lease before processing the appends that follow.
    CreateShard { shard: Shard },
    DeleteShard { shard: Shard },
    TermKey { term: TermId, key: TermKey },
    TermPaste { term: TermId, text: String },
    TermResize { term: TermId, cols: u16, rows: u16 },
    TermScroll { term: TermId, delta: i64 },
    /// Snarf the text between two `(column, history line)` positions of a
    /// terminal (the end exclusive); the answer is a `Snarf` proposal.
    TermText { term: TermId, p0: (u16, u64), p1: (u16, u64) },
    /// Open a file (relative to the window's directory) in a column.
    OpenFile { col: ColumnId, ctx: ExecCtx, name: String },
    /// B3, or `apex plumb`: the rule table decides. `dir` stands in for
    /// the context's directory (a terminal's cwd); `edit_only` is plan 9's
    /// `B` (only rules that open in the session, else the text as a path);
    /// `dry` only reports what would happen (`PlumbTrace`).
    /// `at` is where the pointer (or dot) was, `sel` the text as
    /// expanded or swept, when the plumb came from a buffer.
    Plumb { ctx: ExecCtx, text: String, dir: Option<String>, edit_only: bool, dry: bool, at: Option<Span>, sel: Option<Span> },
    /// A tool's answer to a `Plumb` it was handed: did it take it?
    PlumbAck { id: u64, ok: bool },
    /// Install a plumbing rule: owned by this attachment when `mine`
    /// (gone when it detaches), else by the session. Answered by
    /// `RuleAdded`.
    RuleAdd { rule: PlumbRule, priority: i32, mine: bool },
    RuleRm { id: RuleId },
    /// acme's ^F: complete the path fragment `prefix` typed at `at`.
    Complete { view: ViewId, ctx: ExecCtx, at: usize, prefix: String },
    /// A tool asks the leader to do something; `id` comes back in `Applied`.
    Propose { id: u64, proposal: Proposal },
    /// The leader's answer to a `Propose` it was handed (id 0: nobody waits).
    Applied { id: u64, result: Result<Option<WindowId>, String> },
    /// Latency probe.
    Ping { t: u64 },
    /// Set variables in the session's environment (what terminals and
    /// commands get); the answer is the whole environment.
    Env { set: Vec<(String, String)> },
    /// The daemon exits, its sessions with it (to run a newer build).
    Stop,
    /// A setting: the session's, or `attachment`'s (an attach script
    /// names the attaching client through `$apexattachment`).
    Set { key: String, value: String, attachment: Option<AttachmentId> },
    /// The bytes of a file on the host, for a client that shows or
    /// previews it: answered by `File`.
    ReadFile { path: String },
    /// `ReadFile`, and again with every change to the file until
    /// `Unwatch`, this connection goes, or (a UI) it is fenced.
    Watch { path: String },
    Unwatch { path: String },
}

/// A client's script, run on the host: its `~/.apex/profile` when it
/// makes a session (after the host's own, unless it is the same file),
/// its `~/.apex/attach` whenever it attaches; and its name, for
/// `$apexclient`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub client: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// The daemon's build id, its first frame on every connection. This
    /// variant stays first, and as it is, so that any client can read it
    /// whatever else changed.
    Build { id: String },
    /// The attachment id and a snapshot of the whole session state, with
    /// this attachment already holding its leases.
    Welcome { attachment: AttachmentId, snapshot: Vec<u8> },
    /// Entries of a shard this client follows (or metalog entries).
    Entries { shard: Shard, entries: Vec<Entry> },
    /// The leader is asked to apply this; answer with `Applied{id}` unless
    /// `id` is 0.
    Propose { id: u64, proposal: Proposal },
    /// The outcome of a tool's `Propose`.
    Applied { id: u64, result: Result<Option<WindowId>, String> },
    Sessions { names: Vec<String> },
    /// The server stored the client's entries of `shard` up to `seq`.
    Ack { shard: Shard, seq: Seq },
    /// A client-created shard is recorded and leased.
    ShardReady { shard: Shard },
    Error { text: String },
    Pong { t: u64 },
    /// The session's environment, after an `Env`.
    Env { vars: Vec<(String, String)> },
    /// What a dry-run plumb would do, rule by rule.
    PlumbTrace { lines: Vec<String> },
    /// A rule this tool installed names it: does it take this plumb?
    /// Answer with `PlumbAck{id}` within a second.
    Plumb { id: u64, ctx: ExecCtx, verb: String, text: String, dir: String, groups: Vec<String>, at: Option<Span>, sel: Option<Span> },
    RuleAdded { id: RuleId },
    File { path: String, bytes: Result<Vec<u8>, String> },
}

/// Write one frame: u32 little-endian length, then postcard bytes.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = postcard::to_stdvec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = bytes.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    Ok(())
}

/// Read one frame; `Ok(None)` at a clean end of stream.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > 256 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    postcard::from_bytes(&buf).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
