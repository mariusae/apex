//! Identifiers. All are plain integers issued by whoever creates the thing;
//! uniqueness within a session is the metalog's business.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id {
    ($(#[$m:meta])* $name:ident, $prefix:literal) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }
    };
}

id!(/// A buffer (acme's `File`): a text with a name and history.
    BufferId, "b");
id!(/// A window: a tag view and a body.
    WindowId, "w");
id!(/// A column of the layout.
    ColumnId, "c");
id!(/// A terminal.
    TermId, "t");
id!(/// A fenced client identity within a session.
    AttachmentId, "a");
id!(/// A plumbing rule.
    RuleId, "r");
id!(/// An undo group: the edits of one command or one run of typing.
    GroupId, "g");

/// Sequence number within a shard's log; the first entry is 1.
pub type Seq = u64;
/// Fence epoch of a lease.
pub type Epoch = u32;
/// A buffer's version: the number of modifying entries applied to it.
pub type Version = u64;

/// The attachment id the server itself uses when it leads a shard.
pub const SERVER: AttachmentId = AttachmentId(0);

/// A shard: one independently replicated log and state machine.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Shard {
    Buffer(BufferId),
    Window(WindowId),
    Layout,
    Term(TermId),
    /// The session's metalog: shards, attachments, leases, plumb rules.
    Meta,
}

impl Shard {
    /// Pinned shards never lease out; the server always leads them.
    pub fn is_pinned(&self) -> bool {
        matches!(self, Shard::Term(_) | Shard::Meta)
    }
}

impl fmt::Display for Shard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Shard::Buffer(b) => write!(f, "buffer/{b}"),
            Shard::Window(w) => write!(f, "window/{w}"),
            Shard::Layout => write!(f, "layout"),
            Shard::Term(t) => write!(f, "term/{t}"),
            Shard::Meta => write!(f, "meta"),
        }
    }
}

/// The two texts of a window.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Part {
    Tag,
    Body,
}

/// A view: one text's selection and origin on a buffer (acme's `Text`).
/// Window tags and bodies, column tags and the top row all have one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum ViewId {
    Tag(WindowId),
    Body(WindowId),
    ColTag(ColumnId),
    Top,
}

impl ViewId {
    pub fn window(&self) -> Option<WindowId> {
        match self {
            ViewId::Tag(w) | ViewId::Body(w) => Some(*w),
            _ => None,
        }
    }
}

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViewId::Tag(w) => write!(f, "{w}/tag"),
            ViewId::Body(w) => write!(f, "{w}/body"),
            ViewId::ColTag(c) => write!(f, "{c}/tag"),
            ViewId::Top => write!(f, "top"),
        }
    }
}

/// Where a command was executed from.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum ExecCtx {
    Window(WindowId),
    Column(ColumnId),
    Top,
}
