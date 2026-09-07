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
    /// A web page, rendered by the client; the URL is the window's name
    /// (its tag's first word), nothing else is session state (WEB.md §2).
    Web,
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
    /// acme's `Tab n`: the body's tab stop.
    Tab { n: u32 },
    /// acme's `Indent on|off`: copy the previous line's indentation on newline.
    Indent { on: bool },
    /// acme's `tagexpand`: whether the tag shows all its lines (Down) or one (Up).
    TagExpand { on: bool },
    /// A process is behind this window (a win tool's shell): live, a
    /// state beside clean and dirty. `by` is the attachment that keeps
    /// it so; the state ends with that attachment, or with `None`.
    Live { by: Option<AttachmentId> },
    /// B2 (or the CLI) executed something in this window.
    Exec(ExecOp),
    Status { exec: Seq, status: ExecStatusOp },
    /// The window was closed.
    Delete,
}

// ---- layout ---------------------------------------------------------------

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum LayoutOp {
    /// First entry: the top row's tag buffer and the row's rectangle.
    Init { top: BufferId, r: crate::tiling::Rect },
    /// The whole tiling after an acme layout operation (§tiling): the
    /// row's rectangle and every column with its windows, in order, with
    /// their rectangles. The leader computes it; replicas just take it.
    Arrange { r: crate::tiling::Rect, cols: Vec<crate::state::Column> },
    /// The snarf buffer (acme's is global; the client mirrors the system clipboard).
    Snarf { text: String },
    /// A jump (`Goto`): where it left from goes on the back stack, and
    /// the forward stack is cleared.
    Visit { from: Option<Loc>, to: Loc },
    /// `Back` or `Fwd`: the top of that stack goes, and where the user
    /// was goes on the other.
    NavPop { back: bool, at: Option<Loc> },
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
    /// An OSC 8 hyperlink: 1-based index into the terminal's `links`, 0
    /// for none.
    pub link: u16,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum TermOp {
    Create { cols: u16, rows: u16 },
    /// Replace viewport rows starting at `first`.
    Rows { first: u16, rows: Vec<Vec<Cell>> },
    /// The hyperlinks the viewport's cells refer to (OSC 8), by index.
    Links { links: Vec<String> },
    Cursor { col: u16, row: u16, visible: bool },
    Resize { cols: u16, rows: u16 },
    Exit { status: i32 },
    /// The viewport's first row is this line of the terminal's history
    /// (0 is the oldest line kept); the client anchors selections to it.
    View { top: u64 },
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
    /// The command this rule answers: `plumb` is B3; anything else is a
    /// word in the tag of every window the rule applies to, executed by
    /// B2 there.
    pub verb: String,
    /// The plumbed text (for `plumb`) or the arguments (a verb) must match
    /// this regexp; its groups bind `$0`..`$9`.
    pub text: Option<String>,
    /// The window's name must match this regexp.
    pub file: Option<String>,
    /// The window must be of this kind.
    pub kind: Option<WinKind>,
    /// This (expanded, relative to the window's directory) must be a file.
    pub isfile: Option<String>,
    /// This must be a directory.
    pub isdir: Option<String>,
    pub action: RuleAction,
    /// Where a `Run` command's output goes.
    pub to: Option<RunTo>,
}

/// Is this name a URL (a web window's), not a path? `scheme://...`.
pub fn is_url(name: &str) -> bool {
    match name.split_once("://") {
        Some((scheme, rest)) => !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && !rest.is_empty(),
        None => false,
    }
}

/// A place in the session: a window by name (a file's path), and where
/// in it. Session state, so that a session re-attached elsewhere has the
/// same headspace.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Loc {
    pub name: String,
    pub pos: Pos,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Pos {
    /// Leave the selection as it is (a bare path).
    Keep,
    /// Character offsets.
    Chars(usize, usize),
    /// A line, 1-based (`name:12`).
    Line(usize),
    /// A line and column as language servers count: 0-based, the column
    /// in UTF-16 units.
    LineCol(usize, usize),
}

/// What kind of window a rule applies to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum WinKind {
    File,
    Dir,
    Term,
    Errors,
    Web,
}

impl WinKind {
    pub fn parse(s: &str) -> Option<WinKind> {
        match s {
            "file" => Some(WinKind::File),
            "dir" => Some(WinKind::Dir),
            "term" => Some(WinKind::Term),
            "errors" => Some(WinKind::Errors),
            "web" => Some(WinKind::Web),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            WinKind::File => "file",
            WinKind::Dir => "dir",
            WinKind::Term => "term",
            WinKind::Errors => "errors",
            WinKind::Web => "web",
        }
    }
}

/// What a matching rule does. Templates expand `$0`..`$9`, `$file`,
/// `$dir`, `$win`, `$line`, `$sel`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RuleAction {
    /// Open this path (`name` or `name:line`) in the session: plan 9's
    /// edit port.
    Edit(String),
    /// Run this command on the host, in the window's directory, the
    /// selection on its stdin.
    Run(String),
    /// Ask the UI that asked to do `verb` with `args`; it may refuse.
    Client { verb: String, args: String },
    /// Ask the tool attached under this name; it may refuse (NACK).
    Tool(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RunTo {
    /// `dir/+Errors`, as B2's output.
    Errors,
    /// A new window named after the command.
    Window,
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
    /// A setting: the session's (`SERVER`) or one attachment's, which go
    /// with it. A client reads its own, then the session's.
    Set { owner: AttachmentId, key: String, value: String },
    Unset { owner: AttachmentId, key: String },
}
