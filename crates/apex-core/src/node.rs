//! A node: a replica of the session that may lead some shards. The UI
//! client and the server are both nodes over the same log store. As leader
//! of a shard a node performs the built-in commands, types, selects, and
//! lowers Edit programs into entries; as follower it catches up.

use std::collections::BTreeMap;

use apex_edit::{Edit as EditLang, Intent};

use crate::entry::*;
use crate::ids::*;
use crate::log::{Log, LogError};
use crate::state::{Applied, ApplyError, Layout, State};
use crate::text::Text;
use crate::tiling::{self, Rect, Warp};

/// What a new window's tag holds after its name; the words before `|`
/// are kept up to date by [`Node::update_tags`], as acme's `winsettag`.
pub const WIN_TAG_SUFFIX: &str = " Del Snarf | Look ";
pub const COL_TAG: &str = "New Cut Paste Snarf Sort Zerox Delcol ";
pub const TOP_TAG: &str = "Newcol Newterm Win Web Kill Putall Exit ";
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

/// acme's `textdoubleclick`: a click next to a bracket or quote selects
/// what it encloses; on a line end, the line; else the alphanumeric word.
pub fn double_click(t: &Text, q: usize) -> (usize, usize) {
    const LEFT: [&[char]; 3] = [&['{', '[', '(', '<', '«'], &['\n'], &['\'', '"', '`']];
    const RIGHT: [&[char]; 3] = [&['}', ']', ')', '>', '»'], &['\n'], &['\'', '"', '`']];
    let n = t.len();
    let (mut q0, mut q1) = (q.min(n), q.min(n));
    for i in 0..3 {
        let (l, r) = (LEFT[i], RIGHT[i]);
        // try matching character to left, looking right
        let c = if q0 == 0 { '\n' } else { t.char_at(q0 - 1) };
        if let Some(p) = l.iter().position(|&x| x == c) {
            let mut qq = q0;
            if click_match(t, c, r[p], 1, &mut qq) {
                q1 = qq - (c != '\n') as usize;
            }
            return (q0, q1);
        }
        // try matching character to right, looking left
        let c = if q0 == n { '\n' } else { t.char_at(q0) };
        if let Some(p) = r.iter().position(|&x| x == c) {
            let mut qq = q0;
            if click_match(t, c, l[p], -1, &mut qq) {
                q1 = q0 + (q0 < n && c == '\n') as usize;
                q0 = qq;
                if c != '\n' || qq != 0 || t.char_at(0) == '\n' {
                    q0 += 1;
                }
            }
            return (q0, q1);
        }
    }
    // try filling out word to right, then to left
    while q1 < n && t.char_at(q1).is_alphanumeric() {
        q1 += 1;
    }
    while q0 > 0 && t.char_at(q0 - 1).is_alphanumeric() {
        q0 -= 1;
    }
    (q0, q1)
}

/// acme's `textclickmatch`.
fn click_match(t: &Text, cl: char, cr: char, dir: i32, q: &mut usize) -> bool {
    let n = t.len();
    let mut nest = 1;
    loop {
        let c;
        if dir > 0 {
            if *q == n {
                break;
            }
            c = t.char_at(*q);
            *q += 1;
        } else {
            if *q == 0 {
                break;
            }
            *q -= 1;
            c = t.char_at(*q);
        }
        if c == cr {
            nest -= 1;
            if nest == 0 {
                return true;
            }
        } else if c == cl {
            nest += 1;
        }
    }
    cl == '\n' && nest == 1
}

/// acme's three erasing keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Erase {
    /// ^H, Backspace
    Char,
    /// ^U: to the start of the line
    Line,
    /// ^W: the word before the cursor
    Word,
}

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
    /// What `errors` appended, for the client to show (`take_shows`).
    pub shows: Vec<(ViewId, usize)>,
    /// Places to go (`Goto`, `Back`, `Fwd`), for the client (or a headless
    /// leader) to open and select (`take_gotos`).
    pub gotos: Vec<Loc>,
    /// Windows that were warned once about Del on a dirty buffer.
    warned: BTreeMap<WindowId, Version>,
    edit: EditLang,
    /// What acme's tiling needs to know about text on screen; the client
    /// supplies its measurements, a headless leader the defaults.
    pub tiling: Box<dyn tiling::Info + Send + Sync>,
    /// Where acme would move the mouse after the last layout change; the
    /// client takes it.
    pub warp: Option<Warp>,
    /// acme's `activecol`: where `New` puts its window.
    pub activecol: Option<ColumnId>,
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
            shows: Vec::new(),
            gotos: Vec::new(),
            warned: BTreeMap::new(),
            edit: EditLang::new(),
            tiling: Box::new(tiling::Headless::default()),
            warp: None,
            activecol: None,
        }
    }

    /// Append the whole tiling as it now stands in `l`.
    fn arrange(&mut self, log: &mut Log, l: &Layout) -> Result<()> {
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::Arrange { r: l.r, cols: l.cols.clone() }))?;
        Ok(())
    }

    fn place_of(&self, w: WindowId) -> Result<(usize, usize)> {
        self.state.layout.place_of(w).ok_or_else(|| CoreError::Missing(format!("window {w} is not placed")))
    }

    fn column_index(&self, col: ColumnId) -> Result<usize> {
        self.state.layout.column_index(col).ok_or_else(|| CoreError::Missing(format!("column {col}")))
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
        self.refresh_leases(log);
        Ok(())
    }

    fn catch_up_shard(&mut self, log: &Log, shard: Shard) -> Result<()> {
        let after = self.state.applied(shard);
        for e in log.since(shard, after) {
            self.state.apply(shard, e)?;
        }
        Ok(())
    }

    /// The log store is the fencing authority (a mirror knows the leases
    /// the server will honour), so leases come from it, not from state.
    fn refresh_leases(&mut self, log: &Log) {
        self.epochs = log.held_by(self.attachment);
    }

    /// Append as leader and apply.
    pub fn append(&mut self, log: &mut Log, shard: Shard, op: Op) -> Result<(Seq, Applied)> {
        let epoch = *self.epochs.get(&shard).ok_or(CoreError::NotLeader(shard))?;
        let e = log.append(shard, self.attachment, epoch, op)?;
        let a = self.state.apply(shard, &e)?;
        Ok((e.seq, a))
    }

    /// Create a shard (pinned ones stay with the server; others come to this
    /// node at epoch 1).
    pub fn create_shard(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        for e in log.create_shard(shard, self.attachment)? {
            self.state.apply(Shard::Meta, &e)?;
        }
        if log.is_mirror() {
            // the server's metalog entries arrive later; meanwhile the state
            // must know the shard exists so entries for it apply
            self.state.meta.shards.insert(shard);
        }
        self.refresh_leases(log);
        Ok(())
    }

    pub fn delete_shard(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        if let Some(e) = log.delete_shard(shard)? {
            self.state.apply(Shard::Meta, &e)?;
        } else {
            // a mirror: drop the shard's state now; the metalog entry follows
            let fake = Entry { seq: 0, attachment: SERVER, epoch: 0, op: Op::Meta(MetaOp::ShardDel { shard }) };
            let _ = self.state.apply_unsequenced(&fake);
        }
        self.refresh_leases(log);
        Ok(())
    }

    /// Take the lease of a shard the server holds (or that was released).
    pub fn take_lease(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        let e = log.grant(shard, self.attachment)?;
        self.state.apply(Shard::Meta, &e)?;
        self.refresh_leases(log);
        Ok(())
    }

    /// Flush and hand the lease back (cooperative transfer).
    pub fn release_lease(&mut self, log: &mut Log, shard: Shard) -> Result<()> {
        self.catch_up(log)?;
        let e = log.release(shard, self.attachment, log.last_seq(shard))?;
        self.state.apply(Shard::Meta, &e)?;
        self.refresh_leases(log);
        Ok(())
    }

    // ---- buffers, windows, columns -------------------------------------------

    pub fn create_buffer(&mut self, log: &mut Log, name: &str, text: &str, disk_hash: Option<String>) -> Result<BufferId> {
        let id = BufferId(self.alloc());
        self.create_shard(log, Shard::Buffer(id))?;
        self.append(log, Shard::Buffer(id), Op::Buffer(BufferOp::Create { name: name.into(), text: text.into(), disk_hash }))?;
        Ok(id)
    }

    /// Set up a fresh session's layout: the top row and one column. The
    /// row's rectangle is a guess until a client resizes it.
    pub fn init_session(&mut self, log: &mut Log) -> Result<ColumnId> {
        let top = self.create_buffer(log, "", TOP_TAG, None)?;
        self.append(log, Shard::Buffer(top), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Top }))?;
        self.create_shard(log, Shard::Layout)?;
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::Init { top, r: Rect::new(0, 0, 1100, 700) }))?;
        self.new_column(log, None)
    }

    /// A column at `x` (acme's `rowadd`), or with no `x`, taking 40% of
    /// the last column.
    pub fn new_column(&mut self, log: &mut Log, x: Option<i32>) -> Result<ColumnId> {
        let id = ColumnId(self.alloc());
        let tag = self.create_buffer(log, "", COL_TAG, None)?;
        self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::ColTag(id) }))?;
        let mut l = self.state.layout.clone();
        if tiling::rowadd(&mut l, tiling::AddingCol::New { id, tag }, x, &*self.tiling).is_none() {
            self.delete_shard(log, Shard::Buffer(tag))?;
            return Err(CoreError::Missing("no room for a column".into()));
        }
        self.arrange(log, &l)?;
        Ok(id)
    }

    pub fn delete_column(&mut self, log: &mut Log, col: ColumnId) -> Result<()> {
        let ci = self.column_index(col)?;
        let c = &self.state.layout.cols[ci];
        if !c.wins.is_empty() {
            return Err(CoreError::Missing("column not empty".into()));
        }
        let tag = c.tag;
        let mut l = self.state.layout.clone();
        tiling::rowclose(&mut l, ci, &*self.tiling);
        self.arrange(log, &l)?;
        if self.activecol == Some(col) {
            self.activecol = None;
        }
        self.delete_shard(log, Shard::Buffer(tag))
    }

    /// A window on a new buffer, in `col`, splitting the last window.
    pub fn new_window(&mut self, log: &mut Log, col: ColumnId, name: &str, text: &str) -> Result<WindowId> {
        let body = self.create_buffer(log, name, text, None)?;
        self.open_window(log, col, body)
    }

    /// A window on an existing buffer in `col`, at `y` if given, else
    /// splitting the last window (acme's `coladd`).
    pub fn open_window(&mut self, log: &mut Log, col: ColumnId, body: BufferId) -> Result<WindowId> {
        self.open_window_at(log, col, body, None)
    }

    pub fn open_window_at(&mut self, log: &mut Log, col: ColumnId, body: BufferId, y: Option<i32>) -> Result<WindowId> {
        let name = self.state.buffer(body)?.name.clone();
        let id = WindowId(self.alloc());
        let tag = self.create_buffer(log, "", &format!("{name}{WIN_TAG_SUFFIX}"), None)?;
        self.create_shard(log, Shard::Window(id))?;
        self.append(log, Shard::Window(id), Op::Window(WindowOp::Create { tag, body: Body::Text(body) }))?;
        self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Tag(id) }))?;
        self.append(log, Shard::Buffer(body), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Body(id) }))?;
        self.place(log, col, id, y)?;
        Ok(id)
    }

    /// acme's `makenewwindow`: a window on `body` in the active column
    /// (else the column of `from`, else `fallback`), where the biggest
    /// empty space or the biggest window is.
    pub fn make_window(&mut self, log: &mut Log, from: Option<WindowId>, fallback: ColumnId, body: BufferId) -> Result<WindowId> {
        let col = self
            .activecol
            .filter(|c| self.state.layout.column(*c).is_some())
            .or_else(|| self.seltext.and_then(|v| v.window()).and_then(|w| self.state.layout.column_of(w)))
            .or_else(|| from.and_then(|w| self.state.layout.column_of(w)))
            .unwrap_or(fallback);
        self.activecol = Some(col);
        let ci = self.column_index(col)?;
        let y = tiling::newwindow_y(&self.state.layout, ci, from, &*self.tiling);
        let w = self.open_window_at(log, col, body, y)?;
        // if(w->body.fr.maxlines < 2) colgrow(w->col, w, 1)
        let few = self.state.layout.slot(w).is_some_and(|s| s.fr_maxlines(self.tiling.body_font_height(w).max(1)) < 2);
        if few && from.is_some() {
            self.grow_window(log, w, 1)?;
            self.warp = Some(Warp::NewWindow(w));
        }
        Ok(w)
    }

    /// A window whose body is a terminal (the term shard exists already).
    pub fn open_term_window(&mut self, log: &mut Log, col: ColumnId, name: &str, term: TermId) -> Result<WindowId> {
        let id = WindowId(self.alloc());
        let tag = self.create_buffer(log, "", &format!("{name} Del Snarf | Look "), None)?;
        self.create_shard(log, Shard::Window(id))?;
        self.append(log, Shard::Window(id), Op::Window(WindowOp::Create { tag, body: Body::Term(term) }))?;
        self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Tag(id) }))?;
        self.place(log, col, id, None)?;
        Ok(id)
    }

    /// A web window on `url` in `col` (WEB.md §2): its tag names the URL,
    /// as a terminal's names its directory; the client renders the page,
    /// and Back, Fwd and Get in the tag are the page's history and reload.
    pub fn open_web_window(&mut self, log: &mut Log, col: ColumnId, url: &str) -> Result<WindowId> {
        let id = WindowId(self.alloc());
        let tag = self.create_buffer(log, "", &format!("{url} Del Snarf Back Fwd Get | Look "), None)?;
        self.create_shard(log, Shard::Window(id))?;
        self.append(log, Shard::Window(id), Op::Window(WindowOp::Create { tag, body: Body::Web }))?;
        self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Tag(id) }))?;
        self.place(log, col, id, None)?;
        Ok(id)
    }

    /// A window whose text is HTML shown as a page (WEB.md §2.5): a
    /// buffer named `name` holding `text`, as `new_window` makes one,
    /// with a body the client renders.
    pub fn open_html_window(&mut self, log: &mut Log, col: ColumnId, name: &str, text: &str) -> Result<WindowId> {
        let id = WindowId(self.alloc());
        let body = self.create_buffer(log, name, text, None)?;
        let tag = self.create_buffer(log, "", &format!("{name} Del Snarf | Look "), None)?;
        self.create_shard(log, Shard::Window(id))?;
        self.append(log, Shard::Window(id), Op::Window(WindowOp::Create { tag, body: Body::Html(body) }))?;
        self.append(log, Shard::Buffer(body), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Body(id) }))?;
        self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Tag(id) }))?;
        self.place(log, col, id, None)?;
        Ok(id)
    }

    /// A web window went somewhere: its name follows the page, and the
    /// place it left goes onto the navigation stack, so Back returns.
    pub fn web_navigate(&mut self, log: &mut Log, w: WindowId, url: &str) -> Result<()> {
        let win = self.state.window(w)?;
        if win.body != Body::Web {
            return Err(CoreError::Missing(format!("window {w}: not a web window")));
        }
        let from = self.window_name(w);
        if from == url {
            return Ok(());
        }
        let tag = win.tag;
        let rest = self.state.buffer(tag).map(|t| t.text.to_string()).unwrap_or_default();
        let rest = rest.split_once(' ').map(|(_, r)| r.to_string()).unwrap_or_default();
        self.set_content(log, tag, &format!("{url} {rest}"))?;
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::Visit { from: Some(Loc { name: from, pos: Pos::Keep }), to: Loc { name: url.to_string(), pos: Pos::Keep } }))?;
        Ok(())
    }

    /// Put a (new) window into a column (acme's `coladd`) and record the
    /// mouse warp acme makes: near the layout box, in the body.
    fn place(&mut self, log: &mut Log, col: ColumnId, w: WindowId, y: Option<i32>) -> Result<()> {
        let ci = self.column_index(col)?;
        let mut l = self.state.layout.clone();
        tiling::coladd(&mut l, ci, tiling::Adding::New(w), y, &*self.tiling);
        self.arrange(log, &l)?;
        self.warp = Some(Warp::NewWindow(w));
        Ok(())
    }

    /// A window's tag changed shape (or the client measured it anew):
    /// refit it in its own space, as acme's `winsettag` does.
    pub fn refit_window(&mut self, log: &mut Log, w: WindowId) -> Result<()> {
        let (ci, wi) = self.place_of(w)?;
        let mut l = self.state.layout.clone();
        let r = l.cols[ci].wins[wi].r;
        let mut full = r;
        full.y1 = if wi + 1 < l.cols[ci].wins.len() { l.cols[ci].wins[wi + 1].r.y0 - tiling::BORDER } else { l.cols[ci].r.y1 };
        let last = wi + 1 == l.cols[ci].wins.len();
        tiling::winresize(&mut l, ci, wi, full, last, &*self.tiling);
        if l != self.state.layout {
            self.arrange(log, &l)?;
        }
        Ok(())
    }

    /// acme's `textshow` on a window showing no lines (squeezed to its
    /// tag, or obscured): grow it a little (`colgrow` with button 1) so
    /// what is shown can be seen. No mouse warp: that is the caller's.
    pub fn reveal(&mut self, log: &mut Log, w: WindowId) -> Result<()> {
        let Some((ci, wi)) = self.state.layout.place_of(w) else { return Ok(()) };
        let bf = self.tiling.body_font_height(w).max(1);
        if self.state.layout.cols[ci].wins[wi].fr_maxlines(bf) > 0 {
            return Ok(());
        }
        let mut l = self.state.layout.clone();
        tiling::colgrow(&mut l, ci, wi, 1, &*self.tiling);
        self.arrange(log, &l)
    }

    /// acme's `colgrow` on a window's layout box: button 1 a bit, 2 as
    /// big as can be, 3 the whole column.
    pub fn grow_window(&mut self, log: &mut Log, w: WindowId, but: i32) -> Result<()> {
        let (ci, wi) = self.place_of(w)?;
        let mut l = self.state.layout.clone();
        tiling::colgrow(&mut l, ci, wi, but, &*self.tiling);
        self.arrange(log, &l)?;
        self.warp = Some(Warp::WinButton(w));
        Ok(())
    }

    /// acme's `coldragwin`: a window's layout box pressed with `but` at
    /// `op` and released at `p` (row coordinates).
    pub fn drag_window(&mut self, log: &mut Log, w: WindowId, but: i32, op: (i32, i32), p: (i32, i32)) -> Result<()> {
        let (ci, wi) = self.place_of(w)?;
        let mut l = self.state.layout.clone();
        let warp = tiling::coldragwin(&mut l, ci, wi, but, op, p, &*self.tiling);
        if l != self.state.layout {
            self.arrange(log, &l)?;
        }
        if let Some(c) = self.state.layout.column_of(w) {
            self.activecol = Some(c);
        }
        self.warp = warp;
        Ok(())
    }

    /// acme's `rowdragcol`: a column's layout box dragged from `op` to `p`.
    pub fn drag_column(&mut self, log: &mut Log, col: ColumnId, op: (i32, i32), p: (i32, i32)) -> Result<()> {
        let ci = self.column_index(col)?;
        let mut l = self.state.layout.clone();
        let warp = tiling::rowdragcol(&mut l, ci, op, p, &*self.tiling);
        if l != self.state.layout {
            self.arrange(log, &l)?;
        }
        self.activecol = Some(col);
        self.warp = warp;
        Ok(())
    }

    /// The row's rectangle changed (the window was resized): acme's
    /// `rowresize`, keeping every proportion.
    pub fn resize_layout(&mut self, log: &mut Log, r: Rect) -> Result<()> {
        if r == self.state.layout.r {
            return Ok(());
        }
        let mut l = self.state.layout.clone();
        tiling::rowresize(&mut l, r, &*self.tiling);
        self.arrange(log, &l)
    }

    /// acme's `Sort`: a column's windows in name order.
    pub fn sort_column(&mut self, log: &mut Log, col: ColumnId) -> Result<()> {
        let ci = self.column_index(col)?;
        let mut l = self.state.layout.clone();
        let names: BTreeMap<WindowId, String> = l.cols[ci].wins.iter().map(|s| (s.window, self.window_name(s.window))).collect();
        tiling::colsort(&mut l, ci, |w| names.get(&w).cloned().unwrap_or_default(), &*self.tiling);
        self.arrange(log, &l)
    }

    /// Replace a buffer's whole content (a `Get`, a watcher reload) in one
    /// undo group, keeping views where the text still allows.
    pub fn set_content(&mut self, log: &mut Log, buffer: BufferId, text: &str) -> Result<()> {
        self.end_typing();
        let len = self.state.buffer(buffer)?.text.len();
        let group = self.new_group();
        self.edit_op(log, buffer, 0, len, text, group)
    }

    /// The name shown in a window's tag: its body buffer's name.
    /// Insert `text` at `q0` in a buffer without touching any view's dot
    /// beyond the shift an insert makes (a tool writing at an address,
    /// as win writes its shell's output).
    pub fn insert_text(&mut self, log: &mut Log, buffer: BufferId, q0: usize, text: &str) -> Result<()> {
        let group = self.new_group();
        self.edit_op(log, buffer, q0, 0, text, group)
    }

    /// acme's `wincommit` for a tag: the name typed into it becomes the
    /// buffer's name (a click in the tag, or a command from it, commits).
    /// A relative name stays relative here; `Put` makes it absolute.
    pub fn commit_tag(&mut self, log: &mut Log, window: WindowId) -> Result<()> {
        let w = self.state.window(window)?;
        let Some(b) = w.body_buffer() else { return Ok(()) };
        let typed = self.state.buffer(w.tag)?.text.to_string().split(' ').next().unwrap_or("").to_string();
        if typed != self.state.buffer(b)?.name {
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Rename { name: typed }))?;
        }
        Ok(())
    }

    /// Is a process behind this window: a terminal whose program runs, or a
    /// text window a tool (win) keeps live? A third state beside clean and
    /// dirty; it ends with the program, or with the tool's attachment.
    pub fn window_live(&self, w: WindowId) -> bool {
        let Ok(win) = self.state.window(w) else { return false };
        match win.body {
            Body::Term(t) => self.state.terms.get(&t).is_some_and(|t| t.exit.is_none()),
            _ => win.live.is_some_and(|a| self.state.meta.attachments.contains_key(&a)),
        }
    }

    /// What kind of window this is, for plumbing rules.
    pub fn window_kind(&self, window: WindowId) -> WinKind {
        let Ok(w) = self.state.window(window) else { return WinKind::File };
        if matches!(w.body, Body::Term(_)) {
            return WinKind::Term;
        }
        if matches!(w.body, Body::Web | Body::Html(_)) {
            return WinKind::Web;
        }
        let name = self.window_name(window);
        if name.ends_with("+Errors") {
            WinKind::Errors
        } else if name.ends_with('/') {
            WinKind::Dir
        } else {
            WinKind::File
        }
    }

    pub fn window_name(&self, window: WindowId) -> String {
        let Ok(w) = self.state.window(window) else { return String::new() };
        match w.body_buffer() {
            Some(b) => self.state.buffer(b).map(|b| b.name.clone()).unwrap_or_default(),
            // a terminal has no file: its name lives in its tag, as win's does
            None => self.state.buffer(w.tag).map(|t| t.text.to_string().split(' ').next().unwrap_or("").to_string()).unwrap_or_default(),
        }
    }

    /// acme's `zeroxx`: `coladd(w->col, nil, w, -1)`, another window on
    /// the same buffer, splitting the column's last window.
    pub fn zerox(&mut self, log: &mut Log, window: WindowId) -> Result<WindowId> {
        let w = self.state.window(window)?;
        let body = w.body_buffer().ok_or_else(|| CoreError::Missing("no body buffer".into()))?;
        let col = self.column_of(window)?;
        self.open_window(log, col, body)
    }

    /// Close a window (acme's `colclose`). The body buffer's shard goes
    /// away with its last view. When the next window down takes the
    /// space, acme moves the mouse onto its `Del`; that is recorded in
    /// `warp`.
    pub fn delete_window(&mut self, log: &mut Log, window: WindowId) -> Result<()> {
        let w = self.state.window(window)?.clone();
        self.append(log, Shard::Buffer(w.tag), Op::Buffer(BufferOp::ViewDel { view: ViewId::Tag(window) }))?;
        if let Some(b) = w.body_buffer() {
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::ViewDel { view: ViewId::Body(window) }))?;
        }
        let mut next = None;
        if let Some((ci, wi)) = self.state.layout.place_of(window) {
            let mut l = self.state.layout.clone();
            let (_, n) = tiling::colclose(&mut l, ci, wi, &*self.tiling);
            next = n;
            self.arrange(log, &l)?;
        }
        self.warp = Some(Warp::Closed { window, next });
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
        let mut text = text.to_string();
        if text == "\n" && matches!(view, ViewId::Body(_)) {
            // acme's autoindent: copy the previous line's leading blanks
            let auto = view.window().and_then(|w| self.state.window(w).ok()).is_some_and(|w| w.autoindent);
            if auto {
                let t = &self.state.buffer(b)?.text;
                let nnb = Self::bswidth(t, q0, Erase::Line);
                let mut ws = String::new();
                for i in 0..nnb {
                    let c = t.char_at(q0 - nnb + i);
                    if c != ' ' && c != '\t' {
                        break;
                    }
                    ws.push(c);
                }
                text.push_str(&ws);
            }
        }
        let text = text.as_str();
        self.edit_op(log, b, q0, q1 - q0, text, group)?;
        let p = q0 + count(text);
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0: p, q1: p }))?;
        Ok(())
    }

    /// acme's `textbswidth`: how many runes ^H, ^U or ^W erase before `q0`.
    pub fn bswidth(text: &Text, q0: usize, kind: Erase) -> usize {
        if kind == Erase::Char {
            return 1;
        }
        let mut q = q0;
        let mut skipping = true;
        while q > 0 {
            let r = text.char_at(q - 1);
            if r == '\n' {
                // eat at most one more character
                if q == q0 {
                    q -= 1; // eat the newline
                }
                break;
            }
            if kind == Erase::Word {
                let eq = r.is_alphanumeric();
                if eq && skipping {
                    skipping = false; // found one; stop skipping
                } else if !eq && !skipping {
                    break;
                }
            }
            q -= 1;
        }
        q0 - q
    }

    /// acme's `texttype` for ^H, ^U and ^W (and Backspace, which is ^H):
    /// a selection is cut first, then the width is erased, never past
    /// the window's origin.
    pub fn erase(&mut self, log: &mut Log, view: ViewId, kind: Erase) -> Result<()> {
        let b = self.view_buffer(view)?;
        let (q0, q1) = self.selection(view)?;
        let group = self.typing_group(view);
        let mut q0 = q0;
        if q1 > q0 {
            let text = self.state.buffer(b)?.text.slice(q0, q1);
            self.append(log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text }))?;
            self.edit_op(log, b, q0, q1 - q0, "", group)?;
            self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1: q0 }))?;
        }
        if q0 == 0 {
            return Ok(()); // nothing to erase
        }
        let nnb = {
            let buf = self.state.buffer(b)?;
            Self::bswidth(&buf.text, q0, kind)
        };
        let q1 = q0;
        q0 = q1 - nnb;
        // if selection is at beginning of window, avoid deleting invisible text
        let org = self.state.buffer(b)?.views.get(&view).map(|v| v.origin).unwrap_or(0);
        if q0 < org {
            q0 = org;
        }
        if q1 <= q0 {
            return Ok(());
        }
        self.edit_op(log, b, q0, q1 - q0, "", group)?;
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1: q0 }))?;
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
    /// acme's `search`, in the text `view`: forward from its selection's
    /// end, wrapping around; the match becomes the selection.
    pub fn look(&mut self, log: &mut Log, view: ViewId, needle: &str) -> Result<bool> {
        self.look_dir(log, view, needle, false)
    }

    /// `look`, backwards when `reverse`: the last occurrence ending at or
    /// before the selection's start, wrapping from the end (acme's
    /// `search` with reverse, shift-B3).
    pub fn look_dir(&mut self, log: &mut Log, view: ViewId, needle: &str, reverse: bool) -> Result<bool> {
        let b = self.view_buffer(view)?;
        let buf = self.state.buffer(b)?;
        let (q0, q1) = self.selection(view)?;
        let n: Vec<char> = needle.chars().collect();
        if n.is_empty() {
            return Ok(false);
        }
        let text: Vec<char> = buf.text.to_string().chars().collect();
        if n.len() > text.len() {
            return Ok(false);
        }
        let find = |start: usize, end: usize| -> Option<usize> {
            (start..end.saturating_sub(n.len() - 1)).find(|&i| text[i..i + n.len()] == n[..])
        };
        let rfind = |start: usize, end: usize| -> Option<usize> {
            // matches starting in [start, end - n) with the match ending at most at end
            (start..end.saturating_sub(n.len() - 1)).rev().find(|&i| text[i..i + n.len()] == n[..])
        };
        let hit = if reverse {
            rfind(0, q0).or_else(|| rfind(q0, text.len()))
        } else {
            let from = q1;
            find(from, text.len()).or_else(|| find(0, from + n.len() - 1))
        };
        match hit {
            Some(i) => {
                self.select(log, view, i, i + n.len())?;
                self.seltext = Some(view);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// acme's `winclean`: may this window go? Scratch windows (`+Errors`,
    /// `guide`) and directories always; a dirty window warns once
    /// ("name modified") and goes the second time, as acme clears its
    /// dirty flag after warning.
    pub fn winclean(&mut self, log: &mut Log, w: WindowId, _conservative: bool) -> Result<bool> {
        let name = self.window_name(w);
        let isdir = name.ends_with('/');
        let isscratch = name.ends_with("+Errors") || name.ends_with("/guide");
        if isscratch || isdir || self.window_live(w) {
            return Ok(true); // a live window's text is a transcript, not a file
        }
        if !self.window_dirty(w) {
            return Ok(true);
        }
        let Some(b) = self.state.window(w)?.body_buffer() else { return Ok(true) };
        let (version, len) = {
            let buf = self.state.buffer(b)?;
            (buf.version, buf.text.len())
        };
        if self.warned.get(&w) == Some(&version) {
            return Ok(true);
        }
        let dir = self.error_dir(Some(w));
        if !name.is_empty() {
            self.errors(log, dir.as_deref(), &format!("{name} modified\n"))?;
        } else {
            if len < 100 {
                return Ok(true); // don't whine if it's too small
            }
            self.errors(log, dir.as_deref(), "unnamed file modified\n")?;
        }
        self.warned.insert(w, version);
        Ok(false)
    }

    /// acme's `colclean`.
    pub fn colclean(&mut self, log: &mut Log, col: ColumnId) -> Result<bool> {
        let wins: Vec<WindowId> = self.state.layout.column(col).map(|c| c.wins.iter().map(|s| s.window).collect()).unwrap_or_default();
        let mut clean = true;
        for w in wins {
            clean &= self.winclean(log, w, true)?;
        }
        Ok(clean)
    }

    /// acme's `winsettag1`: the words before `|` in every window's tag —
    /// `Del Snarf`, then `Undo`, `Redo`, `Put`, `Get` as they apply, and
    /// `Back Fwd Get` on a web window — brought up to date. The text
    /// after `|` is the user's.
    pub fn update_tags(&mut self, log: &mut Log) -> Result<()> {
        let wins: Vec<WindowId> = self.state.windows.keys().copied().collect();
        for w in wins {
            let Ok(win) = self.state.window(w) else { continue };
            let tag = win.tag;
            let name = self.window_name(w);
            let mut new = format!("{name} Del Snarf");
            let filemenu = !name.ends_with("+Errors");
            if let (Some(b), true) = (win.body_buffer(), filemenu) {
                let buf = self.state.buffer(b)?;
                if !buf.undo.is_empty() {
                    new.push_str(" Undo");
                }
                if !buf.redo.is_empty() {
                    new.push_str(" Redo");
                }
                let isdir = name.ends_with('/');
                if !isdir && !name.is_empty() && buf.dirty() {
                    new.push_str(" Put");
                }
                if isdir {
                    new.push_str(" Get");
                }
            }
            if win.body == Body::Web {
                // a page's history and reload (the client does them)
                new.push_str(" Back Fwd Get");
            }
            new.push_str(" |");
            let old = self.state.buffer(tag)?.text.to_string();
            // a name typed into the tag stays until it is committed (acme's
            // wincommit): only what follows the first word is ours
            let typed = old.split(' ').next().unwrap_or("").to_string();
            if typed != name && win.body_buffer().is_some() {
                new = format!("{typed}{}", &new[name.len()..]);
            }
            let k = old.chars().position(|c| c == '|').map(|i| i + 1).unwrap_or(old.chars().count());
            let head: String = old.chars().take(k).collect();
            if head != new {
                if !old.contains('|') {
                    new.push_str(" Look ");
                }
                let group = self.new_group();
                self.edit_op(log, tag, 0, k, &new, group)?;
            }
        }
        Ok(())
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
    /// acme's `errorwin`: the directory of a window's file, which names its
    /// `+Errors` window. `None` for unnamed windows and terminals.
    pub fn error_dir(&self, w: Option<WindowId>) -> Option<String> {
        let name = self.window_name(w?);
        if name.is_empty() || name.starts_with('+') {
            return None;
        }
        if name.ends_with('/') {
            return Some(name.trim_end_matches('/').to_string());
        }
        name.rsplit_once('/').map(|(d, _)| if d.is_empty() { "/".to_string() } else { d.to_string() })
    }

    /// acme's `errorwin1`: append `text` to `dir/+Errors` (or `+Errors`),
    /// making the window in the last column if there is none.
    pub fn errors(&mut self, log: &mut Log, dir: Option<&str>, text: &str) -> Result<WindowId> {
        let name = match dir {
            Some(d) if !d.is_empty() => format!("{}/{ERRORS}", d.trim_end_matches('/')),
            _ => ERRORS.to_string(),
        };
        let existing = self.state.windows.keys().copied().find(|w| self.window_name(*w) == name);
        let window = match existing {
            Some(w) => w,
            None => {
                let col = match self.state.layout.cols.last() {
                    Some(c) => c.id,
                    None => self.new_column(log, None)?,
                };
                self.new_window(log, col, &name, "")?
            }
        };
        let view = ViewId::Body(window);
        let b = self.view_buffer(view)?;
        let q0 = self.state.buffer(b)?.text.len();
        let group = self.new_group();
        self.edit_op(log, b, q0, 0, text, group)?;
        let end = self.state.buffer(b)?.text.len();
        // acme's flushwarnings: textshow(q0, end): the new text selected,
        // and its start brought on screen
        self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view, q0, q1: end }))?;
        self.shows.push((view, q0));
        Ok(window)
    }

    pub fn take_gotos(&mut self) -> Vec<Loc> {
        std::mem::take(&mut self.gotos)
    }

    /// Where the user is: the window last selected in, and its dot.
    pub fn current_loc(&self) -> Option<Loc> {
        let v = self.seltext?;
        let w = v.window()?;
        let name = self.window_name(w);
        if name.is_empty() {
            return None;
        }
        let (q0, q1) = self.selection(ViewId::Body(w)).ok()?;
        Some(Loc { name, pos: Pos::Chars(q0, q1) })
    }

    /// The character range a position names in `w`'s body.
    pub fn loc_range(&self, w: WindowId, pos: &Pos) -> Option<(usize, usize)> {
        let b = self.view_buffer(ViewId::Body(w)).ok()?;
        let t = &self.state.buffer(b).ok()?.text;
        let n = t.len();
        Some(match pos {
            Pos::Keep => return None,
            Pos::Chars(q0, q1) => ((*q0).min(n), (*q1).min(n).max((*q0).min(n))),
            Pos::Line(l) => {
                // the whole line, its newline included, as acme's address does
                let line = l.saturating_sub(1).min(t.line_count().saturating_sub(1));
                t.line_range(line).map(|(s, e)| (s, (e + 1).min(n))).unwrap_or((n, n))
            }
            Pos::LineCol(line, col) => {
                let line = (*line).min(t.line_count().saturating_sub(1));
                let Some((s, e)) = t.line_range(line) else { return Some((n, n)) };
                let mut units = 0;
                let mut chars = 0;
                for c in t.slice(s, e).chars() {
                    if units >= *col {
                        break;
                    }
                    units += c.len_utf16();
                    chars += 1;
                }
                (s + chars, s + chars)
            }
        })
    }

    /// Land at a location whose window is open: select, show, warp.
    pub fn land(&mut self, log: &mut Log, loc: &Loc) -> Result<Option<WindowId>> {
        let Some(w) = self.state.windows.keys().copied().find(|w| self.window_name(*w) == loc.name) else { return Ok(None) };
        if let Some((q0, q1)) = self.loc_range(w, &loc.pos) {
            self.select(log, ViewId::Body(w), q0, q1)?;
        }
        self.reveal(log, w)?;
        self.seltext = Some(ViewId::Body(w));
        self.warp = Some(Warp::Sel(ViewId::Body(w)));
        Ok(Some(w))
    }

    /// Positions a client should bring on screen (acme's `textshow`),
    /// since the last call: the start of new `+Errors` text.
    pub fn take_shows(&mut self) -> Vec<(ViewId, usize)> {
        std::mem::take(&mut self.shows)
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

    /// Report the outcome of an exec (the performer does this).
    pub fn append_status(&mut self, log: &mut Log, ctx: ExecCtx, exec: Seq, status: ExecStatusOp) -> Result<()> {
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
            "Cut" | "Paste" | "Snarf" | "Undo" | "Redo" | "Look" | "Edit" | "Newcol" | "Delcol" | "Del" | "Delete" | "Zerox"
            | "Font" | "Sort" | "Exit" | "Tab" | "Indent" | "ID" | "Send" | "Web" => Handler::Leader,
            "New" if t.split_whitespace().nth(1).is_none() => Handler::Leader,
            _ => Handler::Server,
        }
    }

    /// Execute `text` as B2 would from `ctx`. Built-ins run here; anything
    /// else is recorded for the server.
    pub fn exec(&mut self, log: &mut Log, ctx: ExecCtx, text: &str) -> Result<Executed> {
        // acme's get: a dirty window is asked once before reloading
        if text.trim() == "Get" {
            if let ExecCtx::Window(w) = ctx {
                let len = self.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| self.state.buffer(b).ok()).map(|b| b.text.len()).unwrap_or(0);
                if len > 0 && !self.window_name(w).ends_with('/') && !self.winclean(log, w, true)? {
                    return Ok(Executed::Done(0));
                }
            }
        }
        self.end_typing();
        let text = text.trim().to_string();
        let mut handler = Node::resolve(&text);
        if text == "Send" {
            if let ExecCtx::Window(w) = ctx {
                if matches!(self.state.window(w).map(|x| x.body), Ok(Body::Term(_))) {
                    handler = Handler::Server; // the shell gets it
                }
            }
        }
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
        let arg = rest.split_whitespace().next();
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
                self.look(log, ViewId::Body(w), &needle)?;
            }
            "Edit" => {
                let w = win.ok_or_else(|| CoreError::Missing("Edit needs a window".into()))?;
                let run = self.run_edit(log, w, rest)?;
                let dir = self.error_dir(Some(w));
                if !run.output.is_empty() {
                    self.errors(log, dir.as_deref(), &run.output)?;
                }
                for wmsg in run.warnings {
                    self.errors(log, dir.as_deref(), &format!("{wmsg}\n"))?;
                }
                if !run.intents.is_empty() {
                    return Err(CoreError::Missing("Edit: file and pipe commands are not supported here yet".into()));
                }
            }
            "New" => {
                let col = match self.column_ctx(ctx) {
                    Ok(c) => c,
                    Err(_) if self.state.layout.cols.is_empty() => self.new_column(log, None)?,
                    Err(e) => return Err(e),
                };
                let body = self.create_buffer(log, "", "", None)?;
                let w = self.make_window(log, win, col, body)?;
                self.seltext = Some(ViewId::Body(w));
            }
            "Newcol" => {
                // acme's newcol: a column with one empty window in it
                let col = self.new_column(log, None)?;
                let body = self.create_buffer(log, "", "", None)?;
                self.open_window(log, col, body)?;
            }
            "Delcol" => {
                // acme's delcol: colclean warns for each dirty window and
                // refuses; the second Delcol goes through
                let col = self.column_ctx(ctx)?;
                if !self.colclean(log, col)? {
                    return Ok(false);
                }
                let wins: Vec<WindowId> = self.state.layout.column(col).map(|c| c.wins.iter().map(|s| s.window).collect()).unwrap_or_default();
                for w in wins {
                    self.delete_window(log, w)?;
                }
                self.delete_column(log, col)?;
            }
            "Del" | "Delete" => {
                // acme's del: Delete forces; another view on the file, or a
                // clean window, goes at once; a dirty one warns first
                let w = win.ok_or_else(|| CoreError::Missing("Del needs a window".into()))?;
                if cmd == "Delete" || !self.window_last_view(w) || self.winclean(log, w, false)? {
                    self.delete_window(log, w)?;
                }
            }
            "Tab" => {
                let w = win.ok_or_else(|| CoreError::Missing("Tab needs a window".into()))?;
                match arg.and_then(|a| a.parse::<u32>().ok()) {
                    Some(n) if n > 0 => {
                        self.append(log, Shard::Window(w), Op::Window(WindowOp::Tab { n }))?;
                    }
                    _ => {
                        let (name, tab) = (self.window_name(w), self.state.window(w)?.tabstop);
                        let dir = self.error_dir(Some(w));
                        self.errors(log, dir.as_deref(), &format!("{name}: Tab {tab}\n"))?;
                    }
                }
            }
            "Indent" => {
                let w = win.ok_or_else(|| CoreError::Missing("Indent needs a window".into()))?;
                let on = match arg {
                    Some("on") | Some("ON") => true,
                    Some("off") | Some("OFF") => false,
                    _ => return Err(CoreError::Missing("Indent on|off".into())),
                };
                self.append(log, Shard::Window(w), Op::Window(WindowOp::Indent { on }))?;
            }
            "ID" => {
                let w = win.ok_or_else(|| CoreError::Missing("ID needs a window".into()))?;
                let dir = self.error_dir(Some(w));
                self.errors(log, dir.as_deref(), &format!("{}\n", w.0))?;
            }
            "Zerox" => {
                let w = win.ok_or_else(|| CoreError::Missing("Zerox needs a window".into()))?;
                let name = self.window_name(w);
                if name.ends_with('/') {
                    let dir = self.error_dir(Some(w));
                    self.errors(log, dir.as_deref(), &format!("{name} is a directory; Zerox illegal\n"))?;
                } else {
                    self.zerox(log, w)?;
                }
            }
            "Web" => {
                // a web window on the URL given, else the selected text: a
                // file:// URL or a path is the host's file (apexfile://)
                let arg = text.trim().strip_prefix("Web").map(str::trim).unwrap_or("").to_string();
                let target = if !arg.is_empty() { arg } else { self.seltext.and_then(|v| self.selected_text(v).ok()).unwrap_or_default().trim().to_string() };
                if target.is_empty() {
                    return Err(CoreError::Missing("Web needs a URL or a file, given or selected".into()));
                }
                let dir = win.map(|w| self.window_name(w)).and_then(|n| std::path::Path::new(&n).parent().map(|d| d.display().to_string())).unwrap_or_default();
                let url = web_url(&target, &dir);
                let col = win.and_then(|w| self.column_of(w).ok()).or_else(|| self.state.layout.cols.first().map(|c| c.id)).ok_or_else(|| CoreError::Missing("no column".into()))?;
                let w = self.open_web_window(log, col, &url)?;
                self.seltext = Some(ViewId::Body(w));
            }
            "Send" => {
                // acme's sendx on a text window: the selection, else the
                // snarf buffer, appended to the body with a newline
                let w = win.ok_or_else(|| CoreError::Missing("Send needs a window".into()))?;
                let v = ViewId::Body(w);
                let mut text = self.selected_text(v).unwrap_or_default();
                if text.is_empty() {
                    text = self.state.layout.snarf.clone();
                }
                if text.is_empty() {
                    return Ok(false);
                }
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                let b = self.view_buffer(v)?;
                let end = self.state.buffer(b)?.text.len();
                self.select(log, v, end, end)?;
                self.replace_selection(log, v, &text)?;
                let end = self.state.buffer(b)?.text.len();
                self.select(log, v, end, end)?;
            }
            "Font" => {
                let w = win.ok_or_else(|| CoreError::Missing("Font needs a window".into()))?;
                let mono = !self.state.window(w)?.mono;
                self.append(log, Shard::Window(w), Op::Window(WindowOp::Font { mono }))?;
            }
            "Sort" => {
                let col = self.column_ctx(ctx)?;
                self.sort_column(log, col)?;
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

/// What `Web` opens for `target`, typed or selected in a window whose
/// directory is `dir`: a URL as it is, a `file://` URL as the host's
/// file (`apexfile://`), a path likewise, relative ones from `dir`.
pub fn web_url(target: &str, dir: &str) -> String {
    if let Some(rest) = target.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        return format!("apexfile://{path}");
    }
    if crate::is_url(target) {
        return target.to_string();
    }
    if target.starts_with('/') {
        return format!("apexfile://{target}");
    }
    let dir = dir.trim_end_matches('/');
    format!("apexfile://{dir}/{target}")
}
