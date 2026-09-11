//! The server performing execs through a leader, in-process.

use std::time::{Duration, Instant};

use apex_core::state::ExecStatus;
use apex_core::*;
use apex_server::{perform, Proposal, Server, ServerEvent};
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

/// What a client does after a command: perform what the server was
/// handed, then let it see the windows (a terminal's shell starts once
/// its window is there).
fn poll(server: &mut Server, log: &mut Log, node: &mut Node) -> usize {
    let props = server.poll_execs(log, node);
    let n = props.len();
    perform(node, log, props);
    server.close_orphan_terms(log, node);
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
fn get_rule_overrides_filesystem_get_and_stays_scoped() {
    let (mut log, mut node, col, mut server, _rx) = session();
    let dir = std::env::temp_dir().join(format!("apex-get-rule-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    std::fs::write(&path, "disk\n").unwrap();
    let w_file = open(&server, &mut log, &mut node, col, &dir, "f.txt");
    let smartlog = format!("{}/+smartlog", dir.display());
    let w_live = node.new_window(&mut log, col, &smartlog, "generated\n").unwrap();
    let rule = PlumbRule {
        verb: "Get".into(),
        text: None,
        file: Some(r"\+smartlog$".into()),
        kind: Some(WinKind::File),
        isfile: None,
        isdir: None,
        action: RuleAction::Tool("smartlog".into()),
        win: None, to: None,
    };
    let (_, e) = log.install_rule(SERVER, 0, rule);
    node.state.apply(Shard::Meta, &e).unwrap();

    assert_eq!(apex_core::plumb::verbs_for(&node.state.meta.rules, &node.window_name(w_live), node.window_kind(w_live), Some(w_live)), vec!["Get"]);
    assert!(apex_core::plumb::verbs_for(&node.state.meta.rules, &node.window_name(w_file), node.window_kind(w_file), Some(w_file)).is_empty());

    node.exec(&mut log, ExecCtx::Window(w_live), "Get").unwrap();
    assert_eq!(poll(&mut server, &mut log, &mut node), 0);
    assert!(errors_text(&node).is_empty(), "{}", errors_text(&node));
    let starts = server.take_plumb_starts();
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(starts[0].verb, "Get");
    let (id, step) = server.plumb_start(&node, starts.into_iter().next().unwrap());
    assert!(matches!(step, apex_server::PlumbStep::AskTool { ref tool, ref verb, .. } if tool == "smartlog" && verb == "Get"), "{step:?}");
    let props = match server.plumb_next(&node, id, Ok(())) {
        apex_server::PlumbStep::Done(props) => props,
        other => panic!("{other:?}"),
    };
    perform(&mut node, &mut log, props);
    assert_eq!(exec_status(&node, w_live), ExecStatus::Done);
    assert_eq!(body_text(&node, w_live), "generated\n");

    std::fs::write(&path, "reloaded\n").unwrap();
    node.exec(&mut log, ExecCtx::Window(w_file), "Get").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert_eq!(body_text(&node, w_file), "reloaded\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dirty_get_rule_reaches_the_tool_on_first_invocation() {
    let (mut log, mut node, col, mut server, _rx) = session();
    let dir = std::env::temp_dir().join(format!("apex-get-dirty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let smartlog = format!("{}/+smartlog", dir.display());
    let w = node.new_window(&mut log, col, &smartlog, "generated\n").unwrap();
    let rule = PlumbRule {
        verb: "Get".into(),
        text: None,
        file: Some(r"\+smartlog$".into()),
        kind: Some(WinKind::File),
        isfile: None,
        isdir: None,
        action: RuleAction::Tool("smartlog".into()),
        win: None, to: None,
    };
    let (_, e) = log.install_rule(SERVER, 0, rule);
    node.state.apply(Shard::Meta, &e).unwrap();
    let v = ViewId::Body(w);
    let b = node.view_buffer(v).unwrap();
    node.select(&mut log, v, 0, 0).unwrap();
    node.insert(&mut log, v, "dirty ").unwrap();
    assert!(node.state.buffer(b).unwrap().dirty());

    assert!(matches!(node.exec(&mut log, ExecCtx::Window(w), "Get").unwrap(), Executed::Deferred(_)));
    assert_eq!(poll(&mut server, &mut log, &mut node), 0);
    assert!(errors_text(&node).is_empty(), "{}", errors_text(&node));
    let starts = server.take_plumb_starts();
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(starts[0].verb, "Get");
    assert!(node.state.buffer(b).unwrap().dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn timed_out_get_rule_fails_without_reloading_generated_content() {
    let (mut log, mut node, col, mut server, _rx) = session();
    let dir = std::env::temp_dir().join(format!("apex-get-timeout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let smartlog = format!("{}/+smartlog", dir.display());
    let w = node.new_window(&mut log, col, &smartlog, "generated\n").unwrap();
    let rule = PlumbRule {
        verb: "Get".into(),
        text: None,
        file: Some(r"\+smartlog$".into()),
        kind: Some(WinKind::File),
        isfile: None,
        isdir: None,
        action: RuleAction::Tool("smartlog".into()),
        win: None, to: None,
    };
    let (_, e) = log.install_rule(SERVER, 0, rule);
    node.state.apply(Shard::Meta, &e).unwrap();

    node.exec(&mut log, ExecCtx::Window(w), "Get").unwrap();
    assert_eq!(poll(&mut server, &mut log, &mut node), 0);
    let req = server.take_plumb_starts().into_iter().next().expect("Get plumb start");
    let (id, step) = server.plumb_start(&node, req);
    assert!(matches!(step, apex_server::PlumbStep::AskTool { .. }), "{step:?}");
    let props = match server.plumb_next(&node, id, Err("timed out".into())) {
        // refused: no rule took it, and the walk says why (Plumbed's why)
        apex_server::PlumbStep::Refused { props, why } if why.contains("no rule") => props,
        other => panic!("{other:?}"),
    };
    perform(&mut node, &mut log, props);
    assert_eq!(body_text(&node, w), "generated\n");
    assert!(errors_text(&node).contains("Get: no rule takes it here"), "{}", errors_text(&node));
    assert_eq!(exec_status(&node, w), ExecStatus::Failed("Get: no rule".into()));
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
        server.term_key(&mut log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
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

#[test]
fn terminal_selection_follows_the_scrollback_and_keys_scroll_to_the_bottom() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().next().copied().expect("terminal");
    server.close_orphan_terms(&mut log, &node);
    let key = |server: &mut Server, log: &mut Log, c: char| {
        server.term_key(log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
    };
    for c in "for i in $(seq 1 100); do echo line-$i; done\r".chars() {
        key(&mut server, &mut log, c);
    }
    let rows = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).iter().any(|r| r == "line-100")), "grid:\n{}", rows(&node).join("\n"));
    let bottom = node.state.terms[&t].top;
    assert!(bottom > 0, "the output scrolled into history");
    // scroll back: the viewport's first line moves up in the history
    server.term_scroll(&mut log, t, -20);
    node.catch_up(&log).unwrap();
    let top = node.state.terms[&t].top;
    assert_eq!(top, bottom - 20);
    // a selection is addressed by history line, so it names the same text
    // wherever the viewport is
    let shown = rows(&node);
    let r = shown.iter().position(|r| r.starts_with("line-")).expect("a line in view");
    let n: u64 = shown[r][5..].parse().unwrap();
    let line = top + r as u64;
    let Some(Proposal::Snarf { text }) = server.term_text(t, (0, line), (0, line + 2)) else { panic!("no text") };
    assert_eq!(text, format!("line-{n}\nline-{}", n + 1));
    let Some(Proposal::Snarf { text }) = server.term_text(t, (5, line), (7, line)) else { panic!("no text") };
    assert_eq!(text, shown[r][5..7]);
    assert!(server.term_text(t, (3, line), (3, line)).is_none());
    // typing brings the live screen back
    key(&mut server, &mut log, 'x');
    node.catch_up(&log).unwrap();
    assert_eq!(node.state.terms[&t].top, bottom);
}

#[test]
fn terminal_labels_name_the_window_and_its_shell_knows_the_session() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    server.env = vec![("apexsession".into(), "main".into())];
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let w = node.state.windows.values().find(|w| matches!(w.body, Body::Term(_))).map(|w| w.id).expect("terminal window");
    let Body::Term(t) = node.state.window(w).unwrap().body else { unreachable!() };
    server.close_orphan_terms(&mut log, &node);
    let name = |n: &Node| {
        let tag = n.state.window(w).unwrap().tag;
        n.state.buffer(tag).unwrap().text.to_string().split(' ').next().unwrap_or("").to_string()
    };
    // win's name: the directory, then -host
    let host = apex_server::term::sysname();
    assert!(name(&node).ends_with(&format!("/-{host}")), "{}", name(&node));
    let type_ = |server: &mut Server, log: &mut Log, s: &str| {
        for c in s.chars() {
            server.term_key(log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
        }
    };
    let rows = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    // the shell's environment: the session, and a truecolor xterm
    type_(&mut server, &mut log, "echo s=$apexsession c=$COLORTERM t=$TERM w=$winid\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).contains(&format!("s=main c=truecolor t=xterm-256color w={}", w.0))), "grid:\n{}", rows(&node));
    // the rule: {osc7 path}/-{title}. A label (plan9port's) alone is a
    // title, after the directory the terminal started in until one is reported
    let base = std::env::temp_dir().join(format!("apex-label-{}", std::process::id()));
    let (a, b) = (base.join("a"), base.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    type_(&mut server, &mut log, "printf '\\033];x\\007'\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n).ends_with("/-x") && !name(n).starts_with('-')), "name: {}", name(&node));
    // OSC 7 reports the directory: the path, the title after it; B2/B3 resolve there
    type_(&mut server, &mut log, &format!("printf '\\033]7;file://somehost{}\\007'\r", a.display()));
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{}/-x", a.display())), "name: {}", name(&node));
    assert_eq!(server.dir_of(&node, ExecCtx::Window(w)), a);
    // winsettag keeps the name (a terminal's name is its tag's first word)
    node.update_tags(&mut log).unwrap();
    assert_eq!(node.window_name(w), format!("{}/-x", a.display()));
    // another directory: the path follows, the title stays
    type_(&mut server, &mut log, &format!("printf '\\033]7;file://somehost{}\\007'\r", b.display()));
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{}/-x", b.display())), "name: {}", name(&node));
    assert_eq!(server.dir_of(&node, ExecCtx::Window(w)), b);
    // an xterm title is a title; once OSC 7 has spoken, nothing else is the path
    type_(&mut server, &mut log, "printf '\\033]2;hello\\007'\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{}/-hello", b.display())), "name: {}", name(&node));
    type_(&mut server, &mut log, "printf '\\033]2;~/src\\007'\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{}/-~/src", b.display())), "name: {}", name(&node));
    assert_eq!(server.dir_of(&node, ExecCtx::Window(w)), b);
    // the labels never reached the screen
    assert!(!rows(&node).contains("\u{1b}"));
    // the shell's exit is still noticed
    type_(&mut server, &mut log, "exit\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| n.state.terms.get(&t).is_some_and(|t| t.exit.is_some())), "exit noticed");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_rules_verb_shows_in_the_tag_and_b2_runs_it() {
    let (mut log, mut node, col, mut server, mut rx) = session();
    server.install_default_rules(&mut log);
    node.catch_up(&log).unwrap();
    let dir = std::env::temp_dir().join(format!("apex-verb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("notes.md");
    std::fs::write(&md, "# hi\n").unwrap();
    // a rule offering Preview on .md files, run as a command
    let rule = PlumbRule {
        verb: "Preview".into(),
        text: None,
        file: Some(r"\.md$".into()),
        kind: Some(WinKind::File),
        isfile: None,
        isdir: None,
        action: RuleAction::Run("echo previewing $file".into()),
        win: None, to: None,
    };
    let (_, e) = log.install_rule(SERVER, 0, rule);
    node.state.apply(Shard::Meta, &e).unwrap();
    let p = server.open_file(col, None, &dir, "notes.md", None).unwrap();
    let w = perform(&mut node, &mut log, vec![p]).expect("window");
    // the verb is offered in the window's tools menu (B4), not its tag
    let verbs = |n: &Node, w: WindowId| apex_core::plumb::verbs_for(&n.state.meta.rules, &n.window_name(w), n.window_kind(w), Some(w));
    assert_eq!(verbs(&node, w), vec!["Preview"]);
    node.update_tags(&mut log).unwrap();
    let tag = node.state.buffer(node.state.window(w).unwrap().tag).unwrap().text.to_string();
    assert!(!tag.contains("Preview"), "tag: {tag}");
    // a .txt window does not offer it
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    let p = server.open_file(col, None, &dir, "a.txt", None).unwrap();
    let w2 = perform(&mut node, &mut log, vec![p]).expect("window");
    assert!(verbs(&node, w2).is_empty());
    // B2 Preview: the rule runs the command, output in +Errors
    node.exec(&mut log, ExecCtx::Window(w), "Preview").unwrap();
    poll(&mut server, &mut log, &mut node);
    for req in server.take_plumb_starts() {
        let (_, step) = server.plumb_start(&node, req);
        assert!(matches!(step, apex_server::PlumbStep::Done(_)), "{step:?}");
    }
    let errors = |n: &Node| n.state.windows.keys().find(|w| n.window_name(**w).ends_with("+Errors")).and_then(|w| n.state.window(*w).ok()).and_then(|x| x.body_buffer()).and_then(|b| n.state.buffer(b).ok()).map(|b| b.text.to_string()).unwrap_or_default();
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| errors(n).contains(&format!("previewing {}", md.display()))), "errors:\n{}", errors(&node));
    // acme's expand: nothing takes `s.pchr` (no such file), so the word is tried
    let word = ("pchr".to_string(), Span { buffer: BufferId(0), q0: 2, q1: 6 });
    let req = apex_server::PlumbReq { ctx: ExecCtx::Window(w), text: "s.pchr".into(), dir: None, verb: "plumb".into(), edit_only: false, dry: true, exec: None, at: None, sel: None, alt: Some(word), reverse: false };
    let (_, step) = server.plumb_start(&node, req);
    match step {
        apex_server::PlumbStep::Trace(lines) => {
            assert!(lines.iter().any(|l| l.contains("as the word \"pchr\"")), "{lines:?}");
            assert!(lines.last().is_some_and(|l| l.contains("Look")), "{lines:?}");
        }
        other => panic!("{other:?}"),
    }
    // the default rules: B3 on name:line opens the file at the line
    let req = apex_server::PlumbReq { ctx: ExecCtx::Window(w), text: "a.txt:1".into(), dir: None, verb: "plumb".into(), edit_only: false, dry: true, exec: None, at: None, sel: None, alt: None, reverse: false };
    let (_, step) = server.plumb_start(&node, req);
    match step {
        apex_server::PlumbStep::Trace(lines) => assert!(lines.iter().any(|l| l.contains("would open a.txt:1")), "{lines:?}"),
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn newterm_with_a_command_runs_it_instead_of_a_shell() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm printf 'ran %s\\n' here").unwrap();
    poll(&mut server, &mut log, &mut node);
    let w = node.state.windows.values().find(|w| matches!(w.body, Body::Term(_))).map(|w| w.id).expect("terminal window");
    assert!(node.window_name(w).ends_with("/-printf"), "{}", node.window_name(w));
    // live while it runs
    assert!(node.window_live(w));
    let Body::Term(t) = node.state.window(w).unwrap().body else { unreachable!() };
    server.close_orphan_terms(&mut log, &node);
    let rows = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).contains("ran here")), "grid:\n{}", rows(&node));
    // and it is done when the command is: no longer live
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| n.state.terms.get(&t).is_some_and(|t| t.exit.is_some())), "exit noticed");
    assert!(!node.window_live(w));
}

#[test]
fn a_name_typed_into_the_tag_is_where_put_writes() {
    let (mut log, mut node, _col, mut server, _rx) = session();
    let dir = std::env::temp_dir().join(format!("apex-tagname-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    server.cwd = dir.clone();
    // an empty window, some text, a name typed into its tag
    node.exec(&mut log, ExecCtx::Top, "New").unwrap();
    let w = node.state.windows.keys().copied().max().expect("window");
    let b = node.state.window(w).unwrap().body_buffer().unwrap();
    node.set_content(&mut log, b, "hello\n").unwrap();
    let tag = node.state.window(w).unwrap().tag;
    let old = node.state.buffer(tag).unwrap().text.to_string();
    node.set_content(&mut log, tag, &format!("notes.txt{old}")).unwrap();
    // winsettag leaves the typed name alone
    node.update_tags(&mut log).unwrap();
    assert!(node.state.buffer(tag).unwrap().text.to_string().starts_with("notes.txt "), "{}", node.state.buffer(tag).unwrap().text.to_string());
    assert_eq!(node.window_name(w), "");
    // a click in the tag commits it
    node.commit_tag(&mut log, w).unwrap();
    assert_eq!(node.window_name(w), "notes.txt");
    // Put writes it where the window is, and the name becomes absolute
    node.exec(&mut log, ExecCtx::Window(w), "Put").unwrap();
    poll(&mut server, &mut log, &mut node);
    let path = dir.join("notes.txt");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    assert_eq!(node.window_name(w), path.display().to_string());
    assert!(!node.state.buffer(b).unwrap().dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn jumps_stack_up_and_back_returns() {
    let (mut log, mut node, col, server, _rx) = session();
    let dir = std::env::temp_dir().join(format!("apex-nav-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join("b.txt"), "x\ny\n").unwrap();
    // a.txt open, dot on line 2: the origin
    let p = server.open_file(col, None, &dir, "a.txt", None).unwrap();
    let a = perform(&mut node, &mut log, vec![p]).unwrap();
    node.select(&mut log, ViewId::Body(a), 4, 7).unwrap();
    node.seltext = Some(ViewId::Body(a));
    // a jump to b.txt:2, its window not open yet: the origin is recorded,
    // the place is left for whoever opens files
    let b_name = dir.join("b.txt").display().to_string();
    let r = apex_server::proposal::apply(&mut node, &mut log, apex_server::Proposal::Goto { loc: Loc { session: None, name: b_name.clone(), pos: Pos::Line(2) } }).unwrap();
    assert!(r.is_none());
    assert_eq!(node.state.layout.nav_back.len(), 1);
    assert_eq!(node.state.layout.nav_back[0].pos, Pos::Chars(4, 7));
    let gotos = node.take_gotos();
    assert_eq!(gotos.len(), 1);
    // opened (as the daemon or the app would), landing selects line 2
    let p = server.open_file(col, None, &dir, "b.txt", None).unwrap();
    let b = perform(&mut node, &mut log, vec![p]).unwrap();
    node.land(&mut log, &gotos[0]).unwrap();
    assert_eq!(node.selection(ViewId::Body(b)).unwrap(), (2, 4));
    assert_eq!(node.seltext, Some(ViewId::Body(b)));
    // Back: to a.txt at 4..7; where we were goes forward
    let r = apex_server::proposal::apply(&mut node, &mut log, apex_server::Proposal::Nav { back: true }).unwrap();
    assert_eq!(r, Some(a));
    assert_eq!(node.selection(ViewId::Body(a)).unwrap(), (4, 7));
    assert!(node.state.layout.nav_back.is_empty());
    assert_eq!(node.state.layout.nav_forward.len(), 1);
    // and Fwd returns
    let r = apex_server::proposal::apply(&mut node, &mut log, apex_server::Proposal::Nav { back: false }).unwrap();
    assert_eq!(r, Some(b));
    assert!(apex_server::proposal::apply(&mut node, &mut log, apex_server::Proposal::Nav { back: false }).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn osc8_hyperlinks_reach_the_grid() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().copied().next().expect("terminal");
    // a link, as printf writes it: OSC 8 ; ; uri ST text OSC 8 ; ; ST
    let cmd = "printf '\\033]8;;http://x.example/z\\033\\\\LINKED\\033]8;;\\033\\\\ plain\\n'\r";
    for c in cmd.chars() {
        server.term_key(&mut log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
    }
    let linked = |n: &Node| -> Option<(String, bool)> {
        let term = n.state.terms.get(&t)?;
        for row in &term.grid {
            let text: String = row.iter().map(|c| c.ch).collect();
            if let Some(i) = text.find("LINKED plain") {
                let l = row[i];
                let p = row[i + "LINKED ".len()];
                let uri = if l.link == 0 { String::new() } else { term.links[l.link as usize - 1].clone() };
                return Some((uri, p.link == 0));
            }
        }
        None
    };
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| linked(n).is_some_and(|(u, _)| !u.is_empty())), "{:?}", linked(&node));
    let (uri, plain_unlinked) = linked(&node).unwrap();
    assert_eq!(uri, "http://x.example/z");
    assert!(plain_unlinked, "text after the link carries no link");
}

#[test]
fn the_wheel_reaches_programs_that_read_the_mouse() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().copied().next().expect("terminal");
    let type_line = |server: &mut Server, log: &mut Log, line: &str| {
        for c in line.chars() {
            server.term_key(log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
        }
    };
    let grid_text = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    // SGR mouse reporting on, then cat -v shows what the program reads
    type_line(&mut server, &mut log, "printf '\\033[?1000h\\033[?1006h'; cat -v\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("cat -v")));
    std::thread::sleep(Duration::from_millis(200));
    // a wheel notch up at column 2, row 3: button 64 there, 1-based
    server.term_wheel(&mut log, t, -1, Some((2, 3)));
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("^[[<64;3;4M")), "{}", grid_text(&node));
    // from the scrollbar (no cell) the wheel is ours: nothing reaches cat
    server.term_wheel(&mut log, t, 1, None);
    std::thread::sleep(Duration::from_millis(200));
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |_| true));
    assert!(!grid_text(&node).contains("^[[<65"), "{}", grid_text(&node));
    // mouse off, alternate screen: the wheel is arrow keys (alternate scroll).
    // ^D twice: the first hands cat the pending report, the second is EOF
    type_line(&mut server, &mut log, "\u{4}\u{4}");
    type_line(&mut server, &mut log, "printf '\\033[?1000l\\033[?1006l\\033[?1049h'; echo READY; cat -v\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("READY")), "{}", grid_text(&node));
    std::thread::sleep(Duration::from_millis(200));
    server.term_wheel(&mut log, t, 2, Some((0, 0)));
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("^[[B^[[B")), "{}", grid_text(&node));
}

#[test]
fn newweb_opens_a_web_window_whose_name_follows_the_page() {
    let (mut log, mut node, _col, mut server, _rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newweb https://example.com/").unwrap();
    poll(&mut server, &mut log, &mut node);
    let w = node.state.windows.values().find(|w| w.body == Body::Web).map(|w| w.id).expect("a web window");
    assert_eq!(node.window_name(w), "https://example.com/");
    assert_eq!(node.window_kind(w), WinKind::Web);
    assert!(node.state.window(w).unwrap().body_buffer().is_none());
    // the page goes somewhere: the name follows, the place left is behind us
    apex_server::perform(&mut node, &mut log, vec![apex_server::Proposal::WebNavigate { window: w, url: "https://example.com/two".into() }]);
    assert_eq!(node.window_name(w), "https://example.com/two");
    let back = node.state.layout.nav_back.last().cloned().expect("a place to go back to");
    assert_eq!(back.name, "https://example.com/");
    // the same URL again is no move
    apex_server::perform(&mut node, &mut log, vec![apex_server::Proposal::WebNavigate { window: w, url: "https://example.com/two".into() }]);
    assert_eq!(node.state.layout.nav_back.len(), 1);
    // Back: no window shows that page now, so it is a place to open
    apex_server::perform(&mut node, &mut log, vec![apex_server::Proposal::Nav { back: true }]);
    let gotos = node.take_gotos();
    assert_eq!(gotos.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["https://example.com/"]);
    assert!(apex_core::is_url("https://example.com/") && apex_core::is_url("apexfile:///a") && !apex_core::is_url("/a/b") && !apex_core::is_url("a://"));
    // Newweb alone is an error
    node.exec(&mut log, ExecCtx::Top, "Newweb").unwrap();
    poll(&mut server, &mut log, &mut node);
    assert!(errors_text(&node).contains("Newweb needs a URL"), "{}", errors_text(&node));
}

#[test]
fn html_windows_are_text_shown_as_a_page() {
    let (mut log, mut node, col, _server, _rx) = session();
    let w = apex_server::perform(&mut node, &mut log, vec![apex_server::Proposal::OpenHtml { col, name: "/tmp/x/+web".into(), text: "<h1>hi</h1>".into() }]).expect("a window");
    let win = node.state.window(w).unwrap();
    let Body::Html(b) = win.body else { panic!("{:?}", win.body) };
    assert_eq!(win.body_buffer(), Some(b));
    assert_eq!(node.window_name(w), "/tmp/x/+web");
    assert_eq!(node.window_kind(w), WinKind::Web);
    assert_eq!(node.state.buffer(b).unwrap().text.to_string(), "<h1>hi</h1>");
    // its text is edited as any buffer's: the page follows the version
    let version = node.state.buffer(b).unwrap().version;
    apex_server::perform(&mut node, &mut log, vec![apex_server::Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: 4, q1: 6, text: "yo".into() }]);
    assert_eq!(node.state.buffer(b).unwrap().text.to_string(), "<h1>yo</h1>");
    assert!(node.state.buffer(b).unwrap().version > version);
    // and it is placed in the column like any window, with body room
    let slot = node.state.layout.cols.iter().flat_map(|c| c.wins.iter()).find(|s| s.window == w).cloned().expect("placed");
    assert!(slot.body.dy() > 0, "{slot:?}");
}

#[test]
fn web_opens_a_page_on_the_url_given_or_selected() {
    let (mut log, mut node, col, _server, _rx) = session();
    // typed after the word: a URL as it is
    node.exec(&mut log, ExecCtx::Top, "Web https://example.com/").unwrap();
    let w = node.state.windows.values().find(|w| w.body == Body::Web).map(|w| w.id).expect("a web window");
    assert_eq!(node.window_name(w), "https://example.com/");
    assert!(node.state.buffer(node.state.window(w).unwrap().tag).unwrap().text.to_string().contains(" Back Fwd Get "));
    // and winsettag keeps them there
    node.update_tags(&mut log).unwrap();
    assert!(node.state.buffer(node.state.window(w).unwrap().tag).unwrap().text.to_string().starts_with("https://example.com/ Del Snarf Back Fwd Get |"));
    // selected in a text window: a file:// URL and a bare path are the host's files
    let t = node.new_window(&mut log, col, "/tmp/here/notes.txt", "see file:///tmp/a.html and also doc.html\n").unwrap();
    node.select(&mut log, ViewId::Body(t), 4, 22).unwrap();
    node.exec(&mut log, ExecCtx::Window(t), "Web").unwrap();
    let names: Vec<String> = node.state.windows.values().filter(|w| w.body == Body::Web).map(|w| node.window_name(w.id)).collect();
    assert!(names.contains(&"apexfile:///tmp/a.html".to_string()), "{names:?}");
    node.select(&mut log, ViewId::Body(t), 32, 40).unwrap();
    node.exec(&mut log, ExecCtx::Window(t), "Web").unwrap();
    let names: Vec<String> = node.state.windows.values().filter(|w| w.body == Body::Web).map(|w| node.window_name(w.id)).collect();
    assert!(names.contains(&"apexfile:///tmp/here/doc.html".to_string()), "{names:?}");
    assert_eq!(apex_core::node::web_url("file://localhost/x/y", "/d"), "apexfile:///x/y");
    assert_eq!(apex_core::node::web_url("/abs/p", "/d"), "apexfile:///abs/p");
    assert_eq!(apex_core::node::web_url("rel/p", "/d/"), "apexfile:///d/rel/p");
    // nothing given or selected: the command fails, saying so
    node.select(&mut log, ViewId::Body(t), 0, 0).unwrap();
    let r = node.exec(&mut log, ExecCtx::Window(t), "Web").unwrap();
    assert!(matches!(r, apex_core::node::Executed::Failed(_, ref why) if why.contains("Web needs a URL")), "{r:?}");
    assert!(apex_core::node::TOP_TAG.contains(" Web "));
}

#[test]
fn a_terminal_publishes_only_what_changed() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().copied().next().expect("terminal");
    let grid_text = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains('$') || grid_text(n).contains('%')));
    // the prompt is up: publishing again, nothing having changed, adds nothing
    let before = log.last_seq(Shard::Term(t));
    server.publish_term(&mut log, t);
    server.publish_term(&mut log, t);
    assert_eq!(log.last_seq(Shard::Term(t)), before);
    // a line typed: the rows that changed go, not the whole grid
    for c in "echo diffed-rows\r".chars() {
        server.term_key(&mut log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
    }
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("diffed-rows\n") || grid_text(n).matches("diffed-rows").count() >= 2));
    let rows_published: usize = log.since(Shard::Term(t), before).iter().map(|e| match &e.op {
        apex_core::Op::Term(apex_core::TermOp::Rows { rows, .. }) => rows.len(),
        _ => 0,
    }).sum();
    let height = node.state.terms.get(&t).unwrap().rows as usize;
    assert!(rows_published > 0 && rows_published < height * 3, "{rows_published} rows for a few lines of change (height {height})");
}

#[test]
fn a_terminals_name_is_the_reported_directory_and_the_title() {
    use apex_server::term::compose_name;
    let d = std::path::Path::new("/here");
    assert_eq!(compose_name(None, None, d, "host"), "/here/-host");
    // a title before any directory is reported: where the terminal started, then the title
    assert_eq!(compose_name(None, Some("my title here"), d, "host"), "/here/-my title here");
    assert_eq!(compose_name(Some(std::path::Path::new("/foo/bar/")), Some("my title here"), d, "host"), "/foo/bar/-my title here");
    assert_eq!(compose_name(Some(std::path::Path::new("/foo/bar")), None, d, "host"), "/foo/bar/-host");
}

#[test]
fn b3_in_a_terminal_that_no_rule_takes_looks_nowhere_else() {
    // a text window holding the token was the last selected text
    // (seltext: where acme's look3 searches); B3 on the same token in
    // a terminal, taken by no rule, must not search and select it there
    let (mut log, mut node, col, mut server, _rx) = session();
    let a = node.new_window(&mut log, col, "/tmp/smartlog", "commit D117573677 landed\n").unwrap();
    node.select(&mut log, ViewId::Body(a), 0, 0).unwrap();
    assert_eq!(node.seltext, Some(ViewId::Body(a)));
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let term = node.state.windows.values().find(|w| matches!(w.body, Body::Term(_))).map(|w| w.id).expect("terminal window");
    let req = apex_server::PlumbReq { ctx: ExecCtx::Window(term), text: "D117573677".into(), dir: None, verb: "plumb".into(), edit_only: false, dry: false, exec: None, at: None, sel: None, alt: None, reverse: false };
    let (_, step) = server.plumb_start(&node, req);
    let props = match step {
        apex_server::PlumbStep::Refused { props, why } => {
            assert!(why.contains("no rule takes"), "{why}");
            props
        }
        other => panic!("{other:?}"),
    };
    assert!(props.is_empty(), "{props:?}");
    perform(&mut node, &mut log, props);
    assert_eq!(node.selection(ViewId::Body(a)).unwrap(), (0, 0), "the other window's dot moved");
    // the same B3 from the text window itself still looks there
    let req = apex_server::PlumbReq { ctx: ExecCtx::Window(a), text: "D117573677".into(), dir: None, verb: "plumb".into(), edit_only: false, dry: false, exec: None, at: None, sel: None, alt: None, reverse: false };
    let (_, step) = server.plumb_start(&node, req);
    let props = match step {
        apex_server::PlumbStep::Refused { props, .. } => props,
        other => panic!("{other:?}"),
    };
    assert!(props.iter().any(|p| matches!(p, Proposal::Look { .. })), "{props:?}");
    perform(&mut node, &mut log, props);
    assert_eq!(node.selection(ViewId::Body(a)).unwrap(), (7, 17));
}

#[test]
fn a_resize_keeps_the_scrollback_position() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().next().copied().expect("terminal");
    let key = |server: &mut Server, log: &mut Log, c: char| {
        server.term_key(log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
    };
    for c in "for i in $(seq 1 100); do echo line-$i; done\r".chars() {
        key(&mut server, &mut log, c);
    }
    let rows = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).iter().any(|r| r == "line-100")), "grid:\n{}", rows(&node).join("\n"));
    server.term_scroll(&mut log, t, -30);
    node.catch_up(&log).unwrap();
    let top = node.state.terms[&t].top;
    let first = rows(&node)[0].clone();
    assert!(first.starts_with("line-"), "{first}");
    // wider: the same lines stay in view
    server.term_resize(&mut log, t, 100, 24);
    std::thread::sleep(Duration::from_millis(300));
    let deadline = Instant::now() + Duration::from_secs(1);
    pump_until(&mut log, &mut node, &mut server, &mut rx, |_| Instant::now() > deadline);
    assert_eq!(node.state.terms[&t].top, top, "top after a width change; first row {:?}", rows(&node)[0]);
    assert_eq!(rows(&node)[0], first);
    // taller by four: at most four more lines, from above, come into view
    server.term_resize(&mut log, t, 100, 28);
    std::thread::sleep(Duration::from_millis(300));
    let deadline = Instant::now() + Duration::from_secs(1);
    pump_until(&mut log, &mut node, &mut server, &mut rx, |_| Instant::now() > deadline);
    let top2 = node.state.terms[&t].top;
    assert!(top2 <= top && top - top2 <= 4, "top {top} -> {top2}; first row {:?}", rows(&node)[0]);
    let first2 = rows(&node)[0].clone();
    // shorter by twelve: the first line in view stays the first
    server.term_resize(&mut log, t, 100, 16);
    std::thread::sleep(Duration::from_millis(300));
    let deadline = Instant::now() + Duration::from_secs(1);
    pump_until(&mut log, &mut node, &mut server, &mut rx, |_| Instant::now() > deadline);
    assert_eq!(rows(&node)[0], first2, "after shrinking: top {} -> {}", top2, node.state.terms[&t].top);
    // narrower again, while scrolled back: still the same first line
    server.term_resize(&mut log, t, 60, 16);
    std::thread::sleep(Duration::from_millis(300));
    let deadline = Instant::now() + Duration::from_secs(1);
    pump_until(&mut log, &mut node, &mut server, &mut rx, |_| Instant::now() > deadline);
    assert_eq!(rows(&node)[0], first2, "after narrowing: top {}", node.state.terms[&t].top);
}

#[test]
fn clear_drops_a_terminals_scrollback_and_keeps_its_screen() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    server.install_default_rules(&mut log);
    node.catch_up(&log).unwrap();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().next().copied().expect("terminal");
    let w = node.state.windows.values().find(|x| x.body == Body::Term(t)).map(|x| x.id).expect("its window");
    // the verb is offered in a terminal, by the server's rule
    assert!(apex_core::plumb::verbs_for(&node.state.meta.rules, &node.window_name(w), node.window_kind(w), Some(w)).contains(&"Clear".to_string()));
    for c in "for i in $(seq 1 100); do echo line-$i; done\r".chars() {
        server.term_key(&mut log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
    }
    let rows = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).iter().any(|r| r == "line-100")), "grid:\n{}", rows(&node).join("\n"));
    assert!(node.state.terms[&t].top > 0, "output scrolled into history");
    let screen = rows(&node);
    server.term_clear(&mut log, t);
    node.catch_up(&log).unwrap();
    assert_eq!(node.state.terms[&t].top, 0, "no history left");
    assert_eq!(rows(&node), screen, "the screen as it was");
}

#[test]
fn a_resize_while_scrolled_back_waits_for_the_bottom() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().next().copied().expect("terminal");
    for c in "for i in $(seq 1 100); do echo line-$i; done\r".chars() {
        server.term_key(&mut log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
    }
    let rows = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).iter().any(|r| r == "line-100")), "grid:\n{}", rows(&node).join("\n"));
    server.term_scroll(&mut log, t, -30);
    node.catch_up(&log).unwrap();
    let first = rows(&node)[0].clone();
    // the window changes size: the terminal, scrolled back, keeps its
    // size (the program hears nothing, so cannot redraw over the reading)
    server.term_resize(&mut log, t, 100, 40);
    let deadline = Instant::now() + Duration::from_millis(500);
    pump_until(&mut log, &mut node, &mut server, &mut rx, |_| Instant::now() > deadline);
    assert_eq!((node.state.terms[&t].cols, node.state.terms[&t].rows), (80, 24));
    assert_eq!(rows(&node)[0], first);
    // back at the bottom: the size applies, and the program is told
    server.term_scroll(&mut log, t, 100);
    let deadline = Instant::now() + Duration::from_millis(500);
    pump_until(&mut log, &mut node, &mut server, &mut rx, |_| Instant::now() > deadline);
    assert_eq!((node.state.terms[&t].cols, node.state.terms[&t].rows), (100, 40));
    assert_eq!(node.state.terms[&t].grid.len(), 40);
}

#[test]
fn a_program_asking_the_background_is_told_the_clients_colours() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().next().copied().expect("terminal");
    let rows = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    let ask = |server: &mut Server, log: &mut Log| {
        // OSC 11 ?: the answer comes back as input, which the shell shows
        for c in "printf '\\e]11;?\\a'\r".chars() {
            server.term_key(log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
        }
    };
    // no UI has said: acme's paper
    ask(&mut server, &mut log);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).contains("rgb:ffff/ffff/eaea")), "grid:\n{}", rows(&node));
    // a dark UI's paper, once it has said
    server.term_colors = apex_server::proto::TermColors { bg: 0x1E1E14, ..apex_server::proto::TermColors::LIGHT };
    ask(&mut server, &mut log);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).contains("rgb:1e1e/1e1e/1414")), "grid:\n{}", rows(&node));
}

#[test]
fn focus_reaches_programs_that_asked_for_it() {
    let (mut log, mut node, _col, mut server, mut rx) = session();
    node.exec(&mut log, ExecCtx::Top, "Newterm").unwrap();
    poll(&mut server, &mut log, &mut node);
    let t = node.state.terms.keys().copied().next().expect("terminal");
    let type_line = |server: &mut Server, log: &mut Log, line: &str| {
        for c in line.chars() {
            server.term_key(log, t, &apex_server::TermKey { key: c.to_string(), text: Some(c.to_string()), shift: false, control: false, alt: false });
        }
    };
    let grid_text = |n: &Node| n.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    // before a program asks, focus is nothing to it
    server.term_focus(t, true);
    // focus reporting on, then cat -v shows what the program reads
    type_line(&mut server, &mut log, "printf '\\033[?1004h'; cat -v\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("cat -v")));
    std::thread::sleep(Duration::from_millis(200));
    server.term_focus(t, false);
    server.term_focus(t, true);
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| grid_text(n).contains("^[[O^[[I")), "{}", grid_text(&node));
}
