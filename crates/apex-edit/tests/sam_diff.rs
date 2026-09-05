//! Differential tests against plan9port's `sam -d`, the reference
//! implementation of the command language. Skipped (with a note) when sam
//! is not installed; set `SAM=/path/to/sam` or `PLAN9=/path/to/plan9port`.
//!
//! A hand-written corpus runs first, then generated programs and regular
//! expressions over small random texts.

mod common;

use common::{compare, sam_binary};
use proptest::prelude::*;

fn have_sam() -> bool {
    match sam_binary() {
        Some(_) => true,
        None => {
            eprintln!("skipping: plan9port sam not found (set SAM or PLAN9)");
            false
        }
    }
}

const TEXTS: &[&str] = &[
    "",
    "a",
    "a\n",
    "ab\n",
    "abc",
    "hello world\n",
    "l1\nl2\nl3\n",
    "a\nbb\nccc\ndddd\n",
    "a1b22c333\n",
    "aaa\nbbb\naaa\n",
    "foo bar foo\nbar foo bar\n",
    "x\n\ny\n\n",
    "héllo wörld\nñ\n",
    "the quick brown fox\njumps over\nthe lazy dog\n",
    "(a) [b] {c}\n",
];

const PROGRAMS: &[&str] = &[
    "a/X/",
    "i/X/",
    "c/X/",
    "d",
    ",d",
    "1d",
    "2d",
    "$d",
    "$a/end/",
    "0a/top\\n/",
    "1a/X/",
    "2c/X\\n/",
    "#1a/X/",
    "#0,#2d",
    "#1,#3c/Q/",
    ",p",
    "1p",
    "2p",
    "$p",
    "1,2p",
    "2,p",
    ",2p",
    "#1,#4p",
    "+p",
    "-p",
    "+2p",
    "-2p",
    "+-p",
    "-+p",
    "/l2/p",
    "/a/p",
    "?a?p",
    "/a/+#1p",
    "/a/,/b/p",
    "2;+p",
    "2,+p",
    "1/c/p",
    "$-1p",
    "$-0p",
    "s/a/X/",
    "s/a/X/g",
    ",s/a/X/g",
    ",s/a/[&]/g",
    ",s/(a)(b)/\\2\\1/g",
    ",s/x*/-/g",
    ",s/^/> /",
    ",s/^/> /g",
    ",s/$/;/g",
    ",s/\\n/ /g",
    "s2/a/X/",
    "s2/a/X/g",
    "1m$",
    "2m0",
    "1t$",
    "1t1",
    "2m2",
    "1,2m$",
    "#1,#3t#0",
    ",x/a/ c/X/",
    ",x/[0-9]+/ c/N/",
    ",x/[0-9]+/ p",
    ",y/[0-9]+/ p",
    ",y/[0-9]+/ c/-/",
    ",y/a/ c/-/",
    ",x p",
    ",x c/L\\n/",
    ",x d",
    ",x/x*/ c/-/",
    ",x/^/ c/> /",
    ",x/$/ c/;/",
    ",x/.*\\n/ g/b/ d",
    ",x/.*\\n/ v/b/ d",
    ",x/.*\\n/ g/c/ s/c/C/g",
    ",x/.*\\n/ x/[a-z]/ c/X/",
    ",x/a/ x/a/ c/Y/",
    ",x/b+/ i/</",
    ",x/b+/ a/>/",
    ",x/b+/ {\ni/</\na/>/\n}",
    ",x/.*\\n/ {\n=#\np\n}",
    "{\na/X/\n$a/Y/\n}",
    ",x/.*\\n/ =#",
    "2=#",
    ",=#",
    "$=#",
    "2=",
    ",=",
    "/o/=",
    "\n",
    "2\n",
    ",x/o/ \n",
    ",x/ +/ d",
    ",x/[a-z]+/ y/o/ c/_/",
    ",x/fo(o)/ c/\\1/",
    "s/b/X/\ns/c/Y/",
    ",x/a/ c/X/\n,x// c/Y/",
    ",x/l/ {\n c/L/\n}",
    "1,$-1d",
    ".,.+1d",
    "/b/;/c/p",
    "?b?;?a?p",
    "/b/-p",
    "/b/+p",
    "/b/-1p",
    "#2+3p",
    "#2-1p",
    "5p",
    "#99p",
    "2,1p",
    "/zzz/p",
    "s/zzz/y/",
    ",x/(a/ d",
    "1,2m1",
    "{\n#4,#5d\n#1,#2d\n}",
];

#[test]
fn corpus_matches_sam() {
    if !have_sam() {
        return;
    }
    let mut failures = Vec::new();
    for text in TEXTS {
        for program in PROGRAMS {
            if let Err(e) = compare(text, program) {
                failures.push(e);
            }
        }
    }
    if !failures.is_empty() {
        panic!("{} differences from sam:\n\n{}", failures.len(), failures.join("\n\n"));
    }
}

// ---- generated programs ---------------------------------------------------

fn small_text() -> impl Strategy<Value = String> {
    prop::collection::vec(prop_oneof![Just('a'), Just('b'), Just(' '), Just('\n'), Just('c')], 0..12)
        .prop_map(|v| v.into_iter().collect())
}

fn regexp() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        Just("a".to_string()),
        Just("b".to_string()),
        Just("c".to_string()),
        Just(".".to_string()),
        Just("[ab]".to_string()),
        Just("[^a]".to_string()),
        Just("\\n".to_string()),
        Just(" ".to_string()),
    ];
    let piece = atom.prop_flat_map(|a| {
        prop_oneof![Just(a.clone()), Just(format!("{a}*")), Just(format!("{a}+")), Just(format!("{a}?"))]
    });
    let seq = prop::collection::vec(piece, 1..4).prop_map(|v| v.concat());
    let alt = prop::collection::vec(seq, 1..3).prop_map(|v| v.join("|"));
    prop_oneof![
        alt.clone(),
        alt.clone().prop_map(|a| format!("({a})")),
        alt.clone().prop_map(|a| format!("^{a}")),
        alt.prop_map(|a| format!("{a}$")),
    ]
}

/// A regexp usable inside `?...?`: sam(1) says a literal `?` in a backward
/// search must be written as a class, so we simply avoid it.
fn backward_regexp() -> impl Strategy<Value = String> {
    regexp().prop_filter("no ? in a ?re? address", |r| !r.contains('?'))
}

/// A regexp for a command (x, y, g, v, s), which searches a sub-range of the
/// text. sam lets `$` see the character just past the range while acme's
/// regx.c feeds a NUL there, so command regexps avoid `$`; address searches
/// and the top-level regexp test cover `$` over the whole text, where the
/// engines agree.
fn cmd_regexp() -> impl Strategy<Value = String> {
    regexp().prop_filter("no $ in a command regexp", |r| !r.contains('$'))
}

fn simple_addr() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("".to_string()),
        Just(".".to_string()),
        Just("$".to_string()),
        Just("0".to_string()),
        (1..4u8).prop_map(|n| n.to_string()),
        (0..6u8).prop_map(|n| format!("#{n}")),
        regexp().prop_map(|r| format!("/{r}/")),
        backward_regexp().prop_map(|r| format!("?{r}?")),
        Just("+".to_string()),
        Just("-".to_string()),
        Just("+2".to_string()),
        Just("-1".to_string()),
    ]
}

fn address() -> impl Strategy<Value = String> {
    prop_oneof![
        simple_addr(),
        (simple_addr(), simple_addr()).prop_map(|(a, b)| format!("{a},{b}")),
        (simple_addr(), simple_addr()).prop_map(|(a, b)| format!("{a};{b}")),
        Just(",".to_string()),
    ]
}

fn text_arg() -> impl Strategy<Value = String> {
    prop_oneof![Just("X".to_string()), Just("XY".to_string()), Just("".to_string()), Just("\\n".to_string())]
}

fn leaf_cmd() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("p".to_string()),
        Just("=#".to_string()),
        Just("d".to_string()),
        text_arg().prop_map(|t| format!("a/{t}/")),
        text_arg().prop_map(|t| format!("i/{t}/")),
        text_arg().prop_map(|t| format!("c/{t}/")),
        (cmd_regexp(), text_arg(), any::<bool>()).prop_map(|(r, t, g)| format!("s/{r}/{t}/{}", if g { "g" } else { "" })),
        (cmd_regexp(), any::<bool>()).prop_map(|(r, g)| format!("s/{r}/[&]/{}", if g { "g" } else { "" })),
        simple_addr().prop_map(|a| format!("m{a}")),
        simple_addr().prop_map(|a| format!("t{a}")),
    ]
}

fn command() -> impl Strategy<Value = String> {
    let leaf = leaf_cmd();
    leaf.prop_recursive(2, 8, 2, |inner| {
        prop_oneof![
            (cmd_regexp(), inner.clone()).prop_map(|(r, c)| format!("x/{r}/ {c}")),
            (cmd_regexp(), inner.clone()).prop_map(|(r, c)| format!("y/{r}/ {c}")),
            (cmd_regexp(), inner.clone()).prop_map(|(r, c)| format!("g/{r}/ {c}")),
            (cmd_regexp(), inner.clone()).prop_map(|(r, c)| format!("v/{r}/ {c}")),
            inner.clone().prop_map(|c| format!("x {c}")),
            prop::collection::vec(inner, 1..3).prop_map(|cs| format!("{{\n{}\n}}", cs.join("\n"))),
        ]
    })
}

fn program() -> impl Strategy<Value = String> {
    (address(), command()).prop_map(|(a, c)| format!("{a}{c}"))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 400,
        max_shrink_iters: 200,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Off)),
        .. ProptestConfig::default()
    })]

    #[test]
    fn generated_programs_match_sam(text in small_text(), program in program()) {
        if !have_sam() {
            return Ok(());
        }
        if let Err(e) = compare(&text, &program) {
            prop_assert!(false, "{}", e);
        }
    }

    #[test]
    fn generated_regexps_match_sam(text in small_text(), re in regexp()) {
        if !have_sam() {
            return Ok(());
        }
        // sam is the oracle for the regexp engine: print every match
        let program = format!(",x/{re}/ =#");
        if let Err(e) = compare(&text, &program) {
            prop_assert!(false, "{}", e);
        }
        // and the backward machine (a literal ? cannot appear in ?re?)
        if !re.contains('?') {
            let program = format!("$?{re}?=#");
            if let Err(e) = compare(&text, &program) {
                prop_assert!(false, "{}", e);
            }
        }
    }
}
