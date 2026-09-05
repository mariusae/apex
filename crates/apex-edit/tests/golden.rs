//! Acceptance tests for the Edit language, written from sam(1) and acme's
//! semantics. Each case is (text, dot, program) → (text', dot', output) or
//! an error with acme's message.

mod common;

use apex_edit::{Intent, PipeKind};
use common::ours;

struct Case {
    text: &'static str,
    dot: (usize, usize),
    program: &'static str,
    want_text: &'static str,
    want_dot: (usize, usize),
    want_out: &'static str,
}

fn check(cases: &[Case]) {
    let mut bad = Vec::new();
    for c in cases {
        let r = ours(c.text, c.dot, c.program);
        if let Some(e) = &r.error {
            bad.push(format!("{:?} on {:?}: failed: {e}", c.program, c.text));
            continue;
        }
        if r.text != c.want_text || r.dot != c.want_dot || r.output != c.want_out {
            bad.push(format!(
                "{:?} on {:?} dot {:?}:\n   want text={:?} dot={:?} out={:?}\n   got  text={:?} dot={:?} out={:?}",
                c.program, c.text, c.dot, c.want_text, c.want_dot, c.want_out, r.text, r.dot, r.output
            ));
        }
    }
    if !bad.is_empty() {
        panic!("{} case(s) differ:\n{}", bad.len(), bad.join("\n"));
    }
}

fn fails(text: &str, dot: (usize, usize), program: &str, msg: &str) {
    let r = ours(text, dot, program);
    assert_eq!(r.error.as_deref(), Some(msg), "program {program:?} on {text:?}");
}

macro_rules! case {
    ($text:expr, $dot:expr, $prog:expr => $wt:expr, $wd:expr, $wo:expr) => {
        Case { text: $text, dot: $dot, program: $prog, want_text: $wt, want_dot: $wd, want_out: $wo }
    };
}

#[test]
fn text_commands() {
    check(&[
        // a, i, c, d on dot; dot after an insertion selects the inserted text
        case!("hello\n", (0, 0), "a/X/" => "Xhello\n", (0, 1), ""),
        case!("hello\n", (2, 3), "a/X/" => "helXlo\n", (3, 4), ""),
        case!("hello\n", (2, 3), "i/X/" => "heXllo\n", (2, 3), ""),
        case!("hello\n", (2, 3), "c/XY/" => "heXYlo\n", (2, 4), ""),
        case!("hello\n", (1, 4), "d" => "ho\n", (1, 1), ""),
        case!("hello\n", (0, 5), "c//" => "\n", (0, 0), ""),
        // any delimiter; \n in text; elided trailing delimiter
        case!("ab\n", (2, 2), "a,X\\nY," => "abX\nY\n", (2, 5), ""),
        case!("ab\n", (2, 2), "a/X" => "abX\n", (2, 3), ""),
        // multi-line form keeps the trailing newline of the last line
        case!("ab\n", (0, 0), "i\nfirst\nsecond\n.\n" => "first\nsecond\nab\n", (0, 13), ""),
        // addresses on text commands
        case!("l1\nl2\nl3\n", (0, 0), "2d" => "l1\nl3\n", (3, 3), ""),
        case!("l1\nl2\nl3\n", (0, 0), "2c/X/" => "l1\nXl3\n", (3, 4), ""), // a line address includes its newline
        case!("l1\nl2\nl3\n", (0, 0), "2a/X/" => "l1\nl2\nXl3\n", (6, 7), ""),
        case!("l1\nl2\nl3\n", (0, 0), "$a/end/" => "l1\nl2\nl3\nend", (9, 12), ""),
        case!("l1\nl2\nl3\n", (0, 0), "0a/top\\n/" => "top\nl1\nl2\nl3\n", (0, 4), ""),
        case!("l1\nl2\nl3\n", (0, 0), ",d" => "", (0, 0), ""),
        case!("l1\nl2\nl3\n", (0, 0), "#2,#5d" => "l1\nl3\n", (2, 2), ""),
    ]);
}

#[test]
fn substitute() {
    check(&[
        case!("abcabc\n", (0, 7), "s/b/X/" => "aXcabc\n", (0, 7), ""),
        case!("abcabc\n", (0, 7), "s/b/X/g" => "aXcaXc\n", (0, 7), ""),
        case!("abcabc\n", (0, 7), "s2/b/X/" => "abcaXc\n", (0, 7), ""),
        case!("abcabc\n", (0, 7), "s2/b/X/g" => "abcaXc\n", (0, 7), ""),
        case!("abcabc\n", (0, 7), "s/b/[&]/g" => "a[b]ca[b]c\n", (0, 11), ""),
        case!("abcabc\n", (0, 7), "s/(a)(b)/\\2\\1/g" => "bacbac\n", (0, 7), ""),
        case!("abcabc\n", (0, 7), "s/b/\\&/" => "a&cabc\n", (0, 7), ""),
        case!("a.b\n", (0, 4), "s/\\./-/" => "a-b\n", (0, 4), ""),
        case!("ab\n", (0, 3), "s/a/x\\ny/" => "x\nyb\n", (0, 5), ""),
        // empty matches substitute once per position
        case!("abc\n", (0, 3), "s/x*/-/g" => "-a-b-c-\n", (0, 7), ""),
        case!("abc\n", (0, 3), ",s/^/> /" => "> abc\n", (0, 6), ""),
        case!("l1\nl2\n", (0, 0), ",s/$/;/g" => "l1;\nl2;\n", (0, 8), ""),
        // the whole file as range; only within range
        case!("aXa\naXa\n", (0, 0), "2s/X/Y/" => "aXa\naYa\n", (4, 8), ""),
    ]);
    fails("abc\n", (0, 4), "s/zzz/y/", "no substitution");
    fails("abc\n", (0, 4), "s/(a/y/", "bad regexp in s command: unmatched `('");
}

#[test]
fn move_and_copy() {
    check(&[
        // m and t do not set dot in acme (ecmd.c move/copy); dot stays where it was
        case!("l1\nl2\nl3\n", (0, 0), "1m$" => "l2\nl3\nl1\n", (0, 0), ""),
        case!("l1\nl2\nl3\n", (0, 0), "3m0" => "l3\nl1\nl2\n", (0, 3), ""),
        case!("l1\nl2\nl3\n", (0, 0), "1t$" => "l1\nl2\nl3\nl1\n", (0, 0), ""),
        case!("l1\nl2\nl3\n", (0, 0), "1t1" => "l1\nl1\nl2\nl3\n", (0, 0), ""),
        case!("l1\nl2\nl3\n", (0, 0), "2m2" => "l1\nl2\nl3\n", (0, 0), ""),
        case!("abcd", (0, 0), "#1,#3t#0" => "bcabcd", (0, 2), ""),
    ]);
    fails("l1\nl2\nl3\n", (0, 0), "1,2m1", "move overlaps itself");
}

#[test]
fn addresses() {
    check(&[
        case!("l1\nl2\nl3\n", (0, 0), "2p" => "l1\nl2\nl3\n", (3, 6), "l2\n"),
        case!("l1\nl2\nl3\n", (0, 0), "$p" => "l1\nl2\nl3\n", (9, 9), ""),
        case!("l1\nl2\nl3\n", (0, 0), "0p" => "l1\nl2\nl3\n", (0, 0), ""),
        case!("l1\nl2\nl3\n", (0, 0), "1,2p" => "l1\nl2\nl3\n", (0, 6), "l1\nl2\n"),
        case!("l1\nl2\nl3\n", (0, 0), ",p" => "l1\nl2\nl3\n", (0, 9), "l1\nl2\nl3\n"),
        case!("l1\nl2\nl3\n", (0, 0), "2,p" => "l1\nl2\nl3\n", (3, 9), "l2\nl3\n"),
        case!("l1\nl2\nl3\n", (0, 0), ",2p" => "l1\nl2\nl3\n", (0, 6), "l1\nl2\n"),
        case!("l1\nl2\nl3\n", (0, 0), "#1,#4p" => "l1\nl2\nl3\n", (1, 4), "1\nl"),
        case!("l1\nl2\nl3\n", (0, 0), "#3p" => "l1\nl2\nl3\n", (3, 3), ""),
        case!("l1\nl2\nl3\n", (3, 6), ".p" => "l1\nl2\nl3\n", (3, 6), "l2\n"),
        case!("l1\nl2\nl3\n", (3, 6), "+p" => "l1\nl2\nl3\n", (6, 9), "l3\n"),
        case!("l1\nl2\nl3\n", (3, 6), "-p" => "l1\nl2\nl3\n", (0, 3), "l1\n"),
        // from the start of the file, +2 is line 2 (the first newline counts)
        case!("l1\nl2\nl3\n", (0, 0), "+2p" => "l1\nl2\nl3\n", (3, 6), "l2\n"),
        case!("l1\nl2\nl3\n", (6, 9), "-2p" => "l1\nl2\nl3\n", (0, 3), "l1\n"),
        case!("l1\nl2\nl3\n", (4, 5), "+-p" => "l1\nl2\nl3\n", (3, 6), "l2\n"),
        case!("l1\nl2\nl3\n", (4, 5), "-+p" => "l1\nl2\nl3\n", (3, 6), "l2\n"),
        case!("l1\nl2\nl3\n", (0, 0), "/l2/p" => "l1\nl2\nl3\n", (3, 5), "l2"),
        case!("l1\nl2\nl3\n", (0, 0), "/l2/+#1p" => "l1\nl2\nl3\n", (6, 6), ""),
        case!("l1\nl2\nl3\n", (0, 0), "/l2/,/l3/p" => "l1\nl2\nl3\n", (3, 8), "l2\nl3"),
        case!("ab ab ab", (8, 8), "?ab?p" => "ab ab ab", (6, 8), "ab"),
        case!("ab ab ab", (7, 8), "?ab?p" => "ab ab ab", (3, 5), "ab"),
        case!("ab ab ab", (0, 0), "?ab?p" => "ab ab ab", (6, 8), "ab"),
        // ; sets dot for the right side
        case!("a\nb\nc\nd\n", (0, 0), "2;+p" => "a\nb\nc\nd\n", (2, 6), "b\nc\n"),
        // with , the right side is relative to the original dot, not to the left side
        case!("a\nb\nc\nd\n", (0, 0), "2,+p" => "a\nb\nc\nd\n", (2, 2), ""),
        // juxtaposition
        case!("a\nb\nc\nd\n", (0, 0), "1/c/p" => "a\nb\nc\nd\n", (4, 5), "c"),
        case!("a\nb\nc\nd\n", (0, 0), "$-1p" => "a\nb\nc\nd\n", (6, 8), "d\n"),
        // searches wrap around
        case!("a\nb\nc\n", (4, 6), "/a/p" => "a\nb\nc\n", (0, 1), "a"),
        case!("a\nb\nc\n", (0, 0), "?c?p" => "a\nb\nc\n", (4, 5), "c"),
        // #0 and line 0 are the beginning
        case!("abc", (0, 0), "#0a/X/" => "Xabc", (0, 1), ""),
        // last line without newline
        case!("a\nb", (0, 0), "2p" => "a\nb", (2, 3), "b"),
        case!("a\nb", (0, 0), "$-0p" => "a\nb", (2, 3), "b"), // -0 is the line containing the address
    ]);
    fails("l1\nl2\n", (0, 0), "5p", "address out of range");
    fails("l1\nl2\n", (0, 0), "#9p", "address out of range");
    fails("l1\nl2\n", (0, 0), "3,1p", "addresses out of order"); // 2,1 is the empty range at line 2
    fails("l1\nl2\n", (0, 0), "/zz/p", "no match for regexp");
    fails("l1\nl2\n", (0, 0), "-2p", "address out of range"); // - from line 1 is the empty range at 0
}

#[test]
fn loops() {
    check(&[
        // after a modifying loop, dot is the last range adjusted by the (merged)
        // change log, as acme does it: the two replacements merge into one
        case!("a1b22c\n", (0, 0), ",x/[0-9]+/ c/N/" => "aNbNc\n", (1, 4), ""),
        case!("a1b22c\n", (0, 0), ",x/[0-9]+/ p" => "a1b22c\n", (3, 5), "122"),
        case!("a1b22c\n", (0, 0), ",y/[0-9]+/ p" => "a1b22c\n", (5, 7), "abc\n"),
        case!("a1b22c\n", (0, 0), ",y/[0-9]+/ c/-/" => "-1-22-", (0, 6), ""), // the last y range holds the newline
        // x without a pattern loops over lines
        case!("a\nbb\nccc\n", (0, 0), ",x p" => "a\nbb\nccc\n", (5, 9), "a\nbb\nccc\n"),
        case!("a\nbb\nccc\n", (0, 0), ",x c/L\\n/" => "L\nL\nL\n", (0, 6), ""),
        // g and v
        // the loop leaves dot at its last range, even when that range did nothing
        case!("a\nbb\nccc\n", (0, 0), ",x g/bb/ d" => "a\nccc\n", (2, 6), ""),
        case!("a\nbb\nccc\n", (0, 0), ",x v/bb/ d" => "bb\n", (3, 3), ""),
        case!("a\nbb\nccc\n", (0, 0), ",x/.*\\n/ g/c/ s/c/C/g" => "a\nbb\nCCC\n", (5, 9), ""),
        // empty matches: before every char and at the end of the range
        case!("ab\n", (0, 0), ",x/x*/ c/-/" => "-a-b-\n-", (0, 7), ""),
        case!("ab\n", (0, 0), ",x/^/ c/> /" => "> ab\n", (0, 2), ""),
        // nested loops
        case!("a b\nc d\n", (0, 0), ",x/.*\\n/ x/[a-z]/ c/X/" => "X X\nX X\n", (0, 7), ""),
        // the empty pattern is the last one
        case!("abab\n", (0, 0), ",x/b/ c/X/\n,x// c/Y/" => "aXaX\n", (1, 4), ""),
        // dot after x is the last match
        case!("abab\n", (0, 0), ",x/a/ p" => "abab\n", (2, 3), "aa"),
        // s inside x with no match is not an error
        case!("ab\ncd\n", (0, 0), ",x/.*\\n/ s/a/A/" => "Ab\ncd\n", (3, 6), ""),
    ]);
    fails("ab\n", (0, 0), ",x/*/ d\n", "bad regexp in x command: missing operand for *");
}

#[test]
fn braces() {
    check(&[
        case!("abc\n", (0, 0), "{\na/X/\n$a/Y/\n}\n" => "Xabc\nY", (5, 6), ""),
        case!("abc\n", (0, 3), "{\ni/[/\na/]/\n}\n" => "[abc]\n", (4, 5), ""),
        case!("a\nb\n", (0, 0), ",x/.*\\n/ {\n=#\np\n}\n" => "a\nb\n", (2, 4), "#0,#2\na\n#2,#4\nb\n"),
    ]);
}

#[test]
fn printing_addresses() {
    check(&[
        case!("l1\nl2\nl3\n", (0, 0), "2=" => "l1\nl2\nl3\n", (0, 0), "2\n"),
        case!("l1\nl2\nl3\n", (0, 0), ",=" => "l1\nl2\nl3\n", (0, 0), "1,3\n"),
        case!("l1\nl2\nl3\n", (0, 0), "2=#" => "l1\nl2\nl3\n", (0, 0), "#3,#6\n"),
        case!("l1\nl2\nl3\n", (0, 0), "$=#" => "l1\nl2\nl3\n", (0, 0), "#9\n"),
        case!("l1\nl2\nl3\n", (0, 0), "#4=+" => "l1\nl2\nl3\n", (0, 0), "2+#1\n"),
        case!("l1\nl2\nl3\n", (0, 0), "#1,#4=+" => "l1\nl2\nl3\n", (0, 0), "1+#1,2+#1\n"),
        case!("abc", (1, 2), "=" => "abc", (1, 2), "1\n"),
    ]);
    fails("abc", (0, 0), "=x", "newline expected");
}

#[test]
fn newline_command() {
    check(&[
        // extends dot to line boundaries; if unchanged, the next line
        case!("l1\nl2\nl3\n", (4, 4), "\n" => "l1\nl2\nl3\n", (3, 6), ""),
        case!("l1\nl2\nl3\n", (3, 6), "\n" => "l1\nl2\nl3\n", (6, 9), ""),
        case!("l1\nl2\nl3\n", (0, 0), "2\n" => "l1\nl2\nl3\n", (3, 6), ""),
    ]);
}

#[test]
fn intents() {
    let r = ours("abc\n", (0, 4), "u");
    assert_eq!(r.intents, vec![Intent::Undo { n: 1 }]);
    let r = ours("abc\n", (0, 4), "u3");
    assert_eq!(r.intents, vec![Intent::Undo { n: 3 }]);
    let r = ours("abc\n", (0, 4), "u-1");
    assert_eq!(r.intents, vec![Intent::Undo { n: -1 }]);
    let r = ours("abc\n", (1, 2), "|sort");
    assert_eq!(r.intents, vec![Intent::Pipe { kind: PipeKind::Through, cmd: "sort".into(), q0: 1, q1: 2 }]);
    let r = ours("abc\n", (1, 2), ",>wc -l");
    assert_eq!(r.intents, vec![Intent::Pipe { kind: PipeKind::To, cmd: "wc -l".into(), q0: 0, q1: 4 }]);
    let r = ours("abc\n", (1, 2), "<date");
    assert_eq!(r.intents, vec![Intent::Pipe { kind: PipeKind::From, cmd: "date".into(), q0: 1, q1: 2 }]);
    let r = ours("abc\n", (1, 2), "w /tmp/x");
    assert_eq!(r.intents, vec![Intent::Write { name: Some("/tmp/x".into()), q0: 0, q1: 4 }]);
    let r = ours("abc\n", (1, 2), "2,3w /tmp/x");
    assert!(r.error.is_some()); // address out of range on a one-line file
    let r = ours("abc\n", (1, 2), "e /tmp/x");
    assert_eq!(r.intents, vec![Intent::Load { name: Some("/tmp/x".into()) }]);
    let r = ours("abc\n", (1, 2), "r /tmp/x");
    assert_eq!(r.intents, vec![Intent::Read { name: Some("/tmp/x".into()), q0: 1, q1: 2 }]);
    let r = ours("abc\n", (1, 2), "B a.c b.c");
    assert_eq!(r.intents, vec![Intent::Open { list: "a.c b.c".into() }]);
    let r = ours("abc\n", (1, 2), "D");
    assert_eq!(r.intents, vec![Intent::Delete { list: "".into() }]);
    fails("abc\n", (0, 1), "|", "no command specified for |");
    fails("abc\n", (0, 1), "d\nw", "can't write file with pending modifications");
    fails("abc\n", (0, 1), "w", "no name specified for 'w' command");
}

#[test]
fn errors() {
    fails("abc\n", (0, 0), "q", "unknown command q");
    fails("abc\n", (0, 0), "1u", "command takes no address");
    fails("abc\n", (0, 0), "axhellox", "bad delimiter x");
    fails("abc\n", (0, 0), "p x", "newline expected (saw x)");
    fails("abc\n", (0, 0), "}", "right brace with no left brace");
    fails("abc\n", (0, 0), "x//p", "no regular expression defined");
    fails("abc\n", (0, 0), "x/(a/p", "bad regexp in x command: unmatched `('");
    fails("abc\n", (0, 0), "/[a/p", "bad regexp in command address: malformed `[]'");
    fails("abc\n", (0, 0), "X/a/p", "X command is not supported");
    fails("abc\n", (0, 0), "cd /tmp", "unknown command cd");
    fails("abc\n", (0, 0), "s", "no address");
    fails("abc\n", (0, 0), "m", "bad address");
}

#[test]
fn out_of_sequence_warns_and_proceeds_like_acme() {
    let mut t: Vec<char> = "abcdef\n".chars().collect();
    let o = apex_edit::Edit::new().run(&t, (0, 0), None, "{\n#4,#5d\n#1,#2d\n}\n").unwrap();
    assert!(o.output.is_empty());
    assert_eq!(o.warnings, vec!["warning: changes out of sequence".to_string()]);
    // acme flushes the pending change and applies the log last-recorded
    // first, so the earlier delete lands on the already-modified text and
    // removes 'f', not 'e'. acme's own comment calls these addresses bogus;
    // we reproduce it exactly.
    apex_edit::apply(&mut t, &o.changes);
    assert_eq!(t.iter().collect::<String>(), "acde\n");
}

#[test]
fn last_pattern_persists_across_programs() {
    let mut e = apex_edit::Edit::new();
    let r = common::ours_with(&mut e, "abab\n", (0, 0), ",x/b/ c/X/");
    assert_eq!(r.text, "aXaX\n");
    let r = common::ours_with(&mut e, "abab\n", (0, 0), ",x// c/Y/");
    assert_eq!(r.text, "aYaY\n");
}

#[test]
fn runes_not_bytes() {
    check(&[
        case!("héllo wörld\n", (0, 0), "#1,#2p" => "héllo wörld\n", (1, 2), "é"),
        case!("héllo wörld\n", (0, 0), ",x/[éö]/ c/_/" => "h_llo w_rld\n", (1, 8), ""),
        case!("日本語\n", (0, 0), "#1d" => "日本語\n", (1, 1), ""),
        case!("日本語\n", (0, 0), "#1,#2d" => "日語\n", (1, 1), ""),
        case!("日本語\n", (0, 0), ",=#" => "日本語\n", (0, 0), "#0,#4\n"),
    ]);
}
