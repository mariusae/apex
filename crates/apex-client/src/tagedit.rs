//! A window tag's head at work. Its path double-clicked becomes a field
//! (return, or going on to anything else, moves the window there, as
//! `apex win rename` does; escape leaves it where it was; a new
//! window's Untitled is one click); its label double-clicked, the same.
//! The field holds the path as the tag shows it (from the session's
//! directory on, when it is in it), runs along the tag's row as far as
//! the window's edge and no further, scrolling to keep its caret in
//! sight, and B1 in it clicks, sweeps and double-clicks as in text.
//! A folder of the path, or its name, clicked brings a picker down under
//! it, as VS Code's breadcrumbs do: the folder's entries (the name's
//! siblings), listed by the host as ^F's names are, so a remote one's
//! too, and narrowed as a query is typed in the tag itself (the path
//! shows the folder, then what is typed, with its caret: `Head::picking`),
//! the names below in the tag's face, under it; first, the windows open on the
//! folder or on a file in it that are not its own (its errors, a file's
//! preview, a terminal or a tool's pane there), to go to. Return (or a
//! click) opens the
//! one chosen, a file or a folder, in a window of its own; ⌥return in
//! this window in place of what it shows, a file for a file's window and
//! a folder for a folder's (a second time when this one is unsaved).
//! → or tab (or the › on its row) goes into a folder; ← or backspace
//! with nothing typed goes up. B3 on a folder or the name
//! plumbs the path to there; B1 or B2 on a verb runs it.

use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Bounds, Context, MouseButton, Pixels, Point, Window};

use apex_core::{ExecCtx, Loc, Pos, ViewId, WinKind, WindowId};
use apex_server::Proposal;

use crate::app::Acme;
use crate::field::{Edited, LineEdit};
use crate::text_element::Atom;

/// `Candidates::at` for a listing the picker asked for (a completion's is
/// a caret's offset).
pub const LISTING: usize = usize::MAX;

/// How soon a second click on the tag whose picker a click just brought
/// down is the double-click that makes its path a field.
pub const DOUBLE: Duration = Duration::from_millis(500);

/// The picker's rows at once.
const ROWS: usize = 12;

/// The fewest rows it shows, however little room there is under the path.
const MIN_ROWS: usize = 3;

/// The picker's parts' heights, fixed, so how tall it can be is known
/// when it comes down: as many rows as fit under the path. A row is a
/// tag's line, in the tag's face: its names line up with the path's.
fn row_h() -> f32 {
    f32::from(crate::text_element::tag_line_height()).max(20.)
}
use crate::shell::{FOOT_H, HAIRLINE, NOTE_H, POPOVER_PAD};
/// The picker less its rows: hairlines and padding, the line between
/// windows and entries, the "more", the foot.
const PICKER_REST: f32 = 2. * HAIRLINE + 2. * POPOVER_PAD + (HAIRLINE + 8.) + NOTE_H + FOOT_H;
const PICKER_W: f32 = 420.;
/// The narrowest it goes to keep its names under the path: past this it
/// moves left instead.
const PICKER_MIN_W: f32 = 220.;
/// How far in from the picker's edge a row's name starts: its border,
/// its padding, the row's.
const TEXT_IN: f32 = crate::shell::POPOVER_TEXT_IN;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Path,
    Label,
}

/// The path or the label being typed.
pub struct TagEdit {
    pub window: WindowId,
    pub part: Part,
    pub field: LineEdit,
    pub caret_since: Instant,
    /// Where what it edits is drawn.
    pub at: Bounds<Pixels>,
    /// The right edge of the tag's row: the field goes no further.
    pub right: Pixels,
    /// The folder the path is shown from (the session's directory, with
    /// its slash), when it is shown relative to it: a relative path
    /// typed is in it.
    pub base: Option<String>,
    /// How far the text is scrolled left, to keep the caret in sight
    /// (kept as it is drawn: `tag_overlays`).
    pub scroll: Cell<f32>,
    /// B1 pressed in the field and still held: B2 cuts its selection,
    /// B3 pastes over it, acme's chords.
    pub b1: bool,
    /// Where that press was, in characters: a sweep runs from there.
    pub press: Option<usize>,
    /// The press was inside the selection: it becomes a click there when
    /// B1 comes up without a sweep or a chord, so a chord there still
    /// cuts or pastes over the selection.
    pub collapse: bool,
}

/// A row of the picker: a window open on the folder or a file in it
/// (its id, what it is called, its kind), or one of the folder's entries
/// (a folder's with `true`).
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Window(WindowId, String, &'static str),
    Entry(String, bool),
}

pub struct Picker {
    pub window: WindowId,
    /// The windows open on the folder or a file in it, not its own.
    pub windows: Vec<(WindowId, String, &'static str)>,
    /// The folder listed, with its slash.
    pub dir: String,
    pub filter: LineEdit,
    /// The folder's entries (a folder's with `true`), once the host said.
    pub names: Option<Result<Vec<(String, bool)>, String>>,
    pub cursor: usize,
    /// Where its top left is: right under the part clicked, as VS Code's
    /// breadcrumbs' are (left of it only as far as the window's edge
    /// makes it) -- decided once, so it does not move as the list grows
    /// and shrinks.
    pub at: Point<Pixels>,
    /// The rows it shows at once: as many as fit under the path.
    pub rows: usize,
    /// Where what is typed starts in the tag, as the last frame laid it
    /// out (`picker_anchor`): the names go under it.
    pub typed_x: Option<f32>,
    pub caret_since: Instant,
    /// The entry to start on: the one the path goes through.
    pub current: Option<String>,
    /// ⌥return asked once of an unsaved window: again replaces it.
    pub warned: bool,
}

pub(crate) fn caret_on(since: Instant) -> bool {
    (since.elapsed().as_millis() / 530) % 2 == 0
}

impl Picker {
    /// The rows for what is typed: the windows first, then the entries --
    /// all of them, folders first, with nothing typed; else the ones
    /// matching, best first.
    pub fn picks(&self) -> Vec<Choice> {
        let q = self.filter.trim();
        let best = |a: &(f64, Choice), b: &(f64, Choice)| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal);
        let mut out: Vec<Choice> = if q.is_empty() {
            self.windows.iter().map(|(w, t, k)| Choice::Window(*w, t.clone(), k)).collect()
        } else {
            let mut v: Vec<(f64, Choice)> = self.windows.iter().filter_map(|(w, t, k)| crate::finder::score(q, t).map(|s| (s, Choice::Window(*w, t.clone(), k)))).collect();
            v.sort_by(best);
            v.into_iter().map(|x| x.1).collect()
        };
        let Some(Ok(names)) = &self.names else { return out };
        if q.is_empty() {
            let mut v = names.clone();
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            out.extend(v.into_iter().map(|(n, d)| Choice::Entry(n, d)));
        } else {
            let mut v: Vec<(f64, Choice)> = names.iter().filter_map(|n| crate::finder::score(q, &n.0).map(|s| (s, Choice::Entry(n.0.clone(), n.1)))).collect();
            v.sort_by(best);
            out.extend(v.into_iter().map(|x| x.1));
        }
        out
    }
}

/// The folder a part of the path names, with its slash, and the entry in
/// it the path goes through: a folder's own child, a file's name among
/// its siblings -- and a directory's name, the directory itself (its
/// entries listed, as the title bar's crumbs list theirs).
pub fn folder_of(path: &str, atom: Atom) -> Option<(String, Option<String>)> {
    let name_at = path.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
    match atom {
        Atom::Dir(k) => {
            let rest = &path[k..];
            let next = rest.split('/').next().filter(|s| !s.is_empty()).map(String::from);
            Some((path[..k].to_string(), next))
        }
        Atom::Name if path.ends_with('/') => Some((path.to_string(), None)),
        Atom::Name if name_at > 0 => Some((path[..name_at].to_string(), Some(path[name_at..].trim_end_matches('/').to_string()))),
        _ => None,
    }
}

impl Acme {
    /// A press on a part of a tag's head.
    pub fn press_atom(&mut self, w: WindowId, atom: Atom, button: MouseButton, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        match (button, atom) {
            // the verbs run when the button comes up on them
            (MouseButton::Left | MouseButton::Middle, Atom::Verb(_) | Atom::Word(_)) => self.atom_down = Some((w, atom, button)),
            (MouseButton::Left, Atom::Dir(_) | Atom::Name | Atom::Untitled) if clicks >= 2 => {
                self.picker_due = None;
                self.picker = None;
                self.tag_edit_start(w, Part::Path, cx);
            }
            (MouseButton::Left, Atom::Untitled) => self.tag_edit_start(w, Part::Path, cx),
            (MouseButton::Left, Atom::Label) if clicks >= 2 => self.tag_edit_start(w, Part::Label, cx),
            // the picker, unless this is the first of a double-click
            // the picker at once; a second click, a double-click, takes it
            // away for the field (`mouse_down`, `picker_opened`)
            // (and B2 while it is held, the 1-2 chord, snarfs the path to
            // there: `snarf_head`; B3, the 1-3 chord, renames the window
            // to the snarf buffer: `paste_head`)
            (MouseButton::Left, Atom::Dir(_) | Atom::Name) => {
                self.open_path_picker(w, atom, cx);
                self.picker_opened = Some((w, Instant::now()));
                self.head_b1 = Some((w, atom));
            }
            // B2 on the path: the name runs when the button comes up on
            // it (`release_head`)
            (MouseButton::Middle, Atom::Dir(_) | Atom::Name) => self.head_b2 = Some((w, atom)),
            // B3 on the path: plumbed, as far as the part
            (MouseButton::Right, Atom::Dir(k)) => {
                let path = self.node.window_path(w);
                self.look(ExecCtx::Window(w), &path[..k.min(path.len())]);
            }
            (MouseButton::Right, Atom::Name) => {
                let path = self.node.window_path(w);
                self.look(ExecCtx::Window(w), &path);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The 1-2 chord on a part of a tag's path, as acme's on the name
    /// at the start of its tag (swept with B1, cut with B2) would snarf
    /// it: the path to there -- the whole of it from the name, a
    /// folder's with its slash -- in the snarf buffer and on the
    /// clipboard, and the path left as it is (nothing is cut from a
    /// window's name). The picker the B1 press brought down goes.
    pub fn snarf_head(&mut self, w: WindowId, atom: Atom, cx: &mut Context<Self>) {
        self.picker = None;
        self.picker_opened = None;
        let path = self.node.window_path(w);
        let text = match atom {
            Atom::Dir(k) => path[..k.min(path.len())].to_string(),
            _ => path,
        };
        if text.is_empty() {
            return;
        }
        let _ = self.node.append(&mut self.log, apex_core::Shard::Layout, apex_core::Op::Layout(apex_core::LayoutOp::Snarf { text: text.clone() }));
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    /// The 1-3 chord on a part of a tag's path, as acme's over the name
    /// at the start of its tag (swept with B1, pasted over with B3)
    /// would rename the window: its name the snarf buffer's, all of it
    /// (there is no sweep over part of the path), resolved as a name
    /// typed in the field is and taken as the field's is on return. A
    /// snarf of more than a line, or of nothing, is not a name: nothing
    /// happens. The picker the B1 press brought down goes, and nothing
    /// is plumbed.
    pub fn paste_head(&mut self, w: WindowId, cx: &mut Context<Self>) {
        self.picker = None;
        self.picker_opened = None;
        let snarf = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_else(|| self.node.state.layout.snarf.clone());
        let name = snarf.trim();
        if name.contains('\n') || self.node.state.window(w).is_err() {
            return;
        }
        if let Some(p) = self.rename_to(w, name, None) {
            apex_server::perform(&mut self.node, &mut self.log, vec![p]);
            self.after();
        }
    }

    /// The button come up where it went down on a verb: run it.
    pub fn release_atom(&mut self, pos: Point<Pixels>, button: MouseButton, cx: &mut Context<Self>) -> bool {
        let Some((w, atom, b)) = self.atom_down.take() else { return false };
        if b != button {
            return false;
        }
        let here = self.layouts.get(&ViewId::Tag(w)).and_then(|l| l.atom_at(pos));
        if here == Some(atom) {
            match atom {
                Atom::Verb(i) => {
                    self.execute(ExecCtx::Window(w), crate::text_element::verb_command(i), cx);
                    self.after();
                }
                Atom::Word(i) => {
                    if let Some(word) = self.tag_head(w).words.get(i).cloned() {
                        self.execute(ExecCtx::Window(w), &word, cx);
                        self.after();
                    }
                }
                _ => {}
            }
        }
        cx.notify();
        true
    }

    pub fn tag_edit_start(&mut self, w: WindowId, part: Part, cx: &mut Context<Self>) {
        let layout = self.layouts.get(&ViewId::Tag(w));
        let at = match part {
            Part::Path => layout.and_then(|l| l.path_bounds()),
            Part::Label => layout.and_then(|l| l.atom_bounds(Atom::Label)),
        };
        let Some(at) = at.or_else(|| layout.map(|l| Bounds::new(l.text_origin, gpui::size(px(200.), l.line_height)))) else { return };
        let right = layout.map_or(at.right() + px(420.), |l| l.bounds.right());
        let mut field = LineEdit::new();
        let mut base = None;
        match part {
            Part::Path => {
                let full = self.node.window_path(w);
                // as the tag shows it: from the session's directory on
                // when it is in it (`Head::build_in`), so what was
                // clicked stays put
                let cwd = &self.node.state.meta.cwd;
                if !self.node.is_url_page(w) && cwd.ends_with('/') && full.len() > cwd.len() && full.starts_with(cwd.as_str()) {
                    base = Some(cwd.clone());
                }
                let path = base.as_ref().map_or(full.as_str(), |b| &full[b.len()..]);
                field.set(path);
                // the name chosen, its extension left: what a rename
                // changes most
                let name_at = path.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
                let name = &path[name_at..];
                let stem = match name.rfind('.') {
                    Some(d) if d > 0 => d,
                    _ => name.len(),
                };
                let (a, z) = (path[..name_at].chars().count(), path[..name_at + stem].chars().count());
                if a < z {
                    field.anchor = Some(a);
                    field.cursor = z;
                } else {
                    field.select_all();
                }
            }
            Part::Label => {
                field.set(&self.node.window_label(w).unwrap_or_default());
                field.select_all();
            }
        }
        self.picker = None;
        self.tag_edit = Some(TagEdit { window: w, part, field, caret_since: Instant::now(), at, right, base, scroll: Cell::new(f32::MAX), b1: false, press: None, collapse: false });
        cx.notify();
    }

    pub fn tag_edit_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(e) = self.tag_edit.as_mut() else { return };
        e.caret_since = Instant::now();
        match key {
            "escape" => self.tag_edit = None,
            "enter" => self.tag_edit_commit(),
            _ => {
                if e.field.key(key, ch, mods) == Edited::No {
                    return;
                }
            }
        }
        cx.notify();
    }

    /// B2 or B3 pressed on the field: with B1 held in it, a chord, which
    /// cuts (`cut`) or pastes (`paste`) as the Edit menu does there; else
    /// a click off it, for `mouse_down`.
    fn tag_edit_chord(&mut self, what: &str, cx: &mut Context<Self>) {
        let Some(e) = self.tag_edit.as_mut().filter(|e| e.b1) else { return };
        // the chord's: no sweep after it, nor a click when B1 comes up
        e.press = None;
        e.collapse = false;
        cx.stop_propagation();
        self.overlay_edit(what, cx);
    }

    /// B1 pressed in the field: the cursor where it is, and a sweep from
    /// there as it moves (`tag_edit_drag`); a double-click a word, a
    /// third all of it. A press inside the selection keeps it until B1
    /// comes up, for a chord over it.
    fn tag_edit_press(&mut self, ev: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let i = self.tag_edit_index(ev.position, window);
        let Some(e) = self.tag_edit.as_mut() else { return };
        e.b1 = true;
        e.press = None;
        e.collapse = false;
        e.caret_since = Instant::now();
        let Some(i) = i else { return };
        match ev.click_count {
            0 | 1 => {
                e.press = Some(i);
                if e.field.selection().is_some_and(|(a, z)| a < i && i < z) {
                    e.collapse = true;
                } else {
                    e.field.click(i);
                }
            }
            2 => e.field.select_word(i),
            _ => e.field.select_all(),
        }
        cx.notify();
    }

    /// The pointer moved with B1 held from a press in the field: the
    /// selection swept from the press to it. True when that changed it.
    pub fn tag_edit_drag(&mut self, pos: Point<Pixels>, window: &mut Window) -> bool {
        let Some(from) = self.tag_edit.as_ref().filter(|e| e.b1).and_then(|e| e.press) else { return false };
        let Some(i) = self.tag_edit_index(pos, window) else { return false };
        let Some(e) = self.tag_edit.as_mut() else { return false };
        if e.collapse && i == from {
            return false;
        }
        e.collapse = false;
        let was = (e.field.anchor, e.field.cursor);
        e.field.sweep(from, i);
        e.caret_since = Instant::now();
        was != (e.field.anchor, e.field.cursor)
    }

    /// B1 come up: a press inside the selection that swept nothing is a
    /// click there after all.
    pub fn tag_edit_release(&mut self) {
        let Some(e) = self.tag_edit.as_mut() else { return };
        if let (true, Some(i)) = (e.collapse, e.press) {
            e.field.click(i);
        }
        e.b1 = false;
        e.press = None;
        e.collapse = false;
    }

    /// The character of the field under `pos`, as its text is drawn
    /// (`tag_overlays`): from the path's left, less the scroll.
    fn tag_edit_index(&self, pos: Point<Pixels>, window: &mut Window) -> Option<usize> {
        let e = self.tag_edit.as_ref()?;
        let text: &str = &e.field;
        if text.is_empty() {
            return Some(0);
        }
        let line = field_line(text, window);
        let b = line.closest_index_for_x(pos.x - e.at.left() + px(e.scroll.get())).min(text.len());
        Some(text[..b].chars().count())
    }

    /// The field put away, what it says taken: the window moved to the
    /// path typed, or its label changed -- on return, and as well when
    /// the user goes on to something else (a click anywhere, a command
    /// run), as an acme tag's name counts as soon as it is typed. Only
    /// escape throws it away.
    pub fn tag_edit_commit(&mut self) {
        let Some(e) = self.tag_edit.take() else { return };
        if self.node.state.window(e.window).is_err() {
            return;
        }
        let text = e.field.trim().to_string();
        let proposal = match e.part {
            Part::Path => self.rename_to(e.window, &text, e.base.as_deref()),
            Part::Label => {
                let label = Some(text).filter(|t| !t.is_empty());
                (label != self.node.window_label(e.window)).then_some(Proposal::SetLabel { window: e.window, label })
            }
        };
        if let Some(p) = proposal {
            apex_server::perform(&mut self.node, &mut self.log, vec![p]);
            self.after();
        }
    }

    /// The window moved to a path typed (or pasted) for it: nothing for
    /// nothing typed, or for the path it has. A relative one is in
    /// `base` when the field showed the path from there.
    fn rename_to(&self, w: WindowId, text: &str, base: Option<&str>) -> Option<Proposal> {
        if text.is_empty() {
            return None;
        }
        let path = match base {
            Some(b) if !text.starts_with('/') && !text.starts_with("~/") && !apex_core::is_url(text) => format!("{b}{text}"),
            _ => self.absolute_for(w, text),
        };
        (path != self.node.window_path(w)).then_some(Proposal::SetPath { window: w, path })
    }

    /// A path typed for window `w`, absolute: a relative one is in the
    /// folder of the path it had, as acme's names are in their window's
    /// directory; `~` is home.
    fn absolute_for(&self, w: WindowId, typed: &str) -> String {
        if typed.starts_with('/') || apex_core::is_url(typed) {
            return typed.to_string();
        }
        if let Some(rest) = typed.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                return format!("{}/{rest}", home.trim_end_matches('/'));
            }
        }
        let path = self.node.window_path(w);
        let base = match self.node.window_kind(w) {
            WinKind::Dir => path.trim_end_matches('/').rfind('/').map(|i| path[..=i].to_string()),
            _ => path.rfind('/').map(|i| path[..=i].to_string()),
        };
        let base = base.or_else(|| self.node.error_dir(Some(w)).map(|d| format!("{}/", d.trim_end_matches('/'))));
        match base {
            Some(b) => format!("{b}{typed}"),
            None => typed.to_string(),
        }
    }

    pub fn open_path_picker(&mut self, w: WindowId, atom: Atom, cx: &mut Context<Self>) {
        let path = self.node.window_path(w);
        if self.node.is_url_page(w) {
            return;
        }
        let Some((dir, current)) = folder_of(&path, atom) else { return };
        let Some(b) = self.layouts.get(&ViewId::Tag(w)).and_then(|l| l.atom_bounds(atom)) else { return };
        self.tag_edit_commit();
        let windows = self.associated(&dir, w);
        let (right, bottom) = (self.node.state.layout.r.x1 as f32 + self.left(), self.node.state.layout.r.y1 as f32 + self.top());
        let top = f32::from(b.bottom()) + 2.;
        let rows = (((bottom - 8. - top - PICKER_REST) / row_h()).floor().max(0.) as usize).clamp(MIN_ROWS, ROWS);
        // its rows' text under the part's (the panel's padding and the
        // row's before it), unless that would take it past the right edge
        let left = (f32::from(b.left()) - TEXT_IN).min(right - PICKER_MIN_W - 8.).max(8.);
        // where what is typed will start, from the first frame: after a
        // folder (or a directory listed as itself), where a file's name
        // was -- so the panel is in its place as it comes
        let typed_x = Some(f32::from(if matches!(atom, Atom::Name) && !path.ends_with('/') { b.left() } else { b.right() }));
        let at = point(px(left), px(top));
        self.picker = Some(Picker { window: w, windows, dir: dir.clone(), filter: LineEdit::new(), names: None, cursor: 0, at, rows, typed_x, caret_since: Instant::now(), current, warned: false });
        self.list_folder(w, &dir);
        // blinking while it is up
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(530)).await;
            let open = this.update(cx, |acme, cx| {
                let open = acme.picker.is_some() || acme.tag_edit.is_some();
                if open {
                    cx.notify();
                }
                open
            });
            if !matches!(open, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    /// Ask the host for a folder's entries, as ^F asks (`Acme::complete`).
    fn list_folder(&mut self, w: WindowId, dir: &str) {
        self.list_folder_as(ViewId::Tag(w), ExecCtx::Window(w), LISTING, dir);
    }

    /// `list_folder` for whoever asks: the answer comes back as
    /// `Candidates` with `view` and `at`, which say whose it is.
    pub(crate) fn list_folder_as(&mut self, view: ViewId, ctx: ExecCtx, at: usize, dir: &str) {
        match &mut self.backend {
            crate::app::Backend::Local(server) => {
                let here = server.dir_of(&self.node, ctx);
                let names = server.candidates(&here, dir);
                self.candidates.push(apex_server::proto::Candidates { view, at, prefix: dir.to_string(), names });
            }
            crate::app::Backend::Remote(link) => link.send(&apex_server::proto::ClientMsg::Candidates { view, ctx, at, prefix: dir.to_string() }),
        }
    }

    /// A folder's entries came: the picker's, if it still lists it.
    pub fn got_listing(&mut self, c: apex_server::proto::Candidates) {
        let Some(p) = self.picker.as_mut() else { return };
        if ViewId::Tag(p.window) != c.view || p.dir != c.prefix {
            return;
        }
        p.names = Some(c.names);
        let picks = p.picks();
        p.cursor = p.current.as_ref().and_then(|n| picks.iter().position(|x| matches!(x, Choice::Entry(e, _) if e == n))).unwrap_or(0);
    }

    pub fn picker_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(p) = self.picker.as_mut() else { return };
        p.caret_since = Instant::now();
        let n = p.picks().len();
        match key {
            "escape" => self.picker = None,
            "up" => p.cursor = p.cursor.saturating_sub(1),
            "down" => p.cursor = (p.cursor + 1).min(n.saturating_sub(1)),
            "enter" => {
                if let Some(pick) = p.picks().get(p.cursor).cloned() {
                    self.picker_pick(pick, mods.alt, cx);
                }
            }
            // into the folder chosen (else → moves in what is typed)
            "right" | "tab" if matches!(p.picks().get(p.cursor), Some(Choice::Entry(_, true))) => {
                if let Some(Choice::Entry(name, _)) = p.picks().get(p.cursor).cloned() {
                    self.picker_into(&name, cx);
                }
            }
            // nothing typed: up a folder
            "backspace" | "left" if p.filter.is_empty() => self.picker_up(),
            _ => {
                if p.filter.key(key, ch, mods) == Edited::No {
                    return;
                }
                p.cursor = 0;
                p.warned = false;
            }
        }
        cx.notify();
    }

    /// An entry chosen: a folder gone into; a file opened in a window of
    /// its own, or with ⌥ in this one in place of its file.
    fn picker_pick(&mut self, choice: Choice, alt: bool, cx: &mut Context<Self>) {
        let (name, is_dir) = match choice {
            // a window: gone to, by its id (its path is another's too)
            Choice::Window(w, _, _) => {
                self.picker = None;
                let loc = Loc { session: None, name: w.0.to_string(), pos: Pos::Keep };
                let _ = apex_server::proposal::apply(&mut self.node, &mut self.log, Proposal::Goto { loc });
                self.sync();
                self.after();
                cx.notify();
                return;
            }
            Choice::Entry(name, is_dir) => (name, is_dir),
        };
        let Some(p) = self.picker.as_mut() else { return };
        // a folder by its path with its slash, as its window is named
        let path = format!("{}{name}{}", p.dir, if is_dir { "/" } else { "" });
        let w = p.window;
        // here: in place of what this window shows, when it shows the
        // same kind of thing and is a file's or a folder's own
        let kind = self.node.window_kind(w);
        let here = kind == if is_dir { WinKind::Dir } else { WinKind::File } && !self.node.window_scratch(w) && !self.node.window_live(w);
        if alt && here {
            if self.node.window_unsaved(w) && !p.warned {
                p.warned = true;
                cx.notify();
                return;
            }
            self.picker = None;
            apex_server::perform(&mut self.node, &mut self.log, vec![Proposal::SetPath { window: w, path }]);
            self.after();
            self.execute(ExecCtx::Window(w), "Get", cx);
        } else {
            self.picker = None;
            self.goto(Loc { session: None, name: path, pos: Pos::Keep });
        }
        self.after();
        cx.notify();
    }

    /// The windows open on folder `dir` or a file in it (or a folder: a
    /// terminal in one) that are not a plain file's or folder's -- its
    /// errors, a preview, a terminal, a tool's pane -- but for `except`:
    /// its errors first, then previews, then the rest.
    pub fn associated(&self, dir: &str, except: WindowId) -> Vec<(WindowId, String, &'static str)> {
        let n = &self.node;
        let mut out = Vec::new();
        for w in n.state.windows.keys().copied().filter(|w| *w != except) {
            let kind = n.window_kind(w);
            if n.is_url_page(w) || (matches!(kind, WinKind::File | WinKind::Dir) && !n.window_scratch(w)) {
                continue;
            }
            let path = n.window_path(w);
            let Some(rest) = path.strip_prefix(dir).filter(|r| !r.trim_end_matches('/').contains('/')) else { continue };
            let what = n.window_label(w).unwrap_or_else(|| {
                match kind {
                    WinKind::Errors => "Errors",
                    WinKind::Page => "Preview",
                    WinKind::Term => "Terminal",
                    _ => "Window",
                }
                .into()
            });
            let title = if rest.is_empty() { what } else { format!("{rest} · {what}") };
            out.push((w, title, kind.name()));
        }
        let rank = |k: &str| match k {
            "errors" => 0,
            "preview" => 1,
            _ => 2,
        };
        out.sort_by(|a, b| rank(a.2).cmp(&rank(b.2)).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        out
    }

    /// Into folder `name` of the one listed.
    fn picker_into(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(p) = self.picker.as_ref() else { return };
        let dir = format!("{}{name}/", p.dir);
        let windows = self.associated(&dir, p.window);
        let Some(p) = self.picker.as_mut() else { return };
        p.windows = windows;
        p.dir = dir.clone();
        p.names = None;
        p.current = None;
        p.cursor = 0;
        p.warned = false;
        p.filter.clear();
        let w = p.window;
        self.list_folder(w, &dir);
        cx.notify();
    }

    /// Up to the folder the one listed is in, on the one left.
    fn picker_up(&mut self) {
        let Some(p) = self.picker.as_ref() else { return };
        let Some(up) = p.dir.trim_end_matches('/').rfind('/').map(|i| p.dir[..=i].to_string()) else { return };
        let windows = self.associated(&up, p.window);
        let Some(p) = self.picker.as_mut() else { return };
        p.windows = windows;
        let (w, from) = (p.window, p.dir.trim_end_matches('/').rsplit('/').next().map(String::from));
        p.dir = up.clone();
        p.names = None;
        p.current = from;
        p.cursor = 0;
        p.warned = false;
        self.list_folder(w, &up);
    }

    /// Where what is typed starts in the picker's tag, taken from the last
    /// frame's layouts before a frame clears them to lay them out anew.
    /// True when it moved (a folder gone into, or out of): the next frame
    /// puts the picker under it.
    pub fn picker_anchor(&mut self) -> bool {
        let Some(w) = self.picker.as_ref().map(|p| p.window) else { return false };
        let x = self.layouts.get(&ViewId::Tag(w)).and_then(|l| l.atom_bounds(Atom::Typed)).map(|b| f32::from(b.left()));
        let Some(p) = self.picker.as_mut() else { return false };
        if x.is_some() && x != p.typed_x {
            p.typed_x = x;
            return true;
        }
        false
    }

    /// The field over the path or label, and the picker under the path.
    pub fn tag_overlays(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let t = crate::theme::theme();
        let fs = crate::text_element::font_for(false);
        if let Some(e) = &self.tag_edit {
            let hint = match e.part {
                Part::Path => "A path",
                Part::Label => "A label",
            };
            // its text where the path's was (the border and padding to
            // its left), as wide as the text and some room to type --
            // never past the tag's row: past that the text scrolls, the
            // caret kept in sight
            let text: &str = &e.field;
            let line = (!text.is_empty()).then(|| field_line(text, window));
            let text_w = line.as_ref().map_or(0., |l| f32::from(l.width));
            let caret_x = line.as_ref().map_or(0., |l| f32::from(l.x_for_index(text.char_indices().nth(e.field.cursor).map_or(text.len(), |(b, _)| b))));
            let left = e.at.left() - px(5.);
            let room = f32::from(e.right - left) - 2.;
            let least = match e.part {
                Part::Path => f32::from(e.at.size.width) + 48.,
                Part::Label => 200.,
            };
            let width = (text_w + 32.).max(least).min(room).max(40.);
            let shown = width - 10. - 2.;
            // no room left bare at the end (it opens scrolled to there:
            // the name in sight)
            let mut scroll = e.scroll.get().min((text_w - shown).max(0.));
            if caret_x - scroll > shown {
                scroll = caret_x - shown;
            }
            if caret_x < scroll {
                scroll = caret_x;
            }
            scroll = scroll.max(0.);
            e.scroll.set(scroll);
            let field = div()
                .id("tag-edit")
                .w(px(width))
                .h(e.at.size.height)
                .px(px(4.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .overflow_hidden()
                .bg(rgb(t.body_bg))
                .border_1()
                .border_color(rgb(t.accent))
                .font(fs.font.clone())
                .text_size(fs.size)
                .text_color(rgb(t.text))
                .child(self.overlay_mark_by(px(4.)))
                .child(div().flex_1().min_w_0().h_full().flex().items_center().overflow_hidden().child(crate::field::field_view(&e.field, caret_on(e.caret_since), hint, true).flex_none().whitespace_nowrap().relative().left(px(-scroll))))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| this.tag_edit_press(ev, window, cx)))
                // B2 or B3 while B1 is held in it: the chords, cut and paste
                .on_mouse_down(MouseButton::Middle, cx.listener(|this, _, _, cx| this.tag_edit_chord("cut", cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.tag_edit_chord("paste", cx)));
            out.push(deferred(anchored().position(point(e.at.left() - px(5.), e.at.top())).child(field)).with_priority(2).into_any_element());
        }
        if let Some(p) = &self.picker {
            let picks = p.picks();
            let cursor = p.cursor.min(picks.len().saturating_sub(1));
            let rows = p.rows;
            let first = cursor.saturating_sub(rows - 1).min(picks.len().saturating_sub(rows));
            let mut list = div().flex().flex_col();
            let windows = picks.iter().filter(|c| matches!(c, Choice::Window(..))).count();
            for (i, choice) in picks.iter().enumerate().skip(first).take(rows) {
                let picked = i == cursor;
                // a line between the windows and the entries
                if i == windows && windows > 0 {
                    list = list.child(crate::shell::divider());
                }
                let dim = |d: gpui::Stateful<gpui::Div>| d.when(!picked, |d| d.text_color(rgb(t.panel_dim)));
                // a tag's line high: its names line up with the path's
                let mut row = crate::shell::popover_row(div().id(("pick", i)), picked, crate::shell::Act::Look).h(px(row_h()));
                match choice {
                    Choice::Window(_, title, kind) => {
                        row = row.child(div().flex_1().min_w_0().truncate().child(title.clone())).child(dim(div().id(("kind", i)).flex_none().pl(px(8.)).text_size(px(11.))).child(*kind));
                    }
                    Choice::Entry(name, dir) => {
                        // a long name cut short where the picker is narrow
                        row = row.child(div().flex_shrink(1.).min_w_0().truncate().child(name.clone()));
                        if *dir {
                            let name = name.clone();
                            row = row.child(dim(div().id(("slash", i)).flex_none()).child("/")).child(div().flex_1()).child(
                                // a folder's way in, at the row's end
                                dim(div().id(("into", i)).flex_none().px(px(6.)).rounded(px(3.)))
                                    .hover(|s| s.bg(rgb(t.panel_hover)))
                                    .child("›")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.picker_into(&name, cx);
                                            cx.stop_propagation();
                                        }),
                                    ),
                            );
                        }
                    }
                }
                let pick = choice.clone();
                list = list.child(row.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        this.picker_pick(pick.clone(), e.modifiers.alt, cx);
                        cx.stop_propagation();
                    }),
                ));
            }
            if picks.len() > first + rows {
                list = list.child(crate::shell::note(format!("{} more", picks.len() - first - rows)));
            }
            match &p.names {
                None => list = list.child(crate::shell::empty("Looking…").h(px(row_h()))),
                Some(Err(err)) => list = list.child(crate::shell::empty(err.clone()).h(px(row_h()))),
                Some(Ok(_)) if picks.is_empty() => list = list.child(crate::shell::empty(crate::shell::NOTHING).h(px(row_h()))),
                Some(Ok(_)) => {}
            }
            // what it says, warned: one more ⌥↩ replaces an unsaved window
            let warn = p.warned.then(|| div().text_color(rgb(t.accent)).child("Unsaved: ⌥↩ again to replace it").into_any_element());
            let panel = crate::shell::popover()
                .id("path-picker")
                .pt(px(POPOVER_PAD))
                .overflow_hidden()
                // the tag's face, at its size and weight: a name below is
                // set as it would be in the path above it
                .font_family(fs.font.family.clone())
                .font_weight(fs.font.weight)
                .text_size(fs.size)
                .flex()
                .flex_col()
                .child(self.overlay_mark())
                .child(div().px(px(POPOVER_PAD)).pb(px(POPOVER_PAD)).child(list))
                .child(crate::shell::foot(warn, "↩ open  ⌥↩ here  → in  ← up"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            // its rows' names under what is typed in the tag, as that moves
            // (a folder gone into, or up out of); as far left as the
            // window's edge makes it, and at the top it came down to
            let right = self.node.state.layout.r.x1 as f32 + self.left();
            let typed = p.typed_x.map(|x| x - TEXT_IN);
            // narrower where the window's edge is near, so the names stay
            // under the path; moved left only past the narrowest
            let want = typed.unwrap_or(f32::from(p.at.x));
            let width = (right - 8. - want).clamp(PICKER_MIN_W, PICKER_W);
            let left = want.min(right - 8. - width).max(8.);
            let panel = panel.w(px(width));
            out.push(deferred(anchored().position(point(px(left), p.at.y)).child(crate::shell::fade_in("path-picker", panel))).with_priority(2).into_any_element());
        }
        out
    }
}

/// The field's text as it is drawn, in the tag's face: where each of its
/// characters is.
fn field_line(text: &str, window: &mut Window) -> gpui::ShapedLine {
    let fs = crate::text_element::font_for(false);
    let run = gpui::TextRun { len: text.len(), font: fs.font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    window.text_system().shape_line(text.to_string().into(), fs.size, &[run], None)
}

#[cfg(test)]
mod tests {
    use super::{folder_of, Choice, Picker};
    use crate::text_element::Atom;
    use apex_core::WindowId;

    #[test]
    fn the_windows_on_a_folder_come_before_its_entries() {
        let mut p = Picker {
            window: WindowId(1),
            windows: vec![(WindowId(7), "Errors".into(), "errors"), (WindowId(8), "notes.md · Preview".into(), "preview")],
            dir: "/a/".into(),
            filter: crate::field::LineEdit::new(),
            names: Some(Ok(vec![("notes.md".into(), false), ("src".into(), true)])),
            cursor: 0,
            at: gpui::point(gpui::px(0.), gpui::px(0.)),
            rows: super::ROWS,
            typed_x: None,
            caret_since: std::time::Instant::now(),
            current: None,
            warned: false,
        };
        let all = p.picks();
        assert_eq!(all[0], Choice::Window(WindowId(7), "Errors".into(), "errors"));
        assert_eq!(all[1], Choice::Window(WindowId(8), "notes.md · Preview".into(), "preview"));
        // folders first among the entries
        assert_eq!(&all[2..], &[Choice::Entry("src".into(), true), Choice::Entry("notes.md".into(), false)]);
        // typed: the windows matching still first
        p.filter.set("note");
        assert_eq!(p.picks(), vec![Choice::Window(WindowId(8), "notes.md · Preview".into(), "preview"), Choice::Entry("notes.md".into(), false)]);
    }

    #[test]
    fn a_part_of_the_path_is_a_folder_and_the_entry_it_goes_through() {
        let p = "/a/b/notes.md";
        assert_eq!(folder_of(p, Atom::Dir(1)), Some(("/".into(), Some("a".into()))));
        assert_eq!(folder_of(p, Atom::Dir(3)), Some(("/a/".into(), Some("b".into()))));
        assert_eq!(folder_of(p, Atom::Dir(5)), Some(("/a/b/".into(), Some("notes.md".into()))));
        assert_eq!(folder_of(p, Atom::Name), Some(("/a/b/".into(), Some("notes.md".into()))));
        // a directory's name: the directory itself
        assert_eq!(folder_of("/a/b/", Atom::Name), Some(("/a/b/".into(), None)));
        assert_eq!(folder_of("notes.md", Atom::Name), None);
    }
}
