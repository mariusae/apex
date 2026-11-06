//! A daemon's sessions outlive it (`persist`): stopped and started
//! again, it has them back -- the same session, its windows and their
//! unsaved text, its terminals with their old output above a new shell.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::ClientMsg;
use apex_server::remote::Remote;

fn scratch(what: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("apex-persist-{what}-{}-{n}", std::process::id()))
}

/// A daemon on `sock` keeping its sessions in `dumps`.
fn daemon(sock: &Path, dumps: &Path) {
    let (p, d) = (sock.to_path_buf(), dumps.to_path_buf());
    std::thread::spawn(move || Daemon::run_keeping(&p, "main", None, Some(d)).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(sock).is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn stop(sock: &Path) {
    apex_server::remote::stop(sock).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(sock).is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait(r: &mut Remote, mut done: impl FnMut(&Remote) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done(r) {
            return true;
        }
        match r.step(Duration::from_millis(20)) {
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    done(r)
}

fn term_of(r: &Remote) -> Option<TermId> {
    r.node.state.windows.values().find_map(|w| match w.body {
        Body::Term(t) => Some(t),
        _ => None,
    })
}

fn grid(r: &Remote, t: TermId) -> String {
    r.node.state.terms.get(&t).map(|t| t.grid.iter().map(|r| r.iter().map(|c| c.ch).collect::<String>()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

#[test]
fn a_session_outlives_its_daemon() {
    let (sock, dumps, files) = (scratch("sock"), scratch("dumps"), scratch("files"));
    std::fs::create_dir_all(&files).unwrap();
    let path = files.join("f.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();

    daemon(&sock, &dumps);
    let mut c = Remote::connect(&sock, "main", "c1").unwrap();
    let id = c.node.state.meta.id.clone();
    let col = c.node.state.layout.cols[0].id;
    // a file, changed and not put
    c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: path.to_string_lossy().to_string() });
    assert!(wait(&mut c, |r| r.node.state.windows.len() == 1));
    let w = *c.node.state.windows.keys().next().unwrap();
    let v = ViewId::Body(w);
    c.node.select(&mut c.log, v, 0, 0).unwrap();
    c.node.insert(&mut c.log, v, "zero\n").unwrap();
    // a terminal, with something said in it
    c.node.exec(&mut c.log, ExecCtx::Top, "Newterm").unwrap();
    c.flush();
    assert!(wait(&mut c, |r| term_of(r).is_some()));
    let t = term_of(&c).unwrap();
    for ch in "echo persist-$((6*7))\r".chars() {
        c.send(&ClientMsg::TermKey { term: t, key: apex_server::TermKey { key: ch.to_string(), text: Some(ch.to_string()), shift: false, control: false, alt: false } });
    }
    assert!(wait(&mut c, |r| grid(r, t).contains("persist-42")), "grid:\n{}", grid(&c, t));
    let b = c.node.view_buffer(v).unwrap();
    let want = c.log.last_seq(Shard::Buffer(b));
    assert!(wait(&mut c, |r| r.acked(Shard::Buffer(b)) == want));
    drop(c);

    // stopped, it writes its session down
    stop(&sock);
    let dump = dumps.join(format!("{id}.json"));
    assert!(dump.exists(), "no dump at {}", dump.display());

    // and started again, has it back
    daemon(&sock, &dumps);
    let mut c = Remote::connect(&sock, "main", "c2").unwrap();
    assert_eq!(c.node.state.meta.id, id, "the same session");
    let w = c.node.state.windows.values().find(|w| matches!(w.body, Body::Text(_))).map(|w| w.id).expect("the file's window");
    let b = c.node.state.buffer(c.node.state.window(w).unwrap().body_buffer().unwrap()).unwrap();
    assert_eq!(b.name, path.to_string_lossy());
    assert_eq!(b.text.to_string(), "zero\none\ntwo\n");
    assert!(b.dirty(), "unsaved still");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n", "the file untouched");
    // the terminal: its old output, the line saying so, a new shell
    assert!(wait(&mut c, |r| term_of(r).is_some_and(|t| grid(r, t).contains("Restored session"))), "grid:\n{}", term_of(&c).map(|t| grid(&c, t)).unwrap_or_default());
    let t = term_of(&c).unwrap();
    assert!(grid(&c, t).contains("persist-42"), "grid:\n{}", grid(&c, t));
    for ch in "echo again-$((6*7))\r".chars() {
        c.send(&ClientMsg::TermKey { term: t, key: apex_server::TermKey { key: ch.to_string(), text: Some(ch.to_string()), shift: false, control: false, alt: false } });
    }
    assert!(wait(&mut c, |r| grid(r, t).contains("again-42")), "grid:\n{}", grid(&c, t));

    // ended, it is forgotten
    c.send(&ClientMsg::EndSession { name: "main".into(), force: true });
    let deadline = Instant::now() + Duration::from_secs(5);
    while dump.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!dump.exists(), "the dump of an ended session is gone");
    drop(c);
    stop(&sock);
    let _ = std::fs::remove_dir_all(&files);
    let _ = std::fs::remove_dir_all(&dumps);
}

#[test]
fn a_dump_that_will_not_load_is_set_aside() {
    let (sock, dumps) = (scratch("sock"), scratch("dumps"));
    std::fs::create_dir_all(&dumps).unwrap();
    std::fs::write(dumps.join("broken.json"), "{ not a dump").unwrap();
    daemon(&sock, &dumps);
    // the daemon starts, its own session there
    let c = Remote::connect(&sock, "main", "c1").unwrap();
    assert!(!c.node.state.layout.cols.is_empty());
    drop(c);
    assert!(dumps.join("broken.json.bad").exists());
    assert!(!dumps.join("broken.json").exists());
    stop(&sock);
    let _ = std::fs::remove_dir_all(&dumps);
}

#[test]
fn dump_writes_a_session_and_load_makes_one_of_it() {
    let (sock, dumps, files) = (scratch("sock"), scratch("dumps"), scratch("files"));
    std::fs::create_dir_all(&files).unwrap();
    let path = files.join("g.txt");
    std::fs::write(&path, "alpha\n").unwrap();
    daemon(&sock, &dumps);
    let mut c = Remote::connect(&sock, "main", "c1").unwrap();
    let col = c.node.state.layout.cols[0].id;
    c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: path.to_string_lossy().to_string() });
    assert!(wait(&mut c, |r| r.node.state.windows.len() == 1));
    let w = *c.node.state.windows.keys().next().unwrap();
    c.node.select(&mut c.log, ViewId::Body(w), 0, 0).unwrap();
    c.node.insert(&mut c.log, ViewId::Body(w), "beta ").unwrap();
    // Dump, relative to the window's directory
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Dump saved.dump").unwrap();
    c.flush();
    let file = files.join("saved.dump");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !file.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let d: apex_core::dump::Dump = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(d.label, "main");
    assert_eq!(d.windows.len(), 1);
    // Load: a session of its own, beside this one
    c.node.exec(&mut c.log, ExecCtx::Window(w), "Load saved.dump").unwrap();
    c.flush();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut other = None;
    while other.is_none() && Instant::now() < deadline {
        other = Remote::connect(&sock, "main-2", "c2").ok();
        if other.is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let other = other.expect("a session main-2");
    assert_ne!(other.node.state.meta.id, c.node.state.meta.id);
    let w2 = other.node.state.windows.values().next().unwrap().id;
    let b = other.node.state.buffer(other.node.state.window(w2).unwrap().body_buffer().unwrap()).unwrap();
    assert_eq!(b.text.to_string(), "beta alpha\n");
    assert!(b.dirty());
    drop((c, other));
    stop(&sock);
    let _ = std::fs::remove_dir_all(&files);
    let _ = std::fs::remove_dir_all(&dumps);
}
