//! The server performing execs through a leader, in-process.

use std::time::{Duration, Instant};

use apex_core::state::ExecStatus;
use apex_core::*;
use apex_server::{perform, Server, ServerEvent};
use futures::channel::mpsc::UnboundedReceiver;

fn session() -> (Log, Node, ColumnId, Server, UnboundedReceiver<ServerEvent>) {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "test");
    let mut node = Node::new(a);
    node.catch_up(&log).unwrap();
    let col = node.init_session(&mut log).unwrap();
    let (server, rx) = Server::new(&log);
    (log, node, col, server, rx)
}

/// Drain server events until `done` holds or the deadline passes.
fn pump_until(log: &mut Log, node: &mut Node, server: &mut Server, rx: &mut UnboundedReceiver<ServerEvent>, mut done: impl FnMut(&Node) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done(node) {
            return true;
        }
        match rx.try_recv() {
            Ok(ev) => {
                let props = server.pump(log, node, ev);
                perform(node, log, props);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
        node.catch_up(log).unwrap();
    }
    done(node)
}

fn body_text(node: &Node, w: WindowId) -> String {
    let b = node.state.window(w).unwrap().body_buffer().unwrap();
    node.state.buffer(b).unwrap().text.to_string()
}

fn errors_text(node: &Node) -> String {
    node.state.buffers.values().find(|b| b.name.ends_with("+Errors")).map(|b| b.text.to_string()).unwrap_or_default()
}

fn open(server: &Server, log: &mut Log, node: &mut Node, col: ColumnId, dir: &std::path::Path, name: &str) -> WindowId {
    let p = server.open_file(col, None, dir, name, None).unwrap();
    perform(node, log, vec![p]).unwrap()
}

fn poll(server: &mut Server, log: &mut Log, node: &mut Node) -> usize {
    let props = server.poll_execs(log, node);
    let n = props.len();
    perform(node, log, props);
    n
}

fn exec_status(node: &Node, w: WindowId) -> ExecStatus {
    node.state.window(w).unwrap().execs.values().last().unwrap().status.clone()
}

#[test]
fn files_put_and_get() {
    let (mut log, mut node, col, mut server, _rx) = session();
    let dir = std::env::temp_dir().join(format!("apex-server-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();

    let w = open(&server, &mut log, &mut node, col, &dir, "f.txt");
    assert_eq!(body_text(&node, w), "one\ntwo\n");
    assert_eq!(node.window_name(w), path.to_string_lossy());
    // opening it again gives the same window
    assert_eq!(open(&server, &mut log, &mut node, col, &dir, "f.txt"), w);

    // edit, Put, check the file and the clean flag
    let v = ViewId::Body(w);
    node.select(&mut log, v, 0, 0).unwrap();
    node.insert(&mut log, v, "zero\n").unwrap();
    assert!(node.state.buffer(node.view_buffer(v).unwrap()).unwrap().dirty());
    assert!(matches!(node.exec(&mut log, ExecCtx::Window(w), "Put").unwrap(), Executed::Deferred(_)));
    assert_eq!(poll(&mut server, &mut log, &mut node), 2); // Clean + Status
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "zero\none\ntwo\n");
    assert!(!node.state.buffer(node.view_buffer(v).unwrap()).unwrap().dirty());
    assert_eq!(exec_status(&node, w), ExecStatus::Done);

    // change the file outside and Get it back
    std::fs::write(&path, "changed\n").unwrap();
    node.exec(&mut log, ExecCtx::Window(w), "Get").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert_eq!(body_text(&node, w), "changed\n");
    assert!(!node.state.buffer(node.view_buffer(v).unwrap()).unwrap().dirty());

    // a directory opens as a listing
    let d = open(&server, &mut log, &mut node, col, &dir, ".");
    assert!(body_text(&node, d).contains("f.txt\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shell_commands_and_pipes() {
    let (mut log, mut node, col, mut server, mut rx) = session();
    let w = node.new_window(&mut log, col, "scratch", "b\na\nc\n").unwrap();
    // an unknown word runs in the shell; output goes to +Errors
    node.exec(&mut log, ExecCtx::Window(w), "echo hello-apex").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| errors_text(n).contains("hello-apex")));
    assert_eq!(exec_status(&node, w), ExecStatus::Done);
    // |sort replaces the selection with sorted input
    let v = ViewId::Body(w);
    node.select(&mut log, v, 0, 6).unwrap();
    node.exec(&mut log, ExecCtx::Window(w), "|sort").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| body_text(n, w) == "a\nb\nc\n"));
    // <cmd inserts output at the selection
    node.select(&mut log, v, 0, 0).unwrap();
    node.exec(&mut log, ExecCtx::Window(w), "<echo top").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| body_text(n, w) == "top\na\nb\nc\n"));
    // a follower replaying the log agrees with everything that happened
    let mut f = Node::new(AttachmentId(77));
    f.catch_up(&log).unwrap();
    assert_eq!(f.state.hash(), node.state.hash());
}

#[test]
fn newterm_runs_a_shell() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let w = node
        .state
        .windows
        .values()
        .find(|w| matches!(w.body, Body::Term(_)))
        .map(|w| w.id)
        .expect("terminal window");
    let Body::Term(t) = node.state.window(w).unwrap().body else { unreachable!() };
    assert!(node.state.terms.contains_key(&t));
    // the server sees the window (a client does this after every command)
    server.close_orphan_terms(&mut log, &node);
    assert!(node.state.terms.contains_key(&t));
    // type a command into the shell and see its output in the grid
    for c in "echo apex-term-$((6*7))\r".chars() {
        server.term_key(t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
    }
    let grid_text = |n: &Node| {
        n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    };
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("apex-term-42")), "grid:\n{}", grid_text(&node));
    // Del closes the window; the server drops the terminal
    node.exec(&mut log, ExecCtx::Window(w), "Del").unwrap();
    server.close_orphan_terms(&mut log, &node);
    assert!(!node.state.windows.contains_key(&w));
    node.catch_up(&log).unwrap();
    assert!(!node.state.terms.contains_key(&t));
}

#[test]
fn kill_ends_a_running_command() {
    let (mut log, mut node, col, mut server, mut rx) = session();
    let w = node.new_window(&mut log, col, "scratch", "").unwrap();
    let t0 = Instant::now();
    node.exec(&mut log, ExecCtx::Window(w), "sleep 30").unwrap();
    poll(&mut server, &mut log, &mut node);
    std::thread::sleep(Duration::from_millis(200));
    node.exec(&mut log, ExecCtx::Window(w), "Kill sleep").unwrap();
    poll(&mut server, &mut log, &mut node);
    // the sleep's exec completes long before 30 s
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| {
        n.state.window(w).unwrap().execs.values().next().map(|e| e.status != ExecStatus::Pending).unwrap_or(false)
    }));
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[test]
fn completion_extends_a_path_or_lists_candidates() {
    let (mut log, mut node, col, server, _rx) = session();
    let dir = std::env::temp_dir().join(format!("apex-complete-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("subdir")).unwrap();
    std::fs::write(dir.join("alpha.txt"), "").unwrap();
    std::fs::write(dir.join("alpine.txt"), "").unwrap();
    let w = node.new_window(&mut log, col, "scratch", "al").unwrap();
    let v = ViewId::Body(w);
    node.select(&mut log, v, 2, 2).unwrap();
    // "al" → "alp" (common extension), then nothing more: the candidates are listed
    let p = server.complete(v, 2, &dir, "al");
    assert!(matches!(p, apex_server::Proposal::Complete { text: ref t, .. } if t == "p"), "{p:?}");
    perform(&mut node, &mut log, vec![p]);
    assert_eq!(body_text(&node, w), "alp");
    let p = server.complete(v, 3, &dir, "alp");
    assert!(matches!(p, apex_server::Proposal::Errors { text: ref t, .. } if t.contains("alpha.txt") && t.contains("alpine.txt")), "{p:?}");
    // a unique directory completes with a slash, a unique file with a space
    assert!(matches!(server.complete(v, 3, &dir, "su"), apex_server::Proposal::Complete { text: ref t, .. } if t == "bdir/"));
    assert!(matches!(server.complete(v, 3, &dir, "alph"), apex_server::Proposal::Complete { text: ref t, .. } if t == "a.txt "));
    assert!(matches!(server.complete(v, 3, &dir, "zz"), apex_server::Proposal::Errors { text: ref t, .. } if t.contains("no matches")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn running_commands_are_named_in_the_top_row() {
    let (mut log, mut node, col, mut server, mut rx) = session();
    let top = node.state.layout.top.unwrap();
    let top_text = |n: &Node| n.state.buffer(top).unwrap().text.to_string();
    let w = node.new_window(&mut log, col, "scratch", "").unwrap();
    node.exec(&mut log, ExecCtx::Window(w), "sleep 30").unwrap();
    poll(&mut server, &mut log, &mut node);
    // acme's waitthread: the name, without directory, at the front
    assert!(top_text(&node).starts_with("sleep "), "{:?}", top_text(&node));
    node.exec(&mut log, ExecCtx::Window(w), "false").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert!(top_text(&node).starts_with("false sleep "), "{:?}", top_text(&node));
    // false exits 1: its name leaves and the exit is reported
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| !top_text(n).contains("false ")));
    assert!(errors_text(&node).contains("false: exit 1"), "{:?}", errors_text(&node));
    node.exec(&mut log, ExecCtx::Window(w), "Kill sleep").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| !top_text(n).contains("sleep ")));
    // under sh the sleep itself dies of the signal; under rc, rc reports
    // the death and exits 1, as plan9port's does
    assert!(errors_text(&node).contains("sleep: exit "), "{:?}", errors_text(&node));
    assert!(top_text(&node).starts_with("Newcol"), "{:?}", top_text(&node));
}

#[test]
fn commands_run_in_rc_with_acmes_environment() {
    let (mut log, mut node, col, mut server, mut rx) = session();
    let w = node.new_window(&mut log, col, "/tmp/some/file.txt", "").unwrap();
    // acme's runproc: $winid, $% and $samfile name the window and its file
    node.exec(&mut log, ExecCtx::Window(w), "echo id=$winid file=$% same=$samfile").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| errors_text(n).contains("same=/tmp/some/file.txt")), "{}", errors_text(&node));
    let e = errors_text(&node);
    assert!(e.contains(&format!("id={}", w.0)) && e.contains("file=/tmp/some/file.txt"), "{e}");
    // and the shell is rc: its syntax, not sh's
    let shell = apex_server::command_shell();
    if shell.ends_with("rc") {
        node.exec(&mut log, ExecCtx::Window(w), "for(i in a b) echo rc-$i").unwrap();
        poll(&mut server, &mut log, &mut node);
        assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| errors_text(n).contains("rc-b")), "{}", errors_text(&node));
    } else {
        eprintln!("no rc built (target/rc-host/bin/rc): commands fell back to {shell}");
    }
}
