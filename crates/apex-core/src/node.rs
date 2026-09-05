//! A node: a replica of the session that may lead some shards. The UI
//! client and the server are both nodes over the same log store. As leader
//! of a shard a node performs the built-in commands, types, selects, and
//! lowers Edit programs into entries; as follower it catches up.

use std::collections::BTreeMap;

use apex_edit::{Edit as EditLang, Intent};

use crate::entry::*;
use crate::ids::*;
use crate::log::{Log, LogError};
use crate::state::{Applied, ApplyError, State};

pub const WIN_TAG_SUFFIX: &str = " Del Snarf Undo Put | Look ";
pub const COL_TAG: &str = "New Cut Paste Snarf Sort Zerox Delcol ";
pub const TOP_TAG: &str = "Newcol Newterm Kill Putall Dump Exit ";
pub const ERRORS: &str = "+Errors";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Log(#[from] LogError),
    #[error(transparent)]
    Apply(#[from] ApplyError),
    #[error("not the leader of {0}")]
    NotLeader(Shard),
    #[error("{0}")]
    Missing(String),
    #[error("Edit: {0}")]
    Edit(#[from] apex_edit::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// What running a command amounted to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Executed {
    /// Ran here; the exec entry has its Done status.
    Done(Seq),
    /// Recorded for another handler (the server, a tool); pending.
    Deferred(Seq),
    /// Ran here and failed; the status entry carries the reason.
    Failed(Seq, String),
    /// `Exit`: the client should quit after flushing.
    Quit(Seq),
}

/// The result of running an Edit program as leader.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditRun {
    pub output: String,
    pub intents: Vec<Intent>,
    pub warnings: Vec<String>,
}

pub struct Node {
    pub state: State,
    pub attachment: AttachmentId,
    /// Leases this node believes it holds, from the metalog.
    epochs: BTreeMap<Shard, Epoch>,
    next_id: u64,
    next_group: u64,
    /// The view being typed into and its undo group, so consecutive
    /// keystrokes undo together.
    typing: Option<(ViewId, GroupId)>,
    /// The most recently selected text (acme's `seltext`).
    pub seltext: Option<ViewId>,
    /// Windows that were warned once about Del on a dirty buffer.
    warned: BTreeMap<WindowId, Version>,
    edit: EditLang,
}

fn count(s: &str) -> usize {
    s.chars().count()
}

impl Node {
    pub fn new(attachment: AttachmentId) -> Node {
        Node {
            state: State::new(),
            attachment,
            epochs: BTreeMap::new(),
            next_id: 1,
            next_group: 1,
            typing: None,
            seltext: None,
            warned: BTreeMap::new(),
            edit: EditLang::new(),
        }
    }

    /// Ids are unique per attachment without coordination.
    fn alloc(&mut self) -> u64 {
        let id = (self.attachment.0 << 40) | self.next_id;
        self.next_id += 1;
        id
    }

    fn new_group(&mut self) -> GroupId {
        let g = GroupId((self.attachment.0 << 40) | self.next_group);
        self.next_group += 1;
        g
    }

    pub fn leads(&self, shard: Shard) -> bool {
        self.epochs.contains_key(&shard)
    }

    // ---- replication --------------------------------------------------------

    /// Apply everything in `log` this node has not seen, metalog first.
    pub fn catch_up(&mut self, log: &Log) -> Result<()> {
        self.catch_up_shard(log, Shard::Meta)?;
        let shards: Vec<Shard> = log.shards().collect();
        for shard in shards {
            if shard != Shard::Meta {
                self.catch_up_shard(log, shard)?;
            }
        }
        self.refresh_leases();
        Ok(())
    }

    fn catch_up_shard(&mut self, log: &Log, shard: Shard) -> Result<()> {
        let after = self.state.applied(shard);
        for e in log.since(shard, after) {
            self.state.apply(shard, e)?;
        }
        Ok(())
    }

    fn refresh_leases(&mut self) {
        self.epochs.clear();
        for (shard, l) in &self.state.meta.leases {
            if l.holder == self.attachment && l.released.is_none() {
                self.epochs.insert(*shard, l.epoch);
            }
        }
    }

    /// Append as leader and apply.
    pub fn append(&mut self, log: &mut Log, shard: Shard, op: Op) -> Result<(Seq, Applied)> {
        let epoch = *self.epochs.get(&shard).ok_or(CoreError::NotLeader(shard))?;
        let e = log.append(shard, self.attachment, epoch, op)?;
        let a = self.state.apply(shard, &e)?;
        Ok((e.seq, a))
    }

    fn create_shard(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        for e in log.create_shard(shard, self.attachment)? {
            self.state.apply(Shard::Meta, &e)?;
        }
        self.refresh_leases();
        Ok(())
    }

    fn delete_shard(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        let e = log.delete_shard(shard)?;
        self.state.apply(Shard::Meta, &e)?;
        self.refresh_leases();
        Ok(())
    }

    /// Take the lease of a shard the server holds (or that was released).
    pub fn take_lease(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        let e = log.grant(shard, self.attachment)?;
        self.state.apply(Shard::Meta, &e)?;
        self.refresh_leases();
        Ok(())
    }

    /// Flush and hand the lease back (cooperative transfer).
    pub fn release_lease(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        let e = log.release(shard, self.attachment, log.last_seq(shard))?;
        self.state.apply(Shard::Meta, &e)?;
        self.refresh_leases();
        Ok(())
    }

    // ---- buffers, windows, columns -------------------------------------------

    pub fn create_buffer(&mut self, log: &mut Log, name: &str, text: &str, disk_hash: Option<String>) -> Result<BufferId> {
        let id = BufferId(self.alloc());
        self.create_shard(log, Shard::Buffer(id))?;
        self.append(log, Shard::Buffer(id), Op::Buffer(BufferOp::Create { name: name.into(), text: text.into(), disk_hash }))?;
        Ok(id)
    }

    /// Set up a fresh session's layout: the top row and one column.
    pub fn init_session(&mut self, log: &mut Log) -> Result<ColumnId> {
        let top = self.create_buffer(log, "", TOP_TAG, None)?;
        self.append(log, Shard::Buffer(top), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Top }))?;
        self.create_shard(log, Shard::Layout)?;
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::Init { top }))?;
        self.new_column(log, 0)
    }

    pub fn new_column(&mut self, log: &mut Log, at: usize) -> Result<ColumnId> {
        let id = ColumnId(self.alloc());
        let tag = self.create_buffer(log, "", COL_TAG, None)?;
        self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::ColTag(id) }))?;
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::ColNew { id, tag, at, weight: 1 }))?;
        Ok(id)
    }

    pub fn delete_column(&mut self, log: &mut Log, col: ColumnId) -> Result<()> {
        let c = self.state.layout.column(col).ok_or_else(|| CoreError::Missing(format!("column {col}")))?;
        if !c.wins.is_empty() {
            return Err(CoreError::Missing("column not empty".into()));
        }
        let tag = c.tag;
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::ColDel { id: col }))?;
        self.delete_shard(log, Shard::Buffer(tag))
    }

    /// A window on a new buffer.
    pub fn new_window(&mut self, log: &mut Log, col: ColumnId, name: &str, text: &str) -> Result<WindowId> {
        let body = self.create_buffer(log, name, text, None)?;
        self.open_window(log, col, body)
    }

    /// A window on an existing buffer (Zerox is this on the same buffer).
    pub fn open_window(&mut self, log: &mut Log, col: ColumnId, body: BufferId) -> Result<WindowId> {
        let name = self.state.buffer(body)?.name.clone();
        let id = WindowId(self.alloc());
        let tag = self.create_buffer(log, "", &format!("{name}{WIN_TAG_SUFFIX}"), None)?;
        self.create_shard(log, Shard::Window(id))?;
        self.append(log, Shard::Window(id), Op::Window(WindowOp::Create { tag, body: Body::Text(body) }))?;
        self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Tag(id) }))?;
        self.append(log, Shard::Buffer(body), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Body(id) }))?;
        let at = self.state.layout.column(col).map(|c| c.wins.len()).unwrap_or(0);
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::WinPlace { window: id, col, at, weight: 1 }))?;
        Ok(id)
    }

    pub fn zerox(&mut self, log: &mut Log, window: WindowId) -> Result<WindowId> {
        let w = self.state.window(window)?;
        let body = w.body_buffer().ok_or_else(|| CoreError::Missing("no body buffer".into()))?;
        let col = self.column_of(window)?;
        self.open_window(log, col, body)
    }

    /// Close a window. The body buffer's shard goes away with its last view.
    pub fn delete_window(&mut self, log: &mut Log, window: WindowId) -> Result<()> {
        let w = self.state.window(window)?.clone();
        self.append(log, Shard::Buffer(w.tag), Op::Buffer(BufferOp::ViewDel { view: ViewId::Tag(window) }))?;
        if let Some(b) = w.body_buffer() {
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::ViewDel { view: ViewId::Body(window) }))?;
        }
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::WinRemove { window }))?;
        self.append(log, Shard::Window(window), Op::Window(WindowOp::Delete))?;
        self.delete_shard(log, Shard::Window(window))?;
        self.delete_shard(log, Shard::Buffer(w.tag))?;
        if let Some(b) = w.body_buffer() {
            if self.state.buffer(b).map(|b| b.views.is_empty()).unwrap_or(false) {
                self.delete_shard(log, Shard::Buffer(b))?;
            }
        }
        if self.seltext.and_then(|v| v.window()) == Some(window) {
            self.seltext = None;
        }
        self.warned.remove(&window);
        Ok(())
    }

    pub fn column_of(&self, window: WindowId) -> Result<ColumnId> {
        self.state.layout.column_of(window).ok_or_else(|| CoreError::Missing(format!("window {window} is not placed")))
    }

    /// The buffer a view looks at.
    pub fn view_buffer(&self, view: ViewId) -> Result<BufferId> {
        match view {
            ViewId::Tag(w) => Ok(self.state.window(w)?.tag),
            ViewId::Body(w) => self.state.window(w)?.body_buffer().ok_or_else(|| CoreError::Missing("terminal body".into())),
            ViewId::ColTag(c) => {
                self.state.layout.column(c).map(|c| c.tag).ok_or_else(|| CoreError::Missing(format!("column {c}")))
            }
            ViewId::Top => self.state.layout.top.ok_or_else(|| CoreError::Missing("no top row".into())),
        }
    }

    pub fn selection(&self, view: ViewId) -> Result<(usize, usize)> {
        let b = self.view_buffer(view)?;
        let v = self.state.buffer(b)?.view(view);
        Ok((v.q0, v.q1))
    }

    pub fn selected_text(&self, view: ViewId) -> Result<String> {
        let b = self.view_buffer(view)?;
        let buf = self.state.buffer(b)?;
        let v = buf.view(view);
        Ok(buf.text.slice(v.q0, v.q1))
    }

    // ---- editing as leader ----------------------------------------------------

    fn typing_group(&mut self, view: ViewId) -> GroupId {
        match self.typing {
            Some((v, g)) if v == view => g,
            _ => {
                let g = self.new_group();
                self.typing = Some((view, g));
                g
            }
        }
    }

    /// A mouse action or command ends the current run of typing.
    pub fn end_typing(&mut self) {
        self.typing = None;
    }

    fn edit_op(&mut self, log: &mut Log, buffer: BufferId, q0: usize, nd: usize, text: &str, group: GroupId) -> Result<()> {
        let version = self.state.buffer(buffer)?.version;
        self.append(log, Shard::Buffer(buffer), Op::Buffer(BufferOp::Edit { version, q0, nd, text: text.into(), group }))?;
        Ok(())
    }

    /// A mouse selection (B1): ends the current run of typing.
    pub fn select(&mut self, log: &mut Log, view: ViewId, q0: usize, q1: usize) -> Result<()> {
        self.end_typing();
        let b = self.view_buffer(view)?;
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1 }))?;
        self.seltext = Some(view);
        Ok(())
    }

    pub fn set_origin(&mut self, log: &mut Log, view: ViewId, origin: usize) -> Result<()> {
        let b = self.view_buffer(view)?;
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Origin { view, origin }))?;
        Ok(())
    }

    /// Keyboard insertion at the selection (replacing it).
    pub fn insert(&mut self, log: &mut Log, view: ViewId, text: &str) -> Result<()> {
        let b = self.view_buffer(view)?;
        let (q0, q1) = self.selection(view)?;
        let group = self.typing_group(view);
        self.edit_op(log, b, q0, q1 - q0, text, group)?;
        let p = q0 + count(text);
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0: p, q1: p }))?;
        Ok(())
    }

    pub fn backspace(&mut self, log: &mut Log, view: ViewId) -> Result<()> {
        let b = self.view_buffer(view)?;
        let (q0, q1) = self.selection(view)?;
        let group = self.typing_group(view);
        if q1 > q0 {
            self.edit_op(log, b, q0, q1 - q0, "", group)?;
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1: q0 }))?;
        } else if q0 > 0 {
            self.edit_op(log, b, q0 - 1, 1, "", group)?;
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0: q0 - 1, q1: q0 - 1 }))?;
        }
        Ok(())
    }

    pub fn delete_forward(&mut self, log: &mut Log, view: ViewId) -> Result<()> {
        let b = self.view_buffer(view)?;
        let (q0, q1) = self.selection(view)?;
        let len = self.state.buffer(b)?.text.len();
        let group = self.typing_group(view);
        if q1 > q0 {
            self.edit_op(log, b, q0, q1 - q0, "", group)?;
        } else if q0 < len {
            self.edit_op(log, b, q0, 1, "", group)?;
        }
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1: q0 }))?;
        Ok(())
    }

    /// Replace the selection and select the inserted text (acme's Paste).
    pub fn replace_selection(&mut self, log: &mut Log, view: ViewId, text: &str) -> Result<()> {
        self.end_typing();
        let b = self.view_buffer(view)?;
        let (q0, q1) = self.selection(view)?;
        let group = self.new_group();
        self.edit_op(log, b, q0, q1 - q0, text, group)?;
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1: q0 + count(text) }))?;
        Ok(())
    }

    pub fn snarf(&mut self, log: &mut Log, view: ViewId) -> Result<()> {
        let text = self.selected_text(view)?;
        if !text.is_empty() {
            self.append(log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text }))?;
        }
        Ok(())
    }

    pub fn cut(&mut self, log: &mut Log, view: ViewId) -> Result<()> {
        self.snarf(log, view)?;
        let (q0, q1) = self.selection(view)?;
        if q1 > q0 {
            self.end_typing();
            let b = self.view_buffer(view)?;
            let group = self.new_group();
            self.edit_op(log, b, q0, q1 - q0, "", group)?;
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1: q0 }))?;
        }
        Ok(())
    }

    pub fn paste(&mut self, log: &mut Log, view: ViewId) -> Result<()> {
        let text = self.state.layout.snarf.clone();
        self.replace_selection(log, view, &text)
    }

    pub fn undo(&mut self, log: &mut Log, view: ViewId) -> Result<bool> {
        self.end_typing();
        let b = self.view_buffer(view)?;
        let version = self.state.buffer(b)?.version;
        if self.state.buffer(b)?.undo.is_empty() {
            return Ok(false);
        }
        let (_, a) = self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Undo { version }))?;
        if let Applied::UndoRange(Some((q0, q1))) = a {
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1 }))?;
        }
        Ok(true)
    }

    pub fn redo(&mut self, log: &mut Log, view: ViewId) -> Result<bool> {
        self.end_typing();
        let b = self.view_buffer(view)?;
        let version = self.state.buffer(b)?.version;
        if self.state.buffer(b)?.redo.is_empty() {
            return Ok(false);
        }
        let (_, a) = self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Redo { version }))?;
        if let Applied::UndoRange(Some((q0, q1))) = a {
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1 }))?;
        }
        Ok(true)
    }

    /// Search the body forward from the selection, wrapping; select a hit.
    pub fn look(&mut self, log: &mut Log, window: WindowId, needle: &str) -> Result<bool> {
        let view = ViewId::Body(window);
        let b = self.view_buffer(view)?;
        let buf = self.state.buffer(b)?;
        let (_, from) = self.selection(view)?;
        let n: Vec<char> = needle.chars().collect();
        if n.is_empty() {
            return Ok(false);
        }
        let text: Vec<char> = buf.text.to_string().chars().collect();
        let find = |start: usize, end: usize| -> Option<usize> {
            (start..end.saturating_sub(n.len() - 1)).find(|&i| text[i..i + n.len()] == n[..])
        };
        let hit = find(from, text.len()).or_else(|| find(0, from + n.len() - 1));
        match hit {
            Some(i) => {
                self.select(log, view, i, i + n.len())?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Run an Edit program on a window's body as leader and lower its
    /// changes into entries. Effects come back as intents.
    pub fn run_edit(&mut self, log: &mut Log, window: WindowId, program: &str) -> Result<EditRun> {
        self.end_typing();
        let view = ViewId::Body(window);
        let b = self.view_buffer(view)?;
        let (buf_name, dot, text) = {
            let buf = self.state.buffer(b)?;
            let v = buf.view(view);
            (buf.name.clone(), (v.q0, v.q1), buf.text.clone())
        };
        let name = if buf_name.is_empty() { None } else { Some(buf_name.as_str()) };
        let out = self.edit.run(&text, dot, name, program)?;
        let group = self.new_group();
        for c in &out.changes {
            let t: String = c.text.iter().collect();
            self.edit_op(log, b, c.q0, c.nd, &t, group)?;
        }
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0: out.dot.0, q1: out.dot.1 }))?;
        let output = out.output_string();
        let warnings = out.warnings.clone();
        let mut intents = Vec::new();
        for i in out.intents {
            match i {
                Intent::Undo { n } => {
                    for _ in 0..n.unsigned_abs() {
                        if n < 0 { self.redo(log, view)? } else { self.undo(log, view)? };
                    }
                }
                other => intents.push(other),
            }
        }
        Ok(EditRun { output, intents, warnings })
    }

    /// Append to a column's `+Errors` window, creating it if needed.
    pub fn errors(&mut self, log: &mut Log, col: ColumnId, text: &str) -> Result<WindowId> {
        let existing = self
            .state
            .layout
            .column(col)
            .map(|c| c.wins.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.window)
            .find(|w| {
                self.state
                    .window(*w)
                    .ok()
                    .and_then(|w| w.body_buffer())
                    .and_then(|b| self.state.buffer(b).ok())
                    .is_some_and(|b| b.name == ERRORS)
            });
        let window = match existing {
            Some(w) => w,
            None => self.new_window(log, col, ERRORS, "")?,
        };
        let view = ViewId::Body(window);
        let b = self.view_buffer(view)?;
        let end = self.state.buffer(b)?.text.len();
        let group = self.new_group();
        self.edit_op(log, b, end, 0, text, group)?;
        let end = self.state.buffer(b)?.text.len();
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0: end, q1: end }))?;
        Ok(window)
    }

    // ---- commands (B2) ---------------------------------------------------------

    /// Where editing commands executed from `ctx` apply.
    pub fn edit_target(&self, ctx: ExecCtx) -> Option<ViewId> {
        match ctx {
            ExecCtx::Window(w) => match self.seltext {
                Some(v) if v.window() == Some(w) && self.selection(v).map(|(a, b)| a < b).unwrap_or(false) => Some(v),
                _ => Some(ViewId::Body(w)),
            },
            _ => self.seltext,
        }
    }

    fn exec_at(&self, ctx: ExecCtx) -> ExecAt {
        let view = self.edit_target(ctx);
        let (buffer, version, q0, q1) = view
            .and_then(|v| {
                let b = self.view_buffer(v).ok()?;
                let buf = self.state.buffer(b).ok()?;
                let s = buf.view(v);
                Some((Some(b), buf.version, s.q0, s.q1))
            })
            .unwrap_or((None, 0, 0, 0));
        ExecAt { buffer, version, q0, q1 }
    }

    fn append_exec(&mut self, log: &mut Log, ctx: ExecCtx, op: ExecOp) -> Result<Seq> {
        match ctx {
            ExecCtx::Window(w) => Ok(self.append(log, Shard::Window(w), Op::Window(WindowOp::Exec(op)))?.0),
            _ => Ok(self.append(log, Shard::Layout, Op::Layout(LayoutOp::Exec { ctx, op }))?.0),
        }
    }

    fn append_status(&mut self, log: &mut Log, ctx: ExecCtx, exec: Seq, status: ExecStatusOp) -> Result<()> {
        match ctx {
            ExecCtx::Window(w) => {
                self.append(log, Shard::Window(w), Op::Window(WindowOp::Status { exec, status }))?;
            }
            _ => {
                self.append(log, Shard::Layout, Op::Layout(LayoutOp::Status { exec, status }))?;
            }
        }
        Ok(())
    }

    /// Which handler a command resolves to.
    pub fn resolve(text: &str) -> Handler {
        let t = text.trim_start();
        if t.starts_with('|') || t.starts_with('<') || t.starts_with('>') {
            return Handler::Server;
        }
        match t.split_whitespace().next().unwrap_or("") {
            "Cut" | "Paste" | "Snarf" | "Undo" | "Redo" | "Look" | "Edit" | "Newcol" | "Delcol" | "Del" | "Zerox"
            | "Font" | "Sort" | "Exit" => Handler::Leader,
            "New" if t.split_whitespace().nth(1).is_none() => Handler::Leader,
            _ => Handler::Server,
        }
    }

    /// Execute `text` as B2 would from `ctx`. Built-ins run here; anything
    /// else is recorded for the server.
    pub fn exec(&mut self, log: &mut Log, ctx: ExecCtx, text: &str) -> Result<Executed> {
        self.end_typing();
        let text = text.trim().to_string();
        let handler = Node::resolve(&text);
        let at = self.exec_at(ctx);
        let seq = self.append_exec(log, ctx, ExecOp { text: text.clone(), handler: handler.clone(), at })?;
        if handler != Handler::Leader {
            return Ok(Executed::Deferred(seq));
        }
        match self.builtin(log, ctx, &text) {
            Ok(quit) => {
                // Del removes the window and its log; the metalog's ShardDel
                // is the record then.
                let gone = matches!(ctx, ExecCtx::Window(w) if self.state.window(w).is_err());
                if !gone {
                    self.append_status(log, ctx, seq, ExecStatusOp::Done)?;
                }
                Ok(if quit { Executed::Quit(seq) } else { Executed::Done(seq) })
            }
            Err(CoreError::Missing(reason)) | Err(CoreError::Edit(apex_edit::Error(reason))) => {
                self.append_status(log, ctx, seq, ExecStatusOp::Failed(reason.clone()))?;
                Ok(Executed::Failed(seq, reason))
            }
            Err(e) => Err(e),
        }
    }

    fn column_ctx(&self, ctx: ExecCtx) -> Result<ColumnId> {
        match ctx {
            ExecCtx::Window(w) => self.column_of(w),
            ExecCtx::Column(c) => Ok(c),
            ExecCtx::Top => self.state.layout.cols.first().map(|c| c.id).ok_or_else(|| CoreError::Missing("no column".into())),
        }
    }

    /// Returns `Ok(true)` for Exit.
    fn builtin(&mut self, log: &mut Log, ctx: ExecCtx, text: &str) -> Result<bool> {
        let mut words = text.split_whitespace();
        let cmd = words.next().unwrap_or("");
        let rest = text[cmd.len()..].trim();
        let win = match ctx {
            ExecCtx::Window(w) => Some(w),
            _ => None,
        };
        match cmd {
            "Cut" | "Paste" | "Snarf" | "Undo" | "Redo" => {
                let Some(target) = self.edit_target(ctx) else { return Ok(false) };
                match cmd {
                    "Cut" => self.cut(log, target)?,
                    "Paste" => self.paste(log, target)?,
                    "Snarf" => self.snarf(log, target)?,
                    "Undo" => {
                        self.undo(log, target)?;
                    }
                    _ => {
                        self.redo(log, target)?;
                    }
                }
            }
            "Look" => {
                let w = win.ok_or_else(|| CoreError::Missing("Look needs a window".into()))?;
                let needle = if rest.is_empty() { self.selected_text(ViewId::Body(w))? } else { rest.to_string() };
                self.look(log, w, &needle)?;
            }
            "Edit" => {
                let w = win.ok_or_else(|| CoreError::Missing("Edit needs a window".into()))?;
                let run = self.run_edit(log, w, rest)?;
                if !run.output.is_empty() {
                    let col = self.column_of(w)?;
                    self.errors(log, col, &run.output)?;
                }
                for wmsg in run.warnings {
                    let col = self.column_of(w)?;
                    self.errors(log, col, &format!("{wmsg}\n"))?;
                }
                if !run.intents.is_empty() {
                    return Err(CoreError::Missing("Edit: file and pipe commands are not supported here yet".into()));
                }
            }
            "New" => {
                let col = self.column_ctx(ctx)?;
                let w = self.new_window(log, col, "", "")?;
                self.seltext = Some(ViewId::Body(w));
            }
            "Newcol" => {
                let at = self.state.layout.cols.len();
                self.new_column(log, at)?;
            }
            "Delcol" => {
                let col = self.column_ctx(ctx)?;
                if self.state.layout.cols.len() <= 1 {
                    return Err(CoreError::Missing("can't delete last column".into()));
                }
                let wins: Vec<WindowId> = self.state.layout.column(col).map(|c| c.wins.iter().map(|s| s.window).collect()).unwrap_or_default();
                for w in &wins {
                    if self.window_dirty(*w) {
                        return Err(CoreError::Missing("can't delete column: window is dirty".into()));
                    }
                }
                for w in wins {
                    self.delete_window(log, w)?;
                }
                self.delete_column(log, col)?;
            }
            "Del" => {
                let w = win.ok_or_else(|| CoreError::Missing("Del needs a window".into()))?;
                if self.window_dirty(w) && self.window_last_view(w) {
                    let version = self.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| self.state.buffer(b).ok()).map(|b| b.version).unwrap_or(0);
                    if self.warned.get(&w) != Some(&version) {
                        self.warned.insert(w, version);
                        return Err(CoreError::Missing("file modified; Del again to discard".into()));
                    }
                }
                self.delete_window(log, w)?;
            }
            "Zerox" => {
                let w = win.ok_or_else(|| CoreError::Missing("Zerox needs a window".into()))?;
                self.zerox(log, w)?;
            }
            "Font" => {
                let w = win.ok_or_else(|| CoreError::Missing("Font needs a window".into()))?;
                let mono = !self.state.window(w)?.mono;
                self.append(log, Shard::Window(w), Op::Window(WindowOp::Font { mono }))?;
            }
            "Sort" => {
                let col = self.column_ctx(ctx)?;
                let mut slots = self.state.layout.column(col).map(|c| c.wins.clone()).unwrap_or_default();
                let name = |n: &Node, w: WindowId| -> String {
                    n.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| n.state.buffer(b).ok()).map(|b| b.name.clone()).unwrap_or_default()
                };
                slots.sort_by_key(|s| name(self, s.window));
                for (i, s) in slots.iter().enumerate() {
                    self.append(log, Shard::Layout, Op::Layout(LayoutOp::WinPlace { window: s.window, col, at: i, weight: s.weight }))?;
                }
            }
            "Exit" => return Ok(true),
            _ => return Err(CoreError::Missing(format!("{cmd}: not a built-in"))),
        }
        Ok(false)
    }

    fn window_dirty(&self, w: WindowId) -> bool {
        self.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| self.state.buffer(b).ok()).is_some_and(|b| b.dirty())
    }

    fn window_last_view(&self, w: WindowId) -> bool {
        self.state
            .window(w)
            .ok()
            .and_then(|x| x.body_buffer())
            .and_then(|b| self.state.buffer(b).ok())
            .is_some_and(|b| b.views.iter().filter(|(v, _)| matches!(v, ViewId::Body(_))).count() <= 1)
    }
}
