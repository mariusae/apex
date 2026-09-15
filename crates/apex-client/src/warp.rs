//! Moving the mouse, as acme does after layout changes (`moveto`). macOS
//! delivers no event for a warp, so the app remembers where it put the
//! pointer until the next real mouse event.

use gpui::{Pixels, Point, Window};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWarpMouseCursorPosition(p: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
}

/// Is a mouse button down right now (a click is what activated us)?
pub fn button_down() -> bool {
    // kCGEventSourceStateCombinedSessionState = 0; left, right, centre
    unsafe { (0..3).any(|b| CGEventSourceButtonState(0, b)) }
}
/// Put the pointer at `p`, a point in this window's coordinates.
pub fn move_to(window: &Window, p: Point<Pixels>) {
    let p = screen_point(window, p);
    // SAFETY: plain CoreGraphics calls with a by-value point.
    unsafe {
        CGWarpMouseCursorPosition(p);
        // a warp otherwise suppresses mouse movement for a moment
        CGAssociateMouseAndMouseCursorPosition(true);
    }
    // AppKit hides the pointer while keys are typed, until the mouse
    // moves; a warp is not a move, so a pointer put somewhere from the
    // keyboard (ctrl-tab, a Goto) would sit there unseen
    show();
}

#[cfg(target_os = "macos")]
fn screen_point(window: &Window, p: Point<Pixels>) -> CGPoint {
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let fallback = || {
        let b = window.bounds();
        CGPoint {
            x: f32::from(b.origin.x + p.x) as f64,
            y: f32::from(b.origin.y + p.y) as f64,
        }
    };

    let Ok(h) = HasWindowHandle::window_handle(window) else {
        return fallback();
    };
    let RawWindowHandle::AppKit(h) = h.as_raw() else {
        return fallback();
    };
    let view = h.ns_view.as_ptr() as *mut objc::runtime::Object;

    // SAFETY: gpui gives us its live NSView. These are AppKit geometry queries
    // on the main thread. AppKit screen coordinates are y-up; CoreGraphics
    // cursor warps use y-down global display coordinates.
    unsafe {
        let ns_window: *mut objc::runtime::Object = msg_send![view, window];
        if ns_window.is_null() {
            return fallback();
        }
        let screens: *mut objc::runtime::Object = msg_send![class!(NSScreen), screens];
        let main_screen: *mut objc::runtime::Object = msg_send![screens, objectAtIndex: 0usize];
        if main_screen.is_null() {
            return fallback();
        }
        let view_frame: CGRect = msg_send![view, frame];
        let in_window = CGPoint {
            x: view_frame.origin.x + f32::from(p.x) as f64,
            y: view_frame.origin.y + view_frame.size.height - f32::from(p.y) as f64,
        };
        let screen: CGPoint = msg_send![ns_window, convertPointToScreen: in_window];
        let main_frame: CGRect = msg_send![main_screen, frame];
        CGPoint {
            x: screen.x,
            y: main_frame.origin.y + main_frame.size.height - screen.y,
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn screen_point(window: &Window, p: Point<Pixels>) -> CGPoint {
    let b = window.bounds();
    CGPoint {
        x: f32::from(b.origin.x + p.x) as f64,
        y: f32::from(b.origin.y + p.y) as f64,
    }
}

/// The pointer shown, where it is: AppKit's typing-hides-it undone.
pub fn show() {
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: a class method on NSCursor, on the main thread.
    unsafe {
        let _: () = msg_send![class!(NSCursor), setHiddenUntilMouseMoves: false];
    }
}
