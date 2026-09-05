//! The change log: a port of plan9port `src/cmd/acme/elog.c`.
//!
//! Commands record their changes here against the *original* text; the log
//! is applied at the end, last change first, so every address in a program
//! refers to the unmodified text. Adjacent changes are merged exactly as
//! acme merges them, because the merge structure affects where dot ends up.
//!
//! Changes out of sequence (a change starting before the pending one) are
//! handled as acme handles them: a warning is recorded and the changes are
//! applied anyway, with addresses clamped to the text.

use crate::{Result, Text};

/// Distance beneath which changes are merged.
pub const MINSTRING: usize = 16;
/// Maximum length of change we will merge into one (acme's RBUFSIZE).
pub const MAXSTRING: usize = (32 * 1024 + 24) / 4;
const RBUFSIZE: usize = MAXSTRING;

/// Delete `nd` runes at `q0`, then insert `text` there. Coordinates refer to
/// the text as it was before any change of the same program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub q0: usize,
    pub nd: usize,
    pub text: Vec<char>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Null,
    Insert,
    Delete,
    Replace,
}

pub struct Elog {
    kind: Kind,
    q0: usize,
    nd: usize,
    r: Vec<char>,
    log: Vec<Change>,
    any: bool,
    pub warnings: Vec<String>,
}

impl Default for Elog {
    fn default() -> Self {
        Self::new()
    }
}

impl Elog {
    pub fn new() -> Elog {
        Elog { kind: Kind::Null, q0: 0, nd: 0, r: Vec::new(), log: Vec::new(), any: false, warnings: Vec::new() }
    }

    /// True once any change has been recorded.
    pub fn is_modified(&self) -> bool {
        self.any
    }

    /// acme's check: warn (once) and flush the pending change when a new
    /// one starts before `limit`.
    fn sequence(&mut self, q0: usize, limit: usize) {
        self.any = true;
        if self.kind != Kind::Null && q0 < limit {
            if self.warnings.is_empty() {
                self.warnings.push("warning: changes out of sequence".into());
            }
            self.flush();
        }
    }

    fn flush(&mut self) {
        match self.kind {
            Kind::Null => {}
            Kind::Insert | Kind::Replace => {
                self.log.push(Change { q0: self.q0, nd: self.nd, text: std::mem::take(&mut self.r) });
            }
            Kind::Delete => {
                self.log.push(Change { q0: self.q0, nd: self.nd, text: Vec::new() });
            }
        }
        self.kind = Kind::Null;
        self.nd = 0;
        self.r.clear();
    }

    pub fn replace(&mut self, t: &dyn Text, q0: usize, q1: usize, r: &[char]) -> Result<()> {
        if q0 == q1 && r.is_empty() {
            return Ok(());
        }
        self.sequence(q0, self.q0);
        // try to merge with the previous change
        if self.kind == Kind::Replace && q0 >= self.q0 + self.nd {
            let gap = q0 - (self.q0 + self.nd); // gap between previous and this
            if self.r.len() + gap + r.len() < MAXSTRING && gap < MINSTRING {
                if gap > 0 {
                    let from = self.q0 + self.nd;
                    self.r.extend(t.read(from, from + gap));
                }
                self.nd += gap + (q1 - q0);
                self.r.extend_from_slice(r);
                return Ok(());
            }
        }
        self.flush();
        self.kind = Kind::Replace;
        self.q0 = q0;
        self.nd = q1 - q0;
        self.r = r.to_vec();
        Ok(())
    }

    pub fn insert(&mut self, q0: usize, r: &[char]) -> Result<()> {
        if r.is_empty() {
            return Ok(());
        }
        self.sequence(q0, self.q0);
        // try to merge with the previous change
        if self.kind == Kind::Insert && q0 == self.q0 && self.r.len() + r.len() < MAXSTRING {
            self.r.extend_from_slice(r);
            return Ok(());
        }
        for chunk in r.chunks(RBUFSIZE) {
            self.flush();
            self.kind = Kind::Insert;
            self.q0 = q0;
            self.r = chunk.to_vec();
        }
        Ok(())
    }

    pub fn delete(&mut self, q0: usize, q1: usize) -> Result<()> {
        if q0 == q1 {
            return Ok(());
        }
        self.sequence(q0, self.q0 + self.nd);
        // try to merge with the previous change
        if self.kind == Kind::Delete && self.q0 + self.nd == q0 {
            self.nd += q1 - q0;
            return Ok(());
        }
        self.flush();
        self.kind = Kind::Delete;
        self.q0 = q0;
        self.nd = q1 - q0;
        Ok(())
    }

    /// The changes in application order: last recorded first, so that each
    /// change's coordinates are still valid when it is applied.
    pub fn finish(mut self) -> (Vec<Change>, Vec<String>) {
        self.flush();
        self.log.reverse();
        (self.log, self.warnings)
    }
}

/// Where dot ends up after `changes` (in application order) are applied,
/// following acme's `textinsert`/`textdelete` rules and the convention
/// that an insertion at an empty dot selects the inserted text.
pub fn adjust_dot(mut dot: (usize, usize), changes: &[Change], len: usize) -> (usize, usize) {
    let mut nc = len;
    for c in changes {
        let tq0 = c.q0.min(nc);
        let tq1 = (c.q0 + c.nd).min(nc);
        if tq1 > tq0 {
            let n = tq1 - tq0;
            if tq0 < dot.0 {
                dot.0 -= n.min(dot.0 - tq0);
            }
            if tq0 < dot.1 {
                dot.1 -= n.min(dot.1 - tq0);
            }
            nc -= n;
        }
        let nr = c.text.len();
        if nr > 0 {
            if tq0 < dot.1 {
                dot.1 += nr;
            }
            if tq0 < dot.0 {
                dot.0 += nr;
            }
            nc += nr;
        }
        if dot.0 == c.q0 && dot.1 == c.q0 {
            dot.1 += nr;
        }
    }
    if dot.0 > nc || dot.1 > nc || dot.0 > dot.1 {
        dot.1 = dot.1.min(nc);
        dot.0 = dot.0.min(dot.1);
    }
    dot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn merges_like_acme() {
        let t = chars("abcde");
        let mut e = Elog::new();
        e.replace(&t, 0, 1, &chars("X")).unwrap();
        e.replace(&t, 4, 5, &chars("X")).unwrap();
        let (log, _) = e.finish();
        assert_eq!(log, vec![Change { q0: 0, nd: 5, text: chars("XbcdX") }]);
    }

    #[test]
    fn inserts_at_same_point_catenate() {
        let mut e = Elog::new();
        e.insert(3, &chars("a")).unwrap();
        e.insert(3, &chars("b")).unwrap();
        assert_eq!(e.finish().0, vec![Change { q0: 3, nd: 0, text: chars("ab") }]);
    }

    #[test]
    fn out_of_sequence_warns_like_acme() {
        let mut e = Elog::new();
        e.delete(5, 6).unwrap();
        e.delete(2, 3).unwrap();
        let (log, warnings) = e.finish();
        assert_eq!(warnings, vec!["warning: changes out of sequence".to_string()]);
        assert_eq!(log.len(), 2);
        let mut e = Elog::new();
        e.insert(5, &chars("x")).unwrap();
        e.insert(5, &chars("y")).unwrap();
        e.insert(4, &chars("y")).unwrap();
        assert_eq!(e.finish().1.len(), 1);
    }

    #[test]
    fn application_order_is_reversed() {
        let t = chars("a b c");
        let mut e = Elog::new();
        e.delete(0, 1).unwrap();
        e.replace(&t, 4, 5, &chars("Z")).unwrap();
        let (log, _) = e.finish();
        assert_eq!(log[0].q0, 4);
        assert_eq!(log[1].q0, 0);
        let mut text = t.clone();
        crate::apply(&mut text, &log);
        assert_eq!(text.iter().collect::<String>(), " b Z");
    }

    #[test]
    fn dot_selects_inserted_text() {
        let changes = vec![Change { q0: 2, nd: 0, text: chars("xyz") }];
        assert_eq!(adjust_dot((2, 2), &changes, 5), (2, 5));
        let changes = vec![Change { q0: 0, nd: 2, text: chars("Q") }];
        assert_eq!(adjust_dot((3, 4), &changes, 5), (2, 3));
    }
}
