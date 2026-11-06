//! A session written down and made again (`dump`): the same windows,
//! texts and tiling, under new ids.

use std::collections::BTreeMap;

use apex_core::dump::{self, Dump, Host};
use apex_core::*;

fn session() -> (Log, Node, ColumnId) {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "ui");
    let mut node = Node::new(a);
    node.catch_up(&log).unwrap();
    let col = node.init_session(&mut log).unwrap();
    (log, node, col)
}

/// A fresh session's node, nothing laid out yet, as the daemon makes
/// one to load a dump into.
fn bare() -> (Log, Node) {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "ui");
    let mut node = Node::new(a);
    node.catch_up(&log).unwrap();
    (log, node)
}

/// The disk as the loader finds it; no terminals.
struct Disk(BTreeMap<String, String>);

impl Host for Disk {
    fn read(&mut self, name: &str) -> Option<(String, String)> {
        self.0.get(name).map(|t| (name.to_string(), t.clone()))
    }
    fn term(&mut self, _: &mut Log, _: &str, _: &str) -> Option<TermId> {
        None
    }
}

fn file(log: &mut Log, node: &mut Node, col: ColumnId, name: &str, text: &str) -> WindowId {
    let b = node.create_buffer(log, name, text, Some(Text::new(text).content_hash())).unwrap();
    node.open_window(log, col, b).unwrap()
}

fn body(node: &Node, w: WindowId) -> &apex_core::buffer::Buffer {
    node.state.buffer(node.state.window(w).unwrap().body_buffer().unwrap()).unwrap()
}

fn named(node: &Node, name: &str) -> Vec<WindowId> {
    node.state.windows.values().filter(|w| w.body_buffer().is_some_and(|b| node.state.buffer(b).unwrap().name == name)).map(|w| w.id).collect()
}

/// Each column's windows by name, top to bottom, and the stash's.
fn tiling(node: &Node) -> (Vec<Vec<String>>, Vec<String>) {
    let name = |w: WindowId| node.state.window(w).ok().and_then(|x| x.body_buffer()).map(|b| node.state.buffer(b).unwrap().name.clone()).unwrap_or_default();
    let cols = node.state.layout.cols.iter().map(|c| c.wins.iter().map(|s| name(s.window)).collect()).collect();
    (cols, node.state.layout.stash.iter().map(|s| name(s.slot.window)).collect())
}

/// Through its text, as the daemon writes it and reads it back.
fn again(d: &Dump) -> Dump {
    serde_json::from_str(&serde_json::to_string(d).unwrap()).unwrap()
}

#[test]
fn a_session_is_made_again_as_it_was() {
    let (mut log, mut node, col) = session();
    let first = node.state.layout.cols[0].id;
    let clean = file(&mut log, &mut node, col, "/w/clean.txt", "as on disk\n");
    let dirty = file(&mut log, &mut node, col, "/w/dirty.txt", "old\n");
    node.select(&mut log, ViewId::Body(dirty), 0, 0).unwrap();
    node.insert(&mut log, ViewId::Body(dirty), "new ").unwrap();
    let scratch = node.new_window_as(&mut log, first, "/w/+notes", "nowhere else\n", &Spec::scratch(None)).unwrap();
    let copy = node.zerox(&mut log, clean).unwrap();
    // a tag with the user's words in it, a selection, a font
    let tag = node.state.window(scratch).unwrap().tag;
    node.set_content(&mut log, tag, "Look Put mk ").unwrap();
    node.select(&mut log, ViewId::Body(scratch), 3, 7).unwrap();
    node.append(&mut log, Shard::Window(scratch), Op::Window(WindowOp::Font { mono: true })).unwrap();
    node.stash_window(&mut log, copy).unwrap();
    let was = tiling(&node);

    let d = again(&dump::dump(&node.state, &mut |_| None));
    // what is on disk as it was read is not written
    let text_of = |n: &str| d.buffers.iter().find(|b| b.name == n).unwrap().text.clone();
    assert_eq!(text_of("/w/clean.txt"), None);
    assert_eq!(text_of("/w/dirty.txt").as_deref(), Some("new old\n"));

    let (mut log2, mut node2) = bare();
    let mut disk = Disk(BTreeMap::from([("/w/clean.txt".into(), "as on disk\n".into()), ("/w/dirty.txt".into(), "old\n".into())]));
    node2.load_dump(&mut log2, &d, &mut disk).unwrap();

    assert_eq!(tiling(&node2), was);
    assert_eq!(node2.state.windows.len(), node.state.windows.len());
    // the tiling's geometry is the one it had
    let rects = |n: &Node| n.state.layout.cols.iter().map(|c| (c.r, c.wins.iter().map(|s| s.r).collect::<Vec<_>>())).collect::<Vec<_>>();
    assert_eq!(rects(&node2), rects(&node));
    // read again: clean
    let c = named(&node2, "/w/clean.txt");
    assert_eq!(c.len(), 2, "the window and its zerox, one buffer");
    assert_eq!(body(&node2, c[0]).text.to_string(), "as on disk\n");
    assert!(!body(&node2, c[0]).dirty());
    // unsaved: the text over the file's, dirty, not stale
    let w = named(&node2, "/w/dirty.txt")[0];
    assert_eq!(body(&node2, w).text.to_string(), "new old\n");
    assert!(body(&node2, w).dirty());
    assert!(!body(&node2, w).stale);
    // and undo takes it back to the file
    node2.undo(&mut log2, ViewId::Body(w)).unwrap();
    assert_eq!(body(&node2, w).text.to_string(), "old\n");
    // the scratch window's text, tag, selection and font
    let s = named(&node2, "/w/+notes")[0];
    let x = node2.state.window(s).unwrap();
    assert_eq!(body(&node2, s).text.to_string(), "nowhere else\n");
    assert!(body(&node2, s).scratch);
    assert_eq!(node2.state.buffer(x.tag).unwrap().text.to_string(), "Look Put mk ");
    assert!(x.mono);
    assert_eq!(node2.selection(ViewId::Body(s)).unwrap(), (3, 7));
}

#[test]
fn unsaved_text_over_a_file_changed_since_is_stale() {
    let (mut log, mut node, col) = session();
    let w = file(&mut log, &mut node, col, "/w/f.txt", "one\n");
    node.select(&mut log, ViewId::Body(w), 0, 0).unwrap();
    node.insert(&mut log, ViewId::Body(w), "mine ").unwrap();
    let d = again(&dump::dump(&node.state, &mut |_| None));
    let (mut log2, mut node2) = bare();
    let mut disk = Disk(BTreeMap::from([("/w/f.txt".into(), "two\n".into())]));
    node2.load_dump(&mut log2, &d, &mut disk).unwrap();
    let w = named(&node2, "/w/f.txt")[0];
    assert_eq!(body(&node2, w).text.to_string(), "mine one\n");
    assert!(body(&node2, w).dirty());
    assert!(body(&node2, w).stale);
}

#[test]
fn a_tiling_that_does_not_read_places_every_window() {
    let (mut log, mut node, col) = session();
    file(&mut log, &mut node, col, "/w/a.txt", "a\n");
    file(&mut log, &mut node, col, "/w/b.txt", "b\n");
    let mut d = again(&dump::dump(&node.state, &mut |_| None));
    d.layout = Some(serde_json::json!({"no": "such"}));
    let (mut log2, mut node2) = bare();
    let mut disk = Disk(BTreeMap::from([("/w/a.txt".into(), "a\n".into()), ("/w/b.txt".into(), "b\n".into())]));
    node2.load_dump(&mut log2, &d, &mut disk).unwrap();
    assert_eq!(node2.state.layout.cols.len(), 2);
    let placed: usize = node2.state.layout.cols.iter().map(|c| c.wins.len()).sum();
    assert_eq!(placed, 2);
}

#[test]
fn a_window_that_cannot_be_made_again_is_left_out() {
    // a terminal whose shell will not start: the rest come back, laid out
    let (mut log, mut node, col) = session();
    file(&mut log, &mut node, col, "/w/a.txt", "a\n");
    let mut d = again(&dump::dump(&node.state, &mut |_| None));
    let mut term = d.windows[0].clone();
    term.id = 999;
    term.body = dump::DumpBody::Term { history: "$ ls\n".into() };
    d.windows.push(term);
    let (mut log2, mut node2) = bare();
    let mut disk = Disk(BTreeMap::from([("/w/a.txt".into(), "a\n".into())]));
    node2.load_dump(&mut log2, &d, &mut disk).unwrap();
    assert_eq!(node2.state.windows.len(), 1);
    assert_eq!(tiling(&node2).0.concat(), vec!["/w/a.txt".to_string()]);
}

#[test]
fn a_dump_of_a_later_apex_is_refused() {
    let d = Dump { version: dump::VERSION + 1, ..Default::default() };
    let (mut log, mut node) = bare();
    assert!(node.load_dump(&mut log, &d, &mut Disk(BTreeMap::new())).is_err());
}
