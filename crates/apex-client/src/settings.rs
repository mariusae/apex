//! Settings (⌘,): how apex's UI and shell look and behave, in one
//! window, as Manifold's are -- the appearance (the system's, light or
//! dark), the theme, each a small apex drawn in its own colours (a column
//! of windows with their tags, a text with a selection and B2's and B3's
//! sweeps, a terminal printing in its sixteen, the keys' window ringed),
//! the fonts, each named in its own two faces, the text's size, and the
//! switches that were the View menu's. The session's setup, its profile,
//! is ⌘⇧, still: a file, not a setting of the app's.

use gpui::prelude::*;
use gpui::{div, px, rgb, size, App, Bounds, Context, FocusHandle, FontWeight, Global, MouseButton, SharedString, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions};

use crate::fonts;
use crate::theme::{self, Mode, Palette, Theme, PALETTES};

/// The window, once open: ⌘, again brings it forward.
#[derive(Default)]
struct Open(Option<WindowHandle<Settings>>);

impl Global for Open {}

/// ⌘,: the Settings window, made or brought forward.
pub fn open(cx: &mut App) {
    if let Some(h) = cx.try_global::<Open>().and_then(|o| o.0) {
        if h.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Settings".into()), appears_transparent: false, traffic_light_position: None }),
            is_resizable: false,
            ..Default::default()
        },
        |window, cx| {
            let v = cx.new(|cx| Settings { focus: cx.focus_handle(), hovered: None });
            let focus = v.read(cx).focus.clone();
            window.focus(&focus, cx);
            v
        },
    );
    if let Ok(h) = opened {
        cx.set_global(Open(Some(h)));
    }
}

const WIDTH: f32 = 780.;
const HEIGHT: f32 = 620.;
/// The labels' column, right-aligned, as a Mac form's.
const LABEL_W: f32 = 120.;
/// A theme's card.
const CARD: (f32, f32) = (138., 92.);

pub struct Settings {
    focus: FocusHandle,
    /// The theme card under the pointer.
    hovered: Option<Palette>,
}

impl Render for Settings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::theme();
        let form = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(28.))
            .child(self.row("Appearance", self.appearance(cx)))
            .child(self.row_top("Theme", self.themes(cx)))
            .child(self.row_top("Fonts", self.fonts(cx)))
            .child(self.row("Text Size", self.text_size(cx)))
            .child(self.row_top(
                "Behaviour",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(self.check("contrast", "Lift a terminal's colours that don't read on its paper", theme::contrast(), cx, |cx| crate::shell::toggle_contrast(cx)))
                    .child(self.check("blink", "Blink the cursor", theme::blink(), cx, |cx| crate::shell::toggle_blink(cx)))
                    .child(self.check("smooth", "Move the cursor smoothly", theme::smooth_caret(), cx, |cx| crate::shell::toggle_smooth_caret(cx)))
                    .child(self.check("layout", "Animate the layout's changes", theme::animate_layout(), cx, |cx| crate::shell::toggle_layout_animations(cx))),
            ));
        div()
            .id("settings")
            .track_focus(&self.focus)
            .size_full()
            .bg(rgb(t.panel_bg))
            .text_color(rgb(t.panel_text))
            .font_family(fonts::ui())
            .text_size(px(13.))
            .overflow_y_scroll()
            // ⌘W, and the window's own close: this window, not a session's
            .on_action(cx.listener(|_, _: &crate::shell::Del, window, _| window.remove_window()))
            .on_action(cx.listener(|_, _: &crate::shell::CloseWindow, window, _| window.remove_window()))
            .child(form)
    }
}

impl Settings {
    /// A row of the form: its label at the right of the labels' column,
    /// level with the middle of what it sets.
    fn row(&self, label: &str, control: impl IntoElement) -> gpui::Div {
        self.row_with(label, control, false)
    }

    /// A row whose control is tall: its label level with its top.
    fn row_top(&self, label: &str, control: impl IntoElement) -> gpui::Div {
        self.row_with(label, control, true)
    }

    fn row_with(&self, label: &str, control: impl IntoElement, top: bool) -> gpui::Div {
        let t = theme::theme();
        div()
            .flex()
            .flex_row()
            .gap(px(14.))
            .when(!top, |d| d.items_center())
            .child(div().flex_none().w(px(LABEL_W)).when(top, |d| d.pt(px(3.))).flex().justify_end().text_color(rgb(t.panel_dim)).child(format!("{label}:")))
            .child(div().flex_1().min_w_0().child(control))
    }

    /// Choices side by side, the chosen one raised, as a segmented control.
    fn segments<T: Copy + PartialEq + 'static>(&self, id: &'static str, choices: &[(&'static str, T)], now: T, cx: &mut Context<Self>, set: fn(T, &mut App)) -> gpui::Div {
        let t = theme::theme();
        let mut row = div().flex().flex_row().p(px(2.)).gap(px(2.)).rounded(px(7.)).bg(rgb(theme::step(t.panel_bg, 2))).flex_none();
        for (i, (title, v)) in choices.iter().copied().enumerate() {
            let on = v == now;
            row = row.child(
                div()
                    .id((id, i))
                    .px(px(12.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .cursor_default()
                    .when(on, |d| d.bg(rgb(t.panel_bg)).shadow(vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.12), offset: gpui::point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }]))
                    .when(!on, |d| d.text_color(rgb(t.panel_dim)).hover(|s| s.text_color(rgb(t.panel_text))))
                    .child(title)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, _, cx| {
                            set(v, cx);
                            cx.notify();
                        }),
                    ),
            );
        }
        div().flex().child(row)
    }

    fn appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        self.segments("mode", &[("Match System", Mode::System), ("Light", Mode::Light), ("Dark", Mode::Dark)], theme::mode(), cx, crate::shell::set_theme)
    }

    /// The themes, each a card: a small apex in its colours, its name
    /// under it, the chosen one ringed.
    fn themes(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme::theme();
        let dark = theme::is_dark();
        let now = theme::palette();
        let mut row = div().flex().flex_row().gap(px(14.));
        for p in PALETTES {
            let on = p == now;
            let hovered = self.hovered == Some(p);
            let edge = if on { t.panel_text } else if hovered { t.panel_dim } else { t.panel_border };
            row = row.child(
                div()
                    .id(("theme", p as usize))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .child(div().p(px(2.5)).rounded(px(10.)).border(px(if on { 1.5 } else { 0.5 })).border_color(rgb(edge)).child(sketch(theme::of(p, dark))))
                    .child(div().text_color(rgb(if on { t.panel_text } else { t.panel_dim })).when(on, |d| d.font_weight(FontWeight::SEMIBOLD)).child(p.title()))
                    .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                        if *over {
                            this.hovered = Some(p);
                        } else if this.hovered == Some(p) {
                            this.hovered = None;
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, _, cx| {
                            crate::shell::set_palette(p, cx);
                            cx.notify();
                        }),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(row)
            .child(div().text_size(px(11.5)).text_color(rgb(t.panel_dim)).child(if theme::palette() == Palette::Acme { "Acme's own colours, the same light and dark." } else { "Each a light and a dark: the appearance says which." }))
    }

    /// The fonts, each named in its own text face with a line of its mono
    /// beside it, the chosen one on a band.
    fn fonts(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme::theme();
        let now = fonts::current();
        let mut list = div().flex().flex_col().w(px(360.)).p(px(3.)).rounded(px(8.)).border(px(0.5)).border_color(rgb(t.panel_border));
        for s in fonts::offered() {
            let on = s == now;
            let (text, mono) = fonts::families(s);
            list = list.child(
                div()
                    .id(("font", s as usize))
                    .h(px(32.))
                    .px(px(10.))
                    .rounded(px(5.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .cursor_default()
                    .when(on, |d| d.bg(rgb(t.panel_chosen_bg)).text_color(rgb(t.panel_chosen_text)))
                    .when(!on, |d| d.hover(|h| h.bg(rgb(t.panel_hover))))
                    .child(div().w(px(110.)).font_family(SharedString::from(text)).text_size(px(14.)).child(s.title()))
                    .child(div().flex_1().font_family(SharedString::from(mono)).text_size(px(12.)).when(!on, |d| d.text_color(rgb(t.panel_dim))).child("fn main() { 0O1l }"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, _, cx| {
                            crate::shell::set_fonts(s, cx);
                            cx.notify();
                        }),
                    ),
            );
        }
        let note = if fonts::Set::Lucida.available() { "Text, the interface and pages in the first; terminals and mono windows in the second." } else { "Humanist is offered with Lucida Grande Mono W1G installed." };
        div().flex().flex_col().gap(px(6.)).child(list).child(div().text_size(px(11.5)).text_color(rgb(t.panel_dim)).child(note))
    }

    /// ⌘+ and ⌘−, here: the text's size a step, back to the set's own.
    fn text_size(&self, cx: &mut Context<Self>) -> gpui::Div {
        let t = theme::theme();
        let button = |id: &'static str, label: &'static str, by: i8| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border(px(0.5))
                .border_color(rgb(t.panel_border))
                .cursor_default()
                .hover(|s| s.bg(rgb(t.panel_hover)))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _, _, cx| {
                        crate::shell::resize_fonts(by, cx);
                        cx.notify();
                    }),
                )
        };
        let pts = f32::from(fonts::text().size);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(button("smaller", "−", -1))
            .child(div().w(px(56.)).flex().justify_center().child(format!("{pts} pt")))
            .child(button("bigger", "+", 1))
            .child(button("actual", "Actual Size", 0))
            .child(div().text_size(px(11.5)).text_color(rgb(t.panel_dim)).child("⌘+ ⌘− ⌘0"))
    }

    /// A switch: a box, ticked when on, and what it does.
    fn check(&self, id: &'static str, label: &'static str, on: bool, cx: &mut Context<Self>, flip: fn(&mut App)) -> gpui::Stateful<gpui::Div> {
        let t = theme::theme();
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .cursor_default()
            .child(
                div()
                    .size(px(15.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border(px(if on { 0. } else { 1. }))
                    .border_color(rgb(t.panel_border))
                    .when(on, |d| d.bg(rgb(t.panel_chosen_bg)).text_color(rgb(t.panel_chosen_text)))
                    .text_size(px(11.))
                    .child(if on { "✓" } else { "" }),
            )
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, _, cx| {
                    flip(cx);
                    cx.notify();
                }),
            )
    }
}

/// A small apex in theme `t`'s colours: the columns' ground, two windows
/// in a column -- a text's tag and body (a line selected, a word in B2's
/// sweep and one in B3's) and a terminal's, the keys' window ringed in
/// the accent, printing in its sixteen -- and a second column's window.
fn sketch(t: &'static Theme) -> gpui::Div {
    let (w, h) = CARD;
    let mono = fonts::families(fonts::current()).1;
    let word = |s: &'static str, ink: u32| div().flex_none().text_color(rgb(ink)).child(s);
    let line = |parts: Vec<gpui::Div>| div().flex().flex_row().gap(px(2.)).h(px(8.)).items_center().children(parts);
    let tag = |name: &'static str| {
        div()
            .flex_none()
            .h(px(11.))
            .px(px(3.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .bg(rgb(t.tag_bg))
            .border_b(px(0.5))
            .border_color(rgb(t.border))
            .child(div().size(px(4.)).rounded_full().border(px(0.8)).border_color(rgb(t.text_dim)))
            .child(word(name, t.text))
            .child(word("Del Put", crate::text_element::mix(t.text_dim, t.tag_bg, 0.4)))
    };
    let (b2_wash, b2_ink) = t.sweep(true);
    let (b3_wash, b3_ink) = t.sweep(false);
    let text = div()
        .flex()
        .flex_col()
        .flex_1()
        .rounded(px(3.))
        .overflow_hidden()
        .border(px(0.5))
        .border_color(rgb(t.border))
        .child(tag("notes.md"))
        .child(
            div()
                .flex_1()
                .bg(rgb(t.body_bg))
                .px(px(4.))
                .py(px(2.))
                .flex()
                .flex_col()
                .child(line(vec![word("# Plan", t.text)]))
                .child(line(vec![word("run", t.text), div().px(px(1.)).bg(rgb(b2_wash)).child(word("mk", b2_ink)), word("then", t.text)]))
                .child(line(vec![word("see", t.text), div().px(px(1.)).bg(rgb(b3_wash)).child(word("x.go:4", b3_ink))]))
                .child(line(vec![div().bg(rgb(t.body_sel)).child(word("the chosen line", t.text))])),
        );
    let a = t.ansi;
    let term = div()
        .flex()
        .flex_col()
        .h(px(40.))
        .flex_none()
        .rounded(px(3.))
        .overflow_hidden()
        .border(px(1.))
        .border_color(rgb(t.accent))
        .child(tag("~/src/"))
        .child(
            div()
                .flex_1()
                .bg(rgb(t.body_bg))
                .px(px(4.))
                .py(px(2.))
                .flex()
                .flex_col()
                .child(line(vec![word("%", t.text_dim), word("mk test", t.text)]))
                .child(line(vec![word("ok", a[2]), word("3 passed", t.text), word("1 failed", a[1])]))
                .child(line(vec![word("warning", a[3]), word("x.go", a[4])])),
        );
    let second = div()
        .flex()
        .flex_col()
        .w(px(36.))
        .flex_none()
        .rounded(px(3.))
        .overflow_hidden()
        .border(px(0.5))
        .border_color(rgb(t.border))
        .child(tag("/"))
        .child(div().flex_1().bg(rgb(t.body_bg)).px(px(3.)).py(px(2.)).flex().flex_col().child(line(vec![word("bin/", t.text)])).child(line(vec![word("src/", t.text)])));
    div()
        .w(px(w))
        .h(px(h))
        .rounded(px(8.))
        .overflow_hidden()
        .bg(rgb(t.column))
        .p(px(4.))
        .flex()
        .flex_row()
        .gap(px(4.))
        .font_family(SharedString::from(mono))
        .text_size(px(6.))
        .line_height(px(8.))
        .child(div().flex_1().flex().flex_col().gap(px(3.)).child(text).child(term))
        .child(second)
}
