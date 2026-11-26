//! The themes, as a Mac app of now dresses (the modern-mac branch's
//! experiment; acme's tinted papers are on main): palettes to live
//! with and choose among (View ▸ Theme: Alabaster, Xcode, Classic, Acme,
//! GitHub, Nova, rsms), each a light and a dark (Acme the same in both), and the appearance choosing between those
//! (View: Light, Dark, System). Every palette lays out the same way: a
//! paper for bodies and a header for the tags over them, hairlines where
//! acme has black borders, one accent for what is going on. The choices
//! are kept beside the other state files.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Light,
    Dark,
    System,
}

pub struct Theme {
    // acme's texts
    pub body_bg: u32,
    pub body_sel: u32,
    pub body_border: u32,
    pub tag_bg: u32,
    pub tag_sel: u32,
    pub tag_border: u32,
    /// The ink.
    pub text: u32,
    /// The secondary ink: a tag's commands after its name, the column
    /// tags and the top row, a sidebar row's second line.
    pub text_dim: u32,
    /// What is going on or chosen: live and working handles, a chosen
    /// row, the focus.
    pub accent: u32,
    /// The borders between columns and windows (acme's black).
    pub border: u32,
    /// A column where no window is (acme's white).
    pub column: u32,
    // the handles
    pub dirty: u32,
    pub stale: u32,
    pub fenced: u32,
    /// Work going on: the bar across the top of a terminal whose program
    /// says so (OSC 9;4), and on light the colour a working handle's stipple breathes from,
    /// so the two say the same thing. A blue chosen under a deuteranopia
    /// simulation to stand apart from every handle colour it can sit
    /// beside -- the dirty blue and the notification's above all -- and
    /// from both papers.
    pub progress: u32,
    pub exec_hl: u32,
    pub look_hl: u32,
    // the title bar
    pub strip: u32,
    // the overlays (picker, finder, switcher) and their fields
    pub panel_bg: u32,
    pub panel_border: u32,
    pub panel_text: u32,
    pub panel_dim: u32,
    pub panel_hover: u32,
    pub panel_chosen_bg: u32,
    pub panel_chosen_text: u32,
    pub panel_accent: u32,
    pub panel_danger_hover: u32,
    pub field_sel: u32,
    /// The sixteen ANSI colours a program in a terminal gets when it
    /// names one and has not set the palette itself.
    pub ansi: [u32; 16],
    /// apex diff's added and removed lines' tints (blue and orange, as
    /// the colour-blind themes have them).
    pub diff_add: u32,
    pub diff_del: u32,
}

/// GitHub Light Colorblind (Primer's `light_colorblind`, as the GitHub
/// VS Code theme draws it), lifted a little off pure white: the paper is
/// its canvas.subtle (#f6f8fa) and the headers and sidebar a step below
/// that, the ink its gray 8 (#32383f) rather than fg.default, so neither
/// end is at the extreme; sheets and menus stay white, lifted off the
/// paper as a Mac popover is. accent.fg (#0969da) as the accent and
/// caret and, at 20%, the selection; and where GitHub's other themes
/// have red and green, this one's orange (danger, attention) and blue
/// (success): B2's sweep orange, B3's blue, the terminal's ANSI red and
/// green the same orange and blue.
/// (No longer offered: the contrast's tests read against it, and its
/// dark, `DARK`, which the contrast's cache tells apart.)
#[cfg_attr(not(test), allow(dead_code))]
pub const LIGHT: Theme = Theme {
    body_bg: 0xF6F8FA,
    body_sel: 0xC7DBF4,
    body_border: 0xD0D7DE,
    tag_bg: 0xEAEEF2,
    tag_sel: 0xBDD3ED,
    tag_border: 0xD0D7DE,
    text: 0x32383F,
    text_dim: 0x57606A,
    accent: 0x0969DA,
    border: 0xD0D7DE,
    column: 0xEFF2F5,
    dirty: 0x424A53,
    stale: 0x9A6700,
    fenced: 0xB35900,
    progress: 0x0969DA,
    exec_hl: 0xB35900,
    look_hl: 0x0969DA,
    strip: 0xE6EAEF,
    panel_bg: 0xFFFFFF,
    panel_border: 0xD0D7DE,
    panel_text: 0x24292F,
    panel_dim: 0x6E7781,
    panel_hover: 0xF1F3F6,
    panel_chosen_bg: 0x0969DA,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0x0969DA,
    panel_danger_hover: 0xFFF5E8,
    field_sel: 0xC7DBF4,
    // xterm's, as the server sent them before the theme
    ansi: [0x24292F, 0xB35900, 0x0550AE, 0x4D2D00, 0x0969DA, 0x8250DF, 0x1B7C83, 0x6E7781, 0x57606A, 0x8A4600, 0x0969DA, 0x633C01, 0x218BFF, 0xA475F9, 0x3192AA, 0x8C959F],
    diff_add: 0xE9F7FF,
    diff_del: 0xFFF5E7,
};

/// GitHub Dark Dimmed (Primer's `dark_dimmed`), GitHub's own dark that
/// is not so dark: #22272e canvas, #2d333b for headers, the sidebar and
/// sheets, #adbac7 ink, accent.fg #539bf5 and accent.emphasis #316dca
/// for a chosen row -- with the colour-blind themes' orange for red and
/// blue for green kept, from Dimmed's own orange and blue scales, the
/// terminal's ANSI included.
pub const DARK: Theme = Theme {
    body_bg: 0x22272E,
    body_sel: 0x2C3E56,
    body_border: 0x545D68,
    tag_bg: 0x2D333B,
    tag_sel: 0x354860,
    tag_border: 0x444C56,
    text: 0xADBAC7,
    text_dim: 0x768390,
    accent: 0x539BF5,
    border: 0x373E47,
    column: 0x1C2128,
    dirty: 0xADBAC7,
    stale: 0xC69026,
    fenced: 0xE0823D,
    progress: 0x539BF5,
    exec_hl: 0xAE5622,
    look_hl: 0x316DCA,
    strip: 0x2D333B,
    panel_bg: 0x2D333B,
    panel_border: 0x444C56,
    panel_text: 0xADBAC7,
    panel_dim: 0x768390,
    panel_hover: 0x373E47,
    panel_chosen_bg: 0x316DCA,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0x539BF5,
    panel_danger_hover: 0x4D2F22,
    field_sel: 0x2C3E56,
    // the same hues, lit for the dark paper: black a shade of it, white
    // the ink, the rest lighter and a little softer; the bright ones
    // brighter still
    ansi: [0x545D68, 0xF69D50, 0x539BF5, 0xC69026, 0x539BF5, 0xB083F0, 0x39C5CF, 0x909DAB, 0x636E7B, 0xFFBC6F, 0x6CB6FF, 0xDAAA3F, 0x6CB6FF, 0xDCBDFB, 0x56D4DD, 0xCDD9E5],
    diff_add: 0x243145,
    diff_del: 0x372E2C,
};

/// The colours a palette chooses; the rest of a theme follows from them
/// (`make`), as GitHub's are laid out: popovers (sheets, menus) on
/// `popover`, a chosen row in `chosen` with white on it, B2 and B3's
/// sweeps a pale wash of `exec` and `look`, the text in that colour (`Theme::sweep`).
struct Keys {
    paper: u32,
    sel: u32,
    thumb: u32,
    header: u32,
    header_sel: u32,
    line: u32,
    ink: u32,
    dim: u32,
    faint: u32,
    accent: u32,
    chosen: u32,
    column: u32,
    sidebar: u32,
    dirty: u32,
    stale: u32,
    fenced: u32,
    exec: u32,
    look: u32,
    popover: u32,
    hover: u32,
    danger_hover: u32,
    ansi: [u32; 16],
    diff_add: u32,
    diff_del: u32,
}

const fn make(k: Keys) -> Theme {
    Theme {
        body_bg: k.paper,
        body_sel: k.sel,
        body_border: k.thumb,
        tag_bg: k.header,
        tag_sel: k.header_sel,
        tag_border: k.line,
        text: k.ink,
        text_dim: k.dim,
        accent: k.accent,
        border: k.line,
        column: k.column,
        dirty: k.dirty,
        stale: k.stale,
        fenced: k.fenced,
        progress: k.accent,
        exec_hl: k.exec,
        look_hl: k.look,
        strip: k.sidebar,
        panel_bg: k.popover,
        panel_border: k.line,
        panel_text: k.ink,
        panel_dim: k.faint,
        panel_hover: k.hover,
        panel_chosen_bg: k.chosen,
        panel_chosen_text: 0xFFFFFF,
        panel_accent: k.accent,
        panel_danger_hover: k.danger_hover,
        field_sel: k.sel,
        ansi: k.ansi,
        diff_add: k.diff_add,
        diff_del: k.diff_del,
    }
}

/// The terminal's sixteen on light and on dark paper for every palette
/// but GitHub's own: GitHub's colour-blind ones (orange for red, blue for
/// green), since the reader is colour-blind whatever the paper.
const ANSI_LIGHT: [u32; 16] = [0x24292F, 0xB35900, 0x0550AE, 0x4D2D00, 0x0969DA, 0x8250DF, 0x1B7C83, 0x6E7781, 0x57606A, 0x8A4600, 0x0969DA, 0x633C01, 0x218BFF, 0xA475F9, 0x3192AA, 0x8C959F];
const ANSI_DARK: [u32; 16] = [0x545D68, 0xF69D50, 0x539BF5, 0xC69026, 0x539BF5, 0xB083F0, 0x39C5CF, 0x909DAB, 0x636E7B, 0xFFBC6F, 0x6CB6FF, 0xDAAA3F, 0x6CB6FF, 0xDCBDFB, 0x56D4DD, 0xCDD9E5];

/// Classic: acme's make -- cream paper, the tags a pale blue, a yellow
/// selection -- in the hues of go.dev's playground, toned down: its
/// #ffffdd paper warmed only a little off white, its ink (#202224) and
/// secondary (#6e7072), Go blue (#007d9c) for the accent. B2's sweep
/// burnt orange (#c85a00) and B3's a deeper blue (#2a5db0): Go's fuchsia
/// and teal were 46 apart under a deuteranopia simulation, these 114.
pub const CLASSIC_LIGHT: Theme = make(Keys {
    paper: 0xFCFCF2, sel: 0xF1E9A6, thumb: 0xD4D4C4, header: 0xEAF4F7, header_sel: 0xBCE3ED, line: 0xD6DCDD,
    ink: 0x202224, dim: 0x6E7072, faint: 0x8A8C8E, accent: 0x007D9C, chosen: 0x007D9C, column: 0xF5F5EA, sidebar: 0xE8EFF1,
    dirty: 0x2F3A40, stale: 0xC79A00, fenced: 0xCE3262, exec: 0xC85A00, look: 0x2A5DB0,
    popover: 0xFFFFFF, hover: 0xF0F4F5, danger_hover: 0xFBE3EA, ansi: ANSI_LIGHT, diff_add: 0xE1F3F8, diff_del: 0xFFF0DA,
});

/// Classic's dark: go.dev's own (#202224 paper, the playground's output ink
/// #e6e6e6, its dark link blue #50b7e0 as the accent), acme's dark
/// yellow selection.
pub const CLASSIC_DARK: Theme = make(Keys {
    paper: 0x202224, sel: 0x4A4526, thumb: 0x4A4C4E, header: 0x2B2D2F, header_sel: 0x1F4B5A, line: 0x3A3C3E,
    ink: 0xE6E6E6, dim: 0x9A9C9E, faint: 0x6E7072, accent: 0x50B7E0, chosen: 0x007D9C, column: 0x1A1B1D, sidebar: 0x26282A,
    dirty: 0xE6E6E6, stale: 0xFDDD00, fenced: 0xE0547A, exec: 0xC85A00, look: 0x2A5DB0,
    popover: 0x2B2D2F, hover: 0x333537, danger_hover: 0x4A2230, ansi: ANSI_DARK, diff_add: 0x16323D, diff_del: 0x3A2C1C,
});

/// Acme's own, as plan9port's acme.c (`iconinit`) and libdraw's
/// `menuhit` allocate them, for a reader who needs its contrast
/// (issue #5): the bodies DPaleyellow mixed with white (#ffffea),
/// selected in DDarkyellow (#eeee9e), their scrollbars DYellowgreen
/// (#99994c); the tags DPalebluegreen mixed with white (#eaffff),
/// selected in DPalegreygreen (#9eeeee), bordered DPurpleblue (#8888cc);
/// every text black, nothing faded; a modified window's box DMedblue
/// (#000099); B2's sweep #aa0000 and B3's #006600; the menus
/// DPalegreen mixed with white (#eaffea), chosen in DDarkgreen
/// (#448844), bordered DMedgreen (#88cc88); white between the columns,
/// and black lines where acme draws them, between windows and columns.
/// Acme has no dark: the same in either appearance.
pub const ACME: Theme = Theme {
    border: 0x000000,
    panel_border: 0x88CC88,
    ..make(Keys {
        paper: 0xFFFFEA, sel: 0xEEEE9E, thumb: 0x99994C, header: 0xEAFFFF, header_sel: 0x9EEEEE, line: 0x8888CC,
        ink: 0x000000, dim: 0x000000, faint: 0x444444, accent: 0x000099, chosen: 0x448844, column: 0xFFFFFF, sidebar: 0xEAFFFF,
        dirty: 0x000099, stale: 0xC79A00, fenced: 0xAA0000, exec: 0xAA0000, look: 0x006600,
        popover: 0xEAFFEA, hover: 0xD9F3D9, danger_hover: 0xFBE3EA, ansi: ANSI_LIGHT, diff_add: 0xE1F3F8, diff_del: 0xFFF0DA,
    })
};

/// Manifold's Terra (~/src/manifold, `Palette.terra`), grown from this
/// palette's Classic: earthy, of natural materials -- linen and oat
/// paper, walnut ink, a sage accent, a straw selection; B2's sweep its
/// burnt sienna and B3's its indigo (68 apart under a deuteranopia
/// simulation), its terminal's sixteen its own, which keep orange for
/// red and a blue-green verdigris for green (45 apart).
pub const TERRA_LIGHT: Theme = make(Keys {
    paper: 0xFAF7F0, sel: 0xEFE3B8, thumb: 0xD6CDBE, header: 0xF0EBE1, header_sel: 0xE6DCC0, line: 0xDDD5C8,
    ink: 0x2A2520, dim: 0x6B625A, faint: 0x8A8178, accent: 0x55703A, chosen: 0x55703A, column: 0xEFEAE0, sidebar: 0xF3EFE6,
    dirty: 0x2A2520, stale: 0xC79A00, fenced: 0x9B2C4A, exec: 0xA4501F, look: 0x3B5B7A,
    popover: 0xFDFBF6, hover: 0xEFEAE0, danger_hover: 0xF6E3E6,
    ansi: [0x2A2520, 0xA8431E, 0x2C6A5C, 0x86621A, 0x3B5B7A, 0x7D3F6E, 0x2A6F80, 0x6B625A, 0x5A524A, 0xC0582C, 0x3B8571, 0x9A7016, 0x5876A0, 0x97568A, 0x2F7F8F, 0x8A8075],
    diff_add: 0xE0E9F4, diff_del: 0xFBE6D8,
});

/// Terra by night, the one warm dark: warm charcoal on dark stone and
/// smoked oak, bone-coloured ink, a muted sage; its sienna and indigo
/// for B2 and B3 (73 apart), its terminal's sixteen (51).
pub const TERRA_DARK: Theme = make(Keys {
    paper: 0x221E1B, sel: 0x4A4128, thumb: 0x524B44, header: 0x2D2924, header_sel: 0x3A3324, line: 0x433D37,
    ink: 0xEBE4DA, dim: 0xA39A8F, faint: 0x776D62, accent: 0x94A982, chosen: 0x55703A, column: 0x1C1916, sidebar: 0x282420,
    dirty: 0xEBE4DA, stale: 0xD9B25A, fenced: 0xEE8EA2, exec: 0xB8602E, look: 0x4A6E96,
    popover: 0x2D2924, hover: 0x37322D, danger_hover: 0x4A2A30,
    ansi: [0x4A423A, 0xE0805A, 0x6FAFB4, 0xD9B25A, 0x8FA8CC, 0xC993B8, 0x9FCFC8, 0xB5ACA1, 0x877D72, 0xF09B74, 0x8DC4CB, 0xE8C679, 0xA8BFE0, 0xDBABCC, 0xBFE2DA, 0xF2ECE3],
    diff_add: 0x1E2B3A, diff_del: 0x3F2A1C,
});

/// Manifold's Spectrum (`Palette.spectrum`), from Claude's desktop app:
/// white pages on a #fafafa sidebar, #0b0b0b ink, black at 60% for what
/// is secondary, clay (#c6613f) for the accent and B2's sweep, Claude
/// Code's blue (#0073e6) for B3's and its blue selection (112 apart).
/// Its terminal's own red and green were 26 apart under a deuteranopia
/// simulation: the colour-blind sixteen here, as most palettes have.
pub const SPECTRUM_LIGHT: Theme = make(Keys {
    paper: 0xFFFFFF, sel: 0xBFDCF9, thumb: 0xD0D0D0, header: 0xF6F5F8, header_sel: 0xDCE9F7, line: 0xE3E3E3,
    ink: 0x0B0B0B, dim: 0x666666, faint: 0x737373, accent: 0xC6613F, chosen: 0xC6613F, column: 0xFAFAFA, sidebar: 0xFAFAFA,
    dirty: 0x0B0B0B, stale: 0xC79A00, fenced: 0xA4273E, exec: 0xC6613F, look: 0x0073E6,
    popover: 0xFFFFFF, hover: 0xF2F2F2, danger_hover: 0xFBE4E8, ansi: ANSI_LIGHT, diff_add: 0xEBF4FD, diff_del: 0xFAEBE6,
});

/// Spectrum by night: #1b1a1d pages, #312f34 popovers, #efefef ink, the
/// clay lifted (#d97757) for the accent, B3's blue deepened for its
/// wash (97 apart).
pub const SPECTRUM_DARK: Theme = make(Keys {
    paper: 0x1B1A1D, sel: 0x123F5E, thumb: 0x4A4A4E, header: 0x312F34, header_sel: 0x1F3E58, line: 0x3A383D,
    ink: 0xEFEFEF, dim: 0x9C9C9C, faint: 0x7A7A7A, accent: 0xD97757, chosen: 0xC6613F, column: 0x141316, sidebar: 0x1F1F1F,
    dirty: 0xEFEFEF, stale: 0xE6B422, fenced: 0xFF93A1, exec: 0xC6613F, look: 0x0A6FC4,
    popover: 0x312F34, hover: 0x3A383D, danger_hover: 0x4A2230, ansi: ANSI_DARK, diff_add: 0x162B39, diff_del: 0x3F2C25,
});

/// View ▸ Theme: which palette, each with its light and dark (which of
/// those is the appearance's to say: Light, Dark, System).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Palette {
    /// What was Classic (and keeps its word, `classic`): apex's own.
    Classic,
    Acme,
    Terra,
    Spectrum,
}

pub const PALETTES: [Palette; 4] = [Palette::Classic, Palette::Acme, Palette::Terra, Palette::Spectrum];

/// The palette until one is chosen: Apex's own (its place in `PALETTES`),
/// and what one chosen before, and gone since, comes back as.
const DEFAULT_PALETTE: u8 = 0;

impl Palette {
    pub fn title(self) -> &'static str {
        match self {
            Palette::Classic => "Apex",
            Palette::Acme => "Acme",
            Palette::Terra => "Terra",
            Palette::Spectrum => "Spectrum",
        }
    }
    fn word(self) -> &'static str {
        match self {
            Palette::Classic => "classic",
            Palette::Acme => "acme",
            Palette::Terra => "terra",
            Palette::Spectrum => "spectrum",
        }
    }
}

static PALETTE: AtomicU8 = AtomicU8::new(DEFAULT_PALETTE);

pub fn palette() -> Palette {
    PALETTES[PALETTE.load(Ordering::Relaxed) as usize % PALETTES.len()]
}

/// Chosen: kept in the `palette` state file.
pub fn set_palette(p: Palette) {
    PALETTE.store(PALETTES.iter().position(|x| *x == p).map_or(DEFAULT_PALETTE, |i| i as u8), Ordering::Relaxed);
    let f = crate::shell::state_file().with_file_name("palette");
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, format!("{}\n", p.word()));
}

/// The appearance until one is chosen: the system's (2).
static MODE: AtomicU8 = AtomicU8::new(2);
static SYSTEM_DARK: AtomicBool = AtomicBool::new(false);

pub fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Dark,
        2 => Mode::System,
        _ => Mode::Light,
    }
}

pub fn set_mode(m: Mode) {
    MODE.store(match m { Mode::Light => 0, Mode::Dark => 1, Mode::System => 2 }, Ordering::Relaxed);
    save(m);
}

/// What the system's appearance is, as the window reports it (System
/// follows it).
pub fn set_system_dark(dark: bool) {
    SYSTEM_DARK.store(dark, Ordering::Relaxed);
}

/// A step away from `base`: toward the ink on light paper, toward the
/// light on dark, so one rule gives the bar its tabs on either -- the
/// bar itself, what lies on it, and what lies in front, each a step or
/// two further off (ghostty's way with its tabs).
pub fn step(base: u32, n: u32) -> u32 {
    let toward = if is_dark() { 0xFF_FFFF } else { 0x00_0000 };
    crate::text_element::mix(base, toward, 0.045 * n as f32)
}

pub fn is_dark() -> bool {
    match mode() {
        Mode::Light => false,
        Mode::Dark => true,
        Mode::System => SYSTEM_DARK.load(Ordering::Relaxed),
    }
}

impl Theme {
    /// A B2 (`exec`) or B3 sweep, and the pill under what a click would
    /// take, as a chat app draws a link: a pale wash of the button's
    /// colour on the paper, the text in that colour -- the action's for
    /// B2, the blue for B3 -- deepened (or, on dark, lightened) to read on
    /// it. (background, ink)
    /// The faint wash on every place a window's Look word is while a look
    /// goes on there: B3's blue, a good deal paler than its sweep.
    pub fn look_mark(&self) -> u32 {
        crate::text_element::mix(self.body_bg, self.look_hl, if is_dark() { 0.17 } else { 0.07 })
    }

    pub fn sweep(&self, exec: bool) -> (u32, u32) {
        let c = if exec { self.exec_hl } else { self.look_hl };
        if is_dark() {
            // on dark, a wash that stands off the paper (1.3 to 1.6 from
            // it) and its text lighter (6 to 7.5 on it) -- a pale wash
            // reads on white as a dim one does not on black
            (crate::text_element::mix(self.body_bg, c, 0.36), crate::text_element::mix(c, 0xFFFFFF, 0.62))
        } else {
            (crate::text_element::mix(self.body_bg, c, 0.12), crate::text_element::mix(c, 0x000000, 0.2))
        }
    }
}

pub fn theme() -> &'static Theme {
    of(palette(), is_dark())
}

/// Palette `p`'s light or dark (a preview of it draws in these).
pub fn of(p: Palette, dark: bool) -> &'static Theme {
    match (p, dark) {
        (Palette::Classic, false) => &CLASSIC_LIGHT,
        (Palette::Classic, true) => &CLASSIC_DARK,
        // acme has no dark
        (Palette::Acme, _) => &ACME,
        (Palette::Terra, false) => &TERRA_LIGHT,
        (Palette::Terra, true) => &TERRA_DARK,
        (Palette::Spectrum, false) => &SPECTRUM_LIGHT,
        (Palette::Spectrum, true) => &SPECTRUM_DARK,
    }
}

/// The terminal's colours as the daemon should answer programs that
/// ask for them: the theme's ink, paper and sixteen.
pub fn term_colors() -> apex_server::proto::TermColors {
    let t = theme();
    apex_server::proto::TermColors { fg: t.text, bg: t.body_bg, ansi: t.ansi }
}

fn file() -> std::path::PathBuf {
    crate::shell::state_file().with_file_name("theme")
}

/// View ▸ Show Sidebar (off by default): the sessions down the left, as
/// vertical tabs, the one shown with its windows under it. Kept in the
/// `sidebar` state file.
static SIDEBAR: AtomicBool = AtomicBool::new(false);

pub fn sidebar() -> bool {
    SIDEBAR.load(Ordering::Relaxed)
}

pub fn set_sidebar(on: bool) {
    SIDEBAR.store(on, Ordering::Relaxed);
    let p = crate::shell::state_file().with_file_name("sidebar");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, if on { "shown\n" } else { "hidden\n" });
}

/// Settings ▸ Behaviour, a terminal's colours lifted (on by default): a terminal's ink
/// that does not read on its paper is moved until it does
/// (`contrast.rs`). Kept in the `contrast` state file.
static CONTRAST: AtomicBool = AtomicBool::new(true);
/// Settings ▸ Behaviour, the cursor blinking (on by default): the keys' caret (and a terminal's
/// cursor) blinks, or stays on -- the key window's ring says where the
/// keys go too.
static BLINK: AtomicBool = AtomicBool::new(true);

pub fn blink() -> bool {
    BLINK.load(Ordering::Relaxed)
}

pub fn set_blink(on: bool) {
    BLINK.store(on, Ordering::Relaxed);
    let p = crate::shell::state_file().with_file_name("blink");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, if on { "on\n" } else { "off\n" });
}

/// Settings ▸ Behaviour, the cursor moving smoothly: the keys' caret (and a terminal's cursor, where
/// the keys go) glides to where it moves, over a few frames, rather than
/// jumping there -- as Neovide's and Ghostty's cursor shaders do. Off
/// until chosen; kept in the `smoothcaret` state file.
static SMOOTH_CARET: AtomicBool = AtomicBool::new(false);

pub fn smooth_caret() -> bool {
    SMOOTH_CARET.load(Ordering::Relaxed)
}

pub fn set_smooth_caret(on: bool) {
    SMOOTH_CARET.store(on, Ordering::Relaxed);
    let p = crate::shell::state_file().with_file_name("smoothcaret");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, if on { "on\n" } else { "off\n" });
}

/// View ▸ Layout Animations: windows and columns glide to their places,
/// a window opening down from its top, the pointer riding along with the
/// window it goes to (`glide.rs`) -- the layout's own moves, the most
/// common in a session. Off, they are there at once. Off until chosen;
/// kept in the `layoutanim` state file.
static ANIMATE_LAYOUT: AtomicBool = AtomicBool::new(false);

pub fn animate_layout() -> bool {
    ANIMATE_LAYOUT.load(Ordering::Relaxed)
}

pub fn set_animate_layout(on: bool) {
    ANIMATE_LAYOUT.store(on, Ordering::Relaxed);
    let p = crate::shell::state_file().with_file_name("layoutanim");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, if on { "on\n" } else { "off\n" });
}

pub fn contrast() -> bool {
    CONTRAST.load(Ordering::Relaxed)
}

pub fn set_contrast(on: bool) {
    CONTRAST.store(on, Ordering::Relaxed);
    let p = crate::shell::state_file().with_file_name("contrast");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, if on { "on\n" } else { "off\n" });
}

/// The choice of last time, applied.
pub fn load() {
    let side = std::fs::read_to_string(crate::shell::state_file().with_file_name("sidebar")).map(|s| s.trim() == "shown").unwrap_or(false);
    SIDEBAR.store(side, Ordering::Relaxed);
    let contrast = std::fs::read_to_string(crate::shell::state_file().with_file_name("contrast")).map(|s| s.trim() != "off").unwrap_or(true);
    CONTRAST.store(contrast, Ordering::Relaxed);
    let blink = std::fs::read_to_string(crate::shell::state_file().with_file_name("blink")).map(|s| s.trim() != "off").unwrap_or(true);
    BLINK.store(blink, Ordering::Relaxed);
    let smooth = std::fs::read_to_string(crate::shell::state_file().with_file_name("smoothcaret")).map(|s| s.trim() == "on").unwrap_or(false);
    SMOOTH_CARET.store(smooth, Ordering::Relaxed);
    let layout = std::fs::read_to_string(crate::shell::state_file().with_file_name("layoutanim")).map(|s| s.trim() == "on").unwrap_or(false);
    ANIMATE_LAYOUT.store(layout, Ordering::Relaxed);
    let pal = std::fs::read_to_string(crate::shell::state_file().with_file_name("palette")).unwrap_or_default();
    // one no longer offered (Alabaster, Xcode, GitHub, Nova, rsms): Apex's
    let pal = pal.trim();
    if let Some(i) = PALETTES.iter().position(|p| p.word() == pal) {
        PALETTE.store(i as u8, Ordering::Relaxed);
    }
    let m = match std::fs::read_to_string(file()).map(|s| s.trim().to_string()).as_deref() {
        Ok("dark") => Mode::Dark,
        Ok("light") => Mode::Light,
        _ => Mode::System,
    };
    MODE.store(match m { Mode::Light => 0, Mode::Dark => 1, Mode::System => 2 }, Ordering::Relaxed);
}

fn save(m: Mode) {
    let p = file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, match m { Mode::Light => "light\n", Mode::Dark => "dark\n", Mode::System => "system\n" });
}

#[cfg(test)]
mod default_tests {
    use super::*;

    #[test]
    fn the_defaults_are_classic_and_the_systems() {
        assert_eq!(PALETTES[DEFAULT_PALETTE as usize], Palette::Classic);
        assert_eq!(Palette::Classic.title(), "Apex");
        assert_eq!(MODE.load(Ordering::Relaxed), 2, "the system's appearance");
    }

    /// `c` as a deuteranope sees it (Machado, Oliveira and Fernandes's
    /// matrix, severity 1), in CIELAB.
    fn seen(c: u32) -> [f64; 3] {
        const M: [[f64; 3]; 3] = [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.011820, 0.042940, 0.968881]];
        let lin = |v: u32| {
            let v = v as f64 / 255.;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let rgb = [lin((c >> 16) & 255), lin((c >> 8) & 255), lin(c & 255)];
        let s: Vec<f64> = M.iter().map(|row| row.iter().zip(rgb).map(|(m, v)| m * v).sum::<f64>().clamp(0., 1.)).collect();
        let (x, y, z) = (0.4124 * s[0] + 0.3576 * s[1] + 0.1805 * s[2], 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2], 0.0193 * s[0] + 0.1192 * s[1] + 0.9505 * s[2]);
        let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16. / 116. };
        let (fx, fy, fz) = (f(x / 0.95047), f(y), f(z / 1.08883));
        [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
    }

    fn apart(a: u32, b: u32) -> f64 {
        let (a, b) = (seen(a), seen(b));
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    /// The reader is colour-blind: in every palette offered, light and
    /// dark, B2's sweep and B3's stay apart under a deuteranopia
    /// simulation, and so do the terminal's red and green (orange and
    /// blue, as apex keeps them). Acme alone is acme's own red and green,
    /// kept as acme has them for a reader who needs its contrast.
    #[test]
    fn every_palette_keeps_its_sweeps_and_its_red_and_green_apart() {
        for p in PALETTES {
            for dark in [false, true] {
                let t = of(p, dark);
                let sweeps = apart(t.exec_hl, t.look_hl);
                if p != Palette::Acme {
                    assert!(sweeps >= 60., "{} {}: B2 and B3 {sweeps:.0} apart", p.title(), if dark { "dark" } else { "light" });
                }
                let ansi = apart(t.ansi[1], t.ansi[2]);
                assert!(ansi >= 40., "{} {}: red and green {ansi:.0} apart", p.title(), if dark { "dark" } else { "light" });
            }
        }
    }
}
