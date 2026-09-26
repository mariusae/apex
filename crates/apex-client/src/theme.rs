//! The two themes, as a Mac app of now dresses (the modern-mac branch's
//! experiment; acme's tinted papers are on main): a near-white paper for
//! bodies and a quiet grey for the tags over them, as a window's content
//! sits under its title bar; hairlines where acme has black borders; the
//! system's selection blue; ink the system's label colours, primary and
//! secondary; one accent, a blue, for what is going on. Its dark twin
//! keeps every relation. Chosen in the View menu (Light, Dark, System);
//! the choice is kept beside the other state files.

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
    pub panel_divider: u32,
    pub panel_text: u32,
    pub panel_text_dim: u32,
    pub panel_dim: u32,
    pub panel_pick: u32,
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
    panel_divider: 0xD8DEE4,
    panel_text: 0x24292F,
    panel_text_dim: 0x57606A,
    panel_dim: 0x6E7781,
    panel_pick: 0xDDF4FF,
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
    panel_divider: 0x373E47,
    panel_text: 0xADBAC7,
    panel_text_dim: 0x909DAB,
    panel_dim: 0x768390,
    panel_pick: 0x2C3E56,
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
};

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
    if is_dark() { &DARK } else { &LIGHT }
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
