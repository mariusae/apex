//! The sessions by the keys, as Manifold's spaces are.
//!
//! ⌘1 to ⌘8 go to the sessions in the order the sidebar lists them, ⌘9
//! to the last, as a browser's tabs. ⌘ held alone a quarter of a second
//! shows each one's key, a pill by its row in the sidebar; with the
//! sidebar hidden, the sessions' names slide out across the title bar
//! from the session's own, each with its key. Let go, or another
//! modifier with it, and they go; a key with ⌘ (⌘2, then ⌘3) leaves them.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, rgb, AnyElement, Context, FontWeight, MouseButton, Window};

use crate::app::Acme;
use crate::pool::{Pool, TabId};

/// How long ⌘ is held alone before the keys show.
pub const NUMBERS_AFTER: Duration = Duration::from_millis(250);
/// How long the names take to slide out across the title bar.
const SLIDE: Duration = Duration::from_millis(180);

/// ⌘n's n for the session at `i` of `count`: 1 to 8, and 9 the last.
pub fn number(i: usize, count: usize) -> Option<usize> {
    if i + 1 == count {
        return Some(if count <= 9 { i + 1 } else { 9 });
    }
    (i < 8).then_some(i + 1)
}

/// The session ⌘`n` goes to, of `count`, by its place: ⌘9 the last.
pub fn place_of(n: usize, count: usize) -> Option<usize> {
    match n {
        0 => None,
        9 => count.checked_sub(1),
        n if n <= count => Some(n - 1),
        _ => None,
    }
}

/// ⌘n's pill: the key, small, in the quiet ink on a faint wash of the
/// ink over whatever ground it sits on (the sidebar's card, the title
/// bar): there to be read when wanted, not to shout.
pub fn key_pill(n: usize) -> gpui::Div {
    let t = crate::theme::theme();
    div()
        .flex_none()
        .h(px(16.))
        .px(px(5.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .bg(gpui::Hsla::from(rgb(t.text)).opacity(0.08))
        .text_color(rgb(t.text_dim))
        .font_family(crate::fonts::ui())
        .text_size(px(11.))
        .line_height(px(16.))
        .font_weight(crate::fonts::weight(FontWeight::MEDIUM))
        .child(format!("⌘{n}"))
}

impl Acme {
    /// The open sessions in the sidebar's order, which ⌘1 to ⌘9 count.
    pub fn numbered_tabs(&self, cx: &gpui::App) -> Vec<TabId> {
        self.sidebar_tab_order(cx)
    }

    /// ⌘n: the session the keys say, as the sidebar numbers them.
    pub fn go_to_numbered(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.numbered_tabs(cx);
        let Some(&id) = place_of(n, order.len()).and_then(|i| order.get(i)) else { return };
        if id != self.tab {
            self.switch_to(id, window, cx);
            cx.notify();
        }
    }

    /// The modifiers changed: ⌘ down alone starts the wait for its keys;
    /// anything else puts them away.
    pub fn command_mods(&mut self, mods: &gpui::Modifiers, window: &mut Window, cx: &mut Context<Self>) {
        let alone = mods.platform && !mods.shift && !mods.alt && !mods.control && !mods.function;
        if alone {
            let at = Instant::now();
            self.command_down = Some(at);
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(NUMBERS_AFTER).await;
                let _ = this.update_in(cx, |acme, window, cx| {
                    // still held alone, nothing typed, the window ours
                    if acme.command_down == Some(at) && window.is_window_active() {
                        acme.command_down = None;
                        acme.numbers = Some(Instant::now());
                        cx.notify();
                    }
                });
            })
            .detach();
        } else {
            self.command_down = None;
            if self.numbers.take().is_some() {
                cx.notify();
            }
        }
    }

    /// A key went down: with ⌘ the keys stay (⌘2, then ⌘3), but it is no
    /// longer ⌘ held alone, so they do not come if not yet up.
    pub fn command_key(&mut self, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        self.command_down = None;
        if !mods.platform && self.numbers.take().is_some() {
            cx.notify();
        }
    }

    /// The system asked, often (`Acme::over`'s tick): ⌘ held alone long
    /// enough shows the keys though gpui never heard it go down -- a page
    /// had the keys, and the flags went to it.
    pub fn command_poll(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.numbers.is_some() || !window.is_window_active() {
            return;
        }
        if crate::web::command_alone_for().is_some_and(|held| held >= NUMBERS_AFTER) {
            self.command_down = None;
            self.numbers = Some(Instant::now());
            cx.notify();
        }
    }

    /// ⌘ let go where gpui did not hear it (a page had the keys, or
    /// another app): the keys put away.
    pub fn command_released(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.command_down = None;
        if self.numbers.take().is_some() {
            cx.notify();
        }
    }

    /// The sessions' names across the title bar while ⌘ is held with the
    /// sidebar hidden: slid out from under the session's own name, which
    /// they cover, each with its key; the one shown as the sidebar marks
    /// it. Drawn from `left` across, `h` high.
    pub fn numbered_strip(&self, left: f32, h: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let since = self.numbers?;
        if self.sidebar_shown() {
            return None;
        }
        let t = crate::theme::theme();
        let order = self.numbered_tabs(cx);
        if order.is_empty() {
            return None;
        }
        let ground = crate::text_element::ground(&t);
        let tabs = Pool::tabs(cx);
        let count = order.len();
        let mut row = div().flex().flex_row().items_center().gap(px(4.)).h_full().pl(px(4.)).pr(px(12.)).bg(rgb(ground));
        for (i, id) in order.iter().enumerate() {
            let Some(tab) = tabs.iter().find(|x| x.id == *id) else { continue };
            let current = *id == self.tab;
            let name = if current { self.session_label() } else { tab.url.session.clone() };
            let id = *id;
            row = row.child(
                div()
                    .id(("numbered-session", i))
                    .flex_none()
                    .h(px(24.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .rounded(px(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .cursor_default()
                    .when(current, |d| d.bg(rgb(crate::theme::step(ground, 1))))
                    .hover(move |s| s.bg(rgb(crate::theme::step(ground, 1))))
                    .child(div().max_w(px(160.)).truncate().text_size(px(13.)).text_color(rgb(if current { t.text } else { t.text_dim })).when(current, |d| d.font_weight(crate::fonts::weight(FontWeight::MEDIUM))).child(name))
                    .children(number(i, count).map(key_pill))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.numbers = None;
                            if id != this.tab {
                                this.switch_to(id, window, cx);
                            }
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        // slid out from the left, as a drawer from under the name
        let k = (since.elapsed().as_secs_f32() / SLIDE.as_secs_f32()).clamp(0., 1.);
        let eased = crate::glide::ease(k);
        let row = row.relative().left(gpui::relative(eased - 1.)).opacity(eased.max(0.15));
        Some(
            div()
                .id("numbered-strip")
                .absolute()
                .top(px(0.))
                .left(px(left))
                .right(px(0.))
                .h(px(h))
                .overflow_hidden()
                .font_family(crate::fonts::ui())
                .child(row)
                .into_any_element(),
        )
    }

    /// Whether the strip is still sliding (a frame wanted).
    pub fn numbered_strip_moving(&self) -> bool {
        self.numbers.is_some_and(|s| s.elapsed() < SLIDE) && !self.sidebar_shown()
    }

}

#[cfg(test)]
mod tests {
    use super::{number, place_of};

    #[test]
    fn the_keys_count_one_to_eight_and_nine_is_the_last() {
        assert_eq!((0..3).map(|i| number(i, 3)).collect::<Vec<_>>(), vec![Some(1), Some(2), Some(3)]);
        let twelve: Vec<_> = (0..12).map(|i| number(i, 12)).collect();
        assert_eq!(twelve[7], Some(8));
        assert_eq!(twelve[8], None);
        assert_eq!(twelve[11], Some(9));
        // nine exactly: the ninth is both the ninth and the last
        assert_eq!(number(8, 9), Some(9));
    }

    #[test]
    fn a_key_goes_where_its_pill_is() {
        for count in 1..14 {
            for i in 0..count {
                if let Some(n) = number(i, count) {
                    assert_eq!(place_of(n, count), Some(i), "{n} of {count}");
                }
            }
        }
        assert_eq!(place_of(4, 3), None);
        assert_eq!(place_of(9, 0), None);
    }
}
