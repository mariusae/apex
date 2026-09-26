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

/// rsms's bright scheme (github.com/rsms/sublime-theme): its paper,
/// hsl(60 30% 99%), and black ink, its accent and caret blue (blue2,
/// hsl(224 100% 50%)), its selection (blue4 at half), its reds and blues
/// for B2 and B3; the chrome's greys stepped off the paper.
pub const LIGHT: Theme = Theme {
    body_bg: 0xFDFDFB,
    body_sel: 0xD2EDFD,
    body_border: 0xC9C9C6,
    tag_bg: 0xF3F3F0,
    tag_sel: 0xD2EDFD,
    tag_border: 0xDDDDDA,
    text: 0x000000,
    text_dim: 0x666666,
    accent: 0x0044FF,
    sweep_text: 0xFFFFFF,
    border: 0xE2E2DF,
    column: 0xF6F6F3,
    dirty: 0x333333,
    stale: 0xE0A300,
    fenced: 0xCC0000,
    progress: 0x0044FF,
    exec_hl: 0x990000,
    look_hl: 0x0043A8,
    strip: 0xEDEDEA,
    tab_outline_dim: 0xDDDDDA,
    tab_outline: 0xC9C9C6,
    tab_current_text: 0x000000,
    tab_text: 0x333333,
    tab_hover: 0xE3E3E0,
    tab_dim: 0x808080,
    tab_close_hover: 0x000000,
    tab_fenced_text: 0x666666,
    panel_bg: 0xFFFFFF,
    panel_border: 0xD4D4D1,
    panel_divider: 0xEAEAE7,
    panel_text: 0x000000,
    panel_text_dim: 0x666666,
    panel_dim: 0x808080,
    panel_pick: 0xE8F4FC,
    panel_hover: 0xF1F1EE,
    panel_chosen_bg: 0x0044FF,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0x0044FF,
    panel_danger_hover: 0xF6D5D2,
    field_sel: 0xD2EDFD,
    // xterm's, as the server sent them before the theme
    ansi: [0x000000, 0xCC241D, 0x3C8A2A, 0xB08A00, 0x1C4FD6, 0x9A2D9A, 0x0F8A8A, 0xBBBBBB, 0x555555, 0xFF5555, 0x55C055, 0xD6C000, 0x5580FF, 0xDD55DD, 0x33C0C0, 0xFFFFFF],
    menu_bg: 0xF7F7F5,
    menu_hl: 0x0044FF,
    menu_border: 0xD4D4D1,
    menu_text: 0x000000,
    menu_hl_text: 0xFFFFFF,
};

/// rsms's dark scheme: its near-black paper, hsl(0 0% 7%), and white ink
/// at 80%, the secondary at 40%; its pink cursor, hsl(320 90% 70%), as
/// the accent (carets, live rings, the notified tint), its selection blue
/// at 30%; a chosen row in its blue, which white reads on where it does
/// not on the pink.
pub const DARK: Theme = Theme {
    body_bg: 0x121212,
    body_sel: 0x2B4759,
    body_border: 0x3A3A3A,
    tag_bg: 0x1A1A1A,
    tag_sel: 0x2B4759,
    tag_border: 0x2A2A2A,
    text: 0xD0D0D0,
    text_dim: 0x717171,
    accent: 0xF76EC9,
    sweep_text: 0xFFFFFF,
    border: 0x262626,
    column: 0x0E0E0E,
    dirty: 0xD0D0D0,
    stale: 0xF2C230,
    fenced: 0xFF2A00,
    progress: 0xF76EC9,
    exec_hl: 0xE61300,
    look_hl: 0x0080FF,
    strip: 0x0A0A0A,
    tab_outline_dim: 0x262626,
    tab_outline: 0x3A3A3A,
    tab_current_text: 0xFFFFFF,
    tab_text: 0xB0B0B0,
    tab_hover: 0x1E1E1E,
    tab_dim: 0x717171,
    tab_close_hover: 0xFFFFFF,
    tab_fenced_text: 0x717171,
    panel_bg: 0x1C1C1C,
    panel_border: 0x333333,
    panel_divider: 0x2A2A2A,
    panel_text: 0xD0D0D0,
    panel_text_dim: 0x9A9A9A,
    panel_dim: 0x717171,
    panel_pick: 0x2B4759,
    panel_hover: 0x262626,
    panel_chosen_bg: 0x0080FF,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0xF76EC9,
    panel_danger_hover: 0x6A3030,
    field_sel: 0x2B4759,
    // the same hues, lit for the dark paper: black a shade of it, white
    // the ink, the rest lighter and a little softer; the bright ones
    // brighter still
    ansi: [0x2A2A2C, 0xE06060, 0x8AC26A, 0xD6B85A, 0x6A9EE6, 0xC07AC0, 0x5AB8B8, 0xC8C8CC, 0x6A6A6E, 0xF08080, 0xA8D88A, 0xF0D070, 0x8AB8F0, 0xD69AD6, 0x80D0D0, 0xE8E8EA],
    menu_bg: 0x1C1C1C,
    menu_hl: 0x0080FF,
    menu_border: 0x333333,
    menu_text: 0xD0D0D0,
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
