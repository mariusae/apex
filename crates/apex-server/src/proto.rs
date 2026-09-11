//! The attach protocol: postcard-encoded messages in length-prefixed frames
//! over a Unix socket (or any byte stream, e.g. ssh's stdio).
//!
//! `PROTOCOL` is the version of everything on the wire: the messages
//! here, the proposals (`proposal.rs`), the entries, ops and state they
//! carry (apex-core's `entry.rs`, `state.rs`, `ids.rs`), `TermKey`,
//! `Running`. Postcard is not self-describing, so any change to any of
//! those is a new protocol: **bump `PROTOCOL` with the change.** A daemon
//! says its version first on every connection (`ServerMsg::Build`), and
//! a client of another version stops there with advice.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use apex_core::*;

use crate::proposal::Proposal;
use crate::term::TermKey;

/// The wire's version. Bump it whenever anything on the wire changes
/// (see the module doc); nothing else tells a daemon and a client apart.
pub const PROTOCOL: u32 = 18;

/// A client's terminal colours, RGB: the ink, the paper, and the
/// sixteen ANSI colours its theme draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermColors {
    pub fg: u32,
    pub bg: u32,
    pub ansi: [u32; 16],
}

impl TermColors {
    /// acme's paper and black ink with xterm's sixteen: what a session
    /// with no UI attached answers.
    pub const LIGHT: TermColors = TermColors {
        fg: 0x000000,
        bg: 0xFFFFEA,
        ansi: [0x000000, 0xCC241D, 0x3C8A2A, 0xB08A00, 0x1C4FD6, 0x9A2D9A, 0x0F8A8A, 0xBBBBBB, 0x555555, 0xFF5555, 0x55C055, 0xD6C000, 0x5580FF, 0xDD55DD, 0x33C0C0, 0xFFFFFF],
    };

    /// The colour a program asks for by index: 0..15 the palette, 256
    /// the ink, 257 the paper; anything else xterm's cube and greys.
    pub fn color(&self, index: usize) -> crate::term::Rgb8 {
        let hex = match index {
            0..=15 => self.ansi[index],
            256 => self.fg,
            257 => self.bg,
            _ => return crate::term::default_color(index),
        };
        crate::term::Rgb8 { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Attach to a session as a new attachment. A UI attachment takes the
    /// leases; a tool follows and proposes.
    /// `session`: its id, a unique prefix of it, or its label.
    Hello { session: String, name: String, kind: AttachmentKind, attach: Option<Script> },
    /// Make a session (fine if it exists), with what its creator brings
    /// for its init.
    NewSession { name: String },
    ListSessions,
    /// Rename a session; attachments to it stay attached.
    RenameSession { from: String, to: String },
    /// End a session: its commands and terminals killed, everything
    /// attached told (`Ended`) and cut off, the session gone. Refused
    /// while a window is dirty unless `force`. Answered by `Sessions`.
    EndSession { name: String, force: bool },
    /// Entries this client sequenced as leader.
    Append { shard: Shard, entries: Vec<Entry> },
    /// The client allocated a shard; the server records it and grants the
    /// lease before processing the appends that follow.
    CreateShard { shard: Shard },
    DeleteShard { shard: Shard },
    TermKey { term: TermId, key: TermKey },
    TermPaste { term: TermId, text: String },
    TermResize { term: TermId, cols: u16, rows: u16 },
    /// What the client shows: sent on attach and whenever it changes
    /// (the theme), for what is not presentation alone. The terminal's
    /// colours as programs may ask for them (OSC 10, 11 and 4): a
    /// program deciding its own palette by the background must learn
    /// the paper it is drawn on.
    ClientConfig { term: TermColors },
    /// The terminal's scrollback dropped (`Clear`); the screen stays.
    TermClear { term: TermId },
    /// The wheel over a terminal, `delta` lines (positive: down), `at`
    /// the cell under the pointer when it was the wheel (the program
    /// may be reporting the mouse), none from the scrollbar.
    TermScroll { term: TermId, delta: i64, at: Option<(u16, u16)> },
    /// Snarf the text between two `(column, history line)` positions of a
    /// terminal (the end exclusive); the answer is a `Snarf` proposal.
    TermText { term: TermId, p0: (u16, u64), p1: (u16, u64) },
    /// The text of history lines `[from, to)` of a terminal, scrollback
    /// included, answered by `TermLines` (what a client reads to find the
    /// last command's output).
    TermRead { term: TermId, from: u64, to: u64 },
    /// Open a file (relative to the window's directory) in a column.
    OpenFile { col: ColumnId, ctx: ExecCtx, name: String },
    /// B3, or `apex plumb`: the rule table decides. `dir` stands in for
    /// the context's directory (a terminal's cwd); `edit_only` is plan 9's
    /// `B` (only rules that open in the session, else the text as a path);
    /// `dry` only reports what would happen (`PlumbTrace`).
    /// `at` is where the pointer (or dot) was, `sel` the text as
    /// expanded or swept, when the plumb came from a buffer.
    /// `alt` is the word within `text` (acme's isalnum expansion), tried
    /// when no rule takes `text` (the file-name expansion).
    /// B3 (`verb` None: plumb), or a rule's verb at the pointer (cmd-B3
    /// is `Def`), walked with `at` and `sel` as B3's would be.
    Plumb { ctx: ExecCtx, text: String, dir: Option<String>, edit_only: bool, dry: bool, at: Option<Span>, sel: Option<Span>, alt: Option<(String, Span)>, reverse: bool, verb: Option<String> },
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
    /// A script's environment at its end (`apex env -import` from the
    /// profile's exit hook): what it changed, against the environment
    /// the server gave it, is applied to the session's; the answer is
    /// the whole environment.
    EnvImport { vars: Vec<(String, String)> },
    /// The daemon exits, its sessions with it (to run a newer build).
    Stop,
    /// A setting: the session's, or `attachment`'s (an attach script
    /// names the attaching client through `$apexattachment`).
    Set { key: String, value: String, attachment: Option<AttachmentId> },
    /// The commands the server is running (`Ps` answers), and ending them
    /// by name or pid (acme's Kill; `Ps` answers with what is left).
    Ps,
    Kill { targets: Vec<String> },
    /// A running program says what it is called: the command of process
    /// group `group` (the shell's pid) takes `name` in the top row, `ps`
    /// and `Kill`; a program of no known group (started from the profile,
    /// say) is adopted under `pid` for as long as this connection lasts.
    Named { name: String, group: u32, pid: u32, cmd: String },
    /// The I/O plane (WEB.md §1): a stream this connection opened with
    /// `IoFrame::Request`, then its body frames and end. Streams belong
    /// to the connection and end with it.
    Io { stream: u32, frame: IoFrame },
}

/// A frame on the I/O plane. HTTP-shaped: a `Request` opens a stream
/// (the client picks the id), `Response` answers it, `Body` carries
/// bytes either way, `End` finishes a side, `Reset` aborts. What the
/// server answers: `GET file:///path` (the bytes; with a `Watch` header
/// the stream stays open and every change brings a `Body` holding a
/// `FileFrame`), `PUT file:///path` (the body written when it ends).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum IoFrame {
    Request { method: String, url: String, headers: Vec<(String, String)> },
    Response { status: u16, headers: Vec<(String, String)> },
    Body(Vec<u8>),
    End,
    Reset { reason: String },
}

/// What each `Body` of a watched file carries: the file's contents as
/// of `version` (1 for the first, then one per change).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FileFrame {
    pub version: u64,
    pub path: String,
    pub bytes: Vec<u8>,
}

impl FileFrame {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Option<FileFrame> {
        postcard::from_bytes(bytes).ok()
    }
}

/// The path a `file://` URL names on this host, percent-decoded;
/// `file:///p`, `file://localhost/p` and a bare absolute path all do.
pub fn file_url_path(url: &str) -> Option<std::path::PathBuf> {
    let rest = if let Some(r) = url.strip_prefix("file://") {
        r.strip_prefix("localhost").unwrap_or(r)
    } else if url.starts_with('/') {
        url
    } else {
        return None;
    };
    if !rest.starts_with('/') {
        return None;
    }
    let mut out = Vec::with_capacity(rest.len());
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    Some(std::path::PathBuf::from(String::from_utf8_lossy(&out).to_string()))
}

/// A client's script, run on the host: its `~/.apex/attach` whenever it
/// attaches; and its name, for `$apexclient`.
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
    /// The first frame on every connection, frozen in this shape: the
    /// daemon's `PROTOCOL` and, to say which binary it is, its build id.
    Build { protocol: u32, id: String },
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
    Sessions { sessions: Vec<SessionInfo> },
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
    /// A plumb this connection asked for is over: a rule took it, or
    /// none did (`why` says; what was done instead, a Look, is done).
    Plumbed { ok: bool, why: String },
    /// A rule this tool installed names it: does it take this plumb?
    /// Answer with `PlumbAck{id}` within a second.
    Plumb { id: u64, rule: RuleId, ctx: ExecCtx, verb: String, text: String, dir: String, groups: Vec<String>, at: Option<Span>, sel: Option<Span> },
    RuleAdded { id: RuleId },
    Ps { procs: Vec<crate::Running> },
    TermLines { term: TermId, text: String },
    /// A program in a terminal set the clipboard (OSC 52): the snarf
    /// buffer has the text already; a UI puts it on its own clipboard.
    Clipboard { text: String },
    /// The I/O plane: a frame on a stream this connection opened.
    Io { stream: u32, frame: IoFrame },
    /// The session this connection was attached to has been ended; the
    /// connection closes right after.
    Ended { id: String, label: String },
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

#[cfg(test)]
mod io_tests {
    use super::*;

    #[test]
    fn file_urls_name_paths() {
        assert_eq!(file_url_path("file:///a/b c.txt").unwrap(), std::path::PathBuf::from("/a/b c.txt"));
        assert_eq!(file_url_path("file:///a/b%20c.txt").unwrap(), std::path::PathBuf::from("/a/b c.txt"));
        assert_eq!(file_url_path("file://localhost/a").unwrap(), std::path::PathBuf::from("/a"));
        assert_eq!(file_url_path("/plain").unwrap(), std::path::PathBuf::from("/plain"));
        assert!(file_url_path("http://x/").is_none());
        assert!(file_url_path("file://host/a").is_none());
        assert!(file_url_path("relative").is_none());
        let f = FileFrame { version: 3, path: "/p".into(), bytes: b"hi".to_vec() };
        assert_eq!(FileFrame::decode(&f.encode()).unwrap(), f);
    }
}

/// A session as the daemon lists it: its identity, and its label for
/// people. Ids are what sessions are known by; labels can change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub label: String,
}
