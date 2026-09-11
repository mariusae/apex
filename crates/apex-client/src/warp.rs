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
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
}

/// Is a mouse button down right now (a click is what activated us)?
pub fn button_down() -> bool {
    // kCGEventSourceStateCombinedSessionState = 0; left, right, centre
    unsafe { (0..3).any(|b| CGEventSourceButtonState(0, b)) }
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
    // AppKit hides the pointer while keys are typed, until the mouse
    // moves; a warp is not a move, so a pointer put somewhere from the
    // keyboard (ctrl-tab, a Goto) would sit there unseen
    show();
}

/// The pointer shown, where it is: AppKit's typing-hides-it undone.
pub fn show() {
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: a class method on NSCursor, on the main thread.
    unsafe {
        let _: () = msg_send![class!(NSCursor), setHiddenUntilMouseMoves: false];
    }
}
