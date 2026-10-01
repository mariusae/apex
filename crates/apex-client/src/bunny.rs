//! Glenda, the bunny of apex's icon (`mac/glenda.svg`), embossed in an
//! empty column above its hints: her lines drawn three times -- light a
//! pixel up and to the left, dark a pixel down and to the right, and in
//! the ground's own ink between -- so she stands out of the ground as a
//! stamp does. Her pupils are drawn live, each moved toward the pointer
//! as far as its eye lets it, so she watches the mouse.

use gpui::prelude::*;
use gpui::{canvas, div, point, px, size, AnyElement, Bounds, Hsla};

use crate::text_element::{mix, rgb};

/// Her lines, from the icon, in its units: everything but the pupils
/// (drawn live), the colour gone -- strokes, the nose filled.
const LINES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="260 40 504 940" fill="none" stroke="black" stroke-width="20" stroke-linejoin="round" stroke-linecap="round">
<ellipse cx="392" cy="250" rx="72" ry="190" transform="rotate(-14 392 250)"/>
<ellipse cx="392" cy="262" rx="34" ry="140" transform="rotate(-14 392 262)" stroke-width="12"/>
<ellipse cx="632" cy="250" rx="72" ry="190" transform="rotate(14 632 250)"/>
<ellipse cx="632" cy="262" rx="34" ry="140" transform="rotate(14 632 262)" stroke-width="12"/>
<path d="M330 720 Q300 940 512 950 Q724 940 694 720"/>
<ellipse cx="440" cy="930" rx="60" ry="26"/>
<ellipse cx="584" cy="930" rx="60" ry="26"/>
<circle cx="512" cy="560" r="262"/>
<circle cx="512" cy="560" r="230" stroke-width="14"/>
<path d="M330 470 Q380 340 512 320" stroke-width="12"/>
<circle cx="428" cy="540" r="86" stroke-width="16"/>
<circle cx="596" cy="540" r="86" stroke-width="16"/>
<ellipse cx="512" cy="642" rx="26" ry="18" fill="black" stroke="none"/>
<path d="M512 660 Q512 690 486 696 M512 660 Q512 690 538 696" stroke-width="14"/>
<rect x="490" y="690" width="22" height="40" rx="4" stroke-width="12"/>
<rect x="512" y="690" width="22" height="40" rx="4" stroke-width="12"/>
<path d="M300 620 L400 636 M300 660 L400 656 M724 620 L624 636 M724 660 L624 656" stroke-width="12"/>
</svg>"##;

/// Her viewBox: where it starts, and how big it is, in the icon's units.
const VIEW: (f32, f32, f32, f32) = (260., 40., 504., 940.);
/// How tall she is drawn.
pub const HEIGHT: f32 = 96.;
/// Her eyes' centres, their pupils' radius, and how far a pupil goes.
const EYES: [(f32, f32); 2] = [(428., 540.), (596., 540.)];
const PUPIL: f32 = 46.;
const REACH: f32 = 30.;

fn scale() -> f32 {
    HEIGHT / VIEW.3
}

pub fn width() -> f32 {
    VIEW.2 * scale()
}

/// Glenda, `HEIGHT` tall, embossed on the ground `ground` (whose own ink
/// she is drawn in), her eyes on the pointer.
pub fn bunny(ground: u32, text: u32) -> AnyElement {
    let dark = crate::theme::is_dark();
    // light from the top left: a lighter edge there, a darker one under
    let (light, shade) = if dark { (mix(ground, 0xffffff, 0.10), mix(ground, 0x000000, 0.55)) } else { (mix(ground, 0xffffff, 0.85), mix(ground, 0x000000, 0.16)) };
    let face = mix(ground, text, if dark { 0.10 } else { 0.07 });
    let (w, h) = (width(), HEIGHT);
    let layer = |dx: f32, dy: f32, ink: u32| div().absolute().left(px(dx)).top(px(dy)).w(px(w)).h(px(h)).child(gpui::svg().data(LINES.as_bytes()).size_full().text_color(rgb(ink)));
    // the pupils, toward the pointer, embossed as the lines are
    let eyes = canvas(
        |_, _, _| {},
        move |b: Bounds<gpui::Pixels>, _, window, _| {
            let s = scale();
            let mouse = window.mouse_position();
            let at = |(x, y): (f32, f32)| point(b.left() + px((x - VIEW.0) * s), b.top() + px((y - VIEW.1) * s));
            for &eye in &EYES {
                let c = at(eye);
                let (dx, dy) = (f32::from(mouse.x - c.x), f32::from(mouse.y - c.y));
                let d = (dx * dx + dy * dy).sqrt().max(0.001);
                // nearer than its reach, it looks less far off
                let k = (REACH * s).min(d * 0.25) / d;
                let p = point(c.x + px(dx * k), c.y + px(dy * k));
                let r = PUPIL * s;
                let disc = |off: f32, ink: Hsla| gpui::fill(Bounds::new(point(p.x - px(r) + px(off), p.y - px(r) + px(off)), size(px(2. * r), px(2. * r))), ink).corner_radii(px(r));
                window.paint_quad(disc(-0.75, rgb(light)));
                window.paint_quad(disc(0.75, rgb(shade)));
                window.paint_quad(disc(0., rgb(mix(face, shade, 0.55))));
            }
        },
    )
    .absolute()
    .left(px(0.))
    .top(px(0.))
    .w(px(w))
    .h(px(h));
    div().relative().flex_none().w(px(w)).h(px(h)).child(layer(-1., -1., light)).child(layer(1., 1., shade)).child(layer(0., 0., face)).child(eyes).into_any_element()
}
