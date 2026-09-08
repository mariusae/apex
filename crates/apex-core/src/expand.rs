//! acme's B3 expansion (look.c: `expand`, `expandfile`), one for one:
//! from the point clicked, a file name with an address after its colon
//! when such a file exists, else the word; a sweep or selection is
//! taken as it is, still read for a name and address.

use crate::node::acme_isalnum;
use crate::text::Text;

/// acme's `isfilec`: alnum and `.-+/:@`.
pub fn isfilec(c: char) -> bool {
    acme_isalnum(c) || ".-+/:@".contains(c)
}

/// acme's `isaddrc` (addr.c).
pub fn isaddrc(c: char) -> bool {
    "0123456789+-/$.#,;?".contains(c)
}

/// acme's `isregexc` (addr.c).
pub fn isregexc(c: char) -> bool {
    acme_isalnum(c) || "^+-.*?#,;[]()$".contains(c)
}

/// What B3 took: the text `[q0, q1)`, and, when it names a file, the
/// name as written (empty for the window's own file, `:12` alone) with
/// the address text after its colon (empty when none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub q0: usize,
    pub q1: usize,
    pub file: Option<(String, String)>,
}

/// `expand`: `q0 == q1` is a click (the text around it is read); a
/// range is a sweep or selection. `is_file(name)` says whether the name,
/// as written, is a file here (a window's name, or one on disk relative
/// to the window's directory). `None` when nothing is there.
pub fn expand(t: &Text, q0: usize, q1: usize, is_file: &dyn Fn(&str) -> bool) -> Option<Expansion> {
    if let Some(e) = expandfile(t, q0, q1, is_file) {
        return Some(e);
    }
    let (mut q0, mut q1) = (q0, q1);
    if q0 == q1 {
        while q1 < t.len() && acme_isalnum(t.char_at(q1)) {
            q1 += 1;
        }
        while q0 > 0 && acme_isalnum(t.char_at(q0 - 1)) {
            q0 -= 1;
        }
    }
    (q1 > q0).then_some(Expansion { q0, q1, file: None })
}

fn has_at(t: &Text, at: isize, s: &str) -> bool {
    if at < 0 {
        return false;
    }
    let at = at as usize;
    s.chars().enumerate().all(|(i, c)| at + i < t.len() && t.char_at(at + i) == c)
}

/// `expandfile`: a URL is taken whole; a file name (with its address
/// text after the colon) when the name is a file; else nothing.
fn expandfile(t: &Text, q0: usize, q1: usize, is_file: &dyn Fn(&str) -> bool) -> Option<Expansion> {
    let nc = t.len();
    let (mut q0, mut q1) = (q0.min(nc), q1.min(nc));
    let mut amax = q1;
    if q1 == q0 {
        let mut colon: Option<usize> = None;
        while q1 < nc && isfilec(t.char_at(q1)) {
            if t.char_at(q1) == ':' && !has_at(t, q1 as isize - 4, "http://") && !has_at(t, q1 as isize - 5, "https://") {
                colon = Some(q1);
                break;
            }
            q1 += 1;
        }
        while q0 > 0 && {
            let c = t.char_at(q0 - 1);
            isfilec(c) || isaddrc(c) || isregexc(c)
        } {
            q0 -= 1;
            if colon.is_none() && t.char_at(q0) == ':' && !has_at(t, q0 as isize - 4, "http://") && !has_at(t, q0 as isize - 5, "https://") {
                colon = Some(q0);
            }
        }
        // if it looks like it might begin file: , consume address chars
        // after : ; otherwise terminate expansion at :
        if let Some(c) = colon {
            q1 = c;
            if c + 1 < nc && isaddrc(t.char_at(c + 1)) {
                q1 = c + 1;
                while q1 < nc && isaddrc(t.char_at(q1)) {
                    q1 += 1;
                }
            }
        }
        if q1 > q0 {
            match colon {
                Some(c) => {
                    // stop at white space
                    amax = c + 1;
                    while amax < nc && !matches!(t.char_at(amax), ' ' | '\t' | '\n') {
                        amax += 1;
                    }
                }
                None => amax = nc,
            }
        }
    }
    let mut amin = amax;
    let n = q1 - q0;
    if n == 0 {
        return None;
    }
    let r: Vec<char> = t.slice(q0, q1).chars().collect();
    let s: String = r.iter().collect();
    // is it a URL? http:// and https:// are taken whole
    if s.starts_with("http://") || s.starts_with("https://") {
        // avoid capturing end-of-sentence punctuation
        let q1 = if r[n - 1] == '.' { q1 - 1 } else { q1 };
        return Some(Expansion { q0, q1, file: None });
    }
    // first, does it have bad chars?
    let mut nname: Option<usize> = None;
    for (i, &c) in r.iter().enumerate() {
        if c == ':' && nname.is_none() {
            if q0 + i + 1 < nc && (i == n - 1 || isaddrc(t.char_at(q0 + i + 1))) {
                amin = q0 + i;
            } else {
                return None;
            }
            nname = Some(i);
        }
    }
    let nname = nname.unwrap_or(n);
    if r[..nname].iter().any(|&c| !isfilec(c) && c != ' ') {
        return None;
    }
    let name: String = r[..nname].iter().collect();
    // (acme turns <name> into an include file's name here; not done)
    if amin != q0 && !is_file(&name) {
        return None;
    }
    // Isfile: the address is what follows the colon, up to white space
    let addr = if amin < amax { t.slice(amin + 1, amax) } else { String::new() };
    Some(Expansion { q0, q1, file: Some((name, addr)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files<'a>(names: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |n| names.contains(&n)
    }

    #[test]
    fn a_click_anywhere_in_a_name_with_an_address_takes_it_all() {
        let t = Text::new("see plan9port/CHANGES:123:1 there\n");
        let f = files(&["plan9port/CHANGES"]);
        // on the directory, on the file, on the number: the same file
        // and address (the text runs to the first colon right of the
        // pointer, then over address characters, as acme's does)
        for (q, text) in [(5, "plan9port/CHANGES:123"), (15, "plan9port/CHANGES:123"), (23, "plan9port/CHANGES:123:1")] {
            let e = expand(&t, q, q, &f).unwrap();
            assert_eq!(t.slice(e.q0, e.q1), text, "at {q}");
            assert_eq!(e.file, Some(("plan9port/CHANGES".into(), "123:1".into())));
        }
        // no such file: the word under the pointer
        let none = files(&[]);
        let e = expand(&t, 5, 5, &none).unwrap();
        assert_eq!(t.slice(e.q0, e.q1), "plan9port");
        assert_eq!(e.file, None);
        let e = expand(&t, 15, 15, &none).unwrap();
        assert_eq!(t.slice(e.q0, e.q1), "CHANGES");
    }

    #[test]
    fn addresses_urls_and_words() {
        let f = files(&["x.go", "/tmp/a b"]);
        // an address alone is the window's own file
        let t = Text::new("at :12 now\n");
        let e = expand(&t, 4, 4, &f).unwrap();
        assert_eq!((t.slice(e.q0, e.q1), e.file), (":12".into(), Some(("".into(), "12".into()))));
        // a name without an address
        let t = Text::new("x.go, then\n");
        let e = expand(&t, 1, 1, &f).unwrap();
        assert_eq!((t.slice(e.q0, e.q1), e.file), ("x.go".into(), Some(("x.go".into(), "".into()))));
        // a URL, whole, without a final period; the scheme's colon is no
        // address (a later colon ends it, as in acme)
        let t = Text::new("go to https://example.com/a-b. now\n");
        for q in [7, 10, 27] {
            let e = expand(&t, q, q, &f).unwrap();
            assert_eq!((t.slice(e.q0, e.q1), e.file), ("https://example.com/a-b".into(), None), "at {q}");
        }
        // a selection is taken as it is, but read for a name
        let t = Text::new("x.go:3 y\n");
        let e = expand(&t, 0, 6, &f).unwrap();
        assert_eq!((e.q0, e.q1, e.file), (0, 6, Some(("x.go".into(), "3".into()))));
        // (a longer selection still names the file; the address parser
        // stops after the 3, as acme's does)
        let e = expand(&t, 0, 8, &f).unwrap();
        assert_eq!((e.q0, e.q1, e.file), (0, 8, Some(("x.go".into(), "3 y".into()))));
        // a colon followed by anything but an address character is no file
        let t = Text::new("x.go:y\n");
        assert_eq!(expand(&t, 0, 6, &f).unwrap().file, None);
        // nothing at a blank
        let t = Text::new("a  b\n");
        assert_eq!(expand(&t, 2, 2, &f), None);
    }
}
