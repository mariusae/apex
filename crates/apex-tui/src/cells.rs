//! The cell grid: acme's geometry with one character cell for one pixel
//! row, and the wrapping that puts a buffer on it.
//!
//! apex-core's tiling is written against an abstract `Info` — a font
//! height and how many lines a tag and a body need — so the whole of
//! acme's layout works unchanged in a terminal once `font_height` is 1.
//! Nothing in `tiling` had to move for this.

use std::collections::HashMap;

use apex_core::tiling::{self, Info};
use apex_core::{Text, WindowId};

/// One cell row of a wrapped view.
#[derive(Clone, Debug, Default)]
pub struct Row {
    /// The rune offsets this row covers, `q1` exclusive.
    pub q0: usize,
    pub q1: usize,
    /// What is drawn: tabs expanded, nothing wider than the view.
    pub text: String,
    /// The display column each rune of the row starts at, `q0` first.
    /// One more entry than runes: the column just past the last.
    pub cols: Vec<usize>,
}

impl Row {
    /// The rune offset at display column `col`, clamped to the row.
    pub fn offset_at(&self, col: usize) -> usize {
        // the columns are increasing, so the last start at or before col
        let mut out = self.q0;
        for (i, c) in self.cols.iter().enumerate() {
            if *c > col {
                break;
            }
            out = self.q0 + i;
        }
        out.min(self.q1)
    }

    /// The display column a rune offset lands in.
    pub fn column_of(&self, q: usize) -> usize {
        if q <= self.q0 {
            return 0;
        }
        let i = q - self.q0;
        self.cols.get(i).copied().unwrap_or_else(|| self.cols.last().copied().unwrap_or(0))
    }
}

/// A buffer laid out on a grid `width` cells across.
#[derive(Clone, Debug, Default)]
pub struct Wrapped {
    pub rows: Vec<Row>,
}

impl Wrapped {
    /// Lay `text` out, breaking at newlines and at the view's edge, with
    /// tabs to the next multiple of `tabstop`. acme wraps by character,
    /// not by word, and so does this.
    pub fn of(text: &Text, width: i32, tabstop: usize) -> Wrapped {
        let width = width.max(1) as usize;
        let tabstop = tabstop.max(1);
        let mut rows = Vec::new();
        let n = text.len();
        let mut q = 0usize;
        let mut row = Row { q0: 0, q1: 0, text: String::new(), cols: vec![0] };
        let mut col = 0usize;
        while q < n {
            let c = text.char_at(q);
            if c == '\n' {
                row.q1 = q + 1; // the newline belongs to the line it ends
                row.cols.push(col);
                rows.push(std::mem::take(&mut row));
                q += 1;
                row = Row { q0: q, q1: q, text: String::new(), cols: vec![0] };
                col = 0;
                continue;
            }
            let w = if c == '\t' { tabstop - (col % tabstop) } else { char_width(c) };
            if col + w > width && col > 0 {
                // the rune does not fit: the row ends before it
                row.q1 = q;
                rows.push(std::mem::take(&mut row));
                row = Row { q0: q, q1: q, text: String::new(), cols: vec![0] };
                col = 0;
                continue;
            }
            if c == '\t' {
                for _ in 0..w {
                    row.text.push(' ');
                }
            } else {
                row.text.push(c);
            }
            col += w;
            q += 1;
            row.cols.push(col);
        }
        row.q1 = n;
        rows.push(row);
        Wrapped { rows }
    }

    /// The row a rune offset is in.
    pub fn row_of(&self, q: usize) -> usize {
        match self.rows.binary_search_by(|r| if q < r.q0 { std::cmp::Ordering::Greater } else if q >= r.q1 { std::cmp::Ordering::Less } else { std::cmp::Ordering::Equal }) {
            Ok(i) => i,
            Err(i) => i.min(self.rows.len().saturating_sub(1)),
        }
    }

    /// The row and column a rune offset lands on.
    pub fn at(&self, q: usize) -> (usize, usize) {
        let r = self.row_of(q);
        (r, self.rows.get(r).map(|row| row.column_of(q)).unwrap_or(0))
    }

    /// The rune offset at a row and column of the grid.
    pub fn offset(&self, row: usize, col: usize) -> usize {
        match self.rows.get(row) {
            Some(r) => r.offset_at(col),
            None => self.rows.last().map(|r| r.q1).unwrap_or(0),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// How many cells a character takes. Combining marks take none, the wide
/// ranges take two, everything else one — enough for a terminal.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if u == 0 {
        return 0;
    }
    if (0x0300..=0x036F).contains(&u) || (0x200B..=0x200F).contains(&u) || u == 0xFEFF {
        return 0;
    }
    if (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE6F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1F64F).contains(&u)
        || (0x1F900..=0x1F9FF).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u)
    {
        return 2;
    }
    1
}

/// The `Info` the tiling asks: everything is one cell high, a tag needs
/// the lines its text wraps to, a body the lines it has.
#[derive(Clone, Debug, Default)]
pub struct CellInfo {
    /// Wrapped tag lines, and whether the tag ends with a newline.
    pub tags: HashMap<WindowId, (i32, bool)>,
    /// Lines of body text from the origin on, and whether it is a
    /// terminal (which always fills its window).
    pub bodies: HashMap<WindowId, (i32, bool)>,
}

impl Info for CellInfo {
    fn font_height(&self) -> i32 {
        1
    }
    fn taglines(&self, w: WindowId, _width: i32, maxlines: i32) -> i32 {
        let (n, nl) = self.tags.get(&w).copied().unwrap_or((1, false));
        tiling::taglines_rule(n, nl, maxlines)
    }
    fn body_font_height(&self, _w: WindowId) -> i32 {
        1
    }
    fn body_nlines(&self, w: WindowId, _width: i32, maxlines: i32) -> i32 {
        match self.bodies.get(&w) {
            Some((_, true)) => maxlines,
            Some((lines, false)) => (*lines).min(maxlines),
            None => maxlines,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_the_edge_and_keeps_offsets() {
        let t = Text::new("abcdef\ngh");
        let w = Wrapped::of(&t, 4, 4);
        assert_eq!(w.rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["abcd", "ef", "gh"]);
        // the newline stays with the line it ends
        assert_eq!(w.rows[1].q1, 7);
        assert_eq!(w.offset(0, 2), 2);
        assert_eq!(w.offset(2, 1), 8);
        assert_eq!(w.at(8), (2, 1));
    }

    #[test]
    fn tabs_run_to_the_stop() {
        let t = Text::new("a\tb");
        let w = Wrapped::of(&t, 16, 4);
        assert_eq!(w.rows[0].text, "a   b");
        assert_eq!(w.rows[0].column_of(2), 4);
        assert_eq!(w.rows[0].offset_at(2), 1); // inside the tab
    }

    #[test]
    fn an_empty_buffer_is_one_row() {
        let w = Wrapped::of(&Text::new(""), 10, 4);
        assert_eq!(w.len(), 1);
        assert_eq!(w.rows[0].text, "");
    }

    #[test]
    fn a_trailing_newline_makes_a_last_empty_row() {
        let w = Wrapped::of(&Text::new("a\n"), 10, 4);
        assert_eq!(w.rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["a", ""]);
    }
}
