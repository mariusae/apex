//! The session switcher: ctrl-tab, held, steps through the sessions
//! this client knows, most recent first, as an application switcher
//! does; letting go of control switches to the one under the mark,
//! ctrl-shift-tab steps back, escape leaves things as they are.

use gpui::{anchored, deferred, div, point, prelude::*, px, rgb, Context, Window};

use apex_server::providers::SessionUrl;

use crate::app::Acme;
use crate::pool::Pool;
use crate::shell::UI_FONT;

pub struct Switcher {
    /// Most recent first: this window's session, the parked ones, then
    /// the sessions attached to lately, then every other one known.
    pub entries: Vec<SessionUrl>,
    pub index: usize,
}

impl Acme {
    fn switcher_entries(&self, cx: &Context<Self>) -> Vec<SessionUrl> {
        let mut out: Vec<SessionUrl> = vec![self.url.clone()];
        let mut add = |u: SessionUrl| {
            if !out.contains(&u) {
                out.push(u);
            }
        };
        for u in Pool::by_recency(cx) {
            add(u);
        }
        for u in crate::shell::recent() {
            add(u);
        }
        let mut known: Vec<SessionUrl> = crate::shell::known_sessions().into_iter().flat_map(|(h, names)| names.into_iter().map(move |n| h.url(&n))).collect();
        known.sort_by_key(|u| u.to_string());
        for u in known {
            add(u);
        }
        out
    }

    /// ctrl-tab (`back`: ctrl-shift-tab): the switcher, or one step in it.
    pub fn switcher_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.switcher.is_none() {
            let entries = self.switcher_entries(cx);
            self.switcher = Some(Switcher { entries, index: 0 });
        }
        let Some(s) = self.switcher.as_mut() else { return };
        let n = s.entries.len();
        if n > 1 {
            s.index = if back { (s.index + n - 1) % n } else { (s.index + 1) % n };
        }
        cx.notify();
    }

    /// Control let go: the session under the mark.
    pub fn switcher_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.switcher.take() else { return };
        if let Some(url) = s.entries.get(s.index) {
            if *url != self.url {
                self.switch_to(url, window, cx);
            }
        }
        cx.notify();
    }

    pub fn close_switcher(&mut self, cx: &mut Context<Self>) {
        self.switcher = None;
        cx.notify();
    }

    /// The switcher, when up: the sessions in a row (wrapping), the one
    /// under the mark filled.
    pub fn switcher_panel(&self, _cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.switcher.as_ref()?;
        let mut strip = div().flex().flex_row().flex_wrap().gap(px(6.)).p(px(10.));
        for (i, u) in s.entries.iter().enumerate() {
            let on = i == s.index;
            strip = strip.child(
                div()
                    .px(px(12.))
                    .py(px(7.))
                    .rounded(px(7.))
                    .text_size(px(14.))
                    .font_family(UI_FONT)
                    .when(on, |d| d.bg(rgb(0x000099)).text_color(rgb(0xffffff)))
                    .when(!on, |d| d.text_color(rgb(0x111111)))
                    .child(u.describe()),
            );
        }
        let panel = div().w(px(620.)).bg(rgb(0xf4f4f4)).border_1().border_color(rgb(0xc8c8c8)).rounded(px(10.)).shadow_lg().child(strip);
        Some(deferred(anchored().position(point(px(72.), px(self.top() + 40.))).child(panel)).with_priority(2))
    }
}
