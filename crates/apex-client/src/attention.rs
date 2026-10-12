//! The notifications the app is carrying, across every session it has
//! open -- shown in a window or parked in a tab -- and what the Dock
//! says about them: its icon bounces when one comes while no window of
//! the app's is in front, and carries the latest, as the sidebar
//! button's badge does another session's: the symbol of the status that
//! asks for the user (done, blocked, failed), or a blue dot.
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

use crate::app::Acme;
use crate::pool::{Pool, TabId};

/// A notification the app is carrying: the tab it is in, the window it
/// is about, and the entry that raised it.
pub type Note = (TabId, WindowId, Seq);

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
            now.push((a.tab, a.node.notifications().map(|n| (n.window, n.at)).collect()));
        }
    }
    let looked: HashSet<TabId> = cx.default_global::<Looked>().0.clone();
    cx.default_global::<Looked>().0 = now.iter().map(|(id, _)| *id).collect();
    let queue = cx.default_global::<Queue>();
    let came = advance(&mut queue.0, &looked, &now);
    let latest = queue.0.last().copied();
    let mark = latest.map(|n| mark_of(cx, n));
    if cx.default_global::<Shown>().0 != Some(mark) {
        cx.default_global::<Shown>().0 = Some(mark);
        badge(mark);
    }
    if came && !active {
        bounce();
    }
}

/// What a notification shows: the state and kind of its window's status,
/// when that is what asks for the user; none, a notification saying no
/// more.
pub type Mark = Option<(apex_core::status::State, Option<apex_core::status::Kind>)>;

/// Notification `n`'s mark, from its session as it is now: shown in a
/// window, or parked.
pub fn mark_of(cx: &App, (tab, w, _): Note) -> Mark {
    let asks = |node: &apex_core::Node| node.window_status(w).filter(|s| crate::sidebar::asks(s.state)).map(|s| (s.state, s.kind));
    for h in cx.windows().into_iter().filter_map(|w| w.downcast::<Acme>()) {
        if let Ok(a) = h.read(cx) {
            if a.tab == tab {
                return asks(&a.node);
            }
        }
    }
    Pool::parked_nodes(cx).into_iter().find(|(id, _, _)| *id == tab).and_then(|(_, _, node)| asks(node))
}

/// The Dock icon's mark as last drawn: none before the first.
#[derive(Default)]
struct Shown(Option<Option<Mark>>);

impl Global for Shown {}

/// The queue against what the sessions hold now: what has gone is
/// dropped, what has come is put at the end -- each session's in its own
/// order, since only the app can say how two sessions' compare. True if
/// any of what came is news, which a session the app is seeing for the
/// first time (not in `looked`) never is: it brought its own along.
fn advance(queue: &mut Vec<Note>, looked: &HashSet<TabId>, now: &[(TabId, Vec<(WindowId, Seq)>)]) -> bool {
    let here: HashSet<(TabId, Seq)> = now.iter().flat_map(|(id, ns)| ns.iter().map(|(_, at)| (*id, *at))).collect();
    let known: HashSet<(TabId, Seq)> = queue.iter().map(|(id, _, at)| (*id, *at)).collect();
    queue.retain(|(id, _, at)| here.contains(&(*id, *at)));
    let mut came = false;
    for (id, ns) in now {
        for (w, at) in ns {
            if !known.contains(&(*id, *at)) {
                queue.push((*id, *w, *at));
                came |= looked.contains(id);
            }
        }
    }
    came
}

/// The tabs the app has looked at: what one already had when it was
/// first seen is no news.
#[derive(Default)]
struct Looked(HashSet<TabId>);

impl Global for Looked {}

/// The notifications waiting, oldest first.
pub fn queue(cx: &App) -> Vec<Note> {
    cx.try_global::<Queue>().map(|q| q.0.clone()).unwrap_or_default()
}

/// The space bunny, the sticker's own shape on nothing (`mac/space-bunny-1024.png`).
const ICON: &[u8] = include_bytes!("../../../mac/space-bunny-1024.png");

/// The Dock's icon, set as the app runs: the sticker's own outline, with
/// no square round it. (The bundle's icon, shown while it does not run,
/// is the sticker on a navy square, `mac/apex.icon`: since macOS 26 a
/// bundle's icon is a rounded square whatever it is drawn as.)
pub fn dock_icon() {
    // SAFETY: AppKit on the main thread, at launch; the data is copied.
    unsafe {
        let data: *mut Object = msg_send![class!(NSData), dataWithBytes: ICON.as_ptr() length: ICON.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}

/// `-[NSApplication requestUserAttention:]`, informational: one bounce.
fn bounce() {
    const NS_INFORMATIONAL_REQUEST: isize = 10;
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: isize = msg_send![app, requestUserAttention: NS_INFORMATIONAL_REQUEST];
    }
}

/// The SF Symbol a status's mark is drawn with on the Dock icon, on its
/// disc: as the app's own icons say it in a tag (`status_icon`).
fn symbol(state: apex_core::status::State, kind: Option<apex_core::status::Kind>) -> &'static str {
    use apex_core::status::{Kind, State};
    match (state, kind) {
        (State::Done, _) => "checkmark",
        (State::Blocked, Some(Kind::Question)) => "questionmark",
        (State::Blocked, Some(Kind::Auth)) => "lock.fill",
        (State::Error, _) => "exclamationmark",
        _ => "hand.raised.fill",
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    origin: Point,
    size: Point,
}

/// The latest notification on the Dock icon (`None`: none waiting, the
/// icon as it is): over the bunny's top right, on a disc ringed in white,
/// a blue dot -- or, its status asking for the user, its symbol, white
/// on the stale gold.
fn badge(mark: Option<Mark>) {
    let Some(mark) = mark else {
        dock_icon();
        return;
    };
    let t = crate::theme::theme();
    let img = marked(mark, t.accent, t.stale);
    if img.is_null() {
        return;
    }
    // SAFETY: AppKit on the main thread; the image is ours to release
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: img];
        let _: () = msg_send![img, release];
        // and no count over it, as there was
        let tile: *mut Object = msg_send![app, dockTile];
        if !tile.is_null() {
            let nil: *mut Object = std::ptr::null_mut();
            let _: () = msg_send![tile, setBadgeLabel: nil];
        }
    }
}

/// The Dock icon with `mark` over it (`badge`), in the theme's `accent`
/// and `stale`: an image of the caller's to release, or nil.
fn marked(mark: Mark, accent: u32, stale: u32) -> *mut Object {
    let color = |rgb: u32| -> *mut Object {
        let c = |s: u32| f64::from((rgb >> s) & 0xff) / 255.;
        // SAFETY: a plain AppKit class method; the colour is autoreleased
        unsafe { msg_send![class!(NSColor), colorWithSRGBRed: c(16) green: c(8) blue: c(0) alpha: 1.0f64] }
    };
    const SIDE: f64 = 1024.;
    // SAFETY: AppKit on the main thread, drawing into an image of our
    // own; everything made here is autoreleased or released.
    unsafe {
        let data: *mut Object = msg_send![class!(NSData), dataWithBytes: ICON.as_ptr() length: ICON.len()];
        let base: *mut Object = msg_send![class!(NSImage), alloc];
        let base: *mut Object = msg_send![base, initWithData: data];
        if base.is_null() {
            return std::ptr::null_mut();
        }
        let img: *mut Object = msg_send![class!(NSImage), alloc];
        let img: *mut Object = msg_send![img, initWithSize: Point { x: SIDE, y: SIDE }];
        let _: () = msg_send![img, lockFocus];
        let whole = Rect { origin: Point { x: 0., y: 0. }, size: Point { x: SIDE, y: SIDE } };
        let zero = Rect { origin: Point { x: 0., y: 0. }, size: Point { x: 0., y: 0. } };
        // NSCompositingOperationSourceOver
        let _: () = msg_send![base, drawInRect: whole fromRect: zero operation: 2usize fraction: 1.0f64];
        let disc = |d: f64, inset: f64, ink: *mut Object| {
            let r = Rect { origin: Point { x: SIDE - d - inset, y: SIDE - d - inset }, size: Point { x: d, y: d } };
            let path: *mut Object = msg_send![class!(NSBezierPath), bezierPathWithOvalInRect: r];
            let _: () = msg_send![ink, set];
            let _: () = msg_send![path, fill];
            r
        };
        let white = color(0xFFFFFF);
        match mark {
            None => {
                disc(300., 40., white);
                disc(252., 64., color(accent));
            }
            Some((state, kind)) => {
                disc(400., 16., white);
                let r = disc(352., 40., color(stale));
                let name = std::ffi::CString::new(symbol(state, kind)).unwrap();
                let name: *mut Object = msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
                let nil: *mut Object = std::ptr::null_mut();
                let sym: *mut Object = msg_send![class!(NSImage), imageWithSystemSymbolName: name accessibilityDescription: nil];
                if !sym.is_null() {
                    // NSFontWeightBold, in white
                    let size: *mut Object = msg_send![class!(NSImageSymbolConfiguration), configurationWithPointSize: 190.0f64 weight: 0.4f64];
                    let colors: *mut Object = msg_send![class!(NSArray), arrayWithObject: white];
                    let paint: *mut Object = msg_send![class!(NSImageSymbolConfiguration), configurationWithPaletteColors: colors];
                    let conf: *mut Object = msg_send![size, configurationByApplyingConfiguration: paint];
                    let sym: *mut Object = msg_send![sym, imageWithSymbolConfiguration: conf];
                    if !sym.is_null() {
                        let s: Point = msg_send![sym, size];
                        let at = Rect { origin: Point { x: r.origin.x + (r.size.x - s.x) / 2., y: r.origin.y + (r.size.y - s.y) / 2. }, size: s };
                        let _: () = msg_send![sym, drawInRect: at fromRect: zero operation: 2usize fraction: 1.0f64];
                    }
                }
            }
        }
        let _: () = msg_send![img, unlockFocus];
        let _: () = msg_send![base, release];
        img
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

    fn tab(n: u64) -> TabId {
        TabId(n)
    }

    /// The Dock icon with each mark, written as PNGs to $APEX_DOCK_OUT,
    /// to be looked at.
    #[test]
    #[ignore]
    fn the_dock_icons_marks_drawn() {
        use apex_core::status::{Kind, State};
        let Ok(out) = std::env::var("APEX_DOCK_OUT") else { return };
        let marks: [(&str, Mark); 6] = [("dot", None), ("done", Some((State::Done, None))), ("hand", Some((State::Blocked, Some(Kind::Permission)))), ("question", Some((State::Blocked, Some(Kind::Question)))), ("lock", Some((State::Blocked, Some(Kind::Auth)))), ("error", Some((State::Error, None)))];
        for (name, m) in marks {
            let img = marked(m, 0x007D9C, 0xC79A00);
            assert!(!img.is_null());
            // SAFETY: AppKit, our image
            unsafe {
                let tiff: *mut Object = msg_send![img, TIFFRepresentation];
                let rep: *mut Object = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
                let props: *mut Object = msg_send![class!(NSDictionary), dictionary];
                // NSBitmapImageFileTypePNG
                let png: *mut Object = msg_send![rep, representationUsingType: 4usize properties: props];
                let len: usize = msg_send![png, length];
                let bytes: *const u8 = msg_send![png, bytes];
                std::fs::write(format!("{out}/{name}.png"), std::slice::from_raw_parts(bytes, len)).unwrap();
            }
        }
    }

    #[test]
    fn the_queue_keeps_them_in_the_order_they_came() {
        let (a, b) = (tab(1), tab(2));
        let mut q: Vec<Note> = Vec::new();
        let mut looked: HashSet<TabId> = HashSet::new();
        // the first look at a session: what it has is queued, and is no news
        let came = advance(&mut q, &looked, &[(a, vec![(WindowId(1), 7)])]);
        assert!(!came, "a session first seen brings no news");
        assert_eq!(q, vec![(a, WindowId(1), 7)]);
        looked.insert(a);
        // another in the same session, and one in a session also new
        let came = advance(&mut q, &looked, &[(a, vec![(WindowId(1), 7), (WindowId(2), 9)]), (b, vec![(WindowId(5), 2)])]);
        assert!(came, "the one in the session we had is news");
        assert_eq!(q, vec![(a, WindowId(1), 7), (a, WindowId(2), 9), (b, WindowId(5), 2)]);
        looked.insert(b);
        // the oldest is taken: it goes, the rest keep their order
        let came = advance(&mut q, &looked, &[(a, vec![(WindowId(2), 9)]), (b, vec![(WindowId(5), 2)])]);
        assert!(!came);
        assert_eq!(q, vec![(a, WindowId(2), 9), (b, WindowId(5), 2)]);
        // one raised again after being lowered is another notification,
        // and goes to the end
        let came = advance(&mut q, &looked, &[(a, vec![(WindowId(2), 9), (WindowId(1), 11)]), (b, vec![(WindowId(5), 2)])]);
        assert!(came);
        assert_eq!(q, vec![(a, WindowId(2), 9), (b, WindowId(5), 2), (a, WindowId(1), 11)]);
        // and with none left, nothing is queued
        let came = advance(&mut q, &looked, &[(a, vec![]), (b, vec![])]);
        assert!(!came);
        assert!(q.is_empty());
    }
}
