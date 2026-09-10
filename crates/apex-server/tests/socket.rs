//! The attach protocol end to end: a daemon on a thread, clients over a
//! Unix socket.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::ClientMsg;
use apex_server::remote::Remote;

fn daemon() -> PathBuf {
    // pid plus a counter: the clock is too coarse to tell parallel tests apart
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-socket-test-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
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
    // the daemon laid the session out; the UI leads Layout from here
    let col = c.node.state.layout.cols[0].id;
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
    let col = c.node.state.layout.cols[0].id;
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

#[test]
fn a_tool_works_on_a_headless_session_and_a_ui_takes_over() {
    use apex_server::Proposal;
    let sock = daemon();
    // no UI: the daemon leads; a tool attaches and proposes
    let mut tool = Remote::connect_as(&sock, "main", "tool", AttachmentKind::Tool).unwrap();
    let ten = Duration::from_secs(10);
    // a second column, through the leader (the daemon)
    tool.propose(Proposal::Exec { ctx: ExecCtx::Top, text: "Newcol".into() }, ten).unwrap();
    assert!(wait(&mut tool, |r| r.node.state.layout.cols.len() == 2));
    let col = tool.node.state.layout.cols[1].id;
    let dir = std::env::temp_dir().join(format!("apex-headless-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("h.txt");
    std::fs::write(&path, "b\na\n").unwrap();
    tool.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: path.to_string_lossy().to_string() });
    // (Newcol made an empty window too, as acme's does)
    let named = |r: &Remote| r.node.state.windows.keys().copied().find(|w| r.node.window_name(*w).ends_with("h.txt"));
    assert!(wait(&mut tool, |r| named(r).is_some()));
    let w = named(&tool).unwrap();
    // an Edit program, then Put, both by proposal
    let made = tool.propose(Proposal::Edit { window: w, program: ",x/a/ c/A/".into() }, ten).unwrap();
    assert_eq!(made, Some(w));
    assert!(wait(&mut tool, |r| body(r, w) == "b\nA\n"), "body: {:?}", body(&tool, w));
    tool.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Put".into() }, ten).unwrap();
    assert!(wait(&mut tool, |_| std::fs::read_to_string(&path).unwrap() == "b\nA\n"));
    // now a UI attaches: it takes the leases and sees the tool's work
    let mut ui = Remote::connect(&sock, "main", "ui").unwrap();
    assert_eq!(body(&ui, w), "b\nA\n");
    assert_eq!(ui.log.lease(Shard::Buffer(ui.node.view_buffer(ViewId::Body(w)).unwrap())).unwrap().holder, ui.attachment());
    // the tool's next proposal goes through the UI, which applies it and
    // answers; the tool sees the selection stream back
    let v = ViewId::Body(w);
    let id = tool.link.propose(Proposal::Select { view: v, q0: 0, q1: 1 });
    assert!(wait(&mut ui, |r| r.node.selection(v).ok() == Some((0, 1))));
    assert!(wait(&mut tool, |r| r.link.applied.contains_key(&id)));
    // a Select names no window: moving dot is not a look into it
    assert_eq!(tool.link.applied.remove(&id).unwrap(), Ok(None));
    assert!(wait(&mut tool, |r| r.node.selection(v).ok() == Some((0, 1))));
    // the UI goes away: leases return to the daemon, which leads again
    drop(ui);
    assert!(wait(&mut tool, |r| r.node.state.meta.leases.get(&Shard::Layout).map(|l| l.holder) == Some(SERVER)));
    tool.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() }, ten).unwrap();
    assert!(wait(&mut tool, |r| named(r).is_none()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sessions_are_listed_and_made() {
    let sock = daemon();
    let mut c = Remote::connect_as(&sock, "main", "t", AttachmentKind::Tool).unwrap();
    c.send(&ClientMsg::ListSessions);
    assert!(wait(&mut c, |r| r.link.sessions.as_ref().is_some_and(|v| v.iter().map(|s| s.label.as_str()).collect::<Vec<_>>() == ["main"])));
    c.send(&ClientMsg::NewSession { name: "two".into() });
    assert!(wait(&mut c, |r| r.link.sessions.as_ref().map(|s| s.len()) == Some(2)));
    let mut two = Remote::connect(&sock, "two", "ui").unwrap();
    let col = two.node.state.layout.cols[0].id;
    two.node.new_window(&mut two.log, col, "only-in-two", "").unwrap();
    two.flush();
    assert!(wait(&mut two, |r| r.acked(Shard::Layout) >= 1));
    // "main" is untouched
    assert!(wait(&mut c, |r| r.node.state.windows.is_empty()));
}

#[test]
fn the_watcher_reloads_clean_buffers_and_flags_dirty_ones() {
    let sock = daemon();
    let mut c = Remote::connect(&sock, "main", "ui").unwrap();
    let col = c.node.state.layout.cols[0].id;
    let dir = std::env::temp_dir().join(format!("apex-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("w.txt");
    std::fs::write(&path, "one\n").unwrap();
    c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: path.to_string_lossy().to_string() });
    assert!(wait(&mut c, |r| r.node.state.windows.len() == 1));
    let w = *c.node.state.windows.keys().next().unwrap();
    let b = c.node.view_buffer(ViewId::Body(w)).unwrap();
    // let the watch settle before the first outside change
    std::thread::sleep(Duration::from_millis(300));

    // clean buffer, disk changes: the buffer follows. The watch on a new
    // directory takes the OS a moment to start, longer under load, so a
    // change it missed is made again until it is seen.
    let deadline = Instant::now() + Duration::from_secs(15);
    while body(&c, w) != "two\n" && Instant::now() < deadline {
        std::fs::write(&path, "two\n").unwrap();
        let until = Instant::now() + Duration::from_millis(700);
        while body(&c, w) != "two\n" && Instant::now() < until {
            let _ = c.step(Duration::from_millis(50));
        }
    }
    assert_eq!(body(&c, w), "two\n");
    assert!(!c.node.state.buffer(b).unwrap().dirty());

    // dirty buffer, disk changes: stale, and Put refuses once
    let v = ViewId::Body(w);
    c.node.select(&mut c.log, v, 0, 0).unwrap();
    c.node.insert(&mut c.log, v, "mine ").unwrap();
    c.flush();
    std::fs::write(&path, "three\n").unwrap();
    assert!(wait(&mut c, |r| r.node.state.buffer(b).unwrap().stale));
    assert_eq!(body(&c, w), "mine two\n");
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Put").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| r.node.state.buffers.values().any(|b| b.name.ends_with("+Errors") && b.text.to_string().contains("modified since last read"))));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "three\n");
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Put").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| !r.node.state.buffer(b).unwrap().dirty()));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine two\n");
    assert!(!c.node.state.buffer(b).unwrap().stale);
    // our own write did not bounce back as a change
    std::thread::sleep(Duration::from_millis(500));
    let _ = c.step(Duration::from_millis(100));
    assert_eq!(body(&c, w), "mine two\n");
    assert!(!c.node.state.buffer(b).unwrap().stale);
    // Get on a stale buffer reloads it
    std::fs::write(&path, "four\n").unwrap();
    c.node.select(&mut c.log, v, 0, 0).unwrap();
    c.node.insert(&mut c.log, v, "x").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| r.node.state.buffer(b).unwrap().stale));
    // acme's get: a dirty window is asked once ("modified"), then reloads
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Get").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| r.node.state.buffers.values().any(|b| b.name.ends_with("+Errors") && b.text.to_string().contains("w.txt modified"))));
    assert_ne!(body(&c, w), "four\n");
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Get").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| body(r, w) == "four\n" && !r.node.state.buffer(b).unwrap().stale));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn selection_and_scroll_position_survive_reattach() {
    let sock = daemon();
    let mut c = Remote::connect(&sock, "main", "first").unwrap();
    let col = c.node.state.layout.cols[0].id;
    let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let w = c.node.new_window(&mut c.log, col, "long", &text).unwrap();
    let v = ViewId::Body(w);
    c.node.select(&mut c.log, v, 700, 712).unwrap();
    let origin = c.node.state.buffer(c.node.view_buffer(v).unwrap()).unwrap().text.line_start(100);
    c.node.set_origin(&mut c.log, v, origin).unwrap();
    // what the UI does on every frame
    c.flush();
    let b = c.node.view_buffer(v).unwrap();
    let want = c.log.last_seq(Shard::Buffer(b));
    assert!(wait(&mut c, |r| r.acked(Shard::Buffer(b)) == want));
    drop(c);

    let again = Remote::connect(&sock, "main", "second").unwrap();
    assert_eq!(again.node.selection(v).unwrap(), (700, 712));
    let view = again.node.state.buffer(b).unwrap().views[&v];
    assert_eq!(view.origin, origin);
    assert_eq!(again.node.state.layout.cols[0].wins.len(), 1);
}

#[test]
fn the_daemon_says_its_build_first_and_stops_when_told() {
    let sock = daemon();
    // the first frame on any connection is the build id
    let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    apex_server::proto::write_frame(&mut s, &apex_server::proto::ClientMsg::ListSessions).unwrap();
    let mut r = std::io::BufReader::new(s);
    let first = apex_server::proto::read_frame::<_, apex_server::proto::ServerMsg>(&mut r).unwrap().unwrap();
    match first {
        apex_server::proto::ServerMsg::Build { protocol, id } => {
            assert_eq!(protocol, apex_server::proto::PROTOCOL);
            assert_eq!(id, apex_server::BUILD_ID);
        }
        other => panic!("first frame: {other:?}"),
    }
    drop(r);
    // stop: the daemon goes, and takes its socket with it
    apex_server::remote::stop(&sock).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(&sock).is_ok() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(std::os::unix::net::UnixStream::connect(&sock).is_err(), "daemon still answers");
}

/// acme's B3 expansion happens on the server: a click anywhere in
/// `sub/f.txt:2:1` opens sub/f.txt at line 2 (the name before the colon
/// is a file here), and a click on a word that names no file looks.
#[test]
fn b3_expands_as_acme_does_where_the_files_are() {
    let dir = std::env::temp_dir().join(format!("apex-expand-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/f.txt"), "l1\nl2\nl3\n").unwrap();
    let notes = dir.join("notes");
    std::fs::write(&notes, "see sub/f.txt:2:1 here\nplain words\n").unwrap();
    let sock = daemon();
    let mut ui = Remote::connect(&sock, "main", "ui").unwrap();
    let col = ui.node.state.layout.cols[0].id;
    ui.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: notes.display().to_string() });
    let find = |r: &Remote, suffix: &str| r.node.state.windows.keys().copied().find(|w| r.node.window_name(*w).ends_with(suffix));
    assert!(wait(&mut ui, |r| find(r, "/notes").is_some()));
    let w = find(&ui, "/notes").unwrap();
    let b = ui.node.state.window(w).unwrap().body_buffer().unwrap();
    // the target open already (a bare Remote does not open files on a
    // Goto; the app does), its selection at the top
    ui.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: dir.join("sub/f.txt").display().to_string() });
    assert!(wait(&mut ui, |r| find(r, "sub/f.txt").is_some()));
    let f = find(&ui, "sub/f.txt").unwrap();
    // a click on "sub", on "f.txt", on "2": the file, at line 2
    for q in [5, 10, 14] {
        ui.propose(apex_server::Proposal::Select { view: ViewId::Body(f), q0: 0, q1: 0 }, Duration::from_secs(5)).unwrap();
        ui.send(&ClientMsg::Plumb { ctx: ExecCtx::Window(w), text: "x".into(), dir: None, edit_only: false, dry: false, at: Some(Span { buffer: b, q0: q, q1: q }), sel: None, alt: None, reverse: false, verb: None });
        assert!(wait(&mut ui, |r| r.node.selection(ViewId::Body(f)).ok().map(|s| s.0) == Some(3)), "at {q}: line 2, selection {:?}", ui.node.selection(ViewId::Body(f)));
    }
    // a word naming no file is looked for: the next "words" is selected
    ui.propose(apex_server::Proposal::Select { view: ViewId::Body(w), q0: 0, q1: 0 }, Duration::from_secs(5)).unwrap();
    ui.send(&ClientMsg::Plumb { ctx: ExecCtx::Window(w), text: "x".into(), dir: None, edit_only: false, dry: false, at: Some(Span { buffer: b, q0: 30, q1: 30 }), sel: None, alt: None, reverse: false, verb: None });
    assert!(wait(&mut ui, |r| r.node.selection(ViewId::Body(w)).ok() == Some((29, 34))), "looked: {:?}", ui.node.selection(ViewId::Body(w)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A directory window lists the directory anew when it changes, as a
/// clean file follows the disk; edited, it goes stale instead (the
/// orange handle, Get in the tag), and Get lists it again.
#[test]
fn directory_windows_refresh_or_go_stale() {
    let sock = daemon();
    let mut c = Remote::connect(&sock, "main", "ui").unwrap();
    let col = c.node.state.layout.cols[0].id;
    let dir = std::env::temp_dir().join(format!("apex-dirwatch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: dir.to_string_lossy().to_string() });
    assert!(wait(&mut c, |r| r.node.state.windows.len() == 1));
    let w = *c.node.state.windows.keys().next().unwrap();
    let b = c.node.view_buffer(ViewId::Body(w)).unwrap();
    assert_eq!(body(&c, w), "a.txt\n");
    std::thread::sleep(Duration::from_millis(300));
    // clean: a new entry appears on its own (the watch takes the OS a
    // moment to start, so the change is made again until it is seen)
    let deadline = Instant::now() + Duration::from_secs(15);
    while body(&c, w) != "a.txt\nb.txt\n" && Instant::now() < deadline {
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        let until = Instant::now() + Duration::from_millis(700);
        while body(&c, w) != "a.txt\nb.txt\n" && Instant::now() < until {
            let _ = c.step(Duration::from_millis(50));
        }
    }
    assert_eq!(body(&c, w), "a.txt\nb.txt\n");
    assert!(!c.node.state.buffer(b).unwrap().dirty());
    // dirty: the listing stays, flagged stale
    let v = ViewId::Body(w);
    c.node.select(&mut c.log, v, 0, 0).unwrap();
    c.node.insert(&mut c.log, v, "note ").unwrap();
    c.flush();
    std::fs::write(dir.join("c.txt"), "c\n").unwrap();
    assert!(wait(&mut c, |r| r.node.state.buffer(b).unwrap().stale));
    assert_eq!(body(&c, w), "note a.txt\nb.txt\n");
    // Get: listed anew, clean again
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Get").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| body(r, w) == "a.txt\nb.txt\nc.txt\n"), "{}", body(&c, w));
    assert!(!c.node.state.buffer(b).unwrap().stale && !c.node.state.buffer(b).unwrap().dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A place in another session: a Goto or Switch naming one is not
/// landed here but handed to the UI as a switch (`take_switches`),
/// with the window to land on; `apex B session.N:line` makes such a
/// Goto, and Back from there leads back across sessions.
#[test]
fn places_in_other_sessions_become_switches_for_the_ui() {
    let sock = daemon();
    let mut ui = Remote::connect(&sock, "main", "ui").unwrap();
    let here = ui.node.state.meta.id.clone();
    assert_eq!(here.len(), 36);
    // a second session, by its identity
    ui.send(&ClientMsg::NewSession { name: "side".into() });
    assert!(wait(&mut ui, |r| r.link.sessions.is_some()), "no session list came");
    let side = ui.link.sessions.take().unwrap().into_iter().find(|s| s.label == "side").expect("side listed").id;
    assert_ne!(side, here);
    // a tool proposes a Switch: the UI, leading, gets it as a switch
    let mut tool = Remote::connect_as(&sock, "main", "t", AttachmentKind::Tool).unwrap();
    // (proposed without waiting: the UI must step to apply it)
    tool.link.propose(apex_server::Proposal::Switch { session: side.clone(), window: Some(WindowId(7)) });
    assert!(wait(&mut ui, |r| !r.node.switches.is_empty()));
    let sw = ui.node.take_switches();
    assert_eq!(sw, vec![Loc { session: Some(side.clone()), name: "7".into(), pos: Pos::Keep }]);
    // a Goto with a session (apex B side.7:12): the same, with the line
    let b = format!("{}.7:12", &side[..8]);
    tool.send(&ClientMsg::Plumb { ctx: ExecCtx::Top, text: b, dir: None, edit_only: true, dry: false, at: None, sel: None, alt: None, reverse: false, verb: None });
    assert!(wait(&mut ui, |r| !r.node.switches.is_empty()));
    let sw = ui.node.take_switches();
    assert_eq!(sw, vec![Loc { session: Some(side[..8].to_string()), name: "7".into(), pos: Pos::Line(12) }]);
    // a Goto to this session by its own id lands here, as any Goto does
    // (each proposal waited for, both sides stepping: the UI applies)
    let settled = |ui: &mut Remote, tool: &mut Remote, pid: u64| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !tool.link.applied.contains_key(&pid) && Instant::now() < deadline {
            let _ = ui.step(Duration::from_millis(10));
            let _ = tool.step(Duration::from_millis(10));
        }
        tool.link.applied.remove(&pid).expect("answered").expect("applied")
    };
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/here", "").unwrap();
    let w2 = ui.node.new_window(&mut ui.log, col, "/tmp/there", "").unwrap();
    ui.flush();
    let pid = tool.link.propose(apex_server::Proposal::Goto { loc: Loc { session: Some(here.clone()), name: w.0.to_string(), pos: Pos::Keep } });
    settled(&mut ui, &mut tool, pid);
    assert_eq!(ui.node.seltext, Some(ViewId::Body(w)));
    assert!(ui.node.switches.is_empty());
    // the place left by a jump is on the stack with its session: Back
    // from elsewhere would come here
    let pid = tool.link.propose(apex_server::Proposal::Goto { loc: Loc { session: None, name: w2.0.to_string(), pos: Pos::Keep } });
    settled(&mut ui, &mut tool, pid);
    assert_eq!(ui.node.seltext, Some(ViewId::Body(w2)));
    let back = ui.node.state.layout.nav_back.last().cloned();
    assert!(back.as_ref().is_some_and(|l| l.session.as_deref() == Some(here.as_str()) && l.name == "/tmp/here"), "{:?}", ui.node.state.layout.nav_back);
}
