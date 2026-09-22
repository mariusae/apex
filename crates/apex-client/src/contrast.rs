//! Automatic contrast correction for terminals. Programs choose their
//! colours for a dark terminal and name them outright (a 256-colour
//! index, a truecolor triple), so on light paper a `ls` comes out in
//! pale yellow and a `lazygit` in greys nobody can read; and the few
//! that choose for light paper do the reverse on dark. The client
//! checks every cell's ink against the paper it sits on as it paints,
//! and where the two do not read, moves the ink -- and only the ink --
//! in lightness, toward the ink of the paper it is on, until they do.
//! The ink's hue and chroma stay (a yellow stays a yellow, deeper),
//! the hue turned partway toward the nearest of the theme's own
//! sixteen so the corrected colours belong with the theme's. "Reads"
//! is WCAG AA (4.5:1) in the eye of a normal viewer and of a
//! deuteranope both, since a red reads lighter to the latter and one
//! that passes for the first may not for the second. An ink is not
//! set on the edge of reading but mirrored about it: one that lay a
//! little beyond the edge lands a little inside, one far beyond lands
//! deep, so the shades a program tells its things apart by stay apart
//! -- and to a deuteranope, who has lightness where others have hue,
//! stay two colours -- and a dark terminal's bright red is a light
//! one's deep red, which is what bright means on paper. The moves are
//! in Oklab, where a step in lightness looks the same size on any
//! hue, and every answer is cached, so a cell costs a lookup.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::theme::Theme;

/// WCAG AA for text: 4.5 to 1.
pub const TARGET: f32 = 4.5;

/// The nearest theme hue is turned toward this much of the way, and
/// only when it is this close (radians): far enough to belong, not so
/// far that two of a program's colours become one.
const HARMONY_REACH: f32 = 0.7;
const HARMONY_PULL: f32 = 0.5;

/// Under this Oklab chroma a colour is a grey, with no hue to keep.
const GREY: f32 = 0.04;

/// How far inside the edge of reading an ink lands, of how far beyond
/// it it lay.
const MIRROR: f32 = 0.5;

type Lin = [f32; 3];

fn to_linear(c: u32) -> f32 {
    let c = (c & 0xff) as f32 / 255.;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(c: f32) -> u32 {
    let c = c.clamp(0., 1.);
    let s = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1. / 2.4) - 0.055 };
    (s * 255.).round() as u32
}

fn linear(rgb: u32) -> Lin {
    [to_linear(rgb >> 16), to_linear(rgb >> 8), to_linear(rgb)]
}

fn pack(l: Lin) -> u32 {
    (from_linear(l[0]) << 16) | (from_linear(l[1]) << 8) | from_linear(l[2])
}

/// WCAG relative luminance.
fn luminance(l: Lin) -> f32 {
    0.2126 * l[0].clamp(0., 1.) + 0.7152 * l[1].clamp(0., 1.) + 0.0722 * l[2].clamp(0., 1.)
}

/// WCAG contrast ratio, 1 (none) to 21 (black on white).
pub fn contrast(a: Lin, b: Lin) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// What a deuteranope sees of a colour (Machado, Oliveira and
/// Fernandes 2009, deuteranopia at full severity, in linear RGB).
const DEUTERANOPIA: [[f32; 3]; 3] = [
    [0.367322, 0.860646, -0.227968],
    [0.280085, 0.672501, 0.047413],
    [-0.011820, 0.042940, 0.968881],
];

fn deuteranope(l: Lin) -> Lin {
    let row = |r: [f32; 3]| r[0] * l[0] + r[1] * l[1] + r[2] * l[2];
    [row(DEUTERANOPIA[0]), row(DEUTERANOPIA[1]), row(DEUTERANOPIA[2])]
}

/// The contrast as the worse of the two viewers sees it.
fn contrast_for_all(fg: Lin, bg: Lin) -> f32 {
    contrast(fg, bg).min(contrast(deuteranope(fg), deuteranope(bg)))
}

/// Whether ink reads on paper: AA for a normal viewer and a
/// deuteranope both.
pub fn legible(fg: u32, bg: u32) -> bool {
    contrast_for_all(linear(fg), linear(bg)) >= TARGET
}

// Oklab, after Björn Ottosson (2020).

fn oklab(c: Lin) -> [f32; 3] {
    let l = (0.4122214708 * c[0] + 0.5363325363 * c[1] + 0.0514459929 * c[2]).cbrt();
    let m = (0.2119034982 * c[0] + 0.6806995451 * c[1] + 0.1073969566 * c[2]).cbrt();
    let s = (0.0883024619 * c[0] + 0.2817188376 * c[1] + 0.6299787005 * c[2]).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn from_oklab(lab: [f32; 3]) -> Lin {
    let [ll, a, b] = lab;
    let l = (ll + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (ll - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (ll - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

/// Lightness, chroma and hue (radians) of a colour.
fn oklch(c: Lin) -> (f32, f32, f32) {
    let [l, a, b] = oklab(c);
    (l, a.hypot(b), b.atan2(a))
}

fn in_gamut(c: Lin) -> bool {
    c.iter().all(|&x| (-0.0005..=1.0005).contains(&x))
}

/// The colour of this lightness and hue, at this chroma if sRGB has
/// it and else at the most it has.
fn fit(l: f32, c: f32, h: f32) -> Lin {
    let at = |c: f32| from_oklab([l, c * h.cos(), c * h.sin()]);
    if in_gamut(at(c)) {
        return at(c);
    }
    let (mut lo, mut hi) = (0., c);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.;
        if in_gamut(at(mid)) { lo = mid } else { hi = mid }
    }
    at(lo).map(|x| x.clamp(0., 1.))
}

fn hue_distance(a: f32, b: f32) -> f32 {
    let d = (b - a).rem_euclid(std::f32::consts::TAU);
    if d > std::f32::consts::PI { d - std::f32::consts::TAU } else { d }
}

/// The hue turned partway toward the nearest of the theme's own, when
/// one is near: the chromatic twelve of the sixteen, the greys having
/// no hue to offer.
fn harmonise(h: f32, c: f32, th: &Theme) -> f32 {
    if c < GREY {
        return h;
    }
    let nearest = th
        .ansi
        .iter()
        .enumerate()
        .filter(|(i, _)| !matches!(i, 0 | 7 | 8 | 15))
        .map(|(_, &rgb)| oklch(linear(rgb)))
        .filter(|&(_, tc, _)| tc >= GREY)
        .map(|(_, _, th)| hue_distance(h, th))
        .min_by(|a, b| a.abs().total_cmp(&b.abs()));
    match nearest {
        Some(d) if d.abs() <= HARMONY_REACH => h + d * HARMONY_PULL,
        _ => h,
    }
}

/// The ink `fg` as it should be drawn on the paper `bg`: itself where
/// it reads, else moved in lightness until it does and half as far
/// again.
pub fn correct(fg: u32, bg: u32, th: &Theme) -> u32 {
    let dark = std::ptr::eq(th, &crate::theme::DARK);
    let key = (fg, bg, dark);
    if let Some(v) = CACHE.with(|c| c.borrow().get(&key).copied()) {
        return v;
    }
    let v = compute(fg, bg, th);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        c.insert(key, v);
    });
    v
}

const CACHE_CAP: usize = 8192;

thread_local! {
    static CACHE: RefCell<HashMap<(u32, u32, bool), u32>> = RefCell::new(HashMap::new());
}

fn compute(fg: u32, bg: u32, th: &Theme) -> u32 {
    if legible(fg, bg) {
        return fg;
    }
    let f = linear(fg);
    let b = linear(bg);
    let (l, c, h) = oklch(f);
    let h = harmonise(h, c, th);
    // toward whichever of black and white the paper reads better
    // against: down on light paper, up on dark, whatever the theme
    let end = if contrast_for_all([0.; 3], b) >= contrast_for_all([1.; 3], b) { 0. } else { 1. };
    // judged as drawn, in eight bits: the edge found in floats can
    // fall a hair short once packed
    let at = |l: f32| pack(fit(l, c, h));
    let reads = |l: f32| contrast_for_all(linear(at(l)), b) >= TARGET;
    if reads(l) {
        return at(l);
    }
    if !reads(end) {
        // no ink of this hue reads on this paper: as far as it goes
        return at(end);
    }
    // the edge of reading, then the mirror
    let (mut lo, mut hi) = (l, end);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.;
        if reads(mid) { hi = mid } else { lo = mid }
    }
    let edge = hi;
    let mirrored = (edge + (edge - l) * MIRROR).clamp(0., 1.);
    if reads(mirrored) { at(mirrored) } else { at(edge) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DARK, LIGHT};

    fn hue(rgb: u32) -> f32 {
        oklch(linear(rgb)).2
    }
    fn lightness(rgb: u32) -> f32 {
        oklch(linear(rgb)).0
    }
    fn deut_contrast(fg: u32, bg: u32) -> f32 {
        contrast(deuteranope(linear(fg)), deuteranope(linear(bg)))
    }

    #[test]
    fn ink_that_reads_is_left_exactly_as_it_is() {
        assert_eq!(correct(LIGHT.text, LIGHT.body_bg, &LIGHT), LIGHT.text);
        assert_eq!(correct(DARK.text, DARK.body_bg, &DARK), DARK.text);
        // a program's own legible pair, on either paper
        assert_eq!(correct(0x1A1A1A, 0xF0F0F0, &LIGHT), 0x1A1A1A);
        assert_eq!(correct(0xF0F0F0, 0x1A1A1A, &DARK), 0xF0F0F0);
    }

    #[test]
    fn a_dark_terminals_yellow_deepens_on_light_paper_and_stays_a_yellow() {
        let y = 0xD6C000;
        let got = correct(y, LIGHT.body_bg, &LIGHT);
        assert_ne!(got, y);
        assert!(legible(got, LIGHT.body_bg), "{got:06x}");
        assert!(lightness(got) < lightness(y), "darker");
        assert!(hue_distance(hue(y), hue(got)).abs() < 0.4, "still a yellow: {got:06x}");
    }

    #[test]
    fn a_light_terminals_navy_lifts_on_dark_paper() {
        let navy = 0x0000AA;
        let got = correct(navy, DARK.body_bg, &DARK);
        assert!(legible(got, DARK.body_bg), "{got:06x}");
        assert!(lightness(got) > lightness(navy), "lighter");
        assert!(hue_distance(hue(navy), hue(got)).abs() < 0.4, "still a blue: {got:06x}");
    }

    #[test]
    fn ink_on_a_programs_own_dark_block_goes_light_whatever_the_theme() {
        // a dark grey block with the theme's black ink, on light paper
        let got = correct(LIGHT.text, 0x333333, &LIGHT);
        assert!(legible(got, 0x333333), "{got:06x}");
        assert!(lightness(got) > 0.5);
        // and the theme's white ink on a program's white block, on dark
        let got = correct(DARK.text, 0xF8F8F8, &DARK);
        assert!(legible(got, 0xF8F8F8), "{got:06x}");
        assert!(lightness(got) < 0.5);
    }

    #[test]
    fn a_red_that_reads_to_most_eyes_but_not_a_deuteranopes_is_corrected_for_them() {
        let red = 0xE00000;
        assert!(contrast(linear(red), linear(LIGHT.body_bg)) >= TARGET, "reads to a normal viewer as it is");
        assert!(deut_contrast(red, LIGHT.body_bg) < TARGET, "and not to a deuteranope");
        let got = correct(red, LIGHT.body_bg, &LIGHT);
        assert_ne!(got, red);
        assert!(deut_contrast(got, LIGHT.body_bg) >= TARGET, "{got:06x}");
        assert!(hue_distance(hue(red), hue(got)).abs() < 0.4, "still a red: {got:06x}");
    }

    #[test]
    fn shades_a_program_tells_apart_stay_apart_to_a_deuteranope() {
        // xterm's red and yellow on light paper: to a deuteranope both
        // are olives, told apart by the yellow's lightness; the yellow
        // needs correcting and the red (just) does not, and set on the
        // edge the two would be one
        let paper = LIGHT.body_bg;
        let r = correct(0xCC241D, paper, &LIGHT);
        let y = correct(0xB08A00, paper, &LIGHT);
        assert!(legible(r, paper) && legible(y, paper));
        let dl = (lightness(r) - lightness(y)).abs();
        assert!(dl > 0.03, "{r:06x} {y:06x}: dl {dl}");
        // the bright red and green, both far past the edge: what a
        // deuteranope had to tell them apart by -- lightness and the
        // blue-yellow axis, the red-green one being lost on them --
        // survives the mirror at the mirror's scale, not collapsed
        let apart = |a: u32, b: u32| {
            let [al, _, ab] = oklab(deuteranope(linear(a)));
            let [bl, _, bb] = oklab(deuteranope(linear(b)));
            (al - bl).hypot(ab - bb)
        };
        let before = apart(0xFF5555, 0x55C055);
        let after = apart(correct(0xFF5555, paper, &LIGHT), correct(0x55C055, paper, &LIGHT));
        assert!(after >= before * MIRROR * 0.8, "before {before} after {after}");
    }

    #[test]
    fn a_dark_terminals_bright_is_a_light_ones_deep() {
        // xterm's yellow and bright yellow on light paper: the brighter
        // comes out the deeper, and the two stay two
        let paper = LIGHT.body_bg;
        let y = correct(0xB08A00, paper, &LIGHT);
        let by = correct(0xD6C000, paper, &LIGHT);
        assert!(lightness(by) < lightness(y) - 0.03, "{y:06x} {by:06x}");
        // and on dark paper a light terminal's navy and its deeper navy:
        // the deeper comes out the lighter
        let n = correct(0x0000AA, DARK.body_bg, &DARK);
        let dn = correct(0x000066, DARK.body_bg, &DARK);
        assert!(lightness(dn) > lightness(n) + 0.03, "{n:06x} {dn:06x}");
    }

    #[test]
    fn greys_are_not_given_a_hue() {
        let got = correct(0x888888, LIGHT.body_bg, &LIGHT);
        assert!(legible(got, LIGHT.body_bg));
        let (_, c, _) = oklch(linear(got));
        assert!(c < 0.02, "{got:06x} chroma {c}");
    }

    #[test]
    fn black_on_black_and_white_on_white_come_out_the_far_side() {
        assert!(legible(correct(0x000000, 0x000000, &LIGHT), 0x000000));
        assert!(legible(correct(0xFFFFFF, 0xFFFFFF, &DARK), 0xFFFFFF));
    }

    #[test]
    fn every_answer_reads_to_both_viewers_or_is_as_far_as_the_hue_goes() {
        let papers = [LIGHT.body_bg, DARK.body_bg, 0x000000, 0xFFFFFF, 0x333333, 0xCCCCCC, 0x1C4FD6, 0xFFD700];
        for r in (0..=255).step_by(51) {
            for g in (0..=255).step_by(51) {
                for b in (0..=255).step_by(51) {
                    let fg = (r << 16) | (g << 8) | b;
                    for &bg in &papers {
                        for th in [&LIGHT, &DARK] {
                            let got = correct(fg, bg, th);
                            let extreme = got == 0 || got == 0xFFFFFF;
                            assert!(legible(got, bg) || extreme, "{fg:06x} on {bg:06x}: {got:06x}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_themes_own_sixteen_read_on_their_paper_once_corrected() {
        for th in [&LIGHT, &DARK] {
            for (i, &c) in th.ansi.iter().enumerate() {
                let got = correct(c, th.body_bg, th);
                assert!(legible(got, th.body_bg), "ansi {i} {c:06x} -> {got:06x}");
            }
        }
    }

    #[test]
    fn the_theme_pulls_a_near_hue_partway_and_leaves_a_far_one() {
        // a near miss of the light theme's green: turned toward it, not onto it
        let green = hue(LIGHT.ansi[2]);
        let off = 0x60D030; // a yellower green, low contrast on light paper
        let got = correct(off, LIGHT.body_bg, &LIGHT);
        let before = hue_distance(green, hue(off)).abs();
        let after = hue_distance(green, hue(got)).abs();
        assert!(after < before, "nearer the theme's green: {before} -> {after}");
        assert!(after > 0.005, "but not on it: {after}");
    }
}


