//! The file and shell commands, run through a fake [`Host`]: a few files
//! in a map, and a handful of "shell commands" done in Rust. Each command
//! is done when the program reaches it, so commands and text edits happen
//! in the order the program gives them.

use std::collections::HashMap;

use apex_edit::{apply, commands, Edit, Host, Intent, Ran, Result};

#[derive(Default)]
struct Fake {
    files: HashMap<String, String>,
    /// What each command was given, in order.
    ran: Vec<(String, Option<String>)>,
    modified: bool,
}

impl Host for Fake {
    fn path(&self, name: &str) -> String {
        if name.starts_with('/') { name.to_string() } else { format!("/d/{name}") }
    }
    fn modified(&self) -> bool {
        self.modified
    }
    fn run(&mut self, cmd: &str, input: Option<String>) -> Result<Ran> {
        self.ran.push((cmd.to_string(), input.clone()));
        let input = input.unwrap_or_default();
        let ran = match cmd {
            "sort" => {
                let mut lines: Vec<&str> = input.lines().collect();
                lines.sort();
                Ran { out: lines.iter().map(|l| format!("{l}\n")).collect(), errors: String::new() }
            }
            "upper" => Ran { out: input.to_uppercase(), errors: String::new() },
            "wc" => Ran { out: format!("{}\n", input.chars().count()), errors: String::new() },
            "date" => Ran { out: "today\n".into(), errors: String::new() },
            "ls" => Ran { out: "a.c\nb.c\n".into(), errors: String::new() },
            "fail" => Ran { out: String::new(), errors: "fail: exit 1\n".into() },
            _ => return Err(apex_edit::Error::new(format!("{cmd}: not found"))),
        };
        Ok(ran)
    }
    fn read(&mut self, path: &str) -> Result<String> {
        self.files.get(path).cloned().ok_or_else(|| apex_edit::Error::new(format!("can't open {path}")))
    }
    fn write(&mut self, path: &str, text: &str) -> Result<()> {
        self.files.insert(path.to_string(), text.to_string());
        Ok(())
    }
}

struct Run {
    text: String,
    dot: (usize, usize),
    output: String,
    intents: Vec<Intent>,
    clean: Option<bool>,
    name: Option<String>,
}

fn run(host: &mut Fake, text: &str, dot: (usize, usize), program: &str) -> Result<Run> {
    let mut t: Vec<char> = text.chars().collect();
    let o = Edit::new().run_with(&t, dot, Some("/d/f"), program, host)?;
    apply(&mut t, &o.changes);
    Ok(Run { text: t.iter().collect(), dot: o.dot, output: o.output_string(), intents: o.intents, clean: o.clean, name: o.name })
}

#[test]
fn pipe_through_replaces_the_range_with_the_output() {
    let mut h = Fake::default();
    let r = run(&mut h, "c\nb\na\n", (0, 0), ",|sort").unwrap();
    assert_eq!(r.text, "a\nb\nc\n");
    assert_eq!(r.dot, (0, 6));
    assert_eq!(h.ran, vec![("sort".to_string(), Some("c\nb\na\n".to_string()))]);
}

#[test]
fn pipe_from_replaces_the_range_without_input() {
    let mut h = Fake::default();
    let r = run(&mut h, "one\ntwo\n", (0, 0), "2<date").unwrap();
    assert_eq!(r.text, "one\ntoday\n");
    assert_eq!(h.ran, vec![("date".to_string(), None)]);
}

#[test]
fn pipe_to_sends_the_range_and_prints_the_output() {
    let mut h = Fake::default();
    let r = run(&mut h, "abc\n", (1, 2), ",>wc").unwrap();
    assert_eq!(r.text, "abc\n");
    assert_eq!(r.output, "4\n");
    assert_eq!(r.dot, (0, 4));
}

#[test]
fn a_commands_errors_are_printed() {
    let mut h = Fake::default();
    let r = run(&mut h, "abc\n", (0, 4), "|fail").unwrap();
    assert_eq!(r.text, "");
    assert_eq!(r.output, "fail: exit 1\n");
}

#[test]
fn pipes_in_a_loop_each_get_their_own_range() {
    let mut h = Fake::default();
    let r = run(&mut h, "ab cd ef", (0, 0), ",x/[a-z]+/ |upper").unwrap();
    assert_eq!(r.text, "AB CD EF");
    assert_eq!(h.ran.len(), 3);
}

#[test]
fn text_edits_and_pipes_happen_in_program_order() {
    let mut h = Fake::default();
    // every address refers to the original text, as for any command; the
    // pipe sees the text as it was, and its change and the edit's both land
    let r = run(&mut h, "b\na\nx\n", (0, 0), "1,2|sort\n3c/y\\n/").unwrap();
    assert_eq!(r.text, "a\nb\ny\n");
    // a shell error stops the program: nothing is applied
    let e = run(&mut h, "b\na\n", (0, 0), ",x/a/ c/A/\n,|nosuch");
    assert_eq!(e.err().unwrap().0, "nosuch: not found");
}

#[test]
fn write_writes_the_range_and_cleans_the_buffer_written_whole() {
    let mut h = Fake::default();
    let r = run(&mut h, "abc\n", (0, 0), "w").unwrap();
    assert_eq!(h.files["/d/f"], "abc\n");
    assert_eq!(r.clean, Some(true));
    // part of it to its own file: changed
    let r = run(&mut h, "abc\ndef\n", (0, 0), "2w").unwrap();
    assert_eq!(h.files["/d/f"], "def\n");
    assert_eq!(r.clean, Some(false));
    // elsewhere, relative to the window's directory: nothing said
    let r = run(&mut h, "abc\n", (0, 0), "w g").unwrap();
    assert_eq!(h.files["/d/g"], "abc\n");
    assert_eq!(r.clean, None);
    // changed after the write: not clean
    let r = run(&mut h, "abc\n", (0, 0), "w\n,c/x/").unwrap();
    assert_eq!(r.clean, None);
    assert_eq!(r.text, "x");
}

#[test]
fn write_then_a_command_reading_the_file_sees_it_written() {
    let mut h = Fake::default();
    run(&mut h, "abc\n", (0, 0), "w g\nr g").unwrap();
    assert_eq!(h.files["/d/g"], "abc\n");
}

#[test]
fn read_replaces_the_range_with_the_file() {
    let mut h = Fake::default();
    h.files.insert("/d/g".into(), "G\n".into());
    let r = run(&mut h, "one\ntwo\n", (0, 0), "2r g").unwrap();
    assert_eq!(r.text, "one\nG\n");
    assert_eq!(r.clean, None);
    assert_eq!(r.name, None);
    let e = run(&mut h, "one\n", (0, 0), "r nope");
    assert_eq!(e.err().unwrap().0, "can't open /d/nope");
    assert!(run(&mut h, "one\n", (0, 0), "r ").is_err());
}

#[test]
fn edit_loads_the_file_and_names_the_buffer() {
    let mut h = Fake::default();
    h.files.insert("/d/f".into(), "on disk\n".into());
    h.files.insert("/d/g".into(), "G\n".into());
    let r = run(&mut h, "changed\n", (0, 0), "e").unwrap();
    assert_eq!(r.text, "on disk\n");
    assert_eq!(r.clean, Some(true));
    assert_eq!(r.name, None);
    let r = run(&mut h, "changed\n", (2, 3), "e g").unwrap();
    assert_eq!(r.text, "G\n");
    assert_eq!(r.clean, None);
    assert_eq!(r.name.as_deref(), Some("/d/g"));
}

#[test]
fn nul_bytes_are_elided_and_said_to_be() {
    let mut h = Fake::default();
    h.files.insert("/d/f".into(), "a\0b\n".into());
    let r = run(&mut h, "x\n", (0, 0), "e").unwrap();
    assert_eq!(r.text, "ab\n");
    assert_eq!(r.output, "/d/f: NUL bytes elided\n");
    assert_eq!(r.clean, None);
}

#[test]
fn f_names_the_buffer_and_prints_it() {
    let mut h = Fake::default();
    let r = run(&mut h, "x\n", (0, 0), "f").unwrap();
    assert_eq!(r.output, " +. /d/f\n");
    assert_eq!(r.name, None);
    h.modified = true;
    let r = run(&mut h, "x\n", (0, 0), "f").unwrap();
    assert_eq!(r.output, "'+. /d/f\n");
    h.modified = false;
    let r = run(&mut h, "x\n", (0, 0), "f g\n=").unwrap();
    assert_eq!(r.output, "'+. /d/g\n/d/g:1\n");
    assert_eq!(r.name.as_deref(), Some("/d/g"));
}

#[test]
fn b_and_d_list_files_named_or_given_by_a_command() {
    let mut h = Fake::default();
    let r = run(&mut h, "x\n", (0, 0), "B <ls").unwrap();
    assert_eq!(r.intents, vec![Intent::Open { names: vec!["/d/a.c".into(), "/d/b.c".into()] }]);
    let r = run(&mut h, "x\n", (0, 0), "D /e/x y").unwrap();
    assert_eq!(r.intents, vec![Intent::Delete { names: vec!["/e/x".into(), "/d/y".into()] }]);
    // B leaves no window current, as in acme
    let e = run(&mut h, "x\n", (0, 0), "B a.c\n,p");
    assert_eq!(e.err().unwrap().0, "no current window");
    let e = run(&mut h, "x\n", (0, 0), "b a.c");
    assert_eq!(e.err().unwrap().0, "b command is not supported");
}

#[test]
fn commands_lists_nested_commands() {
    assert_eq!(commands(",x/a/ { c/b/\n|sort\n}").unwrap(), vec!['x', '{', 'c', '|']);
    assert_eq!(commands(",x// c/b/").unwrap(), vec!['x', 'c']);
    assert!(commands(",q").is_err());
}
