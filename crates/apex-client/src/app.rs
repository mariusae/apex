//! The apex client: acme's interaction model (three-button mouse, chords,
//! keyboard to the text under the pointer) over the core's state. Every
//! change goes through the leader node as log entries. The server either
//! runs in-process and shares the log, or sits behind a socket: the
//! client's code path is the same, only the [`Backend`] differs.

use std::collections::{HashMap, HashSet};

use gpui::{
    point, px, ClipboardItem, Context, FocusHandle, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Window,
};

use std::path::{Path, PathBuf};

use apex_core::node::Erase;
use apex_core::tiling::{self, Info, SCROLLWID};
use apex_core::*;
use apex_server::proto::ClientMsg;
use apex_server::providers::SessionUrl;
use apex_server::remote::{Link, Wake};
use apex_server::{PlumbReq, PlumbStep, perform, Server, ServerEvent, TermKey};

use crate::shell::{Selector, TITLEBAR_HEIGHT};
use crate::menu;
use crate::text_element::font_for;

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

/// A layout box being dragged: a window's or a column's.
#[derive(Clone, Copy, Debug)]
enum BoxTarget {
    Win(WindowId),
    Col(ColumnId),
}

#[derive(Default)]
struct Mouse {
    b1: Option<Drag>,
    b2: Option<Drag>,
    b3: Option<Drag>,
    chorded: bool,
    /// B1 was pressed while B2 was down: the command gets an argument.
    chord_arg: bool,
    /// acme's `coldragwin`/`rowdragcol`: the box, the button, where it was pressed.
    box_drag: Option<(BoxTarget, MouseButton, Point<Pixels>)>,
    /// acme's `textscroll`: a scrollbar button held, and the pointer's height.
    scrolling: Option<(Target, MouseButton, Pixels)>,
    /// acme's `framescroll`: B1 dragged past the top or bottom of the text.
    autoscroll: Option<(ViewId, i64)>,
    /// B1 held in a terminal: the selection follows the pointer.
    term_drag: Option<WindowId>,
    /// B3 went down with shift: the Look runs backwards.
    b3_reverse: bool,
    /// B2 or B3 held in a terminal (acme's `textselect23`): the window,
    /// the button, the cell pressed and its position.
    term_sweep: Option<(WindowId, MouseButton, (usize, usize), (usize, u64))>,
    left_as: Option<MouseButton>,
    mods: Modifiers,
}

/// A mouse move acme wants, resolved once the screen shows the new layout.
#[derive(Clone, Copy, Debug)]
enum Pending {
    Warp(Warp),
    Restore(Point<Pixels>),
}

/// What acme's tiling asks about text, measured off the last frame.
struct ClientInfo {
    font: i32,
    prop: i32,
    mono: i32,
    /// wrapped tag lines and whether the tag ends with a newline
    tags: HashMap<WindowId, (i32, bool)>,
    /// (mono?, lines of text from the origin on, a terminal?)
    bodies: HashMap<WindowId, (bool, i32, bool)>,
}

impl Info for ClientInfo {
    fn font_height(&self) -> i32 {
        self.font
    }
    fn taglines(&self, w: WindowId, _width: i32, maxlines: i32) -> i32 {
        let (n, nl) = self.tags.get(&w).copied().unwrap_or((1, false));
        tiling::taglines_rule(n, nl, maxlines)
    }
    fn body_font_height(&self, w: WindowId) -> i32 {
        match self.bodies.get(&w) {
            Some((true, _, _)) => self.mono,
            Some((false, _, false)) => self.prop,
            _ => self.mono,
        }
    }
    fn body_nlines(&self, w: WindowId, _width: i32, maxlines: i32) -> i32 {
        match self.bodies.get(&w) {
            Some((_, _, true)) => maxlines,
            Some((_, lines, false)) => (*lines).min(maxlines),
            None => maxlines,
        }
    }
}

#[derive(Clone, Copy, Debug)]
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
    /// The session this window shows.
    pub session: String,
    /// The local daemon's socket, when the session can be switched from here.
    pub socket: Option<std::path::PathBuf>,
    /// Where this window's session is, as a URL.
    pub url: SessionUrl,
    pub wake: Option<Wake>,
    pub selector: Option<Selector>,
    /// Measured by the tag elements each frame: wrapped lines, trailing newline.
    pub tag_need: HashMap<ViewId, (usize, bool)>,
    /// `Exit`: the next frame closes this window.
    pub close_requested: bool,
    pending: Option<Pending>,
    /// The title last given to the OS window.
    pub title_shown: String,
    /// The link to the daemon is up (a socket, or a provider's bridge).
    pub connected: bool,
    /// A selection in a terminal, client-side: the window, and two
    /// positions (column, history line), the end exclusive.
    pub term_sel: Option<(WindowId, (usize, u64), (usize, u64))>,
    /// The snarf buffer as it was when a terminal copy was requested.
    snarf_wanted: Option<String>,
    /// A B2/B3 sweep in a terminal, shown in the button's colour.
    pub term_hl: Option<(WindowId, MouseButton, (usize, u64), (usize, u64))>,
    /// The window is full screen: no title bar, acme's area from the top.
    pub fullscreen: bool,
    /// Positions to bring on screen (new `+Errors` text), by view.
    show_at: HashMap<ViewId, usize>,
    /// A place to go once its file is open (asked of the server).
    pending_goto: Option<Loc>,
    /// ⌘P, when open.
    pub finder: Option<crate::finder::Finder>,
    /// The windows as of the last frame, to notice closings.
    pub last_windows: std::collections::BTreeMap<WindowId, String>,
    /// A new window with the picker open and nothing attached yet: it
    /// closes if the picker is dismissed, and is not remembered.
    pub chooser: bool,
    /// The tools menu while B4 is held.
    pub menu: Option<menu::Menu>,
    /// What the menu ran last: it opens on that item.
    menu_last: Option<String>,
    /// Snarfouts waiting for a terminal's text: (ask id, terminal).
    snarfouts: Vec<(u64, TermId)>,
    /// Previews of remote files waiting for their first bytes: (ask id,
    /// path, app).
    previews: Vec<(u64, String, Option<String>)>,
    /// Remote files being previewed: subscribed, their copies kept current.
    live: std::collections::HashMap<String, Live>,
    /// The extensions Preview is offered for, as last derived.
    preview_wanted: std::collections::BTreeSet<String>,
    /// The heartbeat: when the last ping went out.
    last_ping: Option<std::time::Instant>,
    /// The frame after a layout change has the geometry the warp needs.
    warp_wait: bool,
    /// acme's savemouse/restoremouse: the window whose creation moved the
    /// mouse, and where it was.
    mouse_saved: Option<(WindowId, Point<Pixels>)>,
    /// Where the pointer was put by a warp, until the next real mouse event.
    pointer: Option<Point<Pixels>>,
    last_mouse: Point<Pixels>,
    pub focus: FocusHandle,
    pub layouts: HashMap<ViewId, TextLayout>,
    pub term_layouts: HashMap<WindowId, TermLayout>,
    pub hl: Option<(ViewId, usize, usize, HlKind)>,
    mouse: Mouse,
    want_visible: HashSet<ViewId>,
    typed_start: HashMap<ViewId, usize>,
}

/// acme's `isalnum` (text.c): anything but space, controls and ASCII
/// punctuation (so `_` counts).
fn is_alnum(c: char) -> bool {
    let u = c as u32;
    u > 0x20 && !(0x7F..=0xA0).contains(&u) && !".!\"#$%&'()*+,-./:;<=>?@[\\]^`{|}~".contains(c)
}
/// acme's `isfilec` (look.c): alnum, and `.-+/:@`.
fn is_file_char(c: char) -> bool {
    is_alnum(c) || ".-+/:@".contains(c)
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



impl Acme {
    /// A session with the server in-process.
    pub fn new(cx: &mut Context<Self>, files: Vec<String>) -> (Acme, futures::channel::mpsc::UnboundedReceiver<ServerEvent>) {
        let mut log = Log::new();
        let (a, _) = log.attach(AttachmentKind::Ui, "apex");
        let mut node = Node::new(a);
        node.catch_up(&log).expect("fresh log");
        let col = node.init_session(&mut log).expect("init session");
        let (mut server, rx) = Server::new(&log);
        server.install_default_rules(&mut log);
        node.catch_up(&log).expect("rules");
        let cwd = server.cwd.clone();
        let names: Vec<&str> = if files.is_empty() { vec!["."] } else { files.iter().map(|s| s.as_str()).collect() };
        for f in names {
            match server.open_file(col, None, &cwd, f, None) {
                Ok(p) => {
                    perform(&mut node, &mut log, vec![p]);
                }
                Err(e) => {
                    let _ = node.errors(&mut log, Some(&cwd.to_string_lossy()), &format!("{e}\n"));
                }
            }
        }
        (Self::over(cx, log, node, Backend::Local(server), "local"), rx)
    }

    /// Attach to the session at `url`: this machine's daemon (started if
    /// it must be) or a destination through its provider. `wake` is
    /// called from the reader thread when there is something to poll.
    pub fn attach(cx: &mut Context<Self>, url: &SessionUrl, files: Vec<String>, wake: Wake) -> std::io::Result<Acme> {
        let (mut link, mut log, mut node) = Self::connect(url, wake.clone())?;
        Self::arm(&mut link);
        let col = match node.state.layout.cols.first() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        let mut acme = Self::over(cx, log, node, Backend::Remote(link), &url.session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = url.clone();
        acme.wake = Some(wake);
        crate::shell::note_recent(url);
        acme.open_initial(col, files);
        Ok(acme)
    }

    /// What this client does for the rules, and the rules it brings: it
    /// can `open` things the way the platform does, and URLs go there.
    /// The rules are its own, gone when it detaches.
    fn arm(link: &mut Link) {
        let urls = PlumbRule {
            verb: "plumb".into(),
            text: Some(r"https?://\S+".into()),
            file: None,
            kind: None,
            isfile: None,
            isdir: None,
            action: RuleAction::Client { verb: "open".into(), args: "$0".into() },
            to: None,
        };
        link.send(&ClientMsg::RuleAdd { rule: urls, priority: -10, mine: true });
        // Snarfout in terminals and win windows: the last command's output
        for (kind, file) in [(WinKind::Term, None), (WinKind::File, Some(r"/-[^/]+$".to_string()))] {
            let rule = PlumbRule {
                verb: "Snarfout".into(),
                text: None,
                file,
                kind: Some(kind),
                isfile: None,
                isdir: None,
                action: RuleAction::Client { verb: "snarfout".into(), args: "$win".into() },
                to: None,
            };
            link.send(&ClientMsg::RuleAdd { rule, priority: -10, mine: true });
        }
    }

    /// `Snarfout`: the last command's output in a terminal (its text read
    /// from the host, scrollback and all) or a win window (its buffer),
    /// found by the transcript heuristic, into the snarf buffer and the
    /// clipboard.
    fn snarfout(&mut self, id: u64, w: WindowId) {
        match self.node.state.window(w).map(|x| x.body) {
            Ok(Body::Term(t)) => {
                let Some(term) = self.node.state.terms.get(&t) else {
                    self.send(ClientMsg::Applied { id, result: Err("no such terminal".into()) });
                    return;
                };
                let to = term.top + term.rows as u64;
                self.snarfouts.push((id, t));
                self.send(ClientMsg::TermRead { term: t, from: 0, to });
            }
            Ok(_) => {
                let text = self.view_text(ViewId::Body(w));
                let result = self.snarf_output(&text);
                self.send(ClientMsg::Applied { id, result });
            }
            Err(e) => self.send(ClientMsg::Applied { id, result: Err(e.to_string()) }),
        }
    }

    fn snarf_output(&mut self, transcript: &str) -> Result<Option<WindowId>, String> {
        let Some(out) = apex_core::transcript::last_output(transcript) else { return Err("Snarfout: no earlier prompt to tell the output by".into()) };
        // into the snarf buffer, and the clipboard once it lands
        self.snarf_wanted = Some(self.node.state.layout.snarf.clone());
        let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: out }));
        self.sync();
        Ok(None)
    }

    /// Preview is offered where a setting names an app for the file's
    /// kind (`Preview.md`, say), not for the fallback: one rule of ours
    /// per such extension, kept in step with the settings.
    fn sync_preview_rules(&mut self) {
        let Backend::Remote(link) = &mut self.backend else { return };
        let me = link.attachment;
        let meta = &self.node.state.meta;
        let wanted = preview_exts(meta, me);
        if wanted == self.preview_wanted {
            return;
        }
        let installed: Vec<(RuleId, String)> = meta
            .rules
            .iter()
            .filter(|(_, r)| r.attachment == me && r.rule.verb == "Preview")
            .filter_map(|(id, r)| r.rule.file.as_deref().and_then(ext_of_pattern).map(|e| (*id, e)))
            .collect();
        for ext in wanted.iter() {
            if !installed.iter().any(|(_, e)| e == ext) && !self.preview_wanted.contains(ext) {
                let rule = PlumbRule {
                    verb: "Preview".into(),
                    text: None,
                    file: Some(pattern_of_ext(ext)),
                    kind: Some(WinKind::File),
                    isfile: None,
                    isdir: None,
                    action: RuleAction::Client { verb: "preview".into(), args: "$file".into() },
                    to: None,
                };
                link.send(&ClientMsg::RuleAdd { rule, priority: -10, mine: true });
            }
        }
        for (id, ext) in installed {
            if !wanted.contains(&ext) {
                link.send(&ClientMsg::RuleRm { id });
            }
        }
        self.preview_wanted = wanted;
    }

    /// What rules asked this client to do since the last poll: `open`
    /// and `preview`, answered when done. A preview of a remote file
    /// first asks the host for its bytes.
    fn answer_asks(&mut self) {
        let Backend::Remote(link) = &mut self.backend else { return };
        let asks = std::mem::take(&mut link.client_asks);
        let files = std::mem::take(&mut link.files);
        let me = link.attachment;
        for (id, verb, args) in asks {
            match verb.as_str() {
                "open" => {
                    let result = client_do("open", &args).map(|_| None);
                    self.send(ClientMsg::Applied { id, result });
                }
                "preview" => {
                    let app = self.preview_app(me, &args);
                    if self.url.is_local() {
                        let result = open_preview(app.as_deref(), Path::new(&args)).map(|_| None);
                        self.send(ClientMsg::Applied { id, result });
                    } else if let Some(live) = self.live.get_mut(&args) {
                        // already previewing: show it again
                        let result = open_preview(app.as_deref(), &live.copy).map(|child| {
                            live.child = child;
                        });
                        self.send(ClientMsg::Applied { id, result: result.map(|_| None) });
                    } else {
                        // the file is on the host: subscribe, and show the
                        // copy as it arrives and changes
                        self.previews.push((id, args.clone(), app));
                        self.send(ClientMsg::Watch { path: args });
                    }
                }
                "snarfout" => match args.trim().parse::<u64>() {
                    Ok(n) => self.snarfout(id, WindowId(n)),
                    Err(_) => self.send(ClientMsg::Applied { id, result: Err(format!("snarfout: {args}: not a window")) }),
                },
                other => {
                    self.send(ClientMsg::Applied { id, result: Err(format!("apex-ui cannot {other}")) });
                }
            }
        }
        // terminals' text read for Snarfout
        let lines: Vec<(TermId, String)> = match &mut self.backend {
            Backend::Remote(link) => std::mem::take(&mut link.term_lines),
            _ => Vec::new(),
        };
        for (t, text) in lines {
            if let Some(i) = self.snarfouts.iter().position(|(_, st)| *st == t) {
                let (id, _) = self.snarfouts.remove(i);
                let result = self.snarf_output(&text);
                self.send(ClientMsg::Applied { id, result });
            }
        }
        for (path, bytes) in files {
            if let Some(i) = self.previews.iter().position(|(_, p, _)| *p == path) {
                // the first bytes: the copy opens
                let (id, _, app) = self.previews.remove(i);
                let result = bytes.and_then(|b| {
                    let copy = preview_copy(&self.url, &path, &b)?;
                    let child = open_preview(app.as_deref(), &copy)?;
                    self.live.insert(path.clone(), Live { copy, child });
                    Ok(())
                });
                if result.is_err() {
                    self.send(ClientMsg::Unwatch { path });
                }
                self.send(ClientMsg::Applied { id, result: result.map(|_| None) });
            } else if let Some(live) = self.live.get(&path) {
                // a change: the copy follows, and the previewer sees it
                if let Ok(b) = bytes {
                    let _ = std::fs::write(&live.copy, b);
                }
            }
        }
        self.end_stale_previews();
    }

    /// A live preview ends with its previewer (Quick Look's process),
    /// with the file's window, or with our lead; its subscription with it.
    fn end_stale_previews(&mut self) {
        let fenced = self.fenced();
        let mut done = Vec::new();
        for (path, live) in self.live.iter_mut() {
            let exited = live.child.as_mut().is_some_and(|c| c.try_wait().map(|s| s.is_some()).unwrap_or(true));
            let window_gone = !self.node.state.buffers.values().any(|b| b.name == *path);
            if exited || window_gone || fenced {
                done.push(path.clone());
            }
        }
        for path in done {
            self.live.remove(&path);
            self.send(ClientMsg::Unwatch { path });
        }
    }

    /// The app that previews `path`, from the settings: this attachment's
    /// then the session's `Preview.EXT`, then `Preview`; none means the
    /// platform's own previewer.
    fn preview_app(&self, me: AttachmentId, path: &str) -> Option<String> {
        let meta = &self.node.state.meta;
        let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase());
        ext.and_then(|e| meta.setting(me, &format!("Preview.{e}")).map(String::from)).or_else(|| meta.setting(me, "Preview").map(String::from))
    }

    /// A link to the session at `url`, made if it does not exist: the
    /// local socket, or a destination through its provider (our apex
    /// installed there first).
    fn connect(url: &SessionUrl, wake: Wake) -> std::io::Result<(Link, Log, Node)> {
        match url.dest() {
            None => {
                let socket = apex_server::daemon::default_socket();
                crate::shell::ensure_daemon(&socket)?;
                let stream = std::os::unix::net::UnixStream::connect(&socket)?;
                let w = stream.try_clone()?;
                let closer = stream.try_clone()?;
                Link::over_streams_creating(
                    Box::new(stream),
                    Box::new(w),
                    Some(Box::new(move || {
                        let _ = closer.shutdown(std::net::Shutdown::Both);
                    })),
                    &url.session,
                    "apex",
                    AttachmentKind::Ui,
                    Some(wake),
                    apex_server::remote::local_profile(),
                )
            }
            Some(dest) => {
                apex_server::providers::deploy(&dest)?;
                let cmd = apex_server::providers::attach_command(&dest, &url.session)?;
                Self::connect_via(&cmd, &url.session, wake)
            }
        }
    }

    /// Attach through a command's stdin and stdout.
    pub fn connect_via(cmd: &str, session: &str, wake: Wake) -> std::io::Result<(Link, Log, Node)> {
        let (stdin, stdout, closer) = apex_server::remote::bridge_child(cmd)?;
        Link::over_streams_creating(Box::new(stdout), Box::new(stdin), Some(closer), session, "apex", AttachmentKind::Ui, Some(wake), apex_server::remote::local_profile())
    }

    /// Attach through an arbitrary command (`--via`).
    pub fn attach_via(cx: &mut Context<Self>, cmd: &str, session: &str, files: Vec<String>, wake: Wake) -> std::io::Result<Acme> {
        let (mut link, mut log, mut node) = Self::connect_via(cmd, session, wake.clone())?;
        Self::arm(&mut link);
        let col = match node.state.layout.cols.first() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        let mut acme = Self::over(cx, log, node, Backend::Remote(link), session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = SessionUrl { provider: "via".into(), arg: cmd.split_whitespace().nth(1).unwrap_or("?").to_string(), session: session.to_string() };
        acme.wake = Some(wake);
        acme.open_initial(col, files);
        Ok(acme)
    }

    /// Re-point this window at the session at `url` (the selector). The
    /// old attachment ends; its leases return to its daemon.
    pub fn reattach(&mut self, url: &SessionUrl, window: &mut Window) -> std::io::Result<()> {
        let wake = self.wake.clone().ok_or_else(|| std::io::Error::other("no wake"))?;
        let (mut link, mut log, mut node) = Self::connect(url, wake)?;
        Self::arm(&mut link);
        let col = match node.state.layout.cols.first() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        self.backend = Backend::Remote(link);
        self.connected = true;
        self.last_ping = None;
        self.log = log;
        self.node = node;
        self.session = url.session.clone();
        self.url = url.clone();
        // a window that started offline is one to remember now
        self.socket = Some(apex_server::daemon::default_socket());
        self.chooser = false;
        self.layouts.clear();
        self.term_layouts.clear();
        self.hl = None;
        self.mouse = Mouse::default();
        self.want_visible.clear();
        self.typed_start.clear();
        self.selector = None;
        // a new attachment: its rules are installed afresh, and previews
        // of the old session are over
        self.preview_wanted.clear();
        self.previews.clear();
        self.live.clear();
        window.set_window_title(&Self::title(url));
        crate::shell::note_recent(url);
        self.open_initial(col, Vec::new());
        Ok(())
    }

    /// Attach to this window's session again: a fresh link and snapshot,
    /// taking the leases back. What acme cannot tell from a stuck link,
    /// the user can.
    /// End this window's link now (the app is quitting): the bridge
    /// behind it goes with it, so the far end sees us leave.
    pub fn close_link(&mut self) {
        if let Backend::Remote(link) = &mut self.backend {
            link.close();
        }
    }

    pub fn reconnect(&mut self, window: &mut Window) {
        if self.wake.is_none() {
            return; // an in-process session has nothing to reconnect to
        }
        let url = self.url.clone();
        match self.reattach(&url, window) {
            Ok(()) => self.notice(&format!("{url}: reconnected\n")),
            Err(e) => {
                self.connected = false;
                let msg = Self::connect_error(&url, &e);
                self.notice(&msg);
            }
        }
    }

    /// What to tell the user when a session could not be attached: the
    /// error, and for a daemon of another build, how to get going again.
    pub fn connect_error(url: &SessionUrl, e: &std::io::Error) -> String {
        if e.kind() == std::io::ErrorKind::Unsupported {
            format!("{url}: {e}: Reconnect (⌘⇧R)\n")
        } else {
            format!("{url}: {e}\n")
        }
    }

    /// Rename this window's session on its daemon.
    pub fn rename_session(&mut self, to: &str, window: &mut Window) {
        let from = self.session.clone();
        if to.is_empty() || to == from {
            return;
        }
        self.send(ClientMsg::RenameSession { from: from.clone(), to: to.to_string() });
        let old = self.url.clone();
        self.session = to.to_string();
        self.url = self.url.with_session(to);
        window.set_window_title(&Self::title(&self.url));
        crate::shell::renamed_recent(&old, &self.url);
    }

    pub fn title(url: &SessionUrl) -> String {
        format!("{url} — apex")
    }

    /// The window's title, with the connection and fenced states.
    pub fn current_title(&self) -> String {
        if !self.connected {
            format!("{} — disconnected — apex", self.url)
        } else if self.fenced() {
            format!("{} — fenced (another client leads) — apex", self.url)
        } else {
            Self::title(&self.url)
        }
    }

    /// Open the files named on the command line; a fresh session with
    /// nothing named shows the working directory, as acme does.
    fn open_initial(&mut self, col: ColumnId, files: Vec<String>) {
        if files.is_empty() && self.node.state.windows.is_empty() {
            self.send(ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: ".".into() });
        }
        for f in files {
            self.send(ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: f });
        }
        self.after();
    }


    /// Every few seconds a ping goes to the daemon; a pong that does not
    /// come back in time means the link is dead even if the socket has
    /// not closed (ssh gone quiet). A pong after that means it is back.
    const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(3);
    const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

    fn heartbeat(&mut self) -> bool {
        let Backend::Remote(link) = &mut self.backend else { return false };
        let now = std::time::Instant::now();
        let answered = match (self.last_ping, link.last_pong) {
            (Some(sent), Some(pong)) => pong >= sent,
            (Some(_), None) => false,
            (None, _) => true,
        };
        let overdue = self.last_ping.is_some_and(|sent| !answered && now.duration_since(sent) > Self::HEARTBEAT_TIMEOUT);
        let mut changed = false;
        if overdue && self.connected {
            self.connected = false;
            changed = true;
        } else if answered && !self.connected && link.last_pong.is_some() {
            self.connected = true;
            changed = true;
        }
        if answered || overdue {
            link.send(&ClientMsg::Ping { t: now.elapsed().as_millis() as u64 });
            self.last_ping = Some(now);
        }
        changed
    }

    fn over(cx: &mut Context<Self>, log: Log, node: Node, backend: Backend, session: &str) -> Acme {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Self::HEARTBEAT).await;
            let alive = cx.update(|cx| {
                this.update(cx, |acme, cx| {
                    if acme.heartbeat() {
                        cx.notify();
                    }
                    true
                })
                .unwrap_or(false)
            });
            if !alive {
                break;
            }
        })
        .detach();
        Acme {
            log,
            node,
            backend,
            session: session.to_string(),
            socket: None,
            url: SessionUrl::local(session),
            wake: None,
            selector: None,
            tag_need: HashMap::new(),
            close_requested: false,
            pending: None,
            title_shown: String::new(),
            connected: true,
            last_ping: None,
            term_sel: None,
            snarf_wanted: None,
            term_hl: None,
            fullscreen: false,
            show_at: HashMap::new(),
            pending_goto: None,
            finder: None,
            last_windows: std::collections::BTreeMap::new(),
            chooser: false,
            menu: None,
            menu_last: None,
            snarfouts: Vec::new(),
            previews: Vec::new(),
            live: std::collections::HashMap::new(),
            preview_wanted: std::collections::BTreeSet::new(),
            warp_wait: false,
            mouse_saved: None,
            pointer: None,
            last_mouse: Point::default(),
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
    /// Also ships whatever this node sequenced since the last call: this
    /// runs after every input handler and on every frame, so nothing the
    /// user typed is ever more than a frame away from the daemon.
    pub fn sync(&mut self) {
        // acme's winsettag: Undo/Redo/Put/Get come and go with the state
        let _ = self.node.update_tags(&mut self.log);
        self.track_closed();
        for (v, q) in self.node.take_shows() {
            self.show_at.insert(v, q);
        }
        for loc in self.node.take_gotos() {
            self.goto(loc);
        }
        // a place whose file was being opened: land once it is
        if let Some(loc) = self.pending_goto.clone() {
            if self.node.state.windows.keys().any(|w| self.node.window_name(*w) == loc.name) {
                self.pending_goto = None;
                let _ = self.node.land(&mut self.log, &loc);
                if let Some(w) = self.node.state.windows.keys().copied().find(|w| self.node.window_name(*w) == loc.name) {
                    self.want_visible.insert(ViewId::Body(w));
                }
            }
        }
        if let Backend::Remote(link) = &mut self.backend {
            link.flush(&self.log);
        }
        if let Err(e) = self.node.catch_up(&self.log) {
            eprintln!("catch up: {e}");
        }
        self.take_warp();
    }

    /// Has this client lost its leases (another UI attached and took
    /// them)? The mirror log follows the metalog, so it knows.
    pub fn fenced(&self) -> bool {
        matches!(self.backend, Backend::Remote(_))
            && self.log.lease(Shard::Layout).is_some_and(|l| l.holder != self.node.attachment || l.released.is_some())
    }

    /// A layout box is held: acme shows the box cursor.
    pub fn dragging_box(&self) -> bool {
        self.mouse.box_drag.is_some()
    }

    /// The mouse move acme would make after the last layout change.
    fn take_warp(&mut self) {
        let Some(w) = self.node.warp.take() else { return };
        let p = match w {
            Warp::NewWindow(win) => {
                // savemouse: coming back is possible if this window closes
                self.mouse_saved = Some((win, self.last_mouse));
                Pending::Warp(w)
            }
            Warp::Closed { window, next } => {
                // restoremouse
                let saved = self.mouse_saved.take();
                match saved {
                    Some((sw, at)) if sw == window => Pending::Restore(at),
                    _ => match next {
                        Some(n) => Pending::Warp(Warp::Closed { window, next: Some(n) }),
                        None => return,
                    },
                }
            }
            other => Pending::Warp(other),
        };
        self.pending = Some(p);
        self.warp_wait = true;
    }

    /// Called at the start of a frame: the previous frame's layouts show
    /// where things are now, so the pending warp can be placed.
    pub fn resolve_warp(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        let Some(p) = self.pending else { return };
        if self.warp_wait {
            return; // the frame after this one has the layouts to use
        }
        self.pending = None;
        let font = font_for(false).line_height;
        let fonti = f32::from(font) as i32;
        let l = &self.node.state.layout;
        let top = self.top();
        let row = |x: i32, y: i32| point(px(x as f32), px(y as f32 + top));
        let target = match p {
            Pending::Restore(at) => Some(at),
            Pending::Warp(Warp::NewWindow(w)) => l.slot(w).map(|s| row(s.r.x0 + SCROLLWID + 3, s.tag_y1(fonti) + 3)),
            Pending::Warp(Warp::WinButton(w)) => l.slot(w).map(|s| row(s.r.x0 + SCROLLWID / 2, s.r.y0 + fonti / 2)),
            Pending::Warp(Warp::ColButton(c)) => l.column(c).map(|c| row(c.r.x0 + SCROLLWID / 2, c.r.y0 + fonti / 2)),
            Pending::Warp(Warp::Closed { next: Some(w), .. }) => {
                // movetodel: the rune two past the tag's first space
                let tag = self.node.state.window(w).ok().map(|x| x.tag);
                let text = tag.and_then(|b| self.node.state.buffer(b).ok()).map(|b| b.text.to_string()).unwrap_or_default();
                let n = text.chars().position(|c| c == ' ').map(|i| i + 2).unwrap_or(0);
                self.layouts.get(&ViewId::Tag(w)).and_then(|tl| tl.point_of(n)).map(|q| point(q.x + px(4.), q.y + font - px(4.)))
            }
            Pending::Warp(Warp::Closed { next: None, .. }) => None,
            Pending::Warp(Warp::Sel(v)) => {
                let q0 = self.node.selection(v).map(|s| s.0).unwrap_or(0);
                self.layouts.get(&v).and_then(|tl| tl.point_of(q0)).map(|q| point(q.x + px(4.), q.y + font - px(4.)))
            }
        };
        if std::env::var_os("APEX_DEBUG_WARP").is_some() {
            let slot = match p {
                Pending::Warp(Warp::NewWindow(w)) | Pending::Warp(Warp::WinButton(w)) => self.node.state.layout.slot(w).copied(),
                Pending::Warp(Warp::Sel(v)) => v.window().and_then(|w| self.node.state.layout.slot(w).copied()),
                Pending::Warp(Warp::Closed { next: Some(w), .. }) => self.node.state.layout.slot(w).copied(),
                _ => None,
            };
            eprintln!("warp {p:?} -> {target:?} (window bounds {:?}) slot {slot:?}", window.bounds());
        }
        if let Some(at) = target {
            crate::warp::move_to(window, at);
            self.pointer = Some(at);
            self.last_mouse = at;
        }
    }

    /// At the end of a render: a warp queued by what this frame applied
    /// needs one more frame, whose layouts will show the new geometry.
    pub fn schedule_warp(&mut self, window: &mut Window) {
        if self.pending.is_some() && self.warp_wait {
            self.warp_wait = false;
            window.request_animation_frame();
        }
    }

    /// Where the pointer is for acme's purposes: where a warp put it, until
    /// the mouse really moves.
    fn pointer(&self, window: &Window) -> Point<Pixels> {
        self.pointer.unwrap_or_else(|| window.mouse_position())
    }

    /// Give the tiling this frame's measurements, refit any window whose
    /// tag changed shape (acme's winsettag), and follow the OS window's
    /// size (rowresize).
    pub fn measure(&mut self, viewport: gpui::Size<Pixels>) {
        let font = f32::from(font_for(false).line_height) as i32;
        let mono = f32::from(font_for(true).line_height) as i32;
        let mut tags = HashMap::new();
        let mut bodies = HashMap::new();
        for (w, win) in &self.node.state.windows {
            if !win.tagexpand {
                tags.insert(*w, (1, false)); // acme: Up in the tag shrank it to one line
            } else if let Some((n, nl)) = self.tag_need.get(&ViewId::Tag(*w)) {
                tags.insert(*w, (*n as i32, *nl));
            }
            let term = matches!(win.body, Body::Term(_));
            let lines = match self.layouts.get(&ViewId::Body(*w)) {
                Some(tl) => tl.total_lines.saturating_sub(tl.first_line) as i32,
                None => win.body_buffer().and_then(|b| self.node.state.buffer(b).ok()).map(|b| b.text.line_count() as i32).unwrap_or(1),
            };
            bodies.insert(*w, (win.mono, lines, term));
        }
        self.node.tiling = Box::new(ClientInfo { font, prop: font, mono, tags, bodies });
        // the OS window
        let r = tiling::Rect::new(0, 0, f32::from(viewport.width) as i32, (f32::from(viewport.height) - self.top()) as i32);
        if r.dx() > 0 && r.dy() > 0 && r != self.node.state.layout.r {
            let _ = self.node.resize_layout(&mut self.log, r);
        }
        // tags that wrap differently than their slot allows for
        let refit: Vec<WindowId> = self
            .node
            .state
            .layout
            .cols
            .iter()
            .flat_map(|c| c.wins.iter())
            .filter(|s| {
                self.tag_need.get(&ViewId::Tag(s.window)).is_some_and(|(n, nl)| {
                    let fit = (s.r.dy() / font).max(0);
                    tiling::taglines_rule(*n as i32, *nl, fit.max(s.taglines)) != s.taglines
                })
            })
            .map(|s| s.window)
            .collect();
        for w in refit {
            let before = self.node.state.layout.slot(w).copied();
            let _ = self.node.refit_window(&mut self.log, w);
            let after = self.node.state.layout.slot(w).copied();
            // acme's winresize: pull the mouse up as a tag closes under it,
            // push it down as a tag expands over it
            if let (Some(b), Some(a)) = (before, after) {
                let m = self.row_pt(self.last_mouse);
                let in_tag = |s: &apex_core::state::Slot, y: i32| s.r.x0 <= m.0 && m.0 < s.r.x1 && s.r.y0 <= y && y < s.tag_y1(font);
                let in_body = |s: &apex_core::state::Slot, y: i32| s.r.x0 <= m.0 && m.0 < s.r.x1 && s.body.y0 <= y && y < s.body.y1;
                let mut to = None;
                if in_tag(&b, m.1) && !in_tag(&a, m.1) {
                    to = Some(a.tag_y1(font) - 3);
                } else if in_body(&b, m.1) && in_tag(&a, m.1) {
                    to = Some(a.tag_y1(font) + 3);
                }
                if let Some(y) = to {
                    let at = point(self.last_mouse.x, px(y as f32 + self.top()));
                    self.pending = Some(Pending::Restore(at));
                    self.warp_wait = false;
                }
            }
        }
    }

    /// The column a view belongs to, for acme's activecol.
    fn column_of_view(&self, v: ViewId) -> Option<ColumnId> {
        match v {
            ViewId::ColTag(c) => Some(c),
            ViewId::Tag(w) | ViewId::Body(w) => self.node.state.layout.column_of(w),
            ViewId::Top => None,
        }
    }

    /// Where acme's area starts: below the title bar, or at the top when
    /// the window is full screen.
    pub fn top(&self) -> f32 {
        if self.fullscreen {
            0.
        } else {
            TITLEBAR_HEIGHT
        }
    }

    fn row_pt(&self, p: Point<Pixels>) -> (i32, i32) {
        (f32::from(p.x) as i32, (f32::from(p.y) - self.top()) as i32)
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
        self.connected = alive;
        for w in link.take_made() {
            self.show(w);
        }
        self.answer_asks();
        self.sync_preview_rules();
        alive
    }

    pub fn show(&mut self, w: WindowId) {
        self.node.seltext = Some(ViewId::Body(w));
        self.want_visible.insert(ViewId::Body(w));
        let _ = self.node.reveal(&mut self.log, w); // textshow: a window with no lines grows
    }

    fn send(&self, m: ClientMsg) {
        if let Backend::Remote(link) = &self.backend {
            link.send(&m);
        }
    }

    /// After a command: in-process, let the server perform what it was
    /// handed and close terminals whose windows are gone; over a socket,
    /// ship what we sequenced. Then catch up.
    pub fn after(&mut self) {
        match &mut self.backend {
            Backend::Local(server) => {
                let props = server.poll_execs(&mut self.log, &self.node);
                if let Some(w) = perform(&mut self.node, &mut self.log, props) {
                    self.show(w);
                }
                let mut shown = Vec::new();
                if let Backend::Local(server) = &mut self.backend {
                    // a rule's verb, B2'd: walk the rules here and now
                    for req in server.take_plumb_starts() {
                        shown.extend(plumb_local(server, &mut self.node, &mut self.log, req));
                    }
                }
                for w in shown {
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
            Backend::Local(server) => server.term_key(&mut self.log, t, &key),
            Backend::Remote(link) => link.send(&ClientMsg::TermKey { term: t, key }),
        }
    }

    fn term_paste(&mut self, t: TermId, text: String) {
        match &mut self.backend {
            Backend::Local(server) => server.term_paste(&mut self.log, t, &text),
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
        let (mono, dirty, live) = match view {
            ViewId::Body(w) | ViewId::Tag(w) => {
                let win = self.node.state.window(w).ok()?;
                let dirty = win.body_buffer().and_then(|b| self.node.state.buffer(b).ok()).is_some_and(|b| b.dirty());
                (win.mono, dirty, self.node.window_live(w))
            }
            _ => (false, false, false),
        };
        let hl = self.hl.and_then(|(hv, lo, hi, k)| if hv == view { Some((lo, hi, k)) } else { None });
        Some(Source {
            kind: Kind::of(view),
            mono,
            dirty,
            live,
            unsynced: false,
            fenced: self.fenced(),
            text: buf.text.clone(),
            sel: (v.q0, v.q1),
            origin: v.origin,
            hl,
            want_visible: self.want_visible.remove(&view),
            show_at: self.show_at.remove(&view),
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
        } else if e.modifiers.shift {
            MouseButton::Navigate(gpui::NavigationDirection::Back) // B4: the tools menu
        } else {
            MouseButton::Left
        };
        self.mouse.left_as = Some(b);
        b
    }

    pub fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.finder.is_some() {
            self.close_finder(cx); // a click anywhere else dismisses it
            return;
        }
        if self.selector.is_some() {
            if self.chooser {
                return; // a new window's picker stays until Escape or a choice
            }
            // a click anywhere else dismisses the dropdown
            self.close_selector(cx);
            return;
        }
        if self.menu.is_some() {
            return; // held open by its button; nothing else until it closes
        }
        if matches!(self.logical_button_peek(e), MouseButton::Navigate(_)) {
            self.logical_button(e);
            let at = match self.locate(e.position) {
                Some((Target::View(v), _)) => v.window(),
                Some((Target::Term(w, _), _)) => Some(w),
                None => None,
            };
            if let Some(w) = at {
                self.menu_open(w, e.position, window);
            }
            cx.notify();
            return;
        }
        self.pointer = None;
        self.last_mouse = e.position;
        let button = self.logical_button(e);
        self.mouse.mods = e.modifiers;
        let Some((target, region)) = self.locate(e.position) else { return };
        // a click in a tag commits the name typed there (acme's wincommit)
        if let Target::View(ViewId::Tag(w)) = target {
            let _ = self.node.commit_tag(&mut self.log, w);
        }
        // a layout box: acme's coldragwin/rowdragcol wait for the release
        if let (Target::View(v), Region::LayoutBox) = (target, region) {
            if self.mouse.b1.is_none() {
                let bt = match v {
                    ViewId::Tag(w) => Some(BoxTarget::Win(w)),
                    ViewId::ColTag(c) => Some(BoxTarget::Col(c)),
                    _ => None,
                };
                if let Some(bt) = bt {
                    self.mouse.box_drag = Some((bt, button, e.position));
                    cx.notify(); // the pointer becomes the box
                    return;
                }
            }
        }
        match (target, button) {
            (Target::View(_), MouseButton::Left) if self.mouse.b2.is_some() => {
                // acme's textselect2: button 1 while 2 is down makes the
                // last selection the command's argument
                self.mouse.chord_arg = true;
            }
            (Target::View(v), MouseButton::Left) => match region {
                Region::Text(off) => {
                    self.typed_start.remove(&v);
                    self.node.activecol = self.column_of_view(v); // button 1 only
                    if e.click_count >= 2 {
                        if let Some(t) = self.text_of(v) {
                            let (a, z) = apex_core::node::double_click(&t, off);
                            let _ = self.node.select(&mut self.log, v, a, z);
                        }
                    } else {
                        let _ = self.node.select(&mut self.log, v, off, off);
                    }
                    self.mouse.b1 = Some(Drag { view: v, anchor: off });
                    self.mouse.chorded = false;
                }
                Region::Scrollbar => self.start_scrolling(Target::View(v), MouseButton::Left, e.position, window, cx),
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
                        Region::Scrollbar => self.start_scrolling(Target::View(v), MouseButton::Middle, e.position, window, cx),
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
                            self.mouse.b3_reverse = e.modifiers.shift;
                            self.hl = None;
                        }
                        Region::Scrollbar => self.start_scrolling(Target::View(v), MouseButton::Right, e.position, window, cx),
                        _ => {}
                    }
                }
            }
            (Target::Term(w, t), button) => match (region, button) {
                (Region::Term(c, r), MouseButton::Left) => {
                    let p = (c, self.term_top(w) + r as u64);
                    self.term_sel = Some((w, p, p));
                    self.mouse.term_drag = Some(w);
                    self.node.activecol = self.column_of_view(ViewId::Tag(w));
                }
                (Region::Term(c, r), MouseButton::Middle) if self.mouse.term_drag.is_some() => {
                    // B1+B2 in a terminal: copy (there is nothing to cut)
                    let _ = (c, r);
                    self.mouse.chorded = true;
                    self.term_copy(w, cx);
                }
                (Region::Term(c, r), MouseButton::Middle | MouseButton::Right) => {
                    // acme's textselect23: sweep, then act on what was swept
                    // (or on the word under a plain click)
                    let p = (c, self.term_top(w) + r as u64);
                    self.mouse.term_sweep = Some((w, button, (c, r), p));
                    self.term_hl = Some((w, button, p, p));
                }
                (Region::TermScrollbar, MouseButton::Left) => self.start_scrolling(Target::Term(w, t), MouseButton::Left, e.position, window, cx),
                (Region::TermScrollbar, MouseButton::Right) => self.start_scrolling(Target::Term(w, t), MouseButton::Right, e.position, window, cx),
                _ => {}
            },
            _ => {}
        }
        cx.notify();
    }

    pub fn mouse_move(&mut self, e: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let pos = e.position;
        if self.menu.is_some() {
            self.menu_track(pos);
            self.last_mouse = pos;
            cx.notify();
            return;
        }
        if self.pointer.is_some_and(|p| (p.x - pos.x).abs() > px(1.) || (p.y - pos.y).abs() > px(1.)) {
            self.pointer = None;
        }
        self.last_mouse = pos;
        let mut changed = false;
        if let Some((_, _, y)) = self.mouse.scrolling.as_mut() {
            *y = pos.y; // the bar follows the pointer's height
        }
        if let Some((w, _, _, anchor)) = self.mouse.term_sweep {
            if let Some(l) = self.term_layouts.get(&w) {
                let (c, r) = l.cell_at(pos);
                let (cols, top) = (l.cols as usize, self.term_top(w));
                let line = top + r as u64;
                let end = if (line, c) >= (anchor.1, anchor.0) { ((c + 1).min(cols), line) } else { (c, line) };
                if let Some(hl) = self.term_hl.as_mut() {
                    hl.3 = end;
                }
                changed = true;
            }
        }
        if let Some(w) = self.mouse.term_drag {
            if let (Some(l), Some((sw, anchor, _))) = (self.term_layouts.get(&w), self.term_sel) {
                if sw == w {
                    let (c, r) = l.cell_at(pos);
                    let (cols, top) = (l.cols as usize, self.term_top(w));
                    let line = top + r as u64;
                    // the end is exclusive: past the pointed-at cell when dragging forward
                    let end = if (line, c) >= (anchor.1, anchor.0) { ((c + 1).min(cols), line) } else { (c, line) };
                    self.term_sel = Some((w, anchor, end));
                    changed = true;
                }
            }
        }
        if let Some(d) = self.mouse.b1 {
            if let Some(l) = self.layouts.get(&d.view) {
                let off = l.offset_at(pos);
                let above = pos.y < l.bounds.top();
                let below = pos.y > l.bounds.bottom();
                let _ = self.node.select(&mut self.log, d.view, d.anchor.min(off), d.anchor.max(off));
                // acme's framescroll: keep scrolling while the pointer is outside
                let want = if above { Some(-1) } else if below { Some(1) } else { None };
                match want {
                    Some(dir) => {
                        let fresh = self.mouse.autoscroll.is_none();
                        self.mouse.autoscroll = Some((d.view, dir));
                        if fresh {
                            self.autoscroll_step();
                            cx.spawn(async move |this, cx| loop {
                                cx.background_executor().timer(std::time::Duration::from_millis(80)).await;
                                let going = cx.update(|cx| {
                                    this.update(cx, |acme, cx| {
                                        let r = acme.autoscroll_step();
                                        cx.notify();
                                        r
                                    })
                                    .unwrap_or(false)
                                });
                                if !going {
                                    break;
                                }
                            })
                            .detach();
                        }
                    }
                    None => self.mouse.autoscroll = None,
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
        self.last_mouse = e.position;
        if self.mouse.scrolling.is_some_and(|(_, b, _)| b == button) {
            self.mouse.scrolling = None;
        }
        if let Some((bt, b, start)) = self.mouse.box_drag {
            if b == button {
                self.mouse.box_drag = None;
                let but = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    _ => 3,
                };
                let (op, p) = (self.row_pt(start), self.row_pt(e.position));
                let r = match bt {
                    BoxTarget::Win(w) => self.node.drag_window(&mut self.log, w, but, op, p),
                    BoxTarget::Col(c) => self.node.drag_column(&mut self.log, c, op, p),
                };
                if let Err(err) = r {
                    eprintln!("layout: {err}");
                }
                self.after();
                cx.notify();
                return;
            }
        }
        if let Some((w, b, cell, _)) = self.mouse.term_sweep {
            if b == button {
                self.mouse.term_sweep = None;
                let hl = self.term_hl.take();
                let swept = hl.filter(|(_, _, p0, p1)| p0 != p1).and_then(|(_, _, p0, p1)| self.term_grid_text(w, p0, p1));
                let text = match (swept, button) {
                    (Some(t), _) => Some(t),
                    (None, MouseButton::Middle) => self.term_word(w, cell.0, cell.1, is_exec_char),
                    // B3 on an OSC 8 link plumbs the link, not its text
                    (None, _) => self.term_link(w, cell.0, cell.1).or_else(|| self.term_word(w, cell.0, cell.1, is_file_char)),
                };
                if let Some(text) = text {
                    match button {
                        MouseButton::Middle => self.execute(ExecCtx::Window(w), &text, cx),
                        _ => self.look(ExecCtx::Window(w), &text),
                    }
                }
                cx.notify();
                return;
            }
        }
        match button {
            MouseButton::Left => {
                self.mouse.b1 = None;
                self.mouse.autoscroll = None;
                self.mouse.term_drag = None;
            }
            MouseButton::Middle => {
                if let Some(d) = self.mouse.b2.take() {
                    let text = self.take_range(d, HlKind::Exec);
                    self.hl = None;
                    let arg = if self.mouse.chord_arg { self.node.seltext.and_then(|v| self.node.selected_text(v).ok()) } else { None };
                    self.mouse.chord_arg = false;
                    if let Some(mut text) = text {
                        if let Some(a) = arg.filter(|a| !a.is_empty()) {
                            text.push(' ');
                            text.push_str(&a);
                        }
                        self.execute(self.ctx_of(d.view), &text, cx);
                    }
                }
            }
            MouseButton::Right => {
                if let Some(d) = self.mouse.b3.take() {
                    let found = self.take_range_at(d, HlKind::Look);
                    self.hl = None;
                    if let Some((text, (lo, hi))) = found {
                        // where the button went down, and what it took;
                        // acme's expand: the file-name expansion first, then,
                        // should nothing take it, the word (isalnum)
                        let b = self.node.view_buffer(d.view).ok();
                        let at = b.map(|b| Span { buffer: b, q0: d.anchor, q1: d.anchor });
                        let sel = b.map(|b| Span { buffer: b, q0: lo, q1: hi });
                        let alt = match (b, self.text_of(d.view)) {
                            (Some(b), Some(t)) => {
                                let (a, z) = expand(&t, d.anchor, is_alnum);
                                if a < z && (a, z) != (lo, hi) && lo <= a && z <= hi {
                                    Some((t.slice(a, z), Span { buffer: b, q0: a, q1: z }))
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        };
                        let reverse = self.mouse.b3_reverse;
                        let ctx = self.ctx_of(d.view);
                        if reverse && self.back_offered(d.view.window()) {
                            // shift-B3 in a stack: B3 went somewhere, this
                            // comes back (the Back verb, as cmd-[ issues it)
                            self.execute(ctx, "Back", cx);
                        } else {
                            self.look_at(ctx, &text, at, sel, alt, reverse);
                        }
                    }
                }
            }
            MouseButton::Navigate(_) => self.menu_up(cx),
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
        self.take_range_at(d, kind).map(|(t, _)| t)
    }

    /// `take_range`, with where the text came from.
    fn take_range_at(&mut self, d: Drag, kind: HlKind) -> Option<(String, (usize, usize))> {
        let t = self.text_of(d.view)?;
        if let Some((hv, lo, hi, _)) = self.hl {
            if hv == d.view && lo < hi {
                return Some((t.slice(lo, hi), (lo, hi)));
            }
        }
        let (q0, q1) = self.node.selection(d.view).ok()?;
        if q0 < q1 && q0 <= d.anchor && d.anchor <= q1 {
            return Some((t.slice(q0, q1), (q0, q1)));
        }
        let pred: fn(char) -> bool = match kind {
            HlKind::Exec => is_exec_char,
            HlKind::Look => is_file_char,
        };
        let (a, z) = expand(&t, d.anchor, pred);
        if a == z {
            return None;
        }
        Some((t.slice(a, z), (a, z)))
    }

    /// The text between two `(column, history line)` positions, from the
    /// rows on screen (a sweep is on screen); lines joined by newlines,
    /// trailing blanks dropped.
    fn term_grid_text(&self, w: WindowId, a: (usize, u64), b: (usize, u64)) -> Option<String> {
        let t = self.term_of(w)?;
        let term = self.node.state.terms.get(&t)?;
        let (p0, p1) = if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) };
        let mut out = String::new();
        for (i, row) in term.grid.iter().enumerate() {
            let line = term.top + i as u64;
            if line < p0.1 || line > p1.1 {
                continue;
            }
            let from = if line == p0.1 { p0.0.min(row.len()) } else { 0 };
            let to = if line == p1.1 { p1.0.min(row.len()) } else { row.len() };
            let s: String = row[from.min(to)..to].iter().map(|c| if c.ch == '\0' { ' ' } else { c.ch }).collect();
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(s.trim_end());
        }
        Some(out).filter(|s| !s.trim().is_empty())
    }

    /// The OSC 8 hyperlink under a terminal cell, if any.
    fn term_link(&self, w: WindowId, c: usize, r: usize) -> Option<String> {
        let t = self.node.state.terms.get(&self.term_of(w)?)?;
        let cell = t.grid.get(r)?.get(c)?;
        if cell.link == 0 {
            return None;
        }
        t.links.get(cell.link as usize - 1).cloned()
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

    /// acme's `textscroll`: a button on the scrollbar keeps scrolling while
    /// it is held, by an amount that follows the pointer's height in the
    /// bar; the pointer is kept on the bar. Returns false once released.
    pub fn scroll_step(&mut self, window: &mut Window) -> bool {
        let Some((target, button, y)) = self.mouse.scrolling else { return false };
        let dir = match button {
            MouseButton::Left => -1,
            MouseButton::Middle => 0,
            _ => 1,
        };
        let (bounds, pos) = match target {
            Target::View(v) => {
                let Some(l) = self.layouts.get(&v) else { return false };
                (l.bounds, point(l.bounds.left() + px(SCROLLWID as f32 / 2.), y))
            }
            Target::Term(w, _) => {
                let Some(l) = self.term_layouts.get(&w) else { return false };
                (l.bounds, point(l.bounds.left() + px(SCROLLWID as f32 / 2.), y))
            }
        };
        let y = y.clamp(bounds.top(), bounds.bottom());
        match target {
            Target::View(v) => self.scrollbar_click(v, point(pos.x, y), dir),
            Target::Term(w, t) => self.term_scrollbar_click(w, t, point(pos.x, y), dir),
        }
        let at = point(pos.x, y);
        crate::warp::move_to(window, at);
        self.pointer = Some(at);
        self.sync();
        true
    }

    fn start_scrolling(&mut self, target: Target, button: MouseButton, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.mouse.scrolling = Some((target, button, pos.y));
        self.scroll_step(window);
        // debounce, then repeat while the button is down (acme: 200 ms, then 80 ms)
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            loop {
                let going = cx.update(|cx| {
                    this.update(cx, |acme, cx| {
                        let r = acme.mouse.scrolling.is_some() && acme.with_window(cx, |acme, window| acme.scroll_step(window));
                        cx.notify();
                        r
                    })
                    .unwrap_or(false)
                });
                if !going {
                    break;
                }
                cx.background_executor().timer(std::time::Duration::from_millis(80)).await;
            }
        })
        .detach();
    }

    /// Run `f` with this view's window, if it is open.
    fn with_window(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Window) -> bool) -> bool {
        let Some(handle) = cx.active_window().or_else(|| cx.windows().into_iter().next()) else { return false };
        let mut result = false;
        let _ = handle.update(cx, |_, window, _| {
            result = f(self, window);
        });
        result
    }

    /// One tick of acme's `framescroll`: scroll by how far the pointer is
    /// past the edge, and extend the selection to the text at the edge.
    fn autoscroll_step(&mut self) -> bool {
        let Some((v, dir)) = self.mouse.autoscroll else { return false };
        let Some(d) = self.mouse.b1 else { return false };
        let Some(l) = self.layouts.get(&v) else { return false };
        let (top, bottom, lh) = (l.bounds.top(), l.bounds.bottom(), l.line_height);
        let pos = self.last_mouse;
        let dist = if dir < 0 { top - pos.y } else { pos.y - bottom };
        if dist <= px(0.) {
            self.mouse.autoscroll = None;
            return false;
        }
        let lines = ((f32::from(dist) / f32::from(lh)) as i64).max(1);
        self.scroll_by(v, dir * lines);
        self.sync();
        if let Some(l) = self.layouts.get(&v) {
            let edge = point(pos.x, if dir < 0 { l.bounds.top() } else { l.bounds.bottom() - px(1.) });
            let off = l.offset_at(edge);
            let _ = self.node.select(&mut self.log, v, d.anchor.min(off), d.anchor.max(off));
        }
        true
    }

    fn term_of(&self, w: WindowId) -> Option<TermId> {
        match self.node.state.window(w).ok()?.body {
            Body::Term(t) => Some(t),
            _ => None,
        }
    }

    /// The history line in a terminal window's first row.
    fn term_top(&self, w: WindowId) -> u64 {
        self.term_of(w).and_then(|t| self.node.state.terms.get(&t)).map(|t| t.top).unwrap_or(0)
    }

    /// Copy a terminal's selection: the server, which has the scrollback,
    /// snarfs its text; the clipboard follows the snarf buffer.
    pub fn term_copy(&mut self, w: WindowId, cx: &mut Context<Self>) {
        let Some((sw, a, b)) = self.term_sel else { return };
        if sw != w || a == b {
            return;
        }
        let Some(t) = self.term_of(w) else { return };
        let (p0, p1) = ((a.0 as u16, a.1), (b.0 as u16, b.1));
        self.snarf_wanted = Some(self.node.state.layout.snarf.clone());
        match &mut self.backend {
            Backend::Local(server) => {
                if let Some(p) = server.term_text(t, p0, p1) {
                    perform(&mut self.node, &mut self.log, vec![p]);
                }
            }
            Backend::Remote(link) => link.send(&ClientMsg::TermText { term: t, p0, p1 }),
        }
        self.after();
        self.settle_snarf(cx);
    }

    /// A terminal copy is waiting for the server's text: once the snarf
    /// buffer changes, the clipboard gets it too.
    pub fn settle_snarf(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = &self.snarf_wanted {
            let s = &self.node.state.layout.snarf;
            if s != prev {
                cx.write_to_clipboard(ClipboardItem::new_string(s.clone()));
                self.snarf_wanted = None;
            }
        }
    }

    /// Go to a place: land if its window is open; else have the server
    /// open the file, and land when it arrives.
    pub fn goto(&mut self, loc: Loc) {
        match self.node.land(&mut self.log, &loc) {
            Ok(Some(w)) => {
                self.want_visible.insert(ViewId::Body(w));
            }
            _ => {
                let Some(col) = self.node.state.layout.cols.first().map(|c| c.id) else { return };
                self.pending_goto = Some(loc.clone());
                match &mut self.backend {
                    Backend::Remote(link) => link.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: loc.name.clone() }),
                    Backend::Local(server) => {
                        let dir = std::path::Path::new(&loc.name).parent().map(|d| d.to_path_buf()).unwrap_or_default();
                        if let Ok(p) = server.open_file(col, None, &dir, &loc.name, None) {
                            perform(&mut self.node, &mut self.log, vec![p]);
                        }
                    }
                }
            }
        }
    }

    /// `logical_button` without recording it.
    fn logical_button_peek(&self, e: &MouseDownEvent) -> MouseButton {
        if e.button != MouseButton::Left {
            return e.button;
        }
        if e.modifiers.alt {
            MouseButton::Middle
        } else if e.modifiers.platform {
            MouseButton::Right
        } else if e.modifiers.shift {
            MouseButton::Navigate(gpui::NavigationDirection::Back)
        } else {
            MouseButton::Left
        }
    }

    /// B4 on a window: the tools menu (libdraw's `menuhit`, as the
    /// mariusae/plan9port acme uses it): the verbs the rules offer this
    /// window, popped up so the last one chosen is under the pointer,
    /// which is warped onto it; tracked while the button is held; the
    /// item under the pointer on release runs as B2 would, none if it is
    /// released outside.
    fn menu_open(&mut self, w: WindowId, at: Point<Pixels>, window: &mut Window) {
        let items = apex_core::plumb::verbs_for(&self.node.state.meta.rules, &self.node.window_name(w), self.node.window_kind(w));
        if items.is_empty() {
            return;
        }
        let fs = crate::text_element::font_for(false);
        let ih = f32::from(fs.line_height) as i32 + menu::VSPACING;
        let fh = f32::from(fs.line_height) as i32;
        let run = |len: usize| gpui::TextRun { len, font: fs.font.clone(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        let widths: Vec<i32> = items.iter().map(|i| f32::from(window.text_system().shape_line(i.clone().into(), fs.size, &[run(i.len())], None).width).ceil() as i32).collect();
        let maxwid = widths.iter().copied().max().unwrap_or(0);
        let nitem = items.len() as i32;
        let lasthit = self.menu_last.as_ref().and_then(|l| items.iter().position(|i| i == l)).unwrap_or(0) as i32;
        // the screen, for menuhit, is acme's area
        let screen = self.node.state.layout.r;
        let screenitem = (screen.dy() - 10) / ih;
        let (scrolling, nitemdrawn, wid, off, lasti) = if nitem > menu::MAXUNSCROLL || nitem > screenitem {
            let nitemdrawn = menu::NSCROLL.min(screenitem).max(1);
            let off = (lasthit - nitemdrawn / 2).clamp(0, (nitem - nitemdrawn).max(0));
            (true, nitemdrawn, maxwid + menu::GAP + menu::SCROLLWID, off, lasthit - off)
        } else {
            (false, nitem, maxwid, 0, lasthit)
        };
        let (mx, my) = self.row_pt(at);
        // r = insetrect(Rect(0,0,wid,n*ih), -Margin), moved so item lasti is centred on the pointer
        let mut r = tiling::Rect::new(-menu::MARGIN, -menu::MARGIN, wid + menu::MARGIN, nitemdrawn * ih + menu::MARGIN);
        let (dx, dy) = (mx - wid / 2, my - (lasti * ih + fh / 2));
        r = tiling::Rect::new(r.x0 + dx, r.y0 + dy, r.x1 + dx, r.y1 + dy);
        let mut px_ = 0;
        let mut py_ = 0;
        if r.x1 > screen.x1 {
            px_ = screen.x1 - r.x1;
        }
        if r.y1 > screen.y1 {
            py_ = screen.y1 - r.y1;
        }
        if r.x0 < screen.x0 {
            px_ = screen.x0 - r.x0;
        }
        if r.y0 < screen.y0 {
            py_ = screen.y0 - r.y0;
        }
        let menur = tiling::Rect::new(r.x0 + px_, r.y0 + py_, r.x1 + px_, r.y1 + py_);
        let textr = tiling::Rect::new(menur.x1 - menu::MARGIN - maxwid, menur.y0 + menu::MARGIN, menur.x1 - menu::MARGIN, menur.y0 + menu::MARGIN + nitemdrawn * ih);
        let scrollr = if scrolling { tiling::Rect::new(menur.x0 + menu::BORDER, menur.y0 + menu::BORDER, menur.x0 + menu::BORDER + menu::SCROLLWID, menur.y1 - menu::BORDER) } else { tiling::Rect::new(0, 0, 0, 0) };
        let m = menu::Menu { window: w, items, menur, textr, scrollr, scrolling, nitemdrawn, off, lasti, ih };
        // moveto: the pointer onto the item, so a click alone repeats it
        let ir = m.item_rect(lasti);
        let center = point(px(((ir.x0 + ir.x1) / 2) as f32), px(((ir.y0 + ir.y1) / 2) as f32 + self.top()));
        crate::warp::move_to(window, center);
        self.pointer = Some(center);
        self.last_mouse = center;
        self.menu = Some(m);
    }

    /// The pointer moved with the menu's button held: highlight what is
    /// under it, none outside; on the scroll bar, scroll.
    fn menu_track(&mut self, pos: Point<Pixels>) {
        let (x, y) = self.row_pt(pos);
        let Some(m) = self.menu.as_mut() else { return };
        let i = m.sel(x, y);
        if i >= 0 {
            m.lasti = i;
            return;
        }
        m.lasti = -1;
        if m.scrolling && m.scrollr.contains(x, y) {
            let nitem = m.items.len() as i32;
            let mut noff = ((y - m.scrollr.y0) * nitem) / m.scrollr.dy().max(1) - m.nitemdrawn / 2;
            noff = noff.clamp(0, (nitem - m.nitemdrawn).max(0));
            m.off = noff;
        }
    }

    /// The menu's button came up: the highlighted item runs.
    fn menu_up(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.menu.take() else { return };
        if m.lasti >= 0 {
            if let Some(item) = m.items.get((m.lasti + m.off) as usize).cloned() {
                self.menu_last = Some(item.clone());
                self.execute(ExecCtx::Window(m.window), &item, cx);
            }
        }
    }

    /// acme's `textcomplete`: the path fragment before `q0` goes to the
    /// server, which knows the file system; what comes back is inserted.
    fn complete(&mut self, v: ViewId, q0: usize) {
        let Some(t) = self.text_of(v) else { return };
        let mut q = q0;
        while q > 0 && is_file_char(t.char_at(q - 1)) {
            q -= 1;
        }
        let prefix = t.slice(q, q0);
        let ctx = self.ctx_of(v);
        match &mut self.backend {
            Backend::Local(server) => {
                let dir = server.dir_of(&self.node, ctx);
                let p = server.complete(v, q0, &dir, &prefix);
                perform(&mut self.node, &mut self.log, vec![p]);
            }
            Backend::Remote(link) => link.send(&ClientMsg::Complete { view: v, ctx, at: q0, prefix }),
        }
        self.after();
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
        if self.finder.is_some() {
            let ks = &e.keystroke;
            self.finder_key(&ks.key, ks.key_char.as_deref(), cx);
            return;
        }
        if self.selector.is_some() {
            let ks = &e.keystroke;
            self.selector_key(&ks.key, ks.key_char.as_deref(), window, cx);
            return;
        }
        let target = match self.locate(self.pointer(window)) {
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
            Target::View(v) => {
                // typing makes this the active column (acme's rowtype), but
                // scrolling does not
                if !matches!(ks.key.as_str(), "up" | "down" | "left" | "right" | "pageup" | "pagedown") {
                    self.node.activecol = self.column_of_view(v);
                }
                self.text_key(v, ks, cx)
            }
        }
        cx.notify();
    }

    /// The window acme would act on: the one under the pointer, else the
    /// last selected text's.
    fn window_at_pointer(&self, window: &Window) -> Option<WindowId> {
        match self.locate(self.pointer(window)) {
            Some((Target::View(v), _)) => v.window().or_else(|| self.node.seltext.and_then(|s| s.window())),
            Some((Target::Term(w, _), _)) => Some(w),
            None => self.node.seltext.and_then(|s| s.window()),
        }
    }

    /// A menu item that is an acme command: `Put`, `Del`, `New`, `Edit ,`
    /// run in the window under the pointer, as B2 there would.
    pub fn menu_command(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ctx = match self.window_at_pointer(window) {
            Some(w) => ExecCtx::Window(w),
            None if text == "New" || text == "Back" || text == "Fwd" => ExecCtx::Top,
            None => return,
        };
        self.execute(ctx, text, cx);
        cx.notify();
    }

    /// The window became active or inactive (cmd-` and friends): coming
    /// back, put the pointer where it last was here, wherever it is now.
    /// Unless a mouse button is down: then a click into the window is
    /// what activated it, and the pointer stays where the click was.
    pub fn window_activated(&mut self, active: bool, window: &mut Window) {
        if !active || self.last_mouse == Point::default() || crate::warp::button_down() {
            return;
        }
        let at = self.last_mouse;
        crate::warp::move_to(window, at);
        self.pointer = Some(at);
    }

    /// What the Edit menu (and its shortcuts, which arrive as actions
    /// before any key event) does: acme's rule, the text under the
    /// pointer, else the last selected text.
    pub fn menu_edit(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.locate(self.pointer(window)) {
            Some((t, _)) => t,
            None => match self.node.seltext {
                Some(v) => Target::View(v),
                None => return,
            },
        };
        match target {
            Target::Term(w, t) => match what {
                "paste" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        self.term_paste(t, text);
                    }
                }
                "copy" | "cut" => self.term_copy(w, cx),
                "select-all" => {
                    if let Some(l) = self.term_layouts.get(&w) {
                        let top = self.term_top(w);
                        self.term_sel = Some((w, (0, top), (l.cols as usize, top + l.rows.len().saturating_sub(1) as u64)));
                    }
                }
                _ => {}
            },
            Target::View(v) => {
                match what {
                    "undo" => {
                        let _ = self.node.undo(&mut self.log, v);
                    }
                    "redo" => {
                        let _ = self.node.redo(&mut self.log, v);
                    }
                    "cut" => self.cut(v, cx),
                    "copy" => self.snarf(v, cx),
                    "paste" => self.paste(v, cx),
                    "select-all" => {
                        if let Some(t) = self.text_of(v) {
                            let n = t.len();
                            let _ = self.node.select(&mut self.log, v, 0, n);
                        }
                    }
                    _ => {}
                }
                self.want_visible.insert(v);
            }
        }
        self.sync();
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
        // acme: newline is ignored in column tags and the top row
        if ks.key == "enter" && matches!(v, ViewId::ColTag(_) | ViewId::Top) {
            return;
        }
        // acme's ^F / Insert: complete the file name before the cursor
        if (m.control && ks.key == "f") || ks.key == "insert" {
            self.complete(v, q0);
            return;
        }
        let fit = self.layouts.get(&v).map(|l| l.lines_that_fit()).unwrap_or(1) as i64;
        // acme's texttype in a tag: Up shrinks it to one line, Down expands it
        if let ViewId::Tag(w) = v {
            match ks.key.as_str() {
                "up" | "down" => {
                    let on = ks.key == "down";
                    if self.node.state.window(w).map(|x| x.tagexpand != on).unwrap_or(false) {
                        let _ = self.node.append(&mut self.log, Shard::Window(w), Op::Window(WindowOp::TagExpand { on }));
                        let _ = self.node.refit_window(&mut self.log, w);
                    }
                    return;
                }
                _ => {}
            }
        }
        // acme: up/down scroll by a third of the window, page up/down by two thirds
        let scroll = match ks.key.as_str() {
            "up" if !m.control => Some(-(fit / 3).max(1)),
            "down" if !m.control => Some((fit / 3).max(1)),
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
                // acme's textbswidth rules, exactly
                "u" => {
                    let _ = self.node.erase(&mut self.log, v, Erase::Line);
                }
                "w" => {
                    let _ = self.node.erase(&mut self.log, v, Erase::Word);
                }
                "h" => {
                    let _ = self.node.erase(&mut self.log, v, Erase::Char);
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
                // acme inserts any other control character as itself: win
                // reads ^D and ^C out of the text
                k if k.len() == 1 && k.as_bytes()[0].is_ascii_lowercase() => {
                    let c = (k.as_bytes()[0] - b'a' + 1) as char;
                    self.type_text(v, &c.to_string());
                }
                _ => typed = false,
            }
        } else {
            match ks.key.as_str() {
                "backspace" => {
                    let _ = self.node.erase(&mut self.log, v, Erase::Char);
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
                    // acme: if the insertion point is above the view, show it; else go to the top
                    let org = self.node.state.buffer(self.node.view_buffer(v).unwrap_or(BufferId(0))).ok().and_then(|b| b.views.get(&v).map(|x| x.origin)).unwrap_or(0);
                    if org > q1 {
                        self.want_visible.insert(v);
                    } else {
                        let _ = self.node.select(&mut self.log, v, 0, 0);
                    }
                    typed = false;
                }
                "end" => {
                    // acme: if the insertion point is below the view, show it; else go to the end
                    let shown = self.layouts.get(&v).map(|l| l.first_line + l.lines.len()).unwrap_or(0);
                    let below = t.line_of(q1.min(t.len())) >= shown && shown > 0;
                    if below {
                        self.want_visible.insert(v);
                    } else {
                        let n = t.len();
                        let _ = self.node.select(&mut self.log, v, n, n);
                    }
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

    /// A line for the first column's `+Errors`: where the app tells the
    /// user things, since acme has no dialogs.
    pub fn notice(&mut self, msg: &str) {
        let _ = self.node.errors(&mut self.log, None, msg);
        self.after();
    }

    fn report(&mut self, ctx: ExecCtx, msg: &str) {
        let dir = match ctx {
            ExecCtx::Window(w) => self.node.error_dir(Some(w)),
            _ => None,
        };
        let _ = self.node.errors(&mut self.log, dir.as_deref(), &format!("{msg}\n"));
    }

    pub fn execute(&mut self, ctx: ExecCtx, text: &str, cx: &mut Context<Self>) {
        if let ExecCtx::Window(w) = ctx {
            let _ = self.node.commit_tag(&mut self.log, w);
        }
        let word = text.trim().split_whitespace().next().unwrap_or("").to_string();
        if word == "Snarf" {
            if let ExecCtx::Window(w) = ctx {
                if matches!(self.node.state.window(w).map(|x| x.body), Ok(Body::Term(_))) {
                    self.term_copy(w, cx);
                    return;
                }
            }
        }
        if word == "Paste" {
            if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: t }));
            }
        }
        match self.node.exec(&mut self.log, ctx, text) {
            Ok(Executed::Quit(_)) => match self.backend {
                Backend::Local(_) => cx.quit(),
                Backend::Remote(_) => self.close_requested = true, // Exit detaches; the session lives on
            },
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

    /// Is there somewhere to go back to, and a rule here that takes the
    /// Back verb (the lsp tool's)? Then shift-B3 is Back, not a reverse
    /// look.
    fn back_offered(&self, w: Option<WindowId>) -> bool {
        if self.node.state.layout.nav_back.is_empty() {
            return false;
        }
        let (name, kind) = match w {
            Some(w) => (self.node.window_name(w), self.node.window_kind(w)),
            None => (String::new(), WinKind::File),
        };
        apex_core::plumb::verbs_for(&self.node.state.meta.rules, &name, kind).iter().any(|v| v == "Back")
    }

    pub fn look(&mut self, ctx: ExecCtx, text: &str) {
        self.look_at(ctx, text, None, None, None, false);
    }

    /// B3: plumb `text` from `ctx`, saying where it came from when it
    /// came from a buffer (`at`: the pointer; `sel`: what was taken;
    /// `alt`: the word within it, tried when nothing takes the text).
    pub fn look_at(&mut self, ctx: ExecCtx, text: &str, at: Option<Span>, sel: Option<Span>, alt: Option<(String, Span)>, reverse: bool) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match &mut self.backend {
            Backend::Local(server) => {
                let req = PlumbReq { ctx, text: text.to_string(), dir: None, verb: "plumb".into(), edit_only: false, dry: false, exec: None, at, sel, alt, reverse };
                if let Some(w) = plumb_local(server, &mut self.node, &mut self.log, req) {
                    self.show(w);
                }
            }
            Backend::Remote(link) => link.send(&ClientMsg::Plumb { ctx, text: text.to_string(), dir: None, edit_only: false, dry: false, at, sel, alt, reverse }),
        }
        self.after();
    }

}

/// Run a program detached, for something the platform shows.
fn spawn_quiet(prog: &str, args: &[&str]) -> Result<std::process::Child, String> {
    std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{prog} {}: {e}", args.join(" ")))
}

/// A remote file being previewed from a local copy.
struct Live {
    copy: PathBuf,
    /// The previewer, when it is a process that lives as long as the
    /// preview (Quick Look); `open -a App` returns at once and is not.
    child: Option<std::process::Child>,
}

/// The extensions Preview is offered for: every `Preview.EXT` setting
/// this attachment sees (its own, then the session's).
fn preview_exts(meta: &apex_core::state::Meta, me: AttachmentId) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for owner in [me, SERVER] {
        if let Some(m) = meta.settings.get(&owner) {
            out.extend(m.keys().filter_map(|k| k.strip_prefix("Preview.")).filter(|e| !e.is_empty()).map(|e| e.to_lowercase()));
        }
    }
    out
}

/// The `--file` pattern of our Preview rule for an extension, and back.
fn pattern_of_ext(ext: &str) -> String {
    format!("(?i)\\.{}$", regex_escape(ext))
}

fn ext_of_pattern(p: &str) -> Option<String> {
    p.strip_prefix("(?i)\\.").and_then(|r| r.strip_suffix('$')).map(regex_unescape)
}

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn regex_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// What this client does when a rule asks it, in-process: `open` hands
/// the argument to the platform (`open` on macOS, `xdg-open` elsewhere),
/// `preview` shows a local file with the platform's previewer. Anything
/// else is refused, and the server tries the next rule.
pub fn client_do(verb: &str, args: &str) -> Result<(), String> {
    match verb {
        "open" => spawn_quiet(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }, &[args]).map(|_| ()),
        "preview" => open_preview(None, Path::new(args)).map(|_| ()),
        _ => Err(format!("apex-ui cannot {verb}")),
    }
}

/// Show `path` with `app`, or with the platform's previewer: Quick Look
/// on macOS, `xdg-open` elsewhere. Returns the previewer's process when
/// it lives as long as the preview does.
fn open_preview(app: Option<&str>, path: &Path) -> Result<Option<std::process::Child>, String> {
    let p = path.to_string_lossy().to_string();
    match app {
        Some(app) if cfg!(target_os = "macos") => spawn_quiet("open", &["-a", app, &p]).map(|_| None),
        Some(app) => spawn_quiet(app, &[&p]).map(Some),
        None if cfg!(target_os = "macos") => spawn_quiet("qlmanage", &["-p", &p]).map(Some),
        None => spawn_quiet("xdg-open", &[&p]).map(|_| None),
    }
}

/// A local copy of a remote file for previewing, under this client's
/// temporary directory, keeping the host's path so neighbours can be
/// fetched beside it later.
fn preview_copy(url: &SessionUrl, path: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let host: String = format!("{}-{}", url.provider, url.arg).chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    let copy = std::env::temp_dir().join("apex-preview").join(host).join(path.trim_start_matches('/'));
    if let Some(d) = copy.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(&copy, bytes).map_err(|e| format!("{}: {e}", copy.display()))?;
    Ok(copy)
}

/// Walk the rules in-process (no daemon): the client answers for itself,
/// and there are no tools to ask.
fn plumb_local(server: &mut Server, node: &mut Node, log: &mut Log, req: PlumbReq) -> Option<WindowId> {
    let (id, mut step) = server.plumb_start(node, req);
    loop {
        match step {
            PlumbStep::Done(props) => return perform(node, log, props),
            PlumbStep::Trace(_) => return None,
            PlumbStep::Ask(apex_server::Proposal::ClientDo { verb, args }) => {
                let r = client_do(&verb, &args);
                step = server.plumb_next(node, id, r);
            }
            PlumbStep::Ask(p) => {
                perform(node, log, vec![p]);
                step = server.plumb_next(node, id, Ok(()));
            }
            PlumbStep::AskTool { tool, .. } => step = server.plumb_next(node, id, Err(format!("no tool {tool} in-process"))),
        }
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

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn preview_extensions_come_from_direct_settings_only() {
        let mut meta = apex_core::state::Meta::default();
        let me = AttachmentId(7);
        meta.settings.entry(SERVER).or_default().insert("Preview".into(), "Quick".into());
        meta.settings.entry(SERVER).or_default().insert("Preview.html".into(), "Safari".into());
        meta.settings.entry(me).or_default().insert("Preview.MD".into(), "Marked".into());
        let exts: Vec<String> = preview_exts(&meta, me).into_iter().collect();
        assert_eq!(exts, vec!["html", "md"]);
        assert_eq!(ext_of_pattern(&pattern_of_ext("c++")).as_deref(), Some("c++"));
        assert_eq!(pattern_of_ext("md"), r"(?i)\.md$");
    }
}
