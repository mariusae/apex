//! What the UI sends back: the pointer, the keyboard, and the few things
//! only a UI knows (its size, what its overlays chose).
//!
//! The UI reports where the pointer is in cells and which button moved;
//! acme's meaning of that — B1 selects, B2 executes, B3 looks, and the
//! chords — is decided here, where the state machine is, exactly as the
//! gpui client decides it. The UI stays a renderer and an input device.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    /// acme's B1, B2, B3; B4 and B5 are the wheel.
    B1,
    B2,
    B3,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    Down,
    Up,
    Move,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mods {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
}

/// A key that is not a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NamedKey {
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// ^W, ^U: acme's word and line erase.
    EraseWord,
    EraseLine,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum Event {
    /// The terminal's size in cells. The row is laid out to it.
    Resize { cols: i32, rows: i32 },
    /// The pointer. `x` and `y` are cells within the row.
    Mouse { x: i32, y: i32, button: Button, motion: Motion, #[serde(default)] mods: Mods, #[serde(default)] clicks: u32 },
    /// Text typed: one or more characters, already composed.
    Text { text: String },
    Key { key: NamedKey, #[serde(default)] mods: Mods },
    /// A command from a menu, the palette, or a keybinding — as if it had
    /// been B2'd in the window's tag.
    Exec { #[serde(default)] window: Option<u64>, text: String },
    /// A path the finder chose.
    Open { name: String },
    /// B3 on something the UI resolved itself: a link in a page. The
    /// plumber decides what to do with it, as it would for any B3.
    Plumb { #[serde(default)] window: Option<u64>, text: String },
    /// Ask for the finder's candidates; answered by a frame with the
    /// overlay in it.
    Finder { #[serde(default)] all: bool },
    /// Ask for the sessions.
    Switcher,
    /// Put an overlay away.
    Dismiss,
    /// The session the switcher chose.
    Switch { name: String },
    /// The system clipboard, as the UI has it: kept as the snarf buffer
    /// so a Paste pastes what was copied outside.
    Clipboard { text: String },
    /// The UI is going away.
    Quit,
}
