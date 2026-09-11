//! The two themes: acme's colours (light), and a dark one of the same
//! make. acme's philosophy is two tinted papers, a warm one for bodies
//! and a cool one for tags, with selections a deeper tint of each, mid
//! tone borders, and black ink; the dark theme keeps every relation and
//! inverts the paper: deep warm and deep cool grounds, selections a
//! deeper tint again but lit, the same mid tone borders, warm-white
//! ink. Chosen in the View menu (Light, Dark, System); the choice is
//! kept beside the other state files.

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
    /// The text over a B2/B3 sweep.
    pub sweep_text: u32,
    /// The borders between columns and windows (acme's black).
    pub border: u32,
    /// A column where no window is (acme's white).
    pub column: u32,
    // the handles
    pub dirty: u32,
    pub unsynced: u32,
    pub live: u32,
    pub stale: u32,
    pub fenced: u32,
    pub exec_hl: u32,
    pub look_hl: u32,
    /// What the terminal's cursor tints its cell towards.
    pub cursor_tint_to: u32,
    // the title bar
    pub strip: u32,
    pub tab_bg: u32,
    pub tab_open_bg: u32,
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

/// acme's, from plan9port's acme.c and draw.h (see text_element.rs).
pub const LIGHT: Theme = Theme {
    body_bg: 0xFFFFEA,
    body_sel: 0xEEEE9E,
    body_border: 0x99994C,
    tag_bg: 0xEAFFFF,
    tag_sel: 0x9EEEEE,
    tag_border: 0x8888CC,
    text: 0x000000,
    sweep_text: 0xFFFFFF,
    border: 0x000000,
    column: 0xFFFFFF,
    dirty: 0x000099,
    unsynced: 0x88CC88,
    live: 0xB24073,
    stale: 0xFFD700,
    fenced: 0xAA0000,
    exec_hl: 0xAA0000,
    look_hl: 0x006600,
    cursor_tint_to: 0x000000,
    strip: 0xECECEC,
    tab_bg: 0xEAFFFF,
    tab_open_bg: 0xD4F5F5,
    tab_current_text: 0x000099,
    tab_text: 0x555555,
    tab_hover: 0xE0E0E0,
    tab_dim: 0x9A9A9A,
    tab_close_hover: 0x000000,
    tab_fenced_text: 0x555555,
    panel_bg: 0xF4F4F4,
    panel_border: 0xC8C8C8,
    panel_divider: 0xDDDDDD,
    panel_text: 0x111111,
    panel_text_dim: 0x555555,
    panel_dim: 0x8A8A8A,
    panel_pick: 0x9EEEEE,
    panel_hover: 0xE4E4E4,
    panel_chosen_bg: 0x000099,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0x000099,
    panel_danger_hover: 0xF0C0C0,
    field_sel: 0xB4D5FE,
    // xterm's, as the server sent them before the theme
    ansi: [0x000000, 0xCC241D, 0x3C8A2A, 0xB08A00, 0x1C4FD6, 0x9A2D9A, 0x0F8A8A, 0xBBBBBB, 0x555555, 0xFF5555, 0x55C055, 0xD6C000, 0x5580FF, 0xDD55DD, 0x33C0C0, 0xFFFFFF],
    menu_bg: 0xD4FFD4,
    menu_hl: 0x448844,
    menu_border: 0x88CC88,
    menu_text: 0x000000,
    menu_hl_text: 0xD4FFD4,
};

/// The same, on dark paper: bodies a deep warm ground, tags a deep
/// cool one, selections the tints lit up, ink warm white; the mid tone
/// borders and handle colours stand as they are where they read on
/// dark, lifted where they would not (the dirty blue, the fenced red).
pub const DARK: Theme = Theme {
    body_bg: 0x1E1E14,
    body_sel: 0x5C5A2A,
    body_border: 0x8A8A45,
    tag_bg: 0x142426,
    tag_sel: 0x2A6464,
    tag_border: 0x8888CC,
    text: 0xE8E8DC,
    sweep_text: 0xFFFFFF,
    // the borders a mid warm grey: black would sink into the paper, the
    // ink would shout; halfway between reads as a rule
    border: 0x76766C,
    column: 0x101010,
    dirty: 0x6A6AF0,
    unsynced: 0x88CC88,
    live: 0xC8508A,
    stale: 0xFFD700,
    fenced: 0xFF3030,
    exec_hl: 0xB02020,
    look_hl: 0x2E8B2E,
    cursor_tint_to: 0xFFFFFF,
    strip: 0x2A2A2A,
    tab_bg: 0x142426,
    tab_open_bg: 0x1E3A3C,
    tab_current_text: 0xA0A0FF,
    tab_text: 0xB8B8B8,
    tab_hover: 0x3A3A3A,
    tab_dim: 0x8A8A8A,
    tab_close_hover: 0xFFFFFF,
    tab_fenced_text: 0xB8B8B8,
    panel_bg: 0x262626,
    panel_border: 0x484848,
    panel_divider: 0x3A3A3A,
    panel_text: 0xE6E6E6,
    panel_text_dim: 0xB0B0B0,
    panel_dim: 0x8E8E8E,
    panel_pick: 0x2A6464,
    panel_hover: 0x333333,
    panel_chosen_bg: 0x5050D0,
    panel_chosen_text: 0xFFFFFF,
    panel_accent: 0xA0A0FF,
    panel_danger_hover: 0x6A3030,
    field_sel: 0x2A4A6A,
    // the same hues, lit for the dark paper: black a shade of it, white
    // the ink, the rest lighter and a little softer; the bright ones
    // brighter still
    ansi: [0x2A2A20, 0xE06060, 0x8AC26A, 0xD6B85A, 0x6A9EE6, 0xC07AC0, 0x5AB8B8, 0xC8C8BC, 0x6A6A5E, 0xF08080, 0xA8D88A, 0xF0D070, 0x8AB8F0, 0xD69AD6, 0x80D0D0, 0xE8E8DC],
    menu_bg: 0x1C3A1C,
    menu_hl: 0x448844,
    menu_border: 0x5A9A5A,
    menu_text: 0xE0F0E0,
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

/// The choice of last time, applied.
pub fn load() {
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
