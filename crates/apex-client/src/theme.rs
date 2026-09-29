//! The themes, as a Mac app of now dresses (the modern-mac branch's
//! experiment; acme's tinted papers are on main): four palettes to live
//! with and choose among (View ▸ Theme: Alabaster, Xcode, Classic, GitHub, Nova),
//! each a light and a dark, and the appearance choosing between those
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
    /// The text over a B2/B3 sweep.
    pub sweep_text: u32,
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
    // the tools menu (menuhit's: greenish, negative selection)
    pub menu_bg: u32,
    pub menu_hl: u32,
    pub menu_border: u32,
    pub menu_text: u32,
    pub menu_hl_text: u32,
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
    sweep_text: 0xFFFFFF,
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
    menu_bg: 0xFFFFFF,
    menu_hl: 0x0969DA,
    menu_border: 0xD0D7DE,
    menu_text: 0x24292F,
    menu_hl_text: 0xFFFFFF,
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
    sweep_text: 0xFFFFFF,
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
    menu_bg: 0x2D333B,
    menu_hl: 0x316DCA,
    menu_border: 0x444C56,
    menu_text: 0xADBAC7,
    menu_hl_text: 0xFFFFFF,
    diff_add: 0x243145,
    diff_del: 0x372E2C,
};

/// The colours a palette chooses; the rest of a theme follows from them
/// (`make`), as GitHub's are laid out: popovers (sheets, menus) on
/// `popover`, a chosen row in `chosen` with white on it, B2 and B3's
/// sweeps in white on `exec` and `look`.
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
        sweep_text: 0xFFFFFF,
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
        menu_bg: k.popover,
        menu_hl: k.chosen,
        menu_border: k.line,
        menu_text: k.ink,
        menu_hl_text: 0xFFFFFF,
        diff_add: k.diff_add,
        diff_del: k.diff_del,
    }
}

/// The terminal's sixteen on light and on dark paper for every palette
/// but GitHub's own: GitHub's colour-blind ones (orange for red, blue for
/// green), since the reader is colour-blind whatever the paper.
const ANSI_LIGHT: [u32; 16] = [0x24292F, 0xB35900, 0x0550AE, 0x4D2D00, 0x0969DA, 0x8250DF, 0x1B7C83, 0x6E7781, 0x57606A, 0x8A4600, 0x0969DA, 0x633C01, 0x218BFF, 0xA475F9, 0x3192AA, 0x8C959F];
const ANSI_DARK: [u32; 16] = [0x545D68, 0xF69D50, 0x539BF5, 0xC69026, 0x539BF5, 0xB083F0, 0x39C5CF, 0x909DAB, 0x636E7B, 0xFFBC6F, 0x6CB6FF, 0xDAAA3F, 0x6CB6FF, 0xDCBDFB, 0x56D4DD, 0xCDD9E5];

/// tonsky's Alabaster: #f7f7f7 paper and black ink, #bfdbfe selection,
/// its active blue (#007acc) as the accent and caret, its grey (#777)
/// for what is secondary, its red for B2 and blue for B3.
pub const ALABASTER_LIGHT: Theme = make(Keys {
    paper: 0xF7F7F7, sel: 0xBFDBFE, thumb: 0xCFCFCF, header: 0xEEEEEE, header_sel: 0xBAD4F5, line: 0xDDDDDD,
    ink: 0x000000, dim: 0x777777, faint: 0x999999, accent: 0x007ACC, chosen: 0x007ACC, column: 0xF0F0F0, sidebar: 0xE9E9E9,
    dirty: 0x333333, stale: 0xE09A30, fenced: 0xAA3731, exec: 0xAA3731, look: 0x325CC0,
    popover: 0xFFFFFF, hover: 0xEFEFEF, danger_hover: 0xF8E1E0, ansi: ANSI_LIGHT, diff_add: 0xE3ECFB, diff_del: 0xFFEFD6,
});

/// Alabaster Dark: #0e1415 paper, #cecece ink, #293334 selection, its
/// amber (#cd974b) as the accent and caret, its punctuation's grey-green
/// (#708b8d) for what is secondary.
pub const ALABASTER_DARK: Theme = make(Keys {
    paper: 0x0E1415, sel: 0x293334, thumb: 0x3A4A4C, header: 0x162022, header_sel: 0x2E3C3E, line: 0x243234,
    ink: 0xCECECE, dim: 0x708B8D, faint: 0x5A6E70, accent: 0xCD974B, chosen: 0x3A6EA5, column: 0x0A0F10, sidebar: 0x121A1B,
    dirty: 0xCECECE, stale: 0xDFDF8E, fenced: 0xCC3333, exec: 0xB03030, look: 0x3A6EA5,
    popover: 0x162022, hover: 0x1E2A2C, danger_hover: 0x2B1D1E, ansi: ANSI_DARK, diff_add: 0x16263A, diff_del: 0x33271A,
});

/// Xcode's Default (Light): white paper, the label greys, Xcode's
/// selection (#a4cdff), the system blue (#007aff).
pub const XCODE_LIGHT: Theme = make(Keys {
    paper: 0xFFFFFF, sel: 0xA4CDFF, thumb: 0xC8C8C8, header: 0xF5F5F5, header_sel: 0xB3D4FC, line: 0xDCDCDC,
    ink: 0x1D1D1F, dim: 0x6E6E73, faint: 0x8E8E93, accent: 0x007AFF, chosen: 0x007AFF, column: 0xFAFAFA, sidebar: 0xEBEBEB,
    dirty: 0x3A3A3C, stale: 0xE6A100, fenced: 0xFF3B30, exec: 0xD9480F, look: 0x007AFF,
    popover: 0xFFFFFF, hover: 0xF0F0F0, danger_hover: 0xFFE5E3, ansi: ANSI_LIGHT, diff_add: 0xE6F0FF, diff_del: 0xFFF1E0,
});

/// Xcode's Default (Dark): #1f1f24 paper, its selection (#515b70), the
/// system blue on dark (#0a84ff).
pub const XCODE_DARK: Theme = make(Keys {
    paper: 0x1F1F24, sel: 0x515B70, thumb: 0x4A4A50, header: 0x292A30, header_sel: 0x3F4A63, line: 0x38383D,
    ink: 0xDFDFE0, dim: 0x98989D, faint: 0x6C6C70, accent: 0x0A84FF, chosen: 0x0A84FF, column: 0x18181C, sidebar: 0x252529,
    dirty: 0xDFDFE0, stale: 0xFFD60A, fenced: 0xFF453A, exec: 0xC2410C, look: 0x0A6CD8,
    popover: 0x2C2C31, hover: 0x333338, danger_hover: 0x4A2A28, ansi: ANSI_DARK, diff_add: 0x1B2A44, diff_del: 0x3A2A1C,
});

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

/// Panic's Nova, its standard Bright: a white editor on a #ececec
/// sidebar, #262626 ink, Nova's keyword blue (#255ab1) and red (#bc391c)
/// for B3 and B2, its comment grey (#69727d) for what is secondary, and
/// the blue of its buttons (#3777ea) for the accent (as sampled from
/// Panic's own preview of its themes).
pub const NOVA_LIGHT: Theme = make(Keys {
    paper: 0xFFFFFF, sel: 0xCCE0FA, thumb: 0xC8C8C8, header: 0xF5F5F5, header_sel: 0xC4D9F7, line: 0xE0E0E0,
    ink: 0x262626, dim: 0x69727D, faint: 0x8F97A0, accent: 0x3777EA, chosen: 0x3777EA, column: 0xFAFAFA, sidebar: 0xECECEC,
    dirty: 0x3A3A3A, stale: 0xD39B00, fenced: 0xBC391C, exec: 0xBC391C, look: 0x255AB1,
    popover: 0xFFFFFF, hover: 0xF2F2F2, danger_hover: 0xF9E3DE, ansi: ANSI_LIGHT, diff_add: 0xE6EFFD, diff_del: 0xFDEEE0,
});

/// Nova's standard Dark: a #1b1c1d editor beside a lighter #323232
/// sidebar, its cool white ink (#dbe5f1), its light blue (#78b1f9) for
/// the accent and caret, the buttons' blue for a chosen row.
pub const NOVA_DARK: Theme = make(Keys {
    paper: 0x1B1C1D, sel: 0x23375A, thumb: 0x4A4C4E, header: 0x242628, header_sel: 0x2C3F5E, line: 0x323436,
    ink: 0xDBE5F1, dim: 0x8A96A6, faint: 0x5E6670, accent: 0x78B1F9, chosen: 0x3777EA, column: 0x161718, sidebar: 0x323232,
    dirty: 0xDBE5F1, stale: 0xE5B94B, fenced: 0xF09084, exec: 0xB8462E, look: 0x3777EA,
    popover: 0x2A2B2D, hover: 0x2E3032, danger_hover: 0x4A2A24, ansi: ANSI_DARK, diff_add: 0x1A2A45, diff_del: 0x3A2A20,
});

/// View ▸ Theme: which palette, each with its light and dark (which of
/// those is the appearance's to say: Light, Dark, System).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Palette {
    Alabaster,
    Xcode,
    Classic,
    GitHub,
    Nova,
}

pub const PALETTES: [Palette; 5] = [Palette::Alabaster, Palette::Xcode, Palette::Classic, Palette::GitHub, Palette::Nova];

impl Palette {
    pub fn title(self) -> &'static str {
        match self {
            Palette::Alabaster => "Alabaster",
            Palette::Xcode => "Xcode",
            Palette::Classic => "Classic",
            Palette::GitHub => "GitHub",
            Palette::Nova => "Nova",
        }
    }
    fn word(self) -> &'static str {
        match self {
            Palette::Alabaster => "alabaster",
            Palette::Xcode => "xcode",
            Palette::Classic => "classic",
            Palette::GitHub => "github",
            Palette::Nova => "nova",
        }
    }
}

static PALETTE: AtomicU8 = AtomicU8::new(3);

pub fn palette() -> Palette {
    PALETTES[PALETTE.load(Ordering::Relaxed) as usize % PALETTES.len()]
}

/// Chosen: kept in the `palette` state file.
pub fn set_palette(p: Palette) {
    PALETTE.store(PALETTES.iter().position(|x| *x == p).unwrap_or(3) as u8, Ordering::Relaxed);
    let f = crate::shell::state_file().with_file_name("palette");
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, format!("{}\n", p.word()));
}

static MODE: AtomicU8 = AtomicU8::new(0);
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

pub fn theme() -> &'static Theme {
    match (palette(), is_dark()) {
        (Palette::Alabaster, false) => &ALABASTER_LIGHT,
        (Palette::Alabaster, true) => &ALABASTER_DARK,
        (Palette::Xcode, false) => &XCODE_LIGHT,
        (Palette::Xcode, true) => &XCODE_DARK,
        (Palette::Classic, false) => &CLASSIC_LIGHT,
        (Palette::Classic, true) => &CLASSIC_DARK,
        (Palette::GitHub, false) => &LIGHT,
        (Palette::GitHub, true) => &DARK,
        (Palette::Nova, false) => &NOVA_LIGHT,
        (Palette::Nova, true) => &NOVA_DARK,
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

/// View ▸ Show Sidebar (on by default): the sessions down the left, as
/// vertical tabs, the one shown with its windows under it. Kept in the
/// `sidebar` state file.
static SIDEBAR: AtomicBool = AtomicBool::new(true);

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

/// View ▸ Correct Terminal Contrast (on by default): a terminal's ink
/// that does not read on its paper is moved until it does
/// (`contrast.rs`). Kept in the `contrast` state file.
static CONTRAST: AtomicBool = AtomicBool::new(true);
/// View ▸ Blink Cursor: the keys' caret (and a terminal's cursor) blinks,
/// or stays on -- the key window's ring says where the keys go too.
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
    let side = std::fs::read_to_string(crate::shell::state_file().with_file_name("sidebar")).map(|s| s.trim() != "hidden").unwrap_or(true);
    SIDEBAR.store(side, Ordering::Relaxed);
    let contrast = std::fs::read_to_string(crate::shell::state_file().with_file_name("contrast")).map(|s| s.trim() != "off").unwrap_or(true);
    CONTRAST.store(contrast, Ordering::Relaxed);
    let blink = std::fs::read_to_string(crate::shell::state_file().with_file_name("blink")).map(|s| s.trim() != "off").unwrap_or(true);
    BLINK.store(blink, Ordering::Relaxed);
    let pal = std::fs::read_to_string(crate::shell::state_file().with_file_name("palette")).unwrap_or_default();
    // "system" was Xcode's palette's name before it had its own
    let pal = if pal.trim() == "system" { "xcode" } else { pal.trim() };
    if let Some(i) = PALETTES.iter().position(|p| p.word() == pal) {
        PALETTE.store(i as u8, Ordering::Relaxed);
    }
    let m = match std::fs::read_to_string(file()).map(|s| s.trim().to_string()).as_deref() {
        Ok("dark") => Mode::Dark,
        Ok("system") => Mode::System,
        _ => Mode::Light,
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
