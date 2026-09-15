//! The pane against a daemon on a thread: an agent's log makes a block,
//! B3 in the block opens the agent's transcript, which follows the
//! transcript file and the hooks, and the agent's end takes it away.
//! Each change is waited for less long than the pane's slow pass, so
//! it is the watches that are seen to work.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_agent::event::{self, Event};
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
    let deadline = Instant::now() + Duration::from_secs(3);
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
    std::fs::create_dir_all(&proj).unwrap();
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
    event::append(&logs, &Event { kind: Some("startup".into()), ..ev("SessionStart") }).unwrap();
    event::append(&logs, &Event { text: Some("what is in hosts?".into()), ..ev("UserPromptSubmit") }).unwrap();
    event::append(&logs, &Event { call: Some("t1".into()), tool: Some("Read".into()), title: Some("Read: /etc/hosts".into()), ..ev("PreToolUse") }).unwrap();

    let mut t = Tool::attach_to(&sock, "main", "agents").unwrap();
    // the page's converter: the markdown itself, so the test can read it
    t.set("Preview.md", "cat");
    let pane = Pane::start(t, Opts { cwd: tmp.clone(), dir: logs.clone(), thoughts: false }).unwrap();
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
    // told where it was started, Goto has somewhere to go, and says nothing
    event::append(&logs, &Event { apex: Some("main".into()), win: Some(1), ..ev("PermissionRequest") }).unwrap();
    wait_text(&mut c, &pane_name, |t| t.contains("\n? claude"));
    c.propose(apex_server::Proposal::Exec { ctx: ExecCtx::Window(w), text: "Goto 0b1c".into() }, Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let _ = c.step(Duration::from_millis(20));
    let said: String = c.node.state.windows.keys().filter(|x| c.node.window_name(**x).ends_with("+Errors")).map(|x| text_of(&c, *x)).collect();
    assert_eq!(said.matches("Goto").count(), 1, "{said:?}");

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
