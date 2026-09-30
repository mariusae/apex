//! A window tag's head at work. Its path double-clicked becomes a field
//! (return moves the window there, as `apex win rename` does; a new
//! window's Untitled is one click); its label double-clicked, the same.
//! A folder of the path, or its name, clicked brings a picker down under
//! it, as VS Code's breadcrumbs do: the folder's entries (the name's
//! siblings), listed by the host as ^F's names are, so a remote one's
//! too, and narrowed as a query is typed; first, the windows open on the
//! folder or on a file in it that are not its own (its errors, a file's
//! preview, a terminal or a tool's pane there), to go to. Return (or a
//! click) opens the
//! one chosen, a file or a folder, in a window of its own; ⌥return in
//! this window in place of what it shows, a file for a file's window and
//! a folder for a folder's (a second time when this one is unsaved).
//! → or tab (or the › on its row) goes into a folder; ← or backspace
//! with nothing typed goes up. B3 on a folder or the name
//! plumbs the path to there; B1 or B2 on a verb runs it.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Bounds, Context, MouseButton, Pixels, Point, Window};

use apex_core::{ExecCtx, Loc, Pos, ViewId, WinKind, WindowId};
use apex_server::Proposal;

use crate::app::Acme;
use crate::field::{Edited, LineEdit};
use crate::text_element::{Atom, VERB_ICONS};

/// `Candidates::at` for a listing the picker asked for (a completion's is
/// a caret's offset).
pub const LISTING: usize = usize::MAX;

/// How long a click on the path waits to be a double-click before the
/// picker comes down.
const DOUBLE: Duration = Duration::from_millis(250);

/// The picker's rows at once.
const ROWS: usize = 12;

/// The picker's parts' heights, fixed, so its tallest is known when it
/// comes down and it can be put where that fits (`PICKER_H`), to stay.
const ROW_H: f32 = 24.;
const FIELD_H: f32 = 30.;
const NOTE_H: f32 = 18.;
/// The tallest the picker is: padding, the field, a full list with its
/// line between windows and entries and its "more", the foot, the gaps.
const PICKER_H: f32 = 8. + FIELD_H + ROWS as f32 * ROW_H + 5. + NOTE_H + NOTE_H + 4. * 2.;

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
    /// Where its top left is: under the part clicked when it fits there at
    /// its tallest, else as high as it must be to -- decided once, so it
    /// does not move as the list grows and shrinks.
    pub at: Point<Pixels>,
    pub caret_since: Instant,
    /// The entry to start on: the one the path goes through.
    pub current: Option<String>,
    /// ⌥return asked once of an unsaved window: again replaces it.
    pub warned: bool,
}

fn caret_on(since: Instant) -> bool {
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
/// it the path goes through: a folder's own child, or the name itself
/// among its siblings.
pub fn folder_of(path: &str, atom: Atom) -> Option<(String, Option<String>)> {
    let name_at = path.trim_end_matches('/').rfind('/').map_or(0, |i| i + 1);
    match atom {
        Atom::Dir(k) => {
            let rest = &path[k..];
            let next = rest.split('/').next().filter(|s| !s.is_empty()).map(String::from);
            Some((path[..k].to_string(), next))
        }
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
            (MouseButton::Left | MouseButton::Middle, Atom::Verb(_)) => self.atom_down = Some((w, atom, button)),
            (MouseButton::Left, Atom::Dir(_) | Atom::Name | Atom::Untitled) if clicks >= 2 => {
                self.picker_due = None;
                self.picker = None;
                self.tag_edit_start(w, Part::Path, cx);
            }
            (MouseButton::Left, Atom::Untitled) => self.tag_edit_start(w, Part::Path, cx),
            (MouseButton::Left, Atom::Label) if clicks >= 2 => self.tag_edit_start(w, Part::Label, cx),
            // the picker, unless this is the first of a double-click
            (MouseButton::Left, Atom::Dir(_) | Atom::Name) => {
                let at = Instant::now();
                self.picker_due = Some((w, atom, at));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(DOUBLE).await;
                    let _ = this.update(cx, |acme, cx| {
                        if acme.picker_due.is_some_and(|(_, _, t)| t == at) {
                            acme.picker_due = None;
                            acme.open_path_picker(w, atom, cx);
                        }
                    });
                })
                .detach();
            }
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

    /// The button come up where it went down on a verb: run it.
    pub fn release_atom(&mut self, pos: Point<Pixels>, button: MouseButton, cx: &mut Context<Self>) -> bool {
        let Some((w, atom, b)) = self.atom_down.take() else { return false };
        if b != button {
            return false;
        }
        let here = self.layouts.get(&ViewId::Tag(w)).and_then(|l| l.atom_at(pos));
        if here == Some(atom) {
            if let Atom::Verb(i) = atom {
                self.execute(ExecCtx::Window(w), VERB_ICONS[i].0, cx);
                self.after();
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
        let mut field = LineEdit::new();
        match part {
            Part::Path => {
                let path = self.node.window_path(w);
                field.set(&path);
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
        self.tag_edit = Some(TagEdit { window: w, part, field, caret_since: Instant::now(), at });
        cx.notify();
    }

    pub fn tag_edit_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(e) = self.tag_edit.as_mut() else { return };
        e.caret_since = Instant::now();
        match key {
            "escape" => self.tag_edit = None,
            "enter" => {
                let Some(e) = self.tag_edit.take() else { return };
                let text = e.field.trim().to_string();
                let proposal = match e.part {
                    Part::Path if text.is_empty() => None,
                    Part::Path => Some(Proposal::SetPath { window: e.window, path: self.absolute_for(e.window, &text) }),
                    Part::Label => Some(Proposal::SetLabel { window: e.window, label: Some(text).filter(|t| !t.is_empty()) }),
                };
                if let Some(p) = proposal {
                    apex_server::perform(&mut self.node, &mut self.log, vec![p]);
                    self.after();
                }
            }
            _ => {
                if e.field.key(key, ch, mods) == Edited::No {
                    return;
                }
            }
        }
        cx.notify();
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
        if self.node.window_kind(w) == WinKind::Web {
            return;
        }
        let Some((dir, current)) = folder_of(&path, atom) else { return };
        let Some(b) = self.layouts.get(&ViewId::Tag(w)).and_then(|l| l.atom_bounds(atom)) else { return };
        self.tag_edit = None;
        let windows = self.associated(&dir, w);
        let bottom = self.node.state.layout.r.y1 as f32 + self.top();
        let below = f32::from(b.bottom()) + 2.;
        let top = if below + PICKER_H + 8. <= bottom { below } else { (bottom - PICKER_H - 8.).max(8.) };
        let at = point(b.left() - px(6.), px(top));
        self.picker = Some(Picker { window: w, windows, dir: dir.clone(), filter: LineEdit::new(), names: None, cursor: 0, at, caret_since: Instant::now(), current, warned: false });
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
        let ctx = ExecCtx::Window(w);
        let view = ViewId::Tag(w);
        match &mut self.backend {
            crate::app::Backend::Local(server) => {
                let here = server.dir_of(&self.node, ctx);
                let names = server.candidates(&here, dir);
                self.candidates.push(apex_server::proto::Candidates { view, at: LISTING, prefix: dir.to_string(), names });
            }
            crate::app::Backend::Remote(link) => link.send(&apex_server::proto::ClientMsg::Candidates { view, ctx, at: LISTING, prefix: dir.to_string() }),
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
            if kind == WinKind::Web || (matches!(kind, WinKind::File | WinKind::Dir) && !n.window_scratch(w)) {
                continue;
            }
            let path = n.window_path(w);
            let Some(rest) = path.strip_prefix(dir).filter(|r| !r.trim_end_matches('/').contains('/')) else { continue };
            let what = n.window_label(w).unwrap_or_else(|| {
                match kind {
                    WinKind::Errors => "Errors",
                    WinKind::Preview => "Preview",
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

    /// The field over the path or label, and the picker under the path.
    pub fn tag_overlays(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let t = crate::theme::theme();
        let fs = crate::text_element::font_for(false);
        if let Some(e) = &self.tag_edit {
            let hint = match e.part {
                Part::Path => "A path",
                Part::Label => "A label",
            };
            let field = div()
                .id("tag-edit")
                .min_w(e.at.size.width + px(48.))
                .w(px(match e.part {
                    Part::Path => 420.,
                    Part::Label => 200.,
                }))
                .h(e.at.size.height)
                .px(px(4.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .bg(rgb(t.body_bg))
                .border_1()
                .border_color(rgb(t.accent))
                .font_family(fs.font.family.clone())
                .text_size(fs.size)
                .text_color(rgb(t.text))
                .child(self.overlay_mark_by(px(4.)))
                .child(crate::field::field_view(&e.field, caret_on(e.caret_since), hint, true))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            out.push(deferred(anchored().position(point(e.at.left() - px(5.), e.at.top())).child(field)).with_priority(2).into_any_element());
        }
        if let Some(p) = &self.picker {
            let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.2), offset: gpui::point(px(0.), px(4.)), blur_radius: px(14.), spread_radius: px(0.), inset: false };
            let picks = p.picks();
            let cursor = p.cursor.min(picks.len().saturating_sub(1));
            let first = cursor.saturating_sub(ROWS - 1).min(picks.len().saturating_sub(ROWS));
            let mut list = div().flex().flex_col();
            let note = |text: String| div().flex_none().h(px(ROW_H)).flex().items_center().px(px(8.)).text_color(rgb(t.panel_dim)).child(text);
            let windows = picks.iter().filter(|c| matches!(c, Choice::Window(..))).count();
            for (i, choice) in picks.iter().enumerate().skip(first).take(ROWS) {
                let picked = i == cursor;
                // a line between the windows and the entries
                if i == windows && windows > 0 {
                    list = list.child(div().flex_none().mx(px(8.)).my(px(2.)).h(px(1.)).bg(rgb(t.panel_border)));
                }
                let dim = |d: gpui::Stateful<gpui::Div>| d.when(!picked, |d| d.text_color(rgb(t.panel_dim)));
                let mut row = div()
                    .id(("pick", i))
                    .flex_none()
                    .h(px(ROW_H))
                    .items_center()
                    .px(px(8.))
                    .rounded(px(4.))
                    .flex()
                    .flex_row()
                    .gap(px(1.))
                    .cursor_default()
                    .text_color(rgb(if picked { t.panel_chosen_text } else { t.panel_text }))
                    .when(picked, |d| d.bg(rgb(t.panel_chosen_bg)))
                    .when(!picked, |d| d.hover(|s| s.bg(rgb(t.panel_hover))));
                match choice {
                    Choice::Window(_, title, kind) => {
                        row = row.child(div().flex_1().min_w_0().truncate().child(title.clone())).child(dim(div().id(("kind", i)).flex_none().pl(px(8.)).text_size(px(11.))).child(*kind));
                    }
                    Choice::Entry(name, dir) => {
                        row = row.child(div().flex_none().child(name.clone()));
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
            if picks.len() > first + ROWS {
                list = list.child(div().flex_none().h(px(NOTE_H)).px(px(8.)).text_size(px(11.)).text_color(rgb(t.panel_dim)).child(format!("{} more", picks.len() - first - ROWS)));
            }
            match &p.names {
                None => list = list.child(note("…".into())),
                Some(Err(err)) => list = list.child(note(err.clone())),
                Some(Ok(_)) if picks.is_empty() => list = list.child(note("Nothing matches".into())),
                Some(Ok(_)) => {}
            }
            let foot = if p.warned { "Unsaved: ⌥↩ again to replace it" } else { "↩ open  ⌥↩ here  → in  ← up" };
            let panel = div()
                .id("path-picker")
                .w(px(420.))
                .p(px(4.))
                .rounded(px(7.))
                .bg(rgb(t.panel_bg))
                .border_1()
                .border_color(rgb(t.panel_border))
                .shadow(vec![shadow])
                .font_family(fs.font.family.clone())
                .text_size(px(13.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(self.overlay_mark())
                // the folder's whole path, and what is typed as its next part
                .child(
                    div()
                        .flex_none()
                        .h(px(FIELD_H))
                        .mx(px(4.))
                        .px(px(4.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(rgb(t.panel_border))
                        .flex()
                        .flex_row()
                        .items_center()
                        // a long one cut at its start, its end beside the field
                        .child(div().flex_shrink(1.).min_w_0().overflow_hidden().flex().flex_row().justify_end().child(div().flex_none().whitespace_nowrap().text_color(rgb(t.panel_dim)).child(p.dir.clone())))
                        .child(div().flex_1().min_w(px(60.)).child(crate::field::field_view(&p.filter, caret_on(p.caret_since), "", true))),
                )
                .child(list)
                .child(div().flex_none().h(px(NOTE_H)).px(px(8.)).text_size(px(11.)).text_color(rgb(if p.warned { t.accent } else { t.panel_dim })).child(foot))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            out.push(deferred(anchored().position(p.at).child(panel)).with_priority(2).into_any_element());
        }
        out
    }
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
        // a directory's name: among its parent's entries
        assert_eq!(folder_of("/a/b/", Atom::Name), Some(("/a/".into(), Some("b".into()))));
        assert_eq!(folder_of("notes.md", Atom::Name), None);
    }
}
