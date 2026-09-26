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
    /// What the terminal's cursor tints its cell towards.
    pub cursor_tint_to: u32,
    // the title bar
    pub strip: u32,
    /// The line round a tab not in front -- which is the strip's own
    /// colour, so the line is the only thing that says where it is --
    /// and the line it takes when the pointer is on it.
    pub tab_outline_dim: u32,
    pub tab_outline: u32,
    pub tab_current_text: u32,
    pub tab_text: u32,
    pub tab_hover: u32,
    pub tab_dim: u32,
    pub tab_close_hover: u32,
    pub tab_fenced_text: u32,
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

/// A Mac app's light appearance: the system's label greys, a warm
/// near-white paper, the selection and accent blues.
pub const LIGHT: Theme = Theme {
    body_bg: 0xFCFCFB,
    body_sel: 0xB9D7FB,
    body_border: 0xC5C5C3,
    tag_bg: 0xF2F2F0,
    tag_sel: 0xC9DDF6,
    tag_border: 0xD6D6D3,
    text: 0x1D1D1F,
    text_dim: 0x6E6E73,
    accent: 0x2F6FEB,
    sweep_text: 0xFFFFFF,
    border: 0xDCDCD9,
    column: 0xF7F7F5,
    dirty: 0x3A3A3C,
    stale: 0xE0A300,
    fenced: 0xD93025,
    progress: 0x2F6FEB,
    exec_hl: 0xC8620A,
    look_hl: 0x2F6FEB,
    cursor_tint_to: 0x000000,
    strip: 0xEAEAE8,
    tab_outline_dim: 0xDCDCD9,
    tab_outline: 0xC8C8C5,
    tab_current_text: 0x1D1D1F,
    tab_text: 0x3A3A3C,
    tab_hover: 0xE0E0DE,
    tab_dim: 0x8E8E93,
    tab_close_hover: 0x1D1D1F,
    tab_fenced_text: 0x6E6E73,
    panel_bg: 0xFFFFFF,
    panel_border: 0xD2D2D0,
    panel_divider: 0xE8E8E6,
    panel_text: 0x1D1D1F,
    panel_text_dim: 0x6E6E73,
    panel_dim: 0x8E8E93,
    panel_pick: 0xDCE8F9,
    panel_hover: 0xF0F0EE,
    panel_chosen_bg: 0x2F6FEB,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0x2F6FEB,
    panel_danger_hover: 0xF6D5D2,
    field_sel: 0xB9D7FB,
    // xterm's, as the server sent them before the theme
    ansi: [0x000000, 0xCC241D, 0x3C8A2A, 0xB08A00, 0x1C4FD6, 0x9A2D9A, 0x0F8A8A, 0xBBBBBB, 0x555555, 0xFF5555, 0x55C055, 0xD6C000, 0x5580FF, 0xDD55DD, 0x33C0C0, 0xFFFFFF],
    menu_bg: 0xF6F6F5,
    menu_hl: 0x2F6FEB,
    menu_border: 0xD2D2D0,
    menu_text: 0x1D1D1F,
    menu_hl_text: 0xFFFFFF,
};

/// The same in the dark appearance: the system's dark greys, the
/// selection and accent blues lit for them.
pub const DARK: Theme = Theme {
    body_bg: 0x1E1E1F,
    body_sel: 0x2F4E73,
    body_border: 0x4A4A4C,
    tag_bg: 0x28282A,
    tag_sel: 0x33496A,
    tag_border: 0x3A3A3C,
    text: 0xE8E8EA,
    text_dim: 0x98989D,
    accent: 0x5B9BFF,
    sweep_text: 0xFFFFFF,
    border: 0x333335,
    column: 0x19191A,
    dirty: 0xD8D8DA,
    stale: 0xF2C230,
    fenced: 0xFF5A4F,
    progress: 0x5B9BFF,
    exec_hl: 0xC8620A,
    look_hl: 0x2F6FEB,
    cursor_tint_to: 0xFFFFFF,
    strip: 0x202022,
    tab_outline_dim: 0x333335,
    tab_outline: 0x48484A,
    tab_current_text: 0xE8E8EA,
    tab_text: 0xC8C8CC,
    tab_hover: 0x2E2E30,
    tab_dim: 0x8E8E93,
    tab_close_hover: 0xFFFFFF,
    tab_fenced_text: 0x98989D,
    panel_bg: 0x2A2A2C,
    panel_border: 0x444446,
    panel_divider: 0x38383A,
    panel_text: 0xE8E8EA,
    panel_text_dim: 0xB0B0B4,
    panel_dim: 0x8E8E93,
    panel_pick: 0x2F4E73,
    panel_hover: 0x333335,
    panel_chosen_bg: 0x2F6FEB,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0x5B9BFF,
    panel_danger_hover: 0x6A3030,
    field_sel: 0x2F4E73,
    // the same hues, lit for the dark paper: black a shade of it, white
    // the ink, the rest lighter and a little softer; the bright ones
    // brighter still
    ansi: [0x2A2A2C, 0xE06060, 0x8AC26A, 0xD6B85A, 0x6A9EE6, 0xC07AC0, 0x5AB8B8, 0xC8C8CC, 0x6A6A6E, 0xF08080, 0xA8D88A, 0xF0D070, 0x8AB8F0, 0xD69AD6, 0x80D0D0, 0xE8E8EA],
    menu_bg: 0x2C2C2E,
    menu_hl: 0x2F6FEB,
    menu_border: 0x48484A,
    menu_text: 0xE8E8EA,
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

/// View ▸ Always Show Tabs in Full Screen (on by default, as a
/// browser's): the strip stays as part of the layout in full screen;
/// off, it hides and comes when the pointer is at the top, with the
/// menu bar. Kept in the `fullscreen-tabs` state file.
static FULLSCREEN_TABS: AtomicBool = AtomicBool::new(true);

pub fn fullscreen_tabs() -> bool {
    FULLSCREEN_TABS.load(Ordering::Relaxed)
}

pub fn set_fullscreen_tabs(on: bool) {
    FULLSCREEN_TABS.store(on, Ordering::Relaxed);
    let p = crate::shell::state_file().with_file_name("fullscreen-tabs");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, if on { "always\n" } else { "hover\n" });
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
    let tabs = std::fs::read_to_string(crate::shell::state_file().with_file_name("fullscreen-tabs")).map(|s| s.trim() != "hover").unwrap_or(true);
    FULLSCREEN_TABS.store(tabs, Ordering::Relaxed);
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
