//! The Mac's clipboard for programs in a session's terminals, on any
//! host (`apex_server::clip`): a program there that reads it (a stand-in
//! xclip, wl-paste or xsel: Claude Code's ^V) is answered from here, and
//! what one puts there (xclip -i, wl-copy) is put here.
//!
//! In the daemon's words: `TARGETS` is what there is (`image/png` for any
//! image, `image/jpeg` for a JPEG as it is, `text`); `text` the text;
//! `image/png` the image as a PNG (a TIFF -- a screenshot copied -- or a
//! JPEG made one); `image/jpeg` a JPEG as it is.

#![cfg(target_os = "macos")]

use std::time::Duration;

use objc::{class, msg_send, sel, sel_impl};

use crate::dragdata::{as_png, bytes, has, ns, Id};

const TEXT: &str = "public.utf8-plain-text";
const PNG: &str = "public.png";
const JPEG: &str = "public.jpeg";
const TIFF: [&str; 2] = ["public.tiff", "NSTIFFPboardType"];

fn general() -> Id {
    // SAFETY: the shared pasteboard, on the main thread
    unsafe { msg_send![class!(NSPasteboard), generalPasteboard] }
}

/// What there is, in the daemon's words, the best first.
pub fn targets() -> Vec<&'static str> {
    let pb = general();
    let mut out = Vec::new();
    // SAFETY: plain pasteboard queries on the main thread
    unsafe {
        if has(pb, &[PNG, JPEG, TIFF[0], TIFF[1]]) {
            out.push("image/png");
        }
        if has(pb, &[JPEG]) {
            out.push("image/jpeg");
        }
        if has(pb, &[TEXT]) {
            out.push("text");
        }
    }
    out
}

/// What there is of `want`, as its bytes; none when there is none.
pub fn read(want: &str) -> Option<Vec<u8>> {
    let pb = general();
    // SAFETY: as `targets`
    unsafe {
        match want {
            "TARGETS" => Some(targets().join("\n").into_bytes()),
            "text" => bytes(msg_send![pb, dataForType: ns(TEXT)]),
            "image/jpeg" => bytes(msg_send![pb, dataForType: ns(JPEG)]),
            "image/png" => bytes(msg_send![pb, dataForType: ns(PNG)]).or_else(|| [TIFF[0], TIFF[1], JPEG].into_iter().find_map(|t| as_png(msg_send![pb, dataForType: ns(t)]))),
            _ => None,
        }
    }
}

/// An image a program put on the clipboard (text comes as OSC 52's
/// does, `ServerMsg::Clipboard`).
pub fn put(want: &str, data: &[u8]) {
    let ty = match want {
        "image/png" => PNG,
        "image/jpeg" => JPEG,
        _ => return,
    };
    let pb = general();
    // SAFETY: an NSData of our bytes set on the shared pasteboard, main thread
    unsafe {
        let d: Id = msg_send![class!(NSData), dataWithBytes: data.as_ptr() length: data.len()];
        let _: isize = msg_send![pb, clearContents];
        let _: bool = msg_send![pb, setData: d forType: ns(ty)];
    }
}

/// Answer the daemon's clipboard read `id`: read here, now, then sent on
/// a thread, paced, so an image keeps no key waiting.
pub fn answer(plane: apex_server::plane::IoPlane, id: u64, want: &str) {
    let (status, body) = match read(want) {
        Some(b) => ("200", b),
        None => ("404", Vec::new()),
    };
    std::thread::spawn(move || {
        let _ = plane.put_paced("PUT", &format!("apex-clip-answer:{id}"), &[("Status", status)], &body, Duration::from_secs(60));
    });
}
