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

    /// Number of lines; a text ending in a newline has an empty last line.
    pub fn line_count(&self) -> usize {
        self.0.len_lines()
    }

    /// `(start, end)` of line `n`, `end` excluding the newline; `None` past
    /// the last line. Mirrors acme's line addressing for rendering.
    pub fn line_range(&self, n: usize) -> Option<(usize, usize)> {
        let lines = self.0.len_lines();
        if n >= lines {
            return None;
        }
        let start = self.0.line_to_char(n);
        let end = if n + 1 < lines { self.0.line_to_char(n + 1) - 1 } else { self.len() };
        Some((start, end))
    }

    /// The runes of line `n` without its newline.
    pub fn line(&self, n: usize) -> String {
        match self.line_range(n) {
            Some((a, b)) => self.slice(a, b),
            None => String::new(),
        }
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

/// acme's `trimspaces` (plan9port 1617427, Put in autoindent mode): the
/// blanks -- spaces and tabs -- at the end of every line, and at the end
/// of the text, which autoindent is the leading cause of. Returns the
/// text without them and the runs removed, as character ranges
/// `[q0, q1)` from the end of the text backwards, so deleting them in
/// that order leaves every earlier offset where it was. Blanks before
/// anything but a newline or the end are left alone, and so is a `\r`:
/// only what really ends a line is trimmed.
pub fn trim_trailing_blanks(text: &str) -> (String, Vec<(usize, usize)>) {
    let chars: Vec<char> = text.chars().collect();
    let blank = |c: char| c == ' ' || c == '\t';
    let mut runs = Vec::new();
    // where a run of blanks being walked back over would end: the end of
    // the text to begin with, then each newline; None inside a line
    let mut end = Some(chars.len());
    let mut i = chars.len();
    loop {
        if i == 0 || !blank(chars[i - 1]) {
            if let Some(e) = end {
                if i < e {
                    runs.push((i, e));
                }
            }
            if i == 0 {
                break;
            }
            end = (chars[i - 1] == '\n').then_some(i - 1);
        }
        i -= 1;
    }
    if runs.is_empty() {
        return (text.to_string(), runs);
    }
    let mut keep = vec![true; chars.len()];
    for &(q0, q1) in &runs {
        keep[q0..q1].iter_mut().for_each(|k| *k = false);
    }
    let trimmed = chars.iter().zip(keep).filter(|(_, k)| *k).map(|(c, _)| *c).collect();
    (trimmed, runs)
}

#[cfg(test)]
mod trim_tests {
    use super::trim_trailing_blanks;

    fn trim(s: &str) -> String {
        trim_trailing_blanks(s).0
    }

    #[test]
    fn blanks_at_the_ends_of_lines_go_and_nothing_else_does() {
        assert_eq!(trim("a  \nb\t\n"), "a\nb\n");
        assert_eq!(trim("a \t \nb"), "a\nb");
        // interior blanks, and leading ones, are the line's own
        assert_eq!(trim("  a  b\n\tc\n"), "  a  b\n\tc\n");
        // a line of nothing but blanks, first, last and between
        assert_eq!(trim("   \nx\n \t\ny\n  "), "\nx\n\ny\n");
        // the end of the text is the end of a line too
        assert_eq!(trim("x   "), "x");
        assert_eq!(trim("   "), "");
        // a carriage return ends nothing: CRLF files keep their blanks
        assert_eq!(trim("a  \r\n"), "a  \r\n");
        // nothing to do
        assert_eq!(trim(""), "");
        assert_eq!(trim("a\nb\n"), "a\nb\n");
        // characters, not bytes
        assert_eq!(trim("é  \nñ\t"), "é\nñ");
    }

    #[test]
    fn the_runs_come_from_the_end_so_deleting_in_order_keeps_offsets() {
        let (out, runs) = trim_trailing_blanks("a  \nb\t\nc");
        assert_eq!(runs, vec![(5, 6), (1, 3)]);
        let mut chars: Vec<char> = "a  \nb\t\nc".chars().collect();
        for (q0, q1) in runs {
            chars.drain(q0..q1);
        }
        assert_eq!(chars.into_iter().collect::<String>(), out);
    }
}
