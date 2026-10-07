//! ⌘O: open a file or folder in the session's directory. The host lists
//! everything under it and matches what is typed (`apex_server::find`):
//! the list fills in as the walk goes, never waits for it, and only the
//! best page crosses the wire -- a larger page asked for as the cursor
//! nears the end of what came. A key typed is a new query at once (the
//! host leaves an older one part way); an answer to an older one is shown
//! only while the newest has not come. Closing it (escape, a pick, a
//! click elsewhere, another ⌘O) stops the walk and the matching on the
//! host. Return opens the one chosen; a folder opens as its window.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{deferred, div, px, rgb, AnyElement, Context, MouseButton};

use apex_core::{Loc, Pos};
use apex_server::proto::{ClientMsg, ServerMsg};

use crate::app::{Acme, Backend};
use crate::field::{Edited, LineEdit};

/// A page: how many matches are asked for at a time.
const PAGE: usize = 200;
/// The rows shown at once.
const ROWS: usize = 12;
/// How often what has come is taken in while it is up.
const POLL: Duration = Duration::from_millis(40);

pub struct QuickOpen {
    /// The request, as the host knows it.
    pub id: u64,
    /// The directory listed, with its slash.
    pub root: String,
    pub filter: LineEdit,
    /// The newest query sent, and how many it asked for.
    pub gen: u64,
    pub limit: usize,
    /// The newest answer taken in: its generation, its matches (paths
    /// relative to `root`, and whether each is a folder), and its counts.
    pub shown: u64,
    pub items: Vec<(String, bool)>,
    pub matched: u64,
    pub indexed: u64,
    pub done: bool,
    pub capped: bool,
    pub error: Option<String>,
    pub cursor: usize,
    pub caret_since: Instant,
    /// The caret as last drawn (on or off in its blink).
    blinked: bool,
    /// In-process, the listing itself and what it sends.
    local: Option<(apex_server::find::Job, Arc<Mutex<Vec<ServerMsg>>>)>,
}

/// A count with its thousands set apart: 1,234,567.
fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Acme {
    /// ⌘O: the listing of the session's directory, begun.
    pub fn open_quick(&mut self, cx: &mut Context<Self>) {
        if self.quick.is_some() {
            self.close_quick(cx);
            return;
        }
        let root = self.node.state.meta.cwd.clone();
        if root.is_empty() {
            self.notice("Open: the session has no directory yet\n");
            return;
        }
        self.finder = None;
        self.commands = None;
        self.next_find += 1;
        let id = self.next_find;
        let local = match &mut self.backend {
            Backend::Remote(link) => {
                // what an earlier listing left is no one's now
                link.found.clear();
                link.send(&ClientMsg::FindStart { id, dir: root.clone() });
                None
            }
            Backend::Local(_) => {
                let queue: Arc<Mutex<Vec<ServerMsg>>> = Arc::default();
                let q = queue.clone();
                let job = apex_server::find::Job::start(id, root.clone().into(), move |m| q.lock().unwrap().push(m));
                Some((job, queue))
            }
        };
        self.quick = Some(QuickOpen { id, root, filter: LineEdit::new(), gen: 0, limit: PAGE, shown: 0, items: Vec::new(), matched: 0, indexed: 0, done: false, capped: false, error: None, cursor: 0, caret_since: Instant::now(), blinked: true, local });
        self.quick_query(false);
        // what comes taken in while it is up, and drawn when it changed
        // or the caret blinks
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let open = this.update(cx, |acme, cx| {
                if acme.quick.as_ref().is_none_or(|q| q.id != id) {
                    return false;
                }
                let blink = acme.quick.as_mut().is_some_and(|q| {
                    let on = crate::tagedit::caret_on(q.caret_since);
                    std::mem::replace(&mut q.blinked, on) != on
                });
                if acme.quick_take() || blink {
                    cx.notify();
                }
                true
            });
            if !matches!(open, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    /// Ask for the query as it is now: anew from the top (`fresh`), or a
    /// larger page of the same.
    fn quick_query(&mut self, fresh: bool) {
        let Some(q) = self.quick.as_mut() else { return };
        q.gen += 1;
        if fresh {
            q.limit = PAGE;
            q.cursor = 0;
        }
        let (id, gen, query, limit) = (q.id, q.gen, q.filter.trim().to_string(), q.limit);
        match (&mut self.backend, &q.local) {
            (_, Some((job, _))) => job.query(gen, &query, limit),
            (Backend::Remote(link), None) => link.send(&ClientMsg::FindQuery { id, gen, query, limit: limit as u32 }),
            _ => {}
        }
    }

    /// Take in what has come: the newest answer, if newer than shown.
    /// Whether anything did.
    fn quick_take(&mut self) -> bool {
        let Some(q) = self.quick.as_mut() else { return false };
        let mut took = false;
        let came: Vec<ServerMsg> = match (&mut self.backend, &q.local) {
            (_, Some((_, queue))) => std::mem::take(&mut *queue.lock().unwrap()),
            (Backend::Remote(link), None) => std::mem::take(&mut link.found),
            _ => Vec::new(),
        };
        for m in came {
            if let ServerMsg::Found { id, gen, items, matched, indexed, done, capped, error } = m {
                if id != q.id || gen < q.shown || gen > q.gen {
                    continue;
                }
                // the same query, a larger page: the cursor stays
                q.shown = gen;
                q.items = items;
                q.matched = matched;
                q.indexed = indexed;
                q.done = done;
                q.capped = capped;
                q.error = error;
                q.cursor = q.cursor.min(q.items.len().saturating_sub(1));
                took = true;
            }
        }
        took
    }

    pub fn close_quick(&mut self, cx: &mut Context<Self>) {
        if let Some(q) = self.quick.take() {
            if let (Backend::Remote(link), None) = (&mut self.backend, &q.local) {
                link.send(&ClientMsg::FindStop { id: q.id });
            }
            // in-process, the job ends as it is dropped
        }
        cx.notify();
    }

    /// The cursor moved: when it nears the end of what came and more
    /// match, the next page is asked for.
    fn quick_moved(&mut self) {
        let Some(q) = self.quick.as_mut() else { return };
        let more = (q.matched as usize) > q.items.len();
        if more && q.cursor + ROWS >= q.items.len() && q.limit <= q.items.len() {
            q.limit += PAGE;
            self.quick_query(false);
        }
    }

    /// The field changed: the query anew.
    pub fn quick_changed(&mut self) {
        self.quick_query(true);
    }

    pub fn quick_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let _ = window;
        let Some(q) = self.quick.as_mut() else { return };
        q.caret_since = Instant::now();
        let last = q.items.len().saturating_sub(1);
        let cursor = q.cursor;
        match key {
            "escape" => {
                self.close_quick(cx);
                return;
            }
            "enter" => {
                self.quick_pick(cursor, cx);
                return;
            }
            "up" => q.cursor = q.cursor.saturating_sub(1),
            "down" => q.cursor = (q.cursor + 1).min(last),
            "pageup" => q.cursor = q.cursor.saturating_sub(ROWS),
            "pagedown" => q.cursor = (q.cursor + ROWS).min(last),
            _ => {
                if q.filter.key(key, ch, mods) == Edited::No {
                    return;
                }
                self.quick_changed();
                cx.notify();
                return;
            }
        }
        self.quick_moved();
        cx.notify();
    }

    /// Open match `i`: the file, or the folder as its window.
    fn quick_pick(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some((rel, dir)) = self.quick.as_ref().and_then(|q| q.items.get(i).cloned()) else { return };
        let root = self.quick.as_ref().map(|q| q.root.clone()).unwrap_or_default();
        self.close_quick(cx);
        let name = if dir { format!("{root}{rel}/") } else { format!("{root}{rel}") };
        self.goto(Loc { session: None, name, pos: Pos::Keep });
        self.after();
        cx.notify();
    }

    /// The panel, when up.
    pub fn quick_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = self.quick.as_ref()?;
        let t = crate::theme::theme();
        let hint = format!("Open in {}", crate::sidebar::shown(&q.root, "").trim_end_matches('/'));
        let field = crate::shell::palette_field(crate::field::field_view(&q.filter, crate::tagedit::caret_on(q.caret_since), &hint, true));
        let mut list = div().flex().flex_col().px(px(6.)).pb(px(2.));
        let first = q.cursor.saturating_sub(ROWS - 1).min(q.items.len().saturating_sub(ROWS));
        for (i, (rel, is_dir)) in q.items.iter().enumerate().skip(first).take(ROWS) {
            let picked = i == q.cursor;
            let dim = crate::shell::palette_dim(picked, crate::shell::Act::Look);
            let (dir, name) = match rel.rfind('/') {
                Some(k) => (rel[..=k].to_string(), rel[k + 1..].to_string()),
                None => (String::new(), rel.clone()),
            };
            let name = if *is_dir { format!("{name}/") } else { name };
            let row = crate::shell::palette_row(picked, crate::shell::Act::Look)
                .id(("quick", i))
                .cursor_default()
                .child(div().flex_none().w(px(16.)).flex().justify_center().text_size(px(11.)).text_color(dim).child(if *is_dir { "▸" } else { "·" }))
                .child(div().flex_none().max_w(gpui::relative(0.6)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
                .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().text_size(px(12.)).text_color(dim).child(dir))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.quick_pick(i, cx);
                        cx.stop_propagation();
                    }),
                );
            list = list.child(row);
        }
        let dimc = rgb(t.panel_dim);
        if q.items.is_empty() {
            let why = match (&q.error, q.done, q.filter.trim().is_empty()) {
                (Some(e), _, _) => e.clone(),
                (None, false, _) => "Looking…".into(),
                (None, true, true) => "Nothing here".into(),
                (None, true, false) => "Nothing matches".into(),
            };
            list = list.child(div().px(px(10.)).py(px(8.)).text_size(px(13.)).text_color(dimc).child(why));
        }
        // how far the walk has got, and how many match
        let status = {
            let walked = if q.done { format!("{} entries", count(q.indexed)) } else { format!("Indexing… {}", count(q.indexed)) };
            let capped = if q.capped { " (stopped there: type more of the path)" } else { "" };
            if q.filter.trim().is_empty() {
                format!("{walked}{capped}")
            } else {
                format!("{} matching · {walked}{capped}", count(q.matched))
            }
        };
        let foot = div()
            .flex_none()
            .h(px(28.))
            .px(px(16.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .text_size(px(11.))
            .text_color(dimc)
            .border_t_1()
            .border_color(rgb(t.panel_border))
            .child(div().min_w_0().truncate().child(status))
            .child(div().flex_none().child("↩ open"));
        let panel = crate::shell::palette_panel()
            .child(self.overlay_mark())
            .child(field)
            .child(list)
            .child(foot)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        Some(deferred(crate::shell::palette_place(panel)).with_priority(1).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::count;

    #[test]
    fn counts_have_their_thousands_set_apart() {
        assert_eq!((count(0), count(999), count(1000), count(1234567)), ("0".into(), "999".into(), "1,000".into(), "1,234,567".into()));
    }
}
