//! The space bunny of apex's icon (`mac/space-bunny.svg`), embossed in
//! an empty column above its hints: the drawing three times -- light a
//! pixel up and to the left, dark a pixel down and to the right, and in
//! the ground's own ink between -- so it stands out of the ground as a
//! stamp does. The pupils of the face in the helmet are cut out of it
//! (`assets/space-bunny.svg` masks them) and drawn live, each moved
//! toward the pointer as far as its eye lets it, so it watches the mouse.

use gpui::prelude::*;
use gpui::{canvas, div, point, px, size, AnyElement, Bounds, Hsla};

use crate::text_element::{mix, rgb};

/// The drawing, from the icon: its outline stroked and its ink filled,
/// in whole units (a tenth of a pixel where it is drawn), specks too
/// small to show left out, and the pupils masked away.
const DRAWING: &[u8] = include_bytes!("../assets/space-bunny.svg");

/// Its viewBox: where it starts, and how big it is, in the icon's units.
const VIEW: (f32, f32, f32, f32) = (230., 46., 1702., 2245.);
/// How tall it is drawn.
pub const HEIGHT: f32 = 200.;

/// An eye: the middle of its white, how far a pupil's middle goes from
/// there across and up and down, and the pupil's half-width and
/// half-height (slits, as drawn).
struct Eye {
    at: (f32, f32),
    reach: (f32, f32),
    pupil: (f32, f32),
}

const EYES: [Eye; 2] = [
    Eye { at: (1360., 1250.), reach: (48., 5.), pupil: (20., 46.) },
    Eye { at: (1658., 1270.), reach: (30., 5.), pupil: (19., 40.) },
];

fn scale() -> f32 {
    HEIGHT / VIEW.3
}

pub fn width() -> f32 {
    VIEW.2 * scale()
}

/// The space bunny, `HEIGHT` tall, embossed on the ground `ground` (whose
/// own ink it is drawn in), its eyes on the pointer.
pub fn bunny(ground: u32, text: u32) -> AnyElement {
    let dark = crate::theme::is_dark();
    // light from the top left: a lighter edge there, a darker one under
    let (light, shade) = if dark { (mix(ground, 0xffffff, 0.10), mix(ground, 0x000000, 0.55)) } else { (mix(ground, 0xffffff, 0.85), mix(ground, 0x000000, 0.16)) };
    let face = mix(ground, text, if dark { 0.10 } else { 0.07 });
    let (w, h) = (width(), HEIGHT);
    let layer = |dx: f32, dy: f32, ink: u32| div().absolute().left(px(dx)).top(px(dy)).w(px(w)).h(px(h)).child(gpui::svg().data(DRAWING).size_full().text_color(rgb(ink)));
    // the pupils, toward the pointer, embossed as the drawing is
    let eyes = canvas(
        |_, _, _| {},
        move |b: Bounds<gpui::Pixels>, _, window, _| {
            let s = scale();
            let mouse = window.mouse_position();
            let at = |(x, y): (f32, f32)| point(b.left() + px((x - VIEW.0) * s), b.top() + px((y - VIEW.1) * s));
            for eye in &EYES {
                let c = at(eye.at);
                let (dx, dy) = (f32::from(mouse.x - c.x), f32::from(mouse.y - c.y));
                let d = (dx * dx + dy * dy).sqrt().max(0.001);
                // nearer than its reach, it looks less far off
                let k = ((d * 0.25) / (eye.reach.0 * s)).min(1.) / d;
                let p = point(c.x + px(dx * k * eye.reach.0 * s), c.y + px(dy * k * eye.reach.1 * s));
                let (rx, ry) = (eye.pupil.0 * s, eye.pupil.1 * s);
                let slit = |off: f32, ink: Hsla| gpui::fill(Bounds::new(point(p.x - px(rx) + px(off), p.y - px(ry) + px(off)), size(px(2. * rx), px(2. * ry))), ink).corner_radii(px(rx));
                window.paint_quad(slit(-0.75, rgb(light)));
                window.paint_quad(slit(0.75, rgb(shade)));
                window.paint_quad(slit(0., rgb(mix(face, shade, 0.55))));
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
