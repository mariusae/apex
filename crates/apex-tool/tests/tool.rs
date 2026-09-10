//! The tool API against a daemon on a thread: a window made and
//! written, a verb offered and answered with its range, a watched
//! window's edits by others, a deletion.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::remote::Remote;
use apex_tool::{Event, Rule, Tool, END};

fn daemon() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-tool-test-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

#[test]
fn a_tool_works_a_window_and_answers_its_verb() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "shout").unwrap();
    assert_eq!(t.name(), "shout");
    let w = t.new_window("/tmp/shout-notes").unwrap();
    t.append(w, "hello\n").unwrap();
    t.replace(w, 0, 1, "H").unwrap();
    assert_eq!(t.read(w).unwrap(), "Hello\n");
    assert!(t.windows().iter().any(|x| x.id == w && x.name == "/tmp/shout-notes"));
    // a verb offered in that window, run there: a Plumb event with the
    // window's dot as its range
    let shout = t.offer(Rule::verb("Shout").window(w)).unwrap();
    t.select(w, 0, 5).unwrap();
    // another attachment runs it, as B2 would
    let mut other = Remote::connect_as(&sock, "main", "other", AttachmentKind::Tool).unwrap();
    other.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Shout loud".into() }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("an event");
    let Event::Plumb(p) = ev else { panic!("{ev:?}") };
    assert_eq!((p.rule, p.verb.as_str(), p.text.as_str(), p.window), (shout, "Shout", "loud", Some(w)));
    assert_eq!(p.range().map(|r| (r.q0, r.q1)), Some((0, 5)));
    t.append(w, &format!("{}\n", p.text.to_uppercase())).unwrap();
    t.answer(&p, true).unwrap();
    assert_eq!(t.read(w).unwrap(), "Hello\nLOUD\n");
    // show: a place brought on screen, dot left where it was
    assert_eq!(t.line(w, 2).map(|r| (r.q0, r.q1)).unwrap(), (6, 10));
    t.show_line(w, 2).unwrap();
    t.show(w, 0).unwrap();
    assert!(t.line(w, 4).is_err());
    assert_eq!(t.selection(w).map(|r| (r.q0, r.q1)).unwrap(), (0, 5));
    // watched: an edit by someone else is an event; ours is not
    t.watch(w).unwrap();
    let b = other.node.state.window(w).unwrap().body_buffer().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while other.node.state.buffer(b).unwrap().text.to_string() != "Hello\nLOUD\n" && Instant::now() < deadline {
        let _ = other.step(Duration::from_millis(20));
    }
    let version = other.node.state.buffer(b).unwrap().version;
    other.propose(apex_server::Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: END.min(11), q1: 11, text: "typed\n".into() }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("an edit");
    assert_eq!(ev, Event::Edit(apex_tool::Edit { window: w, q0: 11, nd: 0, text: "typed\n".into() }));
    // a rename and a deletion are events too (live, so Del does not ask)
    t.rename(w, "/tmp/shout-renamed").unwrap();
    t.set_live(w, true).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("a rename");
    assert_eq!(ev, Event::Renamed { window: w, name: "/tmp/shout-renamed".into() });
    other.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("a deletion");
    assert_eq!(ev, Event::Deleted { window: w });
    // nothing more, and the session goes on
    assert_eq!(t.next_event(Some(Duration::from_millis(200))).unwrap(), None);
    assert!(t.window_name(w).is_none());
}
