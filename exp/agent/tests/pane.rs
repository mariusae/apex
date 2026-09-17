//! The pane against a daemon on a thread: an agent's log makes a block,
//! B3 in the block opens the agent's transcript, which follows the
//! transcript file and the hooks, and the agent's end takes it away.
//! Each change is waited for less long than the pane's slow pass, so
//! it is the watches that are seen to work.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_agent::event::{self, Event};
use apex_agent::hook;
use apex_agent::win::{Opts, Pane};
use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::ClientMsg;
use apex_server::remote::Remote;
use apex_tool::Tool;

fn daemon() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-agent-test-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

fn window_named(c: &Remote, name: &str) -> Option<WindowId> {
    c.node.state.windows.keys().copied().find(|w| c.node.window_name(*w) == name)
}

/// Whether the window's body is dirty: written and not said to be whole.
fn dirty(c: &mut Remote, w: WindowId) -> bool {
    let _ = c.step(Duration::from_millis(50));
    let b = c.node.state.window(w).unwrap().body_buffer().unwrap();
    c.node.state.buffer(b).unwrap().dirty()
}

fn text_of(c: &Remote, w: WindowId) -> String {
    let b = c.node.state.window(w).unwrap().body_buffer().unwrap();
    c.node.state.buffer(b).unwrap().text.to_string()
}

/// The window's text once `ok` says so, and soon: sooner than the
/// pane's slow pass, so that it was the watch that saw the change.
fn wait_text(c: &mut Remote, name: &str, ok: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last = String::new();
    while Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(20));
        if let Some(w) = window_named(c, name) {
            last = text_of(c, w);
            if ok(&last) {
                return last;
            }
        }
    }
    let all: Vec<(String, String)> = c.node.state.windows.keys().map(|w| (c.node.window_name(*w), text_of(c, *w))).collect();
    panic!("{name}: waited in vain; last saw {last:?}; windows: {all:?}");
}

fn wait_gone(c: &mut Remote, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(20));
        if window_named(c, name).is_none() {
            return;
        }
    }
    panic!("{name}: still there");
}

#[test]
fn an_agents_log_is_a_block_and_b3_on_it_opens_the_transcript() {
    let sock = daemon();
    let tmp = std::env::temp_dir().join(format!("apex-agent-pane-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let logs = tmp.join("agents");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    // the agent's directory is a git repository, for Changes
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").args(args).current_dir(&proj).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    std::fs::write(proj.join("src/a.rs"), "a\nb\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "one"]);
    let rev = git(&["rev-parse", "HEAD"]);
    // and the directory has had a session before, for History
    let claude_home = tmp.join("claude-home");
    let past_dir = claude_home.join("projects").join(apex_agent::history::claude_project(&tmp.display().to_string()));
    std::fs::create_dir_all(&past_dir).unwrap();
    std::fs::write(past_dir.join("deadbeef-1111.jsonl"), "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"an old prompt\"}}\n{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"an old answer\"}]}}\n").unwrap();
    let transcript = tmp.join("0b1c1425-aaaa.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"what is in hosts?\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Read\",\"input\":{\"file_path\":\"/etc/hosts\"}}]}}\n"
        ),
    )
    .unwrap();
    let ev = |event: &str| Event { ms: event::now_ms(), agent: "claude".into(), event: event.into(), session: "0b1c1425-aaaa".into(), cwd: proj.display().to_string(), transcript: Some(transcript.display().to_string()), ..Event::default() };
    event::append(&logs, &Event { kind: Some("startup".into()), rev: Some(rev.clone()), ..ev("SessionStart") }).unwrap();
    event::append(&logs, &Event { text: Some("what is in hosts?".into()), ..ev("UserPromptSubmit") }).unwrap();
    event::append(&logs, &Event { call: Some("t1".into()), tool: Some("Read".into()), title: Some("Read: /etc/hosts".into()), ..ev("PreToolUse") }).unwrap();

    let mut t = Tool::attach_to(&sock, "main", "agents").unwrap();
    // the page's converter: the markdown itself, so the test can read it
    t.set("Preview.md", "cat");
    let pane = Pane::start(t, Opts { pane: true, all: true, claude_home: claude_home.clone(), codex_home: tmp.join("codex-home"), ..Opts::new(tmp.clone(), logs.clone()) }).unwrap();
    let served = std::thread::spawn(move || {
        let mut pane = pane;
        let r = pane.serve();
        if let Err(e) = &r {
            eprintln!("serve: {}", e.0);
        }
        r
    });
    let pane_name = format!("{}/-agents", tmp.display());
    let mut c = Remote::connect_as(&sock, "main", "watch", AttachmentKind::Tool).unwrap();
    let text = wait_text(&mut c, &pane_name, |t| t.contains("Read: /etc/hosts"));
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = match proj.display().to_string().strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => proj.display().to_string(),
    };
    assert_eq!(text, format!("– 1 agent\n\n▶ claude  {dir}  0b1c1425\n  what is in hosts?\n  ▶ Read: /etc/hosts\n"));

    // written while an agent works, the pane is dirty and pulsing
    let w = window_named(&c, &pane_name).unwrap();
    assert!(dirty(&mut c, w));

    // B3 on the agent's name: its transcript, beside the pane, named
    // for the agent's own directory
    let b = c.node.state.window(w).unwrap().body_buffer().unwrap();
    let chars: Vec<char> = text.chars().collect();
    let at = chars.windows(6).position(|w| w.iter().collect::<String>() == "claude").unwrap();
    let span = Span { buffer: b, q0: at, q1: at + 6 };
    c.send(&ClientMsg::Plumb { ctx: ExecCtx::Window(w), text: "claude".into(), dir: None, edit_only: false, dry: false, at: Some(span), sel: Some(span), alt: None, reverse: false, verb: None });
    let detail_name = format!("{}/-claude+0b1c1425", proj.display());
    let text = wait_text(&mut c, &detail_name, |t| t.contains("Read"));
    // the call the hooks said was running is marked so, though the
    // transcript only knows it was made
    assert_eq!(text, "~\n\nwhat is in hosts?\n\n▶ Read: /etc/hosts\n");

    // the result lands in the transcript and the hook says the call ended
    let mut f = std::fs::OpenOptions::new().append(true).open(&transcript).unwrap();
    std::io::Write::write_all(&mut f, b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t1\",\"content\":\"127.0.0.1 localhost\"}]}}\n{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"It names localhost.\"}]}}\n").unwrap();
    event::append(&logs, &Event { call: Some("t1".into()), ..ev("PostToolUse") }).unwrap();
    event::append(&logs, &Event { text: Some("It names localhost.".into()), ..ev("Stop") }).unwrap();
    let since = Instant::now();
    let text = wait_text(&mut c, &detail_name, |t| t.contains("localhost."));
    let took = since.elapsed();
    assert!(took < Duration::from_millis(1500), "the watch took {took:?} to say so");
    eprintln!("the watch said so in {took:?}");
    assert_eq!(text, "~\n\nwhat is in hosts?\n\n✓ Read: /etc/hosts\n    127.0.0.1 localhost\n• It names localhost.\n");
    let text = wait_text(&mut c, &pane_name, |t| t.contains("\n~ claude"));
    assert_eq!(text, format!("– 1 agent\n\n~ claude  {dir}  0b1c1425\n  what is in hosts?\n  • It names localhost.\n"));
    // no agent busy: the pane is clean
    let since = Instant::now();
    while dirty(&mut c, w) && since.elapsed() < Duration::from_secs(3) {}
    assert!(!dirty(&mut c, w), "the pane stayed dirty with no agent busy");
    eprintln!("the pane was clean in {:?}", since.elapsed());

    // Preview with dot in the block: the last exchange as a page, what
    // was asked quoted and then the answer; written again as the next
    // turn ends
    c.propose(apex_server::Proposal::Select { view: ViewId::Body(w), q0: at, q1: at }, Duration::from_secs(5)).unwrap();
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Preview".into() }, Duration::from_secs(5)).unwrap();
    let page_name = format!("{detail_name}+Preview");
    let text = wait_text(&mut c, &page_name, |t| t.contains("localhost"));
    assert_eq!(text, "> what is in hosts?\n\nIt names localhost.\n");
    event::append(&logs, &Event { text: Some("and /etc/passwd?".into()), ..ev("UserPromptSubmit") }).unwrap();
    event::append(&logs, &Event { text: Some("Users, one a line.".into()), ..ev("Stop") }).unwrap();
    let text = wait_text(&mut c, &page_name, |t| t.contains("passwd"));
    assert_eq!(text, "> and /etc/passwd?\n\nUsers, one a line.\n");
    // Preview again closes it
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Preview 0b1c".into() }, Duration::from_secs(5)).unwrap();
    wait_gone(&mut c, &page_name);

    // Goto with dot in the block: this agent was started outside apex,
    // and +Errors says so rather than going nowhere
    c.propose(apex_server::Proposal::Select { view: ViewId::Body(w), q0: at, q1: at }, Duration::from_secs(5)).unwrap();
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Goto".into() }, Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut said = String::new();
    while Instant::now() < deadline && !said.contains("not started in an apex window") {
        let _ = c.step(Duration::from_millis(20));
        said = c.node.state.windows.keys().filter(|x| c.node.window_name(**x).ends_with("+Errors")).map(|x| text_of(&c, *x)).collect();
    }
    assert!(said.contains("Goto 0b1c1425: claude was not started in an apex window"), "{said:?}");
    // a question: the block offers the words that answer it, +Errors
    // says who asks, and the hook waits on the log for the answer
    event::append(&logs, &Event { apex: Some("main".into()), win: Some(1), call: Some("t7".into()), title: Some("Bash: Remove the build directory".into()), ..ev("PermissionRequest") }).unwrap();
    let text = wait_text(&mut c, &pane_name, |t| t.contains("\n? claude"));
    assert!(text.contains("  ? Bash: Remove the build directory  Allow Deny Ask\n"), "{text}");
    let errs = wait_text(&mut c, "+Errors", |t| t.contains("asks:"));
    assert!(errs.contains("claude 0b1c1425 asks: Bash: Remove the build directory\n"), "{errs:?}");
    let from = std::fs::metadata(event::log_path(&logs, "0b1c1425-aaaa")).unwrap().len();
    let log = event::log_path(&logs, "0b1c1425-aaaa");
    let asked = std::thread::spawn(move || hook::await_decision(&log, from, "t7", Duration::from_secs(5)));
    let at = text.chars().collect::<Vec<char>>().windows(5).position(|w| w.iter().collect::<String>() == "Allow").unwrap();
    c.propose(apex_server::Proposal::Select { view: ViewId::Body(w), q0: at, q1: at }, Duration::from_secs(5)).unwrap();
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Allow".into() }, Duration::from_secs(5)).unwrap();
    assert_eq!(asked.join().unwrap(), Some("allow".to_string()));
    let text = wait_text(&mut c, &pane_name, |t| t.contains("\n▶ claude"));
    assert!(!text.contains("Allow Deny Ask"), "{text}");
    event::append(&logs, &Event { call: Some("t7".into()), ..ev("PostToolUse") }).unwrap();
    event::append(&logs, &Event { text: Some("Removed.".into()), ..ev("Stop") }).unwrap();
    wait_text(&mut c, &pane_name, |t| t.contains("Removed."));

    // Changes: the repository's diff since the session began, in a
    // window at its root, each hunk saying where it lands
    std::fs::write(proj.join("src/a.rs"), "a\nB\nb\n").unwrap();
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Changes 0b1c".into() }, Duration::from_secs(5)).unwrap();
    let diff_name = format!("{}/-claude+0b1c1425+diff", proj.display());
    let text = wait_text(&mut c, &diff_name, |t| t.contains("@@"));
    assert!(text.starts_with(&format!("– changes in {} since the session began ({})\n\n M src/a.rs\n\n", proj.display(), &rev[..12])), "{text}");
    assert!(text.contains("+++ src/a.rs\n@@ -1,2 +1,3 @@  src/a.rs:1\n"), "{text}");

    // History: the directory's past sessions; B3 on an id opens its transcript
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "History".into() }, Duration::from_secs(5)).unwrap();
    let hist_name = format!("{}/-agents+history", tmp.display());
    let text = wait_text(&mut c, &hist_name, |t| t.contains("deadbeef-1111"));
    assert!(text.contains("  deadbeef-1111  "), "{text}");
    assert!(text.contains("  claude  an old prompt\n"), "{text}");
    let hw = window_named(&c, &hist_name).unwrap();
    let hb = c.node.state.window(hw).unwrap().body_buffer().unwrap();
    let at = text.chars().collect::<Vec<char>>().windows(8).position(|x| x.iter().collect::<String>() == "deadbeef").unwrap();
    let span = Span { buffer: hb, q0: at, q1: at + 8 };
    c.send(&ClientMsg::Plumb { ctx: ExecCtx::Window(hw), text: "deadbeef".into(), dir: None, edit_only: false, dry: false, at: Some(span), sel: Some(span), alt: None, reverse: false, verb: None });
    let past_name = format!("{}/-claude+deadbeef", tmp.display());
    let text = wait_text(&mut c, &past_name, |t| t.contains("old answer"));
    assert!(text.starts_with("– a past session, last worked in "), "{text}");
    assert!(text.ends_with("~\n\nan old prompt\n\n• an old answer\n"), "{text}");

    // told where it was started, Goto has somewhere to go, and says nothing
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Goto 0b1c".into() }, Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let _ = c.step(Duration::from_millis(20));
    let said: String = c.node.state.windows.keys().filter(|x| c.node.window_name(**x).ends_with("+Errors")).map(|x| text_of(&c, *x)).collect();
    assert_eq!(said.matches("Goto").count(), 1, "{said:?}");
    // an agent in a terminal of this very session: its verbs are offered
    // on that window too, and the three that answer while it asks
    let col = c.node.state.layout.cols.first().map(|x| x.id).unwrap();
    let tw = c.propose(apex_server::Proposal::NewWindow { col, name: format!("{}/-term", proj.display()) }, Duration::from_secs(5)).unwrap().unwrap();
    let sid = c.node.state.meta.id.clone();
    assert!(!sid.is_empty());
    // the window's tools menu, once every verb wanted is in it (or,
    // wanting none, once Allow has gone)
    let menu = |c: &mut Remote, w: WindowId, want: &[&str]| -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut v = Vec::new();
        while Instant::now() < deadline {
            let _ = c.step(Duration::from_millis(20));
            v = apex_core::plumb::verbs_for(&c.node.state.meta.rules, &c.node.window_name(w), c.node.window_kind(w), Some(w), c.node.window_owner(w));
            if (want.is_empty() && !v.iter().any(|x| x == "Allow")) || (!want.is_empty() && want.iter().all(|x| v.iter().any(|y| y == x))) {
                break;
            }
        }
        v
    };
    event::append(&logs, &Event { apex: Some(sid.clone()), win: Some(tw.0), ..ev("UserPromptSubmit") }).unwrap();
    let v = menu(&mut c, tw, &["Transcript", "Preview", "Changes"]);
    assert!(["Transcript", "Preview", "Changes"].iter().all(|x| v.iter().any(|y| y == x)), "{v:?}");
    assert!(!v.iter().any(|x| x == "Allow"), "{v:?}");
    // Preview in the agent's window is that agent's page
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(tw), text: "Preview".into() }, Duration::from_secs(5)).unwrap();
    wait_text(&mut c, &page_name, |t| t.contains("Removed."));
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(tw), text: "Preview".into() }, Duration::from_secs(5)).unwrap();
    wait_gone(&mut c, &page_name);
    // asking: Allow Deny Ask come to the window, and Allow there answers
    event::append(&logs, &Event { call: Some("t8".into()), title: Some("Bash: rm -rf target".into()), ..ev("PermissionRequest") }).unwrap();
    let v = menu(&mut c, tw, &["Allow", "Deny", "Ask"]);
    assert!(["Allow", "Deny", "Ask"].iter().all(|x| v.iter().any(|y| y == x)), "{v:?}");
    let log = event::log_path(&logs, "0b1c1425-aaaa");
    let from = std::fs::metadata(&log).unwrap().len();
    let asked = std::thread::spawn(move || hook::await_decision(&log, from, "t8", Duration::from_secs(5)));
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(tw), text: "Deny".into() }, Duration::from_secs(5)).unwrap();
    assert_eq!(asked.join().unwrap(), Some("deny".to_string()));
    let v = menu(&mut c, tw, &[]);
    assert!(!v.iter().any(|x| x == "Allow") && v.iter().any(|x| x == "Transcript"), "{v:?}");
    event::append(&logs, &Event { call: Some("t8".into()), ..ev("PostToolUse") }).unwrap();
    event::append(&logs, &Event { text: Some("Left it.".into()), ..ev("Stop") }).unwrap();
    wait_text(&mut c, &pane_name, |t| t.contains("Left it."));

    // Send with nothing to type into is said too; with somewhere, it wants apex
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Send 0b1c hello".into() }, Duration::from_secs(5)).unwrap();
    let said = wait_text(&mut c, "+Errors", |t| t.contains("Send:"));
    assert!(said.contains("Send: ") && (said.contains("apex term send") || said.contains("no apex command") || said.contains("session")), "{said:?}");

    // CopyContext in a file's window: its selection, with where it is,
    // into the snarf buffer; with nothing selected, that is said
    let fw = c.propose(apex_server::Proposal::NewWindow { col, name: proj.join("src/a.rs").display().to_string() }, Duration::from_secs(5)).unwrap().unwrap();
    let v = menu(&mut c, fw, &["CopyContext"]);
    assert!(v.iter().any(|x| x == "CopyContext"), "{v:?}");
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(fw), text: "CopyContext".into() }, Duration::from_secs(5)).unwrap();
    wait_text(&mut c, "+Errors", |t| t.contains("CopyContext: select"));
    let fb = c.node.state.window(fw).unwrap().body_buffer().unwrap();
    let version = c.node.state.buffers[&fb].version;
    c.propose(apex_server::Proposal::Insert { buffer: fb, version, at: 0, text: "one\ntwo\n".into(), follow: false }, Duration::from_secs(5)).unwrap();
    c.propose(apex_server::Proposal::Select { view: ViewId::Body(fw), q0: 4, q1: 8 }, Duration::from_secs(5)).unwrap();
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(fw), text: "CopyContext".into() }, Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !c.node.state.layout.snarf.contains("```") && Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(20));
    }
    assert_eq!(c.node.state.layout.snarf, format!("{}:2:\n```\ntwo\n```", proj.join("src/a.rs").display()));

    // the session ends: the block goes, the log with it, and the
    // transcript window says so and stays
    event::append(&logs, &ev("SessionEnd")).unwrap();
    let text = wait_text(&mut c, &pane_name, |t| t.starts_with("– no agents"));
    assert!(!text.contains("0b1c1425"), "{text}");
    let since = Instant::now();
    while dirty(&mut c, w) && since.elapsed() < Duration::from_secs(3) {}
    assert!(!dirty(&mut c, w), "the pane stayed dirty with no agents");
    let text = wait_text(&mut c, &detail_name, |t| t.contains("gone"));
    let all: Vec<(String, String)> = c.node.state.windows.keys().map(|w| (c.node.window_name(*w), text_of(&c, *w))).collect();
    assert!(text.ends_with("• It names localhost.\n– the agent is gone\n"), "{text:?}; windows: {all:?}");
    let deadline = Instant::now() + Duration::from_secs(3);
    while event::log_path(&logs, "0b1c1425-aaaa").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!event::log_path(&logs, "0b1c1425-aaaa").exists());

    // Del on the pane ends the tool
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() }, Duration::from_secs(5)).unwrap();
    wait_gone(&mut c, &pane_name);
    served.join().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Every notification raised, in the queue's order: who raised it, and
/// the window it is on.
fn flags(c: &Remote) -> Vec<(String, WindowId)> {
    c.node.notifications().map(|n| (c.node.state.meta.attachments.get(&n.by).map(|a| a.name.clone()).unwrap_or_default(), n.window)).collect()
}

fn wait_flags(c: &mut Remote, ok: impl Fn(&[(String, WindowId)]) -> bool) -> Vec<(String, WindowId)> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last = Vec::new();
    while Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(20));
        last = flags(c);
        if ok(&last) {
            return last;
        }
    }
    panic!("the notifications waited in vain; last saw {last:?}");
}

/// Without `-a` there is no window of apex-agent's own: it serves the
/// session's terminals, and says an agent wants you with a
/// notification on the terminal it runs in, one an agent, raised as
/// the turn ends and lowered as the next one begins.
#[test]
fn with_no_pane_a_ready_agent_raises_a_notification_at_its_own_window() {
    let sock = daemon();
    let tmp = std::env::temp_dir().join(format!("apex-agent-flags-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let logs = tmp.join("agents");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();

    let mut c = Remote::connect_as(&sock, "main", "watch", AttachmentKind::Tool).unwrap();
    let sid = c.node.state.meta.id.clone();
    assert!(!sid.is_empty());
    // the terminal the agent runs in
    let col = c.node.state.layout.cols.first().map(|x| x.id).unwrap();
    let tw = c.propose(apex_server::Proposal::NewWindow { col, name: format!("{}/-term", proj.display()) }, Duration::from_secs(5)).unwrap().unwrap();

    let ev = |event: &str| Event {
        ms: event::now_ms(),
        agent: "claude".into(),
        event: event.into(),
        session: "0b1c1425-aaaa".into(),
        cwd: proj.display().to_string(),
        apex: Some(sid.clone()),
        win: Some(tw.0),
        ..Event::default()
    };
    event::append(&logs, &Event { kind: Some("startup".into()), ..ev("SessionStart") }).unwrap();
    event::append(&logs, &Event { text: Some("what is in hosts?".into()), ..ev("UserPromptSubmit") }).unwrap();

    let t = Tool::attach_to(&sock, "main", "agents").unwrap();
    let pane = Pane::start(t, Opts { claude_home: tmp.join("claude-home"), codex_home: tmp.join("codex-home"), ..Opts::new(tmp.clone(), logs.clone()) }).unwrap();
    // it is served until the session is over, which the test does not
    // wait for: nothing to Del, and nothing to join
    std::thread::spawn(move || {
        let mut pane = pane;
        if let Err(e) = pane.serve() {
            eprintln!("serve: {}", e.0);
        }
    });

    // the agent's verbs are on its own window, and there is no window
    // of apex-agent's own
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut menu = Vec::new();
    while Instant::now() < deadline && !["Transcript", "Preview", "Changes"].iter().all(|x| menu.iter().any(|y| y == x)) {
        let _ = c.step(Duration::from_millis(20));
        menu = apex_core::plumb::verbs_for(&c.node.state.meta.rules, &c.node.window_name(tw), c.node.window_kind(tw), Some(tw), c.node.window_owner(tw));
    }
    assert!(["Transcript", "Preview", "Changes"].iter().all(|x| menu.iter().any(|y| y == x)), "{menu:?}");
    assert!(window_named(&c, &format!("{}/-agents", tmp.display())).is_none(), "no pane was asked for");
    // at work, it wants nothing
    assert_eq!(flags(&c), Vec::new());

    // the turn ends: it wants you, and its terminal is notified
    event::append(&logs, &Event { text: Some("It names localhost.".into()), ..ev("Stop") }).unwrap();
    assert_eq!(wait_flags(&mut c, |f| !f.is_empty()), vec![("agents".to_string(), tw)]);

    // the next turn begins: the flag goes
    event::append(&logs, &Event { text: Some("and /etc/passwd?".into()), ..ev("UserPromptSubmit") }).unwrap();
    wait_flags(&mut c, |f| f.is_empty());

    // a question is wanting you too, and so is a turn that failed
    event::append(&logs, &Event { call: Some("t7".into()), title: Some("Bash: rm -rf target".into()), ..ev("PermissionRequest") }).unwrap();
    assert_eq!(wait_flags(&mut c, |f| !f.is_empty()), vec![("agents".to_string(), tw)]);

    // the user takes it: it is not raised again while the agent goes on
    // wanting the same thing
    let mut ui = Remote::connect_as(&sock, "main", "ui", AttachmentKind::Ui).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while ui.node.state.meta.notifications.is_empty() && Instant::now() < deadline {
        let _ = ui.step(Duration::from_millis(20));
    }
    ui.send(&ClientMsg::Unnotify { window: tw });
    wait_flags(&mut c, |f| f.is_empty());
    event::append(&logs, &Event { call: Some("t7".into()), kind: Some("deny".into()), ..ev("Decision") }).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let _ = c.step(Duration::from_millis(50));
    assert_eq!(flags(&c), Vec::new(), "a notification the user took came back");

    // back to work, and wanting you afresh: raised again
    event::append(&logs, &Event { call: Some("t7".into()), ..ev("PostToolUse") }).unwrap();
    event::append(&logs, &Event { text: Some("Left it.".into()), ..ev("Stop") }).unwrap();
    assert_eq!(wait_flags(&mut c, |f| !f.is_empty()), vec![("agents".to_string(), tw)]);

    // the agent goes, and so does its flag
    event::append(&logs, &ev("SessionEnd")).unwrap();
    wait_flags(&mut c, |f| f.is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}
