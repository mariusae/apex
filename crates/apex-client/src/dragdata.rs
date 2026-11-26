//! An image dragged in with no file behind it -- from a browser, a
//! preview, a screenshot's markup -- dropped as if it were one: written
//! to `~/.apex/drops/KEY/NAME` on this Mac (the key the image's own, so
//! the same image dropped again is the same file), and that path dropped
//! where it landed (`drops`), sent on to the host first if the session
//! is on another.
//!
//! gpui's windows take only files dragged in (`NSFilenamesPboardType`).
//! So each window is registered for images too, and gpui's drag methods
//! wrapped: a drag that has files goes to gpui as ever; one that has an
//! image and no file is ours, answered as a copy and, dropped, written
//! out and queued for the window's app to put where it landed (`take`).

#![cfg(target_os = "macos")]

use std::collections::HashSet;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Mutex, Once};

use gpui::{Pixels, Point, Window};
use objc::runtime::{Class, Imp, Method, Object, Sel, BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl};

extern "C" {
    fn class_getInstanceMethod(cls: *const Class, sel: Sel) -> *mut Method;
    fn method_setImplementation(m: *mut Method, imp: Imp) -> Imp;
}

type Id = *mut Object;
type NSUInteger = usize;

#[repr(C)]
#[derive(Clone, Copy)]
struct P {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct R {
    origin: P,
    size: P,
}

const DRAG_NONE: NSUInteger = 0;
const DRAG_COPY: NSUInteger = 1;

/// The image types taken, the best first, with the file each is written
/// as: PNG and JPEG as they come, anything else (TIFF) made a PNG.
const IMAGE_TYPES: [&str; 4] = ["public.png", "public.jpeg", "public.tiff", "NSTIFFPboardType"];

/// What a drag with files has (either says it).
const FILE_TYPES: [&str; 2] = ["NSFilenamesPboardType", "public.file-url"];

type Entered = extern "C" fn(&Object, Sel, Id) -> NSUInteger;
type Perform = extern "C" fn(&Object, Sel, Id) -> BOOL;
static mut ENTERED: Option<Entered> = None;
static mut UPDATED: Option<Entered> = None;
static mut PERFORM: Option<Perform> = None;

/// Images dropped and written, for the window they were dropped on (its
/// NSWindow), at a place in it.
static DROPPED: Mutex<Vec<(usize, PathBuf, Point<Pixels>)>> = Mutex::new(Vec::new());

fn ns(s: &str) -> Id {
    let c = CString::new(s).unwrap_or_default();
    // SAFETY: a plain NSString from a C string, autoreleased
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

/// Whether the drag's pasteboard has one of `types`.
unsafe fn has(pb: Id, types: &[&str]) -> bool {
    let list: Id = msg_send![pb, types];
    if list.is_null() {
        return false;
    }
    let n: NSUInteger = msg_send![list, count];
    for i in 0..n {
        let t: Id = msg_send![list, objectAtIndex: i];
        for want in types {
            let same: BOOL = msg_send![t, isEqualToString: ns(want)];
            if same != NO {
                return true;
            }
        }
    }
    false
}

/// Ours: an image, and no file.
unsafe fn ours(info: Id) -> bool {
    let pb: Id = msg_send![info, draggingPasteboard];
    !pb.is_null() && !has(pb, &FILE_TYPES) && has(pb, &IMAGE_TYPES)
}

unsafe fn bytes(data: Id) -> Option<Vec<u8>> {
    if data.is_null() {
        return None;
    }
    let len: NSUInteger = msg_send![data, length];
    let ptr: *const u8 = msg_send![data, bytes];
    (len > 0 && !ptr.is_null()).then(|| std::slice::from_raw_parts(ptr, len).to_vec())
}

/// The image on the pasteboard as a file's bytes and its extension: PNG
/// or JPEG as it is, TIFF made a PNG.
unsafe fn image(pb: Id) -> Option<(Vec<u8>, &'static str)> {
    if let Some(b) = bytes(msg_send![pb, dataForType: ns("public.png")]) {
        return Some((b, "png"));
    }
    if let Some(b) = bytes(msg_send![pb, dataForType: ns("public.jpeg")]) {
        return Some((b, "jpg"));
    }
    for t in ["public.tiff", "NSTIFFPboardType"] {
        let tiff: Id = msg_send![pb, dataForType: ns(t)];
        if tiff.is_null() {
            continue;
        }
        let rep: Id = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
        if rep.is_null() {
            continue;
        }
        let props: Id = msg_send![class!(NSDictionary), dictionary];
        // NSBitmapImageFileTypePNG
        let png: Id = msg_send![rep, representationUsingType: 4usize properties: props];
        if let Some(b) = bytes(png) {
            return Some((b, "png"));
        }
    }
    None
}

/// What the drag says the image is called: the last part of its URL's
/// path when that is an image's name, else when it was dropped.
unsafe fn name(pb: Id, ext: &str) -> String {
    let url: Id = msg_send![pb, stringForType: ns("public.url")];
    if !url.is_null() {
        let c: *const std::os::raw::c_char = msg_send![url, UTF8String];
        if !c.is_null() {
            let s = std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned();
            // its name, with the extension of what it is as written
            if let Some(n) = url_name(&s) {
                let stem = n.rsplit_once('.').map_or(n.as_str(), |(s, _)| s);
                return format!("{stem}.{ext}");
            }
        }
    }
    format!("Image {}.{ext}", stamp())
}

/// The file name a URL ends in, when it names an image.
pub fn url_name(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let last = path.rsplit('/').next()?;
    let last = percent_decode(last);
    let lower = last.to_ascii_lowercase();
    let image = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff", ".heic"].iter().any(|e| lower.ends_with(e));
    (image && apex_server::drops::valid_name(&last)).then_some(last)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Now, as macOS names a screenshot: "2026-11-26 at 15.20.07".
fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // SAFETY: localtime_r into a zeroed tm of our own
    unsafe {
        let mut tm: libc_tm = std::mem::zeroed();
        let t = now as std::os::raw::c_long;
        if localtime_r(&t, &mut tm).is_null() {
            return now.to_string();
        }
        format!("{:04}-{:02}-{:02} at {:02}.{:02}.{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

#[repr(C)]
struct libc_tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: std::os::raw::c_long,
    tm_zone: *const std::os::raw::c_char,
}

extern "C" {
    fn localtime_r(t: *const std::os::raw::c_long, tm: *mut libc_tm) -> *mut libc_tm;
}

/// The image written where a drop of it goes on this Mac: its path.
pub fn write(bytes: &[u8], name: &str) -> std::io::Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let key = format!("{:016x}", h.finish());
    let dir = apex_server::drops::dir().join(key);
    let path = dir.join(name);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    let part = dir.join(format!(".{name}.part"));
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, &path)?;
    Ok(path)
}

extern "C" fn entered(this: &Object, sel: Sel, info: Id) -> NSUInteger {
    // SAFETY: AppKit's own call, on the main thread; the original is gpui's
    unsafe {
        if ours(info) {
            return DRAG_COPY;
        }
        match ENTERED {
            Some(f) => f(this, sel, info),
            None => DRAG_NONE,
        }
    }
}

extern "C" fn updated(this: &Object, sel: Sel, info: Id) -> NSUInteger {
    // SAFETY: as `entered`
    unsafe {
        if ours(info) {
            return DRAG_COPY;
        }
        match UPDATED {
            Some(f) => f(this, sel, info),
            None => DRAG_NONE,
        }
    }
}

extern "C" fn perform(this: &Object, sel: Sel, info: Id) -> BOOL {
    // SAFETY: as `entered`; the window's content view is gpui's
    unsafe {
        if !ours(info) {
            return match PERFORM {
                Some(f) => f(this, sel, info),
                None => NO,
            };
        }
        let pb: Id = msg_send![info, draggingPasteboard];
        let Some((data, ext)) = image(pb) else { return NO };
        let name = name(pb, ext);
        let Ok(path) = write(&data, &name) else { return NO };
        // where, as gpui has a point: from the content view's top left
        let at: P = msg_send![info, draggingLocation];
        let view: Id = msg_send![this, contentView];
        let frame: R = msg_send![view, frame];
        let pos = Point { x: gpui::px(at.x as f32), y: gpui::px((frame.size.y - at.y) as f32) };
        DROPPED.lock().unwrap().push((this as *const Object as usize, path, pos));
        YES
    }
}

/// gpui's window class's drag methods wrapped (once).
pub fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| unsafe {
        let Some(cls) = Class::get("GPUIWindow") else { return };
        let m = class_getInstanceMethod(cls, sel!(draggingEntered:));
        let u = class_getInstanceMethod(cls, sel!(draggingUpdated:));
        let p = class_getInstanceMethod(cls, sel!(performDragOperation:));
        if m.is_null() || u.is_null() || p.is_null() {
            return;
        }
        ENTERED = Some(std::mem::transmute::<Imp, Entered>(method_setImplementation(m, std::mem::transmute::<Entered, Imp>(entered))));
        UPDATED = Some(std::mem::transmute::<Imp, Entered>(method_setImplementation(u, std::mem::transmute::<Entered, Imp>(updated))));
        PERFORM = Some(std::mem::transmute::<Imp, Perform>(method_setImplementation(p, std::mem::transmute::<Perform, Imp>(perform))));
    });
}

fn ns_window(window: &Window) -> Option<Id> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let h = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = h.as_raw() else { return None };
    let view = h.ns_view.as_ptr() as Id;
    // SAFETY: gpui's own view, alive while the window is; main thread
    let w: Id = unsafe { msg_send![view, window] };
    (!w.is_null()).then_some(w)
}

/// `window` takes images dragged in too (once for each).
pub fn register(window: &Window) {
    static DONE: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
    let Some(w) = ns_window(window) else { return };
    let mut done = DONE.lock().unwrap();
    if !done.get_or_insert_with(HashSet::new).insert(w as usize) {
        return;
    }
    // SAFETY: an NSArray of NSStrings, given to the window on the main thread
    unsafe {
        let types: Vec<Id> = FILE_TYPES.iter().chain(IMAGE_TYPES.iter()).map(|t| ns(t)).collect();
        let array: Id = msg_send![class!(NSArray), arrayWithObjects: types.as_ptr() count: types.len()];
        let () = msg_send![w, registerForDraggedTypes: array];
    }
}

/// Images dropped on `window` since last asked: each written out, and
/// where it was dropped.
pub fn take(window: &Window) -> Vec<(PathBuf, Point<Pixels>)> {
    let Some(w) = ns_window(window) else { return Vec::new() };
    let mut all = DROPPED.lock().unwrap();
    let (mine, rest): (Vec<_>, Vec<_>) = all.drain(..).partition(|(x, _, _)| *x == w as usize);
    *all = rest;
    mine.into_iter().map(|(_, p, at)| (p, at)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_names_an_image_by_its_last_part() {
        assert_eq!(url_name("https://example.com/a/b/cat%20photo.JPG?w=200#x").as_deref(), Some("cat photo.JPG"));
        assert_eq!(url_name("https://example.com/img/diagram.png").as_deref(), Some("diagram.png"));
        assert_eq!(url_name("https://example.com/page.html"), None, "not an image");
        assert_eq!(url_name("https://example.com/"), None);
        assert_eq!(url_name("https://example.com/..%2F..%2Fx.png"), None, "a name that would leave its folder");
    }

    #[test]
    fn now_reads_as_a_screenshot_says_it() {
        let s = stamp();
        // "2026-11-26 at 15.20.07"
        assert_eq!(s.len(), 22, "{s}");
        assert_eq!(&s[10..14], " at ");
    }
}
