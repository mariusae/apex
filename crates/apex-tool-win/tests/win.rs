//! apex tool win against a headless daemon: the shell's output lands at
//! the output point, a typed line goes to the shell, an interrupt
//! drops the typing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::remote::Remote;
use apex_server::Proposal;

fn daemon() -> PathBuf {
    let path = std::env::temp_dir().join(format!("apex-win-{}.sock", std::process::id()));
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
        let _ = apex_tool_win::run(&s2, "main", &d2, &["/bin/sh".to_string()]);
    });
    let mut c = Remote::connect_as(&sock, "main", "test", AttachmentKind::Tool).unwrap();
    let name = format!("{}/-sh", dir.display());
    assert!(until(&mut c, |n| n.state.windows.keys().any(|w| n.window_name(*w) == name)), "win's window");
    let w = c.node.state.windows.keys().copied().find(|w| c.node.window_name(*w) == name).unwrap();
    let b = c.node.state.window(w).unwrap().body_buffer().unwrap();
    let text = |n: &Node| n.state.buffer(b).map(|x| x.text.to_string()).unwrap_or_default();
    // after the prompt, type a command at the end
    let _ = until(&mut c, |n| !text(n).is_empty());
    let end = c.node.state.buffer(b).unwrap().text.len();
    let version = c.node.state.buffer(b).unwrap().version;
    c.propose(Proposal::ReplaceRange { dir: None, buffer: b, version, q0: end, q1: end, text: "echo win-$((6*7))\n".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text(n).contains("win-42\n")), "output:\n{}", text(&c.node));
    // the typed line is still there, once, and the output follows it
    let t = text(&c.node);
    assert_eq!(t.matches("echo win-$((6*7))").count(), 1, "{t}");
    assert!(t.find("echo win-$((6*7))").unwrap() < t.find("win-42\n").unwrap(), "{t}");
    // a second command, once the prompt is back after the first's output
    assert!(until(&mut c, |n| text(n).ends_with("$ ")), "prompt:\n{}", text(&c.node));
    let end = c.node.state.buffer(b).unwrap().text.len();
    let version = c.node.state.buffer(b).unwrap().version;
    c.propose(Proposal::ReplaceRange { dir: None, buffer: b, version, q0: end, q1: end, text: "echo again\n".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text(n).ends_with("again\n") || text(n).contains("\nagain\n")), "output:\n{}", text(&c.node));
    // the tools menu offers Interrupt and EOF here
    let verbs = apex_core::plumb::verbs_for(&c.node.state.meta.rules, &name, WinKind::File);
    assert_eq!(verbs, vec!["Interrupt", "EOF"]);
    let _ = std::fs::remove_dir_all(&dir);
}
