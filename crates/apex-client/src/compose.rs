//! Plan 9's compose sequences: a tap of Alt alone, then a few keys
//! that name a character (`"o` ö, `<=` ≤, `*a` α), or `X` and four hex
//! digits (`X2265` ≥; `XX` five, `XXX` six). The table is plan9port's
//! `lib/keyboard` (`assets/keyboard/`, MIT licence), read as its
//! `mklatinkbd` reads it; the matching is its devdraw's `latin1`.

use std::sync::OnceLock;

/// What the keys typed since the tap make so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The start of some sequence: another key is wanted.
    More,
    /// A whole sequence: its character.
    Done(char),
    /// Nothing starts so: plan9port types the keys as they were.
    Fail,
}

/// The keys `seq` typed since the tap, as a sequence.
pub fn step(seq: &str) -> Step {
    if seq.starts_with('X') {
        return hex(seq);
    }
    let t = table();
    let i = t.partition_point(|(s, _)| s.as_str() < seq);
    let mut exact = None;
    for (s, c) in &t[i..] {
        if !s.starts_with(seq) {
            break;
        }
        if s.len() > seq.len() {
            // a longer one goes on from here: wait for it, even past a
            // shorter one that is whole (as latin1 does)
            return Step::More;
        }
        exact = Some(*c);
    }
    match exact {
        Some(c) => Step::Done(c),
        None => Step::Fail,
    }
}

/// `X` and four hex digits, `XX` and five, `XXX` and six: a code point.
fn hex(seq: &str) -> Step {
    let (digits, n) = if let Some(d) = seq.strip_prefix("XXX") {
        (d, 6)
    } else if let Some(d) = seq.strip_prefix("XX") {
        (d, 5)
    } else {
        (&seq[1..], 4)
    };
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Step::Fail;
    }
    if digits.len() < n {
        return Step::More;
    }
    match u32::from_str_radix(digits, 16).ok().filter(|&c| c != 0).and_then(char::from_u32) {
        Some(c) => Step::Done(c),
        None => Step::Fail,
    }
}

/// The sequences and their characters, sorted by sequence.
fn table() -> &'static [(String, char)] {
    static TABLE: OnceLock<Vec<(String, char)>> = OnceLock::new();
    TABLE.get_or_init(|| parse(include_str!("../assets/keyboard/keyboard")))
}

/// `lib/keyboard` as mklatinkbd reads it: a line is the code point in
/// hex, then from column 6 the sequences (two or more ASCII characters,
/// space separated), up to the first character that isn't ASCII. A
/// sequence given twice keeps its first meaning.
fn parse(text: &str) -> Vec<(String, char)> {
    let mut out: Vec<(String, char)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.len() < 6 {
            continue;
        }
        let Some(c) = line.split(' ').next().and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32) else {
            continue;
        };
        let rest = &line[6..];
        let ascii = &rest[..rest.find(|c: char| !c.is_ascii()).unwrap_or(rest.len())];
        // a sequence running into the character is not one (mklatinkbd
        // takes one only at a space or the line's end)
        let mut words: Vec<&str> = ascii.split(|c: char| c.is_ascii_whitespace()).collect();
        if ascii.len() < rest.len() {
            words.pop();
        }
        for w in words {
            if w.len() >= 2 && !out.iter().any(|(s, _)| s == w) {
                out.push((w.to_string(), c));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_from_the_table() {
        assert_eq!(step("\"o"), Step::Done('ö'));
        assert_eq!(step("<="), Step::Done('≤'));
        assert_eq!(step("->"), Step::Done('→'));
        assert_eq!(step("*a"), Step::Done('α'));
        assert_eq!(step("Or"), Step::Done('®'));
        assert_eq!(step("rO"), Step::Done('®'));
        assert_eq!(step(":,"), Step::Done('😂'));
    }

    #[test]
    fn a_prefix_waits_and_a_stranger_fails() {
        assert_eq!(step("\""), Step::More);
        assert_eq!(step("<"), Step::More);
        assert_eq!(step("\"\u{1}"), Step::Fail);
        assert_eq!(step("\u{7f}"), Step::Fail);
    }

    #[test]
    fn hex_code_points() {
        assert_eq!(step("X"), Step::More);
        assert_eq!(step("X22"), Step::More);
        assert_eq!(step("X2265"), Step::Done('≥'));
        assert_eq!(step("X00e9"), Step::Done('é'));
        assert_eq!(step("X22g"), Step::Fail);
        assert_eq!(step("XX"), Step::More);
        assert_eq!(step("XX1F602"), Step::Done('😂'));
        assert_eq!(step("XXX"), Step::More);
        assert_eq!(step("XXX01F602"), Step::Done('😂'));
        assert_eq!(step("XXX110000"), Step::Fail);
        assert_eq!(step("XD800"), Step::Fail);
        assert_eq!(step("X0000"), Step::Fail);
    }

    #[test]
    fn the_table_is_read_whole() {
        let t = table();
        assert!(t.len() > 500, "{}", t.len());
        // every sequence is two or more printable ASCII characters
        assert!(t.iter().all(|(s, _)| s.len() >= 2 && s.bytes().all(|b| b.is_ascii_graphic())));
        // the first meaning of a sequence given twice is kept
        assert!(t.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn parse_reads_as_mklatinkbd() {
        let t = parse("# comment\n00AE  Or rO       ®\tregistered\n2264  <=          ≤\n2265  <=          ≥\n1F602 :,          😂 face\n");
        let want: Vec<(String, char)> = vec![(":,".into(), '😂'), ("<=".into(), '≤'), ("Or".into(), '®'), ("rO".into(), '®')];
        assert_eq!(t, want);
    }
}
