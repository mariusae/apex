//! The dock icon: it bounces when a notification comes to any session this
//! app has open -- shown in a window or parked in a tab -- while none of
//! its windows is in front, so a tool wanting the user shows from
//! whatever they are doing instead.

use std::collections::HashSet;

use gpui::{App, Global};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

use apex_core::Seq;
use apex_server::providers::SessionUrl;

use crate::app::Acme;
use crate::pool::Pool;

/// The notifications last seen in each session, by the entry that raised
/// each (`Notification::at`), to tell one that has come since.
#[derive(Default)]
struct Seen(Vec<(SessionUrl, HashSet<Seq>)>);

impl Global for Seen {}

/// Every tick: a notification come to any session since the last look,
/// with no window of the app's in front, bounces the dock icon once. A
/// session seen for the first time brings no bounce for what it already
/// had.
pub fn tick(cx: &mut App) {
    let mut active = false;
    let mut now = Pool::notifications(cx);
    for h in cx.windows().into_iter().filter_map(|w| w.downcast::<Acme>()) {
        if let Ok(a) = h.read(cx) {
            active |= a.app_active();
            now.push((a.url.clone(), a.node.notifications().map(|n| n.at).collect()));
        }
    }
    let seen = cx.default_global::<Seen>();
    let came = now.iter().any(|(u, ats)| seen.0.iter().find(|(s, _)| s == u).is_some_and(|(_, was)| !ats.is_subset(was)));
    seen.0 = now;
    if came && !active {
        bounce();
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
