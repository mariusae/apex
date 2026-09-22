//! The notifications the app is carrying, across every session it has
//! open -- shown in a window or parked in a tab -- and what the Dock
//! says about them: its icon bounces when one comes while no window of
//! the app's is in front, and carries how many are waiting.
//!
//! A session's own notifications are in the order they were raised, but
//! the entry that raised each (`Notification::at`) is a sequence in that
//! session's metalog and means nothing beside another session's. So the
//! order across sessions is the order the app first saw them, kept here
//! as a queue: what ⌘G walks.

use std::collections::HashSet;

use gpui::{App, Global};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

use apex_core::{Seq, WindowId};
use apex_server::providers::SessionUrl;

use crate::app::Acme;
use crate::pool::Pool;

/// A notification the app is carrying: the session it is in, the window
/// it is about, and the entry that raised it.
pub type Note = (SessionUrl, WindowId, Seq);

/// The notifications seen so far, oldest first.
#[derive(Default)]
struct Queue(Vec<Note>);

impl Global for Queue {}

/// Every tick: what each session has now, against what it had. One that
/// has come since, with no window of the app's in front, bounces the
/// Dock icon once; and the icon carries how many are waiting. A session
/// seen for the first time brings no bounce for what it already had.
pub fn tick(cx: &mut App) {
    let mut active = false;
    let mut now = Pool::notifications(cx);
    for h in cx.windows().into_iter().filter_map(|w| w.downcast::<Acme>()) {
        if let Ok(a) = h.read(cx) {
            active |= a.app_active();
            now.push((a.url.clone(), a.node.notifications().map(|n| (n.window, n.at)).collect()));
        }
    }
    let looked: HashSet<SessionUrl> = cx.default_global::<Looked>().0.clone();
    cx.default_global::<Looked>().0 = now.iter().map(|(u, _)| u.clone()).collect();
    let queue = cx.default_global::<Queue>();
    let came = advance(&mut queue.0, &looked, &now);
    let waiting = queue.0.len();
    badge(waiting);
    if came && !active {
        bounce();
    }
}

/// The queue against what the sessions hold now: what has gone is
/// dropped, what has come is put at the end -- each session's in its own
/// order, since only the app can say how two sessions' compare. True if
/// any of what came is news, which a session the app is seeing for the
/// first time (not in `looked`) never is: it brought its own along.
fn advance(queue: &mut Vec<Note>, looked: &HashSet<SessionUrl>, now: &[(SessionUrl, Vec<(WindowId, Seq)>)]) -> bool {
    let here: HashSet<(SessionUrl, Seq)> = now.iter().flat_map(|(u, ns)| ns.iter().map(|(_, at)| (u.clone(), *at))).collect();
    let known: HashSet<(SessionUrl, Seq)> = queue.iter().map(|(u, _, at)| (u.clone(), *at)).collect();
    queue.retain(|(u, _, at)| here.contains(&(u.clone(), *at)));
    let mut came = false;
    for (u, ns) in now {
        for (w, at) in ns {
            if !known.contains(&(u.clone(), *at)) {
                queue.push((u.clone(), *w, *at));
                came |= looked.contains(u);
            }
        }
    }
    came
}

/// The sessions the app has looked at: what one already had when it was
/// first seen is no news.
#[derive(Default)]
struct Looked(HashSet<SessionUrl>);

impl Global for Looked {}

/// The notifications waiting, oldest first.
pub fn queue(cx: &App) -> Vec<Note> {
    cx.try_global::<Queue>().map(|q| q.0.clone()).unwrap_or_default()
}

/// `-[NSApplication requestUserAttention:]`, informational: one bounce.
fn bounce() {
    const NS_INFORMATIONAL_REQUEST: isize = 10;
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: isize = msg_send![app, requestUserAttention: NS_INFORMATIONAL_REQUEST];
    }
}

/// How many are waiting, on the Dock icon; nothing while none is.
fn badge(n: usize) {
    use objc::runtime::{BOOL, YES};
    // SAFETY: AppKit on the main thread; the string is autoreleased.
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let tile: *mut Object = msg_send![app, dockTile];
        if tile.is_null() {
            return;
        }
        let label: *mut Object = if n == 0 {
            std::ptr::null_mut()
        } else {
            let s = std::ffi::CString::new(n.to_string()).unwrap_or_else(|_| std::ffi::CString::new("!").unwrap());
            msg_send![class!(NSString), stringWithUTF8String: s.as_ptr()]
        };
        let was: *mut Object = msg_send![tile, badgeLabel];
        // the same label again would redraw the icon for nothing
        let same: bool = if was.is_null() || label.is_null() {
            was.is_null() && label.is_null()
        } else {
            let eq: BOOL = msg_send![was, isEqualToString: label];
            eq == YES
        };
        if same {
            return;
        }
        let _: () = msg_send![tile, setBadgeLabel: label];
    }
}

/// The beep a Mac makes when there is nowhere to go (`NSBeep`).
pub fn beep() {
    // SAFETY: a plain AppKit function of no arguments.
    unsafe { NSBeep() }
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    fn NSBeep();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(name: &str) -> SessionUrl {
        SessionUrl::local(name).with_id(name)
    }

    #[test]
    fn the_queue_keeps_them_in_the_order_they_came() {
        let (a, b) = (url("main"), url("work"));
        let mut q: Vec<Note> = Vec::new();
        let mut looked: HashSet<SessionUrl> = HashSet::new();
        // the first look at a session: what it has is queued, and is no news
        let came = advance(&mut q, &looked, &[(a.clone(), vec![(WindowId(1), 7)])]);
        assert!(!came, "a session first seen brings no news");
        assert_eq!(q, vec![(a.clone(), WindowId(1), 7)]);
        looked.insert(a.clone());
        // another in the same session, and one in a session also new
        let came = advance(&mut q, &looked, &[(a.clone(), vec![(WindowId(1), 7), (WindowId(2), 9)]), (b.clone(), vec![(WindowId(5), 2)])]);
        assert!(came, "the one in the session we had is news");
        assert_eq!(q, vec![(a.clone(), WindowId(1), 7), (a.clone(), WindowId(2), 9), (b.clone(), WindowId(5), 2)]);
        looked.insert(b.clone());
        // the oldest is taken: it goes, the rest keep their order
        let came = advance(&mut q, &looked, &[(a.clone(), vec![(WindowId(2), 9)]), (b.clone(), vec![(WindowId(5), 2)])]);
        assert!(!came);
        assert_eq!(q, vec![(a.clone(), WindowId(2), 9), (b.clone(), WindowId(5), 2)]);
        // one raised again after being lowered is another notification,
        // and goes to the end
        let came = advance(&mut q, &looked, &[(a.clone(), vec![(WindowId(2), 9), (WindowId(1), 11)]), (b.clone(), vec![(WindowId(5), 2)])]);
        assert!(came);
        assert_eq!(q, vec![(a.clone(), WindowId(2), 9), (b.clone(), WindowId(5), 2), (a.clone(), WindowId(1), 11)]);
        // and with none left, nothing is queued
        let came = advance(&mut q, &looked, &[(a.clone(), vec![]), (b.clone(), vec![])]);
        assert!(!came);
        assert!(q.is_empty());
    }
}
