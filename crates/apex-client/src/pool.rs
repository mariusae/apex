//! The tabs, and the sessions behind them.
//!
//! A tab is the app's own: it is made when the user makes one, brought
//! back at launch, and goes only when the user closes it. What it names
//! is a session somewhere, and a session's own identity is not the app's
//! to lean on -- end one, attach to its label again, and the host makes
//! a new session with a new id, so two attaches can disagree about which
//! session a name means. The app therefore names its tabs itself
//! (`TabId`, made here and never changing) and keeps the session's own
//! name beside it (`Tab::url`), filled in and corrected as attaches say
//! more. Everything -- a window, a key, a click, a notification -- means
//! a tab by its id, and only `Pool::open` turns a url into one.
//!
//! A session a window switched away from stays attached, its link tended
//! here, so showing it again is instant: parked, under the same tab.
//! Parked sessions keep their lead: the daemon goes on forwarding tools'
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

/// A tab, as the app names it: made here, never reused, and meaning
/// nothing anywhere else. What the session on the other end calls itself
/// is its own affair, and can change under the tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tab {}", self.0)
    }
}

/// A tab: the app's handle on a session, where that session is as far as
/// the app knows, and what the link under it is doing.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: TabId,
    /// The host and the label always; the session's own identity once an
    /// attach has said what it is.
    pub url: SessionUrl,
    pub state: State,
}

/// What a tab is doing, when it is not simply up. A tab still connecting
/// keeps its place in the bar, and one whose link has gone says so
/// instead of vanishing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// A link is being made, and why.
    Coming(Why),
    /// Attached: shown in the window, or parked here.
    Up,
    /// No link, and why (the tab's card says it in full).
    Down(String),
}

impl State {
    pub fn word(&self) -> Option<&str> {
        match self {
            State::Coming(why) => Some(why.word()),
            State::Up => None,
            State::Down(_) => Some("offline"),
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

/// Whether two urls name one session: the same identity, or -- while
/// either side has none, or holds one the session has left behind -- the
/// same label on the same host. Only `Pool::open` asks this, where a url
/// has to become a tab; everything else goes by `TabId`.
fn one_session(a: &SessionUrl, b: &SessionUrl) -> bool {
    a == b || (a.provider == b.provider && a.arg == b.arg && a.session == b.session)
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
    /// Every tab the app has, in the order the bar shows them.
    tabs: Vec<Tab>,
    /// The sessions attached but not shown, by the tab each belongs to.
    parked: HashMap<TabId, Parked>,
    /// The tabs a window has settled on, the most recent first: where it
    /// stopped, not where ctrl-tab passed through (an application
    /// switcher's order). `by_recency` follows it, so the next ctrl-tab
    /// after settling goes back to the tab left.
    settled: Vec<TabId>,
    /// The next tab's id.
    next: u64,
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
        // the tabs of last time, in their order: made now, with no link
        // under them until `restore` makes one, so the bar is what it was
        // from the first frame
        let text = std::fs::read_to_string(Self::tabs_file()).unwrap_or_default();
        let mut next = 0;
        let tabs: Vec<Tab> = text
            .lines()
            .filter_map(SessionUrl::parse)
            .filter(|u| u.id.is_some())
            .map(|url| {
                next += 1;
                Tab { id: TabId(next), url, state: State::Down("not attached".into()) }
            })
            .collect();
        cx.set_global(Pool { tabs, parked: HashMap::new(), settled: Vec::new(), next, wake });
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                cx.update(|cx| Pool::tend(cx));
            }
        })
        .detach();
    }

    /// The tab for this session: the one the app already has for it, or
    /// a new one. The only place a url becomes a tab -- a name from the
    /// picker, a launch target, a place in another session -- and so the
    /// only place two urls are ever weighed against each other.
    pub fn open(cx: &mut App, url: &SessionUrl) -> TabId {
        if cx.try_global::<Pool>().is_none() {
            return TabId(0);
        }
        let pool = cx.global_mut::<Pool>();
        if let Some(t) = pool.tabs.iter().find(|t| one_session(&t.url, url)) {
            return t.id;
        }
        pool.next += 1;
        let id = TabId(pool.next);
        pool.tabs.push(Tab { id, url: url.clone(), state: State::Down("not attached".into()) });
        pool.save_tabs();
        crate::shell::log_line(&format!("{id} is {url}"));
        id
    }

    /// The tabs, in the order the bar shows them.
    pub fn tabs(cx: &App) -> Vec<Tab> {
        cx.try_global::<Pool>().map(|p| p.tabs.clone()).unwrap_or_default()
    }

    /// One tab, while the app still has it.
    pub fn tab(cx: &App, id: TabId) -> Option<Tab> {
        cx.try_global::<Pool>()?.tabs.iter().find(|t| t.id == id).cloned()
    }

    /// Where a tab's session is, as far as the app knows.
    pub fn url_of(cx: &App, id: TabId) -> Option<SessionUrl> {
        Self::tab(cx, id).map(|t| t.url)
    }

    /// What a tab is doing.
    pub fn state(cx: &App, id: TabId) -> State {
        Self::tab(cx, id).map(|t| t.state).unwrap_or(State::Up)
    }

    fn set(cx: &mut App, id: TabId, s: State) {
        if cx.try_global::<Pool>().is_none() {
            return;
        }
        if let Some(t) = cx.global_mut::<Pool>().tabs.iter_mut().find(|t| t.id == id) {
            t.state = s;
        }
    }

    /// The session a tab holds is this one, as the session itself says.
    /// Only for a url that came off a link, or off a window that has
    /// one: the identity it carries is the one that answered.
    pub fn named(cx: &mut App, id: TabId, url: &SessionUrl) {
        if cx.try_global::<Pool>().is_none() {
            return;
        }
        let pool = cx.global_mut::<Pool>();
        let Some(t) = pool.tabs.iter_mut().find(|t| t.id == id) else { return };
        if t.url == *url && t.url.session == url.session && t.url.id == url.id {
            return;
        }
        crate::shell::log_line(&format!("{id}: {} is {url}", t.url));
        t.url = url.clone();
        pool.save_tabs();
    }

    /// A window is showing this tab, attached: its session is what the
    /// window says it is, and the tab is up.
    pub fn note_open(cx: &mut App, id: TabId, url: &SessionUrl) {
        Self::named(cx, id, url);
        Self::set(cx, id, State::Up);
    }

    /// The tabs, one URL a line, for next time.
    fn tabs_file() -> std::path::PathBuf {
        crate::shell::state_file().with_file_name("open-sessions")
    }

    /// Move a tab (dragged) before `before` in the order, or to the end;
    /// the order is saved, as it is what the next launch restores.
    pub fn move_tab(cx: &mut App, id: TabId, before: Option<TabId>) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let Some(at) = pool.tabs.iter().position(|t| t.id == id) else { return };
        let pool = cx.global_mut::<Pool>();
        let tab = pool.tabs.remove(at);
        let to = before.and_then(|b| pool.tabs.iter().position(|t| t.id == b)).unwrap_or(pool.tabs.len());
        pool.tabs.insert(to, tab);
        pool.save_tabs();
    }

    fn save_tabs(&self) {
        let p = Self::tabs_file();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let text: String = self.tabs.iter().map(|t| format!("{}\n", t.url)).collect();
        let _ = std::fs::write(p, text);
    }

    /// The tabs of last time, attached again in the background: each as
    /// it was named then, by identity, so one whose session is gone is
    /// dropped rather than made anew. The tab a window already holds is
    /// the window's own to attach.
    pub fn restore(cx: &mut App) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let shown = Self::shown_tabs(cx);
        let mine: Vec<TabId> = pool.tabs.iter().map(|t| t.id).filter(|id| !shown.contains(id)).collect();
        crate::shell::log_line(&format!("restoring {} tab(s) of last time; {} shown already", mine.len(), shown.len()));
        for id in mine {
            // the tab is in the bar from this moment, saying what it is
            // doing: waiting for the link is not a reason to hide it
            Self::start(cx, id, Why::Restoring, true, Vec::new());
        }
    }

    /// Bring a tab up: the link is made on a thread, and the tab says
    /// what is happening until it lands. What lands goes to the window
    /// when the window is on that tab, and is parked otherwise, so
    /// switching away from a tab still connecting leaves it coming.
    /// `existing` attaches to a session that must be there already (a
    /// tab of last time); else one of that name is made if it is gone.
    /// A tab already coming up is left to come.
    pub fn start(cx: &mut App, id: TabId, why: Why, existing: bool, files: Vec<String>) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        let Some(tab) = pool.tabs.iter().find(|t| t.id == id) else { return };
        if matches!(tab.state, State::Coming(_)) {
            return;
        }
        let (url, wake) = (tab.url.clone(), pool.wake.clone());
        Self::set(cx, id, State::Coming(why));
        crate::shell::log_line(&format!("{id} ({url}): {}", why.word()));
        let (u, w) = (url.clone(), wake);
        let connecting = cx.background_executor().spawn(async move {
            if existing {
                crate::app::Acme::connect_existing_targeted(&u, w)
            } else {
                crate::app::Acme::connect_blocking(&u, w)
            }
        });
        cx.spawn(async move |cx| {
            let r = connecting.await;
            let _ = cx.update(|cx| Pool::landed(cx, id, url, r, files));
        })
        .detach();
    }

    /// A tab's link has come, or has not. The window on that tab takes
    /// it; otherwise it is parked, under that tab. A session that is
    /// gone takes its tab with it (nothing is there to come back to);
    /// any other failure leaves the tab where it is, saying why.
    fn landed(cx: &mut App, id: TabId, asked: SessionUrl, r: std::io::Result<(Link, Log, Node, WakeTarget)>, files: Vec<String>) {
        if Self::tab(cx, id).is_none() {
            // closed while its link was being made: the link goes with it
            if let Ok((mut link, ..)) = r {
                crate::shell::log_line(&format!("{id}: closed while coming; the link goes"));
                link.close();
            }
            return;
        }
        match r {
            Ok((link, log, node, target)) => {
                // whatever session answered, it is this tab's session
                let real = crate::app::identified(&asked, &node);
                Self::named(cx, id, &real);
                Self::set(cx, id, State::Up);
                match Self::waiting_on(cx, id) {
                    Some(h) => {
                        crate::shell::log_line(&format!("{id} ({real}): attached, shown"));
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
                            Self::set(cx, id, State::Down(why));
                        }
                    }
                    None => {
                        crate::shell::log_line(&format!("{id} ({real}): attached, parked"));
                        let parked = Parked { link, log, node, url: real, target, previews: Vec::new(), live: HashMap::new(), snarfouts: Vec::new(), pending_goto: None, parked_at: Instant::now() };
                        Pool::park(cx, id, parked);
                    }
                }
            }
            Err(e) => {
                let why = e.to_string();
                crate::shell::log_line(&format!("{id} ({asked}): {why}"));
                match Self::waiting_on(cx, id) {
                    Some(h) => {
                        Self::set(cx, id, State::Down(why.clone()));
                        let _ = h.update(cx, |acme, _, cx| {
                            acme.wait_failed(&why);
                            cx.notify();
                        });
                    }
                    // the session is not there any more: its tab goes
                    // with it, there being nothing to come back to
                    None if why.starts_with("no session") => Self::let_go(cx, id),
                    None => Self::set(cx, id, State::Down(why)),
                }
            }
        }
    }

    /// The window sitting on this tab with nothing attached: the one a
    /// link that lands belongs to.
    fn waiting_on(cx: &App, id: TabId) -> Option<gpui::WindowHandle<crate::app::Acme>> {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<crate::app::Acme>())
            .find(|h| h.read(cx).is_ok_and(|a| a.tab == id && !a.connected))
    }

    /// Whether a tab's parked session is fenced: another client leads it,
    /// and nothing it is told there takes.
    pub fn fenced(cx: &App, id: TabId) -> bool {
        let Some(pool) = cx.try_global::<Pool>() else { return false };
        pool.parked.get(&id).is_some_and(|p| p.log.lease(Shard::Layout).is_some_and(|l| l.holder != p.node.attachment || l.released.is_some()))
    }

    /// Let a tab go: its link closes, its place in the bar with it.
    pub fn let_go(cx: &mut App, id: TabId) {
        if cx.try_global::<Pool>().is_none() {
            return;
        }
        let pool = cx.global_mut::<Pool>();
        if let Some(mut p) = pool.parked.remove(&id) {
            p.link.close();
            crate::shell::log_line(&format!("{id} ({}) let go", p.url));
        }
        pool.tabs.retain(|t| t.id != id);
        pool.settled.retain(|s| *s != id);
        pool.save_tabs();
    }

    /// A parked session's link, for a tab's status card: the last log
    /// round trip in ms, and how long since the daemon last answered a
    /// heartbeat, when it has.
    pub fn link_status(cx: &App, id: TabId) -> Option<(Option<u64>, Option<std::time::Duration>)> {
        let p = cx.try_global::<Pool>()?.parked.get(&id)?;
        Some((p.link.ack_ms, p.link.last_pong.map(|t| t.elapsed())))
    }

    /// Whether a tab's parked session has notifications waiting.
    pub fn notified(cx: &App, id: TabId) -> bool {
        cx.try_global::<Pool>().and_then(|pool| pool.parked.get(&id)).is_some_and(|p| p.node.notifications().next().is_some())
    }

    /// Each parked session's notifications, oldest first: the tab, the
    /// window each is about, and the entry that raised it.
    pub fn notifications(cx: &App) -> Vec<(TabId, Vec<(WindowId, Seq)>)> {
        cx.try_global::<Pool>()
            .map(|pool| pool.parked.iter().map(|(id, p)| (*id, p.node.notifications().map(|n| (n.window, n.at)).collect())).collect())
            .unwrap_or_default()
    }

    /// Each parked session and its replica, in the tabs' order, for what
    /// looks across the tabs (⌘⇧P).
    pub fn parked_nodes(cx: &App) -> Vec<(TabId, SessionUrl, &Node)> {
        let Some(pool) = cx.try_global::<Pool>() else { return Vec::new() };
        pool.tabs.iter().filter_map(|t| pool.parked.get(&t.id).map(|p| (t.id, p.url.clone(), &p.node))).collect()
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

    /// The tab the window shows (apex has one window, so this is one tab
    /// or none).
    pub fn shown_tabs(cx: &App) -> Vec<TabId> {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<crate::app::Acme>())
            .filter_map(|h| h.read(cx).ok().map(|a| a.tab))
            .collect()
    }

    /// Park a tab's session: its wake comes here from now on.
    pub fn park(cx: &mut App, id: TabId, p: Parked) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        // the window shows this tab already: a second attachment of ours
        // would take the lead from it (the daemon lets the latest UI
        // lead), leaving the window fenced; this link is let go
        if Self::shown_tabs(cx).contains(&id) {
            crate::shell::log_line(&format!("{id} ({}): shown already; not parked", p.url));
            let mut p = p;
            p.link.close();
            return;
        }
        p.target.set(pool.wake.clone());
        let url = p.url.clone();
        let pool = cx.global_mut::<Pool>();
        // the same tab parked twice: the older link leaves
        if let Some(mut old) = pool.parked.insert(id, p) {
            old.link.close();
        }
        if let Some(t) = pool.tabs.iter_mut().find(|t| t.id == id) {
            t.state = State::Up;
        }
        while pool.parked.len() > CAP {
            let oldest = pool.parked.iter().min_by_key(|(_, p)| p.parked_at).map(|(k, _)| *k);
            match oldest {
                Some(k) => {
                    if let Some(mut p) = pool.parked.remove(&k) {
                        crate::shell::log_line(&format!("{k} ({}) let go: {CAP} is enough", p.url));
                        // the tab stays: it is the user's, not the link's.
                        // Shown again, it is attached again
                        if let Some(t) = pool.tabs.iter_mut().find(|t| t.id == k) {
                            t.state = State::Down(format!("let go: {CAP} sessions are as many as stay attached"));
                        }
                        p.link.close();
                    }
                }
                None => break,
            }
        }
        crate::shell::log_line(&format!("{id} ({url}) parked"));
    }

    /// The tab parked most recently: the one to switch back to.
    pub fn most_recent(cx: &App) -> Option<TabId> {
        Pool::by_recency(cx).into_iter().next()
    }

    /// A window has settled on a tab (shown it, and no ctrl-tab walk is
    /// passing through): the most recent of the settled.
    pub fn note_settled(cx: &mut App, id: TabId) {
        let Some(pool) = cx.try_global::<Pool>() else { return };
        if pool.settled.first() == Some(&id) {
            return;
        }
        let pool = cx.global_mut::<Pool>();
        pool.settled.retain(|s| *s != id);
        pool.settled.insert(0, id);
    }

    /// The parked tabs, the most recently settled on first; ones never
    /// settled on (attached again at launch, say) after those, the most
    /// recently parked first.
    pub fn by_recency(cx: &App) -> Vec<TabId> {
        let Some(pool) = cx.try_global::<Pool>() else { return Vec::new() };
        let mut v: Vec<(usize, std::cmp::Reverse<Instant>, TabId)> =
            pool.parked.iter().map(|(id, p)| (pool.settled.iter().position(|s| s == id).unwrap_or(usize::MAX), std::cmp::Reverse(p.parked_at), *id)).collect();
        v.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        v.into_iter().map(|(_, _, id)| id).collect()
    }

    /// Take a tab's parked session, to show it.
    pub fn take(cx: &mut App, id: TabId) -> Option<Parked> {
        cx.try_global::<Pool>()?;
        let p = cx.global_mut::<Pool>().parked.remove(&id);
        if let Some(p) = &p {
            crate::shell::log_line(&format!("{id} ({}) unparked", p.url));
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
        for (id, p) in pool.parked.iter_mut() {
            if !p.link.poll(&mut p.node, &mut p.log) {
                gone.push(*id);
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
            for (ask, verb, args) in asks {
                let result = match verb.as_str() {
                    "open" => client_do("open", &args).map(|_| None),
                    other => Err(format!("{other}: the session is parked, nobody sees it")),
                };
                p.link.send(&ClientMsg::Applied { id: ask, result });
            }
            p.link.io.clear(); // a parked session's previews are over
            p.link.flush(&p.log);
            let _ = p.node.catch_up(&p.log);
        }
        // a parked session's label as its metalog has it now: renamed
        // from anywhere, the tab says the name it goes by
        let named: Vec<(TabId, String)> = pool.parked.iter().map(|(id, p)| (*id, p.node.state.meta.label.clone())).filter(|(_, l)| !l.is_empty()).collect();
        for (id, label) in named {
            if let Some(t) = pool.tabs.iter_mut().find(|t| t.id == id) {
                if t.url.session != label {
                    t.url.session = label;
                }
            }
        }
        for id in gone {
            if let Some(p) = pool.parked.remove(&id) {
                crate::shell::log_line(&format!("{id} ({}): link ended", p.url));
                // the tab keeps its place: shown again, it attaches again
                if let Some(t) = pool.tabs.iter_mut().find(|t| t.id == id) {
                    t.state = State::Down("the link ended".into());
                }
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
    fn what_makes_a_url_the_tab_the_app_already_has() {
        // a host and a label name a session: the identity under it can
        // change (one that was gone is made again with the same label),
        // and it is the same tab
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
}
