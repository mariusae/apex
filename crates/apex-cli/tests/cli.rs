//! The `apex` command end to end against a daemon on a thread.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::Script;
use apex_server::remote::Remote;

fn daemon() -> PathBuf {
    daemon_with(None)
}

/// A daemon on a thread, with `host_profile` as the host's `~/.apex/profile`
/// (the real one stays out of the tests).
fn daemon_with(host_profile: Option<PathBuf>) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-cli-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", host_profile).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

fn apex(sock: &PathBuf, args: &[&str]) -> (bool, String, String) {
    // the test daemon's session is "main"; a later --session in `args` wins
    let out = Command::new(env!("CARGO_BIN_EXE_apex")).arg(format!("-socket={}", sock.display())).arg("-session=main").args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

/// The labels in `apex ls` output (`label<TAB>id` per line).
fn labels(ls: &str) -> Vec<String> {
    ls.lines().map(|l| l.split('\t').next().unwrap_or("").to_string()).collect()
}

fn ok(sock: &PathBuf, args: &[&str]) -> String {
    let (success, out, err) = apex(sock, args);
    assert!(success, "apex {args:?} failed: {err}");
    out
}

#[test]
fn scripts_drive_a_headless_session() {
    let sock = daemon();
    assert_eq!(labels(&ok(&sock, &["ls"])), vec!["main"]);
    ok(&sock, &["new-session", "side"]);
    assert_eq!(labels(&ok(&sock, &["ls"])), vec!["main", "side"]);

    let dir = std::env::temp_dir().join(format!("apex-cli-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notes.txt");
    std::fs::write(&path, "pear\napple\nfig\n").unwrap();
    let p = path.to_string_lossy().to_string();

    // open, list, read
    let listed = ok(&sock, &["open", &p]);
    assert!(listed.contains(&p), "{listed}");
    let wins = ok(&sock, &["win", "list"]);
    assert!(wins.contains("notes.txt"), "{wins}");
    assert_eq!(ok(&sock, &["text", "read", "notes.txt"]), "pear\napple\nfig\n");
    assert_eq!(ok(&sock, &["text", "read", "-addr=2", "notes.txt"]), "apple\n");

    // the Edit language, then Put through exec
    ok(&sock, &["edit", "notes.txt", ",x/fig/ c/kiwi/"]);
    assert_eq!(ok(&sock, &["text", "read", "notes.txt"]), "pear\napple\nkiwi\n");
    let wins = ok(&sock, &["win", "list"]);
    assert!(wins.contains("*"), "dirty mark: {wins}");
    ok(&sock, &["exec", "notes.txt", "Put"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_to_string(&path).unwrap() != "pear\napple\nkiwi\n" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "pear\napple\nkiwi\n");

    // selections
    ok(&sock, &["sel", "notes.txt", "5", "10"]);
    assert_eq!(ok(&sock, &["sel", "notes.txt"]), "5 10\n");

    // a pipe through the shell, as B2 would: the selection ("apple") sorts
    // to itself, but the replacement dirties the window
    ok(&sock, &["exec", "notes.txt", "|sort"]);
    let dirty = |sock: &PathBuf| ok(sock, &["win", "list"]).lines().any(|l| l.contains("\t*") && l.ends_with("notes.txt"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dirty(&sock) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(dirty(&sock), "|sort dirties the window:\n{}", ok(&sock, &["win", "list"]));
    // sort's output ends in a newline the selection did not have
    assert_eq!(ok(&sock, &["text", "read", "notes.txt"]), "pear\napple\n\nkiwi\n");

    // a terminal
    let t = ok(&sock, &["term", "new"]);
    let t = t.trim().to_string();
    ok(&sock, &["term", "send", &t, "echo cli-$((6*7))\r"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut grid = String::new();
    while Instant::now() < deadline {
        grid = ok(&sock, &["term", "read", &t]);
        if grid.contains("cli-42") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(grid.contains("cli-42"), "grid:\n{grid}");

    // the other session is untouched
    let (_, wins, _) = apex(&sock, &["-session=side", "win", "list"]);
    assert_eq!(wins, "");

    // delete the window: acme warns once while it is dirty, then goes
    ok(&sock, &["win", "del", "notes.txt"]);
    assert!(ok(&sock, &["win", "list"]).contains("notes.txt"));
    assert!(ok(&sock, &["text", "read", "+Errors"]).contains("notes.txt modified"));
    ok(&sock, &["win", "del", "notes.txt"]);
    assert!(!ok(&sock, &["win", "list"]).contains("notes.txt"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn attach_stdio_bridges_the_socket() {
    use apex_core::*;
    use apex_server::remote::Link;
    let sock = daemon();
    // the same path `apex attach host/session` takes, with the bridge run
    // locally instead of through ssh
    let mut child = Command::new(env!("CARGO_BIN_EXE_apex"))
        .arg(format!("-socket={}", sock.display()))
        .args(["attach", "-stdio"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (mut link, mut log, mut node) = Link::over_streams(Box::new(stdout), Box::new(stdin), None, "main", "over-stdio", AttachmentKind::Ui, None).unwrap();
    let col = node.state.layout.cols[0].id;
    let w = node.new_window(&mut log, col, "bridged", "hello\n").unwrap();
    link.flush(&log);
    let b = node.view_buffer(ViewId::Body(w)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while link.acked.get(&Shard::Buffer(b)).copied().unwrap_or(0) < log.last_seq(Shard::Buffer(b)) && Instant::now() < deadline {
        if let Ok(m) = link.rx.recv_timeout(Duration::from_millis(50)) {
            link.handle(&mut node, &mut log, m);
        }
    }
    assert!(link.acked.get(&Shard::Buffer(b)).is_some(), "acked over the bridge");
    // another client sees the window
    assert!(ok(&sock, &["win", "list"]).contains("bridged"));
    drop(link);
    let _ = child.kill();
}

#[test]
fn a_new_session_runs_the_hosts_profile_then_its_creators() {
    let dir = std::env::temp_dir().join(format!("apex-cli-init-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let apex = env!("CARGO_BIN_EXE_apex");
    std::fs::write(dir.join("host.txt"), "host\n").unwrap();
    std::fs::write(dir.join("client.txt"), "client\n").unwrap();
    // the host's file: apex finds the session through its environment
    let host_profile = dir.join("profile");
    std::fs::write(&host_profile, format!("{apex} open {}/host.txt\n{apex} env FROM=host ORDER=$apexsession\n", dir.display())).unwrap();
    let sock = daemon_with(Some(host_profile.clone()));
    // the daemon's own session ran the host file (its creator's is the same file)
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&sock, &["win", "list"]).contains("host.txt") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // what went wrong, if it did: every +Errors window
    let errors = |sock: &PathBuf, session: &str| -> String {
        ok(sock, &[&format!("-session={session}"), "win", "list"])
            .lines()
            .filter(|l| l.ends_with("+Errors"))
            .map(|l| l.split('\t').nth(1).unwrap_or("").trim_start_matches('*').to_string())
            .map(|w| format!("{w}:\n{}", ok(sock, &[&format!("-session={session}"), "text", "read", &w])))
            .collect()
    };
    assert!(ok(&sock, &["win", "list"]).contains("host.txt"), "{}\n{}", ok(&sock, &["win", "list"]), errors(&sock, "main"));
    assert!(ok(&sock, &["env"]).contains("FROM=host\n"), "{}", ok(&sock, &["env"]));
    // a session made from elsewhere runs the host's file too
    apex_server::remote::new_session(&sock, "s2").unwrap();
    let list = |sock: &PathBuf| ok(sock, &["-session=s2", "win", "list"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !list(&sock).contains("host.txt") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(list(&sock).contains("host.txt"), "{}", list(&sock));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&sock, &["-session=s2", "env"]).contains("ORDER=s2") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let env = ok(&sock, &["-session=s2", "env"]);
    assert!(env.contains("FROM=host\n"), "{env}");
    assert!(env.contains("ORDER=s2\n"), "{env}");
    // the init's name left the top row when it was done
    assert!(!ok(&sock, &["-session=s2", "text", "read", "+Errors"]).contains("exit"), "init exited cleanly");
    // a terminal made now sees the environment
    let t = ok(&sock, &["-session=s2", "term", "new"]);
    let t = t.trim().to_string();
    ok(&sock, &["-session=s2", "term", "send", &t, "echo v=$FROM\r"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut grid = String::new();
    while Instant::now() < deadline {
        grid = ok(&sock, &["-session=s2", "term", "read", &t]);
        if grid.contains("v=host") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(grid.contains("v=host"), "grid:\n{grid}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rules_are_installed_walked_and_tools_may_refuse() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-cli-rules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("readme.md"), "hello\n").unwrap();
    let md = dir.join("readme.md").display().to_string();
    // the defaults are there, owned by the session
    let ls = ok(&sock, &["plumb", "rule", "ls"]);
    assert!(ls.contains("session\tp-100\t-text"), "{ls}");
    // a rule of ours, and its id
    let id = ok(&sock, &["plumb", "rule", "add", "-verb=Preview", r"-file=\.md$", "-run=echo preview $file"]);
    assert!(id.trim().starts_with('r'), "{id}");
    let ls = ok(&sock, &["plumb", "rule", "ls"]);
    assert!(ls.contains("-verb=Preview -file='\\.md$' -run='echo preview $file'"), "{ls}");
    // what B3 would do with a path: the default rule opens it
    let trace = ok(&sock, &["plumb", "-dry-run", &md]);
    assert!(trace.contains("would open"), "{trace}");
    // and with a word that is nothing: a Look
    let trace = ok(&sock, &["plumb", "-dry-run", "nothing-here"]);
    assert!(trace.trim_end().ends_with("no rule: Look"), "{trace}");
    // a tool that refuses: the walk goes on to the next rule
    let tool_sock = sock.clone();
    let tool = std::thread::spawn(move || {
        let mut c = Remote::connect_as(&tool_sock, "main", "t", AttachmentKind::Tool).unwrap();
        let rule = PlumbRule { verb: "plumb".into(), text: None, file: None, kind: None, isfile: None, isdir: None, action: RuleAction::Tool("t".into()), win: None, to: None };
        c.rule_add(rule, 10, true, Duration::from_secs(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut refused = 0;
        while Instant::now() < deadline && refused < 1 {
            let _ = c.step(Duration::from_millis(50));
            while let Some(p) = c.link.plumbs.pop() {
                c.plumb_ack(p.id, false);
                refused += 1;
            }
        }
        // stay a moment so the walk finishes before the rule goes with us
        std::thread::sleep(Duration::from_millis(500));
        refused
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["plumb", "rule", "ls"]).contains("-tool=t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ls = ok(&sock, &["plumb", "rule", "ls"]);
    assert!(ls.contains("\tt(a") && ls.contains("p10\t-tool=t"), "{ls}");
    ok(&sock, &["B", &md]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["win", "list"]).contains("readme.md") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok(&sock, &["win", "list"]).contains("readme.md"), "{}", ok(&sock, &["win", "list"]));
    // B only asks rules that open in the session: the tool was not asked
    assert_eq!(tool.join().unwrap(), 0, "B asked the tool");
    // plain plumbing does ask it, and it refuses; the file opens anyway
    let tool_sock = sock.clone();
    std::fs::write(dir.join("other.md"), "x\n").unwrap();
    let other = dir.join("other.md").display().to_string();
    let tool = std::thread::spawn(move || {
        let mut c = Remote::connect_as(&tool_sock, "main", "t", AttachmentKind::Tool).unwrap();
        let rule = PlumbRule { verb: "plumb".into(), text: None, file: None, kind: None, isfile: None, isdir: None, action: RuleAction::Tool("t".into()), win: None, to: None };
        c.rule_add(rule, 10, true, Duration::from_secs(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut refused = 0;
        while Instant::now() < deadline && refused < 1 {
            let _ = c.step(Duration::from_millis(50));
            while let Some(p) = c.link.plumbs.pop() {
                assert_eq!(p.verb, "plumb");
                c.plumb_ack(p.id, false);
                refused += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
        refused
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["plumb", "rule", "ls"]).contains("-tool=t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    ok(&sock, &["plumb", &other]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["win", "list"]).contains("other.md") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok(&sock, &["win", "list"]).contains("other.md"), "{}", ok(&sock, &["win", "list"]));
    assert_eq!(tool.join().unwrap(), 1, "the tool was asked once");
    // the tool is gone: so is its rule
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["plumb", "rule", "ls"]).contains("-tool=t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!ok(&sock, &["plumb", "rule", "ls"]).contains("-tool=t"));
    // Preview shows in the .md window's tag, and B2 runs it
    let (_, wins, _) = apex(&sock, &["win", "list"]);
    let w = wins.lines().find(|l| l.ends_with("readme.md")).and_then(|l| l.split('\t').next()).unwrap().to_string();
    ok(&sock, &["exec", &w, "Preview"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    // (the errors window appears with the first output)
    let errors = || apex(&sock, &["text", "read", "+Errors"]).1;
    while !errors().contains(&format!("preview {md}")) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(errors().contains(&format!("preview {md}")), "{}", errors());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_attach_script_sets_the_clients_own_settings_and_cat_reads_files() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-cli-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bytes.bin"), b"\x00\x01hello\xff").unwrap();
    // a session setting, from anywhere
    ok(&sock, &["set", "Preview", "Quick"]);
    let bin = env!("CARGO_BIN_EXE_apex");
    // a client attaching with a script: its set is its own
    let script = Script { client: "tester".into(), text: format!("{bin} set Preview.md Marked\n") };
    let c = Remote::connect_with(&sock, "main", "ui", AttachmentKind::Ui, Some(script)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&sock, &["set"]).contains("\tPreview.md\tMarked") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let listing = ok(&sock, &["set"]);
    assert!(listing.contains("session\tPreview\tQuick\n"), "{listing}");
    assert!(listing.contains("ui(a") && listing.contains("\tPreview.md\tMarked\n"), "{listing}");
    // the client sees its own first, then the session's
    let me = c.attachment();
    let mut r = Remote::connect_as(&sock, "main", "look", AttachmentKind::Tool).unwrap();
    let _ = r.step(Duration::from_millis(100));
    assert_eq!(r.node.state.meta.setting(me, "Preview.md"), Some("Marked"));
    assert_eq!(r.node.state.meta.setting(me, "Preview"), Some("Quick"));
    assert_eq!(r.node.state.meta.setting(r.attachment(), "Preview.md"), None);
    // and when it goes, its settings go
    drop(c);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["set"]).contains("Marked") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!ok(&sock, &["set"]).contains("Marked"));
    // cat: the bytes of a file on the host
    let out = Command::new(bin).arg(format!("-socket={}", sock.display())).args(["-session=main", "cat"]).arg(dir.join("bytes.bin")).output().unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"\x00\x01hello\xff");
    let (success, _, err) = apex(&sock, &["cat", "/nowhere/at/all"]);
    assert!(!success && err.contains("No such file"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_watched_file_streams_its_changes_until_unwatched() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-cli-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("live.md");
    std::fs::write(&path, "one\n").unwrap();
    let p = path.display().to_string();
    let mut c = Remote::connect_as(&sock, "main", "viewer", AttachmentKind::Tool).unwrap();
    // the bytes now, version 1
    let (stream, first) = c.watch(&p, Duration::from_secs(5)).unwrap();
    assert_eq!(first, b"one\n");
    // and again when the file changes: version 2
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&path, "two\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = None;
    while Instant::now() < deadline && got.is_none() {
        let _ = c.step(Duration::from_millis(100));
        got = c.io_next_file(stream).filter(|f| f.bytes == b"two\n");
    }
    let f = got.expect("no update after the change");
    assert_eq!(f.version, 2);
    assert_eq!(f.path, p);
    // not after the stream ends (settled: the change's own events all arrived)
    c.unwatch(stream);
    let settle = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settle {
        let _ = c.step(Duration::from_millis(50));
    }
    c.link.io.clear();
    std::fs::write(&path, "three\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(100));
    }
    assert!(c.io_next_file(stream).is_none(), "still streaming after the stream ended");
    // the plane from the command line: GET, PUT, a missing file, a watch
    let out = ok(&sock, &["io", "GET", &format!("file://{p}")]);
    assert_eq!(out, "three\n");
    let put = Command::new(env!("CARGO_BIN_EXE_apex")).arg(format!("-socket={}", sock.display())).args(["-session=main", "io", "PUT", &format!("file://{p}")]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    {
        use std::io::Write;
        put.stdin.as_ref().unwrap().write_all(b"four\n").unwrap();
    }
    let put = put.wait_with_output().unwrap();
    assert!(put.status.success(), "{}", String::from_utf8_lossy(&put.stderr));
    assert_eq!(std::fs::read(&path).unwrap(), b"four\n");
    let (success, _, err) = apex(&sock, &["io", "GET", "file:///nowhere/at/all"]);
    assert!(!success && err.contains("404"), "{err}");
    let mut watcher = Command::new(env!("CARGO_BIN_EXE_apex")).arg(format!("-socket={}", sock.display())).args(["-session=main", "io", "-watch", "GET", &format!("file://{p}")]).stdout(std::process::Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(&path, "five\n").unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    watcher.kill().unwrap();
    let out = watcher.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("four\n") && text.contains("five\n"), "{text:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ps_lists_running_commands_and_kill_ends_them() {
    let sock = daemon();
    ok(&sock, &["exec", "sleep 30"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["ps"]).contains("\tsleep\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ps = ok(&sock, &["ps"]);
    let line = ps.lines().find(|l| l.contains("\tsleep\t")).unwrap_or_else(|| panic!("{ps}"));
    let fields: Vec<&str> = line.split('\t').collect();
    assert!(fields[0].parse::<u32>().is_ok(), "pid: {line}");
    assert_eq!(fields[1], "sleep");
    assert_eq!(fields[2], "top");
    assert_eq!(fields[5], "sleep 30");
    // an unknown name is an error; the right one ends it
    let (success, _, err) = apex(&sock, &["kill", "nothing-runs-here"]);
    assert!(!success && err.contains("no such command"), "{err}");
    let left = ok(&sock, &["kill", "sleep"]);
    assert!(!left.contains("\tsleep\t"), "{left}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["ps"]).contains("\tsleep\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!ok(&sock, &["ps"]).contains("\tsleep\t"));
    // a terminal's shell is listed too, named after what runs in it
    let t = ok(&sock, &["term", "new", "sleep", "60"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["ps"]).contains("\tsleep\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ps = ok(&sock, &["ps"]);
    let line = ps.lines().find(|l| l.contains("\tsleep\t")).unwrap_or_else(|| panic!("{ps}"));
    assert!(line.contains("-l -c 'sleep 60'"), "{line}");
    let _ = t;
}

#[test]
fn programs_say_what_they_are_called() {
    let sock = daemon();
    // `apex tool lsp` started by the server is called lsp, not apex,
    // in ps and Kill: the tool announces itself (Named) for its group
    let cmd = format!("{} tool lsp", env!("CARGO_BIN_EXE_apex"));
    ok(&sock, &["exec", &cmd]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&sock, &["ps"]).contains("\tlsp\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ps = ok(&sock, &["ps"]);
    let line = ps.lines().find(|l| l.contains("\tlsp\t")).unwrap_or_else(|| panic!("{ps}"));
    assert!(line.contains("tool lsp"), "{line}");
    assert!(!ps.contains("\tapex\t"), "{ps}");
    // the top row says lsp too, now, not at the next command
    let mut viewer = Remote::connect_as(&sock, "main", "viewer", AttachmentKind::Tool).unwrap();
    let top_text = |n: &Node| n.state.layout.top.and_then(|b| n.state.buffer(b).ok()).map(|b| b.text.to_string()).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !top_text(&viewer.node).starts_with("lsp ") && Instant::now() < deadline {
        let _ = viewer.step(Duration::from_millis(20));
    }
    assert!(top_text(&viewer.node).starts_with("lsp "), "{:?}", top_text(&viewer.node));
    assert!(!top_text(&viewer.node).contains("apex "), "{:?}", top_text(&viewer.node));
    drop(viewer);
    // a program of no known group is adopted for as long as it is connected
    let r = Remote::connect_as(&sock, "main", "orphan", AttachmentKind::Tool).unwrap();
    r.send(&apex_server::proto::ClientMsg::Named { name: "orphan".into(), group: 4_000_000, pid: 4_000_001, cmd: "orphan -x".into() });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["ps"]).contains("\torphan\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ps = ok(&sock, &["ps"]);
    let line = ps.lines().find(|l| l.contains("\torphan\t")).unwrap_or_else(|| panic!("{ps}"));
    assert!(line.starts_with("4000001\t") && line.contains("orphan -x"), "{line}");
    drop(r);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["ps"]).contains("\torphan\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!ok(&sock, &["ps"]).contains("\torphan\t"));
    // Kill by the announced name ends the tool
    let left = ok(&sock, &["kill", "lsp"]);
    assert!(!left.contains("\tlsp\t"), "{left}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["ps"]).contains("\tlsp\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!ok(&sock, &["ps"]).contains("\tlsp\t"));
}

#[test]
fn editor_opens_the_file_and_returns_when_its_window_goes() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-editor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let path = file.display().to_string();
    // $EDITOR is set for commands and terminals
    let env = ok(&sock, &["env"]);
    // one word, apex-editor beside a binary named apex; here the daemon is
    // the test binary, so the fallback `EXE editor`, unquoted
    let editor = env.lines().find(|l| l.starts_with("EDITOR=")).unwrap_or_else(|| panic!("{env}"));
    let value = editor.trim_start_matches("EDITOR=");
    assert!(!value.contains('\''), "{value}");
    let exe = value.trim_end_matches(" editor");
    assert!(std::path::Path::new(exe).is_file(), "{value}");
    // `apex-editor FILE` (the link $EDITOR names) is `apex editor FILE`,
    // and blocks while the window is open
    let link = dir.join("apex-editor");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_apex"), &link).unwrap();
    let (s2, p2) = (sock.clone(), path.clone());
    let child = std::thread::spawn(move || {
        let out = Command::new(&link).env("APEX_SOCKET", &s2).env("apexsession", "main").arg(&p2).output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
    });
    let mut c = Remote::connect_as(&sock, "main", "watcher", AttachmentKind::Tool).unwrap();
    let open = |r: &Remote| r.node.state.windows.keys().copied().find(|w| r.node.window_name(*w) == path);
    let deadline = Instant::now() + Duration::from_secs(5);
    while open(&c).is_none() && Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(20));
    }
    let w = open(&c).expect("the file's window");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!child.is_finished(), "editor returned while the window was open");
    // Del: the window goes and the editor returns
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() }, Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !child.is_finished() && Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(20));
    }
    assert!(child.is_finished(), "editor did not return after Del");
    let (success, _, err) = child.join().unwrap();
    assert!(success, "{err}");
    assert!(err.contains(&format!("editing {}", file.display())), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn newterm_shell_is_a_setting() {
    let sock = daemon();
    ok(&sock, &["set", "Newterm.shell", "/bin/sh"]);
    let t = ok(&sock, &["term", "new"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["ps"]).contains("/bin/sh -l") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ps = ok(&sock, &["ps"]);
    assert!(ps.contains("/bin/sh -l"), "{ps}");
    let _ = t;
}

#[test]
fn tunnels_and_fetches_go_through_the_host() {
    use apex_server::proto::IoFrame;
    use std::io::{Read, Write};
    let sock = daemon();
    // an echo service and a one-shot HTTP server, both on this machine
    let echo = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in echo.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 1024];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 || s.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = http.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in http.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut req = Vec::new();
                let mut buf = [0u8; 1024];
                // the head, then the body: as much as Content-Length says,
                // or chunks to the last (ureq sends a body chunked)
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                    if let Some(i) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&req[..i]).to_string();
                        let chunked = head.lines().any(|l| l.to_ascii_lowercase() == "transfer-encoding: chunked");
                        if chunked {
                            if req.ends_with(b"0\r\n\r\n") {
                                break;
                            }
                            continue;
                        }
                        let want: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ").or_else(|| l.strip_prefix("content-length: "))).and_then(|v| v.parse().ok()).unwrap_or(0);
                        if req.len() - (i + 4) >= want {
                            break;
                        }
                    }
                }
                let i = req.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(req.len());
                let head = String::from_utf8_lossy(&req[..i]).to_string();
                let raw = &req[(i + 4).min(req.len())..];
                // chunks undone: size line, data, blank
                let body: Vec<u8> = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
                    let mut out = Vec::new();
                    let mut rest = raw;
                    while let Some(nl) = rest.windows(2).position(|w| w == b"\r\n") {
                        let size = usize::from_str_radix(String::from_utf8_lossy(&rest[..nl]).trim(), 16).unwrap_or(0);
                        if size == 0 {
                            break;
                        }
                        let start = nl + 2;
                        out.extend_from_slice(&rest[start..(start + size).min(rest.len())]);
                        rest = &rest[(start + size + 2).min(rest.len())..];
                    }
                    out
                } else {
                    raw.to_vec()
                };
                let body = &body[..];
                let line = head.lines().next().unwrap_or("").to_string();
                let answer = format!("{line}|{}", String::from_utf8_lossy(body));
                let (status, answer) = if line.starts_with("GET /missing") { ("404 Not Found", "gone".to_string()) } else { ("200 OK", answer) };
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nX-Served: yes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
            });
        }
    });
    let mut c = Remote::connect_as(&sock, "main", "netter", AttachmentKind::Tool).unwrap();
    // CONNECT: bytes go in, the echo comes back, the far end's close ends it
    let t = c.io_open("CONNECT", &format!("127.0.0.1:{echo_port}"), &[]);
    assert_eq!(c.io_response(t, Duration::from_secs(5)).unwrap(), 200);
    c.io_send(t, b"ping");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = Vec::new();
    while got != b"ping" && Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(50));
        for f in c.io_take(t) {
            if let IoFrame::Body(b) = f {
                got.extend_from_slice(&b);
            }
        }
    }
    assert_eq!(got, b"ping");
    c.io_end(t);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ended = false;
    while !ended && Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(50));
        ended = c.io_take_ended(t);
    }
    assert!(ended, "the tunnel did not end after both sides closed");
    // a tunnel nobody listens on is refused with a 502
    let t = c.io_open("CONNECT", "127.0.0.1:1", &[]);
    let (status, body) = c.io_collect(t, Duration::from_secs(10)).unwrap();
    assert_eq!(status, 502, "{}", String::from_utf8_lossy(&body));
    // GET http://: fetched by the host, headers and body streamed back
    let g = c.io_open("GET", &format!("http://127.0.0.1:{http_port}/hello?x=1"), &[]);
    let (status, body) = c.io_collect(g, Duration::from_secs(5)).unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(String::from_utf8_lossy(&body), "GET /hello?x=1 HTTP/1.1|");
    // a body goes with a POST once the client ends its side; statuses pass through
    let p = c.io_open("POST", &format!("http://127.0.0.1:{http_port}/in"), &[("Content-Type", "text/plain")]);
    c.io_send(p, b"payload");
    c.io_end(p);
    let (status, body) = c.io_collect(p, Duration::from_secs(5)).unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(String::from_utf8_lossy(&body), "POST /in HTTP/1.1|payload");
    let m = c.io_open("GET", &format!("http://127.0.0.1:{http_port}/missing"), &[]);
    let (status, body) = c.io_collect(m, Duration::from_secs(5)).unwrap();
    assert_eq!((status, String::from_utf8_lossy(&body).to_string()), (404, "gone".to_string()));
    // and from the command line
    let out = ok(&sock, &["io", "GET", &format!("http://127.0.0.1:{http_port}/cli")]);
    assert_eq!(out, "GET /cli HTTP/1.1|");
    let mut nc = Command::new(env!("CARGO_BIN_EXE_apex")).arg(format!("-socket={}", sock.display())).args(["-session=main", "io", "CONNECT", &format!("127.0.0.1:{echo_port}")]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    nc.stdin.take().unwrap().write_all(b"over the wire\n").unwrap();
    std::thread::sleep(Duration::from_millis(800));
    nc.kill().unwrap();
    let out = nc.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "over the wire\n");
}

#[test]
fn a_web_views_proxy_and_files_ride_the_plane() {
    use std::io::{Read, Write};
    let sock = daemon();
    // an echo service to tunnel to
    let echo = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in echo.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 1024];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 || s.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    let c = Remote::connect_as(&sock, "main", "viewer", AttachmentKind::Tool).unwrap();
    let plane = c.io_plane();
    // the CONNECT proxy a web view is pointed at: a tunnel per connection,
    // over the plane, through the host
    let port = apex_server::plane::start_connect_proxy(plane.clone()).unwrap();
    let mut p = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(p, "CONNECT 127.0.0.1:{echo_port} HTTP/1.1\r\nHost: 127.0.0.1:{echo_port}\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while p.read(&mut b).unwrap() == 1 {
        head.push(b[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"), "{}", String::from_utf8_lossy(&head));
    p.write_all(b"through the host").unwrap();
    let mut got = vec![0u8; 16];
    p.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"through the host");
    drop(p);
    // the host's loopback under its alias, as a web view is made to ask
    let mut p = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(p, "CONNECT 127-0-0-1.apex-host:{echo_port} HTTP/1.1\r\n\r\n").unwrap();
    let mut head = Vec::new();
    while p.read(&mut b).unwrap() == 1 {
        head.push(b[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"), "{}", String::from_utf8_lossy(&head));
    p.write_all(b"alias").unwrap();
    let mut got = vec![0u8; 5];
    p.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"alias");
    drop(p);
    // a tunnel to nowhere is a 502 at the proxy
    let mut p = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(p, "CONNECT 127.0.0.1:1 HTTP/1.1\r\n\r\n").unwrap();
    let mut head = String::new();
    p.read_to_string(&mut head).unwrap();
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    // apexfile: a file on the host fetched on the plane by a thread
    let dir = std::env::temp_dir().join(format!("apex-cli-plane-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("page.html");
    std::fs::write(&file, "<h1>hi</h1>").unwrap();
    let (status, _, body) = plane.fetch("GET", &apex_server::remote::file_url(&file.display().to_string()), &[], None, Duration::from_secs(5)).unwrap();
    assert_eq!((status, String::from_utf8_lossy(&body).to_string()), (200, "<h1>hi</h1>".to_string()));
    let (status, _, _) = plane.fetch("GET", "file:///nowhere/at/all", &[], None, Duration::from_secs(5)).unwrap();
    assert_eq!(status, 404);
    assert_eq!(apex_server::plane::mime_for("/a/b.html"), "text/html; charset=utf-8");
    assert_eq!(apex_server::plane::mime_for("/a/b.PNG"), "image/png");
    // and a watch stream a thread reads: the file now, then the change
    let (stream, rx) = plane.open("GET", &apex_server::remote::file_url(&file.display().to_string()), &[("Watch", "1")]);
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), apex_server::proto::IoFrame::Response { status: 200, .. }));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), apex_server::proto::IoFrame::Body(_)));
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&file, "<h1>changed</h1>").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut changed = false;
    while !changed && Instant::now() < deadline {
        if let Ok(apex_server::proto::IoFrame::Body(b)) = rx.recv_timeout(Duration::from_millis(200)) {
            changed = apex_server::proto::FileFrame::decode(&b).is_some_and(|f| f.bytes == b"<h1>changed</h1>");
        }
    }
    assert!(changed, "no frame after the change");
    plane.end(stream);
    plane.close(stream);
    let _ = std::fs::remove_dir_all(&dir);
    drop(c);
}

#[test]
fn preview_is_a_live_pipe_through_a_converter() {
    let sock = daemon();
    // the rules the settings derive: the defaults, then a converter of our own
    let rules = ok(&sock, &["plumb", "rule", "ls"]);
    assert!(rules.contains("-verb=Preview") && rules.contains(r"\.md$") && rules.contains("tool preview $file"), "{rules}");
    assert!(!rules.contains(r"\.txt$"), "{rules}");
    ok(&sock, &["set", "Preview.txt", "sed 's/one/ONE/; s/^/<p>/; s/$/<\\/p>/'"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["plumb", "rule", "ls"]).contains(r"\.txt$") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ok(&sock, &["plumb", "rule", "ls"]).contains(r"\.txt$"));
    // a file, not open: the tool opens it, makes FILE+Preview beside it
    // with the converter's output, live
    let dir = std::env::temp_dir().join(format!("apex-cli-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one\n").unwrap();
    let path = file.display().to_string();
    let mut tool = Command::new(env!("CARGO_BIN_EXE_apex")).arg(format!("-socket={}", sock.display())).args(["-session=main", "tool", "preview", &path]).stderr(std::process::Stdio::piped()).spawn().unwrap();
    let mut c = Remote::connect_as(&sock, "main", "watcher", AttachmentKind::Tool).unwrap();
    let preview = format!("{path}+Preview");
    let find = |r: &Remote, name: &str| r.node.state.windows.keys().copied().find(|w| r.node.window_name(*w) == name);
    let text_of = |r: &Remote, w: WindowId| r.node.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| r.node.state.buffer(b).ok()).map(|b| b.text.to_string()).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(50));
        if find(&c, &preview).is_some_and(|w| text_of(&c, w).contains("<p>ONE</p>")) {
            break;
        }
    }
    let src = find(&c, &path).expect("the file opened");
    let page = find(&c, &preview).expect("a preview window");
    assert_eq!(text_of(&c, page), "<p>ONE</p>\n");
    assert!(matches!(c.node.state.window(page).unwrap().body, Body::Html(_)));
    assert!(c.node.window_live(page), "the page is live while the tool runs");
    // an edit to the source: the page follows, unsaved
    let b = c.node.state.window(src).unwrap().body_buffer().unwrap();
    let version = c.node.state.buffer(b).unwrap().version;
    c.propose(apex_server::Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: 3, q1: 3, text: " two".into() }, Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && text_of(&c, page) != "<p>ONE two</p>\n" {
        let _ = c.step(Duration::from_millis(50));
    }
    assert_eq!(text_of(&c, page), "<p>ONE two</p>\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\n", "the file itself is untouched");
    // the tool is a command named preview; Del of the page ends it
    assert!(ok(&sock, &["ps"]).contains("\tpreview\t"), "{}", ok(&sock, &["ps"]));
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(page), text: "Del".into() }, Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && tool.try_wait().unwrap().is_none() {
        let _ = c.step(Duration::from_millis(50));
    }
    let status = tool.try_wait().unwrap().expect("the tool exits with the page");
    assert!(status.success(), "{status}");
    // no converter: a plain complaint
    let odd = dir.join("thing.zzz");
    std::fs::write(&odd, "x").unwrap();
    let (success, _, err) = apex(&sock, &["tool", "preview", &odd.display().to_string()]);
    assert!(!success && err.contains("no converter for .zzz"), "{err}");
    // apex md: a page
    let md = Command::new(env!("CARGO_BIN_EXE_apex")).args(["md"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    {
        use std::io::Write;
        md.stdin.as_ref().unwrap().write_all(b"# Title\n\n- [x] done\n\n| a | b |\n|---|---|\n| 1 | 2 |\n").unwrap();
    }
    let out = String::from_utf8_lossy(&md.wait_with_output().unwrap().stdout).to_string();
    assert!(out.starts_with("<!doctype html>") && out.contains("<h1>Title</h1>") && out.contains("<table>") && out.contains("checked"), "{out}");
    // a marker with the source line before every block: the title on 1,
    // the list item on 3, the table on 5
    let flat = out.replace('\n', "");
    assert!(flat.contains(r#"<span class="apex-line" data-line="1"></span><h1>Title</h1>"#), "{out}");
    assert!(flat.contains(r#"data-line="3"></span><li>"#), "{out}");
    assert!(flat.contains(r#"data-line="5"></span><table>"#), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_session_can_be_ended() {
    let sock = daemon();
    ok(&sock, &["new-session", "side"]);
    // something running in it, a tool attached to it, and an unsaved window
    ok(&sock, &["-session=side", "exec", "sleep 30"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&sock, &["-session=side", "ps"]).contains("\tsleep\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut tool = Remote::connect_as(&sock, "side", "watcher", AttachmentKind::Tool).unwrap();
    let dir = std::env::temp_dir().join(format!("apex-cli-end-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("draft.txt");
    std::fs::write(&file, "x\n").unwrap();
    ok(&sock, &["-session=side", "open", &file.display().to_string()]);
    ok(&sock, &["-session=side", "edit", &file.display().to_string(), ",x/x/c/y/"]);
    // unsaved: refused, and still there
    let (success, _, err) = apex(&sock, &["end-session", "side"]);
    assert!(!success && err.contains("unsaved"), "{err}");
    assert_eq!(labels(&ok(&sock, &["ls"])), vec!["main", "side"]);
    // forced: gone, the tool told and cut off, the command killed
    ok(&sock, &["end-session", "-f", "side"]);
    assert_eq!(labels(&ok(&sock, &["ls"])), vec!["main"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ended = None;
    let mut closed = false;
    while Instant::now() < deadline && !(ended.is_some() && closed) {
        match tool.step(Duration::from_millis(50)) {
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => closed = true,
        }
        if ended.is_none() {
            ended = tool.link.ended.clone();
        }
    }
    assert_eq!(ended.as_deref(), Some("side"));
    assert!(closed, "the tool's link should have closed");
    let (success, _, err) = apex(&sock, &["-session=side", "ps"]);
    assert!(!success, "{err}");
    // the sleep is gone with its session (its group was signalled)
    std::thread::sleep(Duration::from_millis(300));
    let alive = std::process::Command::new("pgrep").args(["-f", "sleep 30"]).output().map(|o| String::from_utf8_lossy(&o.stdout).lines().count()).unwrap_or(0);
    let _ = alive; // other tests may run sleeps of their own: not asserted
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mirror_holds_what_is_outstanding_not_the_history() {
    let sock = daemon();
    let mut c = Remote::connect_as(&sock, "main", "watcher", AttachmentKind::Tool).unwrap();
    let t = ok(&sock, &["term", "new"]);
    let term = TermId(t.trim().parse().unwrap());
    // a terminal spewing: thousands of lines of output
    ok(&sock, &["term", "send", t.trim(), "seq 1 20000; echo spew-done\n"]);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut peak = 0;
    let mut done = false;
    while Instant::now() < deadline && !done {
        let _ = c.step(Duration::from_millis(20));
        peak = peak.max(c.log.len());
        done = c.node.state.terms.get(&term).is_some_and(|t| t.grid.iter().any(|r| r.iter().map(|c| c.ch).collect::<String>().contains("spew-done")));
    }
    assert!(done, "the output never arrived");
    // the mirror applied everything and kept next to nothing
    assert!(c.log.len() < 50, "{} entries kept", c.log.len());
    assert!(peak < 2000, "the mirror grew to {peak} entries while the terminal spewed");
}

#[test]
fn a_script_is_over_when_it_exits_and_what_it_left_behind_is_its_own() {
    // a profile that starts the lsp tool in the background: the tool
    // holds the profile's pipes, but the profile is over when its shell
    // exits, and the tool is listed as lsp in its own right
    let apex = env!("CARGO_BIN_EXE_apex");
    let dir = std::env::temp_dir().join(format!("apex-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let host_profile = dir.join("profile");
    std::fs::write(&host_profile, format!("{apex} tool lsp &\n")).unwrap();
    let sock = daemon_with(Some(host_profile));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&sock, &["ps"]).contains("\tlsp\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["ps"]).contains("\tprofile\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ps = ok(&sock, &["ps"]);
    assert!(ps.contains("\tlsp\t") && !ps.contains("\tprofile\t") && !ps.contains("\tapex\t"), "{ps}");
    // and it can be ended by that name
    ok(&sock, &["kill", "lsp"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["ps"]).contains("\tlsp\t") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!ok(&sock, &["ps"]).contains("\tlsp\t"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_profiles_environment_at_its_end_is_the_sessions() {
    let dir = std::env::temp_dir().join(format!("apex-penv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let host_profile = dir.join("profile");
    // variables, a list, a function, and an unset; then exit, which the hook survives
    std::fs::write(&host_profile, "FOO=bar\nx=(a b)\nfn g { echo hi $* }\nEDITOR=()\nexit\n").unwrap();
    let sock = daemon_with(Some(host_profile));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&sock, &["env"]).contains("FOO=bar\n") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let env = ok(&sock, &["env"]);
    assert!(env.contains("FOO=bar\n"), "{env}");
    assert!(env.contains("x=a\u{1}b\n"), "{env}");
    assert!(env.contains("fn#g={echo hi $*}\n"), "{env}");
    assert!(!env.contains("EDITOR="), "{env}");
    // the shell's own bookkeeping is not the session's
    assert!(!env.contains("\npid=") && !env.contains("\nstatus="), "{env}");
    assert!(env.contains("apexsession=main\n"), "{env}");
    // a command started now has the function
    ok(&sock, &["exec", "g there"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !apex(&sock, &["text", "read", "+Errors"]).1.contains("hi there") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let errors = apex(&sock, &["text", "read", "+Errors"]).1;
    assert!(errors.contains("hi there\n"), "{errors}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `apex tool bridge NAME`: a tool in any language, over JSON lines. A
/// window made and written, a verb offered and answered, a watched
/// window's edits by others reported, a deletion seen.
#[test]
fn the_bridge_speaks_json_for_tools() {
    use std::io::{BufRead, BufReader, Write};
    fn next(out: &mut BufReader<std::process::ChildStdout>) -> serde_json::Value {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }
    fn call(stdin: &mut std::process::ChildStdin, out: &mut BufReader<std::process::ChildStdout>, cmd: serde_json::Value) -> serde_json::Value {
        writeln!(stdin, "{cmd}").unwrap();
        let v = next(out);
        assert!(v.get("id").is_some(), "unexpected event while waiting: {v}");
        v
    }
    let sock = daemon();
    let mut bridge = Command::new(env!("CARGO_BIN_EXE_apex"))
        .arg(format!("-socket={}", sock.display()))
        .args(["-session=main", "tool", "bridge", "t"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let mut out = BufReader::new(bridge.stdout.take().unwrap());
    let hello = next(&mut out);
    assert_eq!(hello["event"], "hello", "{hello}");
    assert_eq!(hello["tool"], "t");
    // a command is answered with its id; a bad one says what is wrong
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 1, "cmd": "nothing" }));
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("no such command"));
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 2, "cmd": "new", "name": "/tmp/bridge-notes" }));
    assert_eq!(r["ok"], true, "{r}");
    let w = r["window"].as_u64().unwrap();
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 3, "cmd": "write", "window": w, "q0": -1, "q1": -1, "text": "hello\n" }));
    assert_eq!(r["ok"], true, "{r}");
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 4, "cmd": "read", "window": w }));
    assert_eq!(r["text"], "hello\n", "{r}");
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 5, "cmd": "windows" }));
    assert!(r["windows"].as_array().unwrap().iter().any(|x| x["id"] == w && x["name"] == "/tmp/bridge-notes"), "{r}");
    // a verb offered in that window: B2 on it comes back as a plumb event
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 6, "cmd": "rule", "verb": "Shout", "window": w }));
    assert_eq!(r["ok"], true, "{r}");
    let rule = r["rule"].as_u64().unwrap();
    assert!(ok(&sock, &["plumb", "rule", "ls"]).contains(&format!("-verb=Shout -win={w} -tool=t")), "{}", ok(&sock, &["plumb", "rule", "ls"]));
    ok(&sock, &["exec", &w.to_string(), "Shout loud"]);
    let ev = next(&mut out);
    assert_eq!(ev["event"], "plumb", "{ev}");
    assert_eq!(ev["rule"], rule);
    assert_eq!(ev["verb"], "Shout");
    assert_eq!(ev["text"], "loud");
    assert_eq!(ev["window"], w);
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 7, "cmd": "ack", "plumb": ev["plumb"], "ok": true }));
    assert_eq!(r["ok"], true);
    // watched: an edit by someone else is reported, ours is not
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 8, "cmd": "watch", "window": w }));
    assert_eq!(r["ok"], true);
    let mut other = Remote::connect_as(&sock, "main", "other", AttachmentKind::Tool).unwrap();
    let b = other.node.state.window(WindowId(w)).unwrap().body_buffer().unwrap();
    let version = other.node.state.buffer(b).unwrap().version;
    other.propose(apex_server::Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: 6, q1: 6, text: "typed\n".into() }, Duration::from_secs(5)).unwrap();
    let ev = next(&mut out);
    assert_eq!(ev["event"], "edit", "{ev}");
    assert_eq!(ev["window"], w);
    assert_eq!(ev["text"], "typed\n");
    assert_eq!(ev["q0"], 6);
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 9, "cmd": "read", "window": w }));
    assert_eq!(r["text"], "hello\ntyped\n");
    // deleted (live, so Del does not ask): seen
    let r = call(&mut stdin, &mut out, serde_json::json!({ "id": 10, "cmd": "live", "window": w, "on": true }));
    assert_eq!(r["ok"], true, "{r}");
    ok(&sock, &["win", "del", &w.to_string()]);
    let ev = next(&mut out);
    assert_eq!(ev["event"], "deleted", "{ev}");
    assert_eq!(ev["window"], w);
    drop(stdin);
    let ev = next(&mut out);
    assert_eq!(ev["event"], "bye", "{ev}");
    assert!(bridge.wait().unwrap().success());
}

/// Sessions are known by their identity: a UUID minted when they are
/// made, in $apexsession (the label in $apexsessionlabel), accepted
/// wherever a session is named (whole, a prefix, or the label), stable
/// across a rename, and naming windows anywhere as `id.N`.
#[test]
fn sessions_have_an_identity_and_labels_for_people() {
    let sock = daemon();
    let ls = ok(&sock, &["ls"]);
    let (label, id) = ls.trim().split_once('\t').unwrap();
    assert_eq!(label, "main");
    assert_eq!(id.len(), 36, "{id}");
    // commands in the session know both
    let env = ok(&sock, &["env"]);
    assert!(env.contains(&format!("apexsession={id}\n")), "{env}");
    assert!(env.contains("apexsessionlabel=main\n"), "{env}");
    // the id, a prefix of it, or the label all name it
    let w = ok(&sock, &[&format!("-session={id}"), "new", "/tmp/ident"]).trim().to_string();
    assert!(ok(&sock, &[&format!("-session={}", &id[..8]), "win", "list"]).contains("/tmp/ident"));
    assert!(ok(&sock, &["-session=main", "win", "list"]).contains("/tmp/ident"));
    // a window named anywhere: id.N, in its session
    assert!(ok(&sock, &["text", "read", &format!("{id}.{w}")]).is_empty());
    assert!(ok(&sock, &["text", "read", &format!("{}.{w}", &id[..6])]).is_empty());
    // a rename changes the label, not the identity; the old label is gone
    ok(&sock, &["rename-session", "main", "renamed"]);
    let ls = ok(&sock, &["ls"]);
    assert_eq!(ls.trim(), format!("renamed\t{id}"), "{ls}");
    assert!(ok(&sock, &[&format!("-session={id}"), "env"]).contains("apexsessionlabel=renamed\n"));
    let (success, _, err) = apex(&sock, &["-session=main", "win", "list"]);
    assert!(!success && err.contains("no session"), "{err}");
    // a second session has its own; ending one by id leaves the other
    ok(&sock, &["new-session", "side"]);
    let ls = ok(&sock, &["ls"]);
    let side = ls.lines().find(|l| l.starts_with("side\t")).unwrap().split('\t').nth(1).unwrap().to_string();
    assert_ne!(side, id);
    ok(&sock, &["end-session", "-f", &side]);
    assert_eq!(labels(&ok(&sock, &["ls"])), vec!["renamed"]);
}
