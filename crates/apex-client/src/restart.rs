//! Restarting this machine's apex server, as Manifold offers to: when an
//! attach finds the server here is from another version (which this apex
//! cannot talk to), once a launch; and whenever asked (Apex ▸ Restart
//! Server…). The server holds the sessions, so it goes only when the
//! user says so, told what goes with it: the sessions' windows, their
//! terminals and the programs in them, and changes not saved. The tabs
//! are the app's and stay; each attaches to a fresh session of its name.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Context, PromptLevel, Window};

use crate::app::Acme;
use crate::pool::{Pool, Why};

static OFFERED: AtomicBool = AtomicBool::new(false);

/// The protocol a mismatch error names the server as speaking.
fn their_protocol(why: &str) -> Option<u32> {
    let rest = why.split("speaks apex protocol ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

impl Acme {
    /// An attach here met a server of another version: offer, once a
    /// launch, to restart it.
    pub fn offer_restart(&mut self, why: &str, window: &mut Window, cx: &mut Context<Self>) {
        if OFFERED.swap(true, Ordering::Relaxed) {
            return;
        }
        let theirs = their_protocol(why).map(|p| format!("protocol {p}, ")).unwrap_or_default();
        let detail = format!(
            "Apex was updated, but the server on this machine is still the old one ({theirs}this is protocol {}), and this Apex cannot attach to its sessions until it is restarted.\n\nRestarting ends those sessions: their windows close, their terminals and the programs in them end, and changes not saved are lost. Your tabs are kept; each attaches to a fresh session of its name.",
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
        let detail = format!(
            "Its sessions end with it: their windows close, their terminals and the programs in them end, and changes not saved are lost.{lost} Your tabs are kept; each attaches to a fresh session of its name."
        );
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

    /// Stop this machine's server, then attach every tab on it again: a
    /// fresh server comes up for the first (`ensure_daemon`).
    pub fn restart_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let socket = apex_server::daemon::default_socket();
        crate::shell::log_line(&format!("restarting the server at {}", socket.display()));
        if let Err(e) = apex_server::remote::stop_any(&socket) {
            crate::shell::log_line(&format!("stopping the server: {e}"));
        }
        // this window's tab, and the others on this machine
        if self.url.is_local() {
            self.reconnect(window, cx);
        }
        let shown = Pool::shown_tabs(cx);
        for t in Pool::tabs(cx) {
            if t.url.is_local() && t.id != self.tab && !shown.contains(&t.id) {
                Pool::start(cx, t.id, Why::Attaching, false, Vec::new());
            }
        }
        // the other windows showing tabs here
        let me = window.window_handle().window_id();
        for h in cx.windows().into_iter().filter_map(|w| w.downcast::<Acme>()) {
            if h.window_id() == me {
                continue;
            }
            let _ = h.update(cx, |acme, window, cx| {
                if acme.url.is_local() {
                    acme.reconnect(window, cx);
                }
            });
        }
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
}
