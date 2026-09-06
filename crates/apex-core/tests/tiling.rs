//! acme's tiling, checked against what plan9port's cols.c and rows.c do
//! by hand for the same inputs. Font height 17, border 2, as in the
//! headless defaults.

use apex_core::state::{Column, Layout, Slot};
use apex_core::tiling::*;
use apex_core::*;

const FONT: i32 = 17;

fn info() -> Headless {
    Headless { font: FONT, body_font: FONT }
}

/// A row 1000 wide and 700 high with one column filling it.
fn row() -> Layout {
    let mut l = Layout { r: Rect::new(0, 0, 1000, 700), ..Default::default() };
    rowadd(&mut l, AddingCol::New { id: ColumnId(1), tag: BufferId(1) }, None, &info());
    l
}

fn add(l: &mut Layout, ci: usize, w: u64, y: Option<i32>) -> usize {
    coladd(l, ci, Adding::New(WindowId(w)), y, &info())
}

fn wins(l: &Layout, ci: usize) -> Vec<(u64, i32, i32)> {
    l.cols[ci].wins.iter().map(|s| (s.window.0, s.r.y0, s.r.y1)).collect()
}

#[test]
fn the_first_column_fills_the_row_below_the_top_tag() {
    let l = row();
    assert_eq!(l.cols.len(), 1);
    // row->r with min.y = tag bottom + Border
    assert_eq!(l.cols[0].r, Rect::new(0, FONT + BORDER, 1000, 700));
    assert!(l.cols[0].safe);
}

#[test]
fn the_first_window_takes_the_whole_column_below_its_tag() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    let s = &l.cols[0].wins[0];
    // starts after the column tag and a border; keeps the extra pixels (last window)
    assert_eq!(s.r, Rect::new(0, FONT + BORDER + FONT + BORDER, 1000, 700));
    assert_eq!(s.taglines, 1);
    // body: tag line, 1-pixel line, then the rest
    assert_eq!(s.body.y0, s.r.y0 + FONT + 1);
    assert_eq!(s.body.y1, 700);
    // a headless body is "full": nlines == fr.maxlines
    assert_eq!(s.nlines, s.fr_maxlines(FONT));
    assert_eq!(s.maxlines, s.fr_maxlines(FONT));
}

#[test]
fn a_second_window_steals_the_lower_half_of_the_last_window() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    let first = l.cols[0].wins[0];
    let i = add(&mut l, 0, 2, None);
    assert_eq!(i, 1);
    let v = l.cols[0].wins[0];
    let w = l.cols[0].wins[1];
    // y = body.min.y + Dy(body)/2, then v is cut to whole lines at or above it
    let y = first.body.y0 + first.body.dy() / 2;
    assert!(v.r.y1 <= y, "v ends at {} for y {y}", v.r.y1);
    assert_eq!((v.r.y1 - v.body.y0) % FONT, 0, "v's body is whole lines");
    assert_eq!(w.r.y0, v.r.y1 + BORDER);
    assert_eq!(w.r.y1, 700);
    assert_eq!(wins(&l, 0).len(), 2);
}

#[test]
fn a_window_lands_after_the_window_under_y() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    // y inside the first window: the new one goes between
    let y = l.cols[0].wins[0].body.y0 + 3 * FONT;
    let i = add(&mut l, 0, 3, Some(y));
    assert_eq!(i, 1);
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 3, 2]);
    // new window must start after v's tag ends
    let v = l.cols[0].wins[0];
    let w = l.cols[0].wins[1];
    assert!(w.r.y0 >= v.tagtop_y1(FONT) + BORDER);
    // and stop where the next window begins
    let next = l.cols[0].wins[2];
    assert_eq!(w.r.y1 + BORDER, next.r.y0);
}

#[test]
fn closing_a_window_extends_the_next_one_up_and_names_it_for_the_mouse() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    add(&mut l, 0, 3, None);
    let top_y0 = l.cols[0].wins[0].r.y0;
    let (removed, next) = colclose(&mut l, 0, 0, &info());
    assert_eq!(removed.window, WindowId(1));
    // "extend next window up": window 2 now starts where 1 did
    assert_eq!(next, Some(WindowId(2)));
    assert_eq!(l.cols[0].wins[0].r.y0, top_y0);
    // closing the last window extends the previous one down, no warp
    let n = l.cols[0].wins.len();
    let (_, next) = colclose(&mut l, 0, n - 1, &info());
    assert_eq!(next, None);
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 700);
}

#[test]
fn button_3_fills_the_column_and_marks_it_unsafe() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    let first_y0 = l.cols[0].wins[0].r.y0;
    colgrow(&mut l, 0, 1, 3, &info());
    assert!(!l.cols[0].safe);
    // the grown window moved to the top and fills the column
    assert_eq!(l.cols[0].wins[0].window, WindowId(2));
    assert_eq!(l.cols[0].wins[0].r, Rect::new(0, first_y0, 1000, 700));
    // the obscured window shows no lines
    assert_eq!(l.cols[0].wins[1].fr_maxlines(FONT), 0);
    // adding another window first repacks the column (it is unsafe): the
    // full window and the new one share it, the other is a tag at the
    // bottom, and nothing overlaps
    let i = add(&mut l, 0, 3, None);
    assert!(l.cols[0].safe);
    assert_eq!(i, 1);
    assert!(l.cols[0].wins[0].fr_maxlines(FONT) >= 1, "{:?}", l.cols[0].wins[0]);
    assert!(l.cols[0].wins[1].fr_maxlines(FONT) >= 1, "{:?}", l.cols[0].wins[1]);
    for pair in l.cols[0].wins.windows(2) {
        assert!(pair[0].r.y1 + BORDER <= pair[1].r.y0, "{:?} above {:?}", pair[0], pair[1]);
    }
    assert!(l.cols[0].wins.last().unwrap().r.y1 <= 700);
}

#[test]
fn button_1_grows_a_window_by_a_few_lines_from_its_neighbours() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    add(&mut l, 0, 3, None);
    let before: Vec<i32> = l.cols[0].wins.iter().map(|s| s.fr_maxlines(FONT)).collect();
    colgrow(&mut l, 0, 1, 1, &info());
    let after: Vec<i32> = l.cols[0].wins.iter().map(|s| s.fr_maxlines(FONT)).collect();
    assert!(after[1] > before[1], "{before:?} -> {after:?}");
    assert!(after[0] < before[0] || after[2] < before[2]);
    assert!(l.cols[0].safe);
    // windows abut with a border between them
    for pair in l.cols[0].wins.windows(2) {
        assert_eq!(pair[0].r.y1 + BORDER, pair[1].r.y0);
    }
}

#[test]
fn a_new_column_takes_forty_percent_of_the_last_one() {
    let mut l = row();
    let i = rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info()).unwrap();
    assert_eq!(i, 1);
    // x = d.min.x + 3*Dx(d)/5; the old column ends at x - Border
    let x = 3 * 1000 / 5;
    assert_eq!(l.cols[0].r.x1, x - BORDER);
    assert_eq!(l.cols[1].r.x0, x);
    assert_eq!(l.cols[1].r.x1, 1000);
    // a column narrower than 100 cannot be split
    let mut narrow = Layout { r: Rect::new(0, 0, 90, 700), ..Default::default() };
    rowadd(&mut narrow, AddingCol::New { id: ColumnId(1), tag: BufferId(1) }, None, &info());
    assert!(rowadd(&mut narrow, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info()).is_none());
}

#[test]
fn resizing_the_row_keeps_proportions() {
    let mut l = row();
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info());
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    let (a, b) = (l.cols[0].wins[0].r.dy(), l.cols[0].wins[1].r.dy());
    rowresize(&mut l, Rect::new(0, 0, 2000, 1400), &info());
    assert_eq!(l.r, Rect::new(0, 0, 2000, 1400));
    // the first column still ends near 60% of the width
    assert!((l.cols[0].r.x1 - (2 * (600 - BORDER))).abs() <= 2, "{:?}", l.cols[0].r);
    assert_eq!(l.cols[1].r.x1, 2000);
    assert_eq!(l.cols[0].wins[1].r.y1, 1400);
    // windows roughly doubled, in proportion
    let (a2, b2) = (l.cols[0].wins[0].r.dy(), l.cols[0].wins[1].r.dy());
    assert!(a2 > a && b2 > b);
    assert!(((a2 as f64 / b2 as f64) - (a as f64 / b as f64)).abs() < 0.2);
}

#[test]
fn closing_a_column_gives_its_width_to_a_neighbour() {
    let mut l = row();
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info());
    rowclose(&mut l, 1, &info());
    assert_eq!(l.cols.len(), 1);
    assert_eq!(l.cols[0].r.x1, 1000);
    rowadd(&mut l, AddingCol::New { id: ColumnId(3), tag: BufferId(3) }, None, &info());
    rowclose(&mut l, 0, &info());
    assert_eq!(l.cols[0].r.x0, 0);
}

#[test]
fn dragging_a_window_box_a_little_grows_it_and_a_lot_moves_it() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    let box_of = |l: &Layout, wi: usize| {
        let s = &l.cols[0].wins[wi];
        (s.r.x0 + SCROLLWID / 2, s.r.y0 + FONT / 2)
    };
    // a click: colgrow with button 1
    let op = box_of(&l, 1);
    let before = l.cols[0].wins[1].fr_maxlines(FONT);
    let warp = coldragwin(&mut l, 0, 1, 1, op, (op.0 + 2, op.1 + 1), &info());
    assert_eq!(warp, Some(Warp::WinButton(WindowId(2))));
    assert!(l.cols[0].wins[1].fr_maxlines(FONT) > before);
    // a drag past the window above (into the one above that): shuffle
    add(&mut l, 0, 3, None);
    let op = box_of(&l, 2);
    let p = (op.0, l.cols[0].wins[0].body.y0 + FONT);
    coldragwin(&mut l, 0, 2, 1, op, p, &info());
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 3, 2]);
    // a drop above every window goes to acme's default place: the bottom
    let op = box_of(&l, 1);
    let p = (op.0, l.cols[0].r.y0 + 2);
    coldragwin(&mut l, 0, 1, 1, op, p, &info());
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    let (_, _) = colclose(&mut l, 0, 2, &info());
    // a drag into another column moves it there
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info());
    let op = box_of(&l, 0);
    let p = (l.cols[1].r.x0 + 20, 300);
    coldragwin(&mut l, 0, 0, 1, op, p, &info());
    assert_eq!(l.cols[0].wins.len(), 1);
    assert_eq!(l.cols[1].wins.len(), 1);
    assert_eq!(l.cols[1].wins[0].window, WindowId(1));
}

#[test]
fn dragging_a_column_box_resizes_against_its_left_neighbour() {
    let mut l = row();
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info());
    let op = (l.cols[1].r.x0 + 6, 30);
    let warp = rowdragcol(&mut l, 1, op, (300, 30), &info());
    assert_eq!(warp, Some(Warp::ColButton(ColumnId(2))));
    assert_eq!(l.cols[0].r.x1, 300);
    assert_eq!(l.cols[1].r.x0, 300 + BORDER);
    // never narrower than 80 + Scrollwid
    rowdragcol(&mut l, 1, (302, 30), (10, 30), &info());
    assert_eq!(l.cols[0].r.x1, 80 + SCROLLWID);
}

#[test]
fn new_windows_go_where_the_empty_space_is() {
    // a column whose first window shows only a few lines of text
    struct Sparse;
    impl Info for Sparse {
        fn font_height(&self) -> i32 {
            FONT
        }
        fn taglines(&self, _: WindowId, _: i32, maxlines: i32) -> i32 {
            taglines_rule(1, false, maxlines)
        }
        fn body_font_height(&self, _: WindowId) -> i32 {
            FONT
        }
        fn body_nlines(&self, w: WindowId, _: i32, maxlines: i32) -> i32 {
            if w == WindowId(1) { 3.min(maxlines) } else { maxlines }
        }
    }
    let mut l = Layout { r: Rect::new(0, 0, 1000, 700), ..Default::default() };
    rowadd(&mut l, AddingCol::New { id: ColumnId(1), tag: BufferId(1) }, None, &Sparse);
    coladd(&mut l, 0, Adding::New(WindowId(1)), None, &Sparse);
    let s = l.cols[0].wins[0];
    // makenewwindow: empty space is big, so y = body top + nlines * font
    let y = newwindow_y(&l, 0, Some(WindowId(1)), &Sparse).unwrap();
    assert_eq!(y, s.body.y0 + 3 * FONT);
}

#[test]
fn arrange_entries_replay_identically() {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "t");
    let mut n = Node::new(a);
    n.catch_up(&log).unwrap();
    let col = n.init_session(&mut log).unwrap();
    let w1 = n.new_window(&mut log, col, "a", "x\n").unwrap();
    let w2 = n.new_window(&mut log, col, "b", "y\n").unwrap();
    n.grow_window(&mut log, w2, 1).unwrap();
    n.new_column(&mut log, None).unwrap();
    n.resize_layout(&mut log, Rect::new(0, 0, 1400, 900)).unwrap();
    n.delete_window(&mut log, w1).unwrap();
    let mut f = Node::new(AttachmentId(9));
    f.catch_up(&log).unwrap();
    assert_eq!(f.state.hash(), n.state.hash());
    assert_eq!(f.state.layout, n.state.layout);
    let _ = Column { id: ColumnId(0), tag: BufferId(0), r: Rect::default(), safe: true, wins: vec![Slot { window: w2, r: Rect::default(), body: Rect::default(), taglines: 1, nlines: 0, frmax: 0, maxlines: 0 }] };
}

#[test]
fn showing_a_window_with_no_lines_grows_it() {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "t");
    let mut n = Node::new(a);
    n.catch_up(&log).unwrap();
    let col = n.init_session(&mut log).unwrap();
    let w1 = n.new_window(&mut log, col, "a", "x\n").unwrap();
    let w2 = n.new_window(&mut log, col, "b", "y\n").unwrap();
    // button 3 on w1's box: w2 is obscured, showing no lines
    n.grow_window(&mut log, w1, 3).unwrap();
    assert_eq!(n.state.layout.slot(w2).unwrap().fr_maxlines(17), 0);
    n.reveal(&mut log, w2).unwrap();
    assert!(n.state.layout.slot(w2).unwrap().fr_maxlines(17) >= 1);
    assert!(n.state.layout.cols[0].safe);
    // a window already showing lines is left alone
    let before = n.state.layout.clone();
    n.reveal(&mut log, w2).unwrap();
    assert_eq!(n.state.layout, before);
}
