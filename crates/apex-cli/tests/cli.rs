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

fn ok(sock: &PathBuf, args: &[&str]) -> String {
    let (success, out, err) = apex(sock, args);
    assert!(success, "apex {args:?} failed: {err}");
    out
}

#[test]
fn scripts_drive_a_headless_session() {
    let sock = daemon();
    assert_eq!(ok(&sock, &["ls"]), "main\n");
    ok(&sock, &["new-session", "side"]);
    assert_eq!(ok(&sock, &["ls"]), "main\nside\n");

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
    // a session made from elsewhere: the host's file, then the creator's script
    let profile = apex_server::proto::Script { client: "tester".into(), text: format!("{apex} open {}/client.txt\n{apex} env FROM=client\n", dir.display()) };
    apex_server::remote::new_session(&sock, "s2", Some(profile)).unwrap();
    let list = |sock: &PathBuf| ok(sock, &["-session=s2", "win", "list"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(list(&sock).contains("host.txt") && list(&sock).contains("client.txt")) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let l = list(&sock);
    let id = |name: &str| l.lines().find(|x| x.ends_with(name)).and_then(|x| x.split('\t').next()).and_then(|n| n.parse::<u64>().ok()).unwrap_or_else(|| panic!("{name} in {l}"));
    assert!(id("host.txt") < id("client.txt"), "host first:\n{l}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&sock, &["-session=s2", "env"]).contains("FROM=client") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let env = ok(&sock, &["-session=s2", "env"]);
    assert!(env.contains("FROM=client\n"), "{env}");
    assert!(env.contains("ORDER=s2\n"), "{env}");
    assert!(env.contains("apexclient=tester\n"), "{env}");
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
        if grid.contains("v=client") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(grid.contains("v=client"), "grid:\n{grid}");
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
        let rule = PlumbRule { verb: "plumb".into(), text: None, file: None, kind: None, isfile: None, isdir: None, action: RuleAction::Tool("t".into()), to: None };
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
        let rule = PlumbRule { verb: "plumb".into(), text: None, file: None, kind: None, isfile: None, isdir: None, action: RuleAction::Tool("t".into()), to: None };
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
    // the bytes now
    assert_eq!(c.watch(&p, Duration::from_secs(5)).unwrap(), b"one\n");
    // and again when the file changes
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&path, "two\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = None;
    while Instant::now() < deadline && got.is_none() {
        let _ = c.step(Duration::from_millis(100));
        got = c.link.files.iter().position(|(q, b)| *q == p && b.as_deref() == Ok(b"two\n")).map(|i| c.link.files.remove(i));
    }
    assert!(got.is_some(), "no update after the change");
    // not after unwatch (settled: the change's own events all arrived)
    c.unwatch(&p);
    let settle = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settle {
        let _ = c.step(Duration::from_millis(50));
    }
    c.link.files.clear();
    std::fs::write(&path, "three\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(100));
    }
    assert!(!c.link.files.iter().any(|(q, b)| *q == p && b.as_deref() == Ok(b"three\n")), "still streaming after unwatch");
    let _ = std::fs::remove_dir_all(&dir);
}
