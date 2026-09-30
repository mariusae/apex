//! The session's place in the title bar (`Meta::host`, `Meta::cwd`): the
//! host it runs on, dim, and its current directory as crumbs. A crumb
//! clicked brings down the folders in it -- folders only: the session's
//! directory is one -- listed by the host as ^F's names are, narrowed as a
//! query is typed, after the crumbs where the rest of the path was. Its
//! first row, `./`, is the folder itself. Return makes the one chosen
//! the session's directory, as `apex cd` does; → or tab (or the › on its
//! row) goes into a folder; ← or backspace with nothing typed goes up.
//! B3 on a crumb plumbs the path to there, as on a tag's path.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Context, FontWeight, MouseButton, Pixels, Point};

use apex_core::{ExecCtx, ViewId};

use crate::app::Acme;
use crate::field::{Edited, LineEdit};

/// `Candidates::at` for a listing the crumbs' picker asked for.
pub const CWD_LISTING: usize = usize::MAX - 1;

/// The most rows it shows at once.
const ROWS: usize = 12;
const ROW_H: f32 = 24.;
const PICKER_W: f32 = 320.;

pub struct CwdPicker {
    /// The folder listed, with its slash.
    pub dir: String,
    pub filter: LineEdit,
    /// Its folders, once the host said.
    pub names: Option<Result<Vec<String>, String>>,
    pub cursor: usize,
    /// Where its top left is: under the crumb clicked.
    pub at: Point<Pixels>,
    pub caret_since: Instant,
    /// The folder to start on: the one the session's directory goes
    /// through.
    pub current: Option<String>,
}

impl CwdPicker {
    /// The rows for what is typed: the folder itself (`None`) first while
    /// nothing is, then its folders -- all of them, or the ones matching,
    /// best first.
    pub fn picks(&self) -> Vec<Option<String>> {
        let q = self.filter.trim();
        let mut out = Vec::new();
        if q.is_empty() {
            out.push(None);
        }
        let Some(Ok(names)) = &self.names else { return out };
        if q.is_empty() {
            let mut v = names.clone();
            v.sort_by_key(|n| n.to_lowercase());
            out.extend(v.into_iter().map(Some));
        } else {
            let mut v: Vec<(f64, &String)> = names.iter().filter_map(|n| crate::finder::score(q, n).map(|s| (s, n))).collect();
            v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            out.extend(v.into_iter().map(|(_, n)| Some(n.clone())));
        }
        out
    }
}

/// A directory's crumbs: each part with its slash, and the path up to and
/// with it (`/`, `/Users/`, `/Users/me/`).
pub fn crumbs(dir: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, c) in dir.char_indices() {
        if c == '/' {
            out.push((dir[from..=i].to_string(), dir[..=i].to_string()));
            from = i + 1;
        }
    }
    if from < dir.len() {
        out.push((dir[from..].to_string(), dir.to_string()));
    }
    out
}

impl Acme {
    /// Make `dir` the session's current directory (absolute, or relative
    /// to the one it has), as `apex cd` does.
    pub fn cd(&mut self, dir: &str) {
        match &mut self.backend {
            crate::app::Backend::Local(server) => {
                if let Ok(op) = server.cd(dir) {
                    self.log.meta(op);
                }
            }
            crate::app::Backend::Remote(link) => link.send(&apex_server::proto::ClientMsg::Cd { dir: dir.to_string() }),
        }
        self.after();
    }

    /// The host, dim, and the session's directory as crumbs -- or, while
    /// its picker is down, the folder listed and what is typed after it.
    pub fn cwd_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let meta = &self.node.state.meta;
        if meta.cwd.is_empty() {
            return None;
        }
        let t = crate::theme::theme();
        let hover = crate::theme::step(crate::text_element::ground(&t), 1);
        let picking = self.cwd_picker.as_ref();
        let dir = picking.map(|p| p.dir.clone()).unwrap_or_else(|| meta.cwd.clone());
        let parts = crumbs(&dir);
        let last = parts.len().saturating_sub(1);
        let mut row = div().flex().flex_row().items_center().flex_none();
        for (i, (part, upto)) in parts.into_iter().enumerate() {
            // the directory's own name the strongest, when it is the
            // session's and not a folder being listed
            let own = i == last && picking.is_none();
            // the folder the crumb names, and the one after it in the path
            let next = crumbs(&dir).get(i + 1).map(|(p, _)| p.trim_end_matches('/').to_string());
            row = row.child(
                div()
                    .id(("cwd", i))
                    .flex_none()
                    .px(px(1.))
                    .rounded(px(4.))
                    .cursor_default()
                    .hover(move |s| s.bg(rgb(hover)))
                    .text_color(rgb(if own { t.text } else { t.text_dim }))
                    .when(own, |d| d.font_weight(crate::fonts::weight(FontWeight::MEDIUM)))
                    .child(part)
                    .on_mouse_down(MouseButton::Left, {
                        let upto = upto.clone();
                        cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                            // ⌘-click is B3, as everywhere in apex
                            if e.modifiers.platform {
                                this.cwd_picker = None;
                                this.look(ExecCtx::Top, &upto);
                                this.after();
                                cx.notify();
                            } else {
                                this.open_cwd_picker(upto.clone(), next.clone(), e.position.x, cx);
                            }
                            cx.stop_propagation();
                        })
                    })
                    // B3, as on a tag's path: the path to there plumbed
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.cwd_picker = None;
                            this.look(ExecCtx::Top, &upto);
                            this.after();
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        if let Some(p) = picking {
            row = row.child(div().flex_none().min_w(px(40.)).text_color(rgb(t.text)).child(crate::field::field_view(&p.filter, crate::tagedit::caret_on(p.caret_since), "", true)));
        }
        Some(
            div()
                .flex_shrink(1.)
                .min_w_0()
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .font_family(crate::fonts::ui())
                .child(div().flex_none().text_size(px(12.)).text_color(rgb(crate::text_element::mix(t.text_dim, crate::text_element::ground(&t), 0.35))).child(meta.host.clone()))
                // a long one cut at its start, its end beside the top row
                .child(div().flex_shrink(1.).min_w_0().overflow_hidden().flex().flex_row().justify_end().child(row))
                .into_any_element(),
        )
    }

    /// A crumb clicked: the folders in `dir` down under it, `current` (the
    /// one the session's directory goes through) chosen to begin with.
    fn open_cwd_picker(&mut self, dir: String, current: Option<String>, x: Pixels, cx: &mut Context<Self>) {
        self.picker = None;
        self.tag_edit = None;
        let at = point(x - px(13.), px(crate::title_h() + 2.));
        self.cwd_picker = Some(CwdPicker { dir: dir.clone(), filter: LineEdit::new(), names: None, cursor: 0, at, caret_since: Instant::now(), current });
        self.list_folder_as(ViewId::Top, ExecCtx::Top, CWD_LISTING, &dir);
        // blinking while it is up
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(530)).await;
            let open = this.update(cx, |acme, cx| {
                let open = acme.cwd_picker.is_some();
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

    /// A folder's entries came: its folders the picker's, if it still
    /// lists it.
    pub fn got_cwd_listing(&mut self, c: apex_server::proto::Candidates) {
        let Some(p) = self.cwd_picker.as_mut() else { return };
        if p.dir != c.prefix {
            return;
        }
        p.names = Some(c.names.map(|v| v.into_iter().filter(|(_, dir)| *dir).map(|(n, _)| n).collect()));
        let picks = p.picks();
        p.cursor = p.current.as_ref().and_then(|n| picks.iter().position(|x| x.as_deref() == Some(n.as_str()))).unwrap_or(0);
    }

    pub fn cwd_picker_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(p) = self.cwd_picker.as_mut() else { return };
        p.caret_since = Instant::now();
        let n = p.picks().len();
        match key {
            "escape" => self.cwd_picker = None,
            "up" => p.cursor = p.cursor.saturating_sub(1),
            "down" => p.cursor = (p.cursor + 1).min(n.saturating_sub(1)),
            "enter" => {
                if let Some(pick) = p.picks().get(p.cursor).cloned() {
                    self.cwd_pick(pick, cx);
                }
            }
            // into the folder chosen (else → moves in what is typed)
            "right" | "tab" if matches!(p.picks().get(p.cursor), Some(Some(_))) => {
                if let Some(Some(name)) = p.picks().get(p.cursor).cloned() {
                    self.cwd_into(&name);
                }
            }
            // nothing typed: up a folder
            "backspace" | "left" if p.filter.is_empty() => self.cwd_up(),
            _ => {
                if p.filter.key(key, ch, mods) == Edited::No {
                    return;
                }
                p.cursor = 0;
            }
        }
        cx.notify();
    }

    /// A row chosen: the folder listed (`None`) or one in it, the
    /// session's directory now.
    fn cwd_pick(&mut self, pick: Option<String>, cx: &mut Context<Self>) {
        let Some(p) = self.cwd_picker.take() else { return };
        let dir = match pick {
            Some(name) => format!("{}{name}/", p.dir),
            None => p.dir,
        };
        self.cd(&dir);
        cx.notify();
    }

    fn cwd_into(&mut self, name: &str) {
        let Some(p) = self.cwd_picker.as_mut() else { return };
        let dir = format!("{}{name}/", p.dir);
        p.dir = dir.clone();
        p.names = None;
        p.current = None;
        p.cursor = 0;
        p.filter.clear();
        self.list_folder_as(ViewId::Top, ExecCtx::Top, CWD_LISTING, &dir);
    }

    fn cwd_up(&mut self) {
        let Some(p) = self.cwd_picker.as_mut() else { return };
        let Some(up) = p.dir.trim_end_matches('/').rfind('/').map(|i| p.dir[..=i].to_string()) else { return };
        let from = p.dir.trim_end_matches('/').rsplit('/').next().map(String::from);
        p.dir = up.clone();
        p.names = None;
        p.current = from;
        p.cursor = 0;
        self.list_folder_as(ViewId::Top, ExecCtx::Top, CWD_LISTING, &up);
    }

    /// The crumbs' picker, when down.
    pub fn cwd_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.cwd_picker.as_ref()?;
        let t = crate::theme::theme();
        let picks = p.picks();
        let bottom = self.node.state.layout.r.y1 as f32 + self.top();
        let rows = (((bottom - f32::from(p.at.y) - 80.) / ROW_H).floor().max(3.) as usize).min(ROWS);
        let cursor = p.cursor.min(picks.len().saturating_sub(1));
        let first = cursor.saturating_sub(rows - 1).min(picks.len().saturating_sub(rows));
        let note = |s: String| div().flex_none().h(px(ROW_H)).flex().items_center().px(px(8.)).text_color(rgb(t.panel_dim)).child(s);
        let mut list = div().flex().flex_col();
        for (i, pick) in picks.iter().enumerate().skip(first).take(rows) {
            let picked = i == cursor;
            let dim = move |d: gpui::Stateful<gpui::Div>| d.when(!picked, |d| d.text_color(rgb(t.panel_dim)));
            let mut row = div()
                .id(("cwd-pick", i))
                .flex_none()
                .h(px(ROW_H))
                .px(px(8.))
                .rounded(px(4.))
                .flex()
                .flex_row()
                .items_center()
                .cursor_default()
                .text_color(rgb(if picked { t.panel_chosen_text } else { t.panel_text }))
                .when(picked, |d| d.bg(rgb(t.panel_chosen_bg)))
                .when(!picked, |d| d.hover(|s| s.bg(rgb(t.panel_hover))));
            match pick {
                None => {
                    row = row.child(div().flex_none().child("./")).child(dim(div().id(("cwd-here", i)).flex_none().pl(px(8.)).text_size(px(11.))).child("this folder"));
                }
                Some(name) => {
                    let into = name.clone();
                    row = row
                        .child(div().flex_shrink(1.).min_w_0().truncate().child(name.clone()))
                        .child(dim(div().id(("cwd-slash", i)).flex_none()).child("/"))
                        .child(div().flex_1())
                        .child(
                            dim(div().id(("cwd-into", i)).flex_none().px(px(6.)).rounded(px(3.)))
                                .hover(|s| s.bg(rgb(t.panel_hover)))
                                .child("›")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.cwd_into(&into);
                                        cx.notify();
                                        cx.stop_propagation();
                                    }),
                                ),
                        );
                }
            }
            let pick = pick.clone();
            list = list.child(row.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.cwd_pick(pick.clone(), cx);
                    cx.stop_propagation();
                }),
            ));
        }
        if picks.len() > first + rows {
            list = list.child(div().flex_none().h(px(18.)).px(px(8.)).text_size(px(11.)).text_color(rgb(t.panel_dim)).child(format!("{} more", picks.len() - first - rows)));
        }
        match &p.names {
            None => list = list.child(note("…".into())),
            Some(Err(e)) => list = list.child(note(e.clone())),
            Some(Ok(_)) if picks.is_empty() => list = list.child(note("No folder matches".into())),
            Some(Ok(_)) => {}
        }
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.2), offset: gpui::point(px(0.), px(4.)), blur_radius: px(14.), spread_radius: px(0.), inset: false };
        let panel = div()
            .id("cwd-picker")
            .w(px(PICKER_W))
            .p(px(4.))
            .rounded(px(7.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .font_family(crate::fonts::ui())
            .text_size(px(13.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(self.overlay_mark())
            .child(list)
            .child(div().flex_none().h(px(18.)).px(px(8.)).text_size(px(11.)).text_color(rgb(t.panel_dim)).child("↩ cd  → in  ← up"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let right = self.node.state.layout.r.x1 as f32 + self.left();
        let left = f32::from(p.at.x).min(right - PICKER_W - 8.).max(8.);
        Some(deferred(anchored().position(point(px(left), p.at.y)).child(panel)).with_priority(2).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::crumbs;

    #[test]
    fn a_directory_is_its_crumbs() {
        assert_eq!(crumbs("/a/bc/"), vec![("/".into(), "/".into()), ("a/".into(), "/a/".into()), ("bc/".into(), "/a/bc/".into())]);
    }
}
