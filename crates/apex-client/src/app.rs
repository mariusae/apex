//! The apex client: acme's interaction model (three-button mouse, chords,
//! keyboard to the text under the pointer) over the core's state. Every
//! change goes through the leader node as log entries. The server either
//! runs in-process and shares the log, or sits behind a socket: the
//! client's code path is the same, only the [`Backend`] differs.

use std::collections::{HashMap, HashSet};

use gpui::{
    px, ClipboardItem, Context, FocusHandle, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Window,
};

use apex_core::*;
use apex_server::proto::ClientMsg;
use apex_server::remote::{Link, Wake};
use apex_server::{perform, Server, ServerEvent, TermKey};

use crate::term_element::TermLayout;
use crate::text_element::{Source, TextLayout};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Top,
    ColTag,
    WinTag,
    Body,
}

impl Kind {
    pub fn of(v: ViewId) -> Kind {
        match v {
            ViewId::Top => Kind::Top,
            ViewId::ColTag(_) => Kind::ColTag,
            ViewId::Tag(_) => Kind::WinTag,
            ViewId::Body(_) => Kind::Body,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HlKind {
    Exec,
    Look,
}

#[derive(Clone, Copy)]
struct Drag {
    view: ViewId,
    anchor: usize,
}

#[derive(Default)]
struct Mouse {
    b1: Option<Drag>,
    b2: Option<Drag>,
    b3: Option<Drag>,
    chorded: bool,
    box_drag: Option<(WindowId, Point<Pixels>)>,
    left_as: Option<MouseButton>,
    mods: Modifiers,
}

enum Region {
    Text(usize),
    Scrollbar,
    LayoutBox,
    Term(usize, usize),
    TermScrollbar,
}

#[derive(Clone, Copy)]
enum Target {
    View(ViewId),
    Term(WindowId, TermId),
}

/// How to reach a daemon.
#[derive(Clone, Debug)]
pub enum Where {
    Socket(std::path::PathBuf),
    /// A command whose stdin/stdout carry the frames (ssh to a bridge).
    Via(String),
}

/// Where the server is.
pub enum Backend {
    /// In this process, sharing the log.
    Local(Server),
    /// Behind a socket; the log is a mirror.
    Remote(Link),
}

pub struct Acme {
    pub log: Log,
    pub node: Node,
    pub backend: Backend,
    pub focus: FocusHandle,
    pub layouts: HashMap<ViewId, TextLayout>,
    pub term_layouts: HashMap<WindowId, TermLayout>,
    pub hl: Option<(ViewId, usize, usize, HlKind)>,
    mouse: Mouse,
    want_visible: HashSet<ViewId>,
    typed_start: HashMap<ViewId, usize>,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn is_file_char(c: char) -> bool {
    c.is_alphanumeric() || ".-+/:@_~$#".contains(c)
}
fn is_exec_char(c: char) -> bool {
    is_file_char(c) || "<|>".contains(c)
}

/// Expand outward from `i` over runes satisfying `pred`.
fn expand(t: &Text, i: usize, pred: impl Fn(char) -> bool) -> (usize, usize) {
    let n = t.len();
    let i = i.min(n);
    let mut a = i;
    while a > 0 && pred(t.char_at(a - 1)) {
        a -= 1;
    }
    let mut b = i;
    while b < n && pred(t.char_at(b)) {
        b += 1;
    }
    (a, b)
}

fn closing(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '<' => Some('>'),
        '"' | '\'' | '`' => Some(c),
        _ => None,
    }
}
fn opening(c: char) -> Option<char> {
    match c {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        '>' => Some('<'),
        '"' | '\'' | '`' => Some(c),
        _ => None,
    }
}

/// acme's double-click: bracketed text next to a bracket or quote, the
/// whole line at a line boundary, otherwise the word.
fn double_click(t: &Text, i: usize) -> (usize, usize) {
    let n = t.len();
    let i = i.min(n);
    let before = if i > 0 { Some(t.char_at(i - 1)) } else { None };
    let at = if i < n { Some(t.char_at(i)) } else { None };
    if let Some(c) = before {
        if let Some(close) = closing(c) {
            let mut depth = 1;
            let mut j = i;
            while j < n {
                let ch = t.char_at(j);
                if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return (i, j);
                    }
                } else if ch == c && c != close {
                    depth += 1;
                }
                j += 1;
            }
        }
    }
    if let Some(c) = at {
        if let Some(open) = opening(c) {
            let mut depth = 1;
            let mut j = i;
            while j > 0 {
                let ch = t.char_at(j - 1);
                if ch == open {
                    depth -= 1;
                    if depth == 0 {
                        return (j, i);
                    }
                } else if ch == c && open != c {
                    depth += 1;
                }
                j -= 1;
            }
        }
    }
    let line_start = |p: usize| t.line_start(t.line_of(p));
    let line_end = |p: usize| t.line_range(t.line_of(p)).map(|(_, e)| e).unwrap_or(n);
    if before.is_none_or(|c| c == '\n') && at.is_some_and(|c| c != '\n') {
        let e = line_end(i);
        return (i, e + usize::from(e < n));
    }
    if at.is_none_or(|c| c == '\n') && before.is_some_and(|c| c != '\n') {
        return (line_start(i), i);
    }
    let r = expand(t, i, is_word_char);
    if r.0 == r.1 {
        return (i, (i + 1).min(n));
    }
    r
}

impl Acme {
    /// A session with the server in-process.
    pub fn new(cx: &mut Context<Self>, files: Vec<String>) -> (Acme, futures::channel::mpsc::UnboundedReceiver<ServerEvent>) {
        let mut log = Log::new();
        let (a, _) = log.attach(AttachmentKind::Ui, "apex");
        let mut node = Node::new(a);
        node.catch_up(&log).expect("fresh log");
        let col = node.init_session(&mut log).expect("init session");
        let (server, rx) = Server::new(&log);
        let cwd = server.cwd.clone();
        let names: Vec<&str> = if files.is_empty() { vec!["."] } else { files.iter().map(|s| s.as_str()).collect() };
        for f in names {
            match server.open_file(col, &cwd, f, None) {
                Ok(p) => {
                    perform(&mut node, &mut log, vec![p]);
                }
                Err(e) => {
                    let _ = node.errors(&mut log, col, &format!("{e}\n"));
                }
            }
        }
        (Self::over(cx, log, node, Backend::Local(server)), rx)
    }

    /// Attach to a session behind `socket`. `wake` is called from the
    /// reader thread when there is something to poll.
    pub fn attach(cx: &mut Context<Self>, at: &Where, session: &str, files: Vec<String>, wake: Wake) -> std::io::Result<Acme> {
        let (link, mut log, mut node) = match at {
            Where::Socket(socket) => Link::connect(socket, session, "apex", AttachmentKind::Ui, Some(wake))?,
            Where::Via(cmd) => {
                let mut child = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .spawn()?;
                let stdin = child.stdin.take().expect("piped");
                let stdout = child.stdout.take().expect("piped");
                let closer = Box::new(move || {
                    let _ = child.kill();
                });
                Link::over_streams(Box::new(stdout), Box::new(stdin), Some(closer), session, "apex", AttachmentKind::Ui, Some(wake))?
            }
        };
        let col = match node.state.layout.cols.first() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        let mut acme = Self::over(cx, log, node, Backend::Remote(link));
        for f in files {
            acme.send(ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: f });
        }
        acme.after();
        Ok(acme)
    }

    fn over(cx: &mut Context<Self>, log: Log, node: Node, backend: Backend) -> Acme {
        Acme {
            log,
            node,
            backend,
            focus: cx.focus_handle(),
            layouts: HashMap::new(),
            term_layouts: HashMap::new(),
            hl: None,
            mouse: Mouse::default(),
            want_visible: HashSet::new(),
            typed_start: HashMap::new(),
        }
    }

    /// Bring the client node up to date with the log (the server appends
    /// terminal rows and metalog entries).
    pub fn sync(&mut self) {
        if let Err(e) = self.node.catch_up(&self.log) {
            eprintln!("catch up: {e}");
        }
    }

    /// An event from the in-process server.
    pub fn pump(&mut self, ev: ServerEvent) {
        if let Backend::Local(server) = &mut self.backend {
            let props = server.pump(&mut self.log, &self.node, ev);
            if let Some(w) = perform(&mut self.node, &mut self.log, props) {
                self.show(w);
            }
        }
        self.sync();
    }

    /// Everything the socket has queued from the server. Returns false
    /// when the connection is gone.
    pub fn poll_remote(&mut self) -> bool {
        let Backend::Remote(link) = &mut self.backend else { return true };
        let alive = link.poll(&mut self.node, &mut self.log);
        for w in link.take_made() {
            self.show(w);
        }
        alive
    }

    fn show(&mut self, w: WindowId) {
        self.node.seltext = Some(ViewId::Body(w));
        self.want_visible.insert(ViewId::Body(w));
    }

    fn send(&self, m: ClientMsg) {
        if let Backend::Remote(link) = &self.backend {
            link.send(&m);
        }
    }

    /// After a command: in-process, let the server perform what it was
    /// handed and close terminals whose windows are gone; over a socket,
    /// ship what we sequenced. Then catch up.
    fn after(&mut self) {
        match &mut self.backend {
            Backend::Local(server) => {
                let props = server.poll_execs(&mut self.log, &self.node);
                if let Some(w) = perform(&mut self.node, &mut self.log, props) {
                    self.show(w);
                }
                if let Backend::Local(server) = &mut self.backend {
                    server.close_orphan_terms(&mut self.log, &self.node);
                }
            }
            Backend::Remote(link) => link.flush(&self.log),
        }
        self.sync();
    }

    fn term_key(&mut self, t: TermId, key: TermKey) {
        match &mut self.backend {
            Backend::Local(server) => server.term_key(t, &key),
            Backend::Remote(link) => link.send(&ClientMsg::TermKey { term: t, key }),
        }
    }

    fn term_paste(&mut self, t: TermId, text: String) {
        match &mut self.backend {
            Backend::Local(server) => server.term_paste(t, &text),
            Backend::Remote(link) => link.send(&ClientMsg::TermPaste { term: t, text }),
        }
    }

    fn term_scroll(&mut self, t: TermId, delta: isize) {
        match &mut self.backend {
            Backend::Local(server) => server.term_scroll(&mut self.log, t, delta),
            Backend::Remote(link) => link.send(&ClientMsg::TermScroll { term: t, delta: delta as i64 }),
        }
        self.sync();
    }

    // ---- what the elements read ------------------------------------------

    pub fn source(&mut self, view: ViewId) -> Option<Source> {
        let b = self.node.view_buffer(view).ok()?;
        let buf = self.node.state.buffer(b).ok()?;
        let v = buf.view(view);
        let (mono, dirty) = match view {
            ViewId::Body(w) | ViewId::Tag(w) => {
                let win = self.node.state.window(w).ok()?;
                let dirty = win.body_buffer().and_then(|b| self.node.state.buffer(b).ok()).is_some_and(|b| b.dirty());
                (win.mono, dirty)
            }
            _ => (false, false),
        };
        let hl = self.hl.and_then(|(hv, lo, hi, k)| if hv == view { Some((lo, hi, k)) } else { None });
        Some(Source {
            kind: Kind::of(view),
            mono,
            dirty,
            unsynced: false,
            text: buf.text.clone(),
            sel: (v.q0, v.q1),
            origin: v.origin,
            hl,
            want_visible: self.want_visible.remove(&view),
        })
    }

    pub fn view_text(&self, view: ViewId) -> String {
        self.node
            .view_buffer(view)
            .ok()
            .and_then(|b| self.node.state.buffer(b).ok())
            .map(|b| b.text.to_string())
            .unwrap_or_default()
    }

    pub fn set_origin(&mut self, view: ViewId, origin: usize) {
        let _ = self.node.set_origin(&mut self.log, view, origin);
    }

    pub fn term_resize(&mut self, term: TermId, cols: u16, rows: u16) {
        match &mut self.backend {
            Backend::Local(server) => server.term_resize(&mut self.log, term, cols, rows),
            Backend::Remote(link) => {
                let same = self.node.state.terms.get(&term).is_some_and(|t| t.cols == cols && t.rows == rows);
                if !same {
                    link.send(&ClientMsg::TermResize { term, cols, rows });
                }
            }
        }
        self.sync();
    }

    // ---- hit testing ---------------------------------------------------------

    fn locate(&self, pos: Point<Pixels>) -> Option<(Target, Region)> {
        for (w, l) in &self.term_layouts {
            if !l.bounds.contains(&pos) {
                continue;
            }
            let t = match self.node.state.window(*w).ok()?.body {
                Body::Term(t) => t,
                _ => continue,
            };
            if l.scrollbar.contains(&pos) {
                return Some((Target::Term(*w, t), Region::TermScrollbar));
            }
            let (c, r) = l.cell_at(pos);
            return Some((Target::Term(*w, t), Region::Term(c, r)));
        }
        for (v, l) in &self.layouts {
            if !l.bounds.contains(&pos) {
                continue;
            }
            if l.scrollbar.is_some_and(|b| b.contains(&pos)) {
                return Some((Target::View(*v), Region::Scrollbar));
            }
            if l.layout_box.is_some_and(|b| b.contains(&pos)) {
                return Some((Target::View(*v), Region::LayoutBox));
            }
            return Some((Target::View(*v), Region::Text(l.offset_at(pos))));
        }
        None
    }

    fn ctx_of(&self, view: ViewId) -> ExecCtx {
        match view {
            ViewId::Tag(w) | ViewId::Body(w) => ExecCtx::Window(w),
            ViewId::ColTag(c) => ExecCtx::Column(c),
            ViewId::Top => ExecCtx::Top,
        }
    }

    fn text_of(&self, view: ViewId) -> Option<Text> {
        let b = self.node.view_buffer(view).ok()?;
        Some(self.node.state.buffer(b).ok()?.text.clone())
    }

    fn scroll_by(&mut self, view: ViewId, delta: i64) {
        let Some(t) = self.text_of(view) else { return };
        let Ok(b) = self.node.view_buffer(view) else { return };
        let origin = self.node.state.buffer(b).map(|b| b.view(view).origin).unwrap_or(0);
        let total = t.line_count();
        let line = t.line_of(origin) as i64;
        let new = (line + delta).clamp(0, total.saturating_sub(1) as i64) as usize;
        self.set_origin(view, t.line_start(new));
    }

    // ---- mouse ---------------------------------------------------------------

    /// plan9port mapping for laptops: option-click is B2, command-click is B3.
    fn logical_button(&mut self, e: &MouseDownEvent) -> MouseButton {
        if e.button != MouseButton::Left {
            return e.button;
        }
        let b = if e.modifiers.alt {
            MouseButton::Middle
        } else if e.modifiers.platform {
            MouseButton::Right
        } else {
            MouseButton::Left
        };
        self.mouse.left_as = Some(b);
        b
    }

    pub fn mouse_down(&mut self, e: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let button = self.logical_button(e);
        self.mouse.mods = e.modifiers;
        let Some((target, region)) = self.locate(e.position) else { return };
        match (target, button) {
            (Target::View(v), MouseButton::Left) => match region {
                Region::Text(off) => {
                    self.typed_start.remove(&v);
                    if e.click_count >= 2 {
                        if let Some(t) = self.text_of(v) {
                            let (a, z) = double_click(&t, off);
                            let _ = self.node.select(&mut self.log, v, a, z);
                        }
                    } else {
                        let _ = self.node.select(&mut self.log, v, off, off);
                    }
                    self.mouse.b1 = Some(Drag { view: v, anchor: off });
                    self.mouse.chorded = false;
                }
                Region::Scrollbar => self.scrollbar_click(v, e.position, -1),
                Region::LayoutBox => {
                    if let Some(w) = v.window() {
                        self.mouse.box_drag = Some((w, e.position));
                    }
                }
                _ => {}
            },
            (Target::View(v), MouseButton::Middle) => {
                if let Some(d) = self.mouse.b1 {
                    self.mouse.chorded = true;
                    self.cut(d.view, cx);
                } else {
                    match region {
                        Region::Text(off) => {
                            self.mouse.b2 = Some(Drag { view: v, anchor: off });
                            self.hl = None;
                        }
                        Region::Scrollbar => self.scrollbar_click(v, e.position, 0),
                        Region::LayoutBox => self.grow_window(v),
                        _ => {}
                    }
                }
            }
            (Target::View(v), MouseButton::Right) => {
                if let Some(d) = self.mouse.b1 {
                    self.mouse.chorded = true;
                    self.paste(d.view, cx);
                } else {
                    match region {
                        Region::Text(off) => {
                            self.mouse.b3 = Some(Drag { view: v, anchor: off });
                            self.hl = None;
                        }
                        Region::Scrollbar => self.scrollbar_click(v, e.position, 1),
                        Region::LayoutBox => self.fill_window(v),
                        _ => {}
                    }
                }
            }
            (Target::Term(w, t), button) => match (region, button) {
                (Region::Term(c, r), MouseButton::Middle) => {
                    if let Some(word) = self.term_word(w, c, r, is_exec_char) {
                        self.execute(ExecCtx::Window(w), &word, cx);
                    }
                }
                (Region::Term(c, r), MouseButton::Right) => {
                    if let Some(word) = self.term_word(w, c, r, is_file_char) {
                        self.look(ExecCtx::Window(w), &word);
                    }
                }
                (Region::TermScrollbar, MouseButton::Left) => self.term_scrollbar_click(w, t, e.position, -1),
                (Region::TermScrollbar, MouseButton::Right) => self.term_scrollbar_click(w, t, e.position, 1),
                _ => {}
            },
            _ => {}
        }
        cx.notify();
    }

    pub fn mouse_move(&mut self, e: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let pos = e.position;
        let mut changed = false;
        if let Some(d) = self.mouse.b1 {
            if let Some(l) = self.layouts.get(&d.view) {
                let off = l.offset_at(pos);
                let above = pos.y < l.bounds.top();
                let below = pos.y > l.bounds.bottom();
                let _ = self.node.select(&mut self.log, d.view, d.anchor.min(off), d.anchor.max(off));
                if above {
                    self.scroll_by(d.view, -1);
                } else if below {
                    self.scroll_by(d.view, 1);
                }
                changed = true;
            }
        }
        for (drag, kind) in [(self.mouse.b2, HlKind::Exec), (self.mouse.b3, HlKind::Look)] {
            if let Some(d) = drag {
                if let Some(l) = self.layouts.get(&d.view) {
                    let off = l.offset_at(pos);
                    self.hl = Some((d.view, d.anchor.min(off), d.anchor.max(off), kind));
                    changed = true;
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn mouse_up(&mut self, e: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let button = if e.button == MouseButton::Left { self.mouse.left_as.take().unwrap_or(MouseButton::Left) } else { e.button };
        match button {
            MouseButton::Left => {
                self.mouse.b1 = None;
                if let Some((w, start)) = self.mouse.box_drag.take() {
                    let moved = (e.position.x - start.x).abs() > px(4.) || (e.position.y - start.y).abs() > px(4.);
                    if moved {
                        self.move_window(w, e.position);
                    }
                }
            }
            MouseButton::Middle => {
                if let Some(d) = self.mouse.b2.take() {
                    let text = self.take_range(d, HlKind::Exec);
                    self.hl = None;
                    if let Some(text) = text {
                        self.execute(self.ctx_of(d.view), &text, cx);
                    }
                }
            }
            MouseButton::Right => {
                if let Some(d) = self.mouse.b3.take() {
                    let text = self.take_range(d, HlKind::Look);
                    self.hl = None;
                    if let Some(text) = text {
                        self.look(self.ctx_of(d.view), &text);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// plan9port chords: while B1 is held, option cuts and command pastes.
    pub fn modifiers_changed(&mut self, e: &ModifiersChangedEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let prev = self.mouse.mods;
        self.mouse.mods = e.modifiers;
        let Some(d) = self.mouse.b1 else { return };
        if e.modifiers.alt && !prev.alt {
            self.mouse.chorded = true;
            self.cut(d.view, cx);
            cx.notify();
        } else if e.modifiers.platform && !prev.platform {
            self.mouse.chorded = true;
            self.paste(d.view, cx);
            cx.notify();
        }
    }

    fn take_range(&mut self, d: Drag, kind: HlKind) -> Option<String> {
        let t = self.text_of(d.view)?;
        if let Some((hv, lo, hi, _)) = self.hl {
            if hv == d.view && lo < hi {
                return Some(t.slice(lo, hi));
            }
        }
        let (q0, q1) = self.node.selection(d.view).ok()?;
        if q0 < q1 && q0 <= d.anchor && d.anchor <= q1 {
            return Some(t.slice(q0, q1));
        }
        let pred: fn(char) -> bool = match kind {
            HlKind::Exec => is_exec_char,
            HlKind::Look => is_file_char,
        };
        let (a, z) = expand(&t, d.anchor, pred);
        if a == z {
            return None;
        }
        Some(t.slice(a, z))
    }

    fn term_word(&self, w: WindowId, c: usize, r: usize, pred: fn(char) -> bool) -> Option<String> {
        let row = self.term_layouts.get(&w)?.rows.get(r)?;
        let chars: Vec<char> = row.chars().collect();
        if c >= chars.len() || !pred(chars[c]) {
            return None;
        }
        let mut a = c;
        while a > 0 && pred(chars[a - 1]) {
            a -= 1;
        }
        let mut b = c + 1;
        while b < chars.len() && pred(chars[b]) {
            b += 1;
        }
        Some(chars[a..b].iter().collect())
    }

    fn scrollbar_click(&mut self, view: ViewId, pos: Point<Pixels>, dir: i64) {
        let Some(l) = self.layouts.get(&view) else { return };
        let frac = ((pos.y - l.bounds.top()) / l.bounds.size.height).clamp(0., 1.);
        let fit = l.lines_that_fit() as f32;
        let total = l.total_lines;
        if dir == 0 {
            if let Some(t) = self.text_of(view) {
                let line = ((total as f32 * frac) as usize).min(total.saturating_sub(1));
                self.set_origin(view, t.line_start(line));
            }
        } else {
            let n = ((fit * frac) as i64).max(1);
            self.scroll_by(view, n * dir);
        }
    }

    fn term_scrollbar_click(&mut self, w: WindowId, t: TermId, pos: Point<Pixels>, dir: i64) {
        let Some(l) = self.term_layouts.get(&w) else { return };
        let frac = ((pos.y - l.bounds.top()) / l.bounds.size.height).clamp(0., 1.);
        let n = ((l.rows.len() as f32 * frac) as isize).max(1);
        self.term_scroll(t, n * dir as isize);
    }

    fn slot_weight(&self, w: WindowId) -> u32 {
        self.node.state.layout.cols.iter().flat_map(|c| c.wins.iter()).find(|s| s.window == w).map(|s| s.weight).unwrap_or(1)
    }

    fn grow_window(&mut self, view: ViewId) {
        if let Some(w) = view.window() {
            let weight = (self.slot_weight(w) * 2).min(64);
            let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::WinResize { window: w, weight }));
        }
    }

    fn fill_window(&mut self, view: ViewId) {
        let Some(w) = view.window() else { return };
        let Ok(col) = self.node.column_of(w) else { return };
        let wins: Vec<WindowId> = self.node.state.layout.column(col).map(|c| c.wins.iter().map(|s| s.window).collect()).unwrap_or_default();
        for o in wins {
            let weight = if o == w { 1 } else { 0 };
            let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::WinResize { window: o, weight }));
        }
    }

    fn move_window(&mut self, w: WindowId, pos: Point<Pixels>) {
        let target_col = self.node.state.layout.cols.iter().find(|c| {
            self.layouts.get(&ViewId::ColTag(c.id)).is_some_and(|l| pos.x >= l.bounds.left() && pos.x <= l.bounds.right())
        });
        let Some(c) = target_col else { return };
        let col = c.id;
        let at = c
            .wins
            .iter()
            .filter(|s| s.window != w)
            .position(|s| self.layouts.get(&ViewId::Tag(s.window)).is_some_and(|l| l.bounds.top() > pos.y))
            .unwrap_or(c.wins.iter().filter(|s| s.window != w).count());
        let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::WinPlace { window: w, col, at, weight: 1 }));
    }

    pub fn scroll_wheel(&mut self, e: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.locate(e.position) else { return };
        let lh = match target {
            Target::Term(w, _) => self.term_layouts.get(&w).map(|l| l.line_height),
            Target::View(v) => self.layouts.get(&v).map(|l| l.line_height),
        };
        let Some(lh) = lh else { return };
        let lines = match e.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y / lh,
        };
        let n = (-lines).round() as i64;
        if n == 0 {
            return;
        }
        match target {
            Target::View(v) => self.scroll_by(v, n),
            Target::Term(_, t) => {
                self.term_scroll(t, n as isize);
            }
        }
        cx.notify();
    }

    // ---- keyboard ------------------------------------------------------------

    /// Keys go to the text under the pointer, as in acme.
    pub fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.locate(window.mouse_position()) {
            Some((t, _)) => t,
            None => match self.node.seltext {
                Some(v) => Target::View(v),
                None => return,
            },
        };
        let ks = &e.keystroke;
        let m = ks.modifiers;
        match target {
            Target::Term(_, t) => {
                if m.platform && ks.key == "v" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        self.term_paste(t, text);
                    }
                } else if !m.platform {
                    self.term_key(t, term_key(ks));
                }
            }
            Target::View(v) => self.text_key(v, ks, cx),
        }
        cx.notify();
    }

    fn text_key(&mut self, v: ViewId, ks: &Keystroke, cx: &mut Context<Self>) {
        let m = ks.modifiers;
        if m.platform {
            match ks.key.as_str() {
                "z" if m.shift => {
                    let _ = self.node.redo(&mut self.log, v);
                }
                "z" => {
                    let _ = self.node.undo(&mut self.log, v);
                }
                "y" => {
                    let _ = self.node.redo(&mut self.log, v);
                }
                "x" => self.cut(v, cx),
                "c" => self.snarf(v, cx),
                "v" => self.paste(v, cx),
                "a" => {
                    if let Some(t) = self.text_of(v) {
                        let n = t.len();
                        let _ = self.node.select(&mut self.log, v, 0, n);
                    }
                }
                "q" => cx.quit(),
                _ => {}
            }
            self.want_visible.insert(v);
            return;
        }
        let Some(t) = self.text_of(v) else { return };
        let Ok((q0, q1)) = self.node.selection(v) else { return };
        let fit = self.layouts.get(&v).map(|l| l.lines_that_fit()).unwrap_or(1) as i64;
        // acme: up/down scroll by half a window, page up/down by two thirds
        let scroll = match ks.key.as_str() {
            "up" if !m.control => Some(-(fit / 2).max(1)),
            "down" if !m.control => Some((fit / 2).max(1)),
            "pageup" => Some(-(fit * 2 / 3).max(1)),
            "pagedown" => Some((fit * 2 / 3).max(1)),
            _ => None,
        };
        if let Some(n) = scroll {
            self.scroll_by(v, n);
            return;
        }
        let mut typed = true;
        if m.control {
            match ks.key.as_str() {
                "u" => {
                    let s = t.line_start(t.line_of(q0));
                    if s < q0 {
                        let _ = self.node.select(&mut self.log, v, s, q0);
                        let _ = self.node.backspace(&mut self.log, v);
                    }
                }
                "w" => {
                    let mut a = q0;
                    while a > 0 && t.char_at(a - 1).is_whitespace() {
                        a -= 1;
                    }
                    while a > 0 && is_word_char(t.char_at(a - 1)) {
                        a -= 1;
                    }
                    if a < q0 {
                        let _ = self.node.select(&mut self.log, v, a, q0);
                        let _ = self.node.backspace(&mut self.log, v);
                    }
                }
                "h" => {
                    let _ = self.node.backspace(&mut self.log, v);
                }
                "a" => {
                    let s = t.line_start(t.line_of(q0));
                    let _ = self.node.select(&mut self.log, v, s, s);
                    typed = false;
                }
                "e" => {
                    let e = t.line_range(t.line_of(q1)).map(|(_, e)| e).unwrap_or(t.len());
                    let _ = self.node.select(&mut self.log, v, e, e);
                    typed = false;
                }
                _ => typed = false,
            }
        } else {
            match ks.key.as_str() {
                "backspace" => {
                    let _ = self.node.backspace(&mut self.log, v);
                }
                "delete" => {
                    let _ = self.node.delete_forward(&mut self.log, v);
                }
                "enter" => self.type_text(v, "\n"),
                "tab" => self.type_text(v, "\t"),
                "escape" => {
                    if let Some(s) = self.typed_start.remove(&v) {
                        let _ = self.node.select(&mut self.log, v, s.min(q1), q1);
                    }
                    typed = false;
                }
                "left" => {
                    let p = if q0 < q1 { q0 } else { q0.saturating_sub(1) };
                    let _ = self.node.select(&mut self.log, v, p, p);
                    typed = false;
                }
                "right" => {
                    let p = if q0 < q1 { q1 } else { (q1 + 1).min(t.len()) };
                    let _ = self.node.select(&mut self.log, v, p, p);
                    typed = false;
                }
                "home" => {
                    let _ = self.node.select(&mut self.log, v, 0, 0);
                    typed = false;
                }
                "end" => {
                    let n = t.len();
                    let _ = self.node.select(&mut self.log, v, n, n);
                    typed = false;
                }
                _ => match &ks.key_char {
                    Some(s) if !s.is_empty() => self.type_text(v, s),
                    _ => typed = false,
                },
            }
        }
        let _ = typed;
        self.want_visible.insert(v);
    }

    fn type_text(&mut self, v: ViewId, s: &str) {
        if let Ok((q0, _)) = self.node.selection(v) {
            let e = self.typed_start.entry(v).or_insert(q0);
            if *e > q0 {
                *e = q0;
            }
        }
        let _ = self.node.insert(&mut self.log, v, s);
    }

    // ---- editing with the system clipboard as snarf --------------------------

    fn snarf(&mut self, v: ViewId, cx: &mut Context<Self>) {
        if let Ok(text) = self.node.selected_text(v) {
            if !text.is_empty() {
                let _ = self.node.snarf(&mut self.log, v);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    fn cut(&mut self, v: ViewId, cx: &mut Context<Self>) {
        self.snarf(v, cx);
        let _ = self.node.cut(&mut self.log, v);
        self.want_visible.insert(v);
    }

    fn paste(&mut self, v: ViewId, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: text.clone() }));
        let _ = self.node.replace_selection(&mut self.log, v, &text);
        self.want_visible.insert(v);
    }

    // ---- execute (B2) and look (B3) ---------------------------------------------

    fn report(&mut self, ctx: ExecCtx, msg: &str) {
        if let Ok(col) = apex_server::column_of(&self.node, ctx) {
            let _ = self.node.errors(&mut self.log, col, &format!("{msg}\n"));
        }
    }

    pub fn execute(&mut self, ctx: ExecCtx, text: &str, cx: &mut Context<Self>) {
        let word = text.trim().split_whitespace().next().unwrap_or("").to_string();
        if word == "Paste" {
            if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: t }));
            }
        }
        match self.node.exec(&mut self.log, ctx, text) {
            Ok(Executed::Quit(_)) => cx.quit(),
            Ok(Executed::Failed(_, reason)) => self.report(ctx, &reason),
            Ok(_) => {}
            Err(e) => self.report(ctx, &e.to_string()),
        }
        if word == "Cut" || word == "Snarf" {
            let s = self.node.state.layout.snarf.clone();
            if !s.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(s));
            }
        }
        if let Some(v) = self.node.edit_target(ctx) {
            self.want_visible.insert(v);
        }
        self.after();
    }

    pub fn look(&mut self, ctx: ExecCtx, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match &self.backend {
            Backend::Local(server) => {
                let p = server.plumb(&self.node, ctx, text);
                if let Some(w) = perform(&mut self.node, &mut self.log, vec![p]) {
                    self.show(w);
                }
            }
            Backend::Remote(link) => link.send(&ClientMsg::Plumb { ctx, text: text.to_string() }),
        }
        self.after();
    }

}

fn term_key(ks: &Keystroke) -> TermKey {
    TermKey {
        key: ks.key.clone(),
        text: ks.key_char.clone(),
        shift: ks.modifiers.shift,
        control: ks.modifiers.control,
        alt: ks.modifiers.alt,
    }
}
