//! Restarting this machine's apex server, as Manifold offers to: when an
//! attach finds the server here is from another version (which this apex
//! cannot talk to), once a launch; and whenever asked (Apex ▸ Restart
//! Server…). The server holds the sessions, so it goes only when the
//! user says so, told what goes with it (`KEPT`): a server that keeps
//! its sessions (`persist`) has them back, the programs in their
//! terminals aside; one from before that ends them. The tabs are the
//! app's and stay; each attaches to its session again, or to a fresh
//! one of its name.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use gpui::{Context, MenuItem, PromptLevel, Window};

use crate::app::Acme;
use crate::pool::{Pool, Why};

static OFFERED: AtomicBool = AtomicBool::new(false);

/// What a restart does to the sessions, said before it is asked for.
/// Which kind the server is cannot be told from here, so both are said.
const KEPT: &str = "A server that keeps its sessions writes them down as it stops and has them back when it starts: their windows, and the changes not saved in them. The programs running in their terminals end; each terminal comes back with a new shell. A server from before Apex kept its sessions ends them, and their changes not saved are lost.";

/// What becomes of the tabs.
const TABS: &str = "Your tabs are kept; each attaches to its session again, or to a fresh one of its name.";

/// The protocol a mismatch error names the server as speaking.
fn their_protocol(why: &str) -> Option<u32> {
    let rest = why.split("speaks apex protocol ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Apex ▸ Restart Server ▸ a host: this machine's server (`dest`
/// `None`), or the daemon on the host at `dest`, named `name`.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = apex, no_json)]
pub struct RestartHost {
    pub dest: Option<String>,
    pub name: String,
}

/// What a host's daemon last said of itself when attached to.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// Nothing yet this launch.
    Unknown,
    /// Its protocol and build id.
    Build(u32, String),
    /// The last try could not reach it.
    Down,
}

/// The hosts attached to (or tried) this launch, this machine's first,
/// and what each daemon last said: Apex ▸ Restart Server's items.
static HOSTS: Mutex<Vec<(Option<String>, String, Said)>> = Mutex::new(Vec::new());

/// What the daemon at `dest` (named `name`) said: whether that changes
/// the menu.
pub fn note(dest: Option<String>, name: &str, said: Said) -> bool {
    let mut hosts = HOSTS.lock().unwrap();
    match hosts.iter_mut().find(|h| h.0 == dest) {
        Some(h) if h.2 == said => false,
        Some(h) => {
            h.2 = said;
            true
        }
        None => {
            hosts.push((dest, name.to_string(), said));
            true
        }
    }
}

/// What an attach's outcome says of the daemon: the protocol and build
/// it gave (`check_build`'s error names them when they are not ours),
/// or that it could not be reached. A daemon that answered with an error
/// of its own (no such session, too many attached) said nothing of its
/// build: `None`.
pub fn said_by(r: &std::io::Result<Option<(u32, String)>>) -> Option<Said> {
    match r {
        Ok(Some((p, id))) => Some(Said::Build(*p, id.clone())),
        Ok(None) => None,
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
            let why = e.to_string();
            let p = their_protocol(&why)?;
            let id = why.split("(build ").nth(1)?.split(')').next()?.to_string();
            Some(Said::Build(p, id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::Other => None,
        Err(_) => Some(Said::Down),
    }
}

/// A host's name in the menu: this machine's, or the host's.
pub fn host_name(url: &apex_server::providers::SessionUrl) -> String {
    if url.is_local() { "This Mac".into() } else { url.arg.clone() }
}

/// How a daemon stands against this app, for the menu.
fn standing(said: &Said) -> String {
    let (ours_p, ours_id) = (apex_server::proto::PROTOCOL, apex_server::BUILD_ID);
    match said {
        Said::Unknown => "not attached".into(),
        Said::Down => "not reachable".into(),
        Said::Build(p, id) if *p != ours_p => format!("another version: protocol {p}, build {id} (this is {ours_p})"),
        Said::Build(p, id) if id != ours_id => format!("another build: {id}, protocol {p}"),
        Said::Build(p, id) => format!("this app's build: {id}, protocol {p}"),
    }
}

/// Apex ▸ Restart Server's items: this machine's server, then each other
/// host's daemon, each saying what it is.
pub fn menu_items() -> Vec<MenuItem> {
    let hosts = HOSTS.lock().unwrap().clone();
    let here = hosts.iter().find(|h| h.0.is_none()).map_or(Said::Unknown, |h| h.2.clone());
    let mut items = vec![MenuItem::action(format!("This Mac — {}…", standing(&here)), RestartHost { dest: None, name: "This Mac".into() })];
    let mut others: Vec<_> = hosts.into_iter().filter(|h| h.0.is_some()).collect();
    others.sort_by(|a, b| a.1.cmp(&b.1));
    if !others.is_empty() {
        items.push(MenuItem::separator());
    }
    for (dest, name, said) in others {
        items.push(MenuItem::action(format!("{name} — {}…", standing(&said)), RestartHost { dest, name }));
    }
    items
}

impl Acme {
    /// Apex ▸ Restart Server ▸ a host: asked first.
    pub fn restart_host_asked(&mut self, h: &RestartHost, window: &mut Window, cx: &mut Context<Self>) {
        match &h.dest {
            None => self.restart_server_asked(window, cx),
            Some(dest) => self.ask_restart_daemon(dest.clone(), h.name.clone(), window, cx),
        }
    }

    /// An attach here met a server of another version: offer, once a
    /// launch, to restart it.
    pub fn offer_restart(&mut self, why: &str, window: &mut Window, cx: &mut Context<Self>) {
        if OFFERED.swap(true, Ordering::Relaxed) {
            return;
        }
        let theirs = their_protocol(why).map(|p| format!("protocol {p}, ")).unwrap_or_default();
        let detail = format!(
            "Apex was updated, but the server on this machine is still the old one ({theirs}this is protocol {}), and this Apex cannot attach to its sessions until it is restarted.\n\n{KEPT} {TABS}",
            apex_server::proto::PROTOCOL
        );
        self.ask_restart("Apex's server is from another version", &detail, "Not Now", window, cx);
    }

    /// Apex ▸ Restart Server…: asked first, saying what goes.
    pub fn restart_server_asked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unsaved = self.local_unsaved(cx);
        let lost = match unsaved {
            0 => String::new(),
            1 => " One window has changes not saved.".to_string(),
            n => format!(" {n} windows have changes not saved."),
        };
        let detail = format!("{KEPT}{lost} {TABS}");
        self.ask_restart("Restart Apex's server?", &detail, "Cancel", window, cx);
    }

    fn ask_restart(&mut self, message: &str, detail: &str, no: &str, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(PromptLevel::Warning, message, Some(detail), &["Restart Server", no], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |acme, window, cx| acme.restart_server(window, cx));
            }
        })
        .detach();
    }

    /// Windows with changes not saved in the sessions on this machine's
    /// server: this window's and the parked ones'.
    fn local_unsaved(&self, cx: &Context<Self>) -> usize {
        let count = |n: &apex_core::Node| n.state.windows.keys().filter(|w| n.window_unsaved(**w)).count();
        let mut total = if self.url.is_local() && self.connected { count(&self.node) } else { 0 };
        for (_, url, node) in Pool::parked_nodes(cx) {
            if url.is_local() {
                total += count(node);
            }
        }
        total
    }

    /// Restart Daemon on the page of a session that cannot be attached:
    /// asked first, as the menu's Restart Server is, since every session
    /// on that daemon goes with it -- this machine's, or the host's.
    pub fn restart_daemon_asked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.url.dest() {
            None => self.restart_server_asked(window, cx),
            Some(dest) => self.ask_restart_daemon(dest, self.url.arg.clone(), window, cx),
        }
    }

    fn ask_restart_daemon(&mut self, dest: String, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let detail = format!("This is every session on {host}. {KEPT} {TABS}");
        let answer = window.prompt(PromptLevel::Warning, &format!("Restart the daemon on {host}?"), Some(&detail), &["Restart Daemon", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |acme, window, cx| acme.restart_daemon(dest, host, window, cx));
            }
        })
        .detach();
    }

    /// Restart the daemon on the host at `dest` (named `host`): our apex
    /// put there (`deploy`) and its `apex stop` run there, which stops a
    /// daemon of any build; off the UI thread, this window's page saying
    /// so meanwhile when its session is there. Then every tab on that
    /// host is attached again, starting a fresh daemon there.
    pub fn restart_daemon(&mut self, dest: String, host: String, window: &mut Window, cx: &mut Context<Self>) {
        crate::shell::log_line(&format!("restarting the daemon on {host}"));
        let here = self.url.dest().as_deref() == Some(dest.as_str());
        if here {
            self.restarting = true;
            self.retry = None;
            self.wait(&format!("Restarting the daemon on {host}…"));
        }
        let d = dest.clone();
        let stopping = cx.background_executor().spawn(async move {
            apex_server::providers::deploy(&d)?;
            apex_server::providers::run(&d, &format!("{} stop", apex_server::providers::REMOTE_BIN), None)
        });
        cx.spawn_in(window, async move |this, cx| {
            let r = stopping.await;
            let _ = this.update_in(cx, |acme: &mut Acme, window, cx| {
                if here {
                    acme.restarting = false;
                }
                match r {
                    Err(e) if here => acme.wait_failed(&format!("restarting the daemon on {host}: {e}")),
                    Err(e) => acme.notice(&format!("Restarting the daemon on {host}: {e}")),
                    Ok(_) => {
                        crate::shell::log_line(&format!("the daemon on {host} stopped; attaching again"));
                        acme.reattach_on(Some(&dest), window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Every tab on the daemon at `dest` (`None`: this machine's) attached
    /// again: this window's, the parked ones, and other windows'.
    fn reattach_on(&mut self, dest: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let on = |url: &apex_server::providers::SessionUrl| url.dest().as_deref() == dest;
        if on(&self.url) {
            self.reconnect(window, cx);
        }
        let shown = Pool::shown_tabs(cx);
        for t in Pool::tabs(cx) {
            if on(&t.url) && t.id != self.tab && !shown.contains(&t.id) {
                Pool::start(cx, t.id, Why::Attaching, false, Vec::new());
            }
        }
        let me = window.window_handle().window_id();
        for h in cx.windows().into_iter().filter_map(|w| w.downcast::<Acme>()) {
            if h.window_id() == me {
                continue;
            }
            let _ = h.update(cx, |acme, window, cx| {
                if on(&acme.url) {
                    acme.reconnect(window, cx);
                }
            });
        }
    }

    /// Stop this machine's server, then attach every tab on it again: a
    /// fresh server comes up for the first (`ensure_daemon`).
    pub fn restart_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let socket = apex_server::daemon::default_socket();
        crate::shell::log_line(&format!("restarting the server at {}", socket.display()));
        if let Err(e) = apex_server::remote::stop_any(&socket) {
            crate::shell::log_line(&format!("stopping the server: {e}"));
        }
        // this window's tab, and the others on this machine
        self.reattach_on(None, window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn protocol_from_the_error() {
        assert_eq!(super::their_protocol("the daemon speaks apex protocol 31 (build x), this is protocol 32"), Some(31));
        assert_eq!(super::their_protocol("no"), None);
    }

    #[test]
    fn what_an_attach_says_of_the_daemon() {
        use super::{said_by, Said};
        use std::io::{Error, ErrorKind};
        assert_eq!(said_by(&Ok(Some((71, "abc".into())))), Some(Said::Build(71, "abc".into())));
        let old = apex_server::remote::check_build(apex_server::proto::PROTOCOL - 1, "0123456789ab").unwrap_err();
        assert_eq!(said_by(&Err(old)), Some(Said::Build(apex_server::proto::PROTOCOL - 1, "0123456789ab".into())));
        // the daemon answered, but not of its build
        assert_eq!(said_by(&Err(Error::other("no session x"))), None);
        // not reached
        assert_eq!(said_by(&Err(Error::new(ErrorKind::ConnectionAborted, "closed before welcome"))), Some(Said::Down));
    }

    #[test]
    fn a_daemon_stands_against_this_app() {
        use super::{standing, Said};
        let (p, id) = (apex_server::proto::PROTOCOL, apex_server::BUILD_ID.to_string());
        assert!(standing(&Said::Build(p, id.clone())).starts_with("this app's build"));
        assert!(standing(&Said::Build(p, "other".into())).starts_with("another build: other"));
        assert!(standing(&Said::Build(p - 1, id)).starts_with("another version"));
        assert_eq!(standing(&Said::Down), "not reachable");
    }

    #[test]
    fn a_host_noted_changes_the_menu_only_when_it_says_something_new() {
        use super::{note, Said};
        let d = Some("ssh:restart-test-host".to_string());
        assert!(note(d.clone(), "restart-test-host", Said::Build(1, "a".into())));
        assert!(!note(d.clone(), "restart-test-host", Said::Build(1, "a".into())));
        assert!(note(d, "restart-test-host", Said::Down));
    }
}
