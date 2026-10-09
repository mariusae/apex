//! Looking as you type, acme's way: the query is the first `Look`'s
//! argument in a window's tag, the result the window's selection. Only
//! an argument written `Look/word` is looked for as typed; the slash says
//! so in the text (a word typed after a bare `Look ` may be a command).
//! While the caret is in that argument, every change to it looks again from
//! where the selection was when the typing began (its anchor): a letter
//! more narrows to the same place or one further on, a letter less goes
//! back; empty, the selection is where it began; nothing found, it stays,
//! and the argument is struck through. Escape ends it, the selection a
//! caret at its end and the pointer there, to edit where it found (while
//! typing, the pointer stays: the keys go where it is). ⌘F takes the
//! caret (and the pointer, as acme's moves) to the argument, making it
//! `Look/…/` (the closing slash lets it have spaces); ⌘G and ⌘⇧G
//! look for it again forwards and back, the pointer on what they find as
//! B3's is. Every place the word is in the window is
//! washed faintly while a look goes on there -- while the argument is
//! live, or the selection is one of them (after ⌘G, or B3, which puts
//! its word in the argument too); once the selection is elsewhere, the
//! marks are gone.
//!
//! A page and a terminal have no buffer to look in: a page is looked in
//! by its view, as the browser finds (its places marked there), and a
//! terminal in its screen and history, which only the host has
//! (`TermFind`), what is found its selection and the view moved to it.

use std::rc::Rc;

use gpui::{Context, Window};

use apex_core::text::{find_all, find_match};
use apex_core::*;

use crate::app::{Acme, Backend};
use apex_server::proto::ClientMsg;

/// A look going on in a window's tag: where the body's selection was when
/// the typing began, what was last looked for, and whether it was found.
pub struct Live {
    pub window: WindowId,
    pub anchor: (usize, usize),
    /// A terminal's anchor: where its selection began, else the top of
    /// its view (`(column, history line)`).
    pub term_at: Option<(u16, u64)>,
    pub arg: String,
    pub failed: bool,
}

/// Where a Look in a window is done when it has no buffer to look in.
enum Elsewhere {
    Page,
    Term(TermId),
}

impl Acme {
    /// A key in window `w`'s tag: the caret in its Look's argument, the
    /// argument changed, looked for again from the anchor; out of it, the
    /// look over.
    pub fn live_look(&mut self, w: WindowId) {
        let Some(LookArg { start, end, arg, .. }) = self.node.look_arg(w).filter(|a| a.live) else {
            if self.looking.as_ref().is_some_and(|l| l.window == w) {
                self.looking = None;
            }
            return;
        };
        let caret = self.node.selection(ViewId::Tag(w)).unwrap_or((usize::MAX, 0));
        if caret.0 != caret.1 || caret.0 < start || caret.0 > end {
            if self.looking.as_ref().is_some_and(|l| l.window == w) {
                self.looking = None;
            }
            return;
        }
        let (anchor, term_at) = match self.looking.as_ref().filter(|l| l.window == w) {
            Some(l) if l.arg == arg => return, // the caret moved, the word did not
            Some(l) => (l.anchor, l.term_at),
            None => (self.node.selection(ViewId::Body(w)).unwrap_or((0, 0)), self.term_anchor(w)),
        };
        self.looking = Some(Live { window: w, anchor, term_at, arg: arg.clone(), failed: false });
        let failed = match self.elsewhere(w) {
            Some(Elsewhere::Page) => {
                self.webs.find_typed(w, &arg);
                false
            }
            // what it finds comes back (`term_found`), and says then
            Some(Elsewhere::Term(_)) => {
                if !arg.is_empty() {
                    self.term_look(w, &arg, false, term_at);
                }
                return;
            }
            None => !self.look_from(w, &arg, anchor, false),
        };
        if let Some(l) = self.looking.as_mut() {
            l.failed = failed;
        }
    }

    /// A window with no buffer to look in, and what is looked in instead.
    fn elsewhere(&self, w: WindowId) -> Option<Elsewhere> {
        match self.node.state.window(w).ok()?.body {
            Body::Term(t) => Some(Elsewhere::Term(t)),
            _ if self.node.state.window(w).ok()?.is_page() => Some(Elsewhere::Page),
            _ => None,
        }
    }

    /// Where a look in terminal window `w` begins: its selection's start,
    /// else the top of its view.
    fn term_anchor(&self, w: WindowId) -> Option<(u16, u64)> {
        let t = self.term_of(w)?;
        match self.term_sel {
            Some((sw, a, b)) if sw == w => {
                let a = if (a.1, a.0) <= (b.1, b.0) { a } else { b };
                Some((a.0 as u16, a.1))
            }
            _ => Some((0, self.node.state.terms.get(&t)?.top)),
        }
    }

    /// `text` looked for in terminal window `w`'s screen and history by the
    /// host (`TermFind`): from `from`, else past the selection (before it,
    /// `reverse`), else from the top of the view. An empty text is the
    /// selection's. What is found comes back to `term_found`.
    pub fn term_look(&mut self, w: WindowId, text: &str, reverse: bool, from: Option<(u16, u64)>) {
        let Some(t) = self.term_of(w) else { return };
        let sel = self.term_sel.filter(|(sw, _, _)| *sw == w).map(|(_, a, b)| if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) });
        let text = if text.is_empty() { sel.and_then(|(a, b)| self.term_grid_text(w, a, b)).unwrap_or_default() } else { text.to_string() };
        if text.is_empty() {
            return;
        }
        let top = self.node.state.terms.get(&t).map_or(0, |x| x.top);
        let from = from.unwrap_or(match (sel, reverse) {
            (Some((a, _)), true) => (a.0 as u16, a.1),
            (Some((_, b)), false) => (b.0 as u16, b.1),
            (None, _) => (0, top),
        });
        match &mut self.backend {
            Backend::Local(server) => {
                let at = server.term_find(&mut self.log, t, &text, from, reverse);
                let _ = self.node.catch_up(&self.log);
                self.term_found(t, at);
            }
            Backend::Remote(link) => link.send(&ClientMsg::TermFind { term: t, text, from, reverse }),
        }
    }

    /// What a `TermFind` found in terminal `t`: its selection, and a live
    /// look there told whether it was found.
    pub fn term_found(&mut self, t: TermId, at: Option<((u16, u64), (u16, u64))>) {
        let Some(w) = self.node.state.windows.iter().find(|(_, x)| x.body == Body::Term(t)).map(|(w, _)| *w) else { return };
        if let Some((a, b)) = at {
            self.term_sel = Some((w, (a.0 as usize, a.1), (b.0 as usize, b.1)));
        }
        if let Some(l) = self.looking.as_mut().filter(|l| l.window == w) {
            l.failed = at.is_none();
        }
    }

    /// Escape in the argument: the look over, the selection made a caret
    /// at its end and the pointer taken there, as B3's is -- the keys
    /// following it into the text, to edit where the look found.
    pub fn look_done(&mut self, w: WindowId) {
        self.looking = None;
        // a page's or a terminal's: what was found stays as it is
        if self.elsewhere(w).is_some() {
            self.after();
            return;
        }
        let v = ViewId::Body(w);
        let (_, q1) = self.node.selection(v).unwrap_or((0, 0));
        let _ = self.node.select(&mut self.log, v, q1, q1);
        self.node.warp = Some(apex_core::tiling::Warp::Sel(v));
        self.show_at.insert(v, (q1, 1));
        self.after();
    }

    /// `arg` looked for in window `w`'s body from `from` (forwards from its
    /// start, or back from it), selected and brought into view; empty, the
    /// selection put back at `from`. False when it is nowhere.
    fn look_from(&mut self, w: WindowId, arg: &str, from: (usize, usize), reverse: bool) -> bool {
        let v = ViewId::Body(w);
        let (q0, q1) = if arg.is_empty() {
            from
        } else {
            let Some(text) = self.node.view_buffer(v).ok().and_then(|b| self.node.state.buffer(b).ok()).map(|b| b.text.to_string()) else { return false };
            match find_match(&text, arg, from.0, reverse) {
                Some(i) => (i, i + arg.chars().count()),
                None => return false,
            }
        };
        let seltext = self.node.seltext;
        let _ = self.node.select(&mut self.log, v, q0, q1);
        // the keys stay where they were going (the tag, while typing)
        self.node.seltext = seltext.or(Some(v));
        self.show_at.insert(v, (q0, 1));
        true
    }

    /// ⌘F: the caret in the Look's argument of the window acme would act
    /// on, made `Look/…/` (`Look// ` typed first when its tag has none),
    /// the argument selected to type over, and the pointer on it.
    pub fn find_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.window_at_pointer(window) else { return };
        if self.node.look_arg(w).is_none() {
            let _ = self.node.select(&mut self.log, ViewId::Tag(w), 0, 0);
            let _ = self.node.insert(&mut self.log, ViewId::Tag(w), "Look// ");
        }
        let _ = self.node.make_look_live(&mut self.log, w);
        let Some(LookArg { start, end, arg, .. }) = self.node.look_arg(w) else { return };
        let _ = self.node.select(&mut self.log, ViewId::Tag(w), start, end);
        self.node.warp = Some(apex_core::tiling::Warp::Sel(ViewId::Tag(w)));
        let anchor = self.node.selection(ViewId::Body(w)).unwrap_or((0, 0));
        let term_at = self.term_anchor(w);
        self.looking = Some(Live { window: w, anchor, term_at, arg, failed: false });
        self.after();
        cx.notify();
    }

    /// ⌘G (or ⌘⇧G, `reverse`): the Look's argument looked for again in
    /// the window acme would act on, past the selection (or before it).
    /// With none, the selection is looked for, and is the argument then.
    pub fn find_next(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.window_at_pointer(window) else { return };
        let v = ViewId::Body(w);
        // a page: the browser's find, again; a terminal: the host's, past
        // its selection
        match self.elsewhere(w) {
            Some(Elsewhere::Page) => {
                let arg = self.node.look_arg(w).map(|a| a.arg).unwrap_or_default();
                self.webs.find(w, &arg, reverse);
                self.after();
                cx.notify();
                return;
            }
            Some(Elsewhere::Term(_)) => {
                let mut arg = self.node.look_arg(w).map(|a| a.arg).unwrap_or_default();
                if arg.is_empty() {
                    let sel = self.term_sel.filter(|(sw, _, _)| *sw == w).map(|(_, a, b)| if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) });
                    arg = sel.and_then(|(a, b)| self.term_grid_text(w, a, b)).unwrap_or_default();
                    let _ = self.node.set_look_arg(&mut self.log, w, &arg);
                }
                self.term_look(w, &arg, reverse, None);
                self.after();
                cx.notify();
                return;
            }
            None => {}
        }
        let mut arg = self.node.look_arg(w).map(|a| a.arg).unwrap_or_default();
        if arg.is_empty() {
            arg = self.node.selected_text(v).unwrap_or_default();
            let _ = self.node.set_look_arg(&mut self.log, w, &arg);
        }
        if arg.is_empty() {
            return;
        }
        let (q0, q1) = self.node.selection(v).unwrap_or((0, 0));
        let from = if reverse { (q0, q0) } else { (q1, q1) };
        let found = self.look_from(w, &arg, from, reverse);
        if let Some(l) = self.looking.as_mut().filter(|l| l.window == w) {
            l.failed = !found;
            l.anchor = self.node.selection(v).unwrap_or(l.anchor);
        }
        // the pointer onto what was found, as B3 takes it
        if found {
            self.node.warp = Some(apex_core::tiling::Warp::Sel(v));
        }
        self.after();
        cx.notify();
    }

    /// Where window `w`'s Look's word is in its body, to be washed: every
    /// place, while a look goes on there (live, or the selection one of
    /// them); else none. Found once a version and word.
    pub fn look_marks(&mut self, w: WindowId) -> Rc<Vec<(usize, usize)>> {
        let none = Rc::new(Vec::new());
        let Some(arg) = self.node.look_arg(w).map(|a| a.arg).filter(|a| !a.is_empty()) else { return none };
        let Some(buf) = self.node.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| self.node.state.buffer(b).ok()) else { return none };
        let marks = match self.look_cache.get(&w) {
            Some((a, v, m)) if *a == arg && *v == buf.version => m.clone(),
            _ => {
                let m = Rc::new(find_all(&buf.text.to_string(), &arg));
                self.look_cache.insert(w, (arg, buf.version, m.clone()));
                m
            }
        };
        let sel = self.node.selection(ViewId::Body(w)).unwrap_or((0, 0));
        let live = self.looking.as_ref().is_some_and(|l| l.window == w);
        if live || marks.binary_search(&sel).is_ok() {
            marks
        } else {
            none
        }
    }

    /// Window `w`'s Look's argument, struck through: a live look that
    /// found nothing.
    pub fn look_strike(&self, w: WindowId) -> Option<(usize, usize)> {
        self.looking.as_ref().filter(|l| l.window == w && l.failed && !l.arg.is_empty())?;
        self.node.look_arg(w).map(|a| (a.start, a.end))
    }
}
