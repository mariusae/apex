//! B3 on a name from `~`: plumbing expands it, to the home directory,
//! and the window opens at the file's own path. A test binary of its
//! own, since it says where home is for the whole process.

use apex_core::*;
use apex_server::{PlumbReq, PlumbStep, Proposal, Server};

#[test]
fn a_name_from_home_opens_the_file_there() {
    let home = std::env::temp_dir().join(format!("apex-home-{}", std::process::id()));
    let src = home.join("src/tries/x/crates/a/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("main.rs"), "fn main() {}\n").unwrap();
    std::env::set_var("HOME", &home);
    let file = src.join("main.rs").display().to_string();

    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "test");
    let mut node = Node::new(a);
    node.catch_up(&log).unwrap();
    let col = node.init_session(&mut log).unwrap();
    let (mut server, _rx) = Server::new(&log);
    server.install_default_rules(&mut log);
    node.catch_up(&log).unwrap();
    let text = "Read ~/src/tries/x/crates/a/src/main.rs:1-130 to see\n";
    let w = node.new_window(&mut log, col, "/tmp/notes", text).unwrap();
    let b = node.state.window(w).unwrap().body_buffer().unwrap();
    let opened = |step: PlumbStep| match step {
        PlumbStep::Done(props) => props.into_iter().find_map(|p| match p {
            Proposal::Goto { loc } => Some((loc.name, loc.pos)),
            _ => None,
        }),
        other => panic!("{other:?}"),
    };

    // a click on the ~, in the name, on the range: the file, at its first line
    for q in [5, 20, 43] {
        let at = Span { buffer: b, q0: q, q1: q };
        let req = PlumbReq { ctx: ExecCtx::Window(w), text: String::new(), dir: None, verb: "plumb".into(), edit_only: false, dry: false, exec: None, at: Some(at), sel: None, alt: None, reverse: false };
        let (_, step) = server.plumb_start(&node, req);
        assert_eq!(opened(step), Some((file.clone(), Pos::Line(1))), "at {q}");
    }
    // the same text without a place (a terminal's B3, apex plumb)
    let req = PlumbReq { ctx: ExecCtx::Window(w), text: "~/src/tries/x/crates/a/src/main.rs:1-130".into(), dir: None, verb: "plumb".into(), edit_only: false, dry: false, exec: None, at: None, sel: None, alt: None, reverse: false };
    let (_, step) = server.plumb_start(&node, req);
    assert_eq!(opened(step), Some((file.clone(), Pos::Line(1))));
    // a name from ~ that is no file is a word, as ever
    let req = PlumbReq { ctx: ExecCtx::Window(w), text: "~/src/nothing.rs:3".into(), dir: None, verb: "plumb".into(), edit_only: false, dry: false, exec: None, at: None, sel: None, alt: None, reverse: false };
    let (_, step) = server.plumb_start(&node, req);
    assert!(matches!(step, PlumbStep::Refused { .. }), "{step:?}");
    let _ = std::fs::remove_dir_all(&home);
}
