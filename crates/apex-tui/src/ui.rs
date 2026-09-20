//! The session as the TermKit UI sees it: the replica, laid out on a
//! cell grid, plus acme's meaning for the three buttons.
//!
//! This is the half of the old gpui client that was never about gpui —
//! hit testing, selection, chords, the layout boxes — rewritten against
//! cells instead of pixels. The drawing, and every widget, is in Swift.

use std::collections::HashMap;

use apex_core::tiling::Warp;
use apex_core::{
    Body, BufferId, ColumnId, ExecCtx, LayoutOp, Op, Shard, Span, Text, TermId, ViewId, WindowId,
};
use apex_core::node::{double_click, Erase, Executed};
use apex_core::{Log, Node};
use apex_server::proto::ClientMsg;
use apex_server::remote::Link;
use apex_server::term::TermKey;

use crate::cells::{CellInfo, Wrapped};
use crate::input::{Button, Event, Mods, Motion, NamedKey};
use crate::model::*;

/// The scrollbar's width, in cells: acme's, narrowed to what a terminal
/// can draw.
pub const SCROLLWID: i32 = 1;

/// The layout box is the first cell of a tag.
pub const BOXWID: i32 = 1;

/// Where a click landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    /// A rune offset in a text view.
    Text(usize),
    Scrollbar,
    LayoutBox,
    /// A cell of a terminal's grid.
    Term(u16, u16),
    TermScrollbar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    View(ViewId),
    Term(WindowId, TermId),
}

impl Target {
    fn window(self) -> Option<WindowId> {
        match self {
            Target::View(v) => v.window(),
            Target::Term(w, _) => Some(w),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    view: ViewId,
    anchor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoxTarget {
    Win(WindowId),
    Col(ColumnId),
}

#[derive(Default)]
struct Mouse {
    b1: Option<Drag>,
    b2: Option<Drag>,
    b3: Option<Drag>,
    /// B1 down while B2 is held: the last selection joins the command.
    chord_arg: bool,
    chorded: bool,
    b3_reverse: bool,
    box_drag: Option<(BoxTarget, Button, (i32, i32))>,
    /// A sweep in a terminal: the window, the button, and where it began.
    term_sweep: Option<(WindowId, Button, (u16, u16))>,
    term_drag: Option<WindowId>,
}

/// Where a view is drawn, for hit testing: the same rectangles the frame
/// was built from.
#[derive(Clone, Debug)]
struct Layout {
    /// The text area, scrollbar excluded.
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    /// The scrollbar, when the view has one.
    scrollbar: Option<(i32, i32, i32, i32)>,
    layout_box: Option<(i32, i32, i32, i32)>,
    wrapped: Wrapped,
    /// The first wrapped row shown.
    top: usize,
}

impl Layout {
    fn contains(&self, x: i32, y: i32) -> bool {
        let sb = self.scrollbar.map(|(a, b, c, d)| x >= a && x < c && y >= b && y < d).unwrap_or(false);
        sb || (x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1)
    }
    fn in_rect(r: Option<(i32, i32, i32, i32)>, x: i32, y: i32) -> bool {
        r.map(|(a, b, c, d)| x >= a && x < c && y >= b && y < d).unwrap_or(false)
    }
    /// The rune offset the pointer is over.
    fn offset_at(&self, x: i32, y: i32) -> usize {
        let row = self.top + (y - self.y0).max(0) as usize;
        let col = (x - self.x0).max(0) as usize;
        self.wrapped.offset(row.min(self.wrapped.len().saturating_sub(1)), col)
    }
}

#[derive(Clone, Debug)]
struct TermLayout {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    scrollbar: (i32, i32, i32, i32),
    cols: u16,
    rows: u16,
}

/// What the UI is showing over the row.
#[derive(Clone, Debug, PartialEq)]
enum Over {
    None,
    Finder { all: bool },
    Switcher,
    Message { title: String, text: String },
}

pub struct Ui {
    pub node: Node,
    pub log: Log,
    pub link: Link,
    pub session: String,
    pub connected: bool,
    pub quit: bool,

    cols: i32,
    rows: i32,
    seq: u64,
    info: CellInfo,
    mouse: Mouse,
    over: Over,
    /// Where each view was drawn last frame.
    layouts: HashMap<ViewId, Layout>,
    term_layouts: HashMap<WindowId, TermLayout>,
    /// B2's and B3's sweep, drawn while the button is down.
    hl: Option<(ViewId, usize, usize, SpanKind)>,
    /// A terminal's selection: the window and two (col, history line).
    term_sel: Option<(WindowId, (u16, u64), (u16, u64))>,
    /// The window the pointer is over.
    active: Option<WindowId>,
    /// Files closed in this session, latest first: the finder's tail.
    closed: Vec<String>,
    /// The sessions this daemon has, as of the last ask.
    sessions: Vec<(String, String)>,
    /// Positions to bring on screen, with the fraction of the window to
    /// leave above (acme's `show`).
    show_at: HashMap<ViewId, (usize, u32)>,
    warp: Option<(i32, i32)>,
    snarf_sent: String,
}

impl Ui {
    pub fn new(node: Node, log: Log, link: Link, session: String) -> Ui {
        Ui {
            node,
            log,
            link,
            session,
            connected: true,
            quit: false,
            cols: 80,
            rows: 24,
            seq: 0,
            info: CellInfo::default(),
            mouse: Mouse::default(),
            over: Over::None,
            layouts: HashMap::new(),
            term_layouts: HashMap::new(),
            hl: None,
            term_sel: None,
            active: None,
            closed: Vec::new(),
            sessions: Vec::new(),
            show_at: HashMap::new(),
            warp: None,
            snarf_sent: String::new(),
        }
    }

    // ---- the replica ---------------------------------------------------------

    /// What the gpui client's `sync` does, less what only it has.
    pub fn sync(&mut self) {
        let _ = self.node.update_tags(&mut self.log);
        self.track_closed();
        for (v, q) in self.node.take_shows() {
            self.show_at.insert(v, (q, 1));
        }
        for loc in self.node.take_gotos() {
            let _ = self.node.land(&mut self.log, &loc);
        }
        self.link.flush(&self.log);
        if let Err(e) = self.node.catch_up(&self.log) {
            eprintln!("apex-tuid: catch up: {e}");
        }
        if let Some(w) = self.node.warp.take() {
            self.take_warp(w);
        }
    }

    /// Poll the link; false when the session is gone.
    pub fn poll(&mut self) -> bool {
        let alive = self.link.poll(&mut self.node, &mut self.log);
        self.connected = alive;
        if let Some(sessions) = self.link.sessions.take() {
            self.sessions = sessions.into_iter().map(|s| (s.id, s.label)).collect();
        }
        if self.link.ended.take().is_some() {
            self.quit = true;
        }
        for w in self.link.take_made() {
            self.show(w);
        }
        // a program's output follows the window when the point it went in
        // at was on screen (acme's shouldscroll)
        let outputs = self.link.take_outputs();
        for (b, at, end) in outputs {
            let views: Vec<ViewId> = self.node.state.windows.iter().filter(|(_, x)| x.body_buffer() == Some(b)).map(|(w, _)| ViewId::Body(*w)).collect();
            for v in views {
                let origin = self.node.view_buffer(v).ok().and_then(|b| self.node.state.buffer(b).ok()).map(|b| b.view(v).origin).unwrap_or(0);
                let fits = self.layouts.get(&v).map(|l| (l.y1 - l.y0).max(1) as usize).unwrap_or(0);
                let on_screen = self
                    .text_of(v)
                    .is_some_and(|t| at >= origin && t.line_of(at.min(t.len())) < t.line_of(origin) + fits);
                if on_screen || self.show_at.get(&v).is_some_and(|(q, _)| at <= *q) {
                    self.show_at.insert(v, (end, 3));
                }
            }
        }
        self.sync();
        if std::mem::take(&mut self.node.quit_requested) {
            self.quit = true;
        }
        alive
    }

    /// Open a file named on the command line, in `col`.
    pub fn open(&mut self, col: ColumnId, name: &str) {
        self.send(ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: name.to_string() });
    }

    fn send(&self, m: ClientMsg) {
        self.link.send(&m);
    }

    fn after(&mut self) {
        self.link.flush(&self.log);
        self.sync();
    }

    fn text_of(&self, v: ViewId) -> Option<Text> {
        let b = self.node.view_buffer(v).ok()?;
        Some(self.node.state.buffer(b).ok()?.text.clone())
    }

    fn buffer_of(&self, v: ViewId) -> Option<BufferId> {
        self.node.view_buffer(v).ok()
    }

    fn ctx_of(&self, v: ViewId) -> ExecCtx {
        match v {
            ViewId::Tag(w) | ViewId::Body(w) => ExecCtx::Window(w),
            ViewId::ColTag(c) => ExecCtx::Column(c),
            ViewId::Top => ExecCtx::Top,
        }
    }

    fn tabstop(&self, v: ViewId) -> usize {
        match v {
            ViewId::Body(w) => self.node.state.window(w).map(|x| x.tabstop as usize).unwrap_or(4),
            _ => 4,
        }
    }

    fn term_of(&self, w: WindowId) -> Option<TermId> {
        match self.node.state.window(w).ok()?.body {
            Body::Term(t) => Some(t),
            _ => None,
        }
    }

    fn term_top(&self, w: WindowId) -> u64 {
        self.term_of(w).and_then(|t| self.node.state.terms.get(&t)).map(|t| t.top).unwrap_or(0)
    }

    /// Files that left the session: the finder finds them by name.
    fn track_closed(&mut self) {
        let open: Vec<String> = self.node.state.windows.keys().map(|w| self.node.window_name(*w)).collect();
        self.closed.retain(|n| !open.contains(n));
    }

    fn show(&mut self, w: WindowId) {
        let _ = self.node.reveal(&mut self.log, w);
        self.show_at.insert(ViewId::Body(w), (self.node.selection(ViewId::Body(w)).map(|(q, _)| q).unwrap_or(0), 1));
    }

    // ---- measuring -----------------------------------------------------------

    /// Lay the row out for a terminal `cols` by `rows` cells. Everything
    /// the tiling needs is measured first, exactly as the gpui client
    /// measures its fonts.
    pub fn measure(&mut self, cols: i32, rows: i32) {
        self.cols = cols.max(20);
        self.rows = rows.max(4);
        // the tag of every window, wrapped to the width it has
        let mut info = CellInfo::default();
        let wins: Vec<WindowId> = self.node.state.windows.keys().copied().collect();
        for w in wins {
            let width = self.tag_width(w);
            let (n, nl) = match self.buffer_of(ViewId::Tag(w)).and_then(|b| self.node.state.buffer(b).ok()) {
                Some(b) => {
                    let wrapped = Wrapped::of(&b.text, width, 4);
                    let ends_nl = b.text.len() > 0 && b.text.char_at(b.text.len() - 1) == '\n';
                    (wrapped.len() as i32, ends_nl)
                }
                None => (1, false),
            };
            info.tags.insert(w, (n, nl));
            let is_term = matches!(self.node.state.window(w).map(|x| x.body), Ok(Body::Term(_)));
            let lines = match self.buffer_of(ViewId::Body(w)).and_then(|b| self.node.state.buffer(b).ok()) {
                Some(b) => {
                    let v = b.view(ViewId::Body(w));
                    let width = self.body_width(w);
                    let wrapped = Wrapped::of(&b.text, width, 4);
                    let from = wrapped.row_of(v.origin.min(b.text.len()));
                    (wrapped.len() - from) as i32
                }
                None => self.rows,
            };
            info.bodies.insert(w, (lines, is_term));
        }
        self.info = info.clone();
        self.node.tiling = Box::new(info);
        let r = apex_core::Rect::new(0, 0, self.cols, self.rows);
        if self.node.state.layout.r != r {
            let _ = self.node.resize_layout(&mut self.log, r);
        }
    }

    /// The width a window's tag text has: its column, less the layout box.
    fn tag_width(&self, w: WindowId) -> i32 {
        let dx = self
            .node
            .state
            .layout
            .place_of(w)
            .and_then(|(ci, wi)| self.node.state.layout.cols.get(ci).and_then(|c| c.wins.get(wi)).map(|s| s.r.dx()))
            .unwrap_or(self.cols);
        (dx - BOXWID).max(1)
    }

    fn body_width(&self, w: WindowId) -> i32 {
        let dx = self
            .node
            .state
            .layout
            .place_of(w)
            .and_then(|(ci, wi)| self.node.state.layout.cols.get(ci).and_then(|c| c.wins.get(wi)).map(|s| s.body.dx()))
            .unwrap_or(self.cols);
        (dx - SCROLLWID).max(1)
    }

    /// acme's warp, in cells: the UI moves the pointer there if it can.
    fn take_warp(&mut self, w: Warp) {
        let at = match w {
            Warp::NewWindow(win) | Warp::WinButton(win) => self.node.state.layout.place_of(win).and_then(|(ci, wi)| {
                let s = self.node.state.layout.cols.get(ci)?.wins.get(wi)?;
                Some((s.r.x0 + 2, s.r.y0))
            }),
            Warp::ColButton(c) => self.node.state.layout.column(c).map(|c| (c.r.x0 + 2, c.r.y0)),
            Warp::Closed { next, .. } => next.and_then(|win| {
                self.node.state.layout.place_of(win).and_then(|(ci, wi)| {
                    let s = self.node.state.layout.cols.get(ci)?.wins.get(wi)?;
                    Some((s.r.x0 + 2, s.r.y0))
                })
            }),
            _ => None,
        };
        self.warp = at;
    }

    // ---- the frame -----------------------------------------------------------

    /// Build what the UI draws, recording where everything went so the
    /// next click can be placed.
    pub fn frame(&mut self) -> Frame {
        self.seq += 1;
        self.layouts.clear();
        self.term_layouts.clear();
        let cols: Vec<ColumnId> = self.node.state.layout.cols.iter().map(|c| c.id).collect();
        let top = self.top_view();
        let columns = cols.into_iter().filter_map(|c| self.column_view(c)).collect();
        let overlay = self.overlay();
        let snarf = {
            let s = self.node.state.layout.snarf.clone();
            if s != self.snarf_sent {
                self.snarf_sent = s.clone();
                Some(s)
            } else {
                None
            }
        };
        Frame {
            seq: self.seq,
            cols: self.cols,
            rows: self.rows,
            title: self.title(),
            top,
            columns,
            overlay,
            notification: self.node.notifications().next().map(|n| self.node.window_name(n.window)),
            warp: self.warp.take(),
            snarf,
            connected: self.connected,
            fenced: !self.node.leads(Shard::Layout),
        }
    }

    fn title(&self) -> String {
        let w = self.active.or_else(|| self.node.state.layout.cols.first().and_then(|c| c.wins.first().map(|s| s.window)));
        match w {
            Some(w) => format!("{} — {}", self.node.window_name(w), self.session),
            None => self.session.clone(),
        }
    }

    /// The row's own tag: one line across the top.
    fn top_view(&mut self) -> TextView {
        let r = self.node.state.layout.r;
        let y = r.y0;
        let view = ViewId::Top;
        let width = (self.cols - BOXWID).max(1);
        let tv = self.text_view(view, width, 1, BOXWID, y, None);
        self.layouts.insert(
            view,
            Layout {
                x0: BOXWID,
                y0: y,
                x1: self.cols,
                y1: y + 1,
                scrollbar: None,
                layout_box: Some((0, y, BOXWID, y + 1)),
                wrapped: self.wrapped_of(view, width),
                top: 0,
            },
        );
        tv
    }

    fn wrapped_of(&self, v: ViewId, width: i32) -> Wrapped {
        match self.buffer_of(v).and_then(|b| self.node.state.buffer(b).ok()) {
            Some(b) => Wrapped::of(&b.text, width, self.tabstop(v)),
            None => Wrapped::default(),
        }
    }

    /// Build a text view's lines, from its origin, `height` rows of them.
    fn text_view(&self, v: ViewId, width: i32, height: i32, _x: i32, _y: i32, caret: Option<()>) -> TextView {
        let Some(bid) = self.buffer_of(v) else {
            return TextView { view: view_name(v), lines: vec![Line::plain("")], origin: 0, total: 1, caret: None };
        };
        let Ok(buf) = self.node.state.buffer(bid) else {
            return TextView { view: view_name(v), lines: vec![Line::plain("")], origin: 0, total: 1, caret: None };
        };
        let bv = buf.view(v);
        let wrapped = Wrapped::of(&buf.text, width, self.tabstop(v));
        let top = wrapped.row_of(bv.origin.min(buf.text.len()));
        let mut lines = Vec::new();
        for i in 0..height.max(1) as usize {
            let Some(row) = wrapped.rows.get(top + i) else {
                lines.push(Line::plain(""));
                continue;
            };
            let mut spans = Vec::new();
            push_span(&mut spans, row, bv.q0, bv.q1, SpanKind::Sel);
            if let Some((hv, a, b, kind)) = self.hl {
                if hv == v {
                    push_span(&mut spans, row, a.min(b), a.max(b), kind);
                }
            }
            // a tag's command name, up to the bar
            if matches!(v, ViewId::Tag(_) | ViewId::ColTag(_) | ViewId::Top) && top + i == 0 {
                let name = match v {
                    ViewId::Tag(w) => apex_core::tag_bar(&buf.text.to_string(), &self.node.window_name(w)),
                    _ => None,
                };
                if let Some(n) = name {
                    push_span(&mut spans, row, 0, n, SpanKind::TagName);
                }
            }
            spans.sort_by_key(|s| (s.start, s.len));
            lines.push(Line { text: row.text.clone(), spans });
        }
        // the caret is drawn only while it is on screen: scrolled away,
        // the view has none, as acme's has none
        let caret = caret.and_then(|_| {
            let (r, c) = wrapped.at(bv.q1);
            let end = top.saturating_add(height.max(1) as usize);
            (r >= top && r < end).then(|| (r - top, c))
        });
        TextView { view: view_name(v), lines, origin: top, total: wrapped.len(), caret }
    }

    fn column_view(&mut self, c: ColumnId) -> Option<ColumnView> {
        let col = self.node.state.layout.column(c)?.clone();
        let width = (col.r.dx() - BOXWID).max(1);
        let tag = self.text_view(ViewId::ColTag(c), width, 1, col.r.x0 + BOXWID, col.r.y0, None);
        self.layouts.insert(
            ViewId::ColTag(c),
            Layout {
                x0: col.r.x0 + BOXWID,
                y0: col.r.y0,
                x1: col.r.x1,
                y1: col.r.y0 + 1,
                scrollbar: None,
                layout_box: Some((col.r.x0, col.r.y0, col.r.x0 + BOXWID, col.r.y0 + 1)),
                wrapped: self.wrapped_of(ViewId::ColTag(c), width),
                top: 0,
            },
        );
        let windows = col.wins.iter().filter_map(|s| self.window_view(s.window)).collect();
        Some(ColumnView {
            id: c.0,
            x0: col.r.x0,
            y0: col.r.y0,
            x1: col.r.x1,
            y1: col.r.y1,
            tag,
            windows,
            strip: apex_core::tiling::is_strip(col.r),
        })
    }

    fn window_view(&mut self, w: WindowId) -> Option<WindowView> {
        let (ci, wi) = self.node.state.layout.place_of(w)?;
        let slot = self.node.state.layout.cols.get(ci)?.wins.get(wi)?.clone();
        let win = self.node.state.window(w).ok()?.clone();
        let taglines = slot.taglines.max(1);
        let tagw = (slot.r.dx() - BOXWID).max(1);
        let tag = self.text_view(ViewId::Tag(w), tagw, taglines, slot.r.x0 + BOXWID, slot.r.y0, Some(()));
        self.layouts.insert(
            ViewId::Tag(w),
            Layout {
                x0: slot.r.x0 + BOXWID,
                y0: slot.r.y0,
                x1: slot.r.x1,
                y1: slot.r.y0 + taglines,
                scrollbar: None,
                layout_box: Some((slot.r.x0, slot.r.y0, slot.r.x0 + BOXWID, slot.r.y0 + 1)),
                wrapped: self.wrapped_of(ViewId::Tag(w), tagw),
                top: 0,
            },
        );
        let body = self.body_view(w, &slot);
        let dirty = win.body_buffer().and_then(|b| self.node.state.buffer(b).ok()).map(|b| b.dirty()).unwrap_or(false);
        Some(WindowView {
            id: w.0,
            x0: slot.r.x0,
            y0: slot.r.y0,
            x1: slot.r.x1,
            y1: slot.r.y1,
            tag,
            taglines,
            bx0: slot.body.x0,
            by0: slot.body.y0,
            bx1: slot.body.x1,
            by1: slot.body.y1,
            body,
            kind: kind_of(self.node.window_kind(w)),
            dirty,
            working: self.node.window_working(w),
            notified: self.node.window_notified(w),
            active: self.active == Some(w),
        })
    }

    fn body_view(&mut self, w: WindowId, slot: &apex_core::state::Slot) -> BodyView {
        let r = slot.body;
        let height = r.dy().max(0);
        let width = (r.dx() - SCROLLWID).max(1);
        match self.node.state.window(w).map(|x| x.body) {
            Ok(Body::Term(t)) => {
                self.term_layouts.insert(
                    w,
                    TermLayout {
                        x0: r.x0 + SCROLLWID,
                        y0: r.y0,
                        x1: r.x1,
                        y1: r.y1,
                        scrollbar: (r.x0, r.y0, r.x0 + SCROLLWID, r.y1),
                        cols: width.max(2) as u16,
                        rows: height.max(1) as u16,
                    },
                );
                self.term_body(w, t, width.max(2) as u16, height.max(1) as u16)
            }
            Ok(Body::Web) => BodyView::Web {
                view: view_name(ViewId::Body(w)),
                url: self.node.window_name(w),
                html: String::new(),
                origin: 0,
                loading: true,
            },
            Ok(Body::Html(b)) => {
                let html = self.node.state.buffer(b).map(|b| b.text.to_string()).unwrap_or_default();
                BodyView::Web { view: view_name(ViewId::Body(w)), url: self.node.window_name(w), html, origin: 0, loading: false }
            }
            _ => {
                let v = ViewId::Body(w);
                self.layouts.insert(
                    v,
                    Layout {
                        x0: r.x0 + SCROLLWID,
                        y0: r.y0,
                        x1: r.x1,
                        y1: r.y1,
                        scrollbar: Some((r.x0, r.y0, r.x0 + SCROLLWID, r.y1)),
                        layout_box: None,
                        wrapped: self.wrapped_of(v, width),
                        top: self.origin_row(v, width),
                    },
                );
                // a markdown file goes to the UI's markdown viewer, which
                // is a reader: the text view is what it is edited in
                let name = self.node.window_name(w);
                if is_markdown(&name) && !self.editing(v) {
                    let source = self.text_of(v).map(|t| t.to_string()).unwrap_or_default();
                    return BodyView::Markdown { view: view_name(v), source, origin: self.origin_row(v, width) };
                }
                BodyView::Text(self.text_view(v, width, height, r.x0 + SCROLLWID, r.y0, Some(())))
            }
        }
    }

    /// A markdown window is read as a page until something is selected in
    /// it: acme keeps one window, so the viewer gives way to the text.
    fn editing(&self, v: ViewId) -> bool {
        self.node.selection(v).map(|(a, b)| a != b).unwrap_or(false) || self.node.seltext == Some(v)
    }

    fn origin_row(&self, v: ViewId, width: i32) -> usize {
        let Some(b) = self.buffer_of(v).and_then(|b| self.node.state.buffer(b).ok()) else { return 0 };
        let origin = b.view(v).origin.min(b.text.len());
        Wrapped::of(&b.text, width, self.tabstop(v)).row_of(origin)
    }

    fn term_body(&self, w: WindowId, t: TermId, cols: u16, height: u16) -> BodyView {
        let Some(term) = self.node.state.terms.get(&t) else {
            return BodyView::Term(TermView { term: t.0, cols, rows: Vec::new(), cursor: None, origin: 0, total: 0, exited: true, sel: None });
        };
        let mut rows = Vec::new();
        for line in term.grid.iter().take(height as usize) {
            let mut text = String::new();
            let mut runs: Vec<(usize, usize, u32, u32, u8)> = Vec::new();
            for (i, cell) in line.iter().take(cols as usize).enumerate() {
                text.push(if cell.ch == '\0' { ' ' } else { cell.ch });
                if cell.fg == 0 && cell.bg == 0 && cell.flags == 0 {
                    continue;
                }
                match runs.last_mut() {
                    Some(last) if last.0 + last.1 == i && last.2 == cell.fg && last.3 == cell.bg && last.4 == cell.flags => last.1 += 1,
                    _ => runs.push((i, 1, cell.fg, cell.bg, cell.flags)),
                }
            }
            while text.ends_with(' ') {
                text.pop();
            }
            rows.push(TermRow { text, runs });
        }
        let sel = self.term_sel.filter(|(sw, a, b)| *sw == w && a != b).map(|(_, a, b)| {
            let top = term.top;
            let to_view = |(c, l): (u16, u64)| (c, l.saturating_sub(top) as u16);
            (to_view(a), to_view(b))
        });
        BodyView::Term(TermView {
            term: t.0,
            cols,
            rows,
            cursor: term.cursor_visible.then_some(term.cursor),
            origin: term.top,
            total: term.top + term.rows as u64,
            exited: term.exit.is_some(),
            sel,
        })
    }

    fn overlay(&self) -> Option<Overlay> {
        match &self.over {
            Over::None => None,
            Over::Finder { all } => Some(Overlay::Finder { candidates: self.candidates(), all: *all }),
            Over::Switcher => Some(Overlay::Switcher {
                sessions: self
                    .sessions
                    .iter()
                    .map(|(id, label)| Candidate { name: label.clone(), where_: id.clone(), kind: WinKind::File, open: *label == self.session })
                    .collect(),
                current: self.session.clone(),
            }),
            Over::Message { title, text } => Some(Overlay::Message { title: title.clone(), text: text.clone() }),
        }
    }

    /// What the finder offers: the open windows in layout order, then the
    /// files closed lately. The UI scores and filters them.
    fn candidates(&self) -> Vec<Candidate> {
        let mut out = Vec::new();
        for c in &self.node.state.layout.cols {
            for s in &c.wins {
                let name = self.node.window_name(s.window);
                out.push(Candidate { name, where_: String::new(), kind: kind_of(self.node.window_kind(s.window)), open: true });
            }
        }
        for name in &self.closed {
            out.push(Candidate { name: name.clone(), where_: String::new(), kind: WinKind::File, open: false });
        }
        out
    }

    // ---- hit testing ---------------------------------------------------------

    fn locate(&self, x: i32, y: i32) -> Option<(Target, Region)> {
        for (w, l) in &self.term_layouts {
            let inside = x >= l.x0 && x < l.x1 && y >= l.y0 && y < l.y1;
            let on_bar = x >= l.scrollbar.0 && x < l.scrollbar.2 && y >= l.scrollbar.1 && y < l.scrollbar.3;
            if !inside && !on_bar {
                continue;
            }
            let t = self.term_of(*w)?;
            if on_bar {
                return Some((Target::Term(*w, t), Region::TermScrollbar));
            }
            let c = ((x - l.x0).max(0) as u16).min(l.cols.saturating_sub(1));
            let r = ((y - l.y0).max(0) as u16).min(l.rows.saturating_sub(1));
            return Some((Target::Term(*w, t), Region::Term(c, r)));
        }
        // tags before bodies: a tag's box is at the window's corner
        let mut best: Option<(ViewId, &Layout)> = None;
        for (v, l) in &self.layouts {
            if !l.contains(x, y) && !Layout::in_rect(l.layout_box, x, y) {
                continue;
            }
            let better = match best {
                None => true,
                Some((bv, _)) => rank(*v) > rank(bv),
            };
            if better {
                best = Some((*v, l));
            }
        }
        let (v, l) = best?;
        if Layout::in_rect(l.layout_box, x, y) {
            return Some((Target::View(v), Region::LayoutBox));
        }
        if Layout::in_rect(l.scrollbar, x, y) {
            return Some((Target::View(v), Region::Scrollbar));
        }
        Some((Target::View(v), Region::Text(l.offset_at(x, y))))
    }

    // ---- events --------------------------------------------------------------

    pub fn event(&mut self, ev: Event) {
        match ev {
            Event::Resize { cols, rows } => {
                self.measure(cols, rows);
                self.resize_terms();
            }
            Event::Mouse { x, y, button, motion, mods, clicks } => self.mouse(x, y, button, motion, mods, clicks),
            Event::Text { text } => self.typed(&text),
            Event::Key { key, mods } => self.key(key, mods),
            Event::Exec { window, text } => {
                let ctx = window.map(|w| ExecCtx::Window(WindowId(w))).unwrap_or(ExecCtx::Top);
                self.execute(ctx, &text);
            }
            Event::Open { name } => {
                self.over = Over::None;
                let col = self.node.activecol.or_else(|| self.node.state.layout.cols.last().map(|c| c.id));
                if let Some(col) = col {
                    self.send(ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name });
                }
            }
            Event::Plumb { window, text } => {
                let ctx = window.map(|w| ExecCtx::Window(WindowId(w))).unwrap_or(ExecCtx::Top);
                self.look(ctx, &text, None, None, false);
            }
            Event::Finder { all } => self.over = Over::Finder { all },
            Event::Switcher => {
                self.over = Over::Switcher;
                self.send(ClientMsg::ListSessions);
            }
            Event::Dismiss => self.over = Over::None,
            // switching session means a new attachment: the UI restarts
            // the view server against it, as `apex attach` would
            Event::Switch { name } => {
                self.over = Over::None;
                eprintln!("apex-tuid: switch to {name} is not wired yet");
            }
            Event::Clipboard { text } => {
                if text != self.node.state.layout.snarf {
                    self.snarf_sent = text.clone();
                    let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text }));
                }
            }
            Event::Quit => self.quit = true,
        }
        self.after();
    }

    /// Tell the server every terminal how big its window now is.
    fn resize_terms(&mut self) {
        let sizes: Vec<(TermId, u16, u16)> = self
            .term_layouts
            .iter()
            .filter_map(|(w, l)| self.term_of(*w).map(|t| (t, l.cols, l.rows)))
            .collect();
        for (t, cols, rows) in sizes {
            let cur = self.node.state.terms.get(&t).map(|x| (x.cols, x.rows));
            if cur != Some((cols, rows)) {
                self.send(ClientMsg::TermResize { term: t, cols, rows });
            }
        }
    }

    fn mouse(&mut self, x: i32, y: i32, button: Button, motion: Motion, mods: Mods, clicks: u32) {
        match motion {
            Motion::Down => self.mouse_down(x, y, button, mods, clicks),
            Motion::Move => self.mouse_move(x, y),
            Motion::Up => self.mouse_up(x, y, button),
        }
    }

    fn mouse_down(&mut self, x: i32, y: i32, button: Button, mods: Mods, clicks: u32) {
        if self.over != Over::None {
            self.over = Over::None; // a click anywhere else dismisses it
            return;
        }
        if matches!(button, Button::WheelUp | Button::WheelDown) {
            let delta = if button == Button::WheelUp { -3 } else { 3 };
            match self.locate(x, y) {
                Some((Target::Term(_, t), _)) => self.send(ClientMsg::TermScroll { term: t, delta, at: None }),
                Some((Target::View(v), _)) => self.scroll_by(v, delta),
                None => {}
            }
            return;
        }
        // a chord: B2 or B3 while B1 sweeps acts on the sweep's window
        if self.chord(button) {
            return;
        }
        let Some((target, region)) = self.locate(x, y) else { return };
        if let Some(w) = target.window() {
            self.active = Some(w);
            let _ = self.node.reveal(&mut self.log, w);
        }
        if let Target::View(ViewId::Tag(w)) = target {
            let _ = self.node.commit_tag(&mut self.log, w);
        }
        // the layout boxes: acme waits for the release to decide
        if let (Target::View(v), Region::LayoutBox) = (target, region) {
            if self.mouse.b1.is_none() {
                let bt = match v {
                    ViewId::Tag(w) => Some(BoxTarget::Win(w)),
                    ViewId::ColTag(c) => Some(BoxTarget::Col(c)),
                    _ => None,
                };
                if let Some(bt) = bt {
                    self.mouse.box_drag = Some((bt, button, (x, y)));
                    return;
                }
            }
        }
        match (target, button) {
            (Target::View(_), Button::B1) if self.mouse.b2.is_some() => self.mouse.chord_arg = true,
            (Target::View(v), Button::B1) => match region {
                Region::Text(off) => {
                    self.node.activecol = self.node.state.layout.column_of(v.window().unwrap_or(WindowId(0)));
                    if clicks >= 2 {
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
                Region::Scrollbar => self.scroll_to(v, y),
                _ => {}
            },
            (Target::View(v), Button::B2) => match region {
                Region::Text(off) => {
                    self.mouse.b2 = Some(Drag { view: v, anchor: off });
                    self.hl = None;
                }
                Region::Scrollbar => self.scroll_to(v, y),
                _ => {}
            },
            (Target::View(v), Button::B3) => match region {
                Region::Text(off) => {
                    self.mouse.b3 = Some(Drag { view: v, anchor: off });
                    self.mouse.b3_reverse = mods.shift;
                    self.hl = None;
                }
                Region::Scrollbar => self.scroll_to(v, y),
                _ => {}
            },
            (Target::Term(w, t), b) => match (region, b) {
                (Region::Term(c, r), Button::B1) => {
                    let p = (c, self.term_top(w) + r as u64);
                    self.term_sel = Some((w, p, p));
                    self.mouse.term_drag = Some(w);
                }
                (Region::Term(c, r), Button::B2 | Button::B3) => {
                    let p = (c, self.term_top(w) + r as u64);
                    self.mouse.term_sweep = Some((w, b, (c, r)));
                    self.term_sel = Some((w, p, p));
                }
                (Region::TermScrollbar, _) => {
                    let l = self.term_layouts.get(&w).cloned();
                    if let Some(l) = l {
                        let frac = (y - l.y0).max(0) as f64 / (l.y1 - l.y0).max(1) as f64;
                        let delta = ((frac - 0.5) * l.rows as f64) as isize;
                        self.send(ClientMsg::TermScroll { term: t, delta: delta as i64, at: None });
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// B2 or B3 while B1 sweeps: cut and paste, as plan 9's chords do.
    fn chord(&mut self, button: Button) -> bool {
        let Some(d) = self.mouse.b1 else { return false };
        match button {
            Button::B2 => {
                self.mouse.chorded = true;
                let _ = self.node.cut(&mut self.log, d.view);
                true
            }
            Button::B3 => {
                self.mouse.chorded = true;
                let _ = self.node.paste(&mut self.log, d.view);
                true
            }
            _ => false,
        }
    }

    fn mouse_move(&mut self, x: i32, y: i32) {
        if let Some(d) = self.mouse.b1 {
            if let Some(l) = self.layouts.get(&d.view) {
                let off = l.offset_at(x, y);
                let _ = self.node.select(&mut self.log, d.view, d.anchor.min(off), d.anchor.max(off));
            }
            return;
        }
        for (b, kind) in [(self.mouse.b2, SpanKind::Exec), (self.mouse.b3, SpanKind::Look)] {
            if let Some(d) = b {
                if let Some(l) = self.layouts.get(&d.view) {
                    let off = l.offset_at(x, y);
                    self.hl = Some((d.view, d.anchor, off, kind));
                }
                return;
            }
        }
        if let Some((w, a, _)) = self.term_sel {
            if self.mouse.term_drag == Some(w) || self.mouse.term_sweep.map(|(sw, _, _)| sw) == Some(w) {
                if let Some(l) = self.term_layouts.get(&w) {
                    let c = ((x - l.x0).max(0) as u16).min(l.cols.saturating_sub(1));
                    let r = ((y - l.y0).max(0) as u16).min(l.rows.saturating_sub(1));
                    let p = (c, self.term_top(w) + r as u64);
                    self.term_sel = Some((w, a, p));
                }
                return;
            }
        }
        // no button down: the window under the pointer is the active one
        if let Some((t, _)) = self.locate(x, y) {
            if let Some(w) = t.window() {
                if self.active != Some(w) {
                    if let Some(old) = self.active.and_then(|w| self.term_of(w)) {
                        self.send(ClientMsg::TermFocus { term: old, focused: false });
                    }
                    self.active = Some(w);
                    if let Some(t) = self.term_of(w) {
                        self.send(ClientMsg::TermFocus { term: t, focused: true });
                    }
                }
            }
        }
    }

    fn mouse_up(&mut self, x: i32, y: i32, button: Button) {
        if let Some((bt, b, start)) = self.mouse.box_drag {
            if b == button {
                self.mouse.box_drag = None;
                let but = match button {
                    Button::B1 => 1,
                    Button::B2 => 2,
                    _ => 3,
                };
                let r = match bt {
                    BoxTarget::Win(w) => self.node.drag_window(&mut self.log, w, but, start, (x, y)),
                    BoxTarget::Col(c) => self.node.drag_column(&mut self.log, c, but, start, (x, y)),
                };
                if let Err(e) = r {
                    eprintln!("apex-tuid: layout: {e}");
                }
                return;
            }
        }
        if let Some((w, b, cell)) = self.mouse.term_sweep {
            if b == button {
                self.mouse.term_sweep = None;
                // the server has the scrollback: it snarfs, then we act
                if let Some((_, a, z)) = self.term_sel.filter(|(sw, a, z)| *sw == w && a != z) {
                    self.send(ClientMsg::TermText { term: self.term_of(w).unwrap_or(TermId(0)), p0: a.min(z), p1: a.max(z) });
                } else if let Some(text) = self.term_word(w, cell) {
                    match button {
                        Button::B2 => self.execute(ExecCtx::Window(w), &text),
                        _ => self.look(ExecCtx::Window(w), &text, None, None, false),
                    }
                }
                return;
            }
        }
        match button {
            Button::B1 => {
                self.mouse.b1 = None;
                self.mouse.term_drag = None;
            }
            Button::B2 => {
                if let Some(d) = self.mouse.b2.take() {
                    let text = self.take_range(d);
                    self.hl = None;
                    let arg = if self.mouse.chord_arg { self.node.seltext.and_then(|v| self.node.selected_text(v).ok()) } else { None };
                    self.mouse.chord_arg = false;
                    if let Some(mut text) = text {
                        if let Some(a) = arg.filter(|a| !a.is_empty()) {
                            text.push(' ');
                            text.push_str(&a);
                        }
                        self.execute(self.ctx_of(d.view), &text);
                    }
                }
            }
            Button::B3 => {
                if let Some(d) = self.mouse.b3.take() {
                    let found = self.take_range_at(d);
                    self.hl = None;
                    if let Some((text, (lo, hi))) = found {
                        let b = self.buffer_of(d.view);
                        let at = b.map(|b| Span { buffer: b, q0: d.anchor, q1: d.anchor });
                        let sel = b.map(|b| Span { buffer: b, q0: lo, q1: hi });
                        let reverse = self.mouse.b3_reverse;
                        let ctx = self.ctx_of(d.view);
                        self.look(ctx, &text, at, sel, reverse);
                    }
                }
            }
            _ => {}
        }
    }

    /// What B2 swept, or the word it clicked on (acme's textselect23).
    fn take_range(&self, d: Drag) -> Option<String> {
        let (_, a, b, _) = self.hl?;
        let t = self.text_of(d.view)?;
        if a == b {
            let (lo, hi) = double_click(&t, a);
            return Some(t.slice(lo, hi));
        }
        Some(t.slice(a.min(b), a.max(b)))
    }

    /// The same for B3, with the offsets, which the plumber wants.
    fn take_range_at(&self, d: Drag) -> Option<(String, (usize, usize))> {
        let t = self.text_of(d.view)?;
        let (lo, hi) = match self.hl {
            Some((_, a, b, _)) if a != b => (a.min(b), a.max(b)),
            _ => {
                // in the selection: the selection; else the word
                let (q0, q1) = self.node.selection(d.view).ok()?;
                if q0 != q1 && d.anchor >= q0 && d.anchor <= q1 {
                    (q0, q1)
                } else {
                    double_click(&t, d.anchor)
                }
            }
        };
        Some((t.slice(lo, hi), (lo, hi)))
    }

    /// The word under a terminal cell, from the grid the client has.
    fn term_word(&self, w: WindowId, (c, r): (u16, u16)) -> Option<String> {
        let t = self.term_of(w)?;
        let term = self.node.state.terms.get(&t)?;
        let row = term.grid.get(r as usize)?;
        let ok = |ch: char| !ch.is_whitespace() && ch != '\0';
        let mut lo = c as usize;
        let mut hi = c as usize;
        if !row.get(lo).map(|x| ok(x.ch)).unwrap_or(false) {
            return None;
        }
        while lo > 0 && row.get(lo - 1).map(|x| ok(x.ch)).unwrap_or(false) {
            lo -= 1;
        }
        while hi + 1 < row.len() && row.get(hi + 1).map(|x| ok(x.ch)).unwrap_or(false) {
            hi += 1;
        }
        Some(row[lo..=hi].iter().map(|x| x.ch).collect())
    }

    fn scroll_by(&mut self, v: ViewId, delta: i64) {
        let Some(b) = self.buffer_of(v).and_then(|b| self.node.state.buffer(b).ok()) else { return };
        let text = b.text.clone();
        let origin = b.view(v).origin;
        let line = text.line_of(origin.min(text.len())) as i64;
        let want = (line + delta).clamp(0, text.line_count().saturating_sub(1) as i64) as usize;
        let q = text.line_start(want);
        let _ = self.node.set_origin(&mut self.log, v, q);
    }

    /// A click on the scrollbar: acme's B1 scrolls back by the fraction,
    /// here simplified to jumping to it.
    fn scroll_to(&mut self, v: ViewId, y: i32) {
        let Some(l) = self.layouts.get(&v) else { return };
        let (y0, y1) = (l.y0, l.y1);
        let Some(b) = self.buffer_of(v).and_then(|b| self.node.state.buffer(b).ok()) else { return };
        let text = b.text.clone();
        let frac = (y - y0).max(0) as f64 / (y1 - y0).max(1) as f64;
        let line = (frac * text.line_count() as f64) as usize;
        let q = text.line_start(line.min(text.line_count().saturating_sub(1)));
        let _ = self.node.set_origin(&mut self.log, v, q);
    }

    // ---- the keyboard --------------------------------------------------------

    fn focused(&self) -> Option<ViewId> {
        let w = self.active?;
        // the tag when the caret is in it, else the body
        let tag = ViewId::Tag(w);
        if self.node.seltext == Some(tag) {
            return Some(tag);
        }
        Some(ViewId::Body(w))
    }

    fn typed(&mut self, text: &str) {
        let Some(v) = self.focused() else { return };
        if let Some(t) = self.body_term(v) {
            self.send(ClientMsg::TermType { term: t, text: text.to_string() });
            return;
        }
        let _ = self.node.insert(&mut self.log, v, text);
    }

    fn body_term(&self, v: ViewId) -> Option<TermId> {
        match v {
            ViewId::Body(w) => self.term_of(w),
            _ => None,
        }
    }

    fn key(&mut self, key: NamedKey, mods: Mods) {
        let Some(v) = self.focused() else { return };
        if let Some(t) = self.body_term(v) {
            if let Some(k) = term_key(key, mods) {
                self.send(ClientMsg::TermKey { term: t, key: k });
            }
            return;
        }
        match key {
            NamedKey::Enter => {
                let _ = self.node.insert(&mut self.log, v, "\n");
            }
            NamedKey::Tab => {
                let _ = self.node.insert(&mut self.log, v, "\t");
            }
            NamedKey::Backspace => {
                let _ = self.node.backspace(&mut self.log, v);
            }
            NamedKey::Delete => {
                let _ = self.node.delete_forward(&mut self.log, v);
            }
            NamedKey::EraseWord => {
                let _ = self.node.erase(&mut self.log, v, Erase::Word);
            }
            NamedKey::EraseLine => {
                let _ = self.node.erase(&mut self.log, v, Erase::Line);
            }
            NamedKey::Escape => {
                self.over = Over::None;
                self.node.end_typing();
            }
            NamedKey::Up | NamedKey::Down | NamedKey::Left | NamedKey::Right | NamedKey::Home | NamedKey::End => self.move_caret(v, key, mods),
            NamedKey::PageUp => self.scroll_by(v, -(self.view_height(v) as i64 - 2).max(1)),
            NamedKey::PageDown => self.scroll_by(v, (self.view_height(v) as i64 - 2).max(1)),
        }
    }

    fn view_height(&self, v: ViewId) -> i32 {
        self.layouts.get(&v).map(|l| (l.y1 - l.y0).max(1)).unwrap_or(1)
    }

    fn move_caret(&mut self, v: ViewId, key: NamedKey, mods: Mods) {
        let Some(t) = self.text_of(v) else { return };
        let Ok((q0, q1)) = self.node.selection(v) else { return };
        let here = if q0 == q1 { q0 } else { q1 };
        let width = self.layouts.get(&v).map(|l| (l.x1 - l.x0).max(1)).unwrap_or(80);
        let wrapped = Wrapped::of(&t, width, self.tabstop(v));
        let (row, col) = wrapped.at(here);
        let to = match key {
            NamedKey::Left => here.saturating_sub(1),
            NamedKey::Right => (here + 1).min(t.len()),
            NamedKey::Up => wrapped.offset(row.saturating_sub(1), col),
            NamedKey::Down => wrapped.offset((row + 1).min(wrapped.len().saturating_sub(1)), col),
            NamedKey::Home => wrapped.rows.get(row).map(|r| r.q0).unwrap_or(here),
            NamedKey::End => wrapped.rows.get(row).map(|r| r.q1.saturating_sub(1).max(r.q0)).unwrap_or(here),
            _ => here,
        };
        self.node.end_typing();
        if mods.shift {
            let anchor = if q0 == q1 { here } else { q0 };
            let _ = self.node.select(&mut self.log, v, anchor.min(to), anchor.max(to));
        } else {
            let _ = self.node.select(&mut self.log, v, to, to);
        }
    }

    // ---- executing and looking -----------------------------------------------

    pub fn execute(&mut self, ctx: ExecCtx, text: &str) {
        if let ExecCtx::Window(w) = ctx {
            let _ = self.node.commit_tag(&mut self.log, w);
        }
        let word = text.trim().split_whitespace().next().unwrap_or("").to_string();
        // Send in a terminal: the selection, typed in with a newline
        if word == "Send" {
            if let ExecCtx::Window(w) = ctx {
                if let (Some(t), Some((sw, a, b))) = (self.term_of(w), self.term_sel) {
                    if sw == w && a != b {
                        self.send(ClientMsg::TermText { term: t, p0: a.min(b), p1: a.max(b) });
                        return;
                    }
                }
            }
        }
        match self.node.exec(&mut self.log, ctx, text) {
            Ok(Executed::Quit(_)) => self.quit = true,
            Ok(Executed::Failed(_, why)) => self.report(ctx, &why),
            Ok(_) => {}
            Err(e) => self.report(ctx, &e.to_string()),
        }
    }

    fn report(&mut self, ctx: ExecCtx, why: &str) {
        let dir = match ctx {
            ExecCtx::Window(w) => self.node.error_dir(Some(w)),
            _ => self.node.error_dir(None),
        };
        let _ = self.node.errors(&mut self.log, dir.as_deref(), &format!("{why}\n"));
    }

    fn look(&mut self, ctx: ExecCtx, text: &str, at: Option<Span>, sel: Option<Span>, reverse: bool) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.send(ClientMsg::Plumb {
            ctx,
            text: text.to_string(),
            dir: None,
            edit_only: false,
            dry: false,
            at,
            sel,
            alt: None,
            reverse,
            verb: None,
        });
    }
}

fn rank(v: ViewId) -> u8 {
    match v {
        ViewId::Top => 3,
        ViewId::ColTag(_) => 2,
        ViewId::Tag(_) => 1,
        ViewId::Body(_) => 0,
    }
}

fn view_name(v: ViewId) -> String {
    match v {
        ViewId::Top => "top".into(),
        ViewId::ColTag(c) => format!("col:{}", c.0),
        ViewId::Tag(w) => format!("tag:{}", w.0),
        ViewId::Body(w) => format!("body:{}", w.0),
    }
}

fn kind_of(k: apex_core::WinKind) -> WinKind {
    match k {
        apex_core::WinKind::File => WinKind::File,
        apex_core::WinKind::Dir => WinKind::Dir,
        apex_core::WinKind::Term => WinKind::Term,
        apex_core::WinKind::Errors => WinKind::Errors,
        apex_core::WinKind::Web => WinKind::Web,
    }
}

fn is_markdown(name: &str) -> bool {
    let n = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
    n.ends_with(".md") || n.ends_with(".markdown")
}

/// Add a span for `[q0, q1)` where it meets this row.
fn push_span(spans: &mut Vec<Span2>, row: &crate::cells::Row, q0: usize, q1: usize, kind: SpanKind) {
    if q1 <= row.q0 || q0 >= row.q1 {
        // an empty selection at the very end of the row still shows
        if !(q0 == q1 && q0 >= row.q0 && q0 <= row.q1) {
            return;
        }
    }
    let lo = q0.max(row.q0);
    let hi = q1.min(row.q1);
    if hi < lo {
        return;
    }
    let c0 = row.column_of(lo);
    let c1 = row.column_of(hi);
    if c1 <= c0 && !(q0 == q1) {
        return;
    }
    spans.push(Span2 { start: c0, len: (c1 - c0).max(if q0 == q1 { 0 } else { 1 }), kind });
}

/// The model's span, under a name that does not clash with the core's.
use crate::model::Span as Span2;

/// A named key, as the terminal wants it: the server encodes it, since
/// the encoding depends on modes only it knows.
fn term_key(key: NamedKey, mods: Mods) -> Option<TermKey> {
    let name = match key {
        NamedKey::Enter => "enter",
        NamedKey::Tab => "tab",
        NamedKey::Backspace => "backspace",
        NamedKey::Delete => "delete",
        NamedKey::Escape => "escape",
        NamedKey::Up => "up",
        NamedKey::Down => "down",
        NamedKey::Left => "left",
        NamedKey::Right => "right",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::PageUp => "pageup",
        NamedKey::PageDown => "pagedown",
        // ^W and ^U are control keys, not named ones
        NamedKey::EraseWord => return Some(TermKey { key: "w".into(), text: None, shift: false, control: true, alt: false }),
        NamedKey::EraseLine => return Some(TermKey { key: "u".into(), text: None, shift: false, control: true, alt: false }),
    };
    Some(TermKey { key: name.into(), text: None, shift: mods.shift, control: mods.ctrl, alt: mods.alt })
}
