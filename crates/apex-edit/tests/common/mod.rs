//! Shared helpers: run a program through apex-edit, and through plan9port's
//! `sam -d` when it is available, and normalise both results to the same
//! shape for comparison.

#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use apex_edit::{apply, Edit, Intent, Printed};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub error: Option<String>,
    pub text: String,
    pub dot: (usize, usize),
    /// everything printed, as acme would show it
    pub output: String,
    /// `p` text, then a NUL, then `=` lines: the shape sam's two streams give
    pub output_split: String,
    pub intents: Vec<Intent>,
    pub warnings: Vec<String>,
}

/// Run `program` over `text` with dot `dot` through apex-edit.
pub fn ours(text: &str, dot: (usize, usize), program: &str) -> Run {
    ours_with(&mut Edit::new(), text, dot, program)
}

pub fn ours_with(edit: &mut Edit, text: &str, dot: (usize, usize), program: &str) -> Run {
    let mut t: Vec<char> = text.chars().collect();
    match edit.run(&t, dot, None, program) {
        Ok(o) => {
            apply(&mut t, &o.changes);
            let mut p_part = String::new();
            let mut eq_part = String::new();
            for item in &o.output {
                match item {
                    Printed::Text(s) => p_part.push_str(s),
                    Printed::Address(s) => eq_part.push_str(s),
                }
            }
            Run {
                error: None,
                text: t.iter().collect(),
                dot: o.dot,
                output: o.output_string(),
                output_split: format!("{p_part}\0{eq_part}"),
                intents: o.intents,
                warnings: o.warnings,
            }
        }
        Err(e) => Run {
            error: Some(e.0),
            text: text.to_string(),
            dot,
            output: String::new(),
            output_split: String::new(),
            intents: vec![],
            warnings: vec![],
        },
    }
}

/// Where plan9port's sam lives, if anywhere.
pub fn sam_binary() -> Option<PathBuf> {
    let candidates = [
        std::env::var("SAM").ok().map(PathBuf::from),
        std::env::var("PLAN9").ok().map(|p| PathBuf::from(p).join("bin/sam")),
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".local/plan9/bin/sam")),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Run `program` over `text` through `sam -d`, from dot (0,0), which is
/// sam's initial dot. `=#` is appended to read dot back and `w` to read the
/// resulting text back from the file.
pub fn sam(text: &str, program: &str) -> Option<Run> {
    let bin = sam_binary()?;
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("apex-edit-sam-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join("f");
    std::fs::write(&file, text).ok()?;
    let mut child = Command::new(&bin)
        .arg("-d")
        .arg(&file)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    {
        let mut stdin = child.stdin.take()?;
        let mut p = program.to_string();
        if !p.ends_with('\n') {
            p.push('\n');
        }
        p.push_str("=#\nw\n");
        let _ = stdin.write_all(p.as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    // sam -d prints `p` text on stdout; the menu line, errors, `=` results
    // and "?changed files" on stderr.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if std::env::var_os("SAM_DEBUG").is_some() {
        eprintln!("--- sam stdout for {program:?} on {text:?}:\n{stdout}--- stderr:\n{stderr}---");
    }
    let result_text = std::fs::read_to_string(&file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    let file_prefix = format!("{}: ", file.display());
    let mut err_lines: Vec<String> = Vec::new();
    for (i, l) in stderr.split_inclusive('\n').enumerate() {
        if i == 0 {
            continue; // the menu line " -. file"
        }
        let l = l.strip_suffix("?changed files").unwrap_or(l);
        if l.starts_with(&file_prefix) {
            break; // the w report and anything after it
        }
        if !l.is_empty() {
            err_lines.push(l.to_string());
        }
    }
    let is_dot_line = |l: &str| {
        let b = l.trim_end_matches('\n');
        b.strip_prefix('#').is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '#'))
    };
    let dot_ix = err_lines.iter().rposition(|l| is_dot_line(l));
    let mut error = None;
    // stdout (p text) and stderr (= results) are kept apart by a NUL so a
    // final p line without a newline is not glued to the next = line.
    let p_part = stdout.strip_suffix("?changed files").unwrap_or(&stdout).to_string();
    let mut output = String::new();
    let mut dot = (0, 0);
    for (i, l) in err_lines.iter().enumerate() {
        let body = l.trim_end_matches('\n');
        if let Some(rest) = body.strip_prefix('?') {
            if rest != "changed files" && error.is_none() {
                error = Some(rest.to_string());
            }
            continue;
        }
        if Some(i) == dot_ix {
            let rest = &body[1..];
            let mut it = rest.split(",#");
            let q0: usize = it.next().unwrap_or("0").parse().unwrap_or(0);
            let q1: usize = it.next().map(|x| x.parse().unwrap_or(q0)).unwrap_or(q0);
            dot = (q0, q1);
            continue;
        }
        output.push_str(l);
    }
    // sam prints `=` as "L[,L]; #q0[,#q1]"; acme prints only the line part
    let eq_part = output
        .split_inclusive('\n')
        .map(|l| {
            let body = l.trim_end_matches('\n');
            match body.split_once("; #") {
                Some((lines, _)) if lines.chars().all(|c| c.is_ascii_digit() || c == ',') => {
                    format!("{lines}{}", if l.ends_with('\n') { "\n" } else { "" })
                }
                _ => l.to_string(),
            }
        })
        .collect::<String>();
    Some(Run {
        error,
        text: result_text,
        dot,
        output: format!("{p_part}{eq_part}"),
        output_split: format!("{p_part}\0{eq_part}"),
        intents: vec![],
        warnings: vec![],
    })
}

/// The command characters used at any depth of the program.
fn command_chars(program: &str) -> Vec<char> {
    fn walk(c: &apex_edit::parse::Cmd, out: &mut Vec<char>) {
        out.push(c.cmdc);
        if let apex_edit::parse::Arg::Cmd(sub) = &c.arg {
            let mut s = Some(sub.as_ref());
            while let Some(x) = s {
                walk(x, out);
                s = x.next.as_deref();
            }
        }
    }
    let mut lp = Vec::new();
    let mut p = apex_edit::parse::Parser::new(program, &mut lp);
    let mut out = Vec::new();
    loop {
        match p.parsecmd(0) {
            Ok(Some(c)) => walk(&c, &mut out),
            Ok(None) => return out,
            Err(_) => {
                out.push('?');
                return out;
            }
        }
    }
}

/// Does the program change the text? Dot after a modifying program follows
/// acme's change-log rules, which differ from sam's, so it is compared with
/// sam only for programs that only look.
pub fn modifying(program: &str) -> bool {
    command_chars(program).iter().any(|c| "acidsmtruew<|>?".contains(*c))
}

/// The bare newline command: `sam -d` prints the resulting dot, acme only
/// selects it, so its output is not comparable.
pub fn uses_newline_cmd(program: &str) -> bool {
    command_chars(program).contains(&'\n')
}

#[allow(dead_code)]
fn unused_walk(program: &str) -> bool {
    fn walk(c: &apex_edit::parse::Cmd) -> bool {
        if "acidsmtruew<|>".contains(c.cmdc) {
            return true;
        }
        if let apex_edit::parse::Arg::Cmd(sub) = &c.arg {
            let mut s = Some(sub.as_ref());
            while let Some(x) = s {
                if walk(x) {
                    return true;
                }
                s = x.next.as_deref();
            }
        }
        false
    }
    let mut lp = Vec::new();
    let mut p = apex_edit::parse::Parser::new(program, &mut lp);
    loop {
        match p.parsecmd(0) {
            Ok(Some(c)) => {
                if walk(&c) {
                    return true;
                }
            }
            Ok(None) => return false,
            Err(_) => return true,
        }
    }
}

/// Output as a sorted multiset of lines: sam splits `p` text and `=`
/// results over two streams, so their interleaving is not comparable.
fn lines_sorted(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .split('\0')
        .flat_map(|part| part.split_inclusive('\n'))
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    v.sort();
    v
}

/// Compare a run of ours against sam's for one case. Errors are compared
/// by presence only (the messages differ); text and output must be equal
/// when neither side failed, and dot too for non-modifying programs.
pub fn compare(text: &str, program: &str) -> Result<(), String> {
    // Known divergence: sam's backward machine lets `$` match at the very
    // end of the text; acme's regx.c (which we follow) never does.
    if program.contains('?') && program.contains('$') {
        return Ok(());
    }
    let Some(s) = sam(text, program) else { return Ok(()) };
    let o = ours(text, (0, 0), program);
    let check_dot = !modifying(program);
    let check_out = !uses_newline_cmd(program);
    match (&s.error, &o.error) {
        (Some(_), Some(_)) => Ok(()),
        // acme's out-of-sequence checks are weaker than sam's: it warns (or
        // says nothing) and proceeds where sam refuses
        (Some(se), None) if se == "changes not in sequence" || !o.warnings.is_empty() => Ok(()),
        (Some(se), None) => Err(format!(
            "sam failed ({se}) but we succeeded\n text={text:?}\n program={program:?}\n ours: text={:?} dot={:?} out={:?}",
            o.text, o.dot, o.output
        )),
        // acme refuses to move a range to a point inside itself (except onto
        // itself); sam's `move` only checks the destination's end
        (None, Some(oe)) if oe == "move overlaps itself" => Ok(()),
        (None, Some(oe)) => Err(format!(
            "we failed ({oe}) but sam succeeded\n text={text:?}\n program={program:?}\n sam: text={:?} dot={:?} out={:?}",
            s.text, s.dot, s.output
        )),
        (None, None) => {
            if s.text != o.text || (check_dot && s.dot != o.dot) || (check_out && lines_sorted(&s.output_split) != lines_sorted(&o.output_split)) {
                Err(format!(
                    "mismatch\n text={text:?}\n program={program:?}\n sam:  text={:?} dot={:?} out={:?}\n ours: text={:?} dot={:?} out={:?}",
                    s.text, s.dot, s.output, o.text, o.dot, o.output
                ))
            } else {
                Ok(())
            }
        }
    }
}
