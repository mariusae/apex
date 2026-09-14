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

/// A window a program writes: the dot follows what goes in at the point
/// it sits at, so nothing need be clicked to go on typing at the end,
/// and a dot anywhere else -- in a draft being typed -- is left in it.
#[test]
fn output_carries_the_dot_along_but_leaves_a_draft_alone() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "acp").unwrap();
    let w = t.new_window("/tmp/acp-out").unwrap();
    let dot = |t: &Tool| t.selection(w).map(|r| (r.q0, r.q1)).unwrap();
    assert_eq!(dot(&t), (0, 0));
    // output at the dot takes it with it, write after write
    t.insert_following(w, 0, "one\n").unwrap();
    assert_eq!(dot(&t), (4, 4));
    t.insert_following(w, 4, "two\n").unwrap();
    assert_eq!(dot(&t), (8, 8));
    // a draft at the end, the cursor in it, past the output point
    t.replace(w, END, END, "draft").unwrap();
    t.select(w, 13, 13).unwrap();
    // output goes in before it: the draft moves along and keeps the
    // cursor, which does not jump back to where the output ended
    t.insert_following(w, 8, "three\n").unwrap();
    assert_eq!(t.read(w).unwrap(), "one\ntwo\nthree\ndraft");
    assert_eq!(dot(&t), (19, 19));
    // and `replace` moves nothing, wherever the dot is
    t.select(w, 0, 0).unwrap();
    t.replace(w, 0, 0, "zero\n").unwrap();
    assert_eq!(dot(&t), (0, 0));
}
/// A rule may name the tool that owns a window, and so speak to that
/// tool's windows and no others, where a name pattern would be guessing.
#[test]
fn a_rule_may_name_the_tool_that_owns_the_window() {
    let sock = daemon();
    let mut win = Tool::attach_to(&sock, "main", "win-42").unwrap();
    let mut acp = Tool::attach_to(&sock, "main", "acp").unwrap();
    let a = win.new_window("/tmp/proj/-sh").unwrap();
    let b = acp.new_window("/tmp/proj/-claude").unwrap();
    win.set_owner(a, true).unwrap();
    acp.set_owner(b, true).unwrap();
    // the client's Snarfout rule: win's windows, whatever they are called
    win.offer(Rule::verb("Snarfout").owner("win-.*")).unwrap();
    let mut c = Remote::connect_as(&sock, "main", "watch", AttachmentKind::Tool).unwrap();
    let menu = |c: &mut Remote, w: WindowId| {
        for _ in 0..40 {
            let _ = c.step(Duration::from_millis(20));
        }
        apex_core::plumb::verbs_for(&c.node.state.meta.rules, &c.node.window_name(w), c.node.window_kind(w), Some(w), c.node.window_owner(w))
    };
    assert_eq!(menu(&mut c, a), vec!["Snarfout"]);
    assert_eq!(c.node.window_owner(a), Some("win-42"));
    assert_eq!(c.node.window_owner(b), Some("acp"));
    // ... and not the agent's, though it is named the same way
    assert!(menu(&mut c, b).is_empty());
    // a window no tool owns is owned by nobody: the empty name, which
    // is how the lsp's Back and Fwd say they are for real files
    let f = acp.new_window("/tmp/proj/main.rs").unwrap();
    acp.offer(Rule::verb("Back").owner("")).unwrap();
    assert_eq!(menu(&mut c, f), vec!["Back"]);
    assert_eq!(menu(&mut c, a), vec!["Snarfout"]);
    assert!(menu(&mut c, b).is_empty());
}
/// A verb that wants a place or an argument is offered `unlisted`: it
/// runs when B2 takes it, but is no word in the window's tools menu.
#[test]
fn an_unlisted_verb_works_but_is_not_in_the_menu() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "acp").unwrap();
    let w = t.new_window("/tmp/acp-notes").unwrap();
    t.offer(Rule::verb("Send").window(w)).unwrap();
    let allow = t.offer(Rule::verb("Allow").window(w).unlisted()).unwrap();
    // the menu has the one, not the other
    let mut other = Remote::connect_as(&sock, "main", "other", AttachmentKind::Tool).unwrap();
    let menu = |r: &Remote| apex_core::plumb::verbs_for(&r.node.state.meta.rules, "/tmp/acp-notes", WinKind::File, Some(w), r.node.window_owner(w));
    let deadline = Instant::now() + Duration::from_secs(5);
    while menu(&other).is_empty() && Instant::now() < deadline {
        let _ = other.step(Duration::from_millis(20));
    }
    assert_eq!(menu(&other), vec!["Send"]);
    // but B2 on the word still reaches the tool, arguments and all
    other.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Allow once".into() }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("an event");
    let Event::Plumb(p) = ev else { panic!("{ev:?}") };
    assert_eq!((p.rule, p.verb.as_str(), p.text.as_str()), (allow, "Allow", "once"));
    t.answer(&p, true).unwrap();
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
    t.append(w, "mine\n").unwrap();
    assert_eq!(t.next_event(Some(Duration::from_millis(300))).unwrap(), None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while other.node.state.buffer(b).unwrap().text.to_string() != "Hello\nLOUD\nmine\n" && Instant::now() < deadline {
        let _ = other.step(Duration::from_millis(20));
    }
    let version = other.node.state.buffer(b).unwrap().version;
    other.propose(apex_server::Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: END.min(16), q1: 16, text: "typed\n".into() }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("an edit");
    assert_eq!(ev, Event::Edit(apex_tool::Edit { window: w, q0: 16, nd: 0, text: "typed\n".into() }));
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

#[test]
fn a_page_window_is_made_and_written_again() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "shower").unwrap();
    let w = t.new_page("/tmp/shower+Preview", "<h1>one</h1>").unwrap();
    assert_eq!(t.read(w).unwrap(), "<h1>one</h1>");
    assert!(t.windows().iter().any(|x| x.id == w && x.name == "/tmp/shower+Preview"));
    // the page is its body: written again, it is another page
    t.replace(w, 0, END, "<h1>two</h1>").unwrap();
    assert_eq!(t.read(w).unwrap(), "<h1>two</h1>");
}

/// The tag's two halves: the leader keeps the words before `|` up to
/// date, a tool writes what follows and it stays written.
#[test]
fn a_tool_furnishes_its_window_tag() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "tagger").unwrap();
    let w = t.new_window("/tmp/tagger-notes").unwrap();
    t.set_tag(w, "Look Send").unwrap();
    assert_eq!(t.tag(w).unwrap(), " Look Send ");
    // words of apex's own come and go in the head (the buffer is
    // dirty, so Undo and Put), and the tool's half is left alone
    t.append(w, "a line\n").unwrap();
    let mut other = Remote::connect_as(&sock, "main", "other", AttachmentKind::Tool).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tag = String::new();
    while Instant::now() < deadline {
        let _ = other.step(Duration::from_millis(20));
        if let Some(text) = other.node.state.window(w).ok().and_then(|win| other.node.state.buffer(win.tag).ok()).map(|b| b.text.to_string()) {
            tag = text;
            if tag.contains("Put") {
                break;
            }
        }
    }
    assert_eq!(tag, "/tmp/tagger-notes Del Snarf Undo Put | Look Send ");
    assert_eq!(t.tag(w).unwrap(), " Look Send ");
    // a name with a bar of its own: the tool's half is still what
    // follows the bar past the name, and writing it leaves the name
    let b = t.new_window("/tmp/tagger|notes").unwrap();
    t.set_tag(b, "Look Send").unwrap();
    assert_eq!(t.tag(b).unwrap(), " Look Send ");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tag = String::new();
    while Instant::now() < deadline {
        let _ = other.step(Duration::from_millis(20));
        if let Some(text) = other.node.state.window(b).ok().and_then(|win| other.node.state.buffer(win.tag).ok()).map(|x| x.text.to_string()) {
            tag = text;
            if tag.contains("Send") {
                break;
            }
        }
    }
    assert_eq!(tag, "/tmp/tagger|notes Del Snarf | Look Send ");
}
#[test]
fn work_behind_a_window_shows_while_the_tool_is_there() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "slow").unwrap();
    let w = t.new_window("/tmp/slow-notes").unwrap();
    let mut other = Remote::connect_as(&sock, "main", "other", AttachmentKind::Tool).unwrap();
    let settle = |other: &mut Remote, want: bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while other.node.window_working(w) != want && Instant::now() < deadline {
            let _ = other.step(Duration::from_millis(20));
        }
        other.node.window_working(w)
    };
    assert!(!settle(&mut other, false), "idle to begin with");
    t.set_working(w, true).unwrap();
    assert!(settle(&mut other, true), "the handle pulses while the tool works");
    t.set_working(w, false).unwrap();
    assert!(!settle(&mut other, false), "and stops when the work is done");
    // a tool that goes while it works leaves no window pulsing forever
    t.set_working(w, true).unwrap();
    assert!(settle(&mut other, true));
    drop(t);
    assert!(!settle(&mut other, false), "the work ends with the tool");
}

#[test]
fn a_tool_takes_a_word_apex_knows_and_can_hand_it_back() {
    let sock = daemon();
    let dir = std::env::temp_dir().join(format!("apex-claim-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("notes.txt");
    std::fs::write(&path, "disk\n").unwrap();
    let name = path.display().to_string();

    let mut t = Tool::attach_to(&sock, "main", "fmt").unwrap();
    let w = t.open(&name, None).unwrap();
    // Put in this window is ours: the word apex has a meaning for
    let put = t.offer(Rule::verb("Put").window(w)).unwrap();
    let mut other = Remote::connect_as(&sock, "main", "other", AttachmentKind::Tool).unwrap();
    let b2 = |other: &mut Remote, text: &str| {
        other.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: text.into() }, Duration::from_secs(5)).unwrap();
    };
    let plumb = |t: &mut Tool| -> apex_tool::Plumb {
        let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("the claimed word");
        let Event::Plumb(p) = ev else { panic!("{ev:?}") };
        p
    };
    let disk = |path: &std::path::Path| std::fs::read_to_string(path).unwrap();

    // taken: the file is not written, the word meant what we said
    t.replace(w, 0, END, "ours\n").unwrap();
    b2(&mut other, "Put");
    let p = plumb(&mut t);
    assert_eq!((p.rule, p.verb.as_str(), p.window), (put, "Put", Some(w)));
    t.answer(&p, true).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(disk(&path), "disk\n", "a taken Put does not write the file");

    // declined: the walk carries on to apex's own Put, which writes it
    b2(&mut other, "Put");
    let p = plumb(&mut t);
    t.answer(&p, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while disk(&path) != "ours\n" && Instant::now() < deadline {
        let _ = t.next_event(Some(Duration::from_millis(50)));
    }
    assert_eq!(disk(&path), "ours\n", "a declined Put falls through to apex's");

    // a rule that says nothing about where it applies may not claim a
    // word apex knows: this Put is apex's, and never reaches the tool
    t.withdraw(put);
    assert!(t.offer(Rule::verb("Put")).is_err(), "an unscoped rule for a word apex knows is refused");
    // one that applies somewhere else does not take it here either
    let elsewhere = t.offer(Rule::verb("Put").file(r"\+never$")).unwrap();
    let _ = elsewhere;
    std::fs::write(&path, "disk again\n").unwrap();
    t.replace(w, 0, END, "second\n").unwrap();
    b2(&mut other, "Put");
    let deadline = Instant::now() + Duration::from_secs(5);
    while disk(&path) != "second\n" && Instant::now() < deadline {
        let _ = t.next_event(Some(Duration::from_millis(50)));
    }
    assert_eq!(disk(&path), "second\n", "an unscoped rule does not take Put");

    // a word the leader performs, not the server: Del, claimed, declined,
    // and the window goes as it always would
    let del = t.offer(Rule::verb("Del").window(w)).unwrap();
    b2(&mut other, "Del");
    let p = plumb(&mut t);
    assert_eq!((p.rule, p.verb.as_str()), (del, "Del"));
    t.answer(&p, true).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(t.windows().iter().any(|x| x.id == w), "a taken Del keeps the window");
    b2(&mut other, "Del");
    let p = plumb(&mut t);
    t.answer(&p, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while t.windows().iter().any(|x| x.id == w) && Instant::now() < deadline {
        let _ = t.next_event(Some(Duration::from_millis(50)));
    }
    assert!(!t.windows().iter().any(|x| x.id == w), "a declined Del falls through to apex's");
    let _ = std::fs::remove_dir_all(&dir);
}
