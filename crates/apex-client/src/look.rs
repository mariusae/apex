//! Looking as you type, acme's way: the query is the first `Look`'s
//! argument in a window's tag, the result the window's selection. While
//! the caret is in that argument, every change to it looks again from
//! where the selection was when the typing began (its anchor): a letter
//! more narrows to the same place or one further on, a letter less goes
//! back; empty, the selection is where it began; nothing found, it stays,
//! and the argument is struck through. Escape ends it, the selection a
//! caret at its end and the pointer there, to edit where it found (while
//! typing, the pointer stays: the keys go where it is). ⌘F takes the
//! caret (and the pointer, as acme's moves) to the argument; ⌘G and ⌘⇧G
//! look for it again forwards and back, the pointer on what they find as
//! B3's is. Every place the word is in the window is
//! washed faintly while a look goes on there -- while the argument is
//! live, or the selection is one of them (after ⌘G, or B3, which puts
//! its word in the argument too); once the selection is elsewhere, the
//! marks are gone.

use std::rc::Rc;

use gpui::{Context, Window};

use apex_core::text::{find_all, find_match};
use apex_core::*;

use crate::app::Acme;

/// A look going on in a window's tag: where the body's selection was when
/// the typing began, what was last looked for, and whether it was found.
pub struct Live {
    pub window: WindowId,
    pub anchor: (usize, usize),
    pub arg: String,
    pub failed: bool,
}

impl Acme {
    /// A key in window `w`'s tag: the caret in its Look's argument, the
    /// argument changed, looked for again from the anchor; out of it, the
    /// look over.
    pub fn live_look(&mut self, w: WindowId) {
        let Some((start, end, arg)) = self.node.look_arg(w) else {
            self.looking = None;
            return;
        };
        let caret = self.node.selection(ViewId::Tag(w)).unwrap_or((usize::MAX, 0));
        if caret.0 != caret.1 || caret.0 < start || caret.0 > end {
            if self.looking.as_ref().is_some_and(|l| l.window == w) {
                self.looking = None;
            }
            return;
        }
        let anchor = match self.looking.as_ref().filter(|l| l.window == w) {
            Some(l) if l.arg == arg => return, // the caret moved, the word did not
            Some(l) => l.anchor,
            None => self.node.selection(ViewId::Body(w)).unwrap_or((0, 0)),
        };
        let failed = !self.look_from(w, &arg, anchor, false);
        self.looking = Some(Live { window: w, anchor, arg, failed });
    }

    /// Escape in the argument: the look over, the selection made a caret
    /// at its end and the pointer taken there, as B3's is -- the keys
    /// following it into the text, to edit where the look found.
    pub fn look_done(&mut self, w: WindowId) {
        self.looking = None;
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
    /// on (`Look ` typed first when its tag has none), the argument
    /// selected to type over, and the pointer on it.
    pub fn find_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.window_at_pointer(window) else { return };
        if self.node.look_arg(w).is_none() {
            let _ = self.node.select(&mut self.log, ViewId::Tag(w), 0, 0);
            let _ = self.node.insert(&mut self.log, ViewId::Tag(w), "Look ");
        }
        let Some((start, end, arg)) = self.node.look_arg(w) else { return };
        let _ = self.node.select(&mut self.log, ViewId::Tag(w), start, end);
        self.node.warp = Some(apex_core::tiling::Warp::Sel(ViewId::Tag(w)));
        let anchor = self.node.selection(ViewId::Body(w)).unwrap_or((0, 0));
        self.looking = Some(Live { window: w, anchor, arg, failed: false });
        self.after();
        cx.notify();
    }

    /// ⌘G (or ⌘⇧G, `reverse`): the Look's argument looked for again in
    /// the window acme would act on, past the selection (or before it).
    /// With none, the selection is looked for, and is the argument then.
    pub fn find_next(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(w) = self.window_at_pointer(window) else { return };
        let v = ViewId::Body(w);
        let mut arg = self.node.look_arg(w).map(|a| a.2).unwrap_or_default();
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
        let Some(arg) = self.node.look_arg(w).map(|a| a.2).filter(|a| !a.is_empty()) else { return none };
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
        self.node.look_arg(w).map(|(a, b, _)| (a, b))
    }
}
