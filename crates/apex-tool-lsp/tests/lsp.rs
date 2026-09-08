//! apex lsp against a fake language server: documents open and sync
//! incrementally, diagnostics land in root/+lsp, B3 goes to the
//! definition, and verbs act.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::ClientMsg;
use apex_server::remote::Remote;
use apex_server::Proposal;

fn daemon() -> PathBuf {
    let path = std::env::temp_dir().join(format!("apex-lsp-{}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(&path).is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    path
}

fn until(c: &mut Remote, mut done: impl FnMut(&Node) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if done(&c.node) {
            return true;
        }
        let _ = c.step(Duration::from_millis(50));
    }
    done(&c.node)
}

fn text_of(n: &Node, name: &str) -> Option<String> {
    n.state.buffers.values().find(|b| b.name.ends_with(name)).map(|b| b.text.to_string())
}

#[test]
fn documents_sync_diagnostics_show_and_verbs_act() {
    let sock = daemon();
    let root = std::env::temp_dir().join(format!("apex-lsp-root-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("go.mod"), "module x\n").unwrap();
    let main = root.join("main.go");
    std::fs::write(&main, "package main\nfunc f() {}\n").unwrap();
    let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake-lsp.py");
    // a tool (the test) sets the server and opens the file
    let mut c = Remote::connect_as(&sock, "main", "test", AttachmentKind::Tool).unwrap();
    c.send(&ClientMsg::Set { key: "lsp.go".into(), value: format!("python3 {} --delay-initialize", fake.display()), attachment: None });
    let col = c.node.state.layout.cols[0].id;
    c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: main.display().to_string() });
    assert!(until(&mut c, |n| text_of(n, "main.go").is_some()), "file opened");
    // the tool
    let s2 = sock.clone();
    std::thread::spawn(move || {
        let _ = apex_tool_lsp::run(&s2, "main");
    });
    // Its rules arrive before the deliberately delayed initialize response.
    assert!(until(&mut c, |n| n.state.meta.rules.values().any(|r| r.rule.verb == "Fmt")), "rules installed");
    let (b, _) = c.node.state.buffers.values().find(|b| b.name.ends_with("main.go")).map(|b| (b.id, b.version)).unwrap();
    let w = c.node.state.windows.keys().copied().find(|w| c.node.window_name(*w).ends_with("main.go")).unwrap();
    c.propose(Proposal::Select { view: ViewId::Body(w), q0: 18, q1: 19 }, Duration::from_secs(5)).unwrap();
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Def".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text_of(n, "+Errors").unwrap_or_default().contains("Def: language server is still initializing")), "initializing error: {:?}", text_of(&c.node, "+Errors"));
    // The diagnostics window then contains the opened text's length.
    let lsp = |n: &Node| text_of(n, "+lsp").unwrap_or_default();
    assert!(until(&mut c, |n| lsp(n).contains("len=25 first=package")), "diagnostics: {}", lsp(&c.node));
    assert!(lsp(&c.node).contains("main.go:1:1: warning:"), "{}", lsp(&c.node));
    // an edit syncs incrementally: the server sees the new length
    let version = c.node.state.buffer(b).unwrap().version;
    c.propose(Proposal::ReplaceRange { dir: None, buffer: b, version, q0: 25, q1: 25, text: "// more\n".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| lsp(n).contains("len=33")), "synced: {}", lsp(&c.node));
    // Once initialized, B3 on an identifier goes to the definition.
    c.propose(Proposal::Select { view: ViewId::Body(w), q0: 0, q1: 0 }, Duration::from_secs(5)).unwrap();
    c.send(&ClientMsg::Plumb { ctx: ExecCtx::Window(w), text: "f".into(), dir: None, edit_only: false, dry: false, at: Some(Span { buffer: b, q0: 18, q1: 18 }), sel: Some(Span { buffer: b, q0: 18, q1: 19 }), alt: None, reverse: false });
    assert!(until(&mut c, |n| n.selection(ViewId::Body(w)).ok() == Some((18, 19))), "selection: {:?}", c.node.selection(ViewId::Body(w)));
    // Hov: the hover text lands in +Errors
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Hov".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text_of(n, "+Errors").unwrap_or_default().contains("hover: f is a func")), "hover: {:?}", text_of(&c.node, "+Errors"));
    // Fmt: the server's edit replaces the text
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Fmt".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| text_of(n, "main.go").as_deref() == Some("package main\n\nfunc f() {}\n")), "formatted: {:?}", text_of(&c.node, "main.go"));
    // what the verbs menu would offer this window: the lsp's, and the stack's
    let verbs = apex_core::plumb::verbs_for(&c.node.state.meta.rules, &c.node.window_name(w), c.node.window_kind(w));
    let want: Vec<String> = apex_tool_lsp::VERBS.iter().chain(apex_tool_lsp::NAV_VERBS.iter()).map(|s| s.to_string()).collect();
    assert_eq!(verbs, want);
    // Def recorded where we came from: Back returns there
    c.propose(Proposal::Select { view: ViewId::Body(w), q0: 0, q1: 0 }, Duration::from_secs(5)).unwrap();
    // (the f moved by one with the formatting; the server reads the text there)
    c.send(&ClientMsg::Plumb { ctx: ExecCtx::Window(w), text: "f".into(), dir: None, edit_only: false, dry: false, at: Some(Span { buffer: b, q0: 19, q1: 19 }), sel: Some(Span { buffer: b, q0: 19, q1: 20 }), alt: None, reverse: false });
    // (the formatted text's line 1 is empty: the server's column clamps to its start)
    assert!(until(&mut c, |n| n.selection(ViewId::Body(w)).ok() == Some((13, 13))), "def again: {:?}", c.node.selection(ViewId::Body(w)));
    assert!(until(&mut c, |n| !n.state.layout.nav_back.is_empty()), "stack");
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Back".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |n| n.selection(ViewId::Body(w)).ok() == Some((0, 0))), "back: {:?}", c.node.selection(ViewId::Body(w)));
    let _ = std::fs::remove_dir_all(&root);
}
