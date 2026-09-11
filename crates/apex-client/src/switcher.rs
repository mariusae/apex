//! ctrl-tab: the connected sessions, the title bar's tabs, stepped
//! through live, as a browser steps its tabs: each press switches the
//! window to the next session, most recently shown first (this one,
//! then the parked ones by when they were parked), ctrl-shift-tab to
//! the one before; the order is the one when control was pressed and
//! holds while it is held, so the presses walk the list rather than
//! bouncing between the last two; letting go ends the walk where it is,
//! and escape goes back to where it began.

use gpui::{Context, Window};

use apex_server::providers::SessionUrl;

use crate::app::Acme;
use crate::pool::Pool;

pub struct Switcher {
    /// The connected sessions (the tabs), most recently shown first as
    /// they were when control was pressed: this window's, then the
    /// parked ones by when they were parked.
    pub entries: Vec<SessionUrl>,
    pub index: usize,
}

impl Acme {
    fn switcher_entries(&self, cx: &Context<Self>) -> Vec<SessionUrl> {
        let mut out: Vec<SessionUrl> = vec![self.url.clone()];
        for u in Pool::by_recency(cx) {
            if !out.contains(&u) {
                out.push(u);
            }
        }
        out
    }

    /// ctrl-tab (`back`: ctrl-shift-tab): one step on through the
    /// sessions, switched to at once.
    pub fn switcher_step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_none() {
            let entries = self.switcher_entries(cx);
            self.switcher = Some(Switcher { entries, index: 0 });
        }
        let Some(s) = self.switcher.as_mut() else { return };
        let n = s.entries.len();
        if n > 1 {
            s.index = if back { (s.index + n - 1) % n } else { (s.index + 1) % n };
        }
        if let Some(url) = self.switcher.as_ref().and_then(|s| s.entries.get(s.index)).cloned() {
            if url != self.url {
                self.switch_to(&url, window, cx);
            }
        }
        cx.notify();
    }

    /// Control let go: the walk is over, where it stands.
    pub fn switcher_commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        cx.notify();
    }

    /// Escape with control still held: back to where the walk began.
    pub fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.switcher.take() {
            if let Some(url) = s.entries.first() {
                if *url != self.url {
                    let url = url.clone();
                    self.switch_to(&url, window, cx);
                }
            }
        }
        cx.notify();
    }
}
