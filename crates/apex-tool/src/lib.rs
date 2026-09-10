//! The Rust API for apex tools: a program that takes part in a session,
//! makes and writes windows, offers verbs in their tools menus, answers
//! them when they are used, and follows what others type.
//!
//! A tool attaches by name and then asks for events:
//!
//! ```no_run
//! use apex_tool::{Event, Rule, Tool};
//!
//! let mut t = Tool::attach("shout")?;
//! let w = t.new_window("/tmp/shout")?;
//! let shout = t.offer(Rule::verb("Shout").window(w))?;
//! while let Some(ev) = t.next_event(None)? {
//!     if let Event::Plumb(p) = ev {
//!         let taken = p.rule == shout && t.append(w, &format!("{}\n", p.text.to_uppercase())).is_ok();
//!         t.answer(&p, taken)?;
//!     }
//! }
//! # Ok::<(), apex_tool::Error>(())
//! ```
//!
//! `next_event` returns `None` when the session is over. A plumb must be
//! answered within a second, or it is taken as refused and the next rule
//! tried. Text offsets count characters, as apex does throughout. The
//! tool finds its session as any command does: `APEX_SOCKET` and
//! `apexsession`, which apex sets for everything it runs.
//!
//! Nothing of the wire protocol or the replicated state shows through;
//! `apex tool bridge` (JSON on stdin and stdout, for tools in other
//! languages) is a client of this crate, and its commands and events
//! are these methods and events one for one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use apex_core::*;
use apex_server::proto::{ClientMsg, ServerMsg};
use apex_server::remote::{Remote, ToolPlumb};
use apex_server::Proposal;

pub use apex_core::entry::WinKind;
pub use apex_core::ids::{RuleId, WindowId};

/// What went wrong, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Error {
        Error(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Error {
        Error(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// As an offset to `replace`: the end of the text.
pub const END: usize = usize::MAX;

/// How long a proposal may take to be answered.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A window as `windows` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub name: String,
    pub kind: WinKind,
    /// A process is behind it (a shell's, a tool's).
    pub live: bool,
}

/// A range of characters in a window's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub q0: usize,
    pub q1: usize,
}

/// A use of one of the tool's rules: the verb run, or text plumbed, in
/// a window. Answer it with `Tool::answer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plumb {
    pub id: u64,
    /// The rule of ours that matched.
    pub rule: RuleId,
    pub verb: String,
    /// The verb's arguments, or the text plumbed.
    pub text: String,
    /// The window's directory, where relative names resolve.
    pub dir: String,
    /// The window it happened in; `None` from the top row.
    pub window: Option<WindowId>,
    /// The rule's text regexp's groups, `$0` first.
    pub groups: Vec<String>,
    /// Where it happened in the window's body: for a verb, the window's
    /// dot (the selection, or the insertion point); for B3, the pointer.
    pub at: Option<Range>,
    /// What B3 took, the text swept or expanded; `None` for a verb.
    pub sel: Option<Range>,
}

impl Plumb {
    /// The text a handler should act on: what B3 took when there is
    /// such a thing, else the window's dot; `None` when neither, or empty.
    pub fn range(&self) -> Option<Range> {
        [self.sel, self.at].into_iter().flatten().find(|r| r.q1 > r.q0)
    }
}

/// A change someone else made to a watched window's body: `nd`
/// characters deleted at `q0` and `text` inserted there, in the text as
/// the tool last saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub window: WindowId,
    pub q0: usize,
    pub nd: usize,
    pub text: String,
}

/// What `next_event` brings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A rule of the tool's matched: answer it.
    Plumb(Plumb),
    /// A watched window's body was edited by someone else.
    Edit(Edit),
    /// A window the tool made, opened or watches was renamed (a shell's
    /// cd, a Put under a new name).
    Renamed { window: WindowId, name: String },
    /// Such a window was deleted.
    Deleted { window: WindowId },
}

/// Where a verb of the tool's is offered. `Rule::verb(..)` or
/// `Rule::plumb()`, then narrowed: each setter is a further condition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    verb: Option<String>,
    text: Option<String>,
    file: Option<String>,
    kind: Option<WinKind>,
    window: Option<WindowId>,
    priority: i32,
}

impl Rule {
    /// A word offered in the tools menu of matching windows and run by
    /// B2 there.
    pub fn verb(verb: &str) -> Rule {
        Rule { verb: Some(verb.to_string()), ..Rule::default() }
    }

    /// B3 (Look) on text the rule matches goes to the tool.
    pub fn plumb() -> Rule {
        Rule::default()
    }

    /// The plumbed text (or the verb's arguments) must match this
    /// regexp whole; its groups arrive in `Plumb::groups`.
    pub fn text(mut self, re: &str) -> Rule {
        self.text = Some(re.to_string());
        self
    }

    /// The window's name must match this regexp.
    pub fn file(mut self, re: &str) -> Rule {
        self.file = Some(re.to_string());
        self
    }

    pub fn kind(mut self, kind: WinKind) -> Rule {
        self.kind = Some(kind);
        self
    }

    /// This one window only, whatever its name (ids are never reused).
    pub fn window(mut self, w: WindowId) -> Rule {
        self.window = Some(w);
        self
    }

    /// Higher goes first among rules that match; 0 is usual.
    pub fn priority(mut self, p: i32) -> Rule {
        self.priority = p;
        self
    }
}

/// The tool's attachment to a session.
pub struct Tool {
    remote: Remote,
    /// Windows whose body edits (by others) are reported.
    watched: BTreeSet<WindowId>,
    /// Windows we made, opened or watch, with their names as last seen:
    /// renames and deletions are reported for these.
    ours: BTreeMap<WindowId, String>,
    events: VecDeque<Event>,
}

impl Tool {
    /// Attach to the session this program was started in, as the tool
    /// called `name` (what rules and `apex ps` know it by).
    pub fn attach(name: &str) -> Result<Tool> {
        let socket = apex_server::daemon::default_socket();
        let session = std::env::var("apexsession").or_else(|_| std::env::var("APEX_SESSION")).unwrap_or_else(|_| apex_server::providers::DEFAULT_SESSION.to_string());
        Tool::attach_to(&socket, &session, name)
    }

    /// Attach to the session `session` of the daemon at `socket`.
    pub fn attach_to(socket: &Path, session: &str, name: &str) -> Result<Tool> {
        let remote = Remote::connect_as(socket, session, name, AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
        // called by our name, not apex, in the top row and ps
        remote.announce(name);
        Ok(Tool { remote, watched: BTreeSet::new(), ours: BTreeMap::new(), events: VecDeque::new() })
    }

    /// The attachment's name.
    pub fn name(&self) -> String {
        self.remote.node.state.meta.attachments.get(&self.remote.attachment()).map(|a| a.name.clone()).unwrap_or_default()
    }

    /// The daemon's socket, for a tool starting others.
    pub fn socket(&self) -> PathBuf {
        apex_server::daemon::default_socket()
    }

    // ---- events ----------------------------------------------------------

    /// The next thing that happened, waiting up to `timeout` (`None`:
    /// as long as it takes). `Ok(None)` once the session is over.
    pub fn next_event(&mut self, timeout: Option<Duration>) -> Result<Option<Event>> {
        let deadline = timeout.map(|t| std::time::Instant::now() + t);
        loop {
            // whatever the link has already brought, even with no time to wait
            let alive = self.drain();
            if let Some(ev) = self.events.pop_front() {
                return Ok(Some(ev));
            }
            if !alive {
                return Ok(None);
            }
            let wait = match deadline {
                Some(d) => {
                    let left = d.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    left.min(Duration::from_millis(100))
                }
                None => Duration::from_millis(100),
            };
            if !self.step(wait) {
                return Ok(None);
            }
        }
    }

    /// Every message the link holds, without waiting; false when the
    /// link ended.
    fn drain(&mut self) -> bool {
        loop {
            match self.remote.link.rx.try_recv() {
                Ok(m) => {
                    self.before(&m);
                    if !self.remote.handle(m) {
                        return false;
                    }
                    self.after();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return true,
                Err(_) => return false,
            }
        }
    }

    /// One message from the link, or a moment's wait; false when the
    /// link ended.
    fn step(&mut self, timeout: Duration) -> bool {
        match self.remote.link.rx.recv_timeout(timeout) {
            Ok(m) => {
                self.before(&m);
                let alive = self.remote.handle(m);
                self.after();
                alive
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.after();
                true
            }
            Err(_) => false,
        }
    }

    /// Edits by others to a watched window's body, seen before they land
    /// in the replica.
    fn before(&mut self, m: &ServerMsg) {
        let ServerMsg::Entries { shard, entries } = m else { return };
        let Shard::Buffer(b) = shard else { return };
        let me = self.remote.attachment();
        let Some(w) = self.watched.iter().copied().find(|w| self.remote.node.state.window(*w).ok().and_then(|x| x.body_buffer()) == Some(*b)) else { return };
        for e in entries {
            if e.attachment == me {
                continue;
            }
            if let Op::Buffer(BufferOp::Edit { q0, nd, text, .. }) = &e.op {
                self.events.push_back(Event::Edit(Edit { window: w, q0: *q0, nd: *nd, text: text.clone() }));
            }
        }
    }

    /// After messages: plumbs for our rules, and our windows renamed or
    /// gone.
    fn after(&mut self) {
        let plumbs: Vec<ToolPlumb> = std::mem::take(&mut self.remote.link.plumbs);
        for p in plumbs {
            let window = match p.ctx {
                ExecCtx::Window(w) => Some(w),
                _ => None,
            };
            let range = |s: Option<Span>| s.map(|s| Range { q0: s.q0, q1: s.q1 });
            self.events.push_back(Event::Plumb(Plumb { id: p.id, rule: p.rule, verb: p.verb, text: p.text, dir: p.dir, window, groups: p.groups, at: range(p.at), sel: range(p.sel) }));
        }
        let mut gone = Vec::new();
        for (w, name) in self.ours.iter_mut() {
            match self.remote.node.state.window(*w) {
                Ok(_) => {
                    let now = self.remote.node.window_name(*w);
                    if now != *name {
                        *name = now.clone();
                        self.events.push_back(Event::Renamed { window: *w, name: now });
                    }
                }
                Err(_) => gone.push(*w),
            }
        }
        for w in gone {
            self.ours.remove(&w);
            self.watched.remove(&w);
            self.events.push_back(Event::Deleted { window: w });
        }
    }

    /// Answer a plumb: taken, or refused (the next rule is tried).
    pub fn answer(&mut self, p: &Plumb, taken: bool) -> Result<()> {
        self.remote.plumb_ack(p.id, taken);
        Ok(())
    }

    /// Propose and wait for the answer, every message on the way seen
    /// for events.
    fn propose(&mut self, p: Proposal) -> Result<Option<WindowId>> {
        let id = self.remote.link.propose(p);
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            if let Some(r) = self.remote.link.applied.remove(&id) {
                return r.map_err(Error);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("timed out waiting for the session".into());
            }
            if !self.step(left.min(Duration::from_millis(50))) {
                return Err("the session is gone".into());
            }
        }
    }

    // ---- windows ---------------------------------------------------------

    /// The session's windows.
    pub fn windows(&self) -> Vec<WindowInfo> {
        let node = &self.remote.node;
        let mut out: Vec<WindowInfo> = node.state.windows.keys().copied().map(|w| WindowInfo { id: w, name: node.window_name(w), kind: node.window_kind(w), live: node.window_live(w) }).collect();
        out.sort_by_key(|w| w.id);
        out
    }

    /// A window's name, if it exists.
    pub fn window_name(&self, w: WindowId) -> Option<String> {
        self.remote.node.state.window(w).ok().map(|_| self.remote.node.window_name(w))
    }

    fn body_of(&self, w: WindowId) -> Result<BufferId> {
        self.remote.node.state.window(w).map_err(|_| Error(format!("no window {}", w.0)))?.body_buffer().ok_or_else(|| "not a text window".into())
    }

    /// A new, empty window with this name in the last column.
    pub fn new_window(&mut self, name: &str) -> Result<WindowId> {
        let col = self.remote.node.state.layout.cols.last().map(|c| c.id).ok_or("no column")?;
        let w = self.propose(Proposal::NewWindow { col, name: name.to_string() })?.ok_or("no window made")?;
        self.remember(w);
        Ok(w)
    }

    /// The file (or directory) of this name shown, opened if it is not
    /// open, at `line` (1-based) when given.
    pub fn open(&mut self, name: &str, line: Option<usize>) -> Result<WindowId> {
        let pos = line.map(Pos::Line).unwrap_or(Pos::Keep);
        self.propose(Proposal::Goto { loc: Loc { session: None, name: name.to_string(), pos } })?;
        // the file may be on its way: wait for its window
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            if let Some(w) = self.remote.node.state.windows.keys().copied().find(|w| self.remote.node.window_name(*w) == name) {
                self.remember(w);
                return Ok(w);
            }
            if std::time::Instant::now() > deadline {
                return Err(format!("{name}: not opened").into());
            }
            if !self.step(Duration::from_millis(50)) {
                return Err("the session is gone".into());
            }
        }
    }

    /// Show another session, at a window there when given: a UI leading
    /// this session switches to it. The session is named by its id, a
    /// unique prefix of it, or its label; the window by its id there.
    pub fn switch(&mut self, session: &str, window: Option<WindowId>) -> Result<()> {
        self.propose(Proposal::Switch { session: session.to_string(), window })?;
        Ok(())
    }

    /// The window's whole text.
    pub fn read(&self, w: WindowId) -> Result<String> {
        let b = self.body_of(w)?;
        Ok(self.remote.node.state.buffer(b).map_err(|e| e.to_string())?.text.to_string())
    }

    /// The window's selection (dot).
    pub fn selection(&self, w: WindowId) -> Result<Range> {
        let (q0, q1) = self.remote.node.selection(ViewId::Body(w)).map_err(|e| e.to_string())?;
        Ok(Range { q0, q1 })
    }

    /// Replace the characters `[q0, q1)` with `text`; `END` for either
    /// means the end of the text. Dot is left where the edit leaves it;
    /// nothing is selected on the tool's behalf.
    pub fn replace(&mut self, w: WindowId, q0: usize, q1: usize, text: &str) -> Result<()> {
        let b = self.body_of(w)?;
        let buf = self.remote.node.state.buffer(b).map_err(|e| e.to_string())?;
        let (len, version) = (buf.text.len(), buf.version);
        let (q0, q1) = (q0.min(len), q1.min(len));
        if q1 < q0 {
            return Err("q1 before q0".into());
        }
        self.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0, q1, text: text.to_string() })?;
        Ok(())
    }

    /// Add text at the end.
    pub fn append(&mut self, w: WindowId, text: &str) -> Result<()> {
        self.replace(w, END, END, text)
    }

    pub fn select(&mut self, w: WindowId, q0: usize, q1: usize) -> Result<()> {
        self.body_of(w)?;
        self.propose(Proposal::Select { view: ViewId::Body(w), q0, q1 })?;
        Ok(())
    }

    /// Bring the text at `at` into view: the window is scrolled only if
    /// `at` is off screen (and grown if it shows no lines). Nothing else
    /// moves: not dot, not the mouse, not the back stack. What a tool
    /// keeping a line it changed in sight wants, where `open` would jump
    /// the user there. acme's `show` after `addr=`.
    pub fn show(&mut self, w: WindowId, at: usize) -> Result<()> {
        self.body_of(w)?;
        self.propose(Proposal::Show { view: ViewId::Body(w), at })?;
        Ok(())
    }

    /// `show` at the start of line `n` (1-based).
    pub fn show_line(&mut self, w: WindowId, n: usize) -> Result<()> {
        let at = self.line_start(w, n)?;
        self.show(w, at)
    }

    /// The characters `[q0, q1)` of line `n` (1-based), the newline
    /// excluded; an error past the last line.
    pub fn line(&self, w: WindowId, n: usize) -> Result<Range> {
        let b = self.body_of(w)?;
        let text = &self.remote.node.state.buffer(b).map_err(|e| e.to_string())?.text;
        let (q0, q1) = text.line_range(n.saturating_sub(1)).ok_or_else(|| format!("no line {n}"))?;
        Ok(Range { q0, q1 })
    }

    fn line_start(&self, w: WindowId, n: usize) -> Result<usize> {
        Ok(self.line(w, n)?.q0)
    }

    /// Give the window (its buffer) a new name.
    pub fn rename(&mut self, w: WindowId, name: &str) -> Result<()> {
        let b = self.body_of(w)?;
        self.propose(Proposal::Rename { buffer: b, window: w, name: name.to_string() })?;
        self.ours.entry(w).and_modify(|n| *n = name.to_string());
        Ok(())
    }

    /// Mark the window as having this tool behind it: its handle shows
    /// so, and Del does not ask about unsaved text. The mark goes when
    /// the tool detaches.
    pub fn set_live(&mut self, w: WindowId, on: bool) -> Result<()> {
        let by = on.then_some(self.remote.attachment());
        self.propose(Proposal::Live { window: w, by })?;
        Ok(())
    }

    /// Close the window (Del there).
    pub fn delete(&mut self, w: WindowId) -> Result<()> {
        self.exec_in(Some(w), "Del")
    }

    /// Run `text` as a command, as B2 on it would: in a window, or in
    /// the top row.
    pub fn exec_in(&mut self, w: Option<WindowId>, text: &str) -> Result<()> {
        let ctx = match w {
            Some(w) => {
                self.remote.node.state.window(w).map_err(|_| Error(format!("no window {}", w.0)))?;
                ExecCtx::Window(w)
            }
            None => ExecCtx::Top,
        };
        self.propose(Proposal::Exec { ctx, text: text.to_string() })?;
        Ok(())
    }

    /// Run `text` as a command in the top row.
    pub fn exec(&mut self, text: &str) -> Result<()> {
        self.exec_in(None, text)
    }

    /// A note in `+Errors` (of `dir`, or the session's), where tools
    /// say things.
    pub fn errors(&mut self, dir: Option<&str>, text: &str) -> Result<()> {
        self.propose(Proposal::Errors { dir: dir.map(String::from), text: text.to_string() })?;
        Ok(())
    }

    /// Report edits by others to the window's body as `Event::Edit`.
    pub fn watch(&mut self, w: WindowId) -> Result<()> {
        self.body_of(w)?;
        self.watched.insert(w);
        self.remember(w);
        Ok(())
    }

    pub fn unwatch(&mut self, w: WindowId) {
        self.watched.remove(&w);
    }

    fn remember(&mut self, w: WindowId) {
        let name = self.remote.node.window_name(w);
        self.ours.insert(w, name);
    }

    // ---- rules ------------------------------------------------------------

    /// Install a rule answered by this tool: its `Plumb` events say
    /// which rule matched.
    pub fn offer(&mut self, r: Rule) -> Result<RuleId> {
        let rule = PlumbRule {
            verb: r.verb.unwrap_or_else(|| "plumb".to_string()),
            text: r.text,
            file: r.file,
            kind: r.kind,
            win: r.window,
            isfile: None,
            isdir: None,
            action: RuleAction::Tool(self.name()),
            to: None,
        };
        rule.check()?;
        self.remote.rule_add(rule, r.priority, true, TIMEOUT).map_err(Error)
    }

    /// Remove a rule.
    pub fn withdraw(&mut self, id: RuleId) {
        self.remote.send(&ClientMsg::RuleRm { id });
    }

    // ---- settings ---------------------------------------------------------

    /// Record a setting of the tool's own (gone when it detaches).
    pub fn set(&mut self, key: &str, value: &str) {
        self.remote.send(&ClientMsg::Set { key: key.to_string(), value: value.to_string(), attachment: None });
    }

    /// A setting: the tool's own, else the session's.
    pub fn setting(&self, key: &str) -> Option<String> {
        self.remote.node.state.meta.setting(self.remote.attachment(), key).map(String::from)
    }
}
