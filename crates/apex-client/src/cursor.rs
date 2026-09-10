//! acme's mouse pointers: plan9port's big arrow as the default, and the
//! box cursor while a layout box is being dragged (`coldragwin`,
//! `rowdragcol`). The bitmaps are the ones in plan9port (`bigarrow.h`,
//! `acme.c`), in Plan 9's cursor format: a `clr` plane of white pixels and
//! a `set` plane of black ones, 16×16, plus a 32×32 pair for high
//! resolution.
//!
//! gpui chooses among the system cursors by name, so the two we need are
//! put behind two styles nothing else here asks for: `NSCursor`'s
//! `dragLinkCursor` class method returns the big arrow and
//! `dragCopyCursor` the box. The acme area asks for the first (the
//! second while a layout box is held); the title bar, the session
//! selector and the menus keep the system arrow.

use std::sync::Once;

use objc::declare::ClassDecl;
use objc::runtime::{Class, Imp, Method, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

extern "C" {
    fn class_getClassMethod(cls: *const Class, sel: Sel) -> *mut Method;
    fn class_getInstanceMethod(cls: *const Class, sel: Sel) -> *mut Method;
    fn method_setImplementation(m: *mut Method, imp: Imp) -> Imp;
}

/// `APEX_CURSOR_DEBUG=1`: every `-[NSCursor set]` logged (who asks for
/// what pointer, and when), to tell whether a page ever asks for its
/// hand over a link.
static mut ORIGINAL_SET: Option<extern "C" fn(&Object, Sel)> = None;
extern "C" fn logged_set(this: &Object, sel: Sel) {
    unsafe {
        let me = this as *const Object as Id;
        let named = |sel_name: &'static str, s: Sel| -> Option<&'static str> {
            let c: Id = msg_send![class!(NSCursor), performSelector: s];
            (c == me).then_some(sel_name)
        };
        let which = if me == ARROW {
            "apex big arrow"
        } else if me == BOXC {
            "apex box"
        } else if me == NATIVE {
            "apex no-op"
        } else {
            named("system arrow", sel!(arrowCursor))
                .or_else(|| named("pointing hand", sel!(pointingHandCursor)))
                .or_else(|| named("I-beam", sel!(IBeamCursor)))
                .or_else(|| named("current", sel!(currentCursor)))
                .unwrap_or("other")
        };
        // who asks: the frames above us, the ones from a framework or us
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<String> = bt
            .lines()
            .filter(|l| l.contains("WebKit") || l.contains("AppKit") || l.contains("gpui") || l.contains("apex") || l.contains("wry"))
            .filter(|l| !l.contains("logged_set"))
            .take(6)
            .map(|l| l.trim().to_string())
            .collect();
        crate::shell::log_line(&format!("cursor set: {which} {me:p} via {}", frames.join(" | ")));
        if let Some(orig) = ORIGINAL_SET {
            orig(this, sel);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct NSSize {
    width: f64,
    height: f64,
}

type Id = *mut Object;

/// Plan 9 `Cursor`: offset, then 16 rows of 2 bytes for each plane.
struct Cursor {
    offset: (i32, i32),
    clr: [u8; 32],
    set: [u8; 32],
}

/// Plan 9 `Cursor2`: 32 rows of 4 bytes for each plane.
struct Cursor2 {
    offset: (i32, i32),
    clr: [u8; 128],
    set: [u8; 128],
}

static BIGARROW: Cursor = Cursor {
    offset: (-1, -1),
    clr: [
        0xFF, 0xFF, 0x80, 0x01, 0x80, 0x02, 0x80, 0x0C, 0x80, 0x10, 0x80, 0x10, 0x80, 0x08, 0x80, 0x04, 0x80, 0x02, 0x80, 0x01, 0x80, 0x02,
        0x8C, 0x04, 0x92, 0x08, 0x91, 0x10, 0xA0, 0xA0, 0xC0, 0x40,
    ],
    set: [
        0x00, 0x00, 0x7F, 0xFE, 0x7F, 0xFC, 0x7F, 0xF0, 0x7F, 0xE0, 0x7F, 0xE0, 0x7F, 0xF0, 0x7F, 0xF8, 0x7F, 0xFC, 0x7F, 0xFE, 0x7F, 0xFC,
        0x73, 0xF8, 0x61, 0xF0, 0x60, 0xE0, 0x40, 0x40, 0x00, 0x00,
    ],
};

static BIGARROW2: Cursor2 = Cursor2 {
    offset: (-2, -2),
    clr: [
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xC0, 0x00, 0x00, 0x03, 0xC0, 0x00, 0x00, 0x07, 0xC0, 0x00, 0x00, 0x1E, 0xC0, 0x00,
        0x00, 0x3C, 0xC0, 0x00, 0x00, 0xF0, 0xC0, 0x00, 0x03, 0xE0, 0xC0, 0x00, 0x0F, 0x80, 0xC0, 0x00, 0x0E, 0x00, 0xC0, 0x00, 0x07, 0x00,
        0xC0, 0x00, 0x03, 0x80, 0xC0, 0x00, 0x01, 0xC0, 0xC0, 0x00, 0x00, 0xE0, 0xC0, 0x00, 0x00, 0x70, 0xC0, 0x00, 0x00, 0x38, 0xC0, 0x00,
        0x00, 0x1C, 0xC0, 0x00, 0x00, 0x0E, 0xC0, 0x00, 0x00, 0x07, 0xC0, 0x00, 0x00, 0x03, 0xC0, 0xC0, 0x00, 0x07, 0xC0, 0xE0, 0x00, 0x0E,
        0xC1, 0xF0, 0x00, 0x1C, 0xC1, 0xB8, 0x00, 0x38, 0xC3, 0x9C, 0x00, 0x70, 0xC3, 0x0E, 0x00, 0xE0, 0xC7, 0x07, 0x01, 0xC0, 0xCE, 0x03,
        0x83, 0x80, 0xCC, 0x01, 0xC7, 0x00, 0xDC, 0x00, 0xEE, 0x00, 0xF8, 0x00, 0x7C, 0x00, 0xF0, 0x00, 0x38, 0x00,
    ],
    set: [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xF8, 0x3F, 0xFF, 0xFF, 0xE0, 0x3F, 0xFF,
        0xFF, 0xC0, 0x3F, 0xFF, 0xFF, 0x00, 0x3F, 0xFF, 0xFC, 0x00, 0x3F, 0xFF, 0xF0, 0x00, 0x3F, 0xFF, 0xF0, 0x00, 0x3F, 0xFF, 0xF8, 0x00,
        0x3F, 0xFF, 0xFC, 0x00, 0x3F, 0xFF, 0xFE, 0x00, 0x3F, 0xFF, 0xFF, 0x00, 0x3F, 0xFF, 0xFF, 0x80, 0x3F, 0xFF, 0xFF, 0xC0, 0x3F, 0xFF,
        0xFF, 0xE0, 0x3F, 0xFF, 0xFF, 0xF0, 0x3F, 0xFF, 0xFF, 0xF8, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0x3F, 0xFF, 0xF8, 0x3F, 0x1F, 0xFF, 0xF0,
        0x3E, 0x0F, 0xFF, 0xE0, 0x3E, 0x07, 0xFF, 0xC0, 0x3C, 0x03, 0xFF, 0x80, 0x3C, 0x01, 0xFF, 0x00, 0x38, 0x00, 0xFE, 0x00, 0x30, 0x00,
        0x7C, 0x00, 0x30, 0x00, 0x38, 0x00, 0x20, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ],
};

static BOX: Cursor = Cursor {
    offset: (-7, -7),
    clr: [
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF8, 0x1F, 0xF8, 0x1F, 0xF8, 0x1F, 0xF8, 0x1F, 0xF8, 0x1F, 0xF8, 0x1F,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ],
    set: [
        0x00, 0x00, 0x7F, 0xFE, 0x7F, 0xFE, 0x7F, 0xFE, 0x70, 0x0E, 0x70, 0x0E, 0x70, 0x0E, 0x70, 0x0E, 0x70, 0x0E, 0x70, 0x0E, 0x70, 0x0E,
        0x70, 0x0E, 0x7F, 0xFE, 0x7F, 0xFE, 0x7F, 0xFE, 0x00, 0x00,
    ],
};

static BOX2: Cursor2 = Cursor2 {
    offset: (-15, -15),
    clr: [
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xC0, 0x03, 0xFF,
        0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0,
        0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF, 0xFF, 0xC0, 0x03, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ],
    set: [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF,
        0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC,
        0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00,
        0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC,
        0x3F, 0x00, 0x00, 0xFC, 0x3F, 0x00, 0x00, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF,
        0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x3F, 0xFF, 0xFF, 0xFC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ],
};

/// RGBA pixels from a pair of 1-bit planes: `set` bits are black, `clr`
/// bits white, the rest transparent.
fn pixels(w: usize, h: usize, clr: &[u8], set: &[u8]) -> Vec<u8> {
    let stride = w / 8;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let bit = 0x80 >> (x % 8);
            let i = y * stride + x / 8;
            let px = &mut out[(y * w + x) * 4..(y * w + x) * 4 + 4];
            if set[i] & bit != 0 {
                px.copy_from_slice(&[0, 0, 0, 255]);
            } else if clr[i] & bit != 0 {
                px.copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    out
}

unsafe fn bitmap_rep(w: usize, h: usize, rgba: &[u8]) -> Id {
    let rep: Id = msg_send![class!(NSBitmapImageRep), alloc];
    let space: Id = msg_send![class!(NSString), stringWithUTF8String: c"NSCalibratedRGBColorSpace".as_ptr()];
    let rep: Id = msg_send![rep,
        initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>()
        pixelsWide: w as isize
        pixelsHigh: h as isize
        bitsPerSample: 8isize
        samplesPerPixel: 4isize
        hasAlpha: true
        isPlanar: false
        colorSpaceName: space
        bytesPerRow: (w * 4) as isize
        bitsPerPixel: 32isize];
    let data: *mut u8 = msg_send![rep, bitmapData];
    std::ptr::copy_nonoverlapping(rgba.as_ptr(), data, rgba.len());
    rep
}

/// An `NSCursor` from a Plan 9 cursor and its double-resolution twin.
unsafe fn make(c: &Cursor, c2: &Cursor2) -> Id {
    let image: Id = msg_send![class!(NSImage), alloc];
    let image: Id = msg_send![image, initWithSize: NSSize { width: 16., height: 16. }];
    let rep1 = bitmap_rep(16, 16, &pixels(16, 16, &c.clr, &c.set));
    let rep2 = bitmap_rep(32, 32, &pixels(32, 32, &c2.clr, &c2.set));
    // both represent 16 points; the second is the 2× rendition
    let _: () = msg_send![rep1, setSize: NSSize { width: 16., height: 16. }];
    let _: () = msg_send![rep2, setSize: NSSize { width: 16., height: 16. }];
    let _: () = msg_send![image, addRepresentation: rep1];
    let _: () = msg_send![image, addRepresentation: rep2];
    // Plan 9 draws the image at mouse + offset: the hot spot is -offset
    // (the 2× cursor's offset is the same point at twice the resolution)
    debug_assert_eq!((c2.offset.0 / 2, c2.offset.1 / 2), c.offset);
    let hot = NSPoint { x: -c.offset.0 as f64, y: -c.offset.1 as f64 };
    let cursor: Id = msg_send![class!(NSCursor), alloc];
    msg_send![cursor, initWithImage: image hotSpot: hot]
}

static INSTALL: Once = Once::new();
static mut ARROW: Id = std::ptr::null_mut();
static mut BOXC: Id = std::ptr::null_mut();
static mut NATIVE: Id = std::ptr::null_mut();

extern "C" fn arrow_cursor(_cls: &Class, _sel: Sel) -> Id {
    unsafe { ARROW }
}
extern "C" fn box_cursor(_cls: &Class, _sel: Sel) -> Id {
    unsafe { BOXC }
}
extern "C" fn native_cursor(_cls: &Class, _sel: Sel) -> Id {
    unsafe { NATIVE }
}
/// `-[ApexNoCursor set]`: nothing. gpui puts the hovered style's cursor
/// in a cursor rect over its whole view; over a page that rect must not
/// speak, so the page's own tracking areas (a hand over a link, a beam
/// over text) decide.
extern "C" fn set_nothing(_this: &Object, _sel: Sel) {}

/// The style the acme area asks for: the big arrow.
pub const BIG_ARROW: gpui::CursorStyle = gpui::CursorStyle::DragLink;
/// The style while a layout box is held: the box.
pub const BOX_CURSOR: gpui::CursorStyle = gpui::CursorStyle::DragCopy;
/// The style over a web or preview body: a cursor that sets nothing,
/// leaving the pointer to the page.
pub const NATIVE_CURSOR: gpui::CursorStyle = gpui::CursorStyle::ContextualMenu;

/// Set the system cursor for a style now, over a page: gpui's cursor
/// rect there is the no-op, so this stands until the page says otherwise.
pub fn apply(style: gpui::CursorStyle) {
    use gpui::CursorStyle::*;
    unsafe {
        let cls = class!(NSCursor);
        let c: Id = match style {
            PointingHand => msg_send![cls, pointingHandCursor],
            IBeam => msg_send![cls, IBeamCursor],
            OpenHand => msg_send![cls, openHandCursor],
            ClosedHand => msg_send![cls, closedHandCursor],
            Crosshair => msg_send![cls, crosshairCursor],
            ResizeLeftRight => msg_send![cls, resizeLeftRightCursor],
            ResizeUpDown => msg_send![cls, resizeUpDownCursor],
            OperationNotAllowed => msg_send![cls, operationNotAllowedCursor],
            _ => msg_send![cls, arrowCursor],
        };
        if !c.is_null() {
            let _: () = msg_send![c, set];
        }
    }
}

/// Put the two cursors behind their styles, for the life of the process.
pub fn install() {
    INSTALL.call_once(|| unsafe {
        ARROW = make(&BIGARROW, &BIGARROW2);
        BOXC = make(&BOX, &BOX2);
        // a cursor that does nothing when set, for the pages
        NATIVE = {
            let mut decl = ClassDecl::new("ApexNoCursor", class!(NSCursor)).expect("a fresh class name");
            decl.add_method(sel!(set), set_nothing as extern "C" fn(&Object, Sel));
            let no_cls = decl.register();
            let image: Id = msg_send![class!(NSImage), alloc];
            let image: Id = msg_send![image, initWithSize: NSSize { width: 1., height: 1. }];
            let c: Id = msg_send![no_cls, alloc];
            msg_send![c, initWithImage: image hotSpot: NSPoint { x: 0., y: 0. }]
        };
        let cls: *const Class = class!(NSCursor);
        if std::env::var_os("APEX_CURSOR_DEBUG").is_some() {
            let set_m = class_getInstanceMethod(cls, sel!(set));
            if !set_m.is_null() {
                let imp: Imp = std::mem::transmute(logged_set as extern "C" fn(&Object, Sel));
                let prev = method_setImplementation(set_m, imp);
                ORIGINAL_SET = Some(std::mem::transmute(prev));
            }
        }
        let arrow_m = class_getClassMethod(cls, sel!(dragLinkCursor));
        let cross_m = class_getClassMethod(cls, sel!(dragCopyCursor));
        let native_m = class_getClassMethod(cls, sel!(contextualMenuCursor));
        if !native_m.is_null() {
            let imp: Imp = std::mem::transmute(native_cursor as extern "C" fn(&Class, Sel) -> Id);
            method_setImplementation(native_m, imp);
        }
        if !arrow_m.is_null() {
            let imp: Imp = std::mem::transmute(arrow_cursor as extern "C" fn(&Class, Sel) -> Id);
            method_setImplementation(arrow_m, imp);
        }
        if !cross_m.is_null() {
            let imp: Imp = std::mem::transmute(box_cursor as extern "C" fn(&Class, Sel) -> Id);
            method_setImplementation(cross_m, imp);
        }
    });
}
