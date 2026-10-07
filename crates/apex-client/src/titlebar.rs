//! The title bar's session: its name in bold after the sidebar's button,
//! a chevron after it, and a mark when another session wants the user.
//! A click on the name makes it a field: return renames the session (on
//! its daemon, as the picker's rename does), escape or a click elsewhere
//! leaves it as it was. A click on the chevron brings the sessions down
//! under it -- this one checked, another notified one wearing pjw -- and a
//! click on one goes to it; New Session opens the picker.

use std::time::Instant;

use gpui::{deferred, div, prelude::*, px, rgb, AnyElement, Context, FontWeight, MouseButton, Window};

use crate::app::Acme;
use crate::field::{Edited, LineEdit};
use crate::pool::Pool;

/// The session's name being typed.
pub struct SessionEdit {
    pub field: LineEdit,
    pub caret_since: Instant,
}

impl SessionEdit {
    fn caret_on(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / 530) % 2 == 0
    }
}

impl Acme {
    /// The name, what the sidebar calls this session too.
    fn session_label(&self) -> String {
        if self.in_process() {
            "in-process".to_string()
        } else {
            self.session.clone()
        }
    }

    /// A click on the name: it becomes a field, all of it selected.
    pub fn session_edit_start(&mut self, cx: &mut Context<Self>) {
        let mut field = LineEdit::new();
        field.set(&self.session_label());
        field.select_all();
        self.session_menu = false;
        self.session_edit = Some(SessionEdit { field, caret_since: Instant::now() });
        cx.notify();
    }

    /// A key while the name is being typed: return renames the session,
    /// escape leaves it as it was, the rest edit it.
    pub fn session_edit_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.session_edit.as_mut() else { return };
        e.caret_since = Instant::now();
        match key {
            "escape" => self.session_edit = None,
            "enter" => {
                let Some(e) = self.session_edit.take() else { return };
                let to = e.field.trim().to_string();
                if !to.is_empty() && !to.contains(char::is_whitespace) {
                    self.rename_session(&to, window);
                    cx.defer(|cx| crate::shell::save_open(cx));
                }
            }
            _ => {
                if e.field.key(key, ch, mods) == Edited::No {
                    return;
                }
            }
        }
        cx.notify();
    }

    /// The name (or its field), the chevron, and the mark for another
    /// session's notifications: the title bar's, after the sidebar's
    /// button.
    pub fn session_title(&self, h: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let ground = crate::text_element::ground(&t);
        let hover = crate::theme::step(ground, 1);
        let name: AnyElement = match &self.session_edit {
            Some(e) => div()
                .id("session-name-field")
                .min_w(px(120.))
                .h(px(24.))
                .px(px(6.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .bg(rgb(t.body_bg))
                .border_1()
                .border_color(rgb(t.accent))
                .text_size(px(13.5))
                .font_weight(crate::fonts::weight(FontWeight::SEMIBOLD))
                .child(crate::field::field_view(&e.field, e.caret_on(), "Session name", true))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .into_any_element(),
            None => div()
                .id("session-name")
                .h(px(24.))
                .px(px(6.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .text_size(px(13.5))
                .font_weight(crate::fonts::weight(FontWeight::SEMIBOLD))
                .text_color(rgb(t.text))
                .whitespace_nowrap()
                .cursor(gpui::CursorStyle::IBeam)
                .hover(move |s| s.bg(rgb(hover)))
                .child(self.session_label())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.session_edit_start(cx);
                        cx.stop_propagation();
                    }),
                )
                .into_any_element(),
        };
        let open = self.session_menu;
        let chevron = div()
            .id("session-chevron")
            .size(px(20.))
            .rounded(px(5.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .relative()
            .when(open, move |d| d.bg(rgb(hover)))
            .hover(move |s| s.bg(rgb(hover)))
            .child(chevron_glyph(crate::text_element::rgb(t.text_dim)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.session_edit = None;
                    this.session_menu = !this.session_menu;
                    cx.notify();
                    cx.stop_propagation();
                }),
            );
        // another session wants the user: pjw on the chevron, as a badge
        // (pjw is only ever for another session, never this one)
        let others = Pool::tabs(cx).into_iter().any(|tab| tab.id != self.tab && self.tab_notified(tab.id, cx));
        let chevron = chevron.when(others, |d| d.child(div().absolute().top(px(-6.)).right(px(-6.)).child(crate::shell::pjw(12., t.accent))));
        let menu = open.then(|| self.session_menu_panel(h, cx));
        div()
            .relative()
            .flex_none()
            .h(px(h))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .font_family(crate::fonts::ui())
            .child(name)
            .child(chevron)
            .children(menu)
            .into_any_element()
    }

    /// The sessions, dropped down under the chevron.
    fn session_menu_panel(&self, h: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.18), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false };
        let mut panel = div()
            .id("session-menu")
            .absolute()
            .top(px(h - 4.))
            .left(px(0.))
            .min_w(px(220.))
            .max_w(px(360.))
            .p(px(5.))
            .rounded(px(9.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(self.overlay_mark())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let row = |id: (&'static str, usize)| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(8.))
                .rounded(px(6.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .cursor_default()
                .text_color(rgb(t.panel_text))
                .hover(|s| s.bg(rgb(t.panel_chosen_bg)).text_color(rgb(t.panel_chosen_text)))
        };
        for (i, tab) in Pool::tabs(cx).into_iter().enumerate() {
            let id = tab.id;
            let current = id == self.tab;
            let name = if current { self.session_label() } else { tab.url.session.clone() };
            let host = (!tab.url.is_local()).then(|| tab.url.arg.clone());
            let notified = !current && self.tab_notified(id, cx);
            // where it stands, when it does not lead: the same word the
            // tabs and the sidebar say
            let word = self.tab_word(&tab, cx);
            panel = panel.child(
                row(("session-menu-row", i))
                    .child(div().flex_none().w(px(12.)).child(if current { "✓" } else { "" }))
                    .child(div().flex_1().min_w_0().truncate().when(current, |d| d.font_weight(crate::fonts::weight(FontWeight::SEMIBOLD))).child(name))
                    .when_some(host, |d, host| d.child(div().flex_none().text_size(px(12.)).text_color(rgb(t.panel_dim)).child(host)))
                    .when_some(word, |d, word| d.child(div().flex_none().text_size(px(12.)).italic().text_color(rgb(t.panel_dim)).child(word)))
                    .when(notified, |d| d.child(div().flex_none().child(crate::shell::pjw(12., t.accent))))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.session_menu = false;
                            if !current {
                                this.switch_to(id, window, cx);
                                cx.defer(|cx| crate::shell::save_open(cx));
                            }
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        panel = panel.child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(rgb(t.panel_border))).child(
            row(("session-menu-new", 0))
                .child(div().flex_none().w(px(12.)).child("+"))
                .child("New Session…")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.session_menu = false;
                        this.open_selector(cx);
                        cx.stop_propagation();
                    }),
                ),
        );
        deferred(panel).with_priority(2).into_any_element()
    }
}

/// A chevron pointing down, 8 across.
fn chevron_glyph(ink: gpui::Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let c = b.center();
            let mut p = gpui::PathBuilder::stroke(px(1.5));
            p.move_to(gpui::point(c.x - px(4.), c.y - px(2.)));
            p.line_to(gpui::point(c.x, c.y + px(2.)));
            p.line_to(gpui::point(c.x + px(4.), c.y - px(2.)));
            if let Ok(path) = p.build() {
                window.paint_path(path, ink);
            }
        },
    )
    .size(px(12.))
}
