//! A web window's header, as the Claude app's browser has it: the
//! handle (so the window moves, grows and is stashed as any other does),
//! back and forward, and the address, and nothing else -- no tag text.
//! The window's tag is still there underneath, the core's: its first
//! word is the address the page follows, as ever (`web_place`), and
//! Back, Fwd and Get in it are what the buttons do. A web window made
//! with no address is a blank page whose address is being typed.

use std::time::Instant;

use gpui::prelude::*;
use gpui::{div, px, rgb, AnyElement, Context, MouseButton};

use apex_core::WindowId;

use crate::app::Acme;
use crate::field::{Edited, LineEdit};
use crate::web::Nav;

/// The address being typed into a web window's header.
pub struct UrlEdit {
    pub window: WindowId,
    pub field: LineEdit,
    pub caret_since: Instant,
}

impl UrlEdit {
    fn caret_on(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / 530) % 2 == 0
    }
}

/// What was typed, as an address: one with a scheme stands; a path is
/// the host's file; anything else is taken for a host on the web.
pub fn address(typed: &str) -> String {
    let t = typed.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.contains("://") || t.starts_with("about:") || t.starts_with("data:") {
        return t.to_string();
    }
    if t.starts_with('/') || t.starts_with('~') {
        return format!("file://{t}");
    }
    if t.starts_with("localhost") || t.starts_with("127.0.0.1") {
        return format!("http://{t}");
    }
    format!("https://{t}")
}

impl Acme {
    /// The address field taken for typing: what is there, all selected.
    pub fn url_edit_start(&mut self, w: WindowId, cx: &mut Context<Self>) {
        let mut field = LineEdit::new();
        field.set(&self.node.window_path(w));
        field.select_all();
        self.url_edit = Some(UrlEdit { window: w, field, caret_since: Instant::now() });
        cx.notify();
    }

    /// A key while the address is being typed: return goes there,
    /// escape leaves it as it was, the rest edit it.
    pub fn url_edit_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let Some(e) = self.url_edit.as_mut() else { return };
        e.caret_since = Instant::now();
        match key {
            "escape" => self.url_edit = None,
            "enter" => {
                let Some(e) = self.url_edit.take() else { return };
                let url = address(&e.field);
                if !url.is_empty() {
                    let _ = self.node.web_navigate(&mut self.log, e.window, &url);
                    self.after();
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

    /// A web window's header in its tag's place, `h` high.
    pub fn web_header(&self, w: WindowId, h: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let bg = t.tag_bg;
        let d = crate::text_element::dot(&t, false, false, true, self.webs.loading(w), self.note_age(w)).squared(self.hides_others(w));
        let lane = crate::text_element::SCROLLWID;
        let handle = div()
            .id(("web-handle", w.0))
            .flex_none()
            .w(px(lane))
            .h_full()
            .cursor(gpui::CursorStyle::OpenHand)
            .child(gpui::canvas(|_, _, _| {}, move |b, _, window, _| crate::text_element::paint_dot(window, &d, gpui::point(b.left() + px(7.5), b.top() + b.size.height / 2.))).size_full())
            // the handle's buttons as any window's: acme's box
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| this.press_handle(w, MouseButton::Left, e.position, e.modifiers.shift, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| this.press_handle(w, MouseButton::Middle, e.position, e.modifiers.shift, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| this.press_handle(w, MouseButton::Right, e.position, e.modifiers.shift, cx)))
            .on_mouse_down(MouseButton::Navigate(gpui::NavigationDirection::Back), cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| this.press_handle(w, MouseButton::Navigate(gpui::NavigationDirection::Back), e.position, false, cx)));
        let button = |id: &'static str, glyph: &'static str, nav: Nav| {
            div()
                .id((id, w.0))
                .flex_none()
                .w(px(22.))
                .h(px((h - 4.).max(12.)))
                .rounded(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(15.))
                .text_color(rgb(t.text_dim))
                .cursor_default()
                .hover(|s| s.bg(rgb(crate::theme::step(t.tag_bg, 1))))
                .child(glyph)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.page_nav(w, nav);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
        };
        let editing = self.url_edit.as_ref().filter(|e| e.window == w);
        let name = self.node.window_path(w);
        let field: AnyElement = match editing {
            Some(e) => crate::field::field_view(&e.field, e.caret_on(), "Enter an address", true).into_any_element(),
            None if name.is_empty() => div().text_color(rgb(t.text_dim)).child("Enter an address").into_any_element(),
            None => div().truncate().text_color(rgb(t.text)).child(name).into_any_element(),
        };
        let pill = div()
            .id(("web-url", w.0))
            .flex_1()
            .min_w_0()
            .h(px((h - 4.).max(12.)))
            .px(px(8.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .bg(rgb(if editing.is_some() { t.body_bg } else { crate::theme::step(t.tag_bg, 1) }))
            .when(editing.is_some(), |d| d.border_1().border_color(rgb(t.accent)))
            .text_size(px(12.5))
            .cursor(gpui::CursorStyle::IBeam)
            .child(field)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if this.url_edit.as_ref().is_none_or(|e| e.window != w) {
                        this.url_edit_start(w, cx);
                    }
                    cx.stop_propagation();
                }),
            );
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .pr(px(6.))
            .bg(rgb(bg))
            .border_b_1()
            .border_color(rgb(t.body_border))
            .font_family(crate::fonts::ui())
            .child(handle)
            .child(button("web-back", "‹", Nav::Back))
            .child(button("web-fwd", "›", Nav::Fwd))
            .child(div().w(px(4.)))
            .child(pill)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::address;

    #[test]
    fn typed_addresses() {
        assert_eq!(address("example.com"), "https://example.com");
        assert_eq!(address(" https://a.b/c "), "https://a.b/c");
        assert_eq!(address("localhost:3000"), "http://localhost:3000");
        assert_eq!(address("/tmp/x.html"), "file:///tmp/x.html");
        assert_eq!(address(""), "");
    }
}
