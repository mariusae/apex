//! Rune-indexed text on a rope, with the `apex_edit::Text` view.

use ropey::Rope;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, Default)]
pub struct Text(Rope);

impl Text {
    pub fn new(s: &str) -> Text {
        Text(Rope::from_str(s))
    }

    /// Length in runes.
    pub fn len(&self) -> usize {
        self.0.len_chars()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn char_at(&self, i: usize) -> char {
        self.0.char(i)
    }

    /// Runes `q0..q1` as a string.
    pub fn slice(&self, q0: usize, q1: usize) -> String {
        let q1 = q1.min(self.len());
        let q0 = q0.min(q1);
        self.0.slice(q0..q1).to_string()
    }

    pub fn to_string(&self) -> String {
        self.0.to_string()
    }

    /// Replace runes `q0..q0+nd` with `s`; clamps to the text.
    pub fn replace(&mut self, q0: usize, nd: usize, s: &str) {
        let q0 = q0.min(self.len());
        let q1 = (q0 + nd).min(self.len());
        if q1 > q0 {
            self.0.remove(q0..q1);
        }
        if !s.is_empty() {
            self.0.insert(q0, s);
        }
    }

    pub fn line_count(&self) -> usize {
        self.0.len_lines()
    }

    /// Rune offset of the start of line `n` (0-based).
    pub fn line_start(&self, n: usize) -> usize {
        let n = n.min(self.0.len_lines().saturating_sub(1));
        self.0.line_to_char(n)
    }

    /// 0-based line containing rune `q`.
    pub fn line_of(&self, q: usize) -> usize {
        self.0.char_to_line(q.min(self.len()))
    }

    /// Feed the text to a hasher, chunk by chunk.
    pub fn hash_into(&self, h: &mut blake3::Hasher) {
        for chunk in self.0.chunks() {
            h.update(chunk.as_bytes());
        }
    }

    /// Content hash, as the file watcher computes it.
    pub fn content_hash(&self) -> String {
        let mut h = blake3::Hasher::new();
        self.hash_into(&mut h);
        h.finalize().to_hex().to_string()
    }
}

impl PartialEq for Text {
    fn eq(&self, other: &Text) -> bool {
        self.0 == other.0
    }
}

impl apex_edit::Text for Text {
    fn len(&self) -> usize {
        Text::len(self)
    }
    fn char_at(&self, i: usize) -> char {
        Text::char_at(self, i)
    }
    fn read(&self, q0: usize, q1: usize) -> Vec<char> {
        self.0.slice(q0..q1).chars().collect()
    }
}

impl Serialize for Text {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Text, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Text::new(&s))
    }
}
