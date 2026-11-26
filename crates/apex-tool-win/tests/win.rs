//! apex tool win against a headless daemon: the shell's output lands at
//! the output point, a typed line goes to the shell, an interrupt
//! drops the typing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::remote::Remote;
use apex_server::Proposal;

/// The shell the tests run, as sh is with nothing of the user's: `ENV`
/// names the file an interactive sh reads first, which may give it a
/// prompt of the user's own where the tests look for sh's `$ `.
fn plain_sh() -> Vec<String> {
    std::env::remove_var("ENV");
    vec!["/bin/sh".to_string()]
}

fn daemon() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-win-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(&path).is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    path
}

fn until(c: &mut Remote, mut done: impl FnMut(&Node) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if done(&c.node) {
            return true;
        }
        let _ = c.step(Duration::from_millis(50));
    }
    done(&c.node)
}

#[test]
fn typed_lines_reach_the_shell_and_its_output_the_window() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-win-dir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (s2, d2) = (sock.clone(), dir.clone());
    // sh without -i: cooked, no readline, as rc is (rc's rcmain from the
    // published rustrc still wants plan9port's 9 for win, so not here)
    std::thread::spawn(move || {
        let _ = apex_tool_win::run(&s2, "main", &d2, &plain_sh());
    });
    let mut c = Remote::connect_as(&sock, "main", "test", AttachmentKind::Tool).unwrap();
    let name = format!("{}/", dir.display());
    let is_win = |n: &Node, w: WindowId| n.window_path(w) == name && n.window_label(w).as_deref() == Some("sh") && n.window_scratch(w);
    assert!(until(&mut c, |n| n.state.windows.keys().any(|w| is_win(n, *w))), "win's window");
    let w = c.node.state.windows.keys().copied().find(|w| is_win(&c.node, *w)).unwrap();
    let b = c.node.state.window(w).unwrap().body_buffer().unwrap();
    let text = |n: &Node| n.state.buffer(b).map(|x| x.text.to_string()).unwrap_or_default();
    // after the prompt, type a command at the end
    let _ = until(&mut c, |n| !text(n).is_empty());
    let end = c.node.state.buffer(b).unwrap().text.len();
    let version = c.node.state.buffer(b).unwrap().version;
    c.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: end, q1: end, text: "echo win-$((6*7))\n".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text(n).contains("win-42\n")), "output:\n{}", text(&c.node));
    // the typed line is still there, once, and the output follows it
    let t = text(&c.node);
    assert_eq!(t.matches("echo win-$((6*7))").count(), 1, "{t}");
    assert!(t.find("echo win-$((6*7))").unwrap() < t.find("win-42\n").unwrap(), "{t}");
    // a second command, once the prompt is back after the first's output
    assert!(until(&mut c, |n| text(n).ends_with("$ ")), "prompt:\n{}", text(&c.node));
    let end = c.node.state.buffer(b).unwrap().text.len();
    let version = c.node.state.buffer(b).unwrap().version;
    c.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: end, q1: end, text: "echo again\n".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text(n).ends_with("again\n") || text(n).contains("\nagain\n")), "output:\n{}", text(&c.node));
    // live while the shell runs: neither clean nor dirty, though edited
    assert!(c.node.window_live(w));
    assert!(c.node.state.buffer(b).unwrap().dirty());
    // the tools menu offers Interrupt and EOF here (exec is no word),
    // and Outline, as every window's does
    let verbs = apex_core::plumb::verbs_for(&c.node.state.meta.rules, &name, WinKind::File, Some(w), None);
    assert_eq!(verbs, vec!["Interrupt", "EOF", "Outline"]);
    // they are the window's, not its name's: another window of that name has none
    assert_eq!(apex_core::plumb::verbs_for(&c.node.state.meta.rules, &name, WinKind::File, None, None), vec!["Outline"]);
    // B2 over an old command line types it to the shell again, at the end
    assert!(until(&mut c, |n| text(n).ends_with("$ ")), "prompt:\n{}", text(&c.node));
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "echo win-$((6*7))".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text(n).matches("win-42\n").count() == 2), "output:\n{}", text(&c.node));
    let t = text(&c.node);
    assert!(t.rfind("win-42\n").unwrap() > t.find("again\n").unwrap(), "{t}");
    // the window renamed (the shell's awd on cd): the rules are the
    // window's, so the menu's verbs, and B2 sending, are there as before
    let renamed = format!("{}/elsewhere/", dir.display());
    c.propose(Proposal::SetPath { window: w, path: renamed.clone() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| n.window_path(w) == renamed));
    assert_eq!(apex_core::plumb::verbs_for(&c.node.state.meta.rules, &renamed, WinKind::File, Some(w), None), vec!["Interrupt", "EOF", "Outline"]);
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "echo win-$((6*7))".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text(n).matches("win-42\n").count() == 3), "output:\n{}", text(&c.node));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn del_typed_in_the_window_interrupts_what_the_shell_runs() {
    // fn-backspace: devdraw's Kdel, 0x7F, which acme types into the window
    // like any other key and plan9port's win takes for the interrupt
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-win-del-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (s2, d2) = (sock.clone(), dir.clone());
    std::thread::spawn(move || {
        let _ = apex_tool_win::run(&s2, "main", &d2, &plain_sh());
    });
    let mut c = Remote::connect_as(&sock, "main", "test", AttachmentKind::Tool).unwrap();
    let name = format!("{}/", dir.display());
    let is_win = |n: &Node, w: WindowId| n.window_path(w) == name && n.window_label(w).as_deref() == Some("sh") && n.window_scratch(w);
    assert!(until(&mut c, |n| n.state.windows.keys().any(|w| is_win(n, *w))), "win's window");
    let w = c.node.state.windows.keys().copied().find(|w| is_win(&c.node, *w)).unwrap();
    let b = c.node.state.window(w).unwrap().body_buffer().unwrap();
    let text = |n: &Node| n.state.buffer(b).map(|x| x.text.to_string()).unwrap_or_default();
    assert!(until(&mut c, |n| text(n).ends_with("$ ")), "prompt:\n{}", text(&c.node));
    let mut typing = |c: &mut Remote, s: &str| {
        let end = c.node.state.buffer(b).unwrap().text.len();
        let version = c.node.state.buffer(b).unwrap().version;
        c.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: end, q1: end, text: s.into() }, Duration::from_secs(5)).unwrap();
    };
    // something slow, which says so when it is interrupted
    typing(&mut c, "trap 'echo got-interrupted' INT; sleep 30; echo slept-through\n");
    // the sleep under way, and what the shell wrote before it (the
    // line's echo) seen here, so that DEL is typed at the end as it is
    // now and not as it was
    let begun = Instant::now();
    while begun.elapsed() < Duration::from_millis(500) {
        let _ = c.step(Duration::from_millis(50));
    }
    let started = Instant::now();
    // DEL, typed at the end, as the key is typed
    typing(&mut c, "\u{7f}");
    assert!(until(&mut c, |n| text(n).contains("got-interrupted")), "not interrupted:\n{}", text(&c.node));
    assert!(started.elapsed() < Duration::from_secs(10), "the sleep ran its course");
    // and the DEL itself does not stay in the window
    assert!(until(&mut c, |n| !text(n).contains('\u{7f}')), "DEL left in the text:\n{:?}", text(&c.node));
}

/// B3 on a name in win's window finds it in the shell's directory, as
/// it does anywhere else: the window is at that directory.
#[test]
fn b3_in_the_window_finds_files_in_its_directory() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-win-b3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hello.txt"), "hi\n").unwrap();
    let (s2, d2) = (sock.clone(), dir.clone());
    std::thread::spawn(move || {
        let _ = apex_tool_win::run(&s2, "main", &d2, &plain_sh());
    });
    let mut c = Remote::connect_as(&sock, "main", "test", AttachmentKind::Tool).unwrap();
    let name = format!("{}/", dir.display());
    assert!(until(&mut c, |n| n.state.windows.keys().any(|w| n.window_path(*w) == name)), "win's window");
    let w = c.node.state.windows.keys().copied().find(|w| c.node.window_path(*w) == name).unwrap();
    // as the UI asks: no directory of its own, the window's
    let trace = c.plumb_dry(ExecCtx::Window(w), "hello.txt", None, false, Duration::from_secs(5)).unwrap();
    assert!(trace.iter().any(|l| l.ends_with("would open hello.txt")), "{trace:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// rc's `rcmain` in apex marks each prompt and command (OSC 133): win
/// takes the marks out of the text and makes its outline of them, each
/// command at its prompt, with how it went. sh stands in for rc here:
/// its prompt carries the prompt's mark and a command prints its own.
#[test]
fn the_shells_prompt_marks_are_the_windows_outline() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-win-outline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (s2, d2) = (sock.clone(), dir.clone());
    std::thread::spawn(move || {
        let _ = apex_tool_win::run(&s2, "main", &d2, &plain_sh());
    });
    let mut c = Remote::connect_as(&sock, "main", "test", AttachmentKind::Tool).unwrap();
    let name = format!("{}/", dir.display());
    let is_win = |n: &Node, w: WindowId| n.window_path(w) == name && n.window_scratch(w);
    assert!(until(&mut c, |n| n.state.windows.keys().any(|w| is_win(n, *w))), "win's window");
    let w = c.node.state.windows.keys().copied().find(|w| is_win(&c.node, *w)).unwrap();
    let b = c.node.state.window(w).unwrap().body_buffer().unwrap();
    let text = |n: &Node| n.state.buffer(b).map(|x| x.text.to_string()).unwrap_or_default();
    let mut type_ = |c: &mut Remote, line: &str| {
        let end = c.node.state.buffer(b).unwrap().text.len();
        let version = c.node.state.buffer(b).unwrap().version;
        c.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: end, q1: end, text: format!("{line}\n") }, Duration::from_secs(5)).unwrap();
    };
    assert!(until(&mut c, |n| text(n).ends_with("$ ")), "prompt:\n{}", text(&c.node));
    // a prompt marked, as rcmain's `fn prompt` marks it
    type_(&mut c, r"PS1=$(printf '\033]133;A\007')'% '");
    assert!(until(&mut c, |n| text(n).ends_with("% ")), "marked prompt:\n{}", text(&c.node));
    let prompt = c.node.state.buffer(b).unwrap().text.len() - 2;
    // a command saying itself and its end, as rcmain's `fn preexec` and
    // the next `fn prompt` do; its semicolons are its own
    type_(&mut c, r"printf '\033]133;C;cmdline=make;  ls\007'; echo built; printf '\033]133;D;2\007'");
    let outline = |n: &Node| n.state.buffer(b).ok().and_then(|x| x.outline.clone()).map(|o| o.items).unwrap_or_default();
    assert!(until(&mut c, |n| outline(n).first().is_some_and(|i| i.ended.is_some())), "outline: {:?}\n{}", outline(&c.node), text(&c.node));
    let items = outline(&c.node);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].title, "make; ls");
    assert_eq!(items[0].exit, Some(2));
    assert!(items[0].started.is_some() && items[0].ended >= items[0].started);
    // at its prompt, which reads as it did: the marks are not in the text
    assert_eq!(items[0].at, prompt, "{:?}", text(&c.node));
    let t = text(&c.node);
    assert!(!t.contains('\u{1b}') && t.contains("built\n"), "{t:?}");
    // its output, as copied: `$` and the command, then what it printed
    let run = apex_server::outline::run_of(&c.node, w, 0).unwrap();
    let apex_server::outline::RunOutput::Text(out) = run.output else { panic!("{run:?}") };
    assert_eq!(apex_server::outline::transcript(&run.cmd, &out), "$ make; ls\nbuilt\n");
    // said as a terminal's command is: failed, and how
    let entries = apex_server::outline::of(&c.node, w, items[0].ended.unwrap()).unwrap();
    assert!(entries[0].failed);
    assert!(entries[0].detail.as_deref().is_some_and(|d| d.contains("exit 2")), "{:?}", entries[0].detail);
    let _ = std::fs::remove_dir_all(&dir);
}
