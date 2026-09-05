//! The attach protocol end to end: a daemon on a thread, clients over a
//! Unix socket.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::ClientMsg;
use apex_server::remote::Remote;

fn daemon() -> PathBuf {
    let path = std::env::temp_dir().join(format!("apex-socket-test-{}-{}.sock", std::process::id(), rand_suffix()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run(&p, "main").unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

fn rand_suffix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as u64
}

/// Pump messages until `done` or the deadline.
fn wait(r: &mut Remote, mut done: impl FnMut(&Remote) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done(r) {
            return true;
        }
        match r.step(Duration::from_millis(20)) {
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    done(r)
}

fn body(r: &Remote, w: WindowId) -> String {
    let b = r.node.state.window(w).unwrap().body_buffer().unwrap();
    r.node.state.buffer(b).unwrap().text.to_string()
}

#[test]
fn attach_edit_ack_and_reattach() {
    let sock = daemon();
    let mut c = Remote::connect(&sock, "main", "c1").unwrap();
    assert_eq!(c.attachment(), AttachmentId(1));
    // the first client sets the session up: it leads Layout
    let col = c.node.init_session(&mut c.log).unwrap();
    let w = c.node.new_window(&mut c.log, col, "scratch", "hello\n").unwrap();
    let v = ViewId::Body(w);
    c.node.select(&mut c.log, v, 5, 5).unwrap();
    c.node.insert(&mut c.log, v, " world").unwrap();
    c.flush();
    let b = c.node.view_buffer(v).unwrap();
    let want = c.log.last_seq(Shard::Buffer(b));
    assert!(wait(&mut c, |r| r.acked(Shard::Buffer(b)) == want), "ack for {want}");
    // the metalog came back: our leases are recorded there too
    assert!(wait(&mut c, |r| r.node.state.meta.leases.get(&Shard::Buffer(b)).map(|l| l.holder) == Some(AttachmentId(1))));

    // a second client attaches and sees the same buffers and windows
    let c2 = Remote::connect(&sock, "main", "c2").unwrap();
    assert_eq!(c2.attachment(), AttachmentId(2));
    assert_eq!(body(&c2, w), "hello world\n");
    assert_eq!(c2.node.state.windows.keys().collect::<Vec<_>>(), c.node.state.windows.keys().collect::<Vec<_>>());
    // it holds the leases now; the first client is fenced at the server
    assert_eq!(c2.log.lease(Shard::Buffer(b)).unwrap().holder, AttachmentId(2));
    let mut c2 = c2;
    c2.node.select(&mut c2.log, v, 0, 0).unwrap();
    c2.node.insert(&mut c2.log, v, "> ").unwrap();
    c2.flush();
    let want = c2.log.last_seq(Shard::Buffer(b));
    assert!(wait(&mut c2, |r| r.acked(Shard::Buffer(b)) == want));
    assert_eq!(body(&c2, w), "> hello world\n");
}

#[test]
fn execs_and_terminals_over_the_socket() {
    let sock = daemon();
    let mut c = Remote::connect(&sock, "main", "c1").unwrap();
    let col = c.node.init_session(&mut c.log).unwrap();
    let dir = std::env::temp_dir().join(format!("apex-socket-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();

    // open a file: a proposal comes back and the client makes the window
    c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: path.to_string_lossy().to_string() });
    assert!(wait(&mut c, |r| r.node.state.windows.len() == 1));
    let w = *c.node.state.windows.keys().next().unwrap();
    assert_eq!(body(&c, w), "one\ntwo\n");

    // edit and Put: the server writes the file and proposes Clean
    let v = ViewId::Body(w);
    c.node.select(&mut c.log, v, 0, 0).unwrap();
    c.node.insert(&mut c.log, v, "zero\n").unwrap();
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Put").unwrap();
    c.flush();
    let b = c.node.view_buffer(v).unwrap();
    assert!(wait(&mut c, |r| !r.node.state.buffer(b).unwrap().dirty()));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "zero\none\ntwo\n");

    // a pipe through the shell
    c.node.select(&mut c.log, v, 0, 13).unwrap();
    c.node.exec(&mut c.log, ExecCtx::Window(w), "|sort").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| body(r, w) == "one\ntwo\nzero\n"), "body: {}", body(&c, w));

    // a terminal: pinned shard led by the server, streamed to us
    c.node.exec(&mut c.log, ExecCtx::Top, "Newterm").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| r.node.state.windows.values().any(|w| matches!(w.body, Body::Term(_)))));
    let t = c.node.state.windows.values().find_map(|w| match w.body {
        Body::Term(t) => Some(t),
        _ => None,
    }).unwrap();
    for ch in "echo apex-sock-$((6*7))\r".chars() {
        c.send(&ClientMsg::TermKey { term: t, key: apex_server::TermKey { key: ch.to_string(), text: Some(ch.to_string()), shift: false, control: false, alt: false } });
    }
    let grid = |r: &Remote| {
        r.node.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    };
    assert!(wait(&mut c, |r| grid(r).contains("apex-sock-42")), "grid:\n{}\nterms: {:?} applied: {:?} leases: {:?}", grid(&c), c.node.state.terms.keys().collect::<Vec<_>>(), c.node.state.applied, c.log.lease(Shard::Term(t)));
    let _ = std::fs::remove_dir_all(&dir);
}
