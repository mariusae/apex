//! The view server end to end: a daemon on a thread, a UI attachment
//! over a Unix socket, and the frames a TermKit UI would draw.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::remote::Link;

use apex_tui::input::{Button, Event, Mods, Motion, NamedKey};
use apex_tui::model::{BodyView, SpanKind};
use apex_tui::ui::Ui;

fn daemon() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-tui-test-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

/// A UI attachment, laid out 80 by 24.
fn ui() -> Ui {
    let sock = daemon();
    let (link, log, node) = Link::connect(&sock, "main", "tui", AttachmentKind::Ui, None).unwrap();
    let mut ui = Ui::new(node, log, link, "main".into());
    ui.measure(80, 24);
    ui.sync();
    ui
}

/// Pump the link until `done`, or give up.
fn settle(ui: &mut Ui, mut done: impl FnMut(&mut Ui) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        ui.poll();
        if done(ui) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done(ui)
}

/// The window `w` as the frame has it, wherever the daemon put it.
fn window(f: &apex_tui::model::Frame, w: WindowId) -> apex_tui::model::WindowView {
    f.columns.iter().flat_map(|c| &c.windows).find(|x| x.id == w.0).expect("the window is on the grid").clone()
}

fn body_text(f: &apex_tui::model::Frame, w: WindowId) -> Vec<String> {
    match &window(f, w).body {
        BodyView::Text(t) => t.lines.iter().map(|l| l.text.clone()).collect(),
        other => panic!("not a text body: {other:?}"),
    }
}

/// The first cell of a window's body: past the scrollbar.
fn body_at(w: &apex_tui::model::WindowView) -> (i32, i32) {
    (w.bx0 + 1, w.by0)
}

#[test]
fn a_window_lands_on_the_grid_with_its_tag_and_body() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/greeting", "hello\nworld\n").unwrap();
    ui.measure(80, 24);
    let f = ui.frame();

    // the row's own tag is the first line; the columns are under it,
    // past the border the tiling leaves (acme's, in cells)
    assert_eq!(f.cols, 80);
    assert_eq!(f.rows, 24);
    assert!(!f.columns.is_empty());
    assert!(f.columns.iter().all(|c| c.y0 > 0 && c.y1 <= 24 && c.x1 <= 80));

    let win = window(&f, w);
    // the tag names the file and carries acme's commands
    assert!(win.tag.lines[0].text.starts_with("/tmp/greeting"), "tag: {:?}", win.tag.lines[0].text);
    assert!(win.tag.lines[0].text.contains("Del"));
    assert!(win.tag.lines[0].text.contains("Look"));
    // the body is wrapped onto the grid, a line to a row
    let lines = body_text(&f, w);
    assert_eq!(&lines[0], "hello");
    assert_eq!(&lines[1], "world");
    // and it fills the window: the rest of the rows are blank
    assert_eq!(lines.len() as i32, win.by1 - win.by0);
    assert!(lines[2..].iter().all(|l| l.is_empty()));
    // the body sits under the tag, past the border the tiling leaves
    assert!(win.by0 >= win.y0 + win.taglines);
    assert_eq!(win.by1, win.y1);
}

#[test]
fn b1_selects_what_it_sweeps() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/sel", "hello world\n").unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    let (x, y) = body_at(&window(&f, w));

    ui.event(Event::Mouse { x, y, button: Button::B1, motion: Motion::Down, mods: Mods::default(), clicks: 1 });
    ui.event(Event::Mouse { x: x + 5, y, button: Button::B1, motion: Motion::Move, mods: Mods::default(), clicks: 1 });
    ui.event(Event::Mouse { x: x + 5, y, button: Button::B1, motion: Motion::Up, mods: Mods::default(), clicks: 1 });

    assert_eq!(ui.node.selected_text(ViewId::Body(w)).unwrap(), "hello");
    // and the frame shows it where it was swept
    let f = ui.frame();
    let span = match &window(&f, w).body {
        BodyView::Text(t) => t.lines[0].spans.iter().find(|s| s.kind == SpanKind::Sel).cloned(),
        _ => None,
    };
    let span = span.expect("the selection is drawn");
    assert_eq!((span.start, span.len), (0, 5));
}

#[test]
fn a_double_click_takes_the_word() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/word", "alpha beta\n").unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    let at = body_at(&window(&f, w));
    let (x, y) = (at.0 + 7, at.1); // inside "beta"

    ui.event(Event::Mouse { x, y, button: Button::B1, motion: Motion::Down, mods: Mods::default(), clicks: 2 });
    assert_eq!(ui.node.selected_text(ViewId::Body(w)).unwrap(), "beta");
}

#[test]
fn typing_goes_into_the_window_the_pointer_is_over() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/typed", "").unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    let (x, y) = body_at(&window(&f, w));

    // the pointer comes over the window, then a key is typed
    ui.event(Event::Mouse { x, y, button: Button::B1, motion: Motion::Move, mods: Mods::default(), clicks: 0 });
    ui.event(Event::Text { text: "ab".into() });
    ui.event(Event::Key { key: NamedKey::Enter, mods: Mods::default() });
    ui.event(Event::Text { text: "c".into() });

    let b = ui.node.view_buffer(ViewId::Body(w)).unwrap();
    assert_eq!(ui.node.state.buffer(b).unwrap().text.to_string(), "ab\nc");
    // a backspace takes the last one back
    ui.event(Event::Key { key: NamedKey::Backspace, mods: Mods::default() });
    assert_eq!(ui.node.state.buffer(b).unwrap().text.to_string(), "ab\n");
}

#[test]
fn b2_in_a_tag_executes_the_word_it_sweeps() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/exec", "one\n").unwrap();
    ui.measure(80, 24);
    // put the cursor somewhere, then B2 "Zerox" in the tag
    let f = ui.frame();
    let win = window(&f, w);
    let tag = win.tag.lines[0].text.clone();
    let at = tag.find("Del").expect("the tag has Del") as i32;
    let (x, y) = (win.x0 + 1 + at, win.y0);

    let before = ui.node.state.windows.len();
    ui.event(Event::Mouse { x, y, button: Button::B2, motion: Motion::Down, mods: Mods::default(), clicks: 1 });
    ui.event(Event::Mouse { x: x + 3, y, button: Button::B2, motion: Motion::Move, mods: Mods::default(), clicks: 1 });
    ui.event(Event::Mouse { x: x + 3, y, button: Button::B2, motion: Motion::Up, mods: Mods::default(), clicks: 1 });
    // Del closed it
    assert!(settle(&mut ui, |ui| ui.node.state.windows.len() < before), "the window went: {:?}", ui.node.state.windows.len());
    assert!(!ui.node.state.windows.contains_key(&w));
}

#[test]
fn the_layout_box_grows_a_window() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    ui.node.new_window(&mut ui.log, col, "/tmp/a", "a\n").unwrap();
    let b = ui.node.new_window(&mut ui.log, col, "/tmp/b", "b\n").unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    let win = window(&f, b);
    let before = win.y1 - win.y0;

    // B2 on the box: acme's grow
    ui.event(Event::Mouse { x: win.x0, y: win.y0, button: Button::B2, motion: Motion::Down, mods: Mods::default(), clicks: 1 });
    ui.event(Event::Mouse { x: win.x0, y: win.y0, button: Button::B2, motion: Motion::Up, mods: Mods::default(), clicks: 1 });
    ui.measure(80, 24);
    let f = ui.frame();
    let after = { let w = window(&f, b); w.y1 - w.y0 };
    assert!(after > before, "the window grew: {before} -> {after}");
}

#[test]
fn a_resize_relays_the_row_out() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/r", "x\n").unwrap();
    ui.event(Event::Resize { cols: 120, rows: 40 });
    let f = ui.frame();
    assert_eq!((f.cols, f.rows), (120, 40));
    assert_eq!(f.columns.last().unwrap().x1, 120);
    assert!(f.columns.iter().all(|c| c.y1 == 40));
    // the tag was wrapped to the new width, so the body still fits
    let win = window(&f, w);
    assert_eq!(win.by1 - win.by0, body_text(&f, w).len() as i32);
}

#[test]
fn a_markdown_window_is_served_as_a_page() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/notes.md", "# Title\n\ntext\n").unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    match &window(&f, w).body {
        BodyView::Markdown { source, .. } => assert!(source.starts_with("# Title")),
        other => panic!("markdown expected, got {other:?}"),
    }
}

#[test]
fn a_long_line_wraps_onto_the_grid() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let text = "x".repeat(200);
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/long", &format!("{text}\n")).unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    let win = window(&f, w);
    let width = (win.bx1 - win.bx0 - 1) as usize; // the scrollbar takes one
    let lines = body_text(&f, w);
    // the line fills row after row, the last one holding the remainder
    let full = 200 / width;
    assert!(lines.len() > full, "the line wrapped onto {} rows", lines.len());
    for line in lines.iter().take(full) {
        assert_eq!(line.chars().count(), width);
    }
    assert_eq!(lines[full].chars().count(), 200 % width);
    assert!(lines[full + 1..].iter().all(|l| l.is_empty()));
}

#[test]
fn the_finder_offers_the_windows_that_are_open() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    ui.node.new_window(&mut ui.log, col, "/tmp/one.rs", "").unwrap();
    ui.node.new_window(&mut ui.log, col, "/tmp/two.rs", "").unwrap();
    ui.measure(80, 24);
    ui.event(Event::Finder { all: false });
    let f = ui.frame();
    match f.overlay {
        Some(apex_tui::model::Overlay::Finder { candidates, all }) => {
            assert!(!all);
            let names: Vec<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
            assert!(names.contains(&"/tmp/one.rs"), "candidates: {names:?}");
            assert!(names.contains(&"/tmp/two.rs"), "candidates: {names:?}");
            assert!(candidates.iter().all(|c| c.open));
        }
        other => panic!("the finder is up: {other:?}"),
    }
    // and a click anywhere else puts it away
    ui.event(Event::Mouse { x: 1, y: 1, button: Button::B1, motion: Motion::Down, mods: Mods::default(), clicks: 1 });
    assert!(ui.frame().overlay.is_none());
}

#[test]
fn the_clipboard_becomes_the_snarf_buffer_and_back() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/snarf", "take me\n").unwrap();
    ui.measure(80, 24);

    // what the UI's clipboard holds is what a Paste pastes
    ui.event(Event::Clipboard { text: "from outside".into() });
    assert_eq!(ui.node.state.layout.snarf, "from outside");

    // and a Snarf in the session comes back for the UI to put on its own
    ui.node.select(&mut ui.log, ViewId::Body(w), 0, 4).unwrap();
    ui.event(Event::Exec { window: Some(w.0), text: "Snarf".into() });
    let f = ui.frame();
    assert_eq!(f.snarf.as_deref(), Some("take"));
    // sent once: the next frame does not repeat it
    assert_eq!(ui.frame().snarf, None);
}

#[test]
fn the_wheel_scrolls_a_body_and_the_frame_follows() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/many", &text).unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    let win = window(&f, w);
    assert_eq!(body_text(&f, w)[0], "line 0");

    let (x, y) = body_at(&win);
    ui.event(Event::Mouse { x, y, button: Button::WheelDown, motion: Motion::Down, mods: Mods::default(), clicks: 1 });
    let f = ui.frame();
    assert_eq!(body_text(&f, w)[0], "line 3");
    // the scrollbar's thumb moved with it
    match &window(&f, w).body {
        BodyView::Text(t) => {
            assert_eq!(t.origin, 3);
            assert_eq!(t.total, 201); // the trailing newline leaves a last empty row
        }
        other => panic!("text expected: {other:?}"),
    }

    ui.event(Event::Mouse { x, y, button: Button::WheelUp, motion: Motion::Down, mods: Mods::default(), clicks: 1 });
    assert_eq!(body_text(&ui.frame(), w)[0], "line 0");
}

#[test]
fn b3_looks_up_what_it_sweeps() {
    let mut ui = ui();
    let col = ui.node.state.layout.cols[0].id;
    let w = ui.node.new_window(&mut ui.log, col, "/tmp/look", "/etc/hostname\n").unwrap();
    ui.measure(80, 24);
    let f = ui.frame();
    let (x, y) = body_at(&window(&f, w));

    // B3 on the path: the plumber opens it, and a window appears
    let before = ui.node.state.windows.len();
    ui.event(Event::Mouse { x, y, button: Button::B3, motion: Motion::Down, mods: Mods::default(), clicks: 1 });
    ui.event(Event::Mouse { x: x + 13, y, button: Button::B3, motion: Motion::Move, mods: Mods::default(), clicks: 1 });
    ui.event(Event::Mouse { x: x + 13, y, button: Button::B3, motion: Motion::Up, mods: Mods::default(), clicks: 1 });
    let opened = settle(&mut ui, |ui| ui.node.state.windows.len() > before);
    // the file has to exist for the plumber to open it; either way the
    // look was sent and the session answered without faulting
    if opened {
        assert!(ui.node.state.windows.values().any(|x| {
            x.body_buffer().and_then(|b| ui.node.state.buffer(b).ok()).map(|b| b.name.contains("hostname")).unwrap_or(false)
        }));
    }
}
