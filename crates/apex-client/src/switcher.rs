//! The session switcher: ctrl-tab, held, steps through the connected
//! sessions — the title bar's tabs — most recently shown first, as an
//! application switcher does; letting go of control switches to the
//! one under the mark, ctrl-shift-tab steps back, escape leaves things
//! as they are.

use gpui::{anchored, deferred, div, point, prelude::*, px, rgb, Context, Window};

use apex_server::providers::SessionUrl;

use crate::app::Acme;
use crate::pool::Pool;
use crate::shell::UI_FONT;

pub struct Switcher {
    /// The connected sessions (the tabs), most recently shown first:
    /// this window's, then the parked ones by when they were parked.
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

    /// The switcher, when up: the sessions in a list, the label first
    /// and the host after it dimmed (none for a local session), the one
    /// under the mark tinted as the picker's rows are, not inverted.
    pub fn switcher_panel(&self, _cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.switcher.as_ref()?;
        let mut list = div().flex().flex_col().py(px(6.)).px(px(6.));
        for (i, u) in s.entries.iter().enumerate() {
            let on = i == s.index;
            let t = crate::theme::theme();
            let (fg, dim) = (rgb(t.panel_text), rgb(t.panel_dim));
            let mut row = div()
                .flex()
                .flex_row()
                .items_baseline()
                .gap(px(8.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(14.))
                .font_family(UI_FONT)
                .when(on, |d| d.bg(rgb(t.panel_pick)))
                .child(div().text_color(fg).child(u.session.clone()));
            if !u.is_local() {
                row = row.child(div().text_size(px(12.)).text_color(dim).child(u.arg.clone()));
            }
            list = list.child(row);
        }
        let t = crate::theme::theme();
        let panel = div().w(px(360.)).max_h(px(560.)).bg(rgb(t.panel_bg)).border_1().border_color(rgb(t.panel_border)).rounded(px(10.)).shadow_lg().overflow_hidden().child(self.overlay_mark()).child(list);
        Some(deferred(anchored().position(point(px(72.), px(self.top() + 40.))).child(panel)).with_priority(2))
    }
}
