//! Fuzzy matching of a query against a path, in Zed's spirit: each query
//! character must appear in order; a character scores 1.0 where it starts
//! the file name, 0.9 right after `/`, 1.0 where it continues the match
//! before it, 0.8 at a word start (after `-`, `_`, `.`, a space, a digit,
//! or a case change), 0.55 otherwise; a case mismatch costs half, and a
//! match in the file name earns 0.15 more. The best alignment is taken,
//! and the score is the mean per query character, a little less for long
//! paths.
//!
//! Fast enough for a million paths a keystroke: a query is prepared once
//! (`Query`), a path is first checked to hold the query's characters in
//! order at all -- most do not, and cost a pass over their bytes -- and
//! only then scored, by a dynamic program over buffers kept between
//! paths (`Scorer`), with no allocation per path.

/// A query, prepared once for many paths.
pub struct Query {
    chars: Vec<char>,
    lower: Vec<char>,
    /// The query in lower case, as bytes, when it is ASCII: the quick
    /// check on an ASCII path.
    ascii: Option<Vec<u8>>,
}

impl Query {
    pub fn new(q: &str) -> Query {
        let chars: Vec<char> = q.chars().collect();
        let lower = chars.iter().map(|c| lower(*c)).collect();
        let ascii = q.is_ascii().then(|| q.bytes().map(|b| b.to_ascii_lowercase()).collect());
        Query { chars, lower, ascii }
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Does `path` hold the query's characters, in order, ignoring case?
    pub fn could_match(&self, path: &str) -> bool {
        match (&self.ascii, path.is_ascii()) {
            (Some(q), true) => {
                let mut qi = 0;
                for b in path.bytes() {
                    if qi < q.len() && b.to_ascii_lowercase() == q[qi] {
                        qi += 1;
                    }
                }
                qi == q.len()
            }
            _ => {
                let mut qi = 0;
                for c in path.chars() {
                    if qi < self.lower.len() && lower(c) == self.lower[qi] {
                        qi += 1;
                    }
                }
                qi == self.lower.len()
            }
        }
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Buffers kept between paths.
#[derive(Default)]
pub struct Scorer {
    p: Vec<char>,
    prev: Vec<f64>,
    cur: Vec<f64>,
    best: Vec<f64>,
}

const NONE: f64 = f64::NEG_INFINITY;

impl Scorer {
    /// `query`'s score against `path`, or None when it does not match.
    pub fn score(&mut self, query: &Query, path: &str) -> Option<f64> {
        let m = query.chars.len();
        if m == 0 || !query.could_match(path) {
            return None;
        }
        self.p.clear();
        self.p.extend(path.chars());
        let p = &self.p;
        let n = p.len();
        if m > n {
            return None;
        }
        let name_at = path.rfind('/').map(|i| path[..i].chars().count() + 1).unwrap_or(0);
        let s = |j: usize, qc: char, cons: bool| -> f64 {
            let pc = p[j];
            let after = if j > 0 { Some(p[j - 1]) } else { None };
            let mut v = if j == name_at {
                1.0
            } else if after == Some('/') {
                0.9
            } else if cons {
                1.0
            } else if after.is_some_and(|a| matches!(a, '-' | '_' | '.' | ' ') || a.is_numeric() || (a.is_lowercase() && pc.is_uppercase())) {
                0.8
            } else {
                0.55
            };
            if pc != qc && pc.is_lowercase() != qc.is_lowercase() {
                v *= 0.5;
            }
            if j >= name_at {
                v += 0.15;
            }
            v
        };
        self.prev.clear();
        self.prev.resize(n, NONE);
        self.cur.clear();
        self.cur.resize(n, NONE);
        self.best.clear();
        self.best.resize(n, NONE);
        for i in 0..m {
            let (qc, ql) = (query.chars[i], query.lower[i]);
            for j in 0..n {
                self.cur[j] = if lower(p[j]) != ql {
                    NONE
                } else if i == 0 {
                    s(j, qc, false)
                } else if j == 0 {
                    NONE
                } else {
                    // after the previous character's match anywhere before
                    // (a gap), or right before (consecutive, never less)
                    let gap = if self.best[j - 1] > NONE { s(j, qc, false) + self.best[j - 1] } else { NONE };
                    let next = if self.prev[j - 1] > NONE { s(j, qc, true) + self.prev[j - 1] } else { NONE };
                    gap.max(next)
                };
            }
            // the best up to each position, for the next character's gaps
            let mut run = NONE;
            for j in 0..n {
                run = run.max(self.cur[j]);
                self.best[j] = run;
            }
            std::mem::swap(&mut self.prev, &mut self.cur);
        }
        let total = self.prev.iter().copied().fold(NONE, f64::max);
        if total == NONE {
            return None;
        }
        // per query character, and a little less for long paths
        Some(total / m as f64 - (n as f64) * 0.0005)
    }
}

/// `query`'s score against `path`, for a one-off.
pub fn score(query: &str, path: &str) -> Option<f64> {
    Scorer::default().score(&Query::new(query), path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The scorer as it was first written (the finder's), by recursion:
    /// the dynamic program must agree with it.
    fn reference(query: &str, path: &str) -> Option<f64> {
        let q: Vec<char> = query.chars().collect();
        let p: Vec<char> = path.chars().collect();
        if q.is_empty() || q.len() > p.len() {
            return None;
        }
        let name_at = path.rfind('/').map(|i| path[..i].chars().count() + 1).unwrap_or(0);
        type Memo = BTreeMap<(usize, usize, bool), Option<f64>>;
        fn best(q: &[char], p: &[char], qi: usize, pi: usize, prev: bool, name_at: usize, memo: &mut Memo) -> Option<f64> {
            if qi == q.len() {
                return Some(0.0);
            }
            if let Some(m) = memo.get(&(qi, pi, prev)) {
                return *m;
            }
            let mut out: Option<f64> = None;
            for j in pi..=p.len().saturating_sub(q.len() - qi) {
                let (pc, qc) = (p[j], q[qi]);
                if pc.to_lowercase().ne(qc.to_lowercase()) {
                    continue;
                }
                let mut s = if j == name_at {
                    1.0
                } else if j > 0 && p[j - 1] == '/' {
                    0.9
                } else if prev && j == pi {
                    1.0
                } else if j > 0 && (matches!(p[j - 1], '-' | '_' | '.' | ' ') || p[j - 1].is_numeric() || (p[j - 1].is_lowercase() && pc.is_uppercase())) {
                    0.8
                } else {
                    0.55
                };
                if pc != qc && !(pc.is_lowercase() == qc.is_lowercase()) {
                    s *= 0.5;
                }
                if j >= name_at {
                    s += 0.15;
                }
                if let Some(rest) = best(q, p, qi + 1, j + 1, true, name_at, memo) {
                    if out.is_none_or(|o| s + rest > o) {
                        out = Some(s + rest);
                    }
                }
            }
            memo.insert((qi, pi, prev), out);
            out
        }
        let mut memo = Memo::new();
        let total = best(&q, &p, 0, 0, false, name_at, &mut memo)?;
        Some(total / q.len() as f64 - (p.len() as f64) * 0.0005)
    }

    #[test]
    fn the_dynamic_program_scores_as_the_recursion_did() {
        let paths = ["/src/apex/crates/apex-client/src/main.rs", "/src/main-things/other/file.rs", "/x/app.rs", "/a/p/p.rs", "crates/apex-core/src/fuzzy.rs", "README.md", "Makefile", "src/FooBar/fooBar.rs", "a/b/c", "aaa/aaa.aa", "Ünïcode/fïle.txt", "", "x"];
        let queries = ["main", "app", "fz", "rdme", "FB", "fb", "aa", "a", "abc", "src", "ünï", "zzz", "MAIN", "c/m"];
        let mut scorer = Scorer::default();
        for p in paths {
            for q in queries {
                let (a, b) = (reference(q, p), scorer.score(&Query::new(q), p));
                match (a, b) {
                    (None, None) => {}
                    (Some(a), Some(b)) => assert!((a - b).abs() < 1e-9, "{q:?} in {p:?}: {a} vs {b}"),
                    _ => panic!("{q:?} in {p:?}: {a:?} vs {b:?}"),
                }
            }
        }
    }

    #[test]
    fn the_quick_check_never_turns_away_a_match() {
        let q = Query::new("Fzr");
        assert!(q.could_match("crates/apex-core/src/fuzzy.rs"));
        assert!(!q.could_match("crates/apex-core/src/zip.rs"));
        assert!(Query::new("ïle").could_match("Ünïcode/fÏLe.txt"));
    }
}
