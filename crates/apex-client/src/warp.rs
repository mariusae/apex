//! Moving the mouse, as acme does after layout changes (`moveto`). macOS
//! delivers no event for a warp, so the app remembers where it put the
//! pointer until the next real mouse event.

use gpui::{Pixels, Point, Window};

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWarpMouseCursorPosition(p: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
}

/// Where the pointer really is, in this window's coordinates (it may be
/// outside the window).
pub fn position_in(window: &Window) -> Option<Point<Pixels>> {
    // SAFETY: an event created from no source describes the current
    // pointer; we release it ourselves.
    let p = unsafe {
        let ev = CGEventCreate(std::ptr::null());
        if ev.is_null() {
            return None;
        }
        let p = CGEventGetLocation(ev);
        CFRelease(ev);
        p
    };
    let b = window.bounds();
    Some(Point { x: gpui::px(p.x as f32 - f32::from(b.origin.x)), y: gpui::px(p.y as f32 - f32::from(b.origin.y)) })
}

/// Put the pointer at `p`, a point in this window's coordinates.
pub fn move_to(window: &Window, p: Point<Pixels>) {
    let b = window.bounds();
    let x = f32::from(b.origin.x + p.x) as f64;
    let y = f32::from(b.origin.y + p.y) as f64;
    // SAFETY: plain CoreGraphics calls with a by-value point.
    unsafe {
        CGWarpMouseCursorPosition(CGPoint { x, y });
        // a warp otherwise suppresses mouse movement for a moment
        CGAssociateMouseAndMouseCursorPosition(true);
    }
}
