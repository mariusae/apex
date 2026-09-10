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
use apex_server::proto::{ClientMsg, FileFrame, IoFrame};
use apex_server::providers::SessionUrl;
use apex_server::remote::{Link, Wake};
use apex_server::{PlumbReq, PlumbStep, Proposal, perform, Server, ServerEvent, TermKey};

use crate::shell::{Selector, TITLEBAR_HEIGHT};
use crate::menu;
use crate::text_element::font_for;

use crate::term_element::TermLayout;
use crate::web::{Nav, WebEvent, Webs};
use crate::pool::{Parked, Pool, WakeTarget};
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
    /// The wheel's fraction of a line not yet scrolled, per target: a
    /// trackpad's small deltas add up instead of being dropped.
    wheel_rest: Option<(Target, f32)>,
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
    /// B3 with command held: `Def` at the pointer (with shift, `Back`).
    b3_cmd: bool,
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
    /// Where the current link's wake goes: this window, until the
    /// session is parked.
    pub wake_target: Option<WakeTarget>,
    pub selector: Option<Selector>,
    /// A tab held with B1: a click until it moves, a drag reordering
    /// the tabs after that.
    pub tab_drag: Option<crate::shell::TabDrag>,
    /// Where the tabs were drawn last frame, for the drag to know
    /// which one the pointer has passed.
    pub tab_bounds: std::rc::Rc<std::cell::RefCell<Vec<(SessionUrl, gpui::Bounds<Pixels>)>>>,
    /// ctrl-tab held: the session switcher.
    pub switcher: Option<crate::switcher::Switcher>,
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
    /// Texts programs put on the clipboard (OSC 52), to be written once
    /// there is a context to write with.
    clips: Vec<String>,
    /// A B2/B3 sweep in a terminal, shown in the button's colour.
    pub term_hl: Option<(WindowId, MouseButton, (usize, u64), (usize, u64))>,
    /// The window is full screen: no title bar, acme's area from the top.
    pub fullscreen: bool,
    /// Positions to bring on screen (new `+Errors` text), by view.
    show_at: HashMap<ViewId, (usize, usize)>,
    /// A place to go once its file is open (asked of the server).
    pending_goto: Option<Loc>,
    /// A place in another session to go to: the next render switches.
    pub pending_switch: Option<Loc>,
    /// A page reported a cursor: apply it on the next tick.
    page_cursor_now: bool,
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
    /// path, app, the watch stream).
    previews: Vec<(u64, String, Option<String>, u32)>,
    /// Remote files being previewed: subscribed, their copies kept current.
    live: std::collections::HashMap<String, Live>,
    /// The heartbeat: when the last ping went out, and how long the last
    /// one took to come back.
    last_ping: Option<std::time::Instant>,
    ping_ms: Option<u64>,
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
    /// The native views of web windows (WEB.md §2).
    pub webs: Webs,
    pub hl: Option<(ViewId, usize, usize, HlKind)>,
    mouse: Mouse,
    want_visible: HashSet<ViewId>,
    typed_start: HashMap<ViewId, usize>,
    /// acme's `iq1`: where the last typing in each view ended, shifted
    /// by a program's output before it; Home and End go back to it.
    iq1: HashMap<ViewId, usize>,
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
        // parked here (a window closed on it, say): shown at once
        if let Some(p) = Pool::take(cx, url) {
            let mut acme = Self::from_parked(cx, p, wake.clone());
            acme.open_initial(acme.node.state.layout.cols.first().map(|c| c.id).unwrap_or(ColumnId(0)), files);
            return Ok(acme);
        }
        let (link, mut log, mut node, target) = Self::connect_targeted(url, wake.clone())?;
        let col = match node.state.layout.cols.first() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        let url = &identified(url, &node);
        let mut acme = Self::over(cx, log, node, Backend::Remote(link), &url.session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = url.clone();
        acme.wake = Some(wake);
        acme.wake_target = Some(target);
        crate::shell::note_recent(url);
        Pool::note_open(cx, url);
        acme.open_initial(col, files);
        Ok(acme)
    }

    /// A window on a parked session: its state as it was left, its wake
    /// pointed here.
    fn from_parked(cx: &mut Context<Self>, p: Parked, wake: Wake) -> Acme {
        p.target.set(wake.clone());
        let mut acme = Self::over(cx, p.log, p.node, Backend::Remote(p.link), &p.url.session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = p.url.clone();
        acme.wake = Some(wake);
        acme.wake_target = Some(p.target);
        acme.previews = p.previews;
        acme.live = p.live;
        acme.snarfouts = p.snarfouts;
        acme.pending_goto = p.pending_goto;
        crate::shell::note_recent(&acme.url.clone());
        Pool::note_open(cx, &acme.url.clone());
        acme
    }

    /// Take this window's session off it, still attached, for the pool:
    /// what a switch or a close does. The window is left on nothing (a
    /// blank in-process log) until it shows something else. None when
    /// there is no link to keep.
    pub fn park(&mut self) -> Option<Parked> {
        if !matches!(self.backend, Backend::Remote(_)) || !self.connected {
            return None;
        }
        let target = self.wake_target.take()?;
        // a blank stand-in, as an offline window has
        let mut log = Log::new();
        let (a, _) = log.attach(AttachmentKind::Ui, "apex");
        let mut node = Node::new(a);
        let _ = node.catch_up(&log);
        let _ = node.init_session(&mut log);
        let (server, _rx) = Server::new(&log);
        let Backend::Remote(link) = std::mem::replace(&mut self.backend, Backend::Local(server)) else { unreachable!() };
        let log = std::mem::replace(&mut self.log, log);
        let node = std::mem::replace(&mut self.node, node);
        self.connected = false;
        self.webs = Webs::new(None, None);
        self.layouts.clear();
        self.term_layouts.clear();
        self.hl = None;
        self.mouse = Mouse::default();
        self.want_visible.clear();
        self.typed_start.clear();
        Some(Parked {
            link,
            log,
            node,
            url: self.url.clone(),
            target,
            previews: std::mem::take(&mut self.previews),
            live: std::mem::take(&mut self.live),
            snarfouts: std::mem::take(&mut self.snarfouts),
            pending_goto: self.pending_goto.take(),
            parked_at: std::time::Instant::now(),
        })
    }

    /// `End`: end this window's session on its host, off the UI thread,
    /// then close the window; a refusal (unsaved windows) is reported.
    fn end_session(&mut self, force: bool, cx: &mut Context<Self>) {
        let Backend::Remote(_) = &self.backend else {
            self.notice("End: an in-process session has no daemon to end\n");
            return;
        };
        let url = self.url.clone();
        let socket = apex_server::daemon::default_socket();
        let ending = cx.background_executor().spawn(async move {
            if url.is_local() {
                apex_server::remote::end_session(&socket, url.session_ref(), force).map_err(|e| e.to_string())
            } else {
                let dest = url.dest().unwrap_or_default();
                let f = if force { " -f" } else { "" };
                apex_server::providers::run(&dest, &format!("{} end-session{f} {}", apex_server::providers::REMOTE_BIN, url.session_ref()), None).map(|_| ()).map_err(|e| e.to_string())
            }
        });
        let url = self.url.clone();
        cx.spawn(async move |this, cx| {
            let r = ending.await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |acme, cx| {
                    match r {
                        Ok(()) if acme.url == url => {
                            crate::shell::log_line(&format!("ended {url}: closing the window"));
                            acme.connected = false; // nothing to park
                            acme.close_now(cx);
                        }
                        Ok(()) => {}
                        Err(e) => acme.notice(&format!("End: {e}\n")),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Park this window's session in the pool (the window is closing).
    pub fn park_into_pool(&mut self, cx: &mut gpui::App) {
        if let Some(p) = self.park() {
            Pool::park(cx, p);
        }
    }

    /// Close this window now: the session parked, the window gone, without
    /// waiting for a frame (a window nobody can see never draws one).
    pub fn close_now(&mut self, cx: &mut Context<Self>) {
        self.park_into_pool(cx);
        let mine = cx.entity_id();
        cx.defer(move |cx| {
            for h in cx.windows() {
                if let Some(h) = h.downcast::<Acme>() {
                    let _ = h.update(cx, |_, window, cx| {
                        if cx.entity_id() == mine {
                            window.remove_window();
                        }
                    });
                }
            }
        });
    }

    /// Show the session at `url` in this window: parked, it is back at
    /// once; local, it is attached now; elsewhere, the window says it is
    /// attaching and the attach comes back from a thread. What this
    /// window showed is parked first.
    /// The current tab's ×: this session let go (its link closes, its
    /// tab goes), the window showing the session parked most recently.
    pub fn close_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prev) = Pool::most_recent(cx) else { return };
        let leaving = self.url.clone();
        self.switch_to(&prev, window, cx);
        Pool::let_go(cx, &leaving);
        cx.notify();
    }

    /// cmd-N: the Nth tab.
    pub fn go_to_tab(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tabs = Pool::tabs(cx, &self.url);
        if let Some(u) = tabs.get(n.saturating_sub(1)) {
            if *u != self.url {
                let u = u.clone();
                self.switch_to(&u, window, cx);
                cx.notify();
            }
        }
    }

    /// cmd-shift-k: back to the session parked most recently.
    pub fn previous_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Pool::most_recent(cx) {
            Some(url) => {
                self.switch_to(&url, window, cx);
                cx.notify();
            }
            None => self.notice("no previous session\n"),
        }
    }

    /// A place in another session (a Goto or Switch named it): switch
    /// to that session, then land there once it is up. The session is
    /// named by identity; its label is what we knew, or a stub the
    /// attach corrects.
    pub fn switch_for(&mut self, loc: Loc, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = loc.session.clone() else { return };
        if !self.node.elsewhere(&loc) {
            self.goto(Loc { session: None, ..loc });
            return;
        }
        // the session as we know it: by id, a prefix of it, or its label
        let here = crate::shell::Host::of(&self.url);
        let known = crate::shell::known_sessions().get(&here).and_then(|v| v.iter().find(|s| s.id == id || s.id.starts_with(&id) || s.label == id).cloned());
        let url = match known {
            Some(s) => here.url_of(&s),
            None if apex_server::providers::valid_label(&id).is_ok() => here.url(&id),
            None => here.url(&id.chars().take(8).collect::<String>()).with_id(&id),
        };
        self.switch_to(&url, window, cx);
        if !loc.name.is_empty() {
            self.pending_goto = Some(Loc { session: None, ..loc });
        }
        cx.notify();
    }

    pub fn switch_to(&mut self, url: &SessionUrl, window: &mut Window, cx: &mut Context<Self>) {
        let wake = self.wake.clone();
        if let Some(p) = self.park() {
            Pool::park(cx, p);
        }
        if let Some(p) = Pool::take(cx, url) {
            self.adopt_parked(p, window);
            Pool::note_open(cx, &self.url.clone());
            return;
        }
        if url.is_local() {
            if let Err(e) = self.reattach(url, window) {
                eprintln!("apex-ui: attach {url}: {e}");
                let msg = Acme::connect_error(url, &e);
                self.notice(&msg);
            }
            return;
        }
        let Some(wake) = wake else { return };
        self.url = url.clone();
        self.session = url.session.clone();
        window.set_window_title(&Self::title(url));
        self.notice(&format!("{}: attaching…\n", url.describe()));
        crate::shell::log_line(&format!("attaching to {url} in the background"));
        let (u, w) = (url.clone(), wake);
        let connecting = cx.background_executor().spawn(async move { Acme::connect_targeted(&u, w) });
        let url = url.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = connecting.await;
            let _ = this.update_in(cx, |acme, window, cx| {
                match r {
                    Ok((link, log, node, target)) => {
                        if acme.url == url && !acme.connected {
                            if let Err(e) = acme.adopt(link, log, node, target, &url, Vec::new(), window) {
                                acme.notice(&Acme::connect_error(&url, &e));
                            } else {
                                crate::shell::log_line(&format!("attached to {url}"));
                            }
                        } else {
                            // the window moved on meanwhile: keep it, parked
                            let parked = Parked { link, log, node, url: url.clone(), target, previews: Vec::new(), live: std::collections::HashMap::new(), snarfouts: Vec::new(), pending_goto: None, parked_at: std::time::Instant::now() };
                            Pool::park(cx, parked);
                        }
                    }
                    Err(e) => {
                        crate::shell::log_line(&format!("attach {url}: {e}"));
                        if acme.url == url {
                            acme.notice(&Acme::connect_error(&url, &e));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A parked session back in this window.
    fn adopt_parked(&mut self, p: Parked, window: &mut Window) {
        if let Some(wake) = &self.wake {
            p.target.set(wake.clone());
        }
        self.backend = Backend::Remote(p.link);
        self.connected = true;
        self.last_ping = None;
        self.log = p.log;
        self.node = p.node;
        self.session = p.url.session.clone();
        self.url = p.url.clone();
        self.wake_target = Some(p.target);
        self.previews = p.previews;
        self.live = p.live;
        self.snarfouts = p.snarfouts;
        self.pending_goto = p.pending_goto;
        self.socket = Some(apex_server::daemon::default_socket());
        self.chooser = false;
        self.webs = Webs::new(self.io_plane(), self.wake.clone());
        self.selector = None;
        window.set_window_title(&Self::title(&p.url));
        crate::shell::note_recent(&p.url);
        self.sync();
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
            win: None, to: None,
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
                win: None, to: None,
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


    /// What rules asked this client to do since the last poll: `open`
    /// and `preview`, answered when done. A preview of a remote file
    /// first asks the host for its bytes.
    fn answer_asks(&mut self) {
        let Backend::Remote(link) = &mut self.backend else { return };
        let asks = std::mem::take(&mut link.client_asks);
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
                        let stream = self.io_open("GET", &apex_server::remote::file_url(&args), &[("Watch", "1")]);
                        self.previews.push((id, args.clone(), app, stream));
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
        self.preview_frames();
        self.end_stale_previews();
    }

    /// What the watch streams of previews brought: the first bytes open
    /// the copy, later ones keep it current; a refusal answers the ask.
    fn preview_frames(&mut self) {
        let Backend::Remote(link) = &mut self.backend else { return };
        let frames = std::mem::take(&mut link.io);
        for (stream, frame) in frames {
            if let Some(i) = self.previews.iter().position(|(_, _, _, s)| *s == stream) {
                match frame {
                    IoFrame::Response { status, .. } if status == 200 => {}
                    IoFrame::Response { status, .. } => {
                        let (id, path, _, _) = self.previews.remove(i);
                        self.send(ClientMsg::Applied { id, result: Err(format!("{path}: {status}")) });
                    }
                    IoFrame::Body(b) => {
                        let (id, path, app, stream) = self.previews.remove(i);
                        let result = FileFrame::decode(&b).ok_or_else(|| "preview: a bad frame".to_string()).and_then(|f| {
                            let copy = preview_copy(&self.url, &path, &f.bytes)?;
                            let child = open_preview(app.as_deref(), &copy)?;
                            self.live.insert(path.clone(), Live { copy, stream, child });
                            Ok(())
                        });
                        if result.is_err() {
                            self.send(ClientMsg::Io { stream, frame: IoFrame::End });
                        }
                        self.send(ClientMsg::Applied { id, result: result.map(|_| None) });
                    }
                    IoFrame::End | IoFrame::Reset { .. } => {
                        let (id, path, _, _) = self.previews.remove(i);
                        self.send(ClientMsg::Applied { id, result: Err(format!("{path}: the host ended the stream")) });
                    }
                    IoFrame::Request { .. } => {}
                }
            } else if let Some(live) = self.live.values().find(|l| l.stream == stream) {
                // a change: the copy follows, and the previewer sees it
                if let IoFrame::Body(b) = frame {
                    if let Some(f) = FileFrame::decode(&b) {
                        let _ = std::fs::write(&live.copy, f.bytes);
                    }
                }
            }
        }
    }

    /// Open a stream on the I/O plane; its id.
    fn io_open(&mut self, method: &str, url: &str, headers: &[(&str, &str)]) -> u32 {
        match &mut self.backend {
            Backend::Remote(link) => link.io_open(method, url, headers),
            Backend::Local(_) => 0,
        }
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
            if let Some(live) = self.live.remove(&path) {
                self.send(ClientMsg::Io { stream: live.stream, frame: IoFrame::End });
            }
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
    /// `connect`, with the wake behind a target that can move: to this
    /// window now, to the pool when the session is parked.
    fn connect_targeted(url: &SessionUrl, wake: Wake) -> std::io::Result<(Link, Log, Node, WakeTarget)> {
        let target = WakeTarget::new(wake);
        let (link, log, node) = Self::connect(url, target.forwarding())?;
        Ok((link, log, node, target))
    }

    fn connect(url: &SessionUrl, wake: Wake) -> std::io::Result<(Link, Log, Node)> {
        let (mut link, log, node) = Self::connect_link(url, wake)?;
        Self::arm(&mut link);
        Ok((link, log, node))
    }

    fn connect_link(url: &SessionUrl, wake: Wake) -> std::io::Result<(Link, Log, Node)> {
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
                    url.session_ref(),
                    &url.session,
                    "apex",
                    AttachmentKind::Ui,
                    Some(wake),
                )
            }
            Some(dest) => {
                apex_server::providers::deploy(&dest)?;
                let cmd = apex_server::providers::attach_command(&dest, url.session_ref())?;
                Self::connect_via_link(&cmd, url.session_ref(), &url.session, wake)
            }
        }
    }

    /// Attach to a session that must already be there (a tab of last
    /// time): nothing is made when it is gone.
    pub(crate) fn connect_existing_targeted(url: &SessionUrl, wake: Wake) -> std::io::Result<(Link, Log, Node, WakeTarget)> {
        let target = WakeTarget::new(wake);
        let wake = target.forwarding();
        let (mut link, log, node) = match url.dest() {
            None => {
                let socket = apex_server::daemon::default_socket();
                crate::shell::ensure_daemon(&socket)?;
                Link::connect(&socket, url.session_ref(), "apex", AttachmentKind::Ui, Some(wake))?
            }
            Some(dest) => {
                apex_server::providers::deploy(&dest)?;
                let cmd = apex_server::providers::attach_command(&dest, url.session_ref())?;
                let (stdin, stdout, closer) = apex_server::remote::bridge_child(&cmd)?;
                Link::over_streams(Box::new(stdout), Box::new(stdin), Some(closer), url.session_ref(), "apex", AttachmentKind::Ui, Some(wake))?
            }
        };
        Self::arm(&mut link);
        Ok((link, log, node, target))
    }

    /// Attach through a command's stdin and stdout. Every link this
    /// client makes is armed with its rules here or in `connect`: a
    /// session shown later from the pool has them too.
    pub fn connect_via(cmd: &str, session: &str, label: &str, wake: Wake) -> std::io::Result<(Link, Log, Node)> {
        let (mut link, log, node) = Self::connect_via_link(cmd, session, label, wake)?;
        Self::arm(&mut link);
        Ok((link, log, node))
    }

    fn connect_via_link(cmd: &str, session: &str, label: &str, wake: Wake) -> std::io::Result<(Link, Log, Node)> {
        let (stdin, stdout, closer) = apex_server::remote::bridge_child(cmd)?;
        Link::over_streams_creating(Box::new(stdout), Box::new(stdin), Some(closer), session, label, "apex", AttachmentKind::Ui, Some(wake))
    }

    /// Attach through an arbitrary command (`--via`).
    pub fn attach_via(cx: &mut Context<Self>, cmd: &str, session: &str, files: Vec<String>, wake: Wake) -> std::io::Result<Acme> {
        let (link, mut log, mut node) = Self::connect_via(cmd, session, session, wake.clone())?;
        let col = match node.state.layout.cols.first() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        let mut acme = Self::over(cx, log, node, Backend::Remote(link), session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = SessionUrl { provider: "via".into(), arg: cmd.split_whitespace().nth(1).unwrap_or("?").to_string(), session: session.to_string(), id: None };
        acme.wake = Some(wake);
        acme.open_initial(col, files);
        Ok(acme)
    }

    /// Re-point this window at the session at `url` (the selector). The
    /// old attachment ends; its leases return to its daemon.
    pub fn reattach(&mut self, url: &SessionUrl, window: &mut Window) -> std::io::Result<()> {
        let wake = self.wake.clone().ok_or_else(|| std::io::Error::other("no wake"))?;
        let (link, log, node, target) = Self::connect_targeted(url, wake)?;
        self.adopt(link, log, node, target, url, Vec::new(), window)
    }

    /// Take a fresh link (made by `connect`, on any thread) as this
    /// window's: the second half of `reattach`, and what an attach made
    /// in the background comes back to.
    pub fn adopt(&mut self, link: Link, mut log: Log, mut node: Node, target: WakeTarget, url: &SessionUrl, files: Vec<String>, window: &mut Window) -> std::io::Result<()> {
        if let Some(old) = self.wake_target.replace(target) {
            drop(old);
        }
        let col = match node.state.layout.cols.first() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        self.backend = Backend::Remote(link);
        self.connected = true;
        self.last_ping = None;
        self.log = log;
        self.node = node;
        let url = &identified(url, &self.node);
        self.session = url.session.clone();
        self.url = url.clone();
        // a window that started offline is one to remember now
        self.socket = Some(apex_server::daemon::default_socket());
        self.chooser = false;
        self.layouts.clear();
        self.term_layouts.clear();
        self.webs = Webs::new(self.io_plane(), self.wake.clone());
        self.hl = None;
        self.mouse = Mouse::default();
        self.want_visible.clear();
        self.typed_start.clear();
        self.selector = None;
        // a new attachment: its rules are installed afresh, and previews
        // of the old session are over
        self.previews.clear();
        self.live.clear();
        window.set_window_title(&Self::title(url));
        crate::shell::note_recent(url);
        self.open_initial(col, files);
        Ok(())
    }

    /// `connect`, for a thread: a remote attach can take a while (the
    /// binary uploaded when it changed, a daemon started there) and must
    /// not hold the UI meanwhile.
    pub fn connect_blocking(url: &SessionUrl, wake: Wake) -> std::io::Result<(Link, Log, Node, WakeTarget)> {
        Self::connect_targeted(url, wake)
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
            Ok(()) => self.notice(&format!("{}: reconnected\n", url.describe())),
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
            format!("{}: {e}: Reconnect (⌘⇧R)\n", url.describe())
        } else {
            format!("{}: {e}\n", url.describe())
        }
    }

    /// Rename this window's session on its daemon.
    pub fn rename_session(&mut self, to: &str, window: &mut Window) {
        let from = self.url.session_ref().to_string();
        if to.is_empty() || to == self.session {
            return;
        }
        self.send(ClientMsg::RenameSession { from, to: to.to_string() });
        let old = self.url.clone();
        self.session = to.to_string();
        self.url = self.url.with_session(to);
        window.set_window_title(&Self::title(&self.url));
        crate::shell::renamed_recent(&old, &self.url);
    }

    pub fn title(url: &SessionUrl) -> String {
        format!("{} — apex", url.describe())
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
        if answered {
            if let (Some(sent), Some(pong)) = (self.last_ping, link.last_pong) {
                if pong >= sent {
                    self.ping_ms = Some(pong.duration_since(sent).as_millis() as u64);
                }
            }
        }
        if answered || overdue {
            link.send(&ClientMsg::Ping { t: now.elapsed().as_millis() as u64 });
            self.last_ping = Some(now);
        }
        changed
    }

    /// What the titlebar shows of the link: the heartbeat's round trip
    /// and the log's (an entry flushed to its Ack), in milliseconds.
    pub fn latency(&self) -> Option<String> {
        let Backend::Remote(link) = &self.backend else { return None };
        if !self.connected {
            return None;
        }
        let ping = self.ping_ms.map(|m| format!("{m}ms")).unwrap_or_else(|| "—".into());
        let log = link.ack_ms.map(|m| format!("{m}ms")).unwrap_or_else(|| "—".into());
        Some(format!("{ping}/{log}"))
    }

    fn over(cx: &mut Context<Self>, log: Log, node: Node, backend: Backend, session: &str) -> Acme {
        // keys follow the pointer over pages too: a native view keeps the
        // pointer's moves to itself, so the system is asked, often
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
            let alive = cx.update(|cx| {
                let mine = this.entity_id();
                let mut alive = false;
                for h in cx.windows() {
                    if let Some(h) = h.downcast::<Acme>() {
                        let _ = h.update(cx, |acme, window, cx| {
                            if cx.entity_id() == mine {
                                alive = true;
                                if acme.web_focus_tick(window) {
                                    cx.notify();
                                }
                            }
                        });
                    }
                }
                alive || this.upgrade().is_some()
            });
            if !alive {
                break;
            }
        })
        .detach();
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
            wake_target: None,
            selector: None,
            tab_drag: None,
            tab_bounds: Default::default(),
            switcher: None,
            tag_need: HashMap::new(),
            close_requested: false,
            pending: None,
            title_shown: String::new(),
            connected: true,
            last_ping: None,
            ping_ms: None,
            term_sel: None,
            snarf_wanted: None,
            clips: Vec::new(),
            term_hl: None,
            fullscreen: false,
            show_at: HashMap::new(),
            pending_goto: None,
            pending_switch: None,
            page_cursor_now: false,
            finder: None,
            last_windows: std::collections::BTreeMap::new(),
            chooser: false,
            menu: None,
            menu_last: None,
            snarfouts: Vec::new(),
            previews: Vec::new(),
            live: std::collections::HashMap::new(),
            warp_wait: false,
            mouse_saved: None,
            pointer: None,
            last_mouse: Point::default(),
            focus: cx.focus_handle(),
            layouts: HashMap::new(),
            term_layouts: HashMap::new(),
            webs: Webs::new(None, None),
            hl: None,
            mouse: Mouse::default(),
            want_visible: HashSet::new(),
            typed_start: HashMap::new(),
            iq1: HashMap::new(),
        }
    }

    /// Bring the client node up to date with the log (the server appends
    /// terminal rows and metalog entries).
    /// Also ships whatever this node sequenced since the last call: this
    /// runs after every input handler and on every frame, so nothing the
    /// user typed is ever more than a frame away from the daemon.
    pub fn sync(&mut self) {
        self.web_events();
        // acme's winsettag: Undo/Redo/Put/Get come and go with the state
        let _ = self.node.update_tags(&mut self.log);
        self.track_closed();
        for (v, q) in self.node.take_shows() {
            self.show_at.insert(v, (q, 1));
        }
        for loc in self.node.take_gotos() {
            self.goto(loc);
        }
        // places in other sessions: the render switches (it has the window)
        if let Some(loc) = self.node.take_switches().pop() {
            self.pending_switch = Some(loc);
        }
        // a place whose file was being opened: land once it is
        if let Some(loc) = self.pending_goto.clone() {
            if let Some(w) = self.node.window_named(&loc.name) {
                self.pending_goto = None;
                let _ = self.node.land(&mut self.log, &loc);
                self.want_visible.insert(ViewId::Body(w));
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
        // over a page the pointer's moves never reach gpui: ask the system
        if !self.webs.is_empty() {
            if let Some(p) = crate::web::native_mouse(window) {
                if self.webs.window_at(p).is_some() {
                    return p;
                }
            }
        }
        self.pointer.unwrap_or_else(|| window.mouse_position())
    }

    /// Keys follow the pointer between pages and the rest (WEB.md §2.2);
    /// what the pages reported is taken; true when something should be
    /// drawn again (a page is loading: its handle pulses).
    pub fn web_focus_tick(&mut self, window: &Window) -> bool {
        if self.webs.is_empty() {
            return false;
        }
        self.web_events();
        if !self.overlay_up() {
            self.webs.focus_tick(window);
        }
        // the pointer over a page: the page's cursor, set by us (WebKit's
        // own never shows inside this window), when it changed
        if self.page_cursor_now {
            self.page_cursor_now = false;
            if let Some(p) = crate::web::native_mouse(window) {
                if let Some(w) = self.webs.window_at(p) {
                    crate::cursor::apply(self.webs.cursor(w));
                }
            }
        }
        self.webs.any_loading()
    }

    /// Is the pointer over a page? Then gpui's cursor rect must say
    /// nothing, and the page's cursor stands.
    pub fn over_page(&self, window: &Window) -> bool {
        !self.webs.is_empty() && crate::web::native_mouse(window).is_some_and(|p| self.webs.window_at(p).is_some())
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
            self.clips.extend(server.take_clips());
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
        let ended = link.ended.take();
        self.clips.append(&mut link.clips);
        if self.connected && !alive {
            crate::shell::log_line(&format!("link to {} ended", self.url));
        }
        self.connected = alive;
        // a place in another session (a Goto or Switch just applied):
        // the tick switches, whether or not the window is being drawn
        if let Some(loc) = self.node.take_switches().pop() {
            self.pending_switch = Some(loc);
        }
        // the label follows a rename made anywhere (the metalog says)
        let (id, label) = (self.node.state.meta.id.clone(), self.node.state.meta.label.clone());
        if !id.is_empty() && self.url.id.as_deref() == Some(id.as_str()) && !label.is_empty() && self.url.session != label {
            self.url.session = label.clone();
            self.session = label;
            crate::shell::note_recent(&self.url);
        }
        let made = link.take_made();
        let outputs = link.take_outputs();
        for w in made {
            self.show(w);
        }
        // acme's rule for a program's output (xfidwrite's shouldscroll):
        // a window follows it when the point it went in at was on screen,
        // shown three quarters down as for a win; scrolled away, it stays
        for (b, at, end) in outputs {
            let views: Vec<ViewId> = self.node.state.windows.iter().filter(|(_, x)| x.body_buffer() == Some(b)).map(|(w, _)| ViewId::Body(*w)).collect();
            for v in views {
                // output before the insertion point moves it along
                if let Some(p) = self.iq1.get_mut(&v) {
                    if at < *p {
                        *p += end - at;
                    }
                }
                // on screen: from the origin, within the lines the window
                // holds, judged on the text as it is now (the last paint's
                // layout predates the newline just typed, and the chunk of
                // output before this one); or brought on screen by that
                // earlier chunk's show, still pending
                let origin = self.node.view_buffer(v).ok().and_then(|b| self.node.state.buffer(b).ok()).map(|b| b.view(v).origin).unwrap_or(0);
                let fits = self.layouts.get(&v).map(|l| (f32::from(l.bounds.size.height) / f32::from(l.line_height)).floor().max(1.) as usize).unwrap_or(0);
                let in_window = self.text_of(v).is_some_and(|t| at >= origin && t.line_of(at.min(t.len())) < t.line_of(origin) + fits);
                let pending = self.show_at.get(&v).is_some_and(|(q, _)| at <= *q);
                if in_window || pending {
                    self.show_at.insert(v, (end, 3));
                }
            }
        }
        if let Some(name) = ended {
            // the session was ended under us: the window says so, offline
            self.connected = false;
            self.notice(&format!("session {name} ended\n"));
        }
        self.answer_asks();
        // what the proposals just applied left to do: places to go (a
        // tool's Goto opens a file), tags to refit, the warp
        self.sync();
        // an Exit proposed from outside (apex exec Exit) closes the window
        if std::mem::take(&mut self.node.quit_requested) {
            crate::shell::log_line(&format!("Exit from outside: closing the window on {}", self.url));
            self.close_requested = true;
        }
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
        self.term_wheel(t, delta, None)
    }

    /// The wheel at a cell (`at`), which the program may be reading.
    fn term_wheel(&mut self, t: TermId, delta: isize, at: Option<(u16, u16)>) {
        match &mut self.backend {
            Backend::Local(server) => server.term_wheel(&mut self.log, t, delta, at),
            Backend::Remote(link) => link.send(&ClientMsg::TermScroll { term: t, delta: delta as i64, at }),
        }
        self.sync();
    }

    // ---- what the elements read ------------------------------------------

    pub fn source(&mut self, view: ViewId) -> Option<Source> {
        let b = self.node.view_buffer(view).ok()?;
        let buf = self.node.state.buffer(b).ok()?;
        let v = buf.view(view);
        let (mono, dirty, stale, live, pulse) = match view {
            ViewId::Body(w) | ViewId::Tag(w) => {
                let win = self.node.state.window(w).ok()?;
                let body = win.body_buffer().and_then(|b| self.node.state.buffer(b).ok());
                let dirty = body.is_some_and(|b| b.dirty());
                let stale = body.is_some_and(|b| b.stale && b.dirty());
                // a page is live as a terminal is; while it loads, its
                // handle breathes between live and pale
                let web = win.body == Body::Web;
                let pulse = if web && self.webs.loading(w) {
                    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) % 1200;
                    let t = ms as f32 / 1200.0;
                    Some(if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 })
                } else {
                    None
                };
                (win.mono, dirty, stale, web || self.node.window_live(w), pulse)
            }
            _ => (false, false, false, false, None),
        };
        let hl = self.hl.and_then(|(hv, lo, hi, k)| if hv == view { Some((lo, hi, k)) } else { None });
        Some(Source {
            kind: Kind::of(view),
            mono,
            dirty,
            stale,
            live,
            pulse,
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
        // a click in acme's part of the window takes the keyboard back
        // from any page that had it
        if !self.webs.is_empty() {
            crate::web::focus_ui(window);
        }
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
                            self.mouse.b3_cmd = Self::b3_cmd(e);
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
                (Region::Term(..), MouseButton::Right) if self.mouse.term_drag.is_some() => {
                    // B1+B3 in a terminal: the clipboard typed into the shell
                    self.mouse.chorded = true;
                    self.term_paste_clipboard(w, cx);
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
        if let Some(d) = &mut self.tab_drag {
            // a tab held: past a few pixels it is a drag; the tab floats
            // under the pointer and passes a neighbour once it covers
            // the whole of it: its far edge past the neighbour's far
            // edge (measured against the neighbours' places with it out
            // of the row, so a swap cannot undo itself; overlapping one
            // partly changes nothing, whichever is the wider)
            d.pos = pos;
            if !d.moved && ((d.start.x - pos.x).abs() > px(4.) || (d.start.y - pos.y).abs() > px(4.)) {
                d.moved = true;
            }
            if d.moved {
                let (url, grab, width) = (d.url.clone(), d.grab, d.width);
                let mut all: Vec<(SessionUrl, gpui::Bounds<Pixels>)> = self.tab_bounds.borrow().clone();
                all.sort_by(|a, b| a.1.origin.x.partial_cmp(&b.1.origin.x).unwrap_or(std::cmp::Ordering::Equal));
                let gap = if all.len() >= 2 { all[1].1.origin.x - (all[0].1.origin.x + all[0].1.size.width) } else { px(4.) };
                let (gl, gr) = (pos.x - grab, pos.x - grab + width);
                // the others' slots with the dragged tab out of the row
                let mut left = all.first().map(|(_, b)| b.origin.x).unwrap_or(px(0.));
                let mut slots: Vec<(SessionUrl, Pixels, Pixels)> = Vec::new();
                for (u, b) in all.iter().filter(|(u, _)| *u != url) {
                    slots.push((u.clone(), left, left + b.size.width));
                    left += b.size.width + gap;
                }
                // where it is now among them, then past each neighbour it
                // has come to occupy: rightwards once its left edge is past
                // the neighbour's left edge, leftwards once its right edge
                // is past the neighbour's right; a narrower tab, which can
                // sit wholly inside a neighbour, goes by its centre there
                // (the two tests exclude each other, so the walk ends and
                // a swap cannot undo itself as the pointer moves on)
                let c = gl + width / 2.;
                let mut k = all.iter().position(|(u, _)| *u == url).unwrap_or(slots.len());
                for _ in 0..=slots.len() {
                    if k < slots.len() && gl >= slots[k].1 && c >= (slots[k].1 + slots[k].2) / 2. {
                        k += 1;
                    } else if k > 0 && gr <= slots[k - 1].2 && c < (slots[k - 1].1 + slots[k - 1].2) / 2. {
                        k -= 1;
                    } else {
                        break;
                    }
                }
                let before = slots.get(k).map(|(u, _, _)| u.clone());
                crate::pool::Pool::move_tab(cx, &url, before.as_ref());
            }
            cx.notify();
            self.last_mouse = pos;
            return;
        }
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
        // acme: a chord ends the sweep; the cut's insertion point (or the
        // paste's selection) stays, whatever the pointer does before release
        if let Some(d) = self.mouse.b1.filter(|_| !self.mouse.chorded) {
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

    pub fn mouse_up(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.tab_drag.take() {
            // let go without moving: the click it was (the current tab
            // toggles the picker; another is switched to)
            if !d.moved {
                if d.current {
                    if self.selector.is_some() {
                        self.close_selector(cx);
                    } else {
                        self.open_selector(cx);
                    }
                } else {
                    self.switch_to(&d.url, window, cx);
                }
            }
            cx.notify();
            return;
        }
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
                    // B3 on an OSC 8 link plumbs the link, not its text;
                    // on a URL, the whole URL (a file word stops at `?`)
                    (None, _) => self.term_link(w, cell.0, cell.1).or_else(|| self.term_url(w, cell.0, cell.1)).or_else(|| self.term_word(w, cell.0, cell.1, is_file_char)),
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
                    // what was swept or selected under the pointer, else a
                    // click: the server expands from the pointer as acme's
                    // look3 does, where the files are (`sel` says which)
                    let explicit = self.explicit_range_at(d);
                    let found = explicit.clone().or_else(|| self.take_range_at(d, HlKind::Look));
                    self.hl = None;
                    if let Some((text, (lo, hi))) = found {
                        let b = self.node.view_buffer(d.view).ok();
                        let at = b.map(|b| Span { buffer: b, q0: d.anchor, q1: d.anchor });
                        let sel = b.filter(|_| explicit.is_some()).map(|b| Span { buffer: b, q0: lo, q1: hi });
                        let alt = None;
                        let reverse = self.mouse.b3_reverse;
                        let ctx = self.ctx_of(d.view);
                        if self.mouse.b3_cmd && reverse {
                            // shift-cmd-B3: Back, as cmd-[ issues it
                            self.execute(ctx, "Back", cx);
                        } else if self.mouse.b3_cmd {
                            // cmd-B3: Def at the pointer (the lsp's rule)
                            self.look_at(ctx, &text, at, sel, alt, false, Some("Def"));
                        } else {
                            // B3 looks; shift-B3 looks backwards
                            self.look_at(ctx, &text, at, sel, alt, reverse, None);
                        }
                    }
                }
            }
            MouseButton::Navigate(_) => self.menu_up(cx),
        }
        cx.notify();
    }

    /// plan9port chords: while B1 is held, option cuts and command pastes.
    pub fn modifiers_changed(&mut self, e: &ModifiersChangedEvent, window: &mut Window, cx: &mut Context<Self>) {
        let prev = self.mouse.mods;
        self.mouse.mods = e.modifiers;
        // control let go with the switcher up: the session under the mark
        if self.switcher.is_some() && !e.modifiers.control {
            self.switcher_commit(window, cx);
            return;
        }
        if let Some(w) = self.mouse.term_drag {
            // in a terminal: option copies (nothing to cut), command pastes
            if e.modifiers.alt && !prev.alt {
                self.mouse.chorded = true;
                self.term_copy(w, cx);
                cx.notify();
            } else if e.modifiers.platform && !prev.platform {
                self.mouse.chorded = true;
                self.term_paste_clipboard(w, cx);
                cx.notify();
            }
            return;
        }
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

    /// The clipboard typed into a terminal (the B1+B3 chord there, as
    /// pasting into text): the snarf buffer gets it too.
    fn term_paste_clipboard(&mut self, w: WindowId, cx: &mut Context<Self>) {
        let Some(t) = self.term_of(w) else { return };
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        if text.is_empty() {
            return;
        }
        let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: text.clone() }));
        self.term_paste(t, text);
        self.after();
    }

    fn take_range(&mut self, d: Drag, kind: HlKind) -> Option<String> {
        self.take_range_at(d, kind).map(|(t, _)| t)
    }

    /// `take_range`, with where the text came from.
    /// What a button took on purpose: the sweep, or the selection the
    /// pointer is in. None for a plain click.
    fn explicit_range_at(&self, d: Drag) -> Option<(String, (usize, usize))> {
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
        None
    }

    fn take_range_at(&mut self, d: Drag, kind: HlKind) -> Option<(String, (usize, usize))> {
        if let Some(r) = self.explicit_range_at(d) {
            return Some(r);
        }
        let t = self.text_of(d.view)?;
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

    /// A URL under a terminal cell: the run of non-blank text there,
    /// from its `http://` or `https://` on, less any punctuation that
    /// closes the sentence around it.
    fn term_url(&self, w: WindowId, c: usize, r: usize) -> Option<String> {
        let row = self.term_layouts.get(&w)?.rows.get(r)?;
        let chars: Vec<char> = row.chars().collect();
        if c >= chars.len() || chars[c].is_whitespace() {
            return None;
        }
        let mut a = c;
        while a > 0 && !chars[a - 1].is_whitespace() {
            a -= 1;
        }
        let mut b = c + 1;
        while b < chars.len() && !chars[b].is_whitespace() {
            b += 1;
        }
        let run: String = chars[a..b].iter().collect();
        let start = ["https://", "http://"].iter().filter_map(|s| run.find(s)).min()?;
        let url = run[start..].trim_end_matches(|ch: char| ".,;:!?)>]}'\"".contains(ch));
        (url.len() > start && url.contains("://") && url.split("://").nth(1).is_some_and(|rest| !rest.is_empty())).then(|| url.to_string())
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
        let Some(d) = self.mouse.b1.filter(|_| !self.mouse.chorded) else { return false };
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
        // OSC 52 from a terminal: the last text set wins
        if let Some(text) = self.clips.drain(..).last() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
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
        if self.node.elsewhere(&loc) {
            self.pending_switch = Some(loc);
            return;
        }
        match self.node.land(&mut self.log, &loc) {
            Ok(Some(w)) => {
                self.want_visible.insert(ViewId::Body(w));
            }
            _ => {
                let Some(col) = self.node.state.layout.cols.first().map(|c| c.id) else { return };
                if apex_core::is_url(&loc.name) {
                    // a page: a web window, here and now (we lead)
                    perform(&mut self.node, &mut self.log, vec![Proposal::OpenWeb { col, url: loc.name.clone() }]);
                    if let Ok(Some(w)) = self.node.land(&mut self.log, &loc) {
                        self.want_visible.insert(ViewId::Body(w));
                    }
                    return;
                }
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

    /// A web window's body landed at `bounds`: its native view goes there
    /// (built on the window's name, the URL, the first time), hidden
    /// while a gpui overlay would be under it.
    pub fn web_place(&mut self, w: WindowId, bounds: gpui::Bounds<Pixels>, window: &Window) {
        let name = self.node.window_name(w);
        if name.is_empty() {
            return;
        }
        if self.webs.is_empty() && !self.webs.armed() {
            // the first view: the plane and the proxy come from the link now
            self.webs = Webs::new(self.io_plane(), self.wake.clone());
        }
        let visible = !self.overlay_up();
        if std::env::var_os("APEX_WEB_DEBUG").is_some() {
            eprintln!("web: place {w} {name} at {bounds:?} body {:?}", self.node.state.window(w).map(|x| x.body));
        }
        match self.node.state.window(w).map(|x| x.body) {
            Ok(Body::Html(b)) => {
                // the buffer's HTML as a page, following its every version
                let Ok(buf) = self.node.state.buffer(b) else { return };
                let (text, version) = (buf.text.to_string(), buf.version);
                let dir = std::path::Path::new(&name).parent().map(|d| d.display().to_string()).unwrap_or_default();
                self.webs.place_html(w, &text, version, &dir, bounds, window, visible);
                // a preview follows dot in its source (WEB.md §3.3)
                if let Some(line) = self.preview_source_line(&name) {
                    self.webs.follow_line(w, line);
                }
            }
            _ => self.webs.place(w, &name, bounds, window, visible),
        }
    }

    /// For a window named `FILE+Preview`: the line (from 1) dot is on in
    /// FILE's window, when it is open.
    fn preview_source_line(&self, name: &str) -> Option<usize> {
        let source = name.strip_suffix("+Preview")?;
        let w = self.node.state.windows.keys().copied().find(|w| self.node.window_name(*w) == source)?;
        let b = self.node.state.window(w).ok()?.body_buffer()?;
        let buf = self.node.state.buffer(b).ok()?;
        let (q0, _) = self.node.selection(ViewId::Body(w)).ok()?;
        Some(buf.text.line_of(q0.min(buf.text.len())) + 1)
    }

    /// The session's I/O plane for the web views' threads, when a link
    /// carries one.
    fn io_plane(&self) -> Option<apex_server::plane::IoPlane> {
        match &self.backend {
            Backend::Remote(link) => Some(link.io_plane()),
            Backend::Local(_) => None,
        }
    }

    /// Is a gpui overlay up that a native view would hide?
    fn overlay_up(&self) -> bool {
        self.menu.is_some() || self.finder.is_some() || self.selector.is_some()
    }

    /// What the pages did: a navigation moves the window's name and the
    /// navigation stack (`WebNavigate`); titles are not kept yet.
    fn web_events(&mut self) {
        if self.webs.is_empty() {
            return;
        }
        for (w, ev) in self.webs.drain() {
            match ev {
                WebEvent::Navigated(url) => {
                    self.webs.navigated(w, &url);
                    if self.node.window_name(w) != url {
                        perform(&mut self.node, &mut self.log, vec![Proposal::WebNavigate { window: w, url }]);
                    }
                }
                WebEvent::Title(_) => {}
                WebEvent::Reload => self.webs.reload(w),
                // a link followed in a page of ours: a web window on it
                WebEvent::Link(url) => self.goto(Loc { session: None, name: url, pos: Pos::Keep }),
                // a file link with a line: the file, at that line
                WebEvent::Open(path, line) => self.goto(Loc { session: None, name: path, pos: line.map(Pos::Line).unwrap_or(Pos::Keep) }),
                WebEvent::Loading(on) => self.webs.set_loading(w, on),
                // the page's cursor: set now, if the pointer is on that page
                WebEvent::Cursor(css) => {
                    self.webs.set_cursor(w, &css);
                    self.page_cursor_now = true;
                }
                // the host's loopback, by its bare name: through the proxy
                WebEvent::Reroute(url) => {
                    self.webs.load(w, &url);
                    if self.node.window_name(w) != url {
                        perform(&mut self.node, &mut self.log, vec![Proposal::WebNavigate { window: w, url }]);
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
            MouseButton::Right // control as well: cmd-B3
        } else if e.modifiers.shift {
            MouseButton::Navigate(gpui::NavigationDirection::Back)
        } else {
            MouseButton::Left
        }
    }

    /// Command held on B3 (a real one), or control on the click that
    /// stands in for it (cmd-click), so cmd-B3 can be typed on a laptop.
    fn b3_cmd(e: &MouseDownEvent) -> bool {
        if e.button == MouseButton::Left {
            e.modifiers.platform && e.modifiers.control
        } else {
            e.modifiers.platform
        }
    }

    /// B4 on a window: the tools menu (libdraw's `menuhit`, as the
    /// mariusae/plan9port acme uses it): the verbs the rules offer this
    /// window, popped up so the last one chosen is under the pointer,
    /// which is warped onto it; tracked while the button is held; the
    /// item under the pointer on release runs as B2 would, none if it is
    /// released outside.
    fn menu_open(&mut self, w: WindowId, at: Point<Pixels>, window: &mut Window) {
        let items = apex_core::plumb::verbs_for(&self.node.state.meta.rules, &self.node.window_name(w), self.node.window_kind(w), Some(w));
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
        let Some((target, region)) = self.locate(e.position) else { return };
        let lh = match target {
            Target::Term(w, _) => self.term_layouts.get(&w).map(|l| l.line_height),
            Target::View(v) => self.layouts.get(&v).map(|l| l.line_height),
        };
        let Some(lh) = lh else { return };
        let lines = match e.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y / lh,
        };
        let rest = match self.mouse.wheel_rest {
            Some((t, r)) if t == target => r,
            _ => 0.,
        };
        let total = -f32::from(lines) + rest;
        let n = total.round() as i64;
        self.mouse.wheel_rest = Some((target, total - n as f32));
        if n == 0 {
            return;
        }
        match target {
            Target::View(v) => self.scroll_by(v, n),
            Target::Term(_, t) => {
                // the cell under the pointer: a program reading the mouse
                // gets the wheel there
                let at = match region {
                    Region::Term(c, r) => Some((c as u16, r as u16)),
                    _ => None,
                };
                self.term_wheel(t, n as isize, at);
            }
        }
        cx.notify();
    }

    // ---- keyboard ------------------------------------------------------------

    /// Keys go to the text under the pointer, as in acme.
    pub fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // ctrl-tab: the session switcher, stepped while control is held
        {
            let ks = &e.keystroke;
            if ks.modifiers.control && ks.key == "tab" {
                self.switcher_step(ks.modifiers.shift, cx);
                return;
            }
            if self.switcher.is_some() {
                if ks.key == "escape" {
                    self.close_switcher(cx);
                }
                return;
            }
        }
        if self.finder.is_some() {
            let ks = &e.keystroke;
            self.finder_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, cx);
            return;
        }
        if self.selector.is_some() {
            let ks = &e.keystroke;
            self.selector_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, window, cx);
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
        let pos = self.pointer(window);
        if let Some(w) = self.webs.window_at(pos) {
            return Some(w); // a page: the window is its
        }
        match self.locate(pos) {
            Some((Target::View(v), _)) => v.window().or_else(|| self.node.seltext.and_then(|s| s.window())),
            Some((Target::Term(w, _), _)) => Some(w),
            None => self.node.seltext.and_then(|s| s.window()),
        }
    }

    /// A menu item that is an acme command: `Put`, `Del`, `New`, `Edit ,`
    /// run in the window under the pointer, as B2 there would.
    pub fn menu_command(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.selector.is_some() || self.finder.is_some() {
            // an overlay has the keyboard: select all is its field's
            if text == "Edit ," {
                self.overlay_edit("select-all", cx);
            }
            return;
        }
        // select all over a page: the page's, as Copy and Paste are
        if text == "Edit ," {
            if let Some(w) = crate::web::native_mouse(window).and_then(|p| self.webs.window_at(p)) {
                if self.webs.edit(w, "select-all") {
                    return;
                }
            }
        }
        let ctx = match self.window_at_pointer(window) {
            Some(w) => ExecCtx::Window(w),
            None if text == "New" || text.starts_with("New ") || text == "Back" || text == "Fwd" => ExecCtx::Top,
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
        // an overlay (the session picker, the finder) has the keyboard:
        // the Edit menu works on its field, not on the text below
        if self.selector.is_some() || self.finder.is_some() {
            self.overlay_edit(what, cx);
            return;
        }
        // the pointer over a page: the page's selection is what Copy
        // means, and Paste goes into its field
        if let Some(w) = crate::web::native_mouse(window).and_then(|p| self.webs.window_at(p)) {
            if self.webs.edit(w, what) {
                return;
            }
        }
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
                "enter" => {
                    // acme -a, always: the new line starts with the whitespace
                    // the one before it starts with, up to dot
                    let indent: String = match v {
                        ViewId::Body(_) => {
                            let start = t.line_start(t.line_of(q0.min(t.len())));
                            t.slice(start, q0).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
                        }
                        _ => String::new(),
                    };
                    self.type_text(v, &format!("\n{indent}"));
                }
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
                    // acme's Khome: the last insertion point scrolled off
                    // above (a win's output ran past the prompt typed at)
                    // comes back with its line at the top; else the top of
                    // the text. The selection stays where it is.
                    let iq1 = self.iq1.get(&v).copied().unwrap_or(0).min(t.len());
                    let org = self.node.state.buffer(self.node.view_buffer(v).unwrap_or(BufferId(0))).ok().and_then(|b| b.views.get(&v).map(|x| x.origin)).unwrap_or(0);
                    if org > iq1 {
                        self.set_origin(v, t.line_start(t.line_of(iq1.saturating_sub(1))));
                    } else {
                        self.set_origin(v, 0);
                    }
                    return;
                }
                "end" => {
                    // acme's Kend: the last insertion point scrolled off
                    // below comes back with its line at the top; else the
                    // end of the text is shown. The selection stays.
                    let iq1 = self.iq1.get(&v).copied().unwrap_or(0).min(t.len());
                    let shown_end = self.layouts.get(&v).and_then(|l| l.lines.last()).map(|l| l.end + l.has_newline as usize);
                    if shown_end.is_some_and(|e| iq1 > e) {
                        self.set_origin(v, t.line_start(t.line_of(iq1.saturating_sub(1))));
                    } else {
                        let quarters = if v.window().is_some_and(|w| self.node.window_live(w)) { 3 } else { 1 };
                        self.show_at.insert(v, (t.len(), quarters));
                    }
                    return;
                }
                _ => match &ks.key_char {
                    Some(s) if !s.is_empty() => self.type_text(v, s),
                    _ => typed = false,
                },
            }
        }
        // typing and erasing leave the insertion point (acme's texttype
        // sets iq1 after each); moving about does not
        if typed || ks.key == "backspace" {
            if let Ok((q0, _)) = self.node.selection(v) {
                self.iq1.insert(v, q0);
            }
        }
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
        // End: the session ended (as apex end-session does), the window closed
        if word == "End" {
            let force = text.split_whitespace().any(|w| w == "-f");
            self.end_session(force, cx);
            return;
        }
        // Send in a terminal, as win's: the selection (swept with B1),
        // typed into the shell with a newline; without one, the server
        // sends the snarf buffer
        if word == "Send" {
            if let ExecCtx::Window(w) = ctx {
                if let (Some(t), Some((sw, a, b))) = (self.term_of(w), self.term_sel) {
                    if sw == w && a != b {
                        if let Some(mut text) = self.term_grid_text(w, a, b) {
                            let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: text.clone() }));
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            if !text.ends_with('\n') {
                                text.push('\n');
                            }
                            self.term_paste(t, text);
                            self.after();
                            return;
                        }
                    }
                }
            }
        }
        // a page's own history and reload: Back, Fwd, Get in a web window
        if let ExecCtx::Window(w) = ctx {
            if self.node.state.window(w).map(|x| x.body) == Ok(Body::Web) {
                let nav = match word.as_str() {
                    "Back" => Some(Nav::Back),
                    "Fwd" => Some(Nav::Fwd),
                    "Get" => Some(Nav::Reload),
                    _ => None,
                };
                if let Some(nav) = nav {
                    self.webs.go(w, nav);
                    return;
                }
            }
        }
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
                Backend::Remote(_) => self.close_now(cx), // Exit: the session lives on, parked
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

    pub fn look(&mut self, ctx: ExecCtx, text: &str) {
        self.look_at(ctx, text, None, None, None, false, None);
    }

    /// B3: plumb `text` from `ctx`, saying where it came from when it
    /// came from a buffer (`at`: the pointer; `sel`: what was taken;
    /// `alt`: the word within it, tried when nothing takes the text).
    pub fn look_at(&mut self, ctx: ExecCtx, text: &str, at: Option<Span>, sel: Option<Span>, alt: Option<(String, Span)>, reverse: bool, verb: Option<&str>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match &mut self.backend {
            Backend::Local(server) => {
                let req = PlumbReq { ctx, text: text.to_string(), dir: None, verb: verb.unwrap_or("plumb").into(), edit_only: false, dry: false, exec: None, at, sel, alt, reverse };
                if let Some(w) = plumb_local(server, &mut self.node, &mut self.log, req) {
                    self.show(w);
                }
            }
            Backend::Remote(link) => link.send(&ClientMsg::Plumb { ctx, text: text.to_string(), dir: None, edit_only: false, dry: false, at, sel, alt, reverse, verb: verb.map(String::from) }),
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
pub struct Live {
    copy: PathBuf,
    /// The watch stream on the I/O plane, ended with the preview.
    stream: u32,
    /// The previewer, when it is a process that lives as long as the
    /// preview (Quick Look); `open -a App` returns at once and is not.
    child: Option<std::process::Child>,
}

pub fn client_do(verb: &str, args: &str) -> Result<(), String> {
    match verb {
        "open" => spawn_quiet(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }, &[args]).map(|_| ()),
        "preview" => open_preview(None, Path::new(args)).map(|_| ()),
        _ => Err(format!("apex-ui cannot {verb}")),
    }
}

fn open_preview(app: Option<&str>, path: &Path) -> Result<Option<std::process::Child>, String> {
    let p = path.to_string_lossy().to_string();
    match app {
        Some(app) if cfg!(target_os = "macos") => spawn_quiet("open", &["-a", app, &p]).map(|_| None),
        Some(app) => spawn_quiet(app, &[&p]).map(Some),
        None if cfg!(target_os = "macos") => spawn_quiet("qlmanage", &["-p", &p]).map(Some),
        None => spawn_quiet("xdg-open", &[&p]).map(|_| None),
    }
}

fn preview_copy(url: &SessionUrl, path: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let host: String = format!("{}-{}", url.provider, url.arg).chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    let copy = std::env::temp_dir().join("apex-preview").join(host).join(path.trim_start_matches('/'));
    if let Some(d) = copy.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(&copy, bytes).map_err(|e| format!("{}: {e}", copy.display()))?;
    Ok(copy)
}

fn plumb_local(server: &mut Server, node: &mut Node, log: &mut Log, req: PlumbReq) -> Option<WindowId> {
    let (id, mut step) = server.plumb_start(node, req);
    loop {
        match step {
            PlumbStep::Done(props) | PlumbStep::Refused { props, .. } => return perform(node, log, props),
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

/// The URL with what the session's metalog says it is: its identity,
/// and its label as it is now.
fn identified(url: &SessionUrl, node: &Node) -> SessionUrl {
    let (id, label) = (&node.state.meta.id, &node.state.meta.label);
    if id.is_empty() {
        return url.clone();
    }
    let mut u = url.clone().with_id(id);
    if !label.is_empty() {
        u.session = label.clone();
    }
    u
}
