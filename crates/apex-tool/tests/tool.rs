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
/// Several edits are one proposal: they land together, as one undo
/// step, each where it stood in the text as the tool saw it, and a
/// watched window does not report them back as someone else's; one
/// made against a text since changed lands not at all.
#[test]
fn several_edits_land_together_or_not_at_all() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "acp").unwrap();
    let w = t.new_scratch("/tmp/acp-edits/", Some("edits")).unwrap();
    t.replace(w, 0, END, "one\ntwo\nthree\n").unwrap();
    t.watch(w).unwrap();
    t.select(w, 8, 8).unwrap();
    t.edit(w, &[(0, 3, "ONE".into()), (4, 7, "2".into()), (14, 14, "four\n".into())]).unwrap();
    assert_eq!(t.read(w).unwrap(), "ONE\n2\nthree\nfour\n");
    // the dot, at "three", moved along with what came before it
    assert_eq!(t.selection(w).map(|r| (r.q0, r.q1)).unwrap(), (6, 6));
    // nothing of ours comes back as an edit by someone else
    while let Ok(Some(ev)) = t.next_event(Some(Duration::from_millis(100))) {
        assert!(!matches!(ev, Event::Edit(_)), "{ev:?}");
    }
    // out of order: refused, and nothing written
    assert!(t.edit(w, &[(4, 5, "x".into()), (0, 1, "y".into())]).is_err());
    assert_eq!(t.read(w).unwrap(), "ONE\n2\nthree\nfour\n");
    // someone else's edit lands first: the tool's, made against what it
    // saw, does not, and the text is theirs alone
    let mut other = Tool::attach_to(&sock, "main", "other").unwrap();
    other.replace(w, 0, 0, "zero\n").unwrap();
    assert!(t.edit(w, &[(0, 3, "one".into())]).is_err());
    assert_eq!(other.read(w).unwrap(), "zero\nONE\n2\nthree\nfour\n");
    // and with nothing behind its file, it is no window to warn about
    let info = t.window(w).unwrap();
    assert!(!info.dirty && !info.stashed, "{info:?}");
}

/// A window says whether it holds text its file has not had, and
/// whether it is put away: what a tool warning about unsaved text, or
/// writing a stashed window differently, asks.
#[test]
fn a_window_says_whether_it_is_dirty_and_whether_it_is_stashed() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "acp").unwrap();
    let f = t.new_window("/tmp/acp-dirty.txt").unwrap();
    assert!(!t.window(f).unwrap().dirty);
    t.replace(f, 0, 0, "unsaved").unwrap();
    assert!(t.window(f).unwrap().dirty);
    let d = t.new_diagnostic("/tmp/acp-diag/", Some("diag")).unwrap();
    let info = t.window(d).unwrap();
    assert!(info.stashed && !info.dirty, "{info:?}");
    assert!(!t.window(f).unwrap().stashed);
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
        apex_core::plumb::verbs_for(&c.node.state.meta.rules, &c.node.window_path(w), c.node.window_kind(w), Some(w), c.node.window_owner(w))
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

/// A window a tool claims, made by another -- a tool before it, whose
/// windows it takes up -- is the tool's as one it made is: its
/// deletion is an event, so the tool forgets it.
#[test]
fn a_window_claimed_says_when_it_is_deleted() {
    let sock = daemon();
    let mut before = Tool::attach_to(&sock, "main", "agent").unwrap();
    let w = before.new_scratch("/tmp/proj/", Some("thread")).unwrap();
    let mut after = Tool::attach_to(&sock, "main", "agent").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while after.window(w).is_none() && Instant::now() < deadline {
        let _ = after.next_event(Some(Duration::from_millis(20)));
    }
    after.set_owner(w, true).unwrap();
    assert_eq!(after.next_event(Some(Duration::from_millis(200))).unwrap(), None);
    before.delete(w).unwrap();
    let ev = after.next_event(Some(Duration::from_secs(5))).unwrap().expect("a deletion");
    assert_eq!(ev, Event::Deleted { window: w });
}

/// A tool that goes leaving windows of its own leaves them waiting for
/// it, saying so, with its notes, and nothing more: it is not started
/// again behind the user's back, though a rule says how to start it. A
/// tool of its name, when one comes, takes them back.
#[test]
fn a_tools_windows_wait_for_it() {
    let sock = daemon();
    let marks = std::env::temp_dir().join(format!("apex-tool-again-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&marks);
    std::fs::create_dir_all(&marks).unwrap();
    let again = marks.join("agent");
    let mut c = Remote::connect_as(&sock, "main", "watch", AttachmentKind::Tool).unwrap();
    let seen = |c: &mut Remote, ok: &dyn Fn(&Remote) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ok(c) && Instant::now() < deadline {
            let _ = c.step(Duration::from_millis(20));
        }
        ok(c)
    };

    // a tool with a window of its own, noted, and a word that starts it
    let mut tool = Tool::attach_to(&sock, "main", "agent").unwrap();
    tool.offer_lasting(Rule::verb("Agents").start(&format!("touch {}", again.display()))).unwrap();
    let w = tool.new_scratch("/tmp/proj/", Some("a thread")).unwrap();
    tool.set_owner(w, true).unwrap();
    tool.set_note(w, Some("thread 0b1c")).unwrap();
    assert!(seen(&mut c, &|c| c.node.window_owner(w) == Some("agent") && c.node.window_note(w) == Some("thread 0b1c")));
    assert_eq!(c.node.window_waiting(w), None);

    // gone: the window waits, and says so; nothing starts the tool
    drop(tool);
    assert!(seen(&mut c, &|c| c.node.window_waiting(w) == Some("agent")), "waits");
    assert_eq!(c.node.window_status(w).and_then(|s| s.msg).as_deref(), Some("agent not running"));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!again.exists(), "not started again");

    // a tool of its name takes it back, by its note
    let mut back = Tool::attach_to(&sock, "main", "agent").unwrap();
    let info = back.window(w).expect("there still");
    assert!(info.waiting && info.owner.as_deref() == Some("agent") && info.note.as_deref() == Some("thread 0b1c"), "{info:?}");
    back.set_owner(w, true).unwrap();
    assert!(seen(&mut c, &|c| c.node.window_waiting(w).is_none()), "taken back");
    assert_eq!(c.node.window_status(w), None);
    let _ = std::fs::remove_dir_all(&marks);
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
    assert!(t.windows().iter().any(|x| x.id == w && x.path == "/tmp/shout-notes" && x.kind == WinKind::File && !x.scratch));
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
    // our own rename, label and status are not events, as our edits are not
    t.rename(w, "/tmp/shout-mine").unwrap();
    t.set_label(w, Some("shouted")).unwrap();
    t.set_status(w, Some(apex_tool::Status::new(apex_tool::State::Done).msg(" Shouting "))).unwrap();
    assert_eq!(t.next_event(Some(Duration::from_millis(200))).unwrap(), None);
    // the status, seen by everyone, with when it was said; the same
    // state and message again keep that
    let seen = |other: &mut Remote| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while other.node.window_status(w).and_then(|s| s.text()).as_deref() != Some("Shouting") && Instant::now() < deadline {
            let _ = other.step(Duration::from_millis(20));
        }
        other.node.window_status(w)
    };
    let said = seen(&mut other).expect("the status");
    assert!(said.since > 0 && said.state == apex_tool::State::Done);
    t.set_status(w, Some(apex_tool::Status::new(apex_tool::State::Done).msg("Shouting"))).unwrap();
    assert_eq!(seen(&mut other), Some(said));
    // someone else's rename and relabel, and a deletion, are (live, so
    // Del does not ask)
    t.set_live(w, true).unwrap();
    other.propose(apex_server::Proposal::SetPath { window: w, path: "/tmp/shout-renamed".into() }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("a rename");
    assert_eq!(ev, Event::Renamed { window: w, path: "/tmp/shout-renamed".into() });
    other.propose(apex_server::Proposal::SetLabel { window: w, label: None }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("a relabel");
    assert_eq!(ev, Event::Relabeled { window: w, label: None });
    other.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() }, Duration::from_secs(5)).unwrap();
    let ev = t.next_event(Some(Duration::from_secs(5))).unwrap().expect("a deletion");
    assert_eq!(ev, Event::Deleted { window: w });
    // nothing more, and the session goes on
    assert_eq!(t.next_event(Some(Duration::from_millis(200))).unwrap(), None);
    assert!(t.window(w).is_none());
}

#[test]
fn a_page_window_is_made_and_written_again() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "shower").unwrap();
    let w = t.new_page("/tmp/shower", Some("shown"), "<h1>one</h1>").unwrap();
    assert_eq!(t.read(w).unwrap(), "<h1>one</h1>");
    assert!(t.windows().iter().any(|x| x.id == w && x.path == "/tmp/shower" && x.label.as_deref() == Some("shown") && x.kind == WinKind::Page && x.scratch));
    // a label changed is an event for no one but those who watch; it is state
    t.set_label(w, Some("shown again")).unwrap();
    assert_eq!(t.window(w).and_then(|x| x.label), Some("shown again".into()));
    // the page is its body: written again, it is another page
    t.replace(w, 0, END, "<h1>two</h1>").unwrap();
    assert_eq!(t.read(w).unwrap(), "<h1>two</h1>");
}

/// The tag is the user's words alone: the path, the label and apex's
/// verbs are the window's state, drawn beside them, never in the text.
#[test]
fn a_tool_furnishes_its_window_tag() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "tagger").unwrap();
    let w = t.new_window("/tmp/tagger-notes").unwrap();
    assert_eq!(t.tag(w).unwrap(), "Look ");
    t.set_tag(w, "Look Send").unwrap();
    assert_eq!(t.tag(w).unwrap(), "Look Send ");
    // apex's verbs follow the window (dirty: Undo and Put), the tag
    // text is left alone
    t.append(w, "a line\n").unwrap();
    let mut other = Remote::connect_as(&sock, "main", "other", AttachmentKind::Tool).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !other.node.window_verbs(w).contains(&"Put") && Instant::now() < deadline {
        let _ = other.step(Duration::from_millis(20));
    }
    assert_eq!(other.node.window_verbs(w), vec!["Del", "Snarf", "Undo", "Put"]);
    let tag = other.node.state.window(w).ok().and_then(|win| other.node.state.buffer(win.tag).ok()).map(|b| b.text.to_string());
    assert_eq!(tag.as_deref(), Some("Look Send "));
    // a path with a bar or a space in it is a path whole
    let b = t.new_window("/tmp/tagger| notes").unwrap();
    t.set_tag(b, "Look Send").unwrap();
    assert_eq!(t.tag(b).unwrap(), "Look Send ");
    assert_eq!(t.window(b).map(|x| x.path), Some("/tmp/tagger| notes".into()));
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
    let working = || Some(apex_tool::Status::new(apex_tool::State::Working));
    t.set_status(w, working()).unwrap();
    assert!(settle(&mut other, true), "the handle pulses while the tool works");
    t.set_status(w, None).unwrap();
    assert!(!settle(&mut other, false), "and stops when the work is done");
    // a tool that goes while it works leaves no window pulsing forever
    t.set_status(w, working()).unwrap();
    assert!(settle(&mut other, true));
    drop(t);
    assert!(!settle(&mut other, false), "the work ends with the tool");
    assert_eq!(other.node.window_status(w), None, "and so does what it said");
}

/// A window's notification follows its status: raised when it comes to
/// ask for the user, lowered when it no longer does, or by the user
/// (the status left as it was); and it goes with the tool that said it.
#[test]
fn a_status_that_asks_for_the_user_raises_the_windows_notification() {
    use apex_tool::{Kind, State, Status};
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "asker").unwrap();
    let w = t.new_window("/tmp/asking-notes").unwrap();
    let mut ui = Remote::connect_as(&sock, "main", "watch", AttachmentKind::Tool).unwrap();
    let until = |ui: &mut Remote, want: bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while ui.node.window_notified(w) != want && Instant::now() < deadline {
            let _ = ui.step(Duration::from_millis(20));
        }
        ui.node.window_notified(w)
    };
    t.set_status(w, Some(Status::new(State::Working))).unwrap();
    assert!(!until(&mut ui, false), "at work: nothing to ask");
    t.set_status(w, Some(Status::new(State::Blocked).kind(Some(Kind::Question)).msg("which?"))).unwrap();
    assert!(until(&mut ui, true), "blocked: raised");
    t.set_status(w, Some(Status::new(State::Working))).unwrap();
    assert!(!until(&mut ui, false), "back at work: lowered");
    t.set_status(w, Some(Status::new(State::Done))).unwrap();
    assert!(until(&mut ui, true), "done: raised");
    // lowered, as the user taking it would (the raiser may too)
    t.unnotify(w).unwrap();
    assert!(!until(&mut ui, false), "taken");
    t.set_status(w, Some(Status::new(State::Done).msg("all of it"))).unwrap();
    assert!(!until(&mut ui, false), "the same state again, other words: not news");
    assert_eq!(t.window(w).and_then(|i| i.status).map(|s| s.state), Some(State::Done), "and the status stands");
    t.set_status(w, Some(Status::new(State::Error))).unwrap();
    assert!(until(&mut ui, true), "another that asks: news");
    drop(t);
    assert!(!until(&mut ui, false), "and it goes with the tool");
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

#[test]
fn a_tool_asks_for_attention_on_a_window_and_the_user_or_the_tool_lowers_it() {
    let sock = daemon();
    let mut agent = Tool::attach_to(&sock, "main", "agent").unwrap();
    let mut build = Tool::attach_to(&sock, "main", "build").unwrap();
    let w = agent.new_window("/tmp/agent-waiting").unwrap();
    let v = build.new_window("/tmp/build-waiting").unwrap();
    let mut ui = Remote::connect_as(&sock, "main", "ui", AttachmentKind::Ui).unwrap();
    let queue = |r: &Remote| r.node.notifications().map(|n| (r.node.state.meta.attachments.get(&n.by).map(|a| a.name.clone()).unwrap_or_default(), n.window)).collect::<Vec<_>>();
    let settle = |r: &mut Remote, want: &dyn Fn(&Remote) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !want(r) && Instant::now() < deadline {
            let _ = r.step(Duration::from_millis(20));
        }
    };

    // raised on windows, oldest first
    agent.notify(w).unwrap();
    settle(&mut ui, &|r| queue(r).len() == 1);
    build.notify(v).unwrap();
    settle(&mut ui, &|r| queue(r).len() == 2);
    assert_eq!(queue(&ui), vec![("agent".to_string(), w), ("build".to_string(), v)]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !agent.notified(w) && Instant::now() < deadline {
        let _ = agent.next_event(Some(Duration::from_millis(20)));
    }
    assert!(agent.notified(w));

    // a tool may not lower another's
    let mut meddler = Remote::connect_as(&sock, "main", "meddler", AttachmentKind::Tool).unwrap();
    meddler.send(&apex_server::proto::ClientMsg::Unnotify { window: w });
    std::thread::sleep(Duration::from_millis(300));
    let _ = ui.step(Duration::from_millis(50));
    assert_eq!(queue(&ui).len(), 2, "a tool cannot dismiss another's notification");

    // the user takes the oldest; the tool sees it is gone
    ui.send(&apex_server::proto::ClientMsg::Unnotify { window: w });
    settle(&mut ui, &|r| queue(r).len() == 1);
    assert_eq!(queue(&ui), vec![("build".to_string(), v)]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while agent.notified(w) && Instant::now() < deadline {
        let _ = agent.next_event(Some(Duration::from_millis(20)));
    }
    assert!(!agent.notified(w), "dismissed");

    // a tool retracts its own
    build.unnotify(v).unwrap();
    settle(&mut ui, &|r| queue(r).is_empty());
    assert!(queue(&ui).is_empty());

    // and a tool that goes takes its notification with it
    agent.notify(v).unwrap();
    settle(&mut ui, &|r| queue(r).len() == 1);
    drop(agent);
    settle(&mut ui, &|r| queue(r).is_empty());
    assert!(queue(&ui).is_empty(), "the notification goes with the tool");
}

#[test]
fn a_tools_snarf_is_the_snarf_buffer_and_the_uis_clipboard() {
    let sock = daemon();
    let mut ui = Remote::connect_as(&sock, "main", "ui", AttachmentKind::Ui).unwrap();
    // the UI leads, so the tool waits on it to apply
    let at = sock.clone();
    let copier = std::thread::spawn(move || Tool::attach_to(&at, "main", "copier").unwrap().snarf("src/a.rs:3:\nhello\n"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while (ui.link.clips.is_empty() || ui.node.state.layout.snarf.is_empty()) && Instant::now() < deadline {
        let _ = ui.step(Duration::from_millis(20));
    }
    while !copier.is_finished() && Instant::now() < deadline {
        let _ = ui.step(Duration::from_millis(20));
    }
    copier.join().unwrap().unwrap();
    assert_eq!(ui.node.state.layout.snarf, "src/a.rs:3:\nhello\n");
    assert_eq!(ui.link.clips, vec!["src/a.rs:3:\nhello\n".to_string()]);
}

#[test]
fn next_event_tells_a_wait_run_out_from_the_session_ending() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "watcher").unwrap();
    // nothing happened: the wait ran out
    assert_eq!(t.next_event(Some(Duration::from_millis(50))).unwrap(), None);
    // the session ends: an error that says so, not another None
    apex_server::remote::end_session(&sock, "main", true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let end = loop {
        match t.next_event(Some(Duration::from_millis(50))) {
            Err(e) => break e,
            Ok(_) if Instant::now() < deadline => continue,
            Ok(ev) => panic!("still going: {ev:?}"),
        }
    };
    assert!(end.is_closed(), "{end}");
}

/// A tool that handles its pages hears of links followed in them, and
/// says where they go; and hears what happens in them.
#[test]
fn a_tool_answers_where_links_in_its_pages_go() {
    use apex_server::proto::{Answer, NavAnswer, Request, WindowEvent};
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "pages").unwrap();
    let w = t.new_page("/tmp/pages", None, "<a href=x>x</a>").unwrap();
    t.set_owner(w, true).unwrap();
    t.handle_pages();
    let mut client = Remote::connect_as(&sock, "main", "client", AttachmentKind::Tool).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && client.node.state.window(w).map(|x| x.owner.is_none()).unwrap_or(true) {
        let _ = client.step(Duration::from_millis(20));
    }
    let id = client.link.ask(Request::Navigate { window: w, url: "file:///tmp/x".into() });
    client.link.window_event(w, WindowEvent::Title { title: "Pages".into() });
    let mut asked = None;
    let mut titled = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && (asked.is_none() || !titled) {
        match t.next_event(Some(Duration::from_millis(50))) {
            Ok(Some(Event::Navigate(n))) => asked = Some(n),
            Ok(Some(Event::Page { window, event: WindowEvent::Title { title } })) => titled = window == w && title == "Pages",
            _ => {}
        }
        let _ = client.step(Duration::from_millis(10));
    }
    let n = asked.expect("asked where the link goes");
    assert_eq!((n.window, n.url.as_str()), (w, "file:///tmp/x"));
    assert!(titled, "told the page's title");
    t.answer_navigation(&n, NavAnswer::Handled).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && client.link.answered.is_empty() {
        let _ = client.step(Duration::from_millis(20));
    }
    assert_eq!(client.link.answered.pop(), Some((id, Some(Answer::Navigate(NavAnswer::Handled)))));
}

/// A tool serves what its pages load: a request for `tool://NAME/...`
/// on the I/O plane reaches it, and its answer comes back; with no tool
/// of that name attached, the request is refused.
#[test]
fn a_tool_serves_its_pages_over_the_plane() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "pages").unwrap();
    let client = Remote::connect_as(&sock, "main", "client", AttachmentKind::Tool).unwrap();
    let plane = client.io_plane();
    let fetch = std::thread::spawn(move || plane.fetch("GET", "tool://pages/hello?x=1", &[], None, Duration::from_secs(5)));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut served = false;
    while Instant::now() < deadline && !served {
        if let Ok(Some(apex_tool::Event::Request(r))) = t.next_event(Some(Duration::from_millis(50))) {
            assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/hello?x=1"));
            t.respond(&r, 200, &[("Content-Type", "text/html")], b"<h1>hello</h1>").unwrap();
            served = true;
        }
    }
    assert!(served, "the tool was asked");
    let (status, headers, body) = fetch.join().unwrap().unwrap();
    assert_eq!(status, 200);
    assert!(headers.iter().any(|(k, v)| k == "Content-Type" && v == "text/html"));
    assert_eq!(body, b"<h1>hello</h1>");
    let plane = client.io_plane();
    let (status, _, _) = plane.fetch("GET", "tool://nobody/x", &[], None, Duration::from_secs(5)).unwrap();
    assert_eq!(status, 503);
}

/// A tool's word put in the top row: at the end, once, the user's words
/// left as they are.
#[test]
fn a_word_is_put_in_the_top_row_once() {
    let sock = daemon();
    let mut tool = Tool::attach_to(&sock, "main", "agent").unwrap();
    let before = tool.top().unwrap();
    assert!(tool.top_word("Agents").unwrap(), "put there");
    let after = tool.top().unwrap();
    assert_eq!(after.trim_end(), format!("{} Agents", before.trim_end()).trim_start(), "{after:?}");
    assert!(!tool.top_word("Agents").unwrap(), "there already");
    assert_eq!(tool.top().unwrap(), after);
}

/// A lens a tool makes on a window, with an input strip: the strip's
/// text the user's to write and the tool's to take, and a draft put
/// back as the strip is made again; the lens gone with the tool.
#[test]
fn a_tools_lens_has_an_input_strip_and_goes_with_the_tool() {
    let sock = daemon();
    let mut t = Tool::attach_to(&sock, "main", "lenser").unwrap();
    let base = t.new_scratch("/tmp/lensed", None).unwrap();
    let lens = t.lens_page(base, "/tmp/lensed", Some("conversation"), "<p>hi</p>").unwrap();
    let mut ui = Remote::connect_as(&sock, "main", "ui", AttachmentKind::Tool).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while ui.node.state.layout.lens_of(base) != Some(lens) && Instant::now() < deadline {
        let _ = ui.step(Duration::from_millis(20));
    }
    assert_eq!(ui.node.state.layout.lens_of(base), Some(lens));
    // a strip, a draft in it, written to as the user would
    t.set_input(lens, Some("half")).unwrap();
    assert_eq!(t.input(lens).as_deref(), Some("half"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while ui.node.state.window(lens).ok().and_then(|w| w.input).is_none() && Instant::now() < deadline {
        let _ = ui.step(Duration::from_millis(20));
    }
    let ib = ui.node.state.window(lens).unwrap().input.expect("the strip, replicated");
    let version = ui.node.state.buffer(ib).unwrap().version;
    ui.propose(apex_server::Proposal::ReplaceRange { select: false, dir: None, buffer: ib, version, q0: 4, q1: 4, text: " a prompt".into() }, Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while t.input(lens).as_deref() != Some("half a prompt") && Instant::now() < deadline {
        let _ = t.next_event(Some(Duration::from_millis(20)));
    }
    // taken: the strip empty
    assert_eq!(t.take_input(lens).unwrap(), "half a prompt");
    assert_eq!(t.input(lens).as_deref(), Some(""));
    // the tool gone: its lens with it, the window back
    drop(t);
    let deadline = Instant::now() + Duration::from_secs(5);
    while (ui.node.state.window(lens).is_ok() || ui.node.state.layout.slot(base).is_none()) && Instant::now() < deadline {
        let _ = ui.step(Duration::from_millis(20));
    }
    assert!(ui.node.state.window(lens).is_err() && ui.node.state.buffer(ib).is_err());
    assert!(ui.node.state.layout.slot(base).is_some());
}
