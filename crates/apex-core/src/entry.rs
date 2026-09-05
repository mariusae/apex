//! Log entries: what a shard's log is made of. Every entry is concrete and
//! `apply` is deterministic given the state it applies to.

use serde::{Deserialize, Serialize};

use crate::ids::*;

/// One log entry, as stored and replicated.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub seq: Seq,
    /// Who sequenced it.
    pub attachment: AttachmentId,
    /// Under which fence epoch.
    pub epoch: Epoch,
    pub op: Op,
}

/// An operation on some shard.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Op {
    Buffer(BufferOp),
    Window(WindowOp),
    Layout(LayoutOp),
    Term(TermOp),
    Meta(MetaOp),
}

impl Op {
    /// Does this op belong on that shard?
    pub fn fits(&self, shard: Shard) -> bool {
        matches!(
            (self, shard),
            (Op::Buffer(_), Shard::Buffer(_))
                | (Op::Window(_), Shard::Window(_))
                | (Op::Layout(_), Shard::Layout)
                | (Op::Term(_), Shard::Term(_))
                | (Op::Meta(_), Shard::Meta)
        )
    }
}

// ---- buffer ---------------------------------------------------------------

/// The body of a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Body {
    Text(BufferId),
    Term(TermId),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum BufferOp {
    /// First entry of a buffer's log.
    Create { name: String, text: String, disk_hash: Option<String> },
    /// Replace `nd` runes at `q0` (of the text at `version`) with `text`.
    /// Consecutive edits with the same `group` undo together.
    Edit { version: Version, q0: usize, nd: usize, text: String, group: GroupId },
    /// Undo the most recent group (its inverse is derived from history).
    Undo { version: Version },
    /// Redo the most recently undone group.
    Redo { version: Version },
    /// The file on disk equals the text at `version`.
    Clean { version: Version, disk_hash: Option<String> },
    /// The file on disk changed underneath a dirty buffer.
    Stale { disk_hash: String },
    Rename { name: String },
    /// A window started viewing this buffer.
    ViewAdd { view: ViewId },
    ViewDel { view: ViewId },
    /// Selection of a view; sequenced with edits so replicas agree.
    Select { view: ViewId, q0: usize, q1: usize },
    /// Scroll origin of a view.
    Origin { view: ViewId, origin: usize },
}

// ---- window ---------------------------------------------------------------

/// Who performs an exec.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Handler {
    /// The leader of the window's shard ran it locally (built-ins).
    Leader,
    /// The server: files, processes, terminals, the plumber.
    Server,
    /// A registered tool.
    Tool(String),
}

/// What an exec acted on, so readers never guess at cross-shard order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExecAt {
    pub buffer: Option<BufferId>,
    pub version: Version,
    pub q0: usize,
    pub q1: usize,
}

/// B2 (or the CLI) executed `text`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ExecOp {
    pub text: String,
    pub handler: Handler,
    pub at: ExecAt,
}

/// The outcome of an exec, reported by its performer.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ExecStatusOp {
    Done,
    Failed(String),
    /// The performer died between seeing the exec and reporting.
    Unknown,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum WindowOp {
    /// First entry of a window's log.
    Create { tag: BufferId, body: Body },
    /// Toggle the alternate (monospace) font.
    Font { mono: bool },
    /// B2 (or the CLI) executed something in this window.
    Exec(ExecOp),
    Status { exec: Seq, status: ExecStatusOp },
    /// The window was closed.
    Delete,
}

// ---- layout ---------------------------------------------------------------

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum LayoutOp {
    /// First entry: the top row's tag buffer.
    Init { top: BufferId },
    ColNew { id: ColumnId, tag: BufferId, at: usize, weight: u32 },
    ColDel { id: ColumnId },
    ColResize { id: ColumnId, weight: u32 },
    /// Put a window into a column at an index (moving it if placed already).
    WinPlace { window: WindowId, col: ColumnId, at: usize, weight: u32 },
    WinRemove { window: WindowId },
    WinResize { window: WindowId, weight: u32 },
    /// The snarf buffer (acme's is global; the client mirrors the system clipboard).
    Snarf { text: String },
    /// Executed from a column tag or the top row.
    Exec { ctx: ExecCtx, op: ExecOp },
    Status { exec: Seq, status: ExecStatusOp },
}

// ---- term -----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Cell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
    pub flags: u8,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum TermOp {
    Create { cols: u16, rows: u16 },
    /// Replace viewport rows starting at `first`.
    Rows { first: u16, rows: Vec<Vec<Cell>> },
    Cursor { col: u16, row: u16, visible: bool },
    Resize { cols: u16, rows: u16 },
    Exit { status: i32 },
}

// ---- meta -----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AttachmentKind {
    Ui,
    Tool,
}

/// A plumbing rule. The predicate language is the plumber's business; the
/// metalog only stores and orders rules.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PlumbRule {
    pub predicate: String,
    pub action: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum MetaOp {
    /// First entry of a session's metalog.
    Init,
    ShardNew { shard: Shard },
    ShardDel { shard: Shard },
    Attach { attachment: AttachmentId, kind: AttachmentKind, name: String },
    Detach { attachment: AttachmentId },
    LeaseRequest { shard: Shard, to: AttachmentId },
    /// The holder flushed up to `seq` and released.
    LeaseRelease { shard: Shard, from: AttachmentId, seq: Seq },
    LeaseGrant { shard: Shard, to: AttachmentId, epoch: Epoch, seq: Seq },
    /// The holder did not answer; the lease was taken at `seq`.
    LeaseReclaim { shard: Shard, from: AttachmentId, epoch: Epoch, seq: Seq },
    PlumbRuleInstall { id: RuleId, attachment: AttachmentId, priority: i32, rule: PlumbRule },
    PlumbRuleRemove { id: RuleId },
}
