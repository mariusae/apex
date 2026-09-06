//! The `apex` command end to end against a daemon on a thread.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use apex_server::daemon::Daemon;

fn daemon() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-cli-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run(&p, "main").unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

fn apex(sock: &PathBuf, args: &[&str]) -> (bool, String, String) {
    // the test daemon's session is "main"; a later --session in `args` wins
    let out = Command::new(env!("CARGO_BIN_EXE_apex")).arg("--socket").arg(sock).args(["--session", "main"]).args(args).output().unwrap();
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
    let listed = ok(&sock, &["new", &p]);
    assert!(listed.contains(&p), "{listed}");
    let wins = ok(&sock, &["win", "list"]);
    assert!(wins.contains("notes.txt"), "{wins}");
    assert_eq!(ok(&sock, &["text", "read", "notes.txt"]), "pear\napple\nfig\n");
    assert_eq!(ok(&sock, &["text", "read", "notes.txt", "--addr", "2"]), "apple\n");

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

    // a pipe through the shell, as B2 would
    ok(&sock, &["exec", "notes.txt", "|sort"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ok(&sock, &["text", "read", "notes.txt"]) != "pear\napple\nkiwi\n".replace("apple", "apple") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

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
    let (_, wins, _) = apex(&sock, &["--session", "side", "win", "list"]);
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
        .arg("--socket")
        .arg(&sock)
        .args(["attach", "--stdio"])
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
