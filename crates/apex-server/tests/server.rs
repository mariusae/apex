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
    type_(&mut server, &mut log, "echo s=$apexsession c=$COLORTERM t=$TERM\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| rows(n).contains("s=main c=truecolor t=xterm-256color")), "grid:\n{}", rows(&node));
    // plan9port's label sequence names the window, and moves its directory
    let base = std::env::temp_dir().join(format!("apex-label-{}", std::process::id()));
    let (a, b) = (base.join("a"), base.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    type_(&mut server, &mut log, &format!("printf '\\033];{}/-x\\007'\r", a.display()));
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{}/-x", a.display())), "name: {}", name(&node));
    assert_eq!(server.dir_of(&node, ExecCtx::Window(w)), a);
    // winsettag keeps the name (a terminal's name is its tag's first word)
    node.update_tags(&mut log).unwrap();
    assert_eq!(name(&node), format!("{}/-x", a.display()));
    assert_eq!(node.window_name(w), format!("{}/-x", a.display()));
    // OSC 7, the working-directory report, keeps the label's name
    type_(&mut server, &mut log, &format!("printf '\\033]7;file://somehost{}\\007'\r", b.display()));
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{}/-x", b.display())), "name: {}", name(&node));
    assert_eq!(server.dir_of(&node, ExecCtx::Window(w)), b);
    // an xterm title is a label too
    type_(&mut server, &mut log, "printf '\\033]2;hello\\007'\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == "hello/-x"), "name: {}", name(&node));
    // a ~ in a title or label (zsh's %~) is the home directory
    let home = std::env::var("HOME").unwrap();
    type_(&mut server, &mut log, "printf '\\033]2;~/src\\007'\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{home}/src/-x")), "name: {}", name(&node));
    type_(&mut server, &mut log, "printf '\\033];~/-y\\007'\r");
    assert!(pump_until(&mut log, &mut node, &mut server, &mut rx, |n| name(n) == format!("{home}/-y")), "name: {}", name(&node));
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
        to: None,
    };
    let (_, e) = log.install_rule(SERVER, 0, rule);
    node.state.apply(Shard::Meta, &e).unwrap();
    let p = server.open_file(col, None, &dir, "notes.md", None).unwrap();
    let w = perform(&mut node, &mut log, vec![p]).expect("window");
    // the verb is offered in the window's tools menu (B4), not its tag
    let verbs = |n: &Node, w: WindowId| apex_core::plumb::verbs_for(&n.state.meta.rules, &n.window_name(w), n.window_kind(w));
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
