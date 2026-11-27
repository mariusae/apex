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

/// The menu bar and the Dock hidden again, the app active with a window
/// in its full screen: what gpui asked for on the way in, said again --
/// cleared and set, since the same options set over themselves do
/// nothing -- for macOS does not always apply them when the app comes
/// back (a window manager bringing its window forward).
#[cfg(target_os = "macos")]
pub fn hide_bars_again() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    /// NSApplicationPresentationAutoHideDock | AutoHideMenuBar
    const HIDE: usize = 1 | 4;
    // SAFETY: the shared application's presentation options, on the main
    // thread
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let now: usize = msg_send![app, presentationOptions];
        let _: () = msg_send![app, setPresentationOptions: now & !HIDE];
        let _: () = msg_send![app, setPresentationOptions: now | HIDE];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn hide_bars_again() {}

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
