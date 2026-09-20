//! The view model: what the TermKit UI draws, in character cells.
//!
//! The UI is a renderer and an input device (DESIGN.md §1), so the whole
//! of what it shows is one value, serialised as JSON and handed over on
//! every change. Nothing here names a widget: the Swift side decides how
//! a tag or a body is drawn, this says only what is in one.

use serde::{Deserialize, Serialize};

/// A run of a line that is drawn differently from the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanKind {
    /// The view's dot (acme's selection).
    Sel,
    /// B2's or B3's sweep, while the button is down.
    Exec,
    Look,
    /// The command name in a tag, before the bar.
    TagName,
    /// Where a search or an Edit address landed.
    Mark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub len: usize,
    pub kind: SpanKind,
}

/// One drawn line: already wrapped and tab-expanded to the view's width,
/// so the UI never has to know a rune offset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Line {
        Line { text: text.into(), spans: Vec::new() }
    }
}

/// A text view (a tag, a column tag, the top row, or a text body).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextView {
    /// The view this belongs to, as the UI quotes it back in an event.
    pub view: String,
    pub lines: Vec<Line>,
    /// The first line of the buffer that `lines[0]` is: the scrollbar's
    /// thumb, and what a click on the bar means.
    pub origin: usize,
    /// Wrapped lines in the whole buffer.
    pub total: usize,
    /// Where the caret is, when this view has the keyboard: (row, col)
    /// within `lines`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caret: Option<(usize, usize)>,
}

/// One terminal cell, packed the way the core's grid has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TermRow {
    pub text: String,
    /// `(start, len, fg, bg, flags)` for every run that is not the
    /// terminal's own ink on its own paper. Colours are `0xRRGGBB`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<(usize, usize, u32, u32, u8)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TermView {
    pub term: u64,
    pub cols: u16,
    pub rows: Vec<TermRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<(u16, u16)>,
    /// The scrollback: the first shown line, and how many there are.
    pub origin: u64,
    pub total: u64,
    pub exited: bool,
    /// What B2 or B3 has swept, as (row, col) pairs within `rows`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sel: Option<((u16, u16), (u16, u16))>,
}

/// What fills a window below its tag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BodyView {
    /// Plain text: the editor's own drawing.
    Text(TextView),
    /// A terminal's grid.
    Term(TermView),
    /// Markdown, for the UI's markdown viewer. `source` is the whole
    /// buffer; the viewer scrolls it itself.
    Markdown { view: String, source: String, origin: usize },
    /// A page, for the UI's browser: the HTML as the session has it (a
    /// `Body::Html` buffer) or as fetched for a `Body::Web` window.
    Web { view: String, url: String, html: String, origin: usize, loading: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WinKind {
    File,
    Dir,
    Term,
    Errors,
    Web,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowView {
    pub id: u64,
    /// The window's whole rectangle, in cells, within the row.
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// The tag: one or more lines, the first carrying the layout box.
    pub tag: TextView,
    pub taglines: i32,
    /// The body's own rectangle. It is not the window less its tag: the
    /// tiling leaves a border between the two (acme's, in cells), and a
    /// click in that border is in neither.
    pub bx0: i32,
    pub by0: i32,
    pub bx1: i32,
    pub by1: i32,
    pub body: BodyView,
    pub kind: WinKind,
    pub dirty: bool,
    pub working: bool,
    pub notified: bool,
    /// The window under the pointer, or the one that last had it.
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnView {
    pub id: u64,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub tag: TextView,
    pub windows: Vec<WindowView>,
    /// A column squeezed to its strip: the UI draws its tag sideways.
    pub strip: bool,
}

/// A row of the fuzzy finder, or of the session switcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub name: String,
    /// Where it is, shown dimmed: a directory, or a session's label.
    #[serde(default)]
    pub where_: String,
    pub kind: WinKind,
    /// Open in the session already.
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Overlay {
    /// ⌘P: the files of the session and the ones closed lately. The UI
    /// scores and filters; the list changes only when the session does.
    Finder { candidates: Vec<Candidate>, all: bool },
    /// The sessions this daemon has.
    Switcher { sessions: Vec<Candidate>, current: String },
    /// A message the UI puts up until it is dismissed.
    Message { title: String, text: String },
}

/// The whole of what the UI draws.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub seq: u64,
    pub cols: i32,
    pub rows: i32,
    pub title: String,
    /// The row's own tag: acme's top line, with the session's square.
    pub top: TextView,
    pub columns: Vec<ColumnView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<Overlay>,
    /// The oldest notification, shown in the square.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<String>,
    /// Where acme would move the pointer to (`Warp`), in cells.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warp: Option<(i32, i32)>,
    /// The snarf buffer, so the UI can put it on the system clipboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snarf: Option<String>,
    /// The link to the session is up.
    pub connected: bool,
    /// This attachment lost its leases to another UI.
    pub fenced: bool,
}
