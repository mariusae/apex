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
    /// Every session a window has shown, in the order first shown: the
    /// order of the title bar's tabs, which list the ones still
    /// connected (shown or parked).
    order: Vec<SessionUrl>,
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
        // the tabs of last time: the order starts as they were, so the
        // first window's own tab joins them rather than replacing them
        let text = std::fs::read_to_string(Self::tabs_file()).unwrap_or_default();
        let order: Vec<SessionUrl> = text.lines().filter_map(SessionUrl::parse).filter(|u| u.id.is_some()).collect();
        cx.set_global(Pool { parked: HashMap::new(), order, wake });
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                cx.update(|cx| Pool::tend(cx));
            }
        })
        .detach();
    }

    /// A window shows this session: a tab for it, in first-shown order.
    /// Only once its identity is known: a session named by label alone
    /// is noted after the attach says which it is.
    pub fn note_open(cx: &mut App, url: &SessionUrl) {
        if url.id.is_none() {
            return;
        }
        let Some(pool) = cx.try_global::<Pool>() else { return };
        if pool.order.contains(url) {
            // the label may have changed: the tab says the current one
            let pool = cx.global_mut::<Pool>();
            if let Some(u) = pool.order.iter_mut().find(|u| *u == url) {
                if *u != *url || u.session != url.session {
                    *u = url.clone();
                    pool.save_tabs();
                }
            }
            return;
        }
        let pool = cx.global_mut::<Pool>();
        pool.order.push(url.clone());
        pool.save_tabs();
    }

    /// The tabs, one URL a line, for next time.
    fn tabs_file() -> std::path::PathBuf {
        crate::shell::state_file().with_file_name("open-sessions")
    }

    fn save_tabs(&self) {
        let p = Self::tabs_file();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let text: String = self.order.iter().map(|u| format!("{u}\n")).collect();
        let _ = std::fs::write(p, text);
    }

    /// The tabs of last time, attached again in the background and
    /// parked, each as it was named then (by identity: one that is gone
    /// is dropped, not made anew); `shown` are the sessions windows
    /// already have.
    pub fn restore(cx: &mut App, shown: &[SessionUrl]) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let wake = pool.wake.clone();
        let urls: Vec<SessionUrl> = pool.order.clone();
        // what the windows show, as they know it now: a window attached by
        // label has the session's id by now, where the launch target
        // carried last time's, stale once the daemon has been restarted;
        // a tab compared against the stale id attached the same session
        // again, and the parked link led while the window sat fenced
        let mut shown: Vec<SessionUrl> = shown.to_vec();
        shown.extend(Self::shown_urls(cx));
        crate::shell::log_line(&format!("restoring {} tab(s) of last time; {} shown already", urls.len(), shown.len()));
        for url in urls {
            if shown.contains(&url) || shown.iter().any(|s| s.id.is_some() && url.id.is_some() && s.provider == url.provider && s.arg == url.arg && s.session == url.session) {
                // the same session, or the same label on the same host
                // under another id: last time's, before a restart
                continue;
            }
            crate::shell::log_line(&format!("tab {url}: attaching again"));
            let (u, w) = (url.clone(), wake.clone());
            let connecting = cx.background_executor().spawn(async move { crate::app::Acme::connect_existing_targeted(&u, w) });
            cx.spawn(async move |cx| {
                let r = connecting.await;
                let _ = cx.update(|cx| match r {
                    Ok((link, log, node, target)) => {
                        crate::shell::log_line(&format!("tab {url} attached again, parked"));
                        let parked = Parked { link, log, node, url: url.clone(), target, previews: Vec::new(), live: std::collections::HashMap::new(), snarfouts: Vec::new(), pending_goto: None, parked_at: Instant::now() };
                        Pool::park(cx, parked);
                    }
                    Err(e) => {
                        crate::shell::log_line(&format!("tab {url}: {e}; forgotten"));
                        let pool = cx.global_mut::<Pool>();
                        pool.order.retain(|u| *u != url);
                        pool.save_tabs();
                    }
                });
            })
            .detach();
        }
    }

    /// The tabs: the sessions still connected — `current`, shown in the
    /// window asking, and the parked ones — in first-shown order.
    pub fn tabs(cx: &App, current: &SessionUrl) -> Vec<SessionUrl> {
        let Some(pool) = cx.try_global::<Pool>() else { return vec![current.clone()] };
        let mut out: Vec<SessionUrl> = pool
            .order
            .iter()
            .filter_map(|u| {
                if *u == *current {
                    Some(current.clone())
                } else {
                    pool.parked.values().find(|p| p.url == *u).map(|p| p.url.clone())
                }
            })
            .collect();
        if !out.contains(current) {
            out.push(current.clone());
        }
        out
    }

    /// Let a parked session go: its link closes, its tab with it.
    pub fn let_go(cx: &mut App, url: &SessionUrl) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let keys: Vec<String> = pool.parked.iter().filter(|(_, p)| p.url == *url).map(|(k, _)| k.clone()).collect();
        let pool = cx.global_mut::<Pool>();
        for k in keys {
            if let Some(mut p) = pool.parked.remove(&k) {
                p.link.close();
                crate::shell::log_line(&format!("parked {k} let go"));
            }
        }
        pool.order.retain(|u| u != url);
        pool.save_tabs();
    }

    /// Park a session: its wake comes here from now on.
    /// The sessions the windows show, as their links know them.
    pub fn shown_urls(cx: &App) -> Vec<SessionUrl> {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<crate::app::Acme>())
            .filter_map(|h| h.read(cx).ok().map(|a| a.url.clone()))
            .collect()
    }

    pub fn park(cx: &mut App, p: Parked) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        // a window shows this session already: a second attachment of
        // ours would take the lead from it (the daemon lets the latest
        // UI lead), leaving the window fenced; this link is let go
        if Self::shown_urls(cx).iter().any(|s| *s == p.url) {
            crate::shell::log_line(&format!("{}: shown already; not parked", p.url));
            let mut p = p;
            p.link.close();
            return;
        }
        p.target.set(pool.wake.clone());
        let key = p.url.to_string();
        let pool = cx.global_mut::<Pool>();
        // the same session parked twice (whatever its label was): the older leaves
        let same: Vec<String> = pool.parked.iter().filter(|(_, x)| x.url == p.url).map(|(k, _)| k.clone()).collect();
        for k in same {
            if let Some(mut old) = pool.parked.remove(&k) {
                old.link.close();
            }
        }
        pool.parked.insert(key.clone(), p);
        while pool.parked.len() > CAP {
            let oldest = pool.parked.iter().min_by_key(|(_, p)| p.parked_at).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    if let Some(mut p) = pool.parked.remove(&k) {
                        crate::shell::log_line(&format!("parked {k} let go: {CAP} is enough"));
                        pool.order.retain(|u| *u != p.url);
                        pool.save_tabs();
                        p.link.close();
                    }
                }
                None => break,
            }
        }
        crate::shell::log_line(&format!("parked {key}"));
    }

    /// The session parked most recently: the one to switch back to.
    pub fn most_recent(cx: &App) -> Option<SessionUrl> {
        Pool::by_recency(cx).into_iter().next()
    }

    /// The parked sessions, the most recently parked first.
    pub fn by_recency(cx: &App) -> Vec<SessionUrl> {
        let Some(pool) = cx.try_global::<Pool>() else { return Vec::new() };
        let mut v: Vec<(Instant, SessionUrl)> = pool.parked.values().map(|p| (p.parked_at, p.url.clone())).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.into_iter().map(|(_, u)| u).collect()
    }

    /// Take a parked session to show it.
    pub fn take(cx: &mut App, url: &SessionUrl) -> Option<Parked> {
        let key = cx.try_global::<Pool>()?.parked.iter().find(|(_, p)| p.url == *url).map(|(k, _)| k.clone())?;
        let p = cx.global_mut::<Pool>().parked.remove(&key);
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
            if let Some(p) = pool.parked.remove(&key) {
                pool.order.retain(|u| *u != p.url);
                pool.save_tabs();
            }
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
        pool.order.clear();
        for (_, mut p) in pool.parked.drain() {
            p.link.close();
        }
    }
}
