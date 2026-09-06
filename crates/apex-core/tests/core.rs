//! Acceptance and property tests for the core: a leader node drives a
//! session through the log store, a follower replays it, and the two must
//! agree byte for byte; plus fencing, undo, views, commands and Edit.

use apex_core::*;
use proptest::prelude::*;

fn session() -> (Log, Node, ColumnId) {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "ui");
    let mut node = Node::new(a);
    node.catch_up(&log).unwrap();
    let col = node.init_session(&mut log).unwrap();
    (log, node, col)
}

fn follower(log: &Log) -> Node {
    let mut f = Node::new(AttachmentId(4242));
    f.catch_up(log).unwrap();
    f
}

fn text(node: &Node, w: WindowId) -> String {
    let b = node.state.window(w).unwrap().body_buffer().unwrap();
    node.state.buffer(b).unwrap().text.to_string()
}

fn sel(node: &Node, v: ViewId) -> (usize, usize) {
    node.selection(v).unwrap()
}

#[test]
fn typing_and_replication() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f.txt", "").unwrap();
    let v = ViewId::Body(w);
    for c in "hello".chars() {
        node.insert(&mut log, v, &c.to_string()).unwrap();
    }
    node.backspace(&mut log, v).unwrap();
    node.insert(&mut log, v, "p!").unwrap();
    assert_eq!(text(&node, w), "hellp!");
    assert_eq!(sel(&node, v), (6, 6));
    let f = follower(&log);
    assert_eq!(f.state.hash(), node.state.hash());
    assert_eq!(text(&f, w), "hellp!");
}

#[test]
fn zerox_shares_the_buffer_with_independent_selections() {
    let (mut log, mut node, col) = session();
    let w1 = node.new_window(&mut log, col, "f", "one two three").unwrap();
    let w2 = node.zerox(&mut log, w1).unwrap();
    let (v1, v2) = (ViewId::Body(w1), ViewId::Body(w2));
    node.select(&mut log, v1, 0, 3).unwrap(); // "one"
    node.select(&mut log, v2, 8, 13).unwrap(); // "three"
    node.replace_selection(&mut log, v1, "ONE!").unwrap();
    assert_eq!(text(&node, w2), "ONE! two three");
    assert_eq!(sel(&node, v2), (9, 14)); // shifted, still "three"
    assert_eq!(node.selected_text(v2).unwrap(), "three");
    // undo in either window undoes the shared buffer
    node.undo(&mut log, v2).unwrap();
    assert_eq!(text(&node, w1), "one two three");
    assert_eq!(f_hash(&log), node.state.hash());
    // deleting one window keeps the buffer; deleting the last removes it
    node.delete_window(&mut log, w1).unwrap();
    assert!(node.state.buffers.values().any(|b| b.name == "f"));
    node.delete_window(&mut log, w2).unwrap();
    assert!(!node.state.buffers.values().any(|b| b.name == "f"));
    assert_eq!(f_hash(&log), node.state.hash());
}

fn f_hash(log: &Log) -> [u8; 32] {
    follower(log).state.hash()
}

#[test]
fn cut_snarf_paste_undo_redo() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f", "abcdef").unwrap();
    let v = ViewId::Body(w);
    node.select(&mut log, v, 1, 3).unwrap();
    node.cut(&mut log, v).unwrap();
    assert_eq!(text(&node, w), "adef");
    assert_eq!(node.state.layout.snarf, "bc");
    node.select(&mut log, v, 4, 4).unwrap();
    node.paste(&mut log, v).unwrap();
    assert_eq!(text(&node, w), "adefbc");
    assert_eq!(sel(&node, v), (4, 6)); // pasted text selected
    assert!(node.undo(&mut log, v).unwrap());
    assert_eq!(text(&node, w), "adef");
    assert!(node.undo(&mut log, v).unwrap());
    assert_eq!(text(&node, w), "abcdef");
    assert_eq!(sel(&node, v), (1, 3));
    assert!(!node.undo(&mut log, v).unwrap());
    assert!(node.redo(&mut log, v).unwrap());
    assert_eq!(text(&node, w), "adef");
    assert_eq!(f_hash(&log), node.state.hash());
}

#[test]
fn typing_undoes_as_one_group_until_a_mouse_action() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f", "").unwrap();
    let v = ViewId::Body(w);
    for c in "abc".chars() {
        node.insert(&mut log, v, &c.to_string()).unwrap();
    }
    node.select(&mut log, v, 3, 3).unwrap(); // a click ends the run
    for c in "def".chars() {
        node.insert(&mut log, v, &c.to_string()).unwrap();
    }
    node.undo(&mut log, v).unwrap();
    assert_eq!(text(&node, w), "abc");
    node.undo(&mut log, v).unwrap();
    assert_eq!(text(&node, w), "");
}

#[test]
fn edit_language_is_lowered_into_entries() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f", "a1b22c\n").unwrap();
    let v = ViewId::Body(w);
    let run = node.run_edit(&mut log, w, ",x/[0-9]+/ c/N/").unwrap();
    assert_eq!(text(&node, w), "aNbNc\n");
    assert_eq!(sel(&node, v), (1, 4)); // acme's merged-change selection
    assert!(run.output.is_empty());
    let run = node.run_edit(&mut log, w, ",p").unwrap();
    assert_eq!(run.output, "aNbNc\n");
    // one undo group for the whole program
    node.undo(&mut log, v).unwrap();
    assert_eq!(text(&node, w), "a1b22c\n");
    // the empty pattern remembers the last one across programs
    node.run_edit(&mut log, w, ",s/[0-9]+/X/").unwrap();
    node.run_edit(&mut log, w, ",x// c/Y/").unwrap();
    assert_eq!(text(&node, w), "aXbYc\n");
    assert_eq!(f_hash(&log), node.state.hash());
    // errors are errors
    assert!(node.run_edit(&mut log, w, "s/zzz/y/").is_err());
}

#[test]
fn commands_are_logged_with_their_handler() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f", "hello world").unwrap();
    let v = ViewId::Body(w);
    node.select(&mut log, v, 0, 5).unwrap();
    // a built-in runs here and is Done
    let r = node.exec(&mut log, ExecCtx::Window(w), "Cut").unwrap();
    assert!(matches!(r, Executed::Done(_)));
    assert_eq!(text(&node, w), " world");
    let win = node.state.window(w).unwrap();
    let rec = win.execs.values().last().unwrap();
    assert_eq!(rec.handler, Handler::Leader);
    assert_eq!(rec.status, state::ExecStatus::Done);
    assert_eq!(rec.at.q1, 5);
    // a server command is recorded and deferred
    let r = node.exec(&mut log, ExecCtx::Window(w), "Put").unwrap();
    assert!(matches!(r, Executed::Deferred(_)));
    assert_eq!(node.state.window(w).unwrap().execs.values().last().unwrap().status, state::ExecStatus::Pending);
    // Look searches the body
    node.select(&mut log, v, 0, 0).unwrap();
    node.exec(&mut log, ExecCtx::Window(w), "Look orl").unwrap();
    assert_eq!(sel(&node, v), (2, 5));
    // Edit from the tag runs on the body and prints to +Errors
    node.exec(&mut log, ExecCtx::Window(w), "Edit ,p").unwrap();
    let errors = node.state.buffers.values().find(|b| b.name == "+Errors").expect("+Errors created");
    assert_eq!(errors.text.to_string(), " world");
    // Exit asks the client to quit
    assert!(matches!(node.exec(&mut log, ExecCtx::Top, "Exit").unwrap(), Executed::Quit(_)));
    assert_eq!(f_hash(&log), node.state.hash());
}

#[test]
fn del_warns_once_on_a_dirty_buffer() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f", "x").unwrap();
    node.insert(&mut log, ViewId::Body(w), "y").unwrap();
    // acme's winclean: the first Del warns in +Errors and does nothing
    let r = node.exec(&mut log, ExecCtx::Window(w), "Del").unwrap();
    assert!(matches!(r, Executed::Done(_)));
    assert!(node.state.window(w).is_ok());
    let errors = node.state.buffers.values().find(|b| b.name.ends_with("+Errors")).map(|b| b.text.to_string()).unwrap_or_default();
    assert!(errors.contains("f modified"), "{errors:?}");
    let r = node.exec(&mut log, ExecCtx::Window(w), "Del").unwrap();
    assert!(matches!(r, Executed::Done(_)));
    assert!(node.state.window(w).is_err());
    // Zerox'd window: Del without warning while another view remains
    let w = node.new_window(&mut log, col, "g", "x").unwrap();
    node.insert(&mut log, ViewId::Body(w), "y").unwrap();
    let w2 = node.zerox(&mut log, w).unwrap();
    assert!(matches!(node.exec(&mut log, ExecCtx::Window(w2), "Del").unwrap(), Executed::Done(_)));
    assert_eq!(f_hash(&log), node.state.hash());
}

#[test]
fn columns_new_delete_sort() {
    let (mut log, mut node, col) = session();
    let c2 = match node.exec(&mut log, ExecCtx::Top, "Newcol").unwrap() {
        Executed::Done(_) => node.state.layout.cols[1].id,
        other => panic!("{other:?}"),
    };
    let wb = node.new_window(&mut log, c2, "b", "").unwrap();
    let wa = node.new_window(&mut log, c2, "a", "").unwrap();
    node.exec(&mut log, ExecCtx::Column(c2), "Sort").unwrap();
    let wins: Vec<WindowId> = node.state.layout.column(c2).unwrap().wins.iter().map(|s| s.window).collect();
    // acme's Newcol made an empty window too; its empty name sorts first
    assert_eq!(wins.len(), 3);
    assert_eq!(&wins[1..], &[wa, wb]);
    assert!(matches!(node.exec(&mut log, ExecCtx::Column(c2), "Delcol").unwrap(), Executed::Done(_)));
    assert_eq!(node.state.layout.cols.len(), 1);
    // acme lets the last column go too; New makes one again
    assert!(matches!(node.exec(&mut log, ExecCtx::Column(col), "Delcol").unwrap(), Executed::Done(_)));
    assert_eq!(node.state.layout.cols.len(), 0);
    node.exec(&mut log, ExecCtx::Top, "New").unwrap();
    assert_eq!(node.state.layout.cols.len(), 1);
    assert_eq!(f_hash(&log), node.state.hash());
}

#[test]
fn fencing_rejects_a_stale_leader() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f", "abc").unwrap();
    let b = node.state.window(w).unwrap().body_buffer().unwrap();
    // a second UI attaches; the first does not answer, so it is reclaimed
    let (a2, _) = log.attach(AttachmentKind::Ui, "ui2");
    log.reclaim(Shard::Buffer(b)).unwrap();
    let mut node2 = Node::new(a2);
    node2.catch_up(&log).unwrap();
    node2.take_lease(&mut log, Shard::Buffer(b)).unwrap();
    // the old leader's appends are refused
    let err = node.insert(&mut log, ViewId::Body(w), "x").unwrap_err();
    assert!(matches!(err, CoreError::Log(LogError::Fenced { .. })));
    // the new leader's are not, and the old node catches up as a follower
    node2.select(&mut log, ViewId::Body(w), 0, 0).unwrap();
    node2.insert(&mut log, ViewId::Body(w), "Z").unwrap();
    node.catch_up(&log).unwrap();
    assert_eq!(text(&node, w), "Zabc");
    assert_eq!(node.state.hash(), node2.state.hash());
    assert!(!node.leads(Shard::Buffer(b)));
    // cooperative transfer back: release, then grant
    node2.release_lease(&mut log, Shard::Buffer(b)).unwrap();
    node.take_lease(&mut log, Shard::Buffer(b)).unwrap();
    node.insert(&mut log, ViewId::Body(w), "!").unwrap();
    assert_eq!(text(&node, w), "Z!abc");
}

#[test]
fn snapshot_plus_tail_equals_replay() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "f", "").unwrap();
    let v = ViewId::Body(w);
    for i in 0..50 {
        node.insert(&mut log, v, &format!("{i} ")).unwrap();
    }
    // snapshot here
    let snap = node.state.to_snapshot();
    let at: Vec<(Shard, Seq)> = node.state.applied.iter().map(|(s, q)| (*s, *q)).collect();
    for i in 50..80 {
        node.insert(&mut log, v, &format!("{i} ")).unwrap();
    }
    node.run_edit(&mut log, w, ",x/7/ c/seven/").unwrap();
    // restore from the snapshot and apply the tail
    let mut restored = Node::new(AttachmentId(7));
    restored.state = State::from_snapshot(&snap).unwrap();
    for (shard, seq) in &at {
        assert_eq!(restored.state.applied(*shard), *seq);
    }
    restored.catch_up(&log).unwrap();
    assert_eq!(restored.state.hash(), node.state.hash());
    // and compacting everything the snapshot covers changes nothing for a
    // node restored from it
    for (shard, seq) in &at {
        log.compact(*shard, *seq);
    }
    let mut again = Node::new(AttachmentId(8));
    again.state = State::from_snapshot(&snap).unwrap();
    again.catch_up(&log).unwrap();
    assert_eq!(again.state.hash(), node.state.hash());
}

// ---- property tests -------------------------------------------------------

#[derive(Debug, Clone)]
enum Action {
    Insert(String),
    Backspace,
    Delete,
    Select(usize, usize),
    Cut,
    Paste,
    Undo,
    Redo,
    Zerox,
    Edit(&'static str),
    NewWindow,
    DelWindow,
    Errors(String),
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        8 => "[a-c \n]{1,3}".prop_map(Action::Insert),
        3 => Just(Action::Backspace),
        2 => Just(Action::Delete),
        4 => (0..20usize, 0..20usize).prop_map(|(a, b)| Action::Select(a.min(b), a.max(b))),
        2 => Just(Action::Cut),
        2 => Just(Action::Paste),
        3 => Just(Action::Undo),
        2 => Just(Action::Redo),
        1 => Just(Action::Zerox),
        2 => prop_oneof![Just(",x/a/ c/X/"), Just(",s/b+/Y/g"), Just("1,$d"), Just(",x/^/ i/> /"), Just("$a/end\\n/")]
            .prop_map(Action::Edit),
        1 => Just(Action::NewWindow),
        1 => Just(Action::DelWindow),
        1 => "[a-z]{1,4}".prop_map(Action::Errors),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, .. ProptestConfig::default() })]

    /// Whatever the leader does, a follower replaying the log, and a node
    /// restored from a mid-way snapshot plus the tail, end up identical.
    #[test]
    fn replicas_agree(actions in prop::collection::vec(action(), 1..60)) {
        let (mut log, mut node, col) = session();
        let mut windows = vec![node.new_window(&mut log, col, "f", "seed text\n").unwrap()];
        let mut snapshot: Option<Vec<u8>> = None;
        for (i, a) in actions.iter().enumerate() {
            if i == actions.len() / 2 {
                snapshot = Some(node.state.to_snapshot());
            }
            let w = *windows.last().unwrap();
            let v = ViewId::Body(w);
            let r = match a {
                Action::Insert(s) => node.insert(&mut log, v, s),
                Action::Backspace => node.backspace(&mut log, v),
                Action::Delete => node.delete_forward(&mut log, v),
                Action::Select(a, b) => node.select(&mut log, v, *a, *b),
                Action::Cut => node.cut(&mut log, v),
                Action::Paste => node.paste(&mut log, v),
                Action::Undo => node.undo(&mut log, v).map(|_| ()),
                Action::Redo => node.redo(&mut log, v).map(|_| ()),
                Action::Zerox => node.zerox(&mut log, w).map(|w2| windows.push(w2)),
                Action::Edit(p) => match node.run_edit(&mut log, w, p) {
                    Ok(_) => Ok(()),
                    Err(CoreError::Edit(_)) => Ok(()), // Edit errors are fine (e.g. nothing to delete)
                    Err(e) => Err(e),
                },
                Action::NewWindow => node.new_window(&mut log, col, "g", "").map(|w2| windows.push(w2)),
                Action::DelWindow => {
                    if windows.len() > 1 {
                        let w = windows.pop().unwrap();
                        node.delete_window(&mut log, w)
                    } else {
                        Ok(())
                    }
                }
                Action::Errors(s) => node.errors(&mut log, None, s).map(|_| ()),
            };
            prop_assert!(r.is_ok(), "{a:?} failed: {r:?}");
            // selections stay inside their buffers
            for b in node.state.buffers.values() {
                for (id, view) in &b.views {
                    prop_assert!(view.q0 <= view.q1 && view.q1 <= b.text.len(), "view {id} out of range");
                }
            }
        }
        let f = follower(&log);
        prop_assert_eq!(f.state.hash(), node.state.hash());
        prop_assert_eq!(&f.state, &node.state);
        if let Some(snap) = snapshot {
            let mut r = Node::new(AttachmentId(9));
            r.state = State::from_snapshot(&snap).unwrap();
            r.catch_up(&log).unwrap();
            prop_assert_eq!(r.state.hash(), node.state.hash());
        }
    }

    /// n undos after n edits restore the text; n redos restore the edits.
    #[test]
    fn undo_redo_round_trip(edits in prop::collection::vec("[a-c]{1,3}", 1..20)) {
        let (mut log, mut node, col) = session();
        let w = node.new_window(&mut log, col, "f", "base").unwrap();
        let v = ViewId::Body(w);
        let mut texts = vec![text(&node, w)];
        for (i, e) in edits.iter().enumerate() {
            let len = text(&node, w).chars().count();
            let p = (i * 7) % (len + 1);
            node.select(&mut log, v, p, p).unwrap();
            node.replace_selection(&mut log, v, e).unwrap();
            texts.push(text(&node, w));
        }
        for i in (0..edits.len()).rev() {
            prop_assert!(node.undo(&mut log, v).unwrap());
            prop_assert_eq!(text(&node, w), texts[i].clone());
        }
        prop_assert!(!node.undo(&mut log, v).unwrap());
        for i in 0..edits.len() {
            prop_assert!(node.redo(&mut log, v).unwrap());
            prop_assert_eq!(text(&node, w), texts[i + 1].clone());
        }
        prop_assert_eq!(f_hash(&log), node.state.hash());
    }

    /// Lowering an Edit program through the core gives the same text as
    /// applying apex-edit's changes directly.
    #[test]
    fn edit_lowering_matches_apex_edit(base in "[a-c \n]{0,20}", prog in prop_oneof![
        Just(",x/a/ c/X/"), Just(",s/b+/Y/g"), Just(",x/^/ i/> /"), Just("$a/end\\n/"), Just(",y/a/ c/-/")
    ]) {
        let (mut log, mut node, col) = session();
        let w = node.new_window(&mut log, col, "f", &base).unwrap();
        let run = node.run_edit(&mut log, w, prog);
        let mut direct: Vec<char> = base.chars().collect();
        let expect = apex_edit::Edit::new().run(&direct, (0, 0), None, prog);
        match (run, expect) {
            (Ok(_), Ok(o)) => {
                apex_edit::apply(&mut direct, &o.changes);
                prop_assert_eq!(text(&node, w), direct.iter().collect::<String>());
                prop_assert_eq!(sel(&node, ViewId::Body(w)), o.dot);
            }
            (Err(_), Err(_)) => {}
            (a, b) => prop_assert!(false, "core {a:?} vs edit {b:?}"),
        }
    }
}

#[test]
fn errors_go_to_the_directory_window_in_the_last_column() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "/tmp/proj/main.rs", "fn main() {}\n").unwrap();
    let c2 = node.new_column(&mut log, None).unwrap();
    // an error from main.rs's window lands in /tmp/proj/+Errors, in the last column
    let dir = node.error_dir(Some(w));
    assert_eq!(dir.as_deref(), Some("/tmp/proj"));
    let e = node.errors(&mut log, dir.as_deref(), "boom\n").unwrap();
    assert_eq!(node.window_name(e), "/tmp/proj/+Errors");
    assert_eq!(node.state.layout.column_of(e), Some(c2));
    // its tag has no Undo/Put words (acme: filemenu off), and Del never asks
    node.update_tags(&mut log).unwrap();
    let tag = node.state.buffer(node.state.window(e).unwrap().tag).unwrap().text.to_string();
    assert!(tag.starts_with("/tmp/proj/+Errors Del Snarf |"), "{tag}");
    assert!(matches!(node.exec(&mut log, ExecCtx::Window(e), "Del").unwrap(), Executed::Done(_)));
    assert!(node.state.window(e).is_err());
    // no directory: plain +Errors
    let e = node.errors(&mut log, None, "x\n").unwrap();
    assert_eq!(node.window_name(e), "+Errors");
}

#[test]
fn send_appends_to_a_text_window_and_zerox_refuses_directories() {
    let (mut log, mut node, col) = session();
    let w = node.new_window(&mut log, col, "notes", "a\n").unwrap();
    node.append(&mut log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text: "from snarf".into() })).unwrap();
    node.exec(&mut log, ExecCtx::Window(w), "Send").unwrap();
    let b = node.view_buffer(ViewId::Body(w)).unwrap();
    assert_eq!(node.state.buffer(b).unwrap().text.to_string(), "a\nfrom snarf\n");
    let d = node.new_window(&mut log, col, "/tmp/", "x\n").unwrap();
    node.exec(&mut log, ExecCtx::Window(d), "Zerox").unwrap();
    assert_eq!(node.state.windows.values().filter(|x| x.body_buffer() == node.state.window(d).unwrap().body_buffer()).count(), 1);
    let errs = node.state.buffers.values().find(|b| b.name.ends_with("+Errors")).map(|b| b.text.to_string()).unwrap_or_default();
    assert!(errs.contains("is a directory; Zerox illegal"), "{errs}");
}
