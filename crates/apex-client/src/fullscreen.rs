//! Apex's own full screen (⌃⌘F, View ▸ Enter Full Screen): the window
//! over the whole of its screen where it is, the menu bar and the Dock
//! hidden until the pointer goes to them -- gpui's simple full screen,
//! borderless, in no Space of its own -- and the title bar up in the
//! menu bar's place. On a screen with a camera's notch the bar is the
//! band beside it, split round it (`Band`). The window's green button is
//! still the Mac's own full screen, in a Space of its own.

use gpui::{Bounds, Pixels, Window};

/// The top of a screen apex's bar takes in full screen: its height, and
/// where a notch is across it (the window's coordinates, left to right),
/// nothing to be drawn between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub h: f32,
    pub notch: Option<(f32, f32)>,
}

/// The band at the top of the screen `window` is on, when it has a
/// notch: the height the system keeps clear of it (`safeAreaInsets`), and
/// the notch between the two areas either side of it. None on a screen
/// without one, where the bar is as tall as ever, at the top edge.
#[cfg(target_os = "macos")]
pub fn band(window: &Window) -> Option<Band> {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Insets {
        top: f64,
        left: f64,
        bottom: f64,
        right: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
    let view = h.ns_view.as_ptr() as *mut Object;
    // SAFETY: gpui's view and its window, alive while the window is; the
    // screen's geometry read on the main thread
    unsafe {
        let w: *mut Object = msg_send![view, window];
        if w.is_null() {
            return None;
        }
        let screen: *mut Object = msg_send![w, screen];
        if screen.is_null() {
            return None;
        }
        let responds: bool = msg_send![screen, respondsToSelector: sel!(safeAreaInsets)];
        if !responds {
            return None;
        }
        let inset: Insets = msg_send![screen, safeAreaInsets];
        if inset.top <= 0. {
            return None;
        }
        let frame: Rect = msg_send![screen, frame];
        let left: Rect = msg_send![screen, auxiliaryTopLeftArea];
        let right: Rect = msg_send![screen, auxiliaryTopRightArea];
        // the notch between the area left of it and the one right of it,
        // by their widths (so by the screen's own left edge)
        let notch = (left.w > 0. && right.w > 0.).then_some((left.w as f32, (frame.w - right.w) as f32));
        Some(Band { h: inset.top as f32, notch })
    }
}

#[cfg(not(target_os = "macos"))]
pub fn band(_window: &Window) -> Option<Band> {
    None
}

/// NSApplicationPresentationAutoHideDock | AutoHideMenuBar: what gpui
/// asks for in its simple full screen, the bar and the Dock shown when the
/// pointer comes to them -- to the bar's whole height, where apex's bar
/// is in full screen.
#[cfg(target_os = "macos")]
const AUTO: usize = 1 | 4;
/// NSApplicationPresentationHideDock | HideMenuBar: neither shown at all
/// (the bar hidden needs the Dock hidden).
#[cfg(target_os = "macos")]
const HIDE: usize = 2 | 8;

/// Whether the bar and the Dock are hidden outright now (`HIDE`), not
/// shown as the pointer comes (`AUTO`).
#[cfg(target_os = "macos")]
static HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app's presentation options with the bar and the Dock as `hidden`
/// says, the rest as they are -- never two of the four at odds, which
/// AppKit refuses with an exception. `again` sets them even when they are
/// already so, cleared and set, since the same options set over
/// themselves do nothing.
#[cfg(target_os = "macos")]
fn set_bars(hidden: bool, again: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: the shared application's presentation options, on the main
    // thread
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let now: usize = msg_send![app, presentationOptions];
        let rest = now & !(AUTO | HIDE);
        let want = rest | if hidden { HIDE } else { AUTO };
        if want == now && !again {
            return;
        }
        if again {
            let _: () = msg_send![app, setPresentationOptions: rest];
        }
        let _: () = msg_send![app, setPresentationOptions: want];
    }
    HIDDEN.store(hidden, std::sync::atomic::Ordering::Relaxed);
}

/// The menu bar and the Dock hidden again, the app active with a window
/// in its full screen: as they were last set, said again, for macOS does
/// not always apply them when the app comes back (a window manager
/// bringing its window forward).
#[cfg(target_os = "macos")]
pub fn hide_bars_again() {
    set_bars(HIDDEN.load(std::sync::atomic::Ordering::Relaxed), true);
}

#[cfg(not(target_os = "macos"))]
pub fn hide_bars_again() {}

/// In apex's full screen, with apex in front: the menu bar shown only for
/// the pointer at the very top of the screen, not anywhere in the bar's
/// height -- apex's own bar is there (the session's chevron, the tabs),
/// and macOS's auto-hiding shows its bar for the pointer anywhere over
/// it, taking the click. So the bar and the Dock are hidden outright, and
/// handed back to macOS (`AUTO`, which shows them at once) when the
/// pointer is at the screen's top edge or the Dock's, and hidden again
/// once it is clear of both, no menu of apex's open. Off the window's
/// screen, macOS has them. Called on the UI's timer.
#[cfg(target_os = "macos")]
pub fn bars_tick(window: &Window, band: Option<Band>, active: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct P {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    let hidden = HIDDEN.load(std::sync::atomic::Ordering::Relaxed);
    if !window.is_simple_fullscreen() || !active {
        // out of it, gpui put the options back as they were; away, they
        // are not ours to keep
        if !window.is_simple_fullscreen() {
            HIDDEN.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        return;
    }
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    let view = h.ns_view.as_ptr() as *mut Object;
    // SAFETY: gpui's view and its window, alive while the window is; the
    // pointer, the screen and the main menu read on the main thread
    let (at, screen, menu_open, bar) = unsafe {
        let w: *mut Object = msg_send![view, window];
        if w.is_null() {
            return;
        }
        let screen: *mut Object = msg_send![w, screen];
        if screen.is_null() {
            return;
        }
        let frame: Rect = msg_send![screen, frame];
        let at: P = msg_send![class!(NSEvent), mouseLocation];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let menu: *mut Object = msg_send![app, mainMenu];
        let (open, bar) = if menu.is_null() {
            (false, 24.)
        } else {
            let item: *mut Object = msg_send![menu, highlightedItem];
            let bar: f64 = msg_send![menu, menuBarHeight];
            (!item.is_null(), bar)
        };
        (at, frame, open, bar)
    };
    let on_screen = at.x >= screen.x && at.x < screen.x + screen.w && at.y >= screen.y && at.y <= screen.y + screen.h;
    if !on_screen {
        if hidden {
            set_bars(false, false);
        }
        return;
    }
    let (dock_side, dock_reach) = dock();
    // how far the pointer is from the top edge, and from the Dock's
    let from_top = screen.y + screen.h - at.y;
    let from_dock = match dock_side {
        DockSide::Left => at.x - screen.x,
        DockSide::Right => screen.x + screen.w - at.x,
        DockSide::Bottom => at.y - screen.y,
    };
    let bar = (band.map_or(0., |b| b.h as f64)).max(bar).max(24.);
    if hidden {
        if from_top <= 1.5 || from_dock <= 1.5 {
            set_bars(false, false);
        }
    } else if from_top > bar + 8. && from_dock > dock_reach && !menu_open {
        set_bars(true, false);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn bars_tick(_window: &Window, _band: Option<Band>, _active: bool) {}

#[cfg(target_os = "macos")]
enum DockSide {
    Left,
    Right,
    Bottom,
}

/// The Dock's side of the screen, and how far from it the pointer is
/// still on the Dock shown (its tiles, magnified where they are, and some
/// room), as the Dock's own settings say.
#[cfg(target_os = "macos")]
fn dock() -> (DockSide, f64) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: the Dock's defaults, read; Foundation objects alive in the
    // shared defaults while this reads them
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let name: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c"com.apple.dock".as_ptr()];
        let d: *mut Object = msg_send![defaults, persistentDomainForName: name];
        if d.is_null() {
            return (DockSide::Bottom, 96.);
        }
        let get = |k: &std::ffi::CStr| -> *mut Object {
            let key: *mut Object = msg_send![class!(NSString), stringWithUTF8String: k.as_ptr()];
            msg_send![d, objectForKey: key]
        };
        let num = |o: *mut Object| -> Option<f64> {
            if o.is_null() {
                return None;
            }
            let ok: bool = msg_send![o, respondsToSelector: sel!(doubleValue)];
            ok.then(|| msg_send![o, doubleValue])
        };
        let side = {
            let o = get(c"orientation");
            let s: *const std::os::raw::c_char = if o.is_null() { std::ptr::null() } else { msg_send![o, UTF8String] };
            match (!s.is_null()).then(|| std::ffi::CStr::from_ptr(s).to_bytes()) {
                Some(b"left") => DockSide::Left,
                Some(b"right") => DockSide::Right,
                _ => DockSide::Bottom,
            }
        };
        let tile = num(get(c"tilesize")).unwrap_or(48.);
        let magnified = num(get(c"magnification")).unwrap_or(0.) != 0.;
        let large = if magnified { num(get(c"largesize")).unwrap_or(tile) } else { tile };
        (side, tile.max(large) * 1.5 + 24.)
    }
}

/// ⌃⌘F: into apex's full screen, the window's frame kept to come back
/// to (`restore`), or out of it; out of the Mac's own, from its green
/// button, too.
pub fn toggle(acme: &mut crate::app::Acme, window: &mut Window) {
    if window.is_fullscreen() {
        window.toggle_fullscreen();
        return;
    }
    if !window.is_simple_fullscreen() {
        acme.full_restore = Some(window.bounds());
    }
    window.toggle_simple_fullscreen();
}

/// The frame to remember a window by: in apex's full screen, the one it
/// had before (the screen's is no place to come back to).
pub fn restore(acme: &crate::app::Acme, window: &Window) -> Option<Bounds<Pixels>> {
    window.is_simple_fullscreen().then_some(acme.full_restore).flatten()
}
