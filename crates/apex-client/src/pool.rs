//! Parked sessions: a session this app showed and switched away from,
//! or closed the window of, stays attached, its link tended here, so
//! showing it again (⌘K back, a new window on it) is instant. Parked
//! sessions keep their lead: the daemon goes on forwarding tools'
//! proposals to them, and they apply them, refresh their tags and open
//! what their gotos ask for. Nothing is parked across a launch.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{App, Global};

use apex_core::*;
use apex_server::proto::ClientMsg;
use apex_server::providers::SessionUrl;
use apex_server::remote::{Link, Wake};
use apex_server::{perform, Proposal};

use crate::app::{client_do, Live};

/// How many sessions stay parked; the least recently parked goes first.
const CAP: usize = 8;

/// Where a link's wake goes: the window showing the session, or the
/// pool while it is parked. The reader thread holds the forwarding
/// closure; the target behind it moves.
#[derive(Clone)]
pub struct WakeTarget(Arc<Mutex<Wake>>);

impl WakeTarget {
    pub fn new(wake: Wake) -> WakeTarget {
        WakeTarget(Arc::new(Mutex::new(wake)))
    }

    /// The wake a link is made with: whatever the target is at the time.
    pub fn forwarding(&self) -> Wake {
        let t = self.0.clone();
        Arc::new(move || {
            let w = t.lock().unwrap().clone();
            w();
        })
    }

    pub fn set(&self, wake: Wake) {
        *self.0.lock().unwrap() = wake;
    }
}

/// A session as a window holds it, off the window.
pub struct Parked {
    pub link: Link,
    pub log: Log,
    pub node: Node,
    pub url: SessionUrl,
    pub target: WakeTarget,
    pub previews: Vec<(u64, String, Option<String>, u32)>,
    pub live: HashMap<String, Live>,
    pub snarfouts: Vec<(u64, TermId)>,
    pub pending_goto: Option<Loc>,
    pub parked_at: Instant,
}

pub struct Pool {
    parked: HashMap<String, Parked>,
    /// Wakes the tending task.
    wake: Wake,
}

impl Global for Pool {}

impl Pool {
    /// Make the pool and the task that tends what is parked whenever a
    /// parked link has something to say.
    pub fn install(cx: &mut App) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let wake: Wake = Arc::new(move || {
            let _ = tx.unbounded_send(());
        });
        cx.set_global(Pool { parked: HashMap::new(), wake });
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                cx.update(|cx| Pool::tend(cx));
            }
        })
        .detach();
    }

    /// Park a session: its wake comes here from now on.
    pub fn park(cx: &mut App, p: Parked) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        p.target.set(pool.wake.clone());
        let key = p.url.to_string();
        let pool = cx.global_mut::<Pool>();
        if let Some(mut old) = pool.parked.insert(key.clone(), p) {
            old.link.close(); // the same session parked twice: the older leaves
        }
        while pool.parked.len() > CAP {
            let oldest = pool.parked.iter().min_by_key(|(_, p)| p.parked_at).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    if let Some(mut p) = pool.parked.remove(&k) {
                        crate::shell::log_line(&format!("parked {k} let go: {CAP} is enough"));
                        p.link.close();
                    }
                }
                None => break,
            }
        }
        crate::shell::log_line(&format!("parked {key}"));
    }

    /// Take a parked session to show it.
    pub fn take(cx: &mut App, url: &SessionUrl) -> Option<Parked> {
        let p = cx.try_global::<Pool>()?.parked.contains_key(&url.to_string()).then(|| cx.global_mut::<Pool>().parked.remove(&url.to_string()))?;
        if p.is_some() {
            crate::shell::log_line(&format!("unparked {url}"));
        }
        p
    }

    /// The leader's duties for every parked session: what came in is
    /// applied (the link does that), tags refreshed, gotos opened, the
    /// rules' asks answered as far as a session nobody sees can, and what
    /// was appended shipped. A link that ended is let go.
    fn tend(cx: &mut App) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        if pool.parked.is_empty() {
            return;
        }
        let pool = cx.global_mut::<Pool>();
        let mut gone = Vec::new();
        for (key, p) in pool.parked.iter_mut() {
            if !p.link.poll(&mut p.node, &mut p.log) {
                gone.push(key.clone());
                continue;
            }
            let _ = p.node.update_tags(&mut p.log);
            let _ = p.node.take_shows();
            let _ = p.link.take_made();
            for loc in p.node.take_gotos() {
                let Some(col) = p.node.state.layout.cols.first().map(|c| c.id) else { continue };
                if apex_core::is_url(&loc.name) {
                    perform(&mut p.node, &mut p.log, vec![Proposal::OpenWeb { col, url: loc.name.clone() }]);
                } else {
                    p.link.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: loc.name.clone() });
                }
            }
            let asks = std::mem::take(&mut p.link.client_asks);
            for (id, verb, args) in asks {
                let result = match verb.as_str() {
                    "open" => client_do("open", &args).map(|_| None),
                    other => Err(format!("{other}: the session is parked, nobody sees it")),
                };
                p.link.send(&ClientMsg::Applied { id, result });
            }
            p.link.io.clear(); // a parked session's previews are over
            p.link.flush(&p.log);
            let _ = p.node.catch_up(&p.log);
        }
        for key in gone {
            crate::shell::log_line(&format!("parked {key}: link ended"));
            pool.parked.remove(&key);
        }
    }

    /// The app is quitting: every parked link ends, so the daemons see
    /// the attachments leave.
    pub fn close_all(cx: &mut App) {
        if let Some(pool) = cx.try_global::<Pool>() {
            if pool.parked.is_empty() {
                return;
            }
        } else {
            return;
        }
        let pool = cx.global_mut::<Pool>();
        for (_, mut p) in pool.parked.drain() {
            p.link.close();
        }
    }
}
