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
    Hello { session: String, name: String, kind: AttachmentKind },
    NewSession { name: String },
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
    /// B3: a file, or else a search.
    Plumb { ctx: ExecCtx, text: String },
    /// acme's ^F: complete the path fragment `prefix` typed at `at`.
    Complete { view: ViewId, ctx: ExecCtx, at: usize, prefix: String },
    /// A tool asks the leader to do something; `id` comes back in `Applied`.
    Propose { id: u64, proposal: Proposal },
    /// The leader's answer to a `Propose` it was handed (id 0: nobody waits).
    Applied { id: u64, result: Result<Option<WindowId>, String> },
    /// Latency probe.
    Ping { t: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
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
