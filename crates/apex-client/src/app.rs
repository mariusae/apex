//! The apex client: acme's interaction model (three-button mouse, chords,
//! keyboard to the text under the pointer) over the core's state. Every
//! change goes through the leader node as log entries. The server either
//! runs in-process and shares the log, or sits behind a socket: the
//! client's code path is the same, only the [`Backend`] differs.

use std::collections::{HashMap, HashSet};

use gpui::{
    point, px, Bounds, ClipboardItem, TouchPhase, Context, FocusHandle, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
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

use crate::shell::Selector;
use crate::menu;
use crate::text_element::font_for;

use crate::term_element::TermLayout;
use crate::web::{Nav, WebEvent, Webs};
use crate::pool::{Parked, Pool, TabId, WakeTarget};
use crate::text_element::{Atom, Head, Source, TextLayout};

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
    /// The line on a column's left, dragged to make the columns wider
    /// or narrower.
    Edge(ColumnId),
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
    row: i32,
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
    // a tag's further lines a body's line each: its pad is once, the tag's
    fn tag_row(&self) -> i32 {
        self.row
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
    /// A part of a window tag's head: its path's, its label, a verb.
    Atom(Atom),
    Scrollbar,
    LayoutBox,
    Term(usize, usize),
    TermScrollbar,
    /// The scrollbar drawn beside a page (a web window, a preview).
    WebScrollbar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    View(ViewId),
    Term(WindowId, TermId),
    /// A page's window, where apex draws on it: its scrollbar.
    Web(WindowId),
}

/// A body's scroll between whole rows, and past its ends (`Acme::smooth`).
/// It steps across rows, not lines: a line that wraps to a screen of them
/// scrolls through as any other text does. (`scroll` is written in lines
/// of any height; it is given rows, each a line of one height.)
#[derive(Clone, Copy, Debug)]
struct Smooth {
    /// The origin this is about: the session's, at the top row. When that
    /// moves by anything else (the scrollbar, a key, a jump, another
    /// client), this is dropped and the body is at its row again.
    origin: usize,
    /// Pixels scrolled down into the top row, less than its height.
    px: f32,
    /// How far past an end the text is pulled: down past the start when
    /// positive, up past the last line when negative. Springs back to 0
    /// once no finger holds it.
    over: f32,
    /// How fast the pull is changing, in pixels of pull (before the rubber
    /// band) a second: the spring's, once no finger holds it.
    vel: f32,
    /// The view's height, which the rubber band never reaches.
    h: f32,
    /// A finger is on the trackpad: what it pulls past an end stays pulled.
    finger: bool,
    /// The momentum since the finger lifted met an end, down past the
    /// last line when negative, up past the start when positive: what more
    /// of it comes that way is spent, even once the bounce is over. (A
    /// scroll the other way is not momentum, and moves the text.)
    spent: f32,
}

/// The spring that brings a body back past an end: critically damped, so
/// that it goes out and back once and never rings; at this rate a bounce
/// is back in about half a second.
const SPRING: f32 = 13.;

impl Smooth {
    fn at(origin: usize) -> Smooth {
        Smooth { origin, px: 0., over: 0., vel: 0., h: 1., finger: false, spent: 0. }
    }

    /// Scrolled `dy` pixels down (up when negative), `dt` seconds after the
    /// last scroll, `phase` saying what the finger is doing, from `line` of
    /// `total`: the new scroll and the new first line. `below` are the
    /// heights of the lines from the first down, `above` of those above it
    /// nearest first (either short, a line is `lh`); `h` is the view's
    /// height.
    ///
    /// The end is where a native view's is: the text's last line at the
    /// bottom of the view (or, text shorter than the view, at its start),
    /// not acme's last line at the top. `below` reaching the last line
    /// says where that is; a body already past it (the scrollbar takes it
    /// further) goes no further down, and bounces.
    ///
    /// Past an end, a finger pulls against the rubber band. Without one,
    /// the pull is the spring's alone: the system's momentum carries on
    /// sending scrolls after the finger lifts, and the spring and they
    /// taking turns at the pull (at whatever rates each comes, which
    /// differ from display to display) shakes the text. So momentum that
    /// reaches an end is handed to the spring as its speed, and what more
    /// of it comes is spent until a finger is down again.
    #[allow(clippy::too_many_arguments)]
    fn scroll(mut self, dy: f32, phase: TouchPhase, dt: f32, mut line: usize, total: usize, below: &[f32], above: &[f32], lh: f32, h: f32) -> (Smooth, usize) {
        self.h = h;
        match phase {
            TouchPhase::Started => {
                self.finger = true;
                self.spent = 0.;
                self.vel = 0.;
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.finger = false;
                // let go past an end: the spring has it from here
                self.spent = if self.over != 0. { self.over.signum() } else { 0. };
            }
            TouchPhase::Moved => {}
        }
        if !self.finger && (self.over != 0. || self.vel != 0. || self.spent * dy < 0.) {
            return (self, line);
        }
        let mut d = dy;
        // moving back towards the text: the pull goes first
        if self.over > 0. && d > 0. || self.over < 0. && d < 0. {
            let raw = unrubber(self.over, h);
            let left = raw - d;
            if left.signum() == raw.signum() && left != 0. {
                self.over = rubber(left, h);
                d = 0.;
            } else {
                self.over = 0.;
                d = -left;
            }
        }
        // past an end by `past` (down past the start when positive)
        let mut past = 0.;
        // down: no further than the end of the text at the bottom
        if d > 0. && line + below.len() >= total {
            let rest: f32 = below.iter().sum();
            let room = (rest - self.px - h).max(0.);
            if d > room {
                past -= d - room;
                d = room;
            }
        }
        self.px += d;
        // down, across whole lines, to the last line at the top
        let mut k = 0;
        while self.px > 0. && line + 1 < total {
            let hk = below.get(k).copied().unwrap_or(lh).max(1.);
            if self.px < hk {
                break;
            }
            self.px -= hk;
            line += 1;
            k += 1;
        }
        if line + 1 >= total && self.px > 0. {
            past -= self.px;
            self.px = 0.;
        }
        // up, across the lines above, to the start
        let mut k = 0;
        while self.px < 0. && line > 0 {
            self.px += above.get(k).copied().unwrap_or(lh).max(1.);
            line -= 1;
            k += 1;
        }
        if self.px < 0. {
            past -= self.px;
            self.px = 0.;
        }
        if past != 0. {
            if self.finger {
                self.over = rubber(unrubber(self.over, h) + past, h);
            } else {
                // momentum: its speed, the scroll over the time it took
                self.vel = (-dy / dt.clamp(1. / 240., 1. / 30.)).clamp(-6000., 6000.);
                self.spent = past.signum();
            }
        }
        (self, line)
    }

    /// `dt` seconds of the spring: what no finger holds goes out as far as
    /// its speed takes it and comes back. False once it is back.
    fn settle(&mut self, dt: f32) -> bool {
        if self.finger || self.over == 0. && self.vel == 0. {
            return false;
        }
        let h = self.h.max(1.);
        let mut x = unrubber(self.over, h);
        let mut v = self.vel;
        let from = if x != 0. { x.signum() } else { v.signum() };
        // in steps of a millisecond, however long the frame was
        let mut left = dt.clamp(0., 0.1);
        while left > 0. {
            let step = left.min(0.001);
            v += (-SPRING * SPRING * x - 2. * SPRING * v) * step;
            x += v * step;
            left -= step;
        }
        if (x.abs() < 0.5 && v.abs() < 20.) || x.signum() == -from {
            self.over = 0.;
            self.vel = 0.;
            return false;
        }
        self.over = rubber(x, h);
        self.vel = v;
        true
    }
}

/// Where B2 on a body's scrollbar `frac` of the way down takes it, as
/// acme's does: as far into the text as the pointer is down the bar, by
/// the rune, moved on to the start of the next line when one is near,
/// and else left where it is, in the middle of a line.
fn bar_origin(t: &Text, frac: f32) -> usize {
    let len = t.len();
    let p = ((len as f64 * frac.clamp(0., 1.) as f64) as usize).min(len);
    if p == 0 || t.char_at(p - 1) == '\n' {
        return p;
    }
    (p..(p + 256).min(len)).find(|&q| t.char_at(q) == '\n').map_or(p, |nl| nl + 1)
}

/// The rubber band past an end, as AppKit's: `raw` pixels of pull show as
/// ever less the further it goes, never as much as `h`, the view's height.
fn rubber(raw: f32, h: f32) -> f32 {
    (1. - 1. / (raw.abs() * 0.55 / h + 1.)) * h * raw.signum()
}

/// `rubber`'s inverse: the pull an overscroll stands for.
fn unrubber(over: f32, h: f32) -> f32 {
    let x = (over.abs() / h).min(0.999);
    (1. / (1. - x) - 1.) * h / 0.55 * over.signum()
}

impl Target {
    /// The window this is part of, if any (not the session's own row).
    fn window(self) -> Option<WindowId> {
        match self {
            Target::View(v) => v.window(),
            Target::Term(w, _) | Target::Web(w) => Some(w),
        }
    }
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
    /// The tab this window shows. The app's own name for it: the session
    /// behind it can turn out to be another one (`pool::Pool::open`),
    /// and the tab is still this tab.
    pub tab: TabId,
    pub wake: Option<Wake>,
    /// Where the current link's wake goes: this window, until the
    /// session is parked.
    pub wake_target: Option<WakeTarget>,
    pub selector: Option<Selector>,
    /// The view the keys go to, whose caret is the blue one that blinks
    /// (none when apex is not in front, or the keys go to a terminal or
    /// a page); whether that caret shows just now; and since when it has
    /// been left alone -- a key, a click or the pointer coming to it
    /// start it again from solid.
    pub caret_view: Option<ViewId>,
    /// The terminal the keys go to, whose cursor blinks with the caret.
    pub caret_term: Option<TermId>,
    pub caret_on: bool,
    /// The keys' caret on its way to where it moved (View ▸ Smooth Cursor).
    pub caret_glide: Option<crate::text_element::CaretGlide>,
    pub caret_since: std::time::Instant,
    /// Where the overlays (the picker, the finder, the tools menu) were
    /// drawn this frame: holes cut in the web views,
    /// which are native views above everything gpui paints, so the
    /// overlays show through them and the pages stay live around them.
    /// Each overlay's bounds this frame, and how far past them its hole
    /// in the pages reaches (its shadow's room).
    pub overlay_bounds: std::rc::Rc<std::cell::RefCell<Vec<(gpui::Bounds<Pixels>, Pixels)>>>,
    /// When each notification shown came, as this client first saw it
    /// (`sync_notes`): a raised one again is a new ping.
    pub noted: std::collections::HashMap<(WindowId, Seq), std::time::Instant>,
    /// A look going on in a tag (`look.rs`), and each window's Look word's
    /// places, found once a version.
    pub looking: Option<crate::look::Live>,
    pub look_cache: std::collections::HashMap<WindowId, (String, Version, std::rc::Rc<Vec<(usize, usize)>>)>,
    /// Each stashed window's card: out of the bunch (working or notified)
    /// or not, and since when -- what its sliding runs from (`sync_pulls`).
    pub pulled: std::collections::HashMap<WindowId, (bool, std::time::Instant)>,
    /// Holes in the web views for what is drawn over a page but is no
    /// overlay -- the pointer over it still is the page's for hovering,
    /// the caret and the wheel: the outline of where a dragged window
    /// would land.
    pub web_cuts: std::rc::Rc<std::cell::RefCell<Vec<(gpui::Bounds<Pixels>, Pixels)>>>,
    /// ctrl-tab, control still held: the walk through the sessions.
    pub switcher: Option<crate::switcher::Switcher>,
    /// A session sliding in (ctrl-tab), and the overview up (⌘⇧\).
    pub switch_slide: Option<crate::switcher::SwitchSlide>,
    pub overview: Option<crate::switcher::Overview>,
    /// The sidebar's session row under the pointer: its × shows, and a
    /// preview of the session beside it.
    pub sidebar_hover: Option<crate::pool::TabId>,
    /// Where each session's row was drawn in the sidebar.
    pub sidebar_rows: std::rc::Rc<std::cell::RefCell<HashMap<crate::pool::TabId, gpui::Bounds<Pixels>>>>,
    /// Errors just written, shown as toasts by their columns.
    pub toasts: Vec<crate::toasts::Toast>,
    /// Where the toasts were drawn last: a click anywhere else puts
    /// them away.
    pub toasts_at: std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<Pixels>>>>,
    /// Each diagnostic window's text as this client last saw it: what is
    /// new against it is a toast (`diagnostic_news`).
    pub diag_seen: std::collections::HashMap<WindowId, String>,
    /// The diagnostic windows there at the start have been seen.
    pub diag_primed: bool,
    /// Windows on their way to where the tiling put them.
    pub glide: crate::glide::Glide,
    /// ^F's list under the caret (`completion.rs`), and the candidates
    /// that came in for it, to be shown.
    pub completion: Option<crate::completion::Completion>,
    pub candidates: Vec<apex_server::proto::Candidates>,
    /// ⌘⇧P: the palette of commands to run.
    pub commands: Option<crate::commands::Commands>,
    /// A web window's address being typed in its header.
    pub url_edit: Option<crate::webbar::UrlEdit>,
    /// The title bar's session name being typed, and its sessions
    /// dropped down (`titlebar.rs`).
    pub session_edit: Option<crate::titlebar::SessionEdit>,
    /// ⌘O's panel (`quickopen.rs`), and the last listing asked for.
    pub quick: Option<crate::quickopen::QuickOpen>,
    pub next_find: u64,
    /// A bunny is drawn in an empty column (`bunny.rs`): the mouse
    /// moving draws again, for her eyes.
    pub bunnies: bool,
    /// A tag's path or label being typed (`tagedit.rs`).
    pub tag_edit: Option<crate::tagedit::TagEdit>,
    /// The picker under a tag's path, and a click on the path waiting to
    /// be a double-click before it comes down.
    pub picker: Option<crate::tagedit::Picker>,
    /// The folders under a crumb of the title bar's directory (`cwdbar.rs`).
    pub cwd_picker: Option<crate::cwdbar::CwdPicker>,
    pub picker_due: Option<(WindowId, Atom, std::time::Instant)>,
    /// The window whose path's picker a click just brought down, and
    /// when: a second click on its tag soon after is a double-click.
    pub picker_opened: Option<(WindowId, std::time::Instant)>,
    /// A verb in a tag's head pressed: it runs if the button comes up on it.
    pub atom_down: Option<(WindowId, Atom, MouseButton)>,
    /// The process pill the pointer is on, and where it is drawn: its
    /// card goes under it (`procs.rs`).
    pub proc_hover: Option<(apex_core::Seq, gpui::Bounds<Pixels>)>,
    pub session_menu: bool,
    /// Every known host and its sessions, for the sidebar: as last seen,
    /// then as each host answers (`sidebar_refresh`), and when it was
    /// last asked.
    pub sidebar_hosts: Vec<(crate::shell::Host, crate::shell::Loading)>,
    pub sidebar_asked: Option<std::time::Instant>,
    /// Blank web windows already given their address field.
    pub url_asked: std::collections::HashSet<WindowId>,
    /// Measured by the tag elements each frame: wrapped lines, trailing newline.
    pub tag_need: HashMap<ViewId, (usize, bool)>,
    /// The tab shown has nothing of its own yet -- its link is being
    /// made, or could not be -- and the window is a blank page saying
    /// this instead of acme. Tabs are the app's own state: the link
    /// goes on coming whether or not the window is on it.
    pub waiting: Option<String>,
    /// `Exit`: the next frame closes this window.
    pub close_requested: bool,
    /// The terminal under the pointer, and whether the app is in front:
    /// the terminal with the keyboard, in acme's model, told of focus
    /// gained and lost (`TermFocus`) as either changes.
    term_under_pointer: Option<TermId>,
    app_active: bool,
    /// The session shown is over (ended here or under us): the window
    /// moves to the session parked last, or closes when there is none.
    pub leave_requested: bool,
    pending: Option<Pending>,
    /// The view a warp riding a glide is taking the pointer to: it has the
    /// keys and the ring on the way, not what the pointer passes over.
    warp_onto: Option<ViewId>,
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
    /// The strip (a column put away) whose slice is out, by the pointer on
    /// it, and when the pointer left it and its slice (`strips.rs`).
    pub strip_open: Option<ColumnId>,
    pub strip_leaving: Option<std::time::Instant>,
    /// The stash in the title bar, fanned out under the pointer.
    pub shelf: crate::shelf::Shelf,
    /// Whether AppKit's title bar container is hidden (full screen).
    pub native_bar_hidden: bool,
    /// Positions to bring on screen (new errors text), by view.
    pub(crate) show_at: HashMap<ViewId, (usize, usize)>,
    /// A place to go once its file is open (asked of the server).
    pub pending_goto: Option<Loc>,
    /// A notification in a tab still attaching (⌘G): taken once it is
    /// here and its window with it.
    pending_note: Option<WindowId>,
    /// A place in another session to go to: the next render switches.
    pub pending_switch: Option<Loc>,
    /// A page reported a cursor: apply it on the next tick.
    page_cursor_now: bool,
    /// ⌘P, when open.
    pub finder: Option<crate::finder::Finder>,
    /// The windows as of the last frame, to notice closings.
    pub last_windows: std::collections::BTreeMap<WindowId, String>,
    /// The session those windows were in: after a switch they are the
    /// other session's, not closed ones of this.
    pub last_windows_of: Option<SessionUrl>,
    /// A new window with the picker open and nothing attached yet: it
    /// closes if the picker is dismissed, and is not remembered.
    /// The tools menu while B4 is held.
    pub menu: Option<menu::Menu>,
    /// What the menu ran last: it opens on that item.
    menu_last: Option<String>,
    /// The tabs as the strip last drew them -- which they are, which
    /// want the user, and what each is doing (`tabs_tick`).
    tabs_shown: Vec<(TabId, String, bool, Option<String>)>,
    /// Bodies scrolled by the trackpad, as a native view scrolls: by the
    /// pixel between whole lines, and past either end. Only this client's;
    /// the origin line is what the session has.
    smooth: HashMap<ViewId, Smooth>,
    /// When each body was last scrolled by the trackpad.
    wheel_at: HashMap<ViewId, std::time::Instant>,
    /// The spring pulling overscrolled bodies back is running.
    springing: bool,
    /// The window under the pointer, which has the keyboard while the app
    /// is in front (`active_window`).
    win_under_pointer: Option<WindowId>,
    /// The notification the active window had when it became active, by
    /// the entry that raised it: it was there first, and stays.
    entered: Option<apex_core::Seq>,
    /// Notifications that came to the active window, dismissed as they
    /// came and never drawn, until the dismissal comes back.
    suppressed: std::collections::HashSet<apex_core::Seq>,
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
    pub(crate) last_mouse: Point<Pixels>,
    pub focus: FocusHandle,
    pub layouts: HashMap<ViewId, TextLayout>,
    pub term_layouts: HashMap<WindowId, TermLayout>,
    /// Where each page's scrollbar was drawn this frame, beside its view.
    pub web_bars: HashMap<WindowId, Bounds<Pixels>>,
    /// The native views of web windows (WEB.md §2).
    pub webs: Webs,
    pub hl: Option<(ViewId, usize, usize, HlKind)>,
    /// The tag, column tag or top row the pointer is on.
    pub hover_view: Option<ViewId>,
    /// The scroller lane the pointer is in: a text's, or a terminal's or
    /// page's (by its window's body).
    lane_hover: Option<ViewId>,
    /// Each scroller's last position and when it last moved: its thumb
    /// shows while it moves and fades after.
    scroll_pos: HashMap<ViewId, u64>,
    scrolled_at: HashMap<ViewId, std::time::Instant>,
    /// ⌘ or ⌥ held over text: what a click would take there.
    pub hint: Option<(ViewId, usize, usize, HlKind)>,
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
pub(crate) fn is_file_char(c: char) -> bool {
    is_alnum(c) || ".-+/:@".contains(c)
}
pub(crate) fn is_exec_char(c: char) -> bool {
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
        let tab = Pool::open(cx, url);
        // parked here (a window closed on it, say): shown at once
        if let Some(p) = Pool::take(cx, tab) {
            let mut acme = Self::from_parked(cx, tab, p, wake.clone());
            acme.open_initial(acme.node.state.layout.cols.first().map(|c| c.id).unwrap_or(ColumnId(0)), files);
            return Ok(acme);
        }
        let (link, mut log, mut node, target) = Self::connect_targeted(url, wake.clone())?;
        // files given open in the last column, as acme's do
        let col = match node.state.layout.cols.last() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        let url = &identified(url, &node);
        let mut acme = Self::over(cx, log, node, Backend::Remote(link), &url.session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = url.clone();
        acme.tab = tab;
        acme.wake = Some(wake);
        acme.wake_target = Some(target);
        crate::shell::note_recent(url);
        Pool::note_open(cx, tab, url);
        acme.open_initial(col, files);
        Ok(acme)
    }

    /// A window on a parked session: its state as it was left, its wake
    /// pointed here.
    fn from_parked(cx: &mut Context<Self>, tab: TabId, p: Parked, wake: Wake) -> Acme {
        p.target.set(wake.clone());
        let mut acme = Self::over(cx, p.log, p.node, Backend::Remote(p.link), &p.url.session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = p.url.clone();
        acme.tab = tab;
        acme.wake = Some(wake);
        acme.wake_target = Some(p.target);
        acme.previews = p.previews;
        acme.live = p.live;
        acme.snarfouts = p.snarfouts;
        acme.pending_goto = p.pending_goto;
        crate::shell::note_recent(&acme.url.clone());
        Pool::note_open(cx, tab, &acme.url.clone());
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
        self.web_bars.clear();
        self.hl = None;
        self.mouse = Mouse::default();
        self.want_visible.clear();
        self.typed_start.clear();
        // the toasts are its, and go; what it had seen goes with it
        self.toasts.clear();
        self.pulled.clear();
        let seen = crate::pool::Seen { diag: std::mem::take(&mut self.diag_seen), noted: std::mem::take(&mut self.noted) };
        self.diag_primed = false;
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
            seen: Some(seen),
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
                            crate::shell::log_line(&format!("ended {url}: leaving it"));
                            acme.connected = false; // nothing to park
                            acme.leave_requested = true;
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
            Pool::park(cx, self.tab, p);
        }
    }

    /// The session shown is over: its tab goes, and the window shows the
    /// session parked last, or closes when no other is connected.
    pub fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_requested = false;
        let (gone, tab) = (self.url.clone(), self.tab);
        let next = self.next_tab(cx);
        Pool::let_go(cx, tab);
        match next {
            Some(prev) => {
                crate::shell::log_line(&format!("{gone} over: the window shows {prev}"));
                self.switch_to(prev, window, cx);
            }
            None => {
                crate::shell::log_line(&format!("{gone} over, nothing else connected: closing the window"));
                self.close_now(cx);
            }
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
        let leaving = self.tab;
        match self.next_tab(cx) {
            Some(prev) => {
                self.switch_to(prev, window, cx);
                Pool::let_go(cx, leaving);
            }
            // the last tab: the window goes with it, as a browser's does
            // (the session stays, parked, for a window asking later)
            None => self.close_requested = true,
        }
        cx.notify();
    }

    /// Where the window goes when the tab it shows is closed or over:
    /// the session parked most recently, else the tab beside this one
    /// (which may be one still coming up, or down -- a tab is a tab).
    fn next_tab(&self, cx: &gpui::App) -> Option<TabId> {
        if let Some(id) = Pool::most_recent(cx).filter(|id| *id != self.tab) {
            return Some(id);
        }
        let tabs = Pool::tabs(cx);
        let at = tabs.iter().position(|t| t.id == self.tab)?;
        tabs.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| tabs.get(i))).map(|t| t.id)
    }

    /// cmd-N: the Nth tab.
    pub fn go_to_tab(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tabs = Pool::tabs(cx);
        if let Some(t) = tabs.get(n.saturating_sub(1)) {
            if t.id != self.tab {
                let id = t.id;
                self.switch_to(id, window, cx);
                cx.notify();
            }
        }
    }

    /// ⌘⇧[ and ⌘⇧]: the tab on either side of this one, in the order the
    /// bar shows them, wrapping at both ends as a browser's do.
    pub fn cycle_tab(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let tabs = Pool::tabs(cx);
        let Some(at) = tabs.iter().position(|t| t.id == self.tab) else { return };
        let id = tabs[around(at, tabs.len(), by)].id;
        if id != self.tab {
            self.switch_to(id, window, cx);
            cx.notify();
        }
    }

    /// cmd-shift-k: back to the session parked most recently.
    pub fn previous_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Pool::most_recent(cx) {
            Some(id) => {
                self.switch_to(id, window, cx);
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
        self.switch_to_url(&url, window, cx);
        if !loc.name.is_empty() {
            self.pending_goto = Some(Loc { session: None, ..loc });
        }
        cx.notify();
    }

    /// Show the session at `url`: its tab, which is the one the app has
    /// for that session or a new one (`Pool::open` -- the one place a
    /// url becomes a tab).
    pub fn switch_to_url(&mut self, url: &SessionUrl, window: &mut Window, cx: &mut Context<Self>) {
        let id = Pool::open(cx, url);
        self.switch_to(id, window, cx);
    }

    /// Show a tab. Attached already (parked here), it is back at once;
    /// otherwise the window is a blank page saying what is happening
    /// while the pool makes the link. Nothing waits on the UI thread,
    /// and nothing is lost by moving on: the link goes on being made,
    /// and lands in its tab whether or not that tab is the one shown.
    pub fn switch_to(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if id == self.tab && self.connected {
            return;
        }
        if let Some(p) = self.park() {
            Pool::park(cx, self.tab, p);
        }
        self.tab = id;
        if let Some(p) = Pool::take(cx, id) {
            self.adopt_parked(p, window);
            Pool::note_open(cx, id, &self.url.clone());
            return;
        }
        let Some(url) = Pool::url_of(cx, id) else { return };
        self.url = url.clone();
        self.session = url.session.clone();
        self.connected = false;
        // a tab already coming up says what it is doing, not what this
        // switch would have said
        let why = match Pool::state(cx, id) {
            crate::pool::State::Coming(why) => why,
            _ => crate::pool::Why::Attaching,
        };
        self.wait(&why.sentence(&url));
        window.set_window_title(&self.current_title());
        Pool::start(cx, id, why, false, Vec::new());
    }

    /// The window has no session to show: the blank page says why. What
    /// is being waited for (or what went wrong) is the only thing on it.
    pub fn wait(&mut self, what: &str) {
        self.waiting = Some(what.to_string());
    }

    /// The link this window was waiting for did not come.
    pub fn wait_failed(&mut self, why: &str) {
        crate::shell::log_line(&format!("{}: {why}", self.url));
        self.connected = false;
        self.waiting = Some(why.to_string());
    }

    /// A parked session back in this window.
    fn adopt_parked(&mut self, p: Parked, window: &mut Window) {
        if let Some(wake) = &self.wake {
            p.target.set(wake.clone());
        }
        self.backend = Backend::Remote(p.link);
        self.connected = true;
        self.waiting = None;
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
        // what this window had seen of it, so only what came meanwhile is
        // news; a session never shown here holds none yet
        match p.seen {
            Some(s) => {
                self.diag_seen = s.diag;
                self.noted = s.noted;
                self.diag_primed = true;
            }
            None => self.forget_seen(),
        }
        self.win_under_pointer = None;
        self.entered = None;
        self.suppressed.clear();
        self.socket = Some(apex_server::daemon::default_socket());
        self.webs = Webs::new(self.io_plane(), self.wake.clone());
        self.selector = None;
        window.set_window_title(&Self::title(&p.url));
        crate::shell::note_recent(&p.url);
        self.sync();
    }

    /// A mark for an overlay's panel: records the panel's bounds this
    /// frame (`overlay_bounds`), for the holes in the web views. Zero
    /// size, so it takes no clicks.
    pub fn overlay_mark(&self) -> gpui::AnyElement {
        self.overlay_mark_by(px(18.))
    }

    /// An overlay's mark whose hole reaches `margin` past it: the panels'
    /// shadows want room; a card flush against a page wants none, or the
    /// page beside it would show a band of bare paper.
    pub fn overlay_mark_by(&self, margin: Pixels) -> gpui::AnyElement {
        use gpui::prelude::*;
        let rc = self.overlay_bounds.clone();
        gpui::div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .child(gpui::canvas(
                move |b, _, _| rc.borrow_mut().push((b, margin)),
                |_, _, _, _| {},
            ).size_full())
            .into_any_element()
    }

    /// A hole in the web views where this element lands (`web_cuts`),
    /// `margin` past it, which is no overlay.
    pub fn web_cut_by(&self, margin: Pixels) -> gpui::AnyElement {
        use gpui::prelude::*;
        let rc = self.web_cuts.clone();
        gpui::div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .child(gpui::canvas(move |b, _, _| rc.borrow_mut().push((b, margin)), |_, _, _, _| {}).size_full())
            .into_any_element()
    }

    /// What the daemon must know of this client beyond presentation:
    /// the terminal colours programs are told (the theme's). Sent when
    /// a link is made and again when the theme changes; then the
    /// terminal with the keyboard loses and regains focus, so a program
    /// that asks its colours on focus asks again.
    pub fn send_config(&mut self) {
        if let Backend::Remote(link) = &mut self.backend {
            link.send(&ClientMsg::ClientConfig { term: crate::theme::term_colors() });
            if let (true, Some(t)) = (self.app_active, self.term_under_pointer) {
                link.send(&ClientMsg::TermFocus { term: t, focused: false });
                link.send(&ClientMsg::TermFocus { term: t, focused: true });
            }
        }
    }

    /// The keyboard's terminal, as the pointer and the app's activation
    /// have it: the one that had it is told it lost it, the one that
    /// has it now that it gained it.
    fn term_focus_now(&mut self, under: Option<TermId>, active: bool) {
        let was = if self.app_active { self.term_under_pointer } else { None };
        let now = if active { under } else { None };
        self.term_under_pointer = under;
        self.app_active = active;
        if was == now {
            return;
        }
        if let Backend::Remote(link) = &mut self.backend {
            if let Some(t) = was {
                link.send(&ClientMsg::TermFocus { term: t, focused: false });
            }
            if let Some(t) = now {
                link.send(&ClientMsg::TermFocus { term: t, focused: true });
            }
        }
    }

    /// What this client does for the rules, and the rules it brings: it
    /// can `open` things the way the platform does, and URLs go there.
    /// The rules are its own, gone when it detaches.
    fn arm(link: &mut Link) {
        link.send(&ClientMsg::ClientConfig { term: crate::theme::term_colors() });
        let urls = PlumbRule {
            verb: "plumb".into(),
            owner: None,
            unlisted: false,
            text: Some(r"https?://\S+".into()),
            file: None,
            kind: None,
            isfile: None,
            isdir: None,
            action: RuleAction::Client { verb: "open".into(), args: "$0".into() },
            win: None, to: None,
        };
        link.send(&ClientMsg::RuleAdd { rule: urls, priority: -10, mine: true });
        // Snarfout in terminals and win windows: the last command's
        // output. A win window says it is win's (`Own`); it is not
        // guessed at by the shape of its name
        for (kind, owner) in [(WinKind::Term, None), (WinKind::File, Some("win-.*".to_string()))] {
            let rule = PlumbRule {
                verb: "Snarfout".into(),
                owner,
                unlisted: false,
                text: None,
                file: None,
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
        // files given open in the last column, as acme's do
        let col = match node.state.layout.cols.last() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        let mut acme = Self::over(cx, log, node, Backend::Remote(link), session);
        acme.socket = Some(apex_server::daemon::default_socket());
        acme.url = SessionUrl { provider: "via".into(), arg: cmd.split_whitespace().nth(1).unwrap_or("?").to_string(), session: session.to_string(), id: None };
        acme.tab = Pool::open(cx, &acme.url.clone());
        acme.wake = Some(wake);
        acme.open_initial(col, files);
        Ok(acme)
    }

    /// Take a fresh link (made by `connect`, on any thread) as this
    /// window's: the second half of `reattach`, and what an attach made
    /// in the background comes back to.
    pub fn adopt(&mut self, link: Link, mut log: Log, mut node: Node, target: WakeTarget, url: &SessionUrl, files: Vec<String>, window: &mut Window) -> std::io::Result<()> {
        // the link was made elsewhere (the pool makes them all): its wake
        // comes here from now on, or nothing would poll it
        if let Some(wake) = &self.wake {
            target.set(wake.clone());
        }
        if let Some(old) = self.wake_target.replace(target) {
            drop(old);
        }
        // files given open in the last column, as acme's do
        let col = match node.state.layout.cols.last() {
            Some(c) => c.id,
            None => node.init_session(&mut log).map_err(std::io::Error::other)?,
        };
        self.backend = Backend::Remote(link);
        self.connected = true;
        self.waiting = None;
        self.last_ping = None;
        self.log = log;
        self.node = node;
        let url = &identified(url, &self.node);
        self.session = url.session.clone();
        self.url = url.clone();
        // a window that started offline is one to remember now
        self.socket = Some(apex_server::daemon::default_socket());
        self.layouts.clear();
        self.term_layouts.clear();
        self.web_bars.clear();
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
        // and nothing it holds already is news
        self.forget_seen();
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

    pub fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.wake.is_none() {
            return; // an in-process session has nothing to reconnect to
        }
        let url = self.url.clone();
        // the link it has (a stuck one, or none) goes; the pool makes
        // another, and the tab says so meanwhile
        if let Some(mut p) = self.park() {
            p.link.close();
        }
        self.connected = false;
        self.wait(&crate::pool::Why::Attaching.sentence(&url));
        window.set_window_title(&self.current_title());
        Pool::start(cx, self.tab, crate::pool::Why::Attaching, false, Vec::new());
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
        if self.waiting.is_some() {
            format!("{} — attaching — apex", self.url)
        } else if !self.connected {
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
                                // a spinner turning: the working handles, or the
                                // page a tab shows while it comes up
                                if acme.web_focus_tick(window) || acme.any_working() || acme.waiting.is_some() {
                                    cx.notify();
                                }
                                // a walk whose key came up where gpui did not
                                // hear it (a page had the keys): ended as the
                                // key coming up ends it
                                let (_, ctrl) = crate::web::modifiers_down();
                                if acme.switcher.is_some() && !ctrl {
                                    acme.switcher_commit(window, cx);
                                }
                                if acme.caret_tick() {
                                    cx.notify();
                                }
                                // the strips, from where the pointer is, asked
                                // of the system (a page keeps its moves to
                                // itself, and it may have left): resting is
                                // no move
                                if let Some(p) = crate::web::native_mouse(window) {
                                    let held = acme.held_any();
                                    if acme.strip_tick(p, held) {
                                        cx.notify();
                                    }
                                }
                                if acme.tabs_tick(cx) {
                                    cx.notify();
                                }
                                // toasts go by themselves after a while
                                if !acme.toasts.is_empty() {
                                    cx.notify();
                                }
                            }
                        });
                    }
                }
                crate::attention::tick(cx);
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
            tab: TabId(0),
            wake: None,
            wake_target: None,
            selector: None,
            caret_view: None,
            caret_term: None,
            caret_on: true,
            caret_glide: None,
            caret_since: std::time::Instant::now(),
            shelf: Default::default(),
            strip_open: None,
            strip_leaving: None,
            native_bar_hidden: false,
            overlay_bounds: Default::default(),
            noted: Default::default(),
            looking: None,
            look_cache: Default::default(),
            pulled: Default::default(),
            web_cuts: Default::default(),
            switcher: None,
            switch_slide: None,
            overview: None,
            url_edit: None,
            session_edit: None,
            quick: None,
            next_find: 0,
            bunnies: false,
            tag_edit: None,
            picker: None,
            cwd_picker: None,
            picker_due: None,
            picker_opened: None,
            atom_down: None,
            proc_hover: None,
            session_menu: false,
            sidebar_hosts: Vec::new(),
            sidebar_asked: None,
            commands: None,
            completion: None,
            candidates: Vec::new(),
            glide: Default::default(),
            toasts: Vec::new(),
            toasts_at: Default::default(),
            diag_seen: Default::default(),
            diag_primed: false,
            sidebar_hover: None,
            sidebar_rows: Default::default(),
            url_asked: std::collections::HashSet::new(),
            tag_need: HashMap::new(),
            waiting: None,
            close_requested: false,
            term_under_pointer: None,
            app_active: true,
            leave_requested: false,
            pending: None,
            warp_onto: None,
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
            pending_note: None,
            pending_switch: None,
            page_cursor_now: false,
            finder: None,
            last_windows: std::collections::BTreeMap::new(),
            last_windows_of: None,
            menu: None,
            menu_last: None,
            tabs_shown: Vec::new(),
            smooth: HashMap::new(),
            wheel_at: HashMap::new(),
            springing: false,
            win_under_pointer: None,
            entered: None,
            suppressed: std::collections::HashSet::new(),
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
            web_bars: HashMap::new(),
            webs: Webs::new(None, None),
            hl: None,
            hint: None,
            hover_view: None,
            lane_hover: None,
            scroll_pos: HashMap::new(),
            scrolled_at: HashMap::new(),
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
        self.track_closed();
        // ^F's candidates, as they come: typed in, or listed
        for c in std::mem::take(&mut self.candidates) {
            if c.at == crate::tagedit::LISTING {
                self.got_listing(c);
            } else if c.at == crate::cwdbar::CWD_LISTING {
                self.got_cwd_listing(c);
            } else {
                self.got_candidates(c);
            }
        }
        // what is new in the diagnostic windows: toasts
        self.diagnostic_news();
        for (v, q) in self.node.take_shows() {
            // errors just written to a diagnostic window not open: its
            // toast says them, and it is not brought out
            if let ViewId::Body(w) = v {
                if self.node.window_diagnostic(w) && !self.diagnostic_seen(w) {
                    continue;
                }
            }
            self.show_at.insert(v, (q, 1));
        }
        // Looks in pages: found in their views
        for (w, text, reverse) in self.node.take_page_finds() {
            self.webs.find(w, &text, reverse);
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
        // a notification taken in a tab that was still attaching (⌘G)
        if let Some(w) = self.pending_note {
            if self.node.state.window(w).is_ok() {
                self.pending_note = None;
                let _ = self.dismiss(w);
                self.show(w);
                self.node.warp = Some(Warp::NewWindow(w));
            }
        }
        if let Backend::Remote(link) = &mut self.backend {
            link.flush(&self.log);
        }
        if let Err(e) = self.node.catch_up(&self.log) {
            eprintln!("catch up: {e}");
        }
        self.suppress_arrivals();
        self.take_warp();
    }

    /// Has this client lost its leases (another UI attached and took
    /// them)? The mirror log follows the metalog, so it knows.
    /// The oldest notification in this session, which the top left square
    /// is showing and a click on it takes.
    pub fn notification_head(&self) -> Option<apex_core::state::Notification> {
        self.shown_notifications().next().copied()
    }

    /// Whether the session a tab names has notifications waiting: this
    /// window's own, or a parked one's.
    pub fn tab_notified(&self, id: TabId, cx: &gpui::App) -> bool {
        if id == self.tab {
            return self.notification_head().is_some();
        }
        Pool::notified(cx, id)
    }

    /// Every tick: the strip as it would be drawn now -- the tabs, which
    /// want the user, and what each is doing -- when that has changed
    /// since the last look. A parked session's entries arrive off this
    /// window, and so does a tab coming up, so nothing else would draw
    /// the strip again.
    pub fn tabs_tick(&mut self, cx: &gpui::App) -> bool {
        let now: Vec<(TabId, String, bool, Option<String>)> =
            Pool::tabs(cx).into_iter().map(|t| (t.id, t.url.to_string(), self.tab_notified(t.id, cx), self.tab_word(&t, cx))).collect();
        if now == self.tabs_shown {
            return false;
        }
        self.tabs_shown = now;
        true
    }

    /// The word after a tab's name: what it is doing, when that is
    /// anything but simply being up -- "connecting…", "restoring…",
    /// "fenced", "offline".
    pub fn tab_word(&self, tab: &crate::pool::Tab, cx: &gpui::App) -> Option<String> {
        use crate::pool::State;
        if tab.id == self.tab {
            // this window knows its own session better than the pool does
            return match &tab.state {
                State::Coming(why) if self.waiting.is_some() => Some(why.word().to_string()),
                _ if self.waiting.is_some() => Some("offline".into()),
                _ if self.fenced() => Some("fenced".into()),
                _ => None,
            };
        }
        match &tab.state {
            State::Up => Pool::fenced(cx, tab.id).then(|| "fenced".to_string()),
            s => s.word().map(str::to_string),
        }
    }

    /// The session's square clicked while a tool wants the user: the
    /// oldest notified window is brought on screen and the pointer taken
    /// to it, as a new window is landed on, and its notification is
    /// dismissed. The next click takes the next; once there are none the
    /// square is the tag's colour again.
    /// ⌘G: the oldest notification the app is carrying, wherever it is.
    /// The tab it is in comes forward if it is not this one, the window
    /// is shown and the pointer lands on it, and the notification is
    /// taken. A beep when there is none left to take.
    pub fn next_notification(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((tab, w, _)) = crate::attention::queue(cx).into_iter().next() else {
            crate::attention::beep();
            return;
        };
        if tab != self.tab {
            self.switch_to(tab, window, cx);
            // a parked tab is here at once; one still attaching takes it
            // when it lands
            if !self.connected {
                self.pending_note = Some(w);
                cx.notify();
                return;
            }
        }
        self.take_note(w, cx);
    }

    /// Take a notification here: it is lowered, its window shown, and
    /// the pointer warped to it, as taking one from the square does.
    fn take_note(&mut self, w: WindowId, cx: &mut Context<Self>) {
        if self.node.state.window(w).is_err() {
            return;
        }
        let _ = self.dismiss(w);
        self.show(w);
        self.node.warp = Some(Warp::NewWindow(w));
        self.after();
        cx.notify();
    }

    /// A window picked in the sidebar: shown and landed on, as a
    /// notification's window is; picking it is attending to it.
    pub fn reveal_window(&mut self, w: WindowId, cx: &mut Context<Self>) {
        let _ = self.dismiss(w);
        self.show(w);
        self.node.warp = Some(Warp::NewWindow(w));
        self.after();
        cx.notify();
    }

    /// Is the window grown to the whole of its column with others hidden
    /// behind it (B3 on its box)? Its handle is square while it is.
    pub fn hides_others(&self, w: WindowId) -> bool {
        let l = &self.node.state.layout;
        l.column_of(w).and_then(|c| l.column(c)).is_some_and(|c| c.hiding() && c.full.is_some_and(|f| f.window == w))
    }

    /// Does the window carry a notification this client shows -- it, or
    /// a window it covers (whose handle is this one's while it is)?
    pub fn window_notified(&self, w: WindowId) -> bool {
        let ws = self.handle_of(w);
        self.shown_notifications().any(|n| ws.contains(&n.window))
    }

    /// The windows a window's handle speaks for: itself, and, the top of
    /// a stack (`Cover`), the windows under it.
    fn handle_of(&self, w: WindowId) -> Vec<WindowId> {
        let l = &self.node.state.layout;
        if l.over(w).is_none() && l.under(w).is_some() {
            l.stack(w)
        } else {
            vec![w]
        }
    }

    /// A session shown here for the first time: what it already holds is
    /// not news -- no toast for its diagnostic windows' text (primed when
    /// next looked at), no ping for its notifications -- and the other
    /// session's toasts are gone.
    fn forget_seen(&mut self) {
        self.toasts.clear();
        self.pulled.clear();
        self.diag_seen.clear();
        self.diag_primed = false;
        let long_ago = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(60)).unwrap_or_else(std::time::Instant::now);
        self.noted = self.shown_notifications().map(|n| ((n.window, n.at), long_ago)).collect();
    }

    /// When this client first saw each notification it shows, kept as
    /// they come and go (each frame): what its handle's ping runs from.
    pub fn sync_notes(&mut self) {
        let now: Vec<(WindowId, Seq)> = self.shown_notifications().map(|n| (n.window, n.at)).collect();
        self.noted.retain(|k, _| now.contains(k));
        for k in now {
            self.noted.entry(k).or_insert_with(std::time::Instant::now);
        }
    }

    /// How long ago window `w`'s notification came, if it has one shown.
    pub fn note_age(&self, w: WindowId) -> Option<f32> {
        // the latest of the windows its handle speaks for
        let ws = self.handle_of(w);
        self.shown_notifications().filter(|n| ws.contains(&n.window)).map(|n| self.noted.get(&(n.window, n.at)).map_or(f32::MAX, |t| t.elapsed().as_secs_f32())).reduce(f32::min)
    }

    fn take_notification(&mut self, cx: &mut Context<Self>) {
        let Some(n) = self.notification_head() else { return };
        let _ = self.dismiss(n.window);
        self.show(n.window);
        self.node.warp = Some(Warp::NewWindow(n.window));
        self.after();
        cx.notify();
    }

    /// The user has attended to a window: its notification, if it has
    /// one, is lowered. Taking it from the session's square does, and so
    /// does a click or a key in the window.
    fn dismiss(&mut self, w: WindowId) -> bool {
        if !self.node.window_notified(w) {
            return false;
        }
        match &mut self.backend {
            Backend::Remote(link) => link.send(&ClientMsg::Unnotify { window: w }),
            Backend::Local(_) => {
                self.log.unnotify(w);
                let _ = self.node.catch_up(&self.log);
            }
        }
        true
    }

    /// A click or a key in a window: its notification, if any, dismissed.
    fn attend(&mut self, w: WindowId) {
        // worked in in the stash's preview: first among its cards after
        self.touch_stashed(w);
        if self.dismiss(w) {
            self.after();
        }
    }

    /// Is this window in front?
    pub fn app_active(&self) -> bool {
        self.app_active
    }

    /// The window with the keys' caret (its card ringed): a text's, or a
    /// terminal's.
    pub fn key_window(&self) -> Option<WindowId> {
        self.caret_view.and_then(|v| v.window()).or_else(|| self.caret_term.and_then(|t| self.node.state.windows.values().find(|w| w.body == apex_core::Body::Term(t)).map(|w| w.id)))
    }

    /// The view the keys go to, as `key_down` finds it: the text under
    /// the pointer; over a page's scrollbar or no text at all, the last
    /// text selected in; over a terminal, none of acme's (the terminal
    /// has its own cursor). None while apex is not in front.
    pub fn key_view(&self) -> Option<ViewId> {
        if !self.app_active {
            return None;
        }
        if let Some(v) = self.warp_onto {
            // a terminal's keys are its own: none of acme's views then
            let text = v.window().and_then(|w| self.node.state.window(w).ok()).is_some_and(|w| !matches!(w.body, Body::Term(_)));
            return text.then_some(v);
        }
        match self.locate(self.pointer.unwrap_or(self.last_mouse)) {
            Some((Target::View(v), _)) => Some(v),
            Some((Target::Term(..), _)) => None,
            Some((Target::Web(_), _)) | None => self.node.seltext,
        }
    }

    /// The blue caret's state brought up to now: which view has it, and
    /// whether it shows -- solid for half a second after it was last
    /// started, then on and off every 530 ms, as the system's blinks.
    /// True when either changed, so the window is drawn again only when
    /// the caret does.
    pub fn caret_tick(&mut self) -> bool {
        // on a toast or a panel: the keys stay where they were
        if self.over_overlay(self.pointer.unwrap_or(self.last_mouse)) {
            return false;
        }
        let view = self.key_view();
        let term = self.key_term();
        if view != self.caret_view || term != self.caret_term {
            self.caret_view = view;
            self.caret_term = term;
            self.caret_since = std::time::Instant::now();
            self.caret_on = true;
            return true;
        }
        let t = self.caret_since.elapsed().as_millis();
        // steady when blinking is off (View ▸ Blink Cursor)
        let on = !crate::theme::blink() || t < 500 || ((t - 500) / 530) % 2 == 1;
        if on != self.caret_on {
            self.caret_on = on;
            return view.is_some() || term.is_some();
        }
        false
    }

    /// The terminal the keys go to: the one under the pointer, while
    /// apex is in front (as `term_focus_now` tells the programs).
    pub fn key_term(&self) -> Option<TermId> {
        self.term_under_pointer.filter(|_| self.app_active)
    }

    /// The window the user is in: the one under the pointer, which has
    /// the keyboard, while this window is in front.
    fn active_window(&self) -> Option<WindowId> {
        self.win_under_pointer.filter(|_| self.app_active)
    }

    /// The pointer or the app's activation moved: a window that becomes
    /// active keeps the notification it had, since coming to a window is
    /// not attending to it. (Before `term_focus_now`, which takes the
    /// activation.)
    fn note_active(&mut self, under: Option<WindowId>, active: bool) {
        let now = under.filter(|_| active);
        if now != self.active_window() {
            self.entered = now.and_then(|w| self.node.notifications().find(|n| n.window == w)).map(|n| n.at);
        }
        self.win_under_pointer = under;
    }

    /// After catching up: a notification that has come to the active
    /// window is attended to already, so it is dismissed at once and never
    /// drawn.
    fn suppress_arrivals(&mut self) {
        self.suppressed.retain(|at| self.node.state.meta.notifications.iter().any(|n| n.at == *at));
        let Some(w) = self.active_window() else { return };
        let Some(n) = self.node.notifications().find(|n| n.window == w).copied() else { return };
        if Some(n.at) == self.entered || !self.suppressed.insert(n.at) {
            return;
        }
        let _ = self.dismiss(w);
    }

    /// The notifications this window shows: the session's, less those
    /// dismissed as they came.
    fn shown_notifications(&self) -> impl Iterator<Item = &apex_core::state::Notification> {
        self.node.notifications().filter(|n| !self.suppressed.contains(&n.at))
    }

    /// The server is in this process (`--local`), which is a session of
    /// its own -- not the blank stand-in a window holds while its tab
    /// has no link.
    pub fn in_process(&self) -> bool {
        matches!(self.backend, Backend::Local(_)) && self.wake.is_none()
    }

    pub fn fenced(&self) -> bool {
        matches!(self.backend, Backend::Remote(_))
            && self.log.lease(Shard::Layout).is_some_and(|l| l.holder != self.node.attachment || l.released.is_some())
    }

    /// A layout box is held: acme shows the box cursor.
    pub fn dragging_box(&self) -> bool {
        self.mouse.box_drag.is_some()
    }

    /// The line between two columns is held.
    pub fn dragging_edge(&self) -> bool {
        matches!(self.mouse.box_drag, Some((BoxTarget::Edge(_), _, _)))
    }

    /// A strip pressed: its column's box, as the column tag's box is.
    pub fn press_col_box(&mut self, c: ColumnId, button: MouseButton, pos: Point<Pixels>, shift: bool, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.minimize_box(BoxTarget::Col(c), button, shift, cx) {
            return;
        }
        if self.mouse.b1.is_none() {
            self.mouse.box_drag = Some((BoxTarget::Col(c), button, pos));
        }
        cx.notify();
    }

    /// Column `c` (a strip) brought back, as B1 on its box brings it.
    pub fn bring_back_column(&mut self, c: ColumnId, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let p = self.row_pt(pos);
        let at = self.node.state.layout.column(c).map(|col| ((col.r.x0 + col.r.x1) / 2, p.1)).unwrap_or(p);
        let _ = self.node.drag_column(&mut self.log, c, 1, at, at);
        self.after();
        cx.notify();
    }

    /// Any button or box held: nothing slides out under a drag.
    pub(crate) fn held_any(&self) -> bool {
        let m = &self.mouse;
        m.b1.is_some() || m.b2.is_some() || m.b3.is_some() || m.box_drag.is_some() || m.scrolling.is_some() || m.term_drag.is_some() || m.term_sweep.is_some()
    }

    /// The line on column `c`'s left pressed.
    pub fn press_edge(&mut self, c: ColumnId, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if self.mouse.b1.is_none() {
            self.mouse.box_drag = Some((BoxTarget::Edge(c), MouseButton::Left, pos));
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Where what is being dragged would land were it let go now, in the
    /// area's coordinates: a window, a column, or the column right of a
    /// line (Manifold's placement preview).
    pub fn drag_preview(&self) -> Option<tiling::Rect> {
        let (bt, b, start) = self.mouse.box_drag?;
        let but = match b {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Navigate(_) => return None,
            _ => 3,
        };
        let (op, p) = (self.row_pt(start), self.row_pt(self.last_mouse));
        match bt {
            BoxTarget::Win(w) => self.node.drag_window_preview(w, but, op, p),
            BoxTarget::Col(c) => self.node.drag_column_preview(c, but, op, p),
            BoxTarget::Edge(c) => self.node.column_edge_preview(c, p.0),
        }
    }

    /// The mouse move acme would make after the last layout change.
    fn take_warp(&mut self) {
        let Some(w) = self.node.warp.take() else { return };
        // taken to a window in a collapsed or hidden column: the column comes
        // back first, and the warp lands on the frame that shows it
        let onto = match w {
            Warp::NewWindow(x) | Warp::WinButton(x) => Some(x),
            Warp::Sel(v) => v.window(),
            Warp::Closed { next, .. } => next,
            Warp::ColButton(_) => None,
        };
        if let Some(x) = onto {
            let _ = self.node.uncover(&mut self.log, x);
        }
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
        // the window gone to still on its way (`glide.rs`): its drawing is
        // not where it lands yet, and the pointer goes where it lands
        let onto = match p {
            Pending::Warp(Warp::NewWindow(x) | Warp::WinButton(x) | Warp::Closed { next: Some(x), .. }) => Some(x),
            Pending::Warp(Warp::Sel(v)) => v.window(),
            _ => None,
        };
        // on its way, the pointer rides along with it, each frame where it
        // is drawn, so it is where it lands the moment it lands: its box,
        // the next Del after a close, a selection
        let gliding = onto.is_some_and(|x| self.glide.gliding(x));
        let font = crate::text_element::tag_line_height();
        let fonti = f32::from(font) as i32;
        let l = &self.node.state.layout;
        let (top, left) = (self.top(), self.left());
        let row = |x: i32, y: i32| point(px(x as f32 + left), px(y as f32 + top));
        // where window `w` is drawn: on its way, where it has got to
        let drawn = |w: WindowId| l.slot(w).map(|s| (self.glide.drawn_at(w).filter(|_| gliding).unwrap_or(s.r), s));
        let target = match p {
            Pending::Restore(at) => Some(at),
            Pending::Warp(Warp::NewWindow(w)) => drawn(w).map(|(r, s)| row(r.x0 + SCROLLWID + 3, r.y0 + (s.tag_y1() - s.r.y0) + 3)),
            Pending::Warp(Warp::WinButton(w)) => drawn(w).map(|(r, _)| row(r.x0 + SCROLLWID / 2, r.y0 + fonti / 2)),
            Pending::Warp(Warp::ColButton(c)) => l.column(c).map(|c| row(c.r.x0 + SCROLLWID / 2, c.r.y0 + fonti / 2)),
            Pending::Warp(Warp::Closed { next: Some(w), .. }) => {
                // movetodel: onto the next window's Del icon, so a click
                // closes that one too -- as the last frame drew it
                let del = crate::text_element::icon_index("Del");
                let stacked = crate::text_element::icon_index(crate::text_element::DEL_STACKED);
                self.layouts.get(&ViewId::Tag(w)).and_then(|tl| tl.atom_bounds(Atom::Verb(del)).or_else(|| tl.atom_bounds(Atom::Verb(stacked)))).map(|b| b.center())
            }
            Pending::Warp(Warp::Closed { next: None, .. }) => None,
            Pending::Warp(Warp::Sel(v)) => {
                let q0 = self.node.selection(v).map(|s| s.0).unwrap_or(0);
                // on the selection's first rune, halfway down its own line
                // (a body's line, not a tag's), so a click there takes it
                // again: B3 on, B3 on, through the matches
                match self.layouts.get(&v).and_then(|tl| tl.point_of(q0).map(|q| (q, tl.line_height))) {
                    Some((q, lh)) => Some(point(q.x + px(4.), q.y + lh / 2.)),
                    // a body that is not text (a terminal, a page) has no
                    // layout to find the selection in: the top of it, where
                    // a new window is landed on, so a Goto to a terminal
                    // still arrives
                    None => v.window().and_then(drawn).map(|(r, s)| row(r.x0 + SCROLLWID + 3, r.y0 + (s.tag_y1() - s.r.y0) + 3)),
                }
            }
        };
        // on its way, the pointer's destination has the keys and the
        // ring, whatever it passes over (`key_view`)
        self.warp_onto = if gliding {
            match p {
                Pending::Warp(Warp::Sel(v)) => Some(v),
                _ => onto.map(ViewId::Body),
            }
        } else {
            None
        };
        if gliding {
            if let Some(at) = target {
                crate::warp::move_to(window, at);
                self.pointer = Some(at);
                self.last_mouse = at;
                // no event says the pointer moved: the ⌘/⌥ pill goes with it
                self.update_hint(at);
            }
            window.request_animation_frame();
            return;
        }
        self.pending = None;
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
            // no event says the pointer moved: the ⌘/⌥ pill goes with it,
            // off what was clicked and onto what it now is over
            self.update_hint(at);
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

    /// Is a tool working behind any window? Their handles pulse, so the
    /// window is drawn again while it lasts.
    pub fn any_working(&self) -> bool {
        self.node.state.windows.keys().any(|w| self.node.window_working(*w))
    }

    /// Keys follow the pointer between pages and the rest (WEB.md §2.2);
    /// what the pages reported is taken; true when something should be
    /// drawn again (a page is loading: its handle pulses).
    pub fn web_focus_tick(&mut self, window: &Window) -> bool {
        if self.webs.is_empty() {
            return false;
        }
        // a click in a page puts the toasts away: drawn again without them
        let toasted = !self.toasts.is_empty();
        self.web_events();
        let dismissed = toasted && self.toasts.is_empty();
        if self.overlay_up() {
            self.webs.unfocus(window);
        } else {
            self.webs.focus_tick(window);
        }
        // a page keeps the pointer's moves to itself: the window it is in
        // is the active one while the pointer is over it
        if let Some(w) = crate::web::native_mouse(window).and_then(|p| self.webs.window_at(p)) {
            let active = self.app_active;
            self.note_active(Some(w), active);
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
        self.webs.any_loading() || dismissed
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
        // a tag's line (the tiling's font height) a little taller than a
        // body's
        let prop = f32::from(font_for(false).line_height) as i32;
        let font = f32::from(crate::text_element::tag_line_height()) as i32;
        let mono = f32::from(font_for(true).line_height) as i32;
        let mut tags = HashMap::new();
        let mut bodies = HashMap::new();
        for (w, win) in &self.node.state.windows {
            if win.body == Body::Web {
                tags.insert(*w, (1, false)); // a page's header is one line
            } else if !win.tagexpand {
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
        let row = f32::from(crate::text_element::tag_row_height()) as i32;
        self.node.tiling = Box::new(ClientInfo { font, row, prop, mono, tags, bodies });
        // the OS window
        // the row's top tag is drawn in the title bar (`title_bar`): its
        // line in the tiling is above the area, the columns start at its top
        let r = tiling::Rect::new(0, -(font + tiling::BORDER), (f32::from(viewport.width) - self.left()) as i32, (f32::from(viewport.height) - self.top()) as i32);
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
            // those hidden behind a window grown to the whole column are
            // not drawn to be measured
            .flat_map(|c| c.wins.iter().filter(move |s| !c.hides(s.window)))
            .filter(|s| {
                self.tag_need.get(&ViewId::Tag(s.window)).is_some_and(|(n, nl)| {
                    let fit = tiling::tag_lines_fit(&*self.node.tiling, s.r.dy()).max(0);
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
                let in_tag = |s: &apex_core::state::Slot, y: i32| s.r.x0 <= m.0 && m.0 < s.r.x1 && s.r.y0 <= y && y < s.tag_y1();
                let in_body = |s: &apex_core::state::Slot, y: i32| s.r.x0 <= m.0 && m.0 < s.r.x1 && s.body.y0 <= y && y < s.body.y1;
                let mut to = None;
                if in_tag(&b, m.1) && !in_tag(&a, m.1) {
                    to = Some(a.tag_y1() - 3);
                } else if in_body(&b, m.1) && in_tag(&a, m.1) {
                    to = Some(a.tag_y1() + 3);
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

    /// Where acme's area starts: the top of the window, always. There is
    /// no title bar (the window's buttons are the sidebar's), so full
    /// screen is the whole screen.
    pub fn top(&self) -> f32 {
        crate::title_h()
    }

    /// Where acme's area starts across: right of the sidebar, while it
    /// is pinned. acme's layout is in its own coordinates, from the
    /// area's corner; the pointer and the pages are in the window's.
    pub fn left(&self) -> f32 {
        if self.sidebar_shown() {
            crate::shell::SIDEBAR_W
        } else {
            0.
        }
    }

    /// The sidebar is pinned: beside the content, which is laid out
    /// right of it. Unpinned it floats over the content when brought.
    pub fn sidebar_shown(&self) -> bool {
        crate::theme::sidebar()
    }

    pub(crate) fn row_pt(&self, p: Point<Pixels>) -> (i32, i32) {
        ((f32::from(p.x) - self.left()) as i32, (f32::from(p.y) - self.top()) as i32)
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
        self.candidates.append(&mut link.candidates);
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
        // (a diagnostic window is made stashed, and stays there: its
        // news is a toast, its work its card's)
        for w in made {
            if !self.node.window_diagnostic(w) {
                self.show(w);
            }
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
            // the session was ended under us: on to another tab, or the
            // window says so, offline
            self.connected = false;
            self.notice(&format!("session {name} ended\n"));
            self.leave_requested = true;
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
        // the stashed window being worked in, in the stash's preview: shown
        // there, and left stashed
        if self.shelf.peeking() == Some(w) {
            return;
        }
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

    /// Input going to terminal `t`: while a full-screen program has it
    /// (the alternate screen), what is on it is the program's to redraw
    /// as the input may have it do, so a selection over it no longer
    /// says what it did -- it goes. (A shell's screen keeps it: the text
    /// stays where it was.)
    fn input_to(&mut self, t: TermId) {
        if !self.node.state.terms.get(&t).is_some_and(|x| x.alt) {
            return;
        }
        let w = self.node.state.windows.values().find(|w| w.body == Body::Term(t)).map(|w| w.id);
        if w.is_some() && self.term_sel.is_some_and(|(sw, _, _)| Some(sw) == w) {
            self.term_sel = None;
        }
    }

    fn term_key(&mut self, t: TermId, key: TermKey) {
        self.input_to(t);
        match &mut self.backend {
            Backend::Local(server) => server.term_key(&mut self.log, t, &key),
            Backend::Remote(link) => link.send(&ClientMsg::TermKey { term: t, key }),
        }
    }

    /// Type text into a terminal: what is sent runs, where a paste would
    /// be held on the line by a shell that asked for bracketed paste.
    fn term_type(&mut self, t: TermId, text: String) {
        self.input_to(t);
        match &mut self.backend {
            Backend::Local(server) => server.term_type(&mut self.log, t, &text),
            Backend::Remote(link) => link.send(&ClientMsg::TermType { term: t, text }),
        }
    }

    fn term_paste(&mut self, t: TermId, text: String) {
        self.input_to(t);
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
        self.input_to(t);
        match &mut self.backend {
            Backend::Local(server) => server.term_wheel(&mut self.log, t, delta, at),
            Backend::Remote(link) => link.send(&ClientMsg::TermScroll { term: t, delta: delta as i64, at }),
        }
        self.sync();
    }

    // ---- what the elements read ------------------------------------------

    pub fn source(&mut self, view: ViewId) -> Option<Source> {
        let b = self.node.view_buffer(view).ok()?;
        // a body's scroller, shown as it moves
        let scroller = match view {
            ViewId::Body(_) => {
                let at = self.node.state.buffer(b).ok()?.view(view).origin as u64;
                self.scroller(view, at)
            }
            _ => (0., false),
        };
        let buf = self.node.state.buffer(b).ok()?;
        let v = buf.view(view);
        let (mono, dirty, stale, live, pulse) = match view {
            ViewId::Body(w) | ViewId::Tag(w) => {
                let win = self.node.state.window(w).ok()?;
                let body = win.body_buffer().and_then(|b| self.node.state.buffer(b).ok());
                // dirty as Del asks about it: a transcript (a live, owned
                // or scratch window's) is never unsaved, however it changed
                let dirty = self.node.window_unsaved(w);
                let stale = dirty && body.is_some_and(|b| b.stale);
                // a page is live as a terminal is; while it loads, and
                // while a tool works behind a window, the handle
                // breathes between its colour and pale
                let web = win.body == Body::Web;
                let pulse = ((web && self.webs.loading(w)) || self.node.window_working(w)).then(breath);
                (win.mono, dirty, stale, web || self.node.window_live(w), pulse)
            }
            _ => (false, false, false, false, None),
        };
        // a window's handle while it is notified (the session's square
        // says nothing of it: the tab's face does)
        let progress = match view {
            ViewId::Tag(w) => self.node.window_progress(w),
            _ => None,
        };
        let note = match view {
            ViewId::Tag(w) => self.note_age(w),
            _ => None,
        };
        let hl = self.hl.and_then(|(hv, lo, hi, k)| if hv == view { Some((lo, hi, k)) } else { None });
        let hint = self.hint.and_then(|(hv, lo, hi, k)| if hv == view { Some((lo, hi, k)) } else { None });
        // a body scrolled by the pixel: moved up by its scroll into the top
        // row, and down by any pull past the start; forgotten once the
        // session's origin is not the one it was scrolled from
        let shift = match self.smooth.get(&view).copied() {
            Some(s) if s.origin == v.origin => s.px - s.over,
            Some(_) => {
                self.smooth.remove(&view);
                0.
            }
            None => 0.,
        };
        // a tag in a strip (a column minimized or stashed) is its
        // box alone: no text laid out in no width, and nothing it would
        // scroll to is taken, so it is still wanted when the column is wide
        let layout = &self.node.state.layout;
        let column = match view {
            ViewId::ColTag(c) => layout.column(c),
            // a stashed window (in the stash's preview) is in none
            ViewId::Tag(w) | ViewId::Body(w) => layout.place_of(w).map(|(ci, _)| &layout.cols[ci]),
            ViewId::Top => None,
        };
        if column.is_some_and(|c| tiling::is_strip(c.r)) {
            return Some(Source {
                shift: 0.,
                kind: Kind::of(view),
                head: None,
                mono,
                dirty,
                stale,
                live,
                pulse,
                progress,
                fenced: false,
                note,
                hovered: false,
                round: (false, false),
                scroller: (0., false),
                hiding: false,
                key_caret: None,
                bare: false,
                text: apex_core::text::Text::new(""),
                sel: (0, 0),
                origin: 0,
                hl: None,
            hint: None,
                marks: Default::default(),
                strike: None,
                want_visible: false,
                show_at: None,
            });
        }
        Some(Source {
            shift,
            kind: Kind::of(view),
            head: match view {
                ViewId::Tag(w) => Some(self.tag_head(w)),
                ViewId::Top => Some(self.top_head()),
                _ => None,
            },
            mono,
            dirty,
            stale,
            live,
            pulse,
            progress,
            fenced: self.fenced(),
            note,
            scroller,
            // a tag with the pointer on it shows its commands plainly
            hovered: self.hover_view == Some(view),
            // a card's outer corners: a tag's top (and foot, folded to its
            // tag), a body's foot
            round: match view {
                ViewId::Tag(w) => (true, layout.slot(w).is_none_or(|s| s.body.dy() <= 0) && !layout.is_stashed(w)),
                ViewId::Body(_) => (false, true),
                _ => (false, false),
            },
            // a window grown to the whole column with others behind it:
            // its handle square
            hiding: match view {
                ViewId::Tag(w) => self.hides_others(w),
                // a column given the whole row, the others hidden
                ViewId::ColTag(c) => layout.full == Some(c) && layout.cols.len() > 1,
                _ => false,
            },
            // the keys' view: its caret the blue one, blinking
            // (not a tag's whose path is being picked: the caret is there)
            key_caret: (self.caret_view == Some(view) && !self.picker.as_ref().is_some_and(|p| view == ViewId::Tag(p.window))).then_some(self.caret_on),
            bare: view == ViewId::Top && self.top_bare(),
            text: buf.text.clone(),
            sel: (v.q0, v.q1),
            origin: v.origin,
            hl,
            hint,
            marks: match view {
                ViewId::Body(w) => self.look_marks(w),
                _ => Default::default(),
            },
            strike: match view {
                ViewId::Tag(w) => self.look_strike(w),
                _ => None,
            },
            // a window on its way scrolls to what it must show once it lands
            want_visible: !view.window().is_some_and(|w| self.glide.gliding(w)) && self.want_visible.remove(&view),
            show_at: if view.window().is_some_and(|w| self.glide.gliding(w)) { None } else { self.show_at.remove(&view) },
        })
    }

    /// What a window's tag shows before the user's words: its path, its
    /// label (an errors window's or a preview's kind when it has none),
    /// and apex's verbs for it.
    pub fn tag_head(&self, w: WindowId) -> Head {
        let n = &self.node;
        let kind = n.window_kind(w);
        // over another window: its Del closes this one only, and says so
        let verbs: Vec<&str> = n.window_verbs(w).into_iter().map(|v| if v == "Del" && n.state.layout.under(w).is_some() { crate::text_element::DEL_STACKED } else { v }).collect();
        let label = n.window_label(w).or_else(|| match kind {
            WinKind::Errors => Some("Errors".into()),
            WinKind::Preview => Some("Preview".into()),
            _ => None,
        });
        // the path's picker down: the path being chosen, typed in place
        if let Some(p) = self.picker.as_ref().filter(|p| p.window == w) {
            return Head::picking_in(&p.dir, &p.filter, p.filter.cursor, crate::tagedit::caret_on(p.caret_since), label.as_deref(), &verbs, &n.state.meta.cwd);
        }
        // a path in the session's directory from there on (only drawn so)
        Head::build_in(&n.window_path(w), label.as_deref(), &verbs, kind != WinKind::Web, kind == WinKind::File && !n.window_scratch(w), &n.state.meta.cwd)
    }

    pub fn view_text(&self, view: ViewId) -> String {
        self.node
            .view_buffer(view)
            .ok()
            .and_then(|b| self.node.state.buffer(b).ok())
            .map(|b| b.text.to_string())
            .unwrap_or_default()
    }

    /// The view is at a whole line again, not scrolled between two.
    pub fn forget_smooth(&mut self, view: ViewId) {
        self.smooth.remove(&view);
    }

    pub fn set_origin(&mut self, view: ViewId, origin: usize) {
        let _ = self.node.set_origin(&mut self.log, view, origin);
    }

    pub fn term_resize(&mut self, term: TermId, cols: u16, rows: u16) {
        // a window on its way keeps its terminal's size until it lands
        let w = self.node.state.windows.values().find(|w| w.body == Body::Term(term)).map(|w| w.id);
        if w.is_some_and(|w| self.glide.gliding(w)) {
            return;
        }
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
        // the stash's preview lies over everything: over it, only the
        // window it shows is there
        let only = self.shelf.peeking().filter(|_| self.shelf.preview_at.get().is_some_and(|b| b.contains(&pos)));
        // any other overlay (a toast, a panel, a list) hides what is under
        // it: the pointer there is on none of acme's texts -- no ⌘/⌥ pill
        // on the window behind a toast
        if only.is_none() && self.over_overlay(pos) {
            return None;
        }
        let mine = |w: WindowId| only.is_none_or(|o| o == w);
        if let Some((w, _)) = self.web_bars.iter().find(|(w, b)| mine(**w) && b.contains(&pos)) {
            return Some((Target::Web(*w), Region::WebScrollbar));
        }
        for (w, l) in &self.term_layouts {
            if !mine(*w) || !l.bounds.contains(&pos) {
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
            if (only.is_some() && v.window() != only) || !l.bounds.contains(&pos) {
                continue;
            }
            if l.scrollbar.is_some_and(|b| b.contains(&pos)) {
                return Some((Target::View(*v), Region::Scrollbar));
            }
            if l.layout_box.is_some_and(|b| b.contains(&pos)) {
                return Some((Target::View(*v), Region::LayoutBox));
            }
            if let Some(a) = l.atom_at(pos) {
                return Some((Target::View(*v), Region::Atom(a)));
            }
            return Some((Target::View(*v), Region::Text(l.offset_at(pos))));
        }
        None
    }

    /// Is `pos` on an overlay (a toast, a panel, a list) other than the
    /// stash's preview, which is a window to work in? The pointer there
    /// leaves the keys and the ring where they were.
    pub(crate) fn over_overlay(&self, pos: Point<Pixels>) -> bool {
        let peeked = self.shelf.peeking().is_some() && self.shelf.preview_at.get().is_some_and(|b| b.contains(&pos));
        !peeked && self.overlay_bounds.borrow().iter().any(|(b, _)| b.contains(&pos))
    }

    /// Where the keys go with the pointer on an overlay: where they went
    /// before it came there (the ringed window's text or terminal).
    fn kept_target(&self) -> Option<Target> {
        if let Some(t) = self.caret_term {
            let w = self.node.state.windows.values().find(|x| x.body == Body::Term(t)).map(|x| x.id)?;
            return Some(Target::Term(w, t));
        }
        self.caret_view.or(self.node.seltext).map(Target::View)
    }

    fn ctx_of(&self, view: ViewId) -> ExecCtx {
        match view {
            ViewId::Tag(w) | ViewId::Body(w) => ExecCtx::Window(w),
            ViewId::ColTag(c) => ExecCtx::Column(c),
            ViewId::Top => ExecCtx::Top,
        }
    }

    pub(crate) fn text_of(&self, view: ViewId) -> Option<Text> {
        let b = self.node.view_buffer(view).ok()?;
        Some(self.node.state.buffer(b).ok()?.text.clone())
    }

    /// A body scrolled `dy` pixels down (up when negative) under the
    /// trackpad, as a native view scrolls: by the pixel, whole lines crossed
    /// becoming the session's first line; carried on by the system's
    /// momentum, which arrives as more of the same after the finger lifts;
    /// and past either end -- the start, or the last line at the top, acme's
    /// end -- against a rubber band that a finger holds and that springs back
    /// once none does. A gesture moving back towards the text gives back
    /// what it pulled first.
    fn smooth_scroll(&mut self, v: ViewId, dy: f32, phase: TouchPhase, cx: &mut Context<Self>) {
        let Ok(b) = self.node.view_buffer(v) else { return };
        let origin = self.node.state.buffer(b).map(|b| b.view(v).origin).unwrap_or(0);
        let Some(l) = self.layouts.get(&v) else { return };
        if l.rows.is_empty() {
            return;
        }
        let lh = f32::from(l.line_height).max(1.);
        let h = f32::from(l.bounds.size.height).max(lh);
        // the rows as the scroll counts them, each a line of one height:
        // the one the origin is on, those laid out below it and a screen of
        // them above; and, where the rows laid out do not reach the start
        // or the end of the text, that far from it as the scroll can tell
        let at = l.rows.partition_point(|&r| r <= origin).saturating_sub(1);
        let base = if l.rows[0] == 0 { 0 } else { 1 << 20 };
        let line = base + at;
        let total = if l.rows_end { base + l.rows.len() } else { usize::MAX / 2 };
        let below = vec![lh; l.rows.len() - at];
        let above = vec![lh; at];
        let s = match self.smooth.get(&v).copied() {
            Some(s) if s.origin == origin => s,
            _ => Smooth::at(origin),
        };
        // how long since this body's last scroll, for momentum's speed
        let now = std::time::Instant::now();
        let dt = self.wheel_at.insert(v, now).map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(1. / 60.);
        let (mut s, new) = s.scroll(dy, phase, dt, line, total, &below, &above, lh, h);
        let to = if new == line { origin } else { l.rows[new.saturating_sub(base).min(l.rows.len() - 1)] };
        if to != origin {
            self.set_origin(v, to);
        }
        s.origin = to;
        let loose = !s.finger && (s.over != 0. || s.vel != 0.);
        self.smooth.insert(v, s);
        if loose {
            self.spring(cx);
        }
        cx.notify();
    }

    /// Bodies pulled past an end with no finger on them go back, as a
    /// native view's do: a little each frame, quickly at first.
    fn spring(&mut self, cx: &mut Context<Self>) {
        if self.springing {
            return;
        }
        self.springing = true;
        let mut last = std::time::Instant::now();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(8)).await;
            // the spring runs on the time that passed, not on the ticks
            let now = std::time::Instant::now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let going = cx
                .update(|cx| {
                    this.update(cx, |acme, cx| {
                        let mut any = false;
                        for s in acme.smooth.values_mut() {
                            any |= s.settle(dt);
                        }
                        cx.notify();
                        if !any {
                            acme.springing = false;
                        }
                        any
                    })
                    .unwrap_or(false)
                });
            if !going {
                break;
            }
        })
        .detach();
    }

    /// Scrolled by `delta` rows, as acme scrolls by the rows of its frame
    /// (a long line is many of them): the last row can come to the top.
    fn scroll_by(&mut self, view: ViewId, delta: i64) {
        let Ok(b) = self.node.view_buffer(view) else { return };
        let origin = self.node.state.buffer(b).map(|b| b.view(view).origin).unwrap_or(0);
        let Some(l) = self.layouts.get(&view) else { return };
        let to = l.row_from(origin, delta);
        if to != origin {
            self.set_origin(view, to);
        }
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
        // a click: the caret solid again, wherever it lands
        self.caret_since = std::time::Instant::now();
        self.caret_on = true;
        // and any look as you type over (typing in the argument again
        // begins another, from where the selection is then)
        self.looking = None;
        // a click in acme's part of the window takes the keyboard back
        // from any page that had it, and from the window itself when a
        // view that had it went and left it there: not only while pages
        // are up, since the loss outlives them
        crate::web::focus_ui(window);
        // a click off the address being typed leaves it as it was, and
        // so does one off the session's name; one off the sessions
        // dropped down puts them away
        self.url_edit = None;
        self.session_edit = None;
        self.session_menu = false;
        // and one off a tag's field or the path's picker puts it away
        self.tag_edit = None;
        self.picker_due = None;
        self.atom_down = None;
        if self.picker.take().is_some() || self.cwd_picker.take().is_some() {
            cx.notify();
        }
        // and one off ^F's list puts it away (its rows take their own)
        if self.completion.take().is_some() {
            cx.notify();
        }
        // and one off the command palette puts it away
        if self.commands.take().is_some() {
            cx.notify();
            return;
        }
        if self.finder.is_some() {
            self.close_finder(cx); // a click anywhere else dismisses it
            return;
        }
        if self.quick.is_some() {
            self.close_quick(cx); // as the finder: a click elsewhere
            return;
        }
        if self.selector.is_some() {
            // a click anywhere else dismisses the dropdown
            self.close_selector(cx);
            return;
        }
        if self.menu.is_some() {
            return; // held open by its button; nothing else until it closes
        }
        if matches!(self.logical_button_peek(e), MouseButton::Navigate(_)) {
            let button = self.logical_button(e);
            // B4 on a layout box (shift-B1 on a laptop): the window down to
            // its tag, the column a strip -- minimized where they stand
            let boxed = match self.locate(e.position) {
                Some((Target::View(ViewId::Tag(w)), Region::LayoutBox)) => Some(BoxTarget::Win(w)),
                Some((Target::View(ViewId::ColTag(c)), Region::LayoutBox)) => Some(BoxTarget::Col(c)),
                _ => None,
            };
            if let Some(bt) = boxed {
                self.minimize_box(bt, button, false, cx);
                return;
            }
            let at = match self.locate(e.position) {
                Some((Target::View(v), _)) => v.window(),
                Some((Target::Term(w, _), _)) | Some((Target::Web(w), _)) => Some(w),
                None => None,
            };
            if let Some(w) = at {
                self.attend(w);
                self.menu_open(w, e.position, window);
            }
            cx.notify();
            return;
        }
        self.pointer = None;
        self.last_mouse = e.position;
        // the left button going down cannot already be down: a sweep still
        // held from it is one whose release never came, and it would keep
        // this press off a layout box and make B2 or B3 a chord
        if e.button == MouseButton::Left {
            self.mouse.b1 = None;
            self.mouse.autoscroll = None;
            self.mouse.term_drag = None;
        }
        let button = self.logical_button(e);
        self.mouse.mods = e.modifiers;
        if self.chord(button, cx) {
            cx.notify();
            return;
        }
        let Some((target, region)) = self.locate(e.position) else { return };
        // a click in a window attends to it: its notification goes
        if let Some(w) = target.window() {
            self.attend(w);
        }
        // the session's own square, while a tool wants the user: the oldest
        // notification is taken, and the pointer goes where it points
        if let (Target::View(ViewId::Top), Region::LayoutBox, MouseButton::Left) = (target, region, button) {
            if self.notification_head().is_some() {
                self.take_notification(cx);
                return;
            }
        }
        // the top row past its text, bare ground: B1 there moves the Mac
        // window, as a title bar does
        if let (Target::View(ViewId::Top), Region::Text(q), MouseButton::Left) = (target, region, button) {
            let plain = !(e.modifiers.platform || e.modifiers.alt || e.modifiers.control || e.modifiers.shift);
            let at_end = self.text_of(ViewId::Top).is_some_and(|t| q >= t.len() || t.char_at(q) == '\n');
            let past = self.layouts.get(&ViewId::Top).and_then(|l| l.point_of(q)).is_some_and(|p| e.position.x > p.x + px(6.));
            if plain && e.click_count == 1 && at_end && past {
                window.start_window_move();
                return;
            }
        }
        // the second click of a double-click on a tag whose picker the
        // first brought down: the path a field, wherever it lands (the
        // tag shows the folder being typed in by now, not the name)
        if let Some((pw, at)) = self.picker_opened.take() {
            if e.click_count >= 2 && button == MouseButton::Left && at.elapsed() < crate::tagedit::DOUBLE && target == Target::View(ViewId::Tag(pw)) {
                self.picker = None;
                self.tag_edit_start(pw, crate::tagedit::Part::Path, cx);
                return;
            }
        }
        // a process's pill in the session's tag
        if let (Target::View(ViewId::Top), Region::Atom(a)) = (target, region) {
            self.press_proc(a, button, cx);
            return;
        }
        // the head of a window's tag: its path, label and verbs
        if let (Target::View(ViewId::Tag(w)), Region::Atom(a)) = (target, region) {
            if self.mouse.b1.is_none() && self.mouse.b2.is_none() {
                self.press_atom(w, a, button, e.click_count, window, cx);
                return;
            }
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
            (Target::Web(w), b @ (MouseButton::Left | MouseButton::Middle | MouseButton::Right)) => self.start_scrolling(Target::Web(w), b, e.position, window, cx),
            (Target::Web(_), _) => {}
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
            (Target::View(v), MouseButton::Middle) => match region {
                Region::Text(off) => {
                    self.mouse.b2 = Some(Drag { view: v, anchor: off });
                    self.hl = None;
                }
                Region::Scrollbar => self.start_scrolling(Target::View(v), MouseButton::Middle, e.position, window, cx),
                _ => {}
            },
            (Target::View(v), MouseButton::Right) => match region {
                Region::Text(off) => {
                    self.mouse.b3 = Some(Drag { view: v, anchor: off });
                    self.mouse.b3_reverse = e.modifiers.shift;
                    self.mouse.b3_cmd = Self::b3_cmd(e);
                    self.hl = None;
                }
                Region::Scrollbar => self.start_scrolling(Target::View(v), MouseButton::Right, e.position, window, cx),
                _ => {}
            },
            (Target::Term(w, t), button) => match (region, button) {
                (Region::Term(c, r), MouseButton::Left) => {
                    let top = self.term_top(w);
                    match self.term_layouts.get(&w).filter(|_| e.click_count >= 2) {
                        // double-clicked: what acme's text windows select,
                        // over the rows on screen -- the word, the line at
                        // either end of it, what brackets or quotes enclose
                        Some(l) => {
                            let ((c0, r0), (c1, r1)) = term_double_click(&l.rows, c, r);
                            self.term_sel = Some((w, (c0, top + r0 as u64), (c1, top + r1 as u64)));
                        }
                        None => {
                            let p = (c, top + r as u64);
                            self.term_sel = Some((w, p, p));
                            self.mouse.term_drag = Some(w);
                        }
                    }
                    self.node.activecol = self.column_of_view(ViewId::Tag(w));
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

    /// B2 or B3 while B1 sweeps: the sweep's chord, in the window the
    /// sweep is in, wherever the pointer has gone since -- another window,
    /// a page, a box, off the window altogether -- as in acme, whose
    /// textselect acts on its text for as long as a button is held.
    /// True if it was one.
    fn chord(&mut self, button: MouseButton, cx: &mut Context<Self>) -> bool {
        if let Some(d) = self.mouse.b1 {
            match button {
                MouseButton::Middle => {
                    self.mouse.chorded = true;
                    self.cut(d.view, cx);
                    return true;
                }
                MouseButton::Right => {
                    self.mouse.chorded = true;
                    self.paste(d.view, cx);
                    return true;
                }
                _ => {}
            }
        }
        if let Some(w) = self.mouse.term_drag {
            match button {
                // B1+B2 in a terminal: copy (there is nothing to cut)
                MouseButton::Middle => {
                    self.mouse.chorded = true;
                    self.term_copy(w, cx);
                    return true;
                }
                // B1+B3 in a terminal: the clipboard typed into the shell
                MouseButton::Right => {
                    self.mouse.chorded = true;
                    self.term_paste_clipboard(w, cx);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// B5, a mouse's forward button (the fifth, past B4's back): `Back`
    /// in the window under the pointer, what ⌘[ and ⇧⌘-B3 issue, on the
    /// press. Nothing while the tools menu is up, which B4 holds.
    pub fn b5_down(&mut self, _e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            return;
        }
        self.menu_command("Back", window, cx);
    }

    /// A button pressed off the window (AppKit sends it here while another
    /// is held from a press in it): only a chord means anything.
    pub fn mouse_down_out(&mut self, e: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.mouse.b1.is_none() && self.mouse.term_drag.is_none() {
            return;
        }
        let button = self.logical_button_peek(e);
        if self.chord(button, cx) {
            cx.notify();
        }
    }

    pub fn mouse_move(&mut self, e: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let pos = e.position;
        self.last_mouse = pos;
        self.hint_mods(e.modifiers);
        if self.caret_tick() {
            cx.notify();
        }
        if self.shelf.pointer_at(pos) {
            cx.notify();
        }
        let held = self.held_any();
        if self.strip_tick(pos, held) {
            cx.notify();
        }
        if self.update_hint(pos) {
            cx.notify();
        }
        // a bunny watching the mouse
        if self.bunnies {
            cx.notify();
        }
        // a scroller's lane under the pointer: its thumb shows
        let lane = match self.locate(pos) {
            Some((Target::View(v), Region::Scrollbar)) => Some(v),
            Some((Target::Term(w, _), Region::TermScrollbar)) | Some((Target::Web(w), Region::WebScrollbar)) => Some(ViewId::Body(w)),
            _ => None,
        };
        if lane != self.lane_hover {
            self.lane_hover = lane;
            cx.notify();
        }
        // the tag under the pointer: its commands come up
        let over = match self.locate(pos) {
            Some((Target::View(v @ (ViewId::Tag(_) | ViewId::ColTag(_) | ViewId::Top)), _)) => Some(v),
            _ => None,
        };
        if over != self.hover_view {
            self.hover_view = over;
            cx.notify();
        }
        // a process's pill under the pointer: its card
        let pill = if self.over_overlay(pos) { None } else { self.pill_at(pos) };
        if pill.map(|p| p.0) != self.proc_hover.map(|p| p.0) {
            self.proc_hover = pill;
            cx.notify();
        }
        // a box held: where it would land follows the pointer
        if self.mouse.box_drag.is_some() {
            cx.notify();
        }
        if self.menu.is_some() {
            self.menu_track(pos);
            self.last_mouse = pos;
            cx.notify();
            return;
        }
        if self.pointer.is_some_and(|p| (p.x - pos.x).abs() > px(1.) || (p.y - pos.y).abs() > px(1.)) {
            self.pointer = None;
            // the mouse really moved: the keys follow it again
            self.warp_onto = None;
        }
        self.last_mouse = pos;
        // the terminal under the pointer has the keyboard, and the window
        // (on a toast or a panel, whichever had them keeps them)
        let at = if self.over_overlay(pos) { self.kept_target() } else { self.locate(pos).map(|(t, _)| t) };
        let under = match at {
            Some(Target::Term(_, t)) => Some(t),
            _ => None,
        };
        let active = self.app_active;
        self.note_active(at.and_then(|t| t.window()), active);
        self.term_focus_now(under, active);
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

    pub fn mouse_up(&mut self, e: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let button = if e.button == MouseButton::Left { self.mouse.left_as.take().unwrap_or(MouseButton::Left) } else { e.button };
        self.last_mouse = e.position;
        if self.release_atom(e.position, button, cx) {
            return;
        }
        if self.mouse.scrolling.is_some_and(|(_, b, _)| b == button) {
            self.mouse.scrolling = None;
        }
        if let Some((bt, b, start)) = self.mouse.box_drag {
            if b == button {
                self.mouse.box_drag = None;
                let but = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Navigate(_) => 4,
                    _ => 3,
                };
                let (op, p) = (self.row_pt(start), self.row_pt(e.position));
                // B4 is a click wherever it is let go: it never drags
                let p = if but == 4 { op } else { p };
                let r = match bt {
                    BoxTarget::Win(w) => self.node.drag_window(&mut self.log, w, but, op, p),
                    BoxTarget::Col(c) => self.node.drag_column(&mut self.log, c, but, op, p),
                    // the line goes where it is let go; a click moves nothing
                    BoxTarget::Edge(c) if (p.0 - op.0).abs() >= 2 => self.node.move_column_edge(&mut self.log, c, p.0),
                    BoxTarget::Edge(_) => Ok(()),
                };
                if let Err(err) = r {
                    eprintln!("layout: {err}");
                }
                self.after();
                cx.notify();
                return;
            }
        }
        if let Some((w, b, cell, press)) = self.mouse.term_sweep {
            if b == button {
                self.mouse.term_sweep = None;
                let hl = self.term_hl.take();
                let swept = hl.filter(|(_, _, p0, p1)| p0 != p1).and_then(|(_, _, p0, p1)| self.term_grid_text(w, p0, p1));
                // acme's execute: B2 inside the selection takes the
                // selection, not the word under the pointer
                let selected = match (button, self.term_sel) {
                    (MouseButton::Middle, Some((sw, a, b))) if sw == w && in_selection(a, b, press) => self.term_grid_text(w, a, b),
                    _ => None,
                };
                let text = match (swept.or(selected), button) {
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
        // gpui takes ctrl-tab for typing (tab has a character, control or
        // not) and hides the pointer for it; while control is down, no key
        // is typing, so none hides it
        if e.modifiers.control != prev.control {
            cx.set_cursor_hide_mode(if e.modifiers.control { gpui::CursorHideMode::Never } else { gpui::CursorHideMode::OnTyping });
        }
        // ⌘ or ⌥ pressed or let go over text: the underline follows
        let at = self.last_mouse;
        if self.update_hint(at) {
            cx.notify();
        }
        // control let go: the ctrl-tab walk ends where it stands
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
                // the lane is at the body's right
                (l.bounds, point(l.bounds.right() - px(SCROLLWID as f32 / 2.), y))
            }
            Target::Term(w, _) => {
                let Some(l) = self.term_layouts.get(&w) else { return false };
                (l.bounds, point(l.bounds.right() - px(SCROLLWID as f32 / 2.), y))
            }
            Target::Web(w) => {
                let Some(b) = self.web_bars.get(&w).copied() else { return false };
                (b, point(b.left() + px(SCROLLWID as f32 / 2.), y))
            }
        };
        let y = y.clamp(bounds.top(), bounds.bottom());
        match target {
            Target::View(v) => self.scrollbar_click(v, point(pos.x, y), dir),
            Target::Term(w, t) => self.term_scrollbar_click(w, t, point(pos.x, y), dir),
            Target::Web(w) => self.web_scrollbar_click(w, point(pos.x, y), dir),
        }
        let at = point(pos.x, y);
        crate::warp::move_to(window, at);
        self.pointer = Some(at);
        self.update_hint(at);
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
        self.term_snarf(t, p0, p1, cx);
    }

    /// The text of terminal `t` from `p0` to `p1` (column, history line;
    /// the end exclusive) to the snarf buffer and the clipboard: the
    /// server has the scrollback.
    fn term_snarf(&mut self, t: TermId, p0: (u16, u64), p1: (u16, u64), cx: &mut Context<Self>) {
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

    /// ⌘↑ (⌘↓): the terminal's view to the prompt above (below) the
    /// one at its top, as the shell marked them (OSC 133).
    fn term_jump(&mut self, t: TermId, up: bool) {
        let Some(term) = self.node.state.terms.get(&t) else { return };
        let top = term.top;
        let to = if up {
            term.marks.iter().rev().map(|m| m.prompt).find(|&p| p < top)
        } else {
            term.marks.iter().map(|m| m.prompt).find(|&p| p > top)
        };
        if let Some(to) = to {
            self.term_scroll(t, to as isize - top as isize);
        }
    }

    /// ⌘⇧C: the output of the last command the shell marked as ended, to
    /// the snarf buffer and the clipboard.
    fn term_copy_last(&mut self, t: TermId, cx: &mut Context<Self>) {
        let Some(term) = self.node.state.terms.get(&t) else { return };
        let last = term.marks.iter().rev().find_map(|m| match (m.output, m.end) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => None,
        });
        if let Some((a, b)) = last {
            self.term_snarf(t, (0, a), (0, b), cx);
        }
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
    /// (built on the window's path, the URL, the first time), hidden
    /// while a gpui overlay would be under it.
    pub fn web_place(&mut self, w: WindowId, bounds: gpui::Bounds<Pixels>, window: &Window) {
        let name = self.node.window_path(w);
        if name.is_empty() {
            return;
        }
        if self.webs.is_empty() && !self.webs.armed() {
            // the first view: the plane and the proxy come from the link now
            self.webs = Webs::new(self.io_plane(), self.wake.clone());
        }
        // shown under an overlay too: a hole is cut where the overlay is
        // (`Webs::set_holes`), the page live around it
        let visible = true;
        if std::env::var_os("APEX_WEB_DEBUG").is_some() {
            eprintln!("web: place {w} {name} at {bounds:?} body {:?}", self.node.state.window(w).map(|x| x.body));
        }
        match self.node.state.window(w).map(|x| x.body) {
            Ok(Body::Html(b)) => {
                // the buffer's HTML as a page, following its every version
                let Ok(buf) = self.node.state.buffer(b) else { return };
                let (text, version) = (buf.text.to_string(), buf.version);
                // a page is at a directory (its links are relative to it)
                // or a file (its preview's links, to the file's)
                let dir = match name.strip_suffix('/') {
                    Some(d) => d.to_string(),
                    None => std::path::Path::new(&name).parent().map(|d| d.display().to_string()).unwrap_or_default(),
                };
                self.webs.place_html(w, &text, version, &dir, bounds, window, visible);
                // a preview follows dot in its source (WEB.md §3.3)
                if let Some(line) = self.preview_source_line(w) {
                    self.webs.follow_line(w, line);
                }
            }
            _ => self.webs.place(w, &name, bounds, window, visible),
        }
    }

    /// For a preview of FILE: the line (from 1) dot is on in FILE's
    /// window, when it is open.
    fn preview_source_line(&self, page: WindowId) -> Option<usize> {
        if self.node.window_kind(page) != WinKind::Preview {
            return None;
        }
        let w = self.node.window_of(&self.node.window_path(page), WinKind::File)?;
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
        // an address being typed keeps the keys from the pages too
        // and a walk held open by a modifier: the key coming up ends it
        self.menu.is_some() || self.finder.is_some() || self.selector.is_some() || self.url_edit.is_some() || self.commands.is_some() || self.switcher.is_some() || self.overview.is_some() || self.session_edit.is_some() || self.session_menu || self.tag_edit.is_some() || self.picker.is_some() || self.cwd_picker.is_some() || self.quick.is_some()
    }

    /// A web window's handle pressed (its header draws it, not a tag):
    /// acme's box, as any window's.
    /// B4 on a layout box -- shift-B1 on a laptop, as everywhere -- the
    /// window minimized, down to its tag where it stands, or the column, a
    /// strip where it stands. True when it was. (`shift`: a press heard
    /// by its own element, as B1, before the mapping to B4.)
    fn minimize_box(&mut self, bt: BoxTarget, button: MouseButton, shift: bool, cx: &mut Context<Self>) -> bool {
        let b4 = matches!(button, MouseButton::Navigate(_)) || (shift && button == MouseButton::Left);
        if !b4 || self.mouse.b1.is_some() {
            return false;
        }
        let r = match bt {
            BoxTarget::Win(w) => self.node.minimize_window(&mut self.log, w),
            BoxTarget::Col(c) => self.node.minimize_column(&mut self.log, c),
            BoxTarget::Edge(_) => return false,
        };
        if let Err(err) = r {
            eprintln!("layout: {err}");
        }
        self.after();
        cx.notify();
        true
    }

    pub fn press_handle(&mut self, w: WindowId, button: MouseButton, pos: Point<Pixels>, shift: bool, cx: &mut Context<Self>) {
        if self.minimize_box(BoxTarget::Win(w), button, shift, cx) {
            cx.stop_propagation();
            return;
        }
        self.url_edit = None;
        if self.mouse.b1.is_none() {
            self.mouse.box_drag = Some((BoxTarget::Win(w), button, pos));
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// What the pages did: a navigation moves the window's path and the
    /// navigation stack (`WebNavigate`); titles are not kept yet.
    fn web_events(&mut self) {
        if self.webs.is_empty() {
            return;
        }
        for (w, ev) in self.webs.drain() {
            match ev {
                WebEvent::Navigated(url) => {
                    self.webs.navigated(w, &url);
                    if self.node.window_path(w) != url {
                        perform(&mut self.node, &mut self.log, vec![Proposal::WebNavigate { window: w, url }]);
                    }
                }
                WebEvent::Title(_) => {}
                WebEvent::Reload => self.webs.reload(w),
                // a link followed in a page of ours that leaves the host:
                // the system's browser, as a link in a document is; one
                // to the host's files, or its loopback (which only the
                // host reaches, through Web's proxy), a window on it
                WebEvent::Link(url) if (url.starts_with("http://") || url.starts_with("https://")) && apex_server::plane::alias_loopback_url(&url) == url => {
                    if let Err(e) = std::process::Command::new("/usr/bin/open").arg(&url).spawn() {
                        eprintln!("web: open {url}: {e}");
                    }
                }
                WebEvent::Link(url) => self.goto(Loc { session: None, name: url, pos: Pos::Keep }),
                // a file link with a line: the file, at that line
                WebEvent::Open(path, line) => self.goto(Loc { session: None, name: path, pos: line.map(Pos::Line).unwrap_or(Pos::Keep) }),
                WebEvent::Loading(on) => self.webs.set_loading(w, on),
                WebEvent::Down => self.toasts.clear(),
                // a code block's copy handle: into the snarf buffer, and
                // the clipboard with it
                WebEvent::Copy(text) => {
                    let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: text.clone() }));
                    self.clips.push(text);
                    self.after();
                }
                // a page with diagrams to draw: mermaid for it
                WebEvent::Mermaid => self.webs.give_mermaid(w),
                // where the page is scrolled, for its scrollbar
                WebEvent::Scroll { top, height, view } => self.webs.set_scroll(w, top, height, view),
                // the page's cursor: set now, if the pointer is on that page
                WebEvent::Cursor(css) => {
                    self.webs.set_cursor(w, &css);
                    self.page_cursor_now = true;
                }
                // the host's loopback, by its bare name: through the proxy
                WebEvent::Reroute(url) => {
                    self.webs.load(w, &url);
                    if self.node.window_path(w) != url {
                        perform(&mut self.node, &mut self.log, vec![Proposal::WebNavigate { window: w, url }]);
                    }
                }
            }
        }
    }

    /// `logical_button` without recording it.
    /// ⌘ held (⌘-click is B3) or ⌥ (⌥-click is B2), no button down:
    /// what a click at `pos` would take is underlined, as an editor
    /// underlines a link under ⌘ -- the selection when the pointer is in
    /// it, else the word B3 would look for or open, or the one B2 would
    /// run. True when it changed.
    /// ⌘ and ⌥ as an event says they are now. Their release can go
    /// where gpui never hears it -- to a page with the keys, another
    /// window, another app -- and the pill (and the hand with it) would
    /// stay on every move after, until a click; the pointer's own events
    /// say what is held. (Control is left to `modifiers_changed`, which
    /// ends the ctrl-tab walk on its release.)
    pub fn hint_mods(&mut self, m: gpui::Modifiers) {
        self.mouse.mods.platform = m.platform;
        self.mouse.mods.alt = m.alt;
    }

    pub fn update_hint(&mut self, pos: Point<Pixels>) -> bool {
        let m = self.mouse.mods;
        let held = self.mouse.b1.is_some() || self.mouse.b2.is_some() || self.mouse.b3.is_some() || self.mouse.box_drag.is_some();
        let kind = if held || m.shift || m.control {
            None
        } else if m.platform && !m.alt {
            Some(HlKind::Look)
        } else if m.alt && !m.platform {
            Some(HlKind::Exec)
        } else {
            None
        };
        let new = kind.and_then(|k| match self.locate(pos) {
            Some((Target::View(v), Region::Text(off))) => {
                let d = Drag { view: v, anchor: off };
                let (a, z) = match self.explicit_range_at(d) {
                    Some((_, r)) => r,
                    None => {
                        let t = self.text_of(v)?;
                        expand(&t, off, if k == HlKind::Exec { is_exec_char } else { is_file_char })
                    }
                };
                (a < z).then_some((v, a, z, k))
            }
            _ => None,
        });
        let changed = new != self.hint;
        self.hint = new;
        changed
    }

    /// How much of a scroller's thumb shows, as macOS's overlay scrollers
    /// do: all of it while its text (terminal, page) moves, while the
    /// pointer is in its lane or dragging it; fading a second after it
    /// stopped; none otherwise. `key` names the scroller (a text, or a
    /// window's body), `at` where it is now. And whether it is changing,
    /// to be drawn again.
    /// Is the pointer in `key`'s scroller lane: its gutter out, over the
    /// text's edge, and B1 B2 B3 acme's scrollbar there.
    pub fn lane_open(&self, key: ViewId) -> bool {
        self.lane_hover == Some(key)
    }

    pub fn scroller(&mut self, key: ViewId, at: u64) -> (f32, bool) {
        if self.scroll_pos.insert(key, at).is_some_and(|was| was != at) {
            self.scrolled_at.insert(key, std::time::Instant::now());
        }
        let held = self.lane_hover == Some(key)
            || matches!(self.mouse.scrolling, Some((t, _, _)) if match t {
                Target::View(v) => v == key,
                Target::Term(w, _) | Target::Web(w) => ViewId::Body(w) == key,
            });
        if held {
            return (1., false);
        }
        match self.scrolled_at.get(&key).map(|t| t.elapsed().as_secs_f32()) {
            Some(e) if e < 0.8 => (1., true),
            Some(e) if e < 1.1 => (1. - (e - 0.8) / 0.3, true),
            _ => (0., false),
        }
    }

    /// A force click (a trackpad pressed hard, macOS's "look up"): B3.
    /// The press so far was B1's; at the deep press it becomes a B3 press
    /// where it is, and its release is B3's -- a Look, or what the
    /// plumber opens. Only a press that has not swept anything yet, and
    /// only once a press.
    pub fn mouse_pressure(&mut self, e: &gpui::MousePressureEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.stage != gpui::PressureStage::Force || self.mouse.left_as != Some(MouseButton::Left) {
            return;
        }
        let swept = match self.mouse.b1 {
            Some(Drag { view, anchor }) => self.node.state.buffer(match self.node.view_buffer(view) {
                Ok(b) => b,
                Err(_) => return,
            }).map(|b| {
                let v = b.view(view);
                v.q0 != anchor || v.q1 != anchor
            }).unwrap_or(false),
            None if self.mouse.term_drag.is_some() => self.term_sel.is_some_and(|(_, a, b)| a != b),
            None => return,
        };
        if swept {
            return;
        }
        // B1's press is let go of, unfinished; B3's is made in its place
        self.mouse.b1 = None;
        self.mouse.term_drag = None;
        self.term_sel = None;
        self.mouse.left_as = None;
        let down = MouseDownEvent { button: MouseButton::Left, position: e.position, modifiers: gpui::Modifiers { platform: true, ..Default::default() }, click_count: 1, first_mouse: false };
        self.mouse_down(&down, window, cx);
        cx.notify();
    }

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
        let items = apex_core::plumb::verbs_for(&self.node.state.meta.rules, &self.node.window_path(w), self.node.window_kind(w), Some(w), self.node.window_owner(w));
        if items.is_empty() {
            return;
        }
        // the items measured in the face the menu sets them in, a tag's
        let fs = crate::text_element::font_for(false);
        let run = |len: usize| gpui::TextRun { len, font: fs.font.clone(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        let maxwid = items.iter().map(|i| f32::from(window.text_system().shape_line(i.clone().into(), fs.size, &[run(i.len())], None).width).ceil() as i32).max().unwrap_or(0);
        let checked = self.menu_last.as_ref().and_then(|l| items.iter().position(|i| i == l));
        // its rows a tag's lines; the screen, for menuhit, acme's area
        let ih = f32::from(crate::text_element::tag_line_height()) as i32;
        let m = menu::Menu::place(w, items, checked, maxwid, ih, self.row_pt(at), self.node.state.layout.r);
        // moveto: the pointer onto the item, so a click alone repeats it
        let ir = m.item_rect(m.lasti);
        let center = point(px(((ir.x0 + ir.x1) / 2) as f32 + self.left()), px(((ir.y0 + ir.y1) / 2) as f32 + self.top()));
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

    /// acme's `textcomplete`, inline: the path fragment before `q0` goes
    /// to the server, which knows the file system; the names that complete
    /// it come back to be typed in or chosen from (`completion.rs`).
    pub fn complete(&mut self, v: ViewId, q0: usize) {
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
                let names = server.candidates(&dir, &prefix);
                self.candidates.push(apex_server::proto::Candidates { view: v, at: q0, prefix, names });
            }
            Backend::Remote(link) => link.send(&ClientMsg::Candidates { view: v, ctx, at: q0, prefix }),
        }
        self.after();
    }

    fn scrollbar_click(&mut self, view: ViewId, pos: Point<Pixels>, dir: i64) {
        let Some(l) = self.layouts.get(&view) else { return };
        let frac = ((pos.y - l.bounds.top()) / l.bounds.size.height).clamp(0., 1.);
        let fit = l.lines_that_fit() as f32;
        if dir == 0 {
            if let Some(t) = self.text_of(view) {
                self.set_origin(view, bar_origin(&t, frac));
            }
        } else {
            let n = ((fit * frac) as i64).max(1);
            self.scroll_by(view, n * dir);
        }
    }

    /// acme's scrollbar on a page: button 1 takes the page back by as
    /// far as the pointer is down the bar, button 3 forward by as much
    /// (what is at the pointer comes to the top), button 2 to the part of
    /// the page as far down as the pointer is down the bar.
    fn web_scrollbar_click(&mut self, w: WindowId, pos: Point<Pixels>, dir: i64) {
        let Some(b) = self.web_bars.get(&w).copied() else { return };
        let down = f64::from(f32::from(pos.y - b.top()));
        // at least a line, as acme scrolls at least one
        let step = down.max(16.);
        match dir {
            -1 => self.webs.scroll_by(w, -step),
            1 => self.webs.scroll_by(w, step),
            _ => self.webs.scroll_to_fraction(w, (down / f64::from(f32::from(b.size.height)).max(1.)).clamp(0., 1.)),
        }
    }

    fn term_scrollbar_click(&mut self, w: WindowId, t: TermId, pos: Point<Pixels>, dir: i64) {
        let Some(l) = self.term_layouts.get(&w) else { return };
        let frac = ((pos.y - l.bounds.top()) / l.bounds.size.height).clamp(0., 1.);
        let rows = l.rows.len();
        if dir == 0 {
            // B2: the line as far down the history and screen together as
            // the pointer is down the bar comes to the top, as a text's
            // does (the screen's last rows at most)
            let Some(term) = self.node.state.terms.get(&t) else { return };
            let last = term.total.saturating_sub(rows as u64);
            let to = ((term.total as f64 * frac as f64) as u64).min(last);
            let delta = to as i64 - term.top as i64;
            if delta != 0 {
                self.term_scroll(t, delta as isize);
            }
            return;
        }
        let n = ((rows as f32 * frac) as isize).max(1);
        self.term_scroll(t, n * dir as isize);
    }

    pub fn scroll_wheel(&mut self, e: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // an overlay up has the wheel (its list scrolls itself)
        if self.selector.is_some() || self.finder.is_some() || self.quick.is_some() {
            return;
        }
        let Some((target, region)) = self.locate(e.position) else { return };
        if let Target::Web(w) = target {
            let dy = match e.delta {
                ScrollDelta::Lines(p) => -f64::from(f32::from(p.y)) * 40.,
                ScrollDelta::Pixels(p) => -f64::from(f32::from(p.y)),
            };
            self.webs.scroll_by(w, dy);
            return;
        }
        // a body under the trackpad (a device that says how far, in pixels):
        // scrolled as a native view is; a wheel that clicks by lines keeps
        // acme's steps
        if let (Target::View(v @ ViewId::Body(_)), ScrollDelta::Pixels(p)) = (target, e.delta) {
            self.smooth_scroll(v, -f32::from(p.y), e.touch_phase, cx);
            return;
        }
        let lh = match target {
            Target::Term(w, _) => self.term_layouts.get(&w).map(|l| l.line_height),
            Target::View(v) => self.layouts.get(&v).map(|l| l.line_height),
            Target::Web(_) => None,
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
            Target::Web(_) => {}
        }
        cx.notify();
    }

    // ---- keyboard ------------------------------------------------------------

    /// Keys go to the text under the pointer, as in acme.
    pub fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // typing: the caret solid while it goes on
        self.caret_since = std::time::Instant::now();
        self.caret_on = true;
        // ctrl-tab: the next session, switched to live, while control is
        // held; escape then goes back to where it began
        {
            let ks = &e.keystroke;
            // the overview up: the keys are its (the arrows, return, escape)
            if self.overview.is_some() {
                let key = ks.key.clone();
                self.overview_key(&key, window, cx);
                return;
            }
            if ks.modifiers.control && ks.key == "tab" {
                self.switcher_step(ks.modifiers.shift, window, cx);
                return;
            }
            if self.switcher.is_some() {
                if ks.key == "escape" {
                    self.close_switcher(window, cx);
                }
                return;
            }
        }
        // ^F's list: ↑ ↓ return tab escape are its; the rest go to the
        // text, and the list follows
        if self.completion.is_some() && !e.keystroke.modifiers.platform && self.completion_key(&e.keystroke.key, cx) {
            return;
        }
        if self.finder.is_some() {
            let ks = &e.keystroke;
            self.finder_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, window, cx);
            return;
        }
        if self.quick.is_some() && !(e.keystroke.modifiers.platform && e.keystroke.key == "o") {
            let ks = &e.keystroke;
            self.quick_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, window, cx);
            return;
        }
        if self.url_edit.is_some() {
            let ks = &e.keystroke;
            self.url_edit_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, cx);
            return;
        }
        if self.tag_edit.is_some() {
            let ks = &e.keystroke;
            self.tag_edit_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, cx);
            return;
        }
        if self.picker.is_some() {
            let ks = &e.keystroke;
            self.picker_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, cx);
            return;
        }
        if self.cwd_picker.is_some() {
            let ks = &e.keystroke;
            self.cwd_picker_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, cx);
            return;
        }
        if self.session_edit.is_some() {
            let ks = &e.keystroke;
            self.session_edit_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, window, cx);
            return;
        }
        if self.session_menu && e.keystroke.key == "escape" {
            self.session_menu = false;
            cx.notify();
            return;
        }
        if self.commands.is_some() {
            let ks = &e.keystroke;
            self.commands_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, cx);
            return;
        }
        if self.selector.is_some() {
            let ks = &e.keystroke;
            self.selector_key(&ks.key, ks.key_char.as_deref(), &ks.modifiers, window, cx);
            return;
        }
        // the pointer on a page: the key was the page's, which let it go
        // (an arrow at its top or foot) and WebKit passed it up to us --
        // not the last selected text's, a window beside it
        if !e.keystroke.modifiers.platform && crate::web::native_mouse(window).and_then(|p| self.webs.window_at(p)).is_some() {
            return;
        }
        let pos = self.pointer(window);
        let target = match self.locate(pos) {
            // on a toast or a panel: where the keys went before
            None if self.over_overlay(pos) => match self.kept_target() {
                Some(t) => t,
                None => return,
            },
            // a page's scrollbar holds no text: the last selected does
            Some((Target::Web(_), _)) | None => match self.node.seltext {
                Some(v) => Target::View(v),
                None => return,
            },
            Some((t, _)) => t,
        };
        // a key in a window attends to it: its notification goes
        if let Some(w) = target.window() {
            self.attend(w);
        }
        let ks = &e.keystroke;
        let m = ks.modifiers;
        match target {
            Target::Term(_, t) => {
                // the shell's prompt marks (OSC 133): ⌘↑ ⌘↓ from prompt to
                // prompt, ⌘⇧C the last command's output
                if m.platform && !m.shift && (ks.key == "up" || ks.key == "down") {
                    self.term_jump(t, ks.key == "up");
                } else if m.platform && m.shift && ks.key == "c" {
                    self.term_copy_last(t, cx);
                } else if m.platform && ks.key == "v" {
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
                self.text_key(v, ks, cx);
                // in a tag: its Look's argument, looked for as it is typed;
                // in a body: any look there is over
                match v {
                    ViewId::Tag(w) => self.live_look(w),
                    ViewId::Body(w) if self.looking.as_ref().is_some_and(|l| l.window == w) => self.looking = None,
                    _ => {}
                }
            }
            Target::Web(_) => {} // not reached: a page's scrollbar takes no keys
        }
        self.completion_follow(cx);
        cx.notify();
    }

    /// The window acme would act on: the one under the pointer, else the
    /// last selected text's.
    pub(crate) fn window_at_pointer(&self, window: &Window) -> Option<WindowId> {
        let pos = self.pointer(window);
        if let Some(w) = self.webs.window_at(pos) {
            return Some(w); // a page: the window is its
        }
        match self.locate(pos) {
            Some((Target::View(v), _)) => v.window().or_else(|| self.node.seltext.and_then(|s| s.window())),
            Some((Target::Term(w, _), _)) | Some((Target::Web(w), _)) => Some(w),
            None => self.node.seltext.and_then(|s| s.window()),
        }
    }

    /// A menu item that is an acme command: `Put`, `Del`, `New`, `Edit ,`
    /// run in the window under the pointer, as B2 there would.
    pub fn menu_command(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.selector.is_some() || self.finder.is_some() || self.quick.is_some() {
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

    /// The window became active or inactive: the terminal under the
    /// pointer loses the keyboard with the app, and has it back with it.
    /// The pointer is left where it is: put back where it last was here,
    /// it jumped under the hand whenever the window came forward (a new
    /// window, the dock, cmd-tab).
    pub fn window_activated(&mut self, active: bool, window: &mut Window) {
        // ⌘ or ⌥ let go while another window (or app) had the keys: as
        // they are now, and the pill with them
        self.hint_mods(window.modifiers());
        let at = self.last_mouse;
        self.update_hint(at);
        let under = self.term_under_pointer;
        self.note_active(self.win_under_pointer, active);
        self.term_focus_now(under, active);
    }

    /// What the Edit menu (and its shortcuts, which arrive as actions
    /// before any key event) does: acme's rule, the text under the
    /// pointer, else the last selected text.
    pub fn menu_edit(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        // an overlay (the session picker, the finder) has the keyboard:
        // the Edit menu works on its field, not on the text below
        if self.selector.is_some() || self.finder.is_some() || self.url_edit.is_some() || self.commands.is_some() || self.quick.is_some() {
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
        let pos = self.pointer(window);
        let target = match self.locate(pos) {
            // on a toast or a panel: where the keys went before
            None if self.over_overlay(pos) => match self.kept_target() {
                Some(t) => t,
                None => return,
            },
            // a page's scrollbar holds no text: the last selected does
            Some((Target::Web(_), _)) | None => match self.node.seltext {
                Some(v) => Target::View(v),
                None => return,
            },
            Some((t, _)) => t,
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
            Target::Web(_) => {} // not reached: a page's scrollbar holds no text
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
                    // fn-backspace is plan9port's Kdel, DEL, which acme types
                    // like any key and win takes for the interrupt: in the
                    // body of a window a process keeps, typed as DEL, so
                    // win interrupts what runs; elsewhere the Mac's forward
                    // delete, since DEL typed into a file is only a hazard
                    if matches!(v, ViewId::Body(w) if self.node.window_live(w)) {
                        self.type_text(v, "\u{7f}");
                    } else {
                        let _ = self.node.delete_forward(&mut self.log, v);
                    }
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

    /// A line for the first column's errors: where the app tells the
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
        let word = text.trim().split_whitespace().next().unwrap_or("").to_string();
        // End: the session ended (as apex end-session does), the window
        // closed -- as Del closes a window: a modified file warns first
        // (Del's rule, `Node::session_clean`), and End again with nothing
        // changed ends it all the same. -f ends it at once.
        if word == "End" {
            let force = text.split_whitespace().any(|w| w == "-f");
            if !force {
                match self.node.session_clean(&mut self.log) {
                    Ok(true) => {}
                    Ok(false) => {
                        self.after();
                        cx.notify();
                        return;
                    }
                    Err(e) => {
                        self.notice(&format!("End: {e}\n"));
                        return;
                    }
                }
            }
            // asked already: the daemon's own check (another rule) is not
            // asked again
            self.end_session(true, cx);
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
                            self.term_type(t, text);
                            self.after();
                            return;
                        }
                    }
                }
            }
        }
        // a word a page from a buffer says it answers (apex diff's Prev and
        // Next): run in the page, not as a command
        if let ExecCtx::Window(w) = ctx {
            if let Ok(Body::Html(b)) = self.node.state.window(w).map(|x| x.body) {
                // the head is at the top: no need of the rest of the page
                let html = self.node.state.buffer(b).map(|b| b.text.slice(0, b.text.len().min(8192))).unwrap_or_default();
                if crate::web::page_verbs(&html).iter().any(|v| *v == word) {
                    self.webs.verb(w, &word);
                    return;
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

/// `by` steps from `at` around a ring of `len`: past the end is back at
/// the start, and before the start is at the end.
fn around(at: usize, len: usize, by: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (at as isize + by).rem_euclid(len as isize) as usize
}

/// acme's double-click (`node::double_click`) on a terminal's screen, so
/// a terminal selects as a text window does: the rows as lines, each
/// without the blanks after its text, so that a click anywhere past the
/// end of a row is a click at the end of its line -- which takes the
/// line. The selection comes back as two (column, row) cells, the end
/// exclusive. Rows are one character a cell (`TermLayout::rows`), so a
/// character's place in its row is its column.
fn term_double_click(rows: &[String], c: usize, r: usize) -> ((usize, usize), (usize, usize)) {
    let lines: Vec<&str> = rows.iter().map(|s| s.trim_end()).collect();
    let mut starts = Vec::with_capacity(lines.len());
    let mut text = String::new();
    let mut at = 0;
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            text.push('\n');
            at += 1;
        }
        starts.push(at);
        text.push_str(l);
        at += l.chars().count();
    }
    if lines.is_empty() {
        return ((c, r), (c, r));
    }
    let r = r.min(lines.len() - 1);
    let q = starts[r] + c.min(lines[r].chars().count());
    let (q0, q1) = apex_core::node::double_click(&apex_core::Text::new(&text), q);
    let cell = |o: usize| {
        let row = starts.iter().rposition(|&s| s <= o).unwrap_or(0);
        (o - starts[row], row)
    };
    (cell(q0), cell(q1))
}

/// Is the cell `p` within the selection `a`..`b` (either way round)?
/// Cells are (column, line), and lines order before columns.
fn in_selection(a: (usize, u64), b: (usize, u64), p: (usize, u64)) -> bool {
    if a == b {
        return false;
    }
    let (lo, hi) = if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) };
    (lo.1, lo.0) <= (p.1, p.0) && (p.1, p.0) <= (hi.1, hi.0)
}

/// Where a pulsing handle is this instant: 0 at its own colour, 1 at
/// pale, back and forth over a second and a half.
fn breath() -> f32 {
    const PERIOD: u128 = 1440;
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) % PERIOD;
    let t = ms as f32 / PERIOD as f32;
    if t < 0.5 {
        t * 2.0
    } else {
        2.0 - t * 2.0
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
            PlumbStep::AskTool { tool, .. } => step = server.plumb_failed(node, id, format!("no tool {tool} in-process")),
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
pub(crate) fn identified(url: &SessionUrl, node: &Node) -> SessionUrl {
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

#[cfg(test)]
mod tab_ring_tests {
    use super::around;

    #[test]
    fn a_step_off_either_end_of_the_tabs_comes_round() {
        assert_eq!(around(0, 3, 1), 1);
        assert_eq!(around(2, 3, 1), 0, "past the last is the first");
        assert_eq!(around(0, 3, -1), 2, "before the first is the last");
        assert_eq!(around(0, 1, 1), 0, "one tab stays where it is");
        assert_eq!(around(0, 0, 1), 0, "and none is nowhere to go");
    }
}

#[cfg(test)]
mod term_double_click_tests {
    use super::term_double_click;

    fn rows(lines: &[&str]) -> Vec<String> {
        // as the screen has them: every row the terminal's width
        lines.iter().map(|l| format!("{l:<40}")).collect()
    }

    #[test]
    fn a_double_click_in_a_terminal_selects_as_a_text_window_does() {
        let screen = rows(&["ls -l src/main.rs", "total 8", "f(a, b) and 'q'"]);
        // in a word: the word (acme's: `.` and `/` end it)
        assert_eq!(term_double_click(&screen, 1, 0), ((0, 0), (2, 0)));
        assert_eq!(term_double_click(&screen, 12, 0), ((10, 0), (14, 0)), "main");
        // at the end of a line, and anywhere in the blanks past it: the
        // line, and its newline with it, as a text window takes it
        assert_eq!(term_double_click(&screen, 17, 0), ((0, 0), (0, 1)));
        assert_eq!(term_double_click(&screen, 35, 0), ((0, 0), (0, 1)), "past the text is the line's end");
        // at the start of a line: the line
        assert_eq!(term_double_click(&screen, 0, 1), ((0, 1), (0, 2)));
        // inside brackets and quotes: what they enclose
        assert_eq!(term_double_click(&screen, 2, 2), ((2, 2), (6, 2)), "(a, b)");
        assert_eq!(term_double_click(&screen, 13, 2), ((13, 2), (14, 2)), "'q'");
        // the last line has no newline after it, and still is a line
        assert_eq!(term_double_click(&screen, 39, 2), ((0, 2), (15, 2)));
    }
}

#[cfg(test)]
mod term_selection_tests {
    use super::in_selection;

    #[test]
    fn b2_lands_inside_a_terminal_selection_or_it_does_not() {
        // a selection over two lines: from line 3 column 5 to line 4 column 2
        let (a, b) = ((5, 3u64), (2, 4u64));
        assert!(in_selection(a, b, (5, 3)), "its first cell");
        assert!(in_selection(a, b, (9, 3)), "later on the first line");
        assert!(in_selection(a, b, (0, 4)), "the start of the last line");
        assert!(in_selection(a, b, (2, 4)), "its last cell");
        assert!(!in_selection(a, b, (4, 3)), "before it on the first line");
        assert!(!in_selection(a, b, (3, 4)), "after it on the last line");
        assert!(!in_selection(a, b, (7, 2)), "a line above");
        assert!(!in_selection(a, b, (7, 5)), "a line below");
        // swept the other way round, the same selection
        assert!(in_selection(b, a, (9, 3)));
        // nothing selected: a click is never inside it
        assert!(!in_selection(a, a, (5, 3)));
    }
}

#[cfg(test)]
mod bar_origin_tests {
    use super::bar_origin;
    use apex_core::text::Text;

    #[test]
    fn b2_on_the_scrollbar_goes_by_the_rune_into_a_long_line() {
        // one line of 10000 runes: half way down the bar is half way along it
        let t = Text::new(&"x".repeat(10000));
        assert_eq!(bar_origin(&t, 0.5), 5000);
        assert_eq!(bar_origin(&t, 0.), 0);
        assert_eq!(bar_origin(&t, 1.), 10000);
        // a line start near: moved on to it, as acme's textsetorigin does
        let t = Text::new(&format!("{}\n{}", "a".repeat(60), "b".repeat(40)));
        assert_eq!(bar_origin(&t, 0.5), 61);
        // already at one: left there
        assert_eq!(bar_origin(&Text::new("ab\ncd"), 0.6), 3);
    }
}

#[cfg(test)]
mod smooth_scroll_tests {
    use super::{rubber, unrubber, Smooth};
    use gpui::TouchPhase::{Ended, Moved, Started};

    const LH: f32 = 17.;
    const H: f32 = 600.;
    const DT: f32 = 1. / 60.;

    #[test]
    fn the_rubber_band_resists_more_the_further_it_goes_and_undoes_itself() {
        assert_eq!(rubber(0., H), 0.);
        let (a, b) = (rubber(100., H), rubber(200., H));
        assert!(a < 100. && b < 200. && b - a < a, "{a} {b}");
        assert!(rubber(1e6, H) < H);
        assert!(rubber(-100., H) == -a);
        for over in [1., 50., 200., 500.] {
            assert!((rubber(unrubber(over, H), H) - over).abs() < 0.01);
        }
    }

    #[test]
    fn scrolling_by_the_pixel_crosses_whole_lines_and_keeps_the_rest() {
        let s = Smooth::at(0);
        // 40 px down over lines of 17 and 34 (a wrapped one)
        let (s, line) = s.scroll(40., Moved, DT, 5, 100, &[17., 34.], &[], LH, H);
        assert_eq!((line, s.px), (6, 23.));
        // 30 px up: back across the line above, 17 high
        let (s, line) = s.scroll(-30., Moved, DT, line, 100, &[34.], &[17.], LH, H);
        assert_eq!((line, s.px), (5, 10.));
        assert_eq!(s.over, 0.);
    }

    #[test]
    fn a_finger_pulls_past_the_start_and_the_spring_brings_it_back_when_it_lifts() {
        let s = Smooth::at(0);
        let (s, line) = s.scroll(-80., Started, DT, 0, 100, &[], &[], LH, H);
        assert_eq!((line, s.px), (0, 0.));
        assert!(s.over > 0. && s.over < 80., "{}", s.over);
        // held: the spring leaves it
        let mut held = s;
        assert!(!held.settle(DT));
        assert_eq!(held.over, s.over);
        // let go: back to the start, frame by frame
        let (mut s, _) = s.scroll(0., Ended, DT, 0, 100, &[], &[], LH, H);
        let mut frames = 0;
        while s.settle(DT) {
            frames += 1;
            assert!(frames < 100);
        }
        assert_eq!(s.over, 0.);
        assert!(frames > 3, "it goes back over frames, not at once: {frames}");
    }

    #[test]
    fn moving_back_gives_back_the_pull_before_the_text_moves() {
        let s = Smooth::at(0);
        let (s, _) = s.scroll(-80., Started, DT, 0, 100, &[], &[], LH, H);
        let pulled = s.over;
        // a little back: only the pull shrinks
        let (s, line) = s.scroll(10., Moved, DT, 0, 100, &[17.], &[], LH, H);
        assert!(s.over < pulled && s.over > 0.);
        assert_eq!((line, s.px), (0, 0.));
        // far back: the pull is gone and the rest scrolls the text
        let (s, line) = s.scroll(400., Moved, DT, 0, 100, &[17.; 40], &[], LH, H);
        assert_eq!(s.over, 0.);
        assert!(line > 0, "{line} {}", s.px);
    }

    #[test]
    fn the_end_is_the_text_at_the_bottom_of_the_view() {
        // 40 lines of 17 left from line 60 of 100 is 680: 80 to go
        let (s, line) = Smooth::at(0).scroll(60., Started, DT, 60, 100, &[17.; 40], &[], LH, H);
        assert_eq!((line, s.px, s.over), (63, 9., 0.));
        // 60 more: 20 of it scrolls, the rest pulls past the end
        let (s, line) = s.scroll(60., Moved, DT, line, 100, &[17.; 37], &[], LH, H);
        assert_eq!((line, s.px), (64, 12.));
        assert!(s.over < 0. && s.over > -40., "{}", s.over);
        // back up: the pull goes first, then the text
        let (s, line) = s.scroll(-200., Moved, DT, line, 100, &[17.; 36], &[17.; 40], LH, H);
        assert_eq!(s.over, 0.);
        assert!(line < 64, "{line}");
        // text shorter than the view: it does not move down at all
        let (s, line) = Smooth::at(0).scroll(30., Started, DT, 0, 10, &[17.; 10], &[], LH, H);
        assert_eq!((line, s.px), (0, 0.));
        assert!(s.over < 0.);
        // one line wrapped to 100 rows, the last of the text: the trackpad
        // takes it down to its last row at the bottom of the view (the
        // view 600 high holds 35 rows and 5 pixels)
        let (s, line) = Smooth::at(0).scroll(5000., Started, DT, 0, 100, &[17.; 100], &[], LH, H);
        assert_eq!((line, s.px), (64, 12.), "the end: 100 rows less 600 px of them");
        assert!(s.over < 0.);
        // past the end already (the scrollbar put it there): only a bounce
        let (s, line) = Smooth::at(0).scroll(30., Started, DT, 90, 100, &[17.; 10], &[], LH, H);
        assert_eq!((line, s.px), (90, 0.));
        assert!(s.over < 0.);
    }

    #[test]
    fn momentum_at_an_end_bounces_out_and_back_once_whatever_the_rates() {
        // the same fling, its scrolls coming at 60 or 120 a second, the
        // spring ticked at 60 or 120: out and back, never shaking
        for (events, ticks) in [(60., 60.), (120., 60.), (60., 120.), (120., 120.)] {
            let (dt, tick) = (1. / events, 1. / ticks);
            let (mut s, line) = Smooth::at(0).scroll(1500. * dt, Moved, dt, 99, 100, &[17.], &[], LH, H);
            assert_eq!(line, 99);
            assert!(s.vel < 0., "the fling is the spring's now");
            let (mut outs, mut ins, mut last, mut frames) = (0, 0, 0f32, 0);
            let mut deepest = 0f32;
            let mut t = 0.;
            while frames < 200 {
                // momentum still coming, fading: spent
                t += tick;
                while t >= dt {
                    t -= dt;
                    let before = s.over;
                    (s, _) = s.scroll(300. * dt, Moved, dt, 99, 100, &[17.], &[], LH, H);
                    assert_eq!(s.over, before, "momentum after the end moves nothing");
                }
                let going = s.settle(tick);
                assert!(s.over <= 0., "never past the other way: {}", s.over);
                if s.over < last {
                    outs += 1;
                    assert_eq!(ins, 0, "out again after coming back: shaking ({events}/{ticks})");
                } else if s.over > last {
                    ins += 1;
                }
                deepest = deepest.min(s.over);
                last = s.over;
                frames += 1;
                if !going {
                    break;
                }
            }
            assert_eq!(s.over, 0., "back ({events}/{ticks})");
            // and the momentum that is still coming does not bounce it again
            (s, _) = s.scroll(200. * dt, Moved, dt, 99, 100, &[17.], &[], LH, H);
            assert!(!s.settle(tick) && s.over == 0. && s.vel == 0.);
            // a scroll the other way is no momentum: it moves the text
            let (_, back) = s.scroll(-40., Moved, dt, 99, 100, &[17.], &[17.; 40], LH, H);
            assert!(back < 99);
            assert!(outs > 0 && ins > 0 && deepest < -5., "a bounce: {outs} {ins} {deepest}");
            assert!((frames as f32) * tick < 1.5, "back in time: {frames} frames");
        }
    }

    #[test]
    fn a_finger_catches_a_bounce() {
        let (mut s, _) = Smooth::at(0).scroll(40., Moved, DT, 99, 100, &[17.], &[], LH, H);
        s.settle(0.05);
        assert!(s.over < 0.);
        let (s, _) = s.scroll(0., Started, DT, 99, 100, &[17.], &[], LH, H);
        let mut held = s;
        assert!(!held.settle(0.1));
        assert_eq!(held.over, s.over);
    }
}
