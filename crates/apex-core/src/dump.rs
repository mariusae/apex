//! A session written down, to be made again (DESIGN.md §12): acme's
//! `Dump`, kept up by the daemon as the session changes and read back
//! when it starts, so a session outlives its server -- a restart, a
//! crash, a reboot.
//!
//! A dump is not a snapshot of the state. It says what the windows are
//! -- a file at a path, a terminal in a directory, a page at an
//! address -- and what of them is nowhere else: the text not yet put,
//! the scratch windows', a terminal's output. Loading it makes each
//! again with the ordinary ops, under new ids, and lays the tiling out
//! as it was in one step, so a dump written by one build reads in the
//! next. What belonged to the server's process goes with it: the
//! attachments and their leases, the running commands, undo.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::entry::*;
use crate::ids::*;
use crate::log::Log;
use crate::node::{CoreError, Node, COL_TAG, TOP_TAG};
use crate::state::{Column, Cover, Full, Stashed, State};
use crate::text::Text;
use crate::tiling::{self, Rect};

/// The dump format this build writes; a dump of a later one is refused.
pub const VERSION: u32 = 1;

/// A session as `dump` writes it.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Dump {
    pub version: u32,
    /// The session's identity and label: the same session again, so a
    /// UI's tab on it finds it.
    pub id: String,
    pub label: String,
    /// The session's directory, and its settings (`apex set`).
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// When it was written, in seconds since the epoch.
    #[serde(default)]
    pub saved: u64,
    /// The top row's tag.
    #[serde(default)]
    pub top: String,
    #[serde(default)]
    pub columns: Vec<DumpColumn>,
    #[serde(default)]
    pub buffers: Vec<DumpBuffer>,
    #[serde(default)]
    pub windows: Vec<DumpWindow>,
    /// The tiling as it was, under the ids above: laid out again as it
    /// is when it reads, else each window placed anew in its column.
    #[serde(default)]
    pub layout: Option<serde_json::Value>,
}

/// A column: its tag, by its id in `layout`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct DumpColumn {
    pub id: u64,
    pub tag: String,
}

/// A buffer a window shows. Its text is written when it is nowhere
/// else: a file or directory not changed since it was read is read
/// again instead.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct DumpBuffer {
    pub id: u64,
    pub name: String,
    pub kind: WinKind,
    pub scratch: bool,
    #[serde(default)]
    pub text: Option<String>,
    /// The file as it was when last read or put: a file changed under
    /// unsaved text since is stale.
    #[serde(default)]
    pub disk_hash: Option<String>,
}

/// A window: its tag, what it shows, and how.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct DumpWindow {
    pub id: u64,
    pub tag: String,
    pub body: DumpBody,
    /// A terminal's directory, a page's address.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub mono: bool,
    #[serde(default = "tabstop")]
    pub tabstop: u32,
    #[serde(default = "yes")]
    pub autoindent: bool,
    #[serde(default)]
    pub tagexpand: bool,
    #[serde(default)]
    pub diagnostic: bool,
    #[serde(default)]
    pub via: Via,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub scroll: Option<Scroll>,
    /// The body's selection and the line its view starts at.
    #[serde(default)]
    pub sel: (usize, usize),
    #[serde(default)]
    pub origin: usize,
}

fn tabstop() -> u32 {
    4
}

fn yes() -> bool {
    true
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DumpBody {
    Text { buffer: u64 },
    /// A terminal: what it showed, history and screen, as text.
    Term { history: String },
    /// A page: of a buffer, or (none) at its address.
    Page { buffer: Option<u64> },
}

/// The tiling's parts `LayoutOp::Arrange` lays out.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
struct DumpLayout {
    r: Rect,
    cols: Vec<Column>,
    #[serde(default)]
    full: Option<ColumnId>,
    #[serde(default)]
    stash: Vec<Stashed>,
    #[serde(default)]
    covers: Vec<Cover>,
}

/// `state` written down; `history` gives a terminal's output as text.
/// The session's identity, settings and directory are the caller's to
/// fill in.
pub fn dump(state: &State, history: &mut dyn FnMut(TermId) -> Option<String>) -> Dump {
    let text = |b: BufferId| state.buffers.get(&b).map(|b| b.text.to_string()).unwrap_or_default();
    let mut d = Dump { version: VERSION, ..Default::default() };
    d.top = state.layout.top.map(text).unwrap_or_default();
    d.columns = state.layout.cols.iter().map(|c| DumpColumn { id: c.id.0, tag: text(c.tag) }).collect();
    let mut shown = std::collections::BTreeSet::new();
    for w in state.windows.values() {
        let body = match w.body {
            Body::Text(b) => {
                shown.insert(b);
                DumpBody::Text { buffer: b.0 }
            }
            Body::Page(Source::Buffer(b)) => {
                shown.insert(b);
                DumpBody::Page { buffer: Some(b.0) }
            }
            Body::Page(Source::Url) => DumpBody::Page { buffer: None },
            Body::Term(t) => DumpBody::Term { history: history(t).unwrap_or_default() },
        };
        let view = match w.body {
            Body::Text(b) => state.buffers.get(&b).and_then(|b| b.views.get(&ViewId::Body(w.id))).cloned().unwrap_or_default(),
            _ => Default::default(),
        };
        d.windows.push(DumpWindow {
            id: w.id.0,
            tag: text(w.tag),
            body,
            path: w.path.clone(),
            label: w.label.clone(),
            mono: w.mono,
            tabstop: w.tabstop,
            autoindent: w.autoindent,
            tagexpand: w.tagexpand,
            diagnostic: w.diagnostic,
            via: w.via.clone(),
            base: w.base.clone(),
            scroll: w.scroll.clone(),
            sel: (view.q0, view.q1),
            origin: view.origin,
        });
    }
    for b in shown.iter().filter_map(|b| state.buffers.get(b)) {
        // what is on disk as it was read is read again (a buffer never
        // read nor put has no file to be read from)
        let on_disk = matches!(b.kind, WinKind::File | WinKind::Dir) && !b.scratch && b.name.starts_with('/') && b.disk_hash.is_some() && !b.dirty();
        d.buffers.push(DumpBuffer { id: b.id.0, name: b.name.clone(), kind: b.kind, scratch: b.scratch, text: (!on_disk).then(|| b.text.to_string()), disk_hash: b.disk_hash.clone() });
    }
    let l = &state.layout;
    d.layout = serde_json::to_value(DumpLayout { r: l.r, cols: l.cols.clone(), full: l.full, stash: l.stash.clone(), covers: l.covers.clone() }).ok();
    d
}

/// What loading a dump needs of the host: the files, and terminals.
pub trait Host {
    /// A file or directory as it is now: its name as shown (a
    /// directory's with its slash) and its text; none when it is gone.
    fn read(&mut self, name: &str) -> Option<(String, String)>;
    /// A terminal in `dir` for a window made again, `history` written
    /// into it before its shell starts.
    fn term(&mut self, log: &mut Log, dir: &str, history: &str) -> Option<TermId>;
}

impl Node {
    /// Make a fresh session's (one with nothing laid out) windows as `d`
    /// says, under new ids, and lay them out as they were. A window that
    /// cannot be made again (a terminal whose shell will not start) is
    /// left out.
    pub fn load_dump(&mut self, log: &mut Log, d: &Dump, host: &mut dyn Host) -> Result<(), CoreError> {
        if d.version > VERSION {
            return Err(CoreError::Missing(format!("a dump of version {} (this apex reads {VERSION})", d.version)));
        }
        let saved: Option<DumpLayout> = d.layout.clone().and_then(|v| serde_json::from_value(v).ok());
        // the top row
        let top = self.create_buffer(log, "", if d.top.is_empty() { TOP_TAG } else { &d.top }, None)?;
        self.append(log, Shard::Buffer(top), Op::Buffer(BufferOp::ViewAdd { view: ViewId::Top }))?;
        self.create_shard(log, Shard::Layout)?;
        let r = saved.as_ref().map_or(Rect::new(0, 0, 1100, 700), |l| l.r);
        self.append(log, Shard::Layout, Op::Layout(LayoutOp::Init { top, r }))?;
        // the columns' tags, by the columns' ids then and now
        let mut cols: BTreeMap<u64, (ColumnId, BufferId)> = BTreeMap::new();
        for c in &d.columns {
            let id = ColumnId(self.alloc());
            let tag = self.create_buffer(log, "", if c.tag.is_empty() { COL_TAG } else { &c.tag }, None)?;
            self.append(log, Shard::Buffer(tag), Op::Buffer(BufferOp::ViewAdd { view: ViewId::ColTag(id) }))?;
            cols.insert(c.id, (id, tag));
        }
        // the buffers: read again, or as written
        let mut bufs: BTreeMap<u64, BufferId> = BTreeMap::new();
        for b in &d.buffers {
            let id = match &b.text {
                None => {
                    let (name, text) = host.read(&b.name).unwrap_or_else(|| (b.name.clone(), String::new()));
                    let hash = Text::new(&text).content_hash();
                    self.create_buffer_as(log, &name, &text, Some(hash), b.kind, b.scratch)?
                }
                Some(text) if matches!(b.kind, WinKind::File | WinKind::Dir) && !b.scratch && b.name.starts_with('/') => {
                    // unsaved: the file as it is now, the text over it,
                    // so it is dirty against the file, and stale if the
                    // file moved on from the one it was read from
                    let disk = host.read(&b.name).map(|(_, t)| t);
                    let hash = disk.as_deref().map(|t| Text::new(t).content_hash());
                    let id = self.create_buffer_as(log, &b.name, disk.as_deref().unwrap_or(""), hash.clone(), b.kind, b.scratch)?;
                    if disk.as_deref() != Some(text.as_str()) {
                        self.set_content(log, id, text)?;
                    }
                    if let Some(now) = hash.filter(|h| b.disk_hash.as_ref().is_some_and(|was| was != h)) {
                        self.append(log, Shard::Buffer(id), Op::Buffer(BufferOp::Stale { disk_hash: now }))?;
                    }
                    id
                }
                Some(text) => self.create_buffer_as(log, &b.name, text, b.disk_hash.clone(), b.kind, b.scratch)?,
            };
            bufs.insert(b.id, id);
        }
        // the windows, nowhere yet
        let mut wins: BTreeMap<u64, WindowId> = BTreeMap::new();
        for w in &d.windows {
            let body = match &w.body {
                DumpBody::Text { buffer } => bufs.get(buffer).map(|&b| Body::Text(b)),
                DumpBody::Page { buffer: Some(buffer) } => bufs.get(buffer).map(|&b| Body::Page(Source::Buffer(b))),
                DumpBody::Page { buffer: None } => Some(Body::Page(Source::Url)),
                DumpBody::Term { history } => host.term(log, &w.path, history).map(Body::Term),
            };
            let Some(body) = body else { continue };
            let id = self.make_window_shards(log, body, w.path.clone(), w.label.clone(), w.diagnostic, w.via.clone(), w.base.clone())?;
            let sh = Shard::Window(id);
            let x = self.state.window(id)?.clone();
            if w.mono != x.mono {
                self.append(log, sh, Op::Window(WindowOp::Font { mono: w.mono }))?;
            }
            if w.tabstop != x.tabstop {
                self.append(log, sh, Op::Window(WindowOp::Tab { n: w.tabstop }))?;
            }
            if w.autoindent != x.autoindent {
                self.append(log, sh, Op::Window(WindowOp::Indent { on: w.autoindent }))?;
            }
            if w.tagexpand != x.tagexpand {
                self.append(log, sh, Op::Window(WindowOp::TagExpand { on: w.tagexpand }))?;
            }
            if w.scroll.is_some() {
                self.append(log, sh, Op::Window(WindowOp::PageScroll { scroll: w.scroll.clone() }))?;
            }
            if !w.tag.is_empty() && self.state.buffer(x.tag)?.text.to_string() != w.tag {
                self.set_content(log, x.tag, &w.tag)?;
            }
            if let Body::Text(b) = body {
                let len = self.state.buffer(b)?.text.len();
                let (q0, q1) = (w.sel.0.min(len), w.sel.1.min(len));
                self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Select { view: ViewId::Body(id), q0, q1: q1.max(q0) }))?;
                self.append(log, Shard::Buffer(b), Op::Buffer(BufferOp::Origin { view: ViewId::Body(id), origin: w.origin.min(len) }))?;
            }
            wins.insert(w.id, id);
        }
        // laid out as they were, or failing that, each placed in its
        // column (the last, for one whose column is not known)
        let mut l = self.state.layout.clone();
        match saved.and_then(|s| remap(s, &cols, &wins)) {
            Some(s) => {
                l.r = s.r;
                l.cols = s.cols;
                l.full = s.full;
                l.stash = s.stash;
                l.covers = s.covers;
                self.arrange(log, &l)?;
                // a window the tiling did not have a place for (its
                // column gone, the one over it gone) is placed anew
                let last = self.state.layout.cols.last().map(|c| c.id);
                for &w in wins.values() {
                    let l = &self.state.layout;
                    let placed = l.place_of(w).is_some() || l.is_stashed(w) || l.covers.iter().any(|c| c.under == w);
                    if let (false, Some(col)) = (placed, last) {
                        self.place(log, col, w, None)?;
                    }
                }
            }
            None => {
                for &(id, tag) in cols.values() {
                    let mut l = self.state.layout.clone();
                    tiling::rowadd(&mut l, tiling::AddingCol::New { id, tag }, None, &*self.tiling);
                    self.arrange(log, &l)?;
                }
                if self.state.layout.cols.is_empty() {
                    self.new_column(log, None)?;
                }
                let last = self.state.layout.cols.last().map(|c| c.id).expect("a column");
                for &w in wins.values() {
                    self.place(log, last, w, None)?;
                }
            }
        }
        Ok(())
    }
}

/// `l`'s ids then as they are now; none when it names a column that was
/// not written down. Windows not made again leave it.
fn remap(mut l: DumpLayout, cols: &BTreeMap<u64, (ColumnId, BufferId)>, wins: &BTreeMap<u64, WindowId>) -> Option<DumpLayout> {
    let col = |c: ColumnId| cols.get(&c.0).map(|x| x.0);
    let win = |w: WindowId| wins.get(&w.0).copied();
    for c in &mut l.cols {
        let (id, tag) = *cols.get(&c.id.0)?;
        c.id = id;
        c.tag = tag;
        c.wins.retain(|s| win(s.window).is_some());
        for s in &mut c.wins {
            s.window = win(s.window)?;
        }
        c.full = c.full.and_then(|f| win(f.window).map(|window| Full { window, ..f })).filter(|f| c.wins.iter().any(|s| s.window == f.window));
        c.after = c.after.and_then(col);
    }
    l.full = l.full.and_then(col);
    l.stash = l
        .stash
        .into_iter()
        .filter_map(|mut s| {
            s.slot.window = win(s.slot.window)?;
            s.col = col(s.col)?;
            s.above = s.above.and_then(win);
            Some(s)
        })
        .collect();
    l.covers = l.covers.into_iter().filter_map(|c| Some(Cover { top: win(c.top)?, under: win(c.under)? })).collect();
    Some(l)
}
