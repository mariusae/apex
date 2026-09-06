//! The session's state: a pure function of its logs. `apply` is
//! deterministic, does no I/O, and is the only way state changes.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::buffer::Buffer;
use crate::entry::*;
use crate::ids::*;
use crate::tiling::Rect;

// ---- windows ----------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ExecStatus {
    Pending,
    Done,
    Failed(String),
    Unknown,
}

impl From<&ExecStatusOp> for ExecStatus {
    fn from(s: &ExecStatusOp) -> ExecStatus {
        match s {
            ExecStatusOp::Done => ExecStatus::Done,
            ExecStatusOp::Failed(r) => ExecStatus::Failed(r.clone()),
            ExecStatusOp::Unknown => ExecStatus::Unknown,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ExecRecord {
    pub text: String,
    pub handler: Handler,
    pub at: ExecAt,
    pub status: ExecStatus,
}

impl ExecRecord {
    fn new(x: &ExecOp) -> ExecRecord {
        ExecRecord { text: x.text.clone(), handler: x.handler.clone(), at: x.at, status: ExecStatus::Pending }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub tag: BufferId,
    pub body: Body,
    pub mono: bool,
    /// acme's `tabstop` (default 4) and `autoindent`.
    pub tabstop: u32,
    pub autoindent: bool,
    /// acme's `tagexpand`: false after Up in the tag, true after Down.
    pub tagexpand: bool,
    pub execs: BTreeMap<Seq, ExecRecord>,
}

impl Window {
    pub fn body_buffer(&self) -> Option<BufferId> {
        match self.body {
            Body::Text(b) => Some(b),
            Body::Term(_) => None,
        }
    }
}

// ---- layout -----------------------------------------------------------------

/// A window's place in a column, as acme keeps it: its rectangle `r`
/// (bottom trimmed to whole body lines), the body's rectangle, how many
/// lines the tag takes, how many lines of text the body shows, and
/// acme's `w->maxlines` (the most lines it has shown, which `colgrow`
/// uses as its natural size).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Slot {
    pub window: WindowId,
    pub r: Rect,
    pub body: Rect,
    pub taglines: i32,
    pub nlines: i32,
    /// acme's `body.fr.maxlines`: whole lines that fit the body. Zero for
    /// a window obscured by a full-column one, whose rectangles go stale.
    pub frmax: i32,
    pub maxlines: i32,
}

/// A column: its rectangle (the tag is its first line) and its windows
/// top to bottom. `safe` is acme's: false while one window has been
/// grown to the whole column and the others are obscured.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Column {
    pub id: ColumnId,
    pub tag: BufferId,
    pub r: Rect,
    pub safe: bool,
    pub wins: Vec<Slot>,
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Layout {
    pub top: Option<BufferId>,
    /// The row's rectangle: the top tag is its first line.
    pub r: Rect,
    pub cols: Vec<Column>,
    pub snarf: String,
    /// Execs from column tags and the top row.
    pub execs: BTreeMap<Seq, (ExecCtx, ExecRecord)>,
}

impl Layout {
    pub fn column_of(&self, w: WindowId) -> Option<ColumnId> {
        self.cols.iter().find(|c| c.wins.iter().any(|s| s.window == w)).map(|c| c.id)
    }
    pub fn column(&self, id: ColumnId) -> Option<&Column> {
        self.cols.iter().find(|c| c.id == id)
    }
    pub fn column_index(&self, id: ColumnId) -> Option<usize> {
        self.cols.iter().position(|c| c.id == id)
    }
    /// The column and window indices of a placed window.
    pub fn place_of(&self, w: WindowId) -> Option<(usize, usize)> {
        self.cols.iter().enumerate().find_map(|(ci, c)| c.wins.iter().position(|s| s.window == w).map(|wi| (ci, wi)))
    }
    pub fn slot(&self, w: WindowId) -> Option<&Slot> {
        self.place_of(w).map(|(ci, wi)| &self.cols[ci].wins[wi])
    }
    fn unplace(&mut self, w: WindowId) {
        for c in &mut self.cols {
            c.wins.retain(|s| s.window != w);
        }
    }
}

// ---- terminals --------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Term {
    pub cols: u16,
    pub rows: u16,
    pub grid: Vec<Vec<Cell>>,
    pub cursor: (u16, u16),
    pub cursor_visible: bool,
    pub exit: Option<i32>,
}

// ---- meta -------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Lease {
    pub holder: AttachmentId,
    pub epoch: Epoch,
    /// Sequence the holder leads from.
    pub seq: Seq,
    /// An attachment waiting for a transfer.
    pub pending: Option<AttachmentId>,
    /// The holder has released at this sequence.
    pub released: Option<Seq>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub attachment: AttachmentId,
    pub priority: i32,
    pub rule: PlumbRule,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    pub shards: BTreeSet<Shard>,
    pub attachments: BTreeMap<AttachmentId, Attachment>,
    pub leases: BTreeMap<Shard, Lease>,
    pub rules: BTreeMap<RuleId, Rule>,
}

// ---- state ------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub buffers: BTreeMap<BufferId, Buffer>,
    pub windows: BTreeMap<WindowId, Window>,
    pub layout: Layout,
    pub terms: BTreeMap<TermId, Term>,
    pub meta: Meta,
    /// Last applied sequence per shard.
    pub applied: BTreeMap<Shard, Seq>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("{op} does not belong on shard {shard}")]
    WrongShard { shard: Shard, op: String },
    #[error("{shard}: expected seq {expected}, got {got}")]
    Sequence { shard: Shard, expected: Seq, got: Seq },
    #[error("{shard}: expected version {expected}, got {got}")]
    Version { shard: Shard, expected: Version, got: Version },
    #[error("{0} does not exist")]
    Missing(String),
    #[error("{0} already exists")]
    Exists(String),
}

/// What `apply` did, when the caller needs to know more than "ok".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    Ok,
    /// The range an undo or redo affected, if anything was undone.
    UndoRange(Option<(usize, usize)>),
}

impl State {
    pub fn new() -> State {
        State::default()
    }

    pub fn buffer(&self, id: BufferId) -> Result<&Buffer, ApplyError> {
        self.buffers.get(&id).ok_or_else(|| ApplyError::Missing(format!("buffer {id}")))
    }

    fn buffer_mut(&mut self, id: BufferId) -> Result<&mut Buffer, ApplyError> {
        self.buffers.get_mut(&id).ok_or_else(|| ApplyError::Missing(format!("buffer {id}")))
    }

    pub fn window(&self, id: WindowId) -> Result<&Window, ApplyError> {
        self.windows.get(&id).ok_or_else(|| ApplyError::Missing(format!("window {id}")))
    }

    fn window_mut(&mut self, id: WindowId) -> Result<&mut Window, ApplyError> {
        self.windows.get_mut(&id).ok_or_else(|| ApplyError::Missing(format!("window {id}")))
    }

    pub fn applied(&self, shard: Shard) -> Seq {
        self.applied.get(&shard).copied().unwrap_or(0)
    }

    /// Apply a metalog op out of band (a mirror dropping a shard before the
    /// server's entry arrives). Does not advance any sequence.
    pub fn apply_unsequenced(&mut self, e: &Entry) -> Result<Applied, ApplyError> {
        match &e.op {
            Op::Meta(m) => self.apply_meta(m),
            _ => Err(ApplyError::WrongShard { shard: Shard::Meta, op: format!("{:?}", e.op) }),
        }
    }

    /// Apply one entry of `shard`'s log. Entries must arrive in sequence.
    pub fn apply(&mut self, shard: Shard, e: &Entry) -> Result<Applied, ApplyError> {
        if !e.op.fits(shard) {
            return Err(ApplyError::WrongShard { shard, op: format!("{:?}", e.op) });
        }
        let expected = self.applied(shard) + 1;
        if e.seq != expected {
            return Err(ApplyError::Sequence { shard, expected, got: e.seq });
        }
        let r = match (&e.op, shard) {
            (Op::Buffer(op), Shard::Buffer(id)) => self.apply_buffer(id, op)?,
            (Op::Window(op), Shard::Window(id)) => self.apply_window(id, op, e.seq)?,
            (Op::Layout(op), Shard::Layout) => self.apply_layout(op, e.seq)?,
            (Op::Term(op), Shard::Term(id)) => self.apply_term(id, op)?,
            (Op::Meta(op), Shard::Meta) => self.apply_meta(op)?,
            _ => unreachable!("fits() checked"),
        };
        self.applied.insert(shard, e.seq);
        Ok(r)
    }

    fn apply_buffer(&mut self, id: BufferId, op: &BufferOp) -> Result<Applied, ApplyError> {
        match op {
            BufferOp::Create { name, text, disk_hash } => {
                if self.buffers.contains_key(&id) {
                    return Err(ApplyError::Exists(format!("buffer {id}")));
                }
                self.buffers.insert(id, Buffer::new(id, name, text, disk_hash.clone()));
            }
            BufferOp::Edit { version, q0, nd, text, group } => {
                let b = self.buffer_mut(id)?;
                if b.version != *version {
                    return Err(ApplyError::Version { shard: Shard::Buffer(id), expected: b.version, got: *version });
                }
                b.edit(*q0, *nd, text, *group);
            }
            BufferOp::Undo { version } => {
                let b = self.buffer_mut(id)?;
                if b.version != *version {
                    return Err(ApplyError::Version { shard: Shard::Buffer(id), expected: b.version, got: *version });
                }
                return Ok(Applied::UndoRange(b.undo()));
            }
            BufferOp::Redo { version } => {
                let b = self.buffer_mut(id)?;
                if b.version != *version {
                    return Err(ApplyError::Version { shard: Shard::Buffer(id), expected: b.version, got: *version });
                }
                return Ok(Applied::UndoRange(b.redo()));
            }
            BufferOp::Clean { version, disk_hash } => {
                let b = self.buffer_mut(id)?;
                b.clean_version = *version;
                b.disk_hash = disk_hash.clone();
                b.stale = false;
            }
            BufferOp::Stale { disk_hash } => {
                let b = self.buffer_mut(id)?;
                b.stale = true;
                b.disk_hash = Some(disk_hash.clone());
            }
            BufferOp::Rename { name } => {
                self.buffer_mut(id)?.name = name.clone();
            }
            BufferOp::ViewAdd { view } => {
                self.buffer_mut(id)?.views.entry(*view).or_default();
            }
            BufferOp::ViewDel { view } => {
                self.buffer_mut(id)?.views.remove(view);
            }
            BufferOp::Select { view, q0, q1 } => {
                self.buffer_mut(id)?.set_select(*view, *q0, *q1);
            }
            BufferOp::Origin { view, origin } => {
                self.buffer_mut(id)?.set_origin(*view, *origin);
            }
        }
        Ok(Applied::Ok)
    }

    fn apply_window(&mut self, id: WindowId, op: &WindowOp, seq: Seq) -> Result<Applied, ApplyError> {
        match op {
            WindowOp::Create { tag, body } => {
                if self.windows.contains_key(&id) {
                    return Err(ApplyError::Exists(format!("window {id}")));
                }
                self.windows.insert(id, Window { id, tag: *tag, body: *body, mono: false, tabstop: 4, autoindent: false, tagexpand: true, execs: BTreeMap::new() });
            }
            WindowOp::Font { mono } => self.window_mut(id)?.mono = *mono,
            WindowOp::Tab { n } => self.window_mut(id)?.tabstop = (*n).max(1),
            WindowOp::Indent { on } => self.window_mut(id)?.autoindent = *on,
            WindowOp::TagExpand { on } => self.window_mut(id)?.tagexpand = *on,
            WindowOp::Exec(x) => {
                self.window_mut(id)?.execs.insert(seq, ExecRecord::new(x));
            }
            WindowOp::Status { exec, status } => {
                let w = self.window_mut(id)?;
                let r = w.execs.get_mut(exec).ok_or_else(|| ApplyError::Missing(format!("exec {exec} of window {id}")))?;
                r.status = ExecStatus::from(status);
            }
            WindowOp::Delete => {
                self.windows.remove(&id);
                self.layout.unplace(id);
            }
        }
        Ok(Applied::Ok)
    }

    fn apply_layout(&mut self, op: &LayoutOp, seq: Seq) -> Result<Applied, ApplyError> {
        let l = &mut self.layout;
        match op {
            LayoutOp::Exec { ctx, op } => {
                l.execs.insert(seq, (*ctx, ExecRecord::new(op)));
            }
            LayoutOp::Status { exec, status } => {
                let r = l.execs.get_mut(exec).ok_or_else(|| ApplyError::Missing(format!("layout exec {exec}")))?;
                r.1.status = ExecStatus::from(status);
            }
            LayoutOp::Init { top, r } => {
                l.top = Some(*top);
                l.r = *r;
            }
            LayoutOp::Arrange { r, cols } => {
                // a window may sit in one place only
                let mut seen = std::collections::BTreeSet::new();
                for c in cols {
                    for s in &c.wins {
                        if !seen.insert(s.window) {
                            return Err(ApplyError::Exists(format!("window {} placed twice", s.window)));
                        }
                    }
                }
                l.r = *r;
                l.cols = cols.clone();
            }
            LayoutOp::Snarf { text } => l.snarf = text.clone(),
        }
        Ok(Applied::Ok)
    }

    fn apply_term(&mut self, id: TermId, op: &TermOp) -> Result<Applied, ApplyError> {
        match op {
            TermOp::Create { cols, rows } => {
                if self.terms.contains_key(&id) {
                    return Err(ApplyError::Exists(format!("term {id}")));
                }
                let blank = vec![Cell { ch: ' ', fg: 0, bg: 0, flags: 0 }; *cols as usize];
                self.terms.insert(
                    id,
                    Term {
                        cols: *cols,
                        rows: *rows,
                        grid: vec![blank; *rows as usize],
                        cursor: (0, 0),
                        cursor_visible: true,
                        exit: None,
                    },
                );
            }
            _ => {
                let t = self.terms.get_mut(&id).ok_or_else(|| ApplyError::Missing(format!("term {id}")))?;
                match op {
                    TermOp::Rows { first, rows } => {
                        for (i, row) in rows.iter().enumerate() {
                            let r = *first as usize + i;
                            if r < t.grid.len() {
                                t.grid[r] = row.clone();
                            }
                        }
                    }
                    TermOp::Cursor { col, row, visible } => {
                        t.cursor = (*col, *row);
                        t.cursor_visible = *visible;
                    }
                    TermOp::Resize { cols, rows } => {
                        t.cols = *cols;
                        t.rows = *rows;
                        let blank = Cell { ch: ' ', fg: 0, bg: 0, flags: 0 };
                        t.grid.resize(*rows as usize, vec![blank; *cols as usize]);
                        for r in &mut t.grid {
                            r.resize(*cols as usize, blank);
                        }
                    }
                    TermOp::Exit { status } => t.exit = Some(*status),
                    TermOp::Create { .. } => unreachable!(),
                }
            }
        }
        Ok(Applied::Ok)
    }

    fn apply_meta(&mut self, op: &MetaOp) -> Result<Applied, ApplyError> {
        let m = &mut self.meta;
        match op {
            MetaOp::Init => {}
            MetaOp::ShardNew { shard } => {
                m.shards.insert(*shard);
                m.leases.entry(*shard).or_insert(Lease { holder: SERVER, epoch: 0, seq: 0, pending: None, released: None });
            }
            MetaOp::ShardDel { shard } => {
                m.shards.remove(shard);
                m.leases.remove(shard);
                match shard {
                    Shard::Buffer(b) => {
                        self.buffers.remove(b);
                    }
                    Shard::Window(w) => {
                        self.windows.remove(w);
                        self.layout.unplace(*w);
                    }
                    Shard::Term(t) => {
                        self.terms.remove(t);
                    }
                    _ => {}
                }
                self.applied.remove(shard);
            }
            MetaOp::Attach { attachment, kind, name } => {
                m.attachments.insert(*attachment, Attachment { kind: *kind, name: name.clone() });
            }
            MetaOp::Detach { attachment } => {
                m.attachments.remove(attachment);
            }
            MetaOp::LeaseRequest { shard, to } => {
                let l = m.leases.get_mut(shard).ok_or_else(|| ApplyError::Missing(format!("lease {shard}")))?;
                l.pending = Some(*to);
            }
            MetaOp::LeaseRelease { shard, seq, .. } => {
                let l = m.leases.get_mut(shard).ok_or_else(|| ApplyError::Missing(format!("lease {shard}")))?;
                l.released = Some(*seq);
            }
            MetaOp::LeaseGrant { shard, to, epoch, seq } => {
                let l = m.leases.get_mut(shard).ok_or_else(|| ApplyError::Missing(format!("lease {shard}")))?;
                *l = Lease { holder: *to, epoch: *epoch, seq: *seq, pending: None, released: None };
            }
            MetaOp::LeaseReclaim { shard, epoch, seq, .. } => {
                let l = m.leases.get_mut(shard).ok_or_else(|| ApplyError::Missing(format!("lease {shard}")))?;
                *l = Lease { holder: SERVER, epoch: *epoch, seq: *seq, pending: l.pending, released: None };
            }
            MetaOp::PlumbRuleInstall { id, attachment, priority, rule } => {
                m.rules.insert(*id, Rule { attachment: *attachment, priority: *priority, rule: rule.clone() });
            }
            MetaOp::PlumbRuleRemove { id } => {
                m.rules.remove(id);
            }
        }
        Ok(Applied::Ok)
    }

    // ---- hashing and snapshots ----------------------------------------------

    /// A hash of the whole state, for divergence checks between replicas.
    pub fn hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        for b in self.buffers.values() {
            b.hash_into(&mut h);
        }
        h.update(b"windows");
        for w in self.windows.values() {
            h.update(&w.id.0.to_le_bytes());
            h.update(&w.tag.0.to_le_bytes());
            match w.body {
                Body::Text(b) => {
                    h.update(&[1]);
                    h.update(&b.0.to_le_bytes());
                }
                Body::Term(t) => {
                    h.update(&[2]);
                    h.update(&t.0.to_le_bytes());
                }
            }
            h.update(&[w.mono as u8, w.autoindent as u8, w.tagexpand as u8]);
            h.update(&w.tabstop.to_le_bytes());
            for (seq, e) in &w.execs {
                h.update(&seq.to_le_bytes());
                h.update(e.text.as_bytes());
                h.update(format!("{:?}{:?}", e.handler, e.status).as_bytes());
            }
        }
        h.update(b"layout");
        h.update(&self.layout.top.map(|b| b.0).unwrap_or(0).to_le_bytes());
        h.update(&postcard::to_stdvec(&self.layout.r).unwrap_or_default());
        for c in &self.layout.cols {
            h.update(&c.id.0.to_le_bytes());
            h.update(&c.tag.0.to_le_bytes());
            h.update(&postcard::to_stdvec(&(c.r, c.safe)).unwrap_or_default());
            for s in &c.wins {
                h.update(&postcard::to_stdvec(s).unwrap_or_default());
            }
        }
        h.update(self.layout.snarf.as_bytes());
        for (seq, (ctx, e)) in &self.layout.execs {
            h.update(&seq.to_le_bytes());
            h.update(format!("{ctx:?}{}{:?}{:?}", e.text, e.handler, e.status).as_bytes());
        }
        h.update(b"terms");
        for (id, t) in &self.terms {
            h.update(&id.0.to_le_bytes());
            h.update(&t.cols.to_le_bytes());
            h.update(&t.rows.to_le_bytes());
            for row in &t.grid {
                for c in row {
                    h.update(&(c.ch as u32).to_le_bytes());
                    h.update(&c.fg.to_le_bytes());
                    h.update(&c.bg.to_le_bytes());
                    h.update(&[c.flags]);
                }
            }
            h.update(&[t.cursor.0 as u8, t.cursor.1 as u8, t.cursor_visible as u8]);
            h.update(&t.exit.unwrap_or(-1).to_le_bytes());
        }
        h.update(b"meta");
        h.update(&postcard::to_stdvec(&self.meta).unwrap_or_default());
        h.update(b"applied");
        for (s, q) in &self.applied {
            h.update(format!("{s}={q};").as_bytes());
        }
        *h.finalize().as_bytes()
    }

    /// A compact binary snapshot of the whole state.
    pub fn to_snapshot(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("state serializes")
    }

    pub fn from_snapshot(bytes: &[u8]) -> Result<State, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}
