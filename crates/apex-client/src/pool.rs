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
/// A tab beyond that keeps its place, with no link under it, until it is
/// shown again.
const CAP: usize = 8;

/// What a tab is doing, when it is not simply up. Tabs are local state:
/// one is made when the user makes it and brought back at launch, and it
/// goes only when the user closes it (or its session does) -- whatever
/// the link under it is doing. So a tab still connecting keeps its place
/// in the bar, and one whose link has gone says so instead of vanishing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    /// A link is being made, and why.
    Coming(Why),
    /// Attached: shown in the window, or parked here.
    Up,
    /// No link, and why (the tab's card says it in full).
    Down(String),
}

impl Tab {
    /// The word after the session's name in its tab, when there is one.
    pub fn word(&self) -> Option<&str> {
        match self {
            Tab::Coming(why) => Some(why.word()),
            Tab::Up => None,
            Tab::Down(_) => Some("offline"),
        }
    }
}

/// Why a link is being made: a word for the tab, and a sentence for the
/// blank page of the tab while it waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A tab the user has just asked for.
    Attaching,
    /// A tab of last time, coming back at launch.
    Restoring,
}

impl Why {
    pub fn word(self) -> &'static str {
        match self {
            Why::Attaching => "connecting…",
            Why::Restoring => "restoring…",
        }
    }

    pub fn sentence(self, url: &SessionUrl) -> String {
        match self {
            Why::Attaching => format!("attaching to {}…", url.describe()),
            Why::Restoring => format!("restoring {}…", url.describe()),
        }
    }
}

/// What names a tab: a host and a label. Identity is the session's own
/// and can change under a tab -- a session that was gone is made again
/// with the same label and a new id, and a window holds the one it asked
/// with until its link lands -- so what a tab is doing is kept by the
/// name, not by the identity.
type Key = (String, String, String);

fn key(u: &SessionUrl) -> Key {
    (u.provider.clone(), u.arg.clone(), u.session.clone())
}

/// One tab a session: the same host and label, or the same identity
/// under a label one side has not heard about yet. Everything the pool
/// holds is matched this way, so a window holding an identity the
/// session has left behind still finds its own tab, its own parked link
/// and its own state -- it does not make a second of any of them.
pub fn one_session(a: &SessionUrl, b: &SessionUrl) -> bool {
    a == b || key(a) == key(b)
}

/// The tabs the bar shows: the order, with the session the window shows
/// named as the window knows it (its label may have changed since the
/// order was written), and the window's own session added when the order
/// has no tab for it at all.
fn tab_list(order: &[SessionUrl], current: &SessionUrl) -> Vec<SessionUrl> {
    let mut out: Vec<SessionUrl> = order.iter().map(|u| if *u == *current { current.clone() } else { u.clone() }).collect();
    if !out.iter().any(|u| one_session(u, current)) {
        out.push(current.clone());
    }
    out
}

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
    /// The sessions a window has settled on, the most recent first:
    /// where it stopped, not where ctrl-tab passed through (an
    /// application switcher's order). `by_recency` follows it, so the
    /// next ctrl-tab after settling goes back to the session left.
    settled: Vec<SessionUrl>,
    /// What each tab is doing, for the bar: absent is up.
    state: HashMap<Key, Tab>,
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
        cx.set_global(Pool { parked: HashMap::new(), order, settled: Vec::new(), state: HashMap::new(), wake });
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
        if cx.try_global::<Pool>().is_none() {
            return;
        }
        // its tab, whatever name that tab was made under (the label may
        // have changed; the session may have been made again)
        Self::claim(cx, url);
        cx.global_mut::<Pool>().state.insert(key(url), Tab::Up);
    }

    /// The tabs, one URL a line, for next time.
    fn tabs_file() -> std::path::PathBuf {
        crate::shell::state_file().with_file_name("open-sessions")
    }

    /// Move a tab (dragged) before `before` in the order, or to the end;
    /// the order is saved, as it is what the next launch restores.
    pub fn move_tab(cx: &mut App, url: &SessionUrl, before: Option<&SessionUrl>) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let mut order = pool.order.clone();
        order.retain(|u| u != url);
        let at = before.and_then(|b| order.iter().position(|u| u == b)).unwrap_or(order.len());
        order.insert(at, url.clone());
        if order != pool.order {
            let pool = cx.global_mut::<Pool>();
            pool.order = order;
            pool.save_tabs();
        }
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
            if shown.iter().any(|s| *s == url) {
                continue; // the window has it: its tab is that one
            }
            if shown.iter().any(|s| one_session(s, &url)) {
                // the same label on the same host under another id: last
                // time's, before a restart. The window's tab is the
                // session now, and this entry is a tab of a session that
                // is not there -- it goes, or the bar would show both
                crate::shell::log_line(&format!("tab {url}: last time's id for a session the window has; forgotten"));
                Self::let_go(cx, &url);
                continue;
            }
            // the tab is in the bar from this moment, saying what it is
            // doing: waiting for the link is not a reason to hide it
            Self::start(cx, &url, Why::Restoring, true, Vec::new());
        }
    }

    /// Bring a tab up: the link is made on a thread, and the tab says
    /// what is happening until it lands. What lands goes to the window
    /// when the window is waiting on that tab, and is parked otherwise,
    /// so switching away from a tab still connecting leaves it coming.
    /// `existing` attaches to a session that must be there already (a
    /// tab of last time); else one of that name is made if it is gone.
    /// A tab already coming up is left to come.
    pub fn start(cx: &mut App, url: &SessionUrl, why: Why, existing: bool, files: Vec<String>) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        if matches!(pool.state.get(&key(url)), Some(Tab::Coming(_))) {
            return;
        }
        let wake = pool.wake.clone();
        // its tab, made now if the app has none for it; the tab it has
        // keeps the identity it has -- this url's may be the stale one,
        // and a tab with a link must not be renamed away from it
        let k = Self::ensure(cx, url);
        cx.global_mut::<Pool>().state.insert(k, Tab::Coming(why));
        crate::shell::log_line(&format!("tab {url}: {}", why.word()));
        let (u, w) = (url.clone(), wake);
        let connecting = cx.background_executor().spawn(async move {
            if existing {
                crate::app::Acme::connect_existing_targeted(&u, w)
            } else {
                crate::app::Acme::connect_blocking(&u, w)
            }
        });
        let url = url.clone();
        cx.spawn(async move |cx| {
            let r = connecting.await;
            let _ = cx.update(|cx| Pool::landed(cx, url, r, files));
        })
        .detach();
    }

    /// A tab's link has come, or has not. The window waiting on that tab
    /// takes it; otherwise it is parked. A session that is gone takes
    /// its tab with it (nothing is there to come back to); any other
    /// failure leaves the tab where it is, saying why.
    fn landed(cx: &mut App, url: SessionUrl, r: std::io::Result<(Link, Log, Node, WakeTarget)>, files: Vec<String>) {
        match r {
            Ok((link, log, node, target)) => {
                // the session says which it is: the tab takes that name
                let real = crate::app::identified(&url, &node);
                Self::claim(cx, &real);
                Self::set(cx, &real, Tab::Up);
                // the window that asked for the link takes it, whatever
                // identity came back: a session that was gone is made
                // again under a new one, and the window -- still holding
                // the old one -- is the window waiting for it
                match Self::waiting_on(cx, &url).or_else(|| Self::waiting_on(cx, &real)) {
                    Some(h) => {
                        crate::shell::log_line(&format!("tab {real}: attached, shown"));
                        let bad = h
                            .update(cx, |acme, window, cx| {
                                let bad = acme.adopt(link, log, node, target, &real, files, window).err().map(|e| e.to_string());
                                if let Some(why) = &bad {
                                    acme.wait_failed(why);
                                }
                                cx.notify();
                                bad
                            })
                            .ok()
                            .flatten();
                        if let Some(why) = bad {
                            Self::set(cx, &real, Tab::Down(why));
                        }
                    }
                    // the tab was closed while its link was being made:
                    // the link goes with it
                    None if !Self::has(cx, &real) => {
                        crate::shell::log_line(&format!("tab {real}: closed while coming; the link goes"));
                        let mut link = link;
                        link.close();
                    }
                    None => {
                        crate::shell::log_line(&format!("tab {real}: attached, parked"));
                        let parked = Parked { link, log, node, url: real, target, previews: Vec::new(), live: HashMap::new(), snarfouts: Vec::new(), pending_goto: None, parked_at: Instant::now() };
                        Pool::park(cx, parked);
                    }
                }
            }
            Err(e) => {
                let why = e.to_string();
                crate::shell::log_line(&format!("tab {url}: {why}"));
                match Self::waiting_on(cx, &url) {
                    Some(h) => {
                        Self::set(cx, &url, Tab::Down(why.clone()));
                        let _ = h.update(cx, |acme, _, cx| {
                            acme.wait_failed(&why);
                            cx.notify();
                        });
                    }
                    // the session is not there any more: its tab goes
                    // with it, there being nothing to come back to
                    None if why.starts_with("no session") => Self::let_go(cx, &url),
                    None => Self::set(cx, &url, Tab::Down(why)),
                }
            }
        }
    }

    /// The window sitting on this tab with nothing attached: the one a
    /// link that lands belongs to.
    fn waiting_on(cx: &App, url: &SessionUrl) -> Option<gpui::WindowHandle<crate::app::Acme>> {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<crate::app::Acme>())
            .find(|h| h.read(cx).is_ok_and(|a| a.url == *url && !a.connected))
    }

    /// Whether the app still has a tab for this session.
    fn has(cx: &App, url: &SessionUrl) -> bool {
        cx.try_global::<Pool>().is_some_and(|p| p.order.iter().any(|u| one_session(u, url)))
    }

    /// What a tab is doing. One nothing has been said about is up if it
    /// is parked here, and down if it is not: a tab in the order with no
    /// link is a tab with no link, whatever nobody has said about it.
    pub fn tab(cx: &App, url: &SessionUrl) -> Tab {
        let Some(pool) = cx.try_global::<Pool>() else { return Tab::Up };
        if let Some(t) = pool.state.get(&key(url)) {
            return t.clone();
        }
        if pool.parked.values().any(|p| p.url == *url) {
            Tab::Up
        } else {
            Tab::Down("not attached".into())
        }
    }

    fn set(cx: &mut App, url: &SessionUrl, t: Tab) {
        if cx.try_global::<Pool>().is_none() {
            return;
        }
        cx.global_mut::<Pool>().state.insert(key(url), t);
    }

    /// The tab for this session: the one the app has, or a new one.
    /// Answers the name its state is kept under. Nothing is renamed: the
    /// asking url may hold an identity the session has left behind.
    fn ensure(cx: &mut App, url: &SessionUrl) -> Key {
        let pool = cx.global_mut::<Pool>();
        match pool.order.iter().position(|u| one_session(u, url)) {
            Some(at) => key(&pool.order[at]),
            None => {
                pool.order.push(url.clone());
                pool.save_tabs();
                key(url)
            }
        }
    }

    /// This is the session, as the session itself says: its tab takes
    /// that name and that identity, whatever it was made under. Only for
    /// a url that came off a link -- the identity it carries is the one
    /// that answered.
    fn claim(cx: &mut App, url: &SessionUrl) {
        if cx.try_global::<Pool>().is_none() {
            return;
        }
        let pool = cx.global_mut::<Pool>();
        let Some(at) = pool.order.iter().position(|u| one_session(u, url)) else {
            pool.order.push(url.clone());
            pool.save_tabs();
            return;
        };
        let old = pool.order[at].clone();
        if old.id == url.id && old.session == url.session {
            return;
        }
        crate::shell::log_line(&format!("tab {old} is {url}"));
        pool.order[at] = url.clone();
        if key(&old) != key(url) {
            if let Some(t) = pool.state.remove(&key(&old)) {
                pool.state.insert(key(url), t);
            }
        }
        pool.save_tabs();
    }

    /// Whether a parked session is fenced: another client leads it, and
    /// nothing it is told there takes.
    pub fn fenced(cx: &App, url: &SessionUrl) -> bool {
        let Some(pool) = cx.try_global::<Pool>() else { return false };
        pool.parked
            .values()
            .find(|p| one_session(&p.url, url))
            .is_some_and(|p| p.log.lease(Shard::Layout).is_some_and(|l| l.holder != p.node.attachment || l.released.is_some()))
    }

    /// The tabs, in first-shown order: every one the app has, whatever
    /// its link is doing — shown, parked, still coming up, or down. The
    /// one the window shows is named as the window knows it (its label
    /// may have changed since the order was written).
    pub fn tabs(cx: &App, current: &SessionUrl) -> Vec<SessionUrl> {
        let Some(pool) = cx.try_global::<Pool>() else { return vec![current.clone()] };
        tab_list(&pool.order, current)
    }

    /// Let a parked session go: its link closes, its tab with it.
    pub fn let_go(cx: &mut App, url: &SessionUrl) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let keys: Vec<String> = pool.parked.iter().filter(|(_, p)| one_session(&p.url, url)).map(|(k, _)| k.clone()).collect();
        let pool = cx.global_mut::<Pool>();
        for k in keys {
            if let Some(mut p) = pool.parked.remove(&k) {
                p.link.close();
                crate::shell::log_line(&format!("parked {k} let go"));
            }
        }
        pool.order.retain(|u| !one_session(u, url));
        pool.state.remove(&key(url));
        pool.save_tabs();
    }

    /// Park a session: its wake comes here from now on.
    /// A parked session's link, for a tab's status card: the last log
    /// round trip in ms, and how long since the daemon last answered a
    /// heartbeat, when it has.
    pub fn link_status(cx: &App, url: &SessionUrl) -> Option<(Option<u64>, Option<std::time::Duration>)> {
        let pool = cx.try_global::<Pool>()?;
        let p = pool.parked.values().find(|p| one_session(&p.url, url))?;
        Some((p.link.ack_ms, p.link.last_pong.map(|t| t.elapsed())))
    }

    /// Whether a parked session has notifications waiting, for its tab.
    pub fn notified(cx: &App, url: &SessionUrl) -> bool {
        cx.try_global::<Pool>().and_then(|pool| pool.parked.values().find(|p| one_session(&p.url, url))).is_some_and(|p| p.node.notifications().next().is_some())
    }

    /// Each parked session's notifications, oldest first: the window
    /// each is about, and the entry that raised it.
    pub fn notifications(cx: &App) -> Vec<(SessionUrl, Vec<(WindowId, Seq)>)> {
        cx.try_global::<Pool>()
            .map(|pool| pool.parked.values().map(|p| (p.url.clone(), p.node.notifications().map(|n| (n.window, n.at)).collect())).collect())
            .unwrap_or_default()
    }

    /// Each parked session and its replica, in the tabs' order, for what
    /// looks across the tabs (⌘⇧P).
    pub fn parked_nodes(cx: &App) -> Vec<(SessionUrl, &Node)> {
        let Some(pool) = cx.try_global::<Pool>() else { return Vec::new() };
        pool.order.iter().filter_map(|u| pool.parked.values().find(|p| one_session(&p.url, u)).map(|p| (p.url.clone(), &p.node))).collect()
    }

    /// The theme changed: every parked link tells its daemon the colours
    /// too, so a session shown later is right from the start.
    pub fn send_config(cx: &mut App) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let term = crate::theme::term_colors();
        for p in pool.parked.values() {
            p.link.send(&apex_server::proto::ClientMsg::ClientConfig { term });
        }
    }

    /// The session the window shows, as its link knows it (apex has one
    /// window, so this is one url or none).
    pub fn shown_urls(cx: &App) -> Vec<SessionUrl> {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<crate::app::Acme>())
            .filter_map(|h| h.read(cx).ok().map(|a| a.url.clone()))
            .collect()
    }

    pub fn park(cx: &mut App, p: Parked) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        // the window shows this session already: a second attachment of
        // ours would take the lead from it (the daemon lets the latest
        // UI lead), leaving the window fenced; this link is let go
        if Self::shown_urls(cx).iter().any(|s| one_session(s, &p.url)) {
            crate::shell::log_line(&format!("{}: shown already; not parked", p.url));
            let mut p = p;
            p.link.close();
            return;
        }
        p.target.set(pool.wake.clone());
        let name = p.url.to_string();
        let pool = cx.global_mut::<Pool>();
        // the same session parked twice (whatever its label was): the older leaves
        let same: Vec<String> = pool.parked.iter().filter(|(_, x)| one_session(&x.url, &p.url)).map(|(k, _)| k.clone()).collect();
        for k in same {
            if let Some(mut old) = pool.parked.remove(&k) {
                old.link.close();
            }
        }
        pool.state.insert(key(&p.url), Tab::Up);
        pool.parked.insert(name.clone(), p);
        while pool.parked.len() > CAP {
            let oldest = pool.parked.iter().min_by_key(|(_, p)| p.parked_at).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    if let Some(mut p) = pool.parked.remove(&k) {
                        crate::shell::log_line(&format!("parked {k} let go: {CAP} is enough"));
                        // the tab stays: it is the user's, not the link's.
                        // Shown again, it is attached again
                        pool.state.insert(key(&p.url), Tab::Down(format!("let go: {CAP} sessions are as many as stay attached")));
                        p.link.close();
                    }
                }
                None => break,
            }
        }
        crate::shell::log_line(&format!("parked {name}"));
    }

    /// The session parked most recently: the one to switch back to.
    pub fn most_recent(cx: &App) -> Option<SessionUrl> {
        Pool::by_recency(cx).into_iter().next()
    }

    /// A window has settled on `url` (shown it, and no ctrl-tab walk is
    /// passing through): the most recent of the settled.
    pub fn note_settled(cx: &mut App, url: &SessionUrl) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        if pool.settled.first() == Some(url) {
            return;
        }
        let pool = cx.global_mut::<Pool>();
        pool.settled.retain(|u| u != url);
        pool.settled.insert(0, url.clone());
    }

    /// The parked sessions, the most recently settled on first; ones
    /// never settled on (attached again at launch, say) after those, the
    /// most recently parked first.
    pub fn by_recency(cx: &App) -> Vec<SessionUrl> {
        let Some(pool) = cx.try_global::<Pool>() else { return Vec::new() };
        let mut v: Vec<(usize, std::cmp::Reverse<Instant>, SessionUrl)> = pool
            .parked
            .values()
            .map(|p| (pool.settled.iter().position(|u| *u == p.url).unwrap_or(usize::MAX), std::cmp::Reverse(p.parked_at), p.url.clone()))
            .collect();
        v.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        v.into_iter().map(|(_, _, u)| u).collect()
    }

    /// Take a parked session to show it.
    pub fn take(cx: &mut App, url: &SessionUrl) -> Option<Parked> {
        let key = cx.try_global::<Pool>()?.parked.iter().find(|(_, p)| one_session(&p.url, url)).map(|(k, _)| k.clone())?;
        let p = cx.global_mut::<Pool>().parked.remove(&key);
        if let Some(p) = &p {
            // by the session, so the name in the log is the session's own
            crate::shell::log_line(&format!("unparked {}", p.url));
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
        for name in gone {
            crate::shell::log_line(&format!("parked {name}: link ended"));
            if let Some(p) = pool.parked.remove(&name) {
                // the tab keeps its place: shown again, it attaches again
                pool.state.insert(key(&p.url), Tab::Down("the link ended".into()));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn url(label: &str, id: &str) -> SessionUrl {
        SessionUrl::local(label).with_id(id)
    }

    #[test]
    fn what_makes_two_urls_one_session() {
        // a host and a label name a session: the identity under it can
        // change (one that was gone is made again with the same label)
        assert!(one_session(&url("work", "old"), &url("work", "new")));
        // and a label can change under an identity (a rename one side
        // has not heard about yet)
        let mut renamed = url("work", "same");
        renamed.session = "toil".into();
        assert!(one_session(&url("work", "same"), &renamed));
        // but two names and two identities are two sessions
        assert!(!one_session(&url("work", "a"), &url("play", "b")));
        // and a host of its own is a session of its own
        let mut elsewhere = url("work", "a");
        elsewhere.provider = "ssh".into();
        elsewhere.arg = "box".into();
        assert!(!one_session(&url("work", "b"), &elsewhere));
    }

    #[test]
    fn one_tab_a_session_whatever_identity_each_side_holds() {
        // the pool has the session as the daemon named it; the window is
        // still holding the identity it asked with (the session it asked
        // for was gone, and one of that label was made again)
        let order = vec![url("work", "new")];
        assert_eq!(tab_list(&order, &url("work", "old")), order, "one session, one tab");
        // the window's own session, which the pool has no tab for
        let out = tab_list(&order, &url("side", "c"));
        assert_eq!(out.len(), 2, "a session the pool has no tab for is a tab all the same");
        // the shown session is named as the window knows it: the label
        // it has now, not the one the order was written with
        let mut renamed = url("work", "new");
        renamed.session = "toil".into();
        assert_eq!(tab_list(&order, &renamed)[0].session, "toil");
    }
}
