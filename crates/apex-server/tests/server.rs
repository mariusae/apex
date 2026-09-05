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
                let props = server.pump(log, ev);
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
    node.state.buffers.values().find(|b| b.name == "+Errors").map(|b| b.text.to_string()).unwrap_or_default()
}

fn open(server: &Server, log: &mut Log, node: &mut Node, col: ColumnId, dir: &std::path::Path, name: &str) -> WindowId {
    let p = server.open_file(col, dir, name, None).unwrap();
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
