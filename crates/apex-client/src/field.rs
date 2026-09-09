//! A one-line text field for the overlays (the session picker, its
//! new-host form, the finder): a cursor, a selection, and the editing
//! keys a Mac text field answers — arrows with shift, option and
//! command, delete forward and back, the emacs control keys, select
//! all, cut, copy, paste and undo.

use gpui::{div, prelude::*, px, rgb, Div};
use std::ops::Deref;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineEdit {
    text: String,
    /// The cursor, in characters.
    pub cursor: usize,
    /// The other end of the selection, when there is one.
    pub anchor: Option<usize>,
    undo: Vec<(String, usize)>,
}

/// What a key did to the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edited {
    /// The text changed.
    Changed,
    /// The cursor or selection moved.
    Moved,
    /// Not an editing key.
    No,
}

impl Deref for LineEdit {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl From<&str> for LineEdit {
    fn from(s: &str) -> LineEdit {
        let mut e = LineEdit::default();
        e.set(s);
        e
    }
}

impl LineEdit {
    pub fn new() -> LineEdit {
        LineEdit::default()
    }

    pub fn set(&mut self, s: &str) {
        self.text = s.to_string();
        self.cursor = self.len();
        self.anchor = None;
    }

    pub fn clear(&mut self) {
        self.set("");
        self.undo.clear();
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte(&self, i: usize) -> usize {
        self.text.char_indices().nth(i).map(|(b, _)| b).unwrap_or(self.text.len())
    }

    /// The selection as an ordered, non-empty range of characters.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        let (lo, hi) = (a.min(self.cursor), a.max(self.cursor));
        (lo < hi).then_some((lo, hi))
    }

    pub fn selected(&self) -> Option<String> {
        self.selection().map(|(a, z)| self.text[self.byte(a)..self.byte(z)].to_string())
    }

    fn remember(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
    }

    fn delete_selection(&mut self) -> bool {
        let Some((a, z)) = self.selection() else { return false };
        let (ba, bz) = (self.byte(a), self.byte(z));
        self.text.replace_range(ba..bz, "");
        self.cursor = a;
        self.anchor = None;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.remember();
        self.delete_selection();
        let b = self.byte(self.cursor);
        self.text.insert_str(b, s);
        self.cursor += s.chars().count();
        self.anchor = None;
    }

    pub fn backspace(&mut self) {
        self.remember();
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        let (a, z) = (self.byte(self.cursor - 1), self.byte(self.cursor));
        self.text.replace_range(a..z, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        self.remember();
        if self.delete_selection() || self.cursor >= self.len() {
            return;
        }
        let (a, z) = (self.byte(self.cursor), self.byte(self.cursor + 1));
        self.text.replace_range(a..z, "");
    }

    /// Erase from `from` to the cursor (^U to the start, ^W a word).
    fn erase_back_to(&mut self, from: usize) {
        self.remember();
        if self.delete_selection() {
            return;
        }
        let (a, z) = (self.byte(from), self.byte(self.cursor));
        self.text.replace_range(a..z, "");
        self.cursor = from;
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    /// The start of the word before the cursor (non-word characters
    /// skipped first), as option-left and ^W count it.
    pub fn word_left(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i > 0 && !Self::is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && Self::is_word(chars[i - 1]) {
            i -= 1;
        }
        i
    }

    pub fn word_right(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i < chars.len() && !Self::is_word(chars[i]) {
            i += 1;
        }
        while i < chars.len() && Self::is_word(chars[i]) {
            i += 1;
        }
        i
    }

    fn move_to(&mut self, p: usize, extend: bool) {
        let p = p.min(self.len());
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = p;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len();
    }

    pub fn cut(&mut self) -> Option<String> {
        let s = self.selected()?;
        self.remember();
        self.delete_selection();
        Some(s)
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some((text, cursor)) => {
                self.text = text;
                self.cursor = cursor.min(self.len());
                self.anchor = None;
                true
            }
            None => false,
        }
    }

    /// An editing key, if it is one: arrows (shift extends, option by
    /// words, command to the ends), home and end, delete back (option:
    /// a word, command: to the start) and forward, ^A ^E ^B ^F ^D ^H ^U
    /// ^W ^K, and a character typed.
    pub fn key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers) -> Edited {
        let ext = mods.shift;
        if mods.control {
            return match key {
                "a" => {
                    self.move_to(0, ext);
                    Edited::Moved
                }
                "e" => {
                    self.move_to(self.len(), ext);
                    Edited::Moved
                }
                "b" => {
                    self.move_to(self.cursor.saturating_sub(1), ext);
                    Edited::Moved
                }
                "f" => {
                    self.move_to(self.cursor + 1, ext);
                    Edited::Moved
                }
                "d" => {
                    self.delete();
                    Edited::Changed
                }
                "h" => {
                    self.backspace();
                    Edited::Changed
                }
                "u" => {
                    self.erase_back_to(0);
                    Edited::Changed
                }
                "w" => {
                    let w = self.word_left();
                    self.erase_back_to(w);
                    Edited::Changed
                }
                "k" => {
                    self.remember();
                    if !self.delete_selection() {
                        let b = self.byte(self.cursor);
                        self.text.truncate(b);
                    }
                    Edited::Changed
                }
                _ => Edited::No,
            };
        }
        match key {
            "left" => {
                let p = if mods.platform {
                    0
                } else if mods.alt {
                    self.word_left()
                } else if let (Some((a, _)), false) = (self.selection(), ext) {
                    a
                } else {
                    self.cursor.saturating_sub(1)
                };
                self.move_to(p, ext);
                Edited::Moved
            }
            "right" => {
                let p = if mods.platform {
                    self.len()
                } else if mods.alt {
                    self.word_right()
                } else if let (Some((_, z)), false) = (self.selection(), ext) {
                    z
                } else {
                    self.cursor + 1
                };
                self.move_to(p, ext);
                Edited::Moved
            }
            "home" => {
                self.move_to(0, ext);
                Edited::Moved
            }
            "end" => {
                self.move_to(self.len(), ext);
                Edited::Moved
            }
            "backspace" => {
                if mods.platform {
                    self.erase_back_to(0);
                } else if mods.alt {
                    let w = self.word_left();
                    self.erase_back_to(w);
                } else {
                    self.backspace();
                }
                Edited::Changed
            }
            "delete" => {
                self.delete();
                Edited::Changed
            }
            _ => match ch {
                Some(c) if !mods.platform && !c.is_empty() && !c.chars().any(char::is_control) => {
                    self.insert(c);
                    Edited::Changed
                }
                _ => Edited::No,
            },
        }
    }
}

/// The field drawn: its text with the selection marked and the caret at
/// the cursor when `active`, or the hint when empty.
pub fn field_view(e: &LineEdit, caret_on: bool, hint: &str, active: bool) -> Div {
    let mut row = div().flex().flex_row().items_center();
    let caret = || div().w(px(1.5)).h(px(16.)).flex_none().when(caret_on, |d| d.bg(rgb(0x000099)));
    if e.is_empty() {
        if active {
            row = row.child(caret());
        }
        return row.child(div().pl(px(4.)).text_color(rgb(0x8a8a8a)).child(hint.to_string()));
    }
    let sel = e.selection();
    let mut points = vec![0, e.cursor, e.len()];
    if let Some((a, z)) = sel {
        points.push(a);
        points.push(z);
    }
    points.sort_unstable();
    points.dedup();
    let chars: Vec<char> = e.chars().collect();
    for w in points.windows(2) {
        let (a, z) = (w[0], w[1]);
        if a == e.cursor && active {
            row = row.child(caret());
        }
        let text: String = chars[a..z].iter().collect();
        let selected = sel.is_some_and(|(s0, s1)| s0 <= a && z <= s1);
        row = row.child(div().text_color(rgb(0x111111)).when(selected, |d| d.bg(rgb(0xb4d5fe))).child(text));
    }
    if e.cursor == e.len() && active {
        row = row.child(caret());
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, platform: bool, control: bool) -> gpui::Modifiers {
        gpui::Modifiers { shift, alt, platform, control, function: false }
    }

    #[test]
    fn editing_keys_do_what_a_mac_field_does() {
        let m = mods(false, false, false, false);
        let mut e = LineEdit::new();
        for c in ["a", "b", " ", "c", "d"] {
            assert_eq!(e.key(c, Some(c), &m), Edited::Changed);
        }
        assert_eq!((&*e, e.cursor), ("ab cd", 5));
        // option-left: a word; shift-arrows select; typing replaces
        e.key("left", None, &mods(false, true, false, false));
        assert_eq!(e.cursor, 3);
        e.key("left", None, &mods(true, false, false, false));
        e.key("left", None, &mods(true, false, false, false));
        assert_eq!(e.selected().as_deref(), Some("b "));
        e.key("X", Some("X"), &m);
        assert_eq!(&*e, "aXcd");
        // command-backspace to the start; undo brings it back
        e.key("backspace", None, &mods(false, false, true, false));
        assert_eq!(&*e, "cd");
        assert!(e.undo());
        assert_eq!((&*e, e.cursor), ("aXcd", 2));
        // ^W a word, ^A ^E the ends, select all and cut
        e.key("e", None, &mods(false, false, false, true));
        e.key("w", None, &mods(false, false, false, true));
        assert_eq!(&*e, "");
        e.set("one two");
        e.select_all();
        assert_eq!(e.cut().as_deref(), Some("one two"));
        assert!(e.is_empty());
        // a command key that is not editing is not taken
        assert_eq!(e.key("k", Some("k"), &mods(false, false, true, false)), Edited::No);
    }
}
