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
    assert!(l.cols[0].full.is_none());
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

fn click(l: &mut Layout, wi: usize, but: i32) -> Option<Warp> {
    let s = l.cols[0].wins[wi].r;
    let at = (s.x0 + 3, s.y0 + 3);
    coldragwin(l, 0, wi, but, at, at, &info())
}

#[test]
fn button_3_grows_a_window_to_the_whole_column_in_its_place() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    let before = l.clone();
    assert_eq!(click(&mut l, 1, 3), Some(Warp::WinButton(WindowId(2))));
    // it keeps its place in the column; the others are hidden behind it
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(l.cols[0].full.map(|f| f.window), Some(WindowId(2)));
    assert!(l.cols[0].hiding());
    assert!(l.cols[0].hides(WindowId(1)) && l.cols[0].hides(WindowId(3)) && !l.cols[0].hides(WindowId(2)));
    let s = l.cols[0].wins[1];
    assert_eq!((s.r.y0, s.r.y1), (FONT + BORDER + FONT + BORDER, 700));
    assert_eq!(l.cols[0].wins[0].frmax, 0);
    // B3 again: every window back where it was
    click(&mut l, 1, 3);
    assert!(l.cols[0].full.is_none());
    assert_eq!(wins(&l, 0), wins(&before, 0));
    // and B1 does as well
    click(&mut l, 2, 3);
    assert!(l.cols[0].full.is_some());
    click(&mut l, 2, 1);
    assert!(l.cols[0].full.is_none());
    assert_eq!(wins(&l, 0), wins(&before, 0));
}

#[test]
fn a_window_alone_grown_to_its_column_hides_nothing() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    click(&mut l, 0, 3);
    assert!(l.cols[0].full.is_some());
    assert!(!l.cols[0].hiding());
}

#[test]
fn a_column_resized_keeps_its_window_grown_to_the_whole_of_it() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    click(&mut l, 2, 3);
    let r = Rect::new(0, FONT + BORDER, 800, 600);
    colresize(&mut l, 0, r, &info());
    assert_eq!(l.cols[0].full.map(|f| f.window), Some(WindowId(3)));
    let s = l.cols[0].wins[2];
    assert_eq!((s.r.y0, s.r.y1, s.r.x1), (FONT + BORDER + FONT + BORDER, 600, 800));
    // and gives the others back laid out in the new rectangle
    unfull(&mut l, 0, &info());
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 600);
    for pair in l.cols[0].wins.windows(2) {
        assert_eq!(pair[0].r.y1 + BORDER, pair[1].r.y0);
    }
}

#[test]
fn a_window_added_or_closed_gives_the_hidden_ones_back() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    click(&mut l, 0, 3);
    add(&mut l, 0, 4, None);
    assert!(l.cols[0].full.is_none());
    assert_eq!(wins(&l, 0).len(), 4);
    click(&mut l, 1, 3);
    colclose(&mut l, 0, 1, &info());
    assert!(l.cols[0].full.is_none());
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 700);
    for pair in l.cols[0].wins.windows(2) {
        assert_eq!(pair[0].r.y1 + BORDER, pair[1].r.y0);
    }
}

#[test]
fn a_stashed_window_leaves_its_column_and_its_neighbour_takes_the_space() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    stash(&mut l, 0, 1, &info());
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 3]);
    assert_eq!(l.stash.len(), 1);
    assert_eq!(l.stash[0].slot.window, WindowId(2));
    assert_eq!(l.stash[0].col, ColumnId(1));
    assert_eq!(l.stash[0].above, Some(WindowId(1)));
    assert!(l.is_stashed(WindowId(2)));
    assert_eq!(l.column_of(WindowId(2)), Some(ColumnId(1)));
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 700);
    for pair in l.cols[0].wins.windows(2) {
        assert_eq!(pair[0].r.y1 + BORDER, pair[1].r.y0);
    }
    assert_eq!(stash_order(&l, 0), vec![(WindowId(1), false), (WindowId(2), true), (WindowId(3), false)]);
}

#[test]
fn the_last_window_in_a_column_may_be_stashed() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    stash(&mut l, 0, 0, &info());
    assert!(l.cols[0].wins.is_empty());
    assert_eq!(l.stash.len(), 1);
    assert_eq!(recall(&mut l, 0, None, &info()), Some(0));
    assert_eq!(wins(&l, 0), vec![(1, FONT + BORDER + FONT + BORDER, 700)]);
}

#[test]
fn stashing_the_window_grown_to_the_column_gives_the_others_back() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    click(&mut l, 1, 3);
    stash(&mut l, 0, 1, &info());
    assert!(l.cols[0].full.is_none());
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 3]);
    assert!(l.cols[0].wins.iter().all(|s| s.frmax > 0));
}

/// Every window in column `ci` but `keep` stashed, top to bottom.
fn stash_all_but(l: &mut Layout, ci: usize, keep: u64) {
    while let Some(wi) = l.cols[ci].wins.iter().position(|s| s.window != WindowId(keep)) {
        stash(l, ci, wi, &info());
    }
}

/// Stashed window `w` recalled, to column `or` if its own is gone.
fn back(l: &mut Layout, w: u64, or: Option<usize>) -> Option<usize> {
    let si = l.stashed_of(WindowId(w)).unwrap();
    recall(l, si, or, &info())
}

#[test]
fn button_2_maximizes_a_window_and_button_1_on_it_gives_the_others_back() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    let heights = |l: &Layout| l.cols[0].wins.iter().map(|s| s.r.dy()).collect::<Vec<_>>();
    let before = heights(&l);
    // B2 on 2: it has the column, the others down to their tags
    assert_eq!(click(&mut l, 1, 2), Some(Warp::WinButton(WindowId(2))));
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert!(l.cols[0].wins[0].body.dy() <= 0 && l.cols[0].wins[2].body.dy() <= 0, "{:?}", heights(&l));
    assert!(is_maximized_win(&l.cols[0], 1));
    // B2 again changes nothing
    let max = l.clone();
    click(&mut l, 1, 2);
    assert_eq!(l, max);
    // B1 on it: every window back at the size it had
    click(&mut l, 1, 1);
    let after = heights(&l);
    for (a, b) in before.iter().zip(&after) {
        assert!((a - b).abs() <= FONT, "{before:?} -> {after:?}");
    }
    assert!(!is_maximized_win(&l.cols[0], 1));
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 700);
}

#[test]
fn button_2_on_a_window_grown_to_the_column_maximizes_it_among_the_others() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    click(&mut l, 1, 3);
    click(&mut l, 1, 2);
    assert!(l.cols[0].full.is_none());
    assert!(is_maximized_win(&l.cols[0], 1));
}

#[test]
fn a_recalled_window_comes_back_where_it_was_at_its_share() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    let dy = l.cols[0].wins[1].r.dy();
    stash(&mut l, 0, 1, &info());
    assert_eq!(recall(&mut l, 0, None, &info()), Some(0));
    assert!(l.stash.is_empty());
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert!((l.cols[0].wins[1].r.dy() - dy).abs() <= FONT, "{dy} -> {:?}", wins(&l, 0));
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 700);
    for pair in l.cols[0].wins.windows(2) {
        assert_eq!(pair[0].r.y1 + BORDER, pair[1].r.y0);
    }
}

#[test]
fn recalled_out_of_order_each_goes_back_to_its_place() {
    let mut l = row();
    for w in 1..=5 {
        add(&mut l, 0, w, None);
    }
    // all but 3 stashed, then 2 and 4 back, in the other order
    stash_all_but(&mut l, 0, 3);
    back(&mut l, 4, None);
    back(&mut l, 2, None);
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![2, 3, 4]);
    assert_eq!(stash_order(&l, 0).iter().map(|&(w, _)| w.0).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
    back(&mut l, 5, None);
    back(&mut l, 1, None);
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 700);
}

#[test]
fn the_stash_is_the_sessions_not_a_columns() {
    let mut l = row();
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info());
    for w in 1..=2 {
        add(&mut l, 0, w, None);
    }
    for w in 3..=4 {
        add(&mut l, 1, w, None);
    }
    stash(&mut l, 0, 0, &info());
    stash(&mut l, 1, 1, &info());
    assert_eq!(l.stash.iter().map(|s| (s.slot.window.0, s.col.0)).collect::<Vec<_>>(), vec![(1, 1), (4, 2)]);
    assert_eq!(stash_order(&l, 0).iter().map(|&(w, st)| (w.0, st)).collect::<Vec<_>>(), vec![(1, true), (2, false)]);
    assert_eq!(stash_order(&l, 1).iter().map(|&(w, st)| (w.0, st)).collect::<Vec<_>>(), vec![(3, false), (4, true)]);
    // each back to its own column, whichever is asked for instead
    assert_eq!(back(&mut l, 4, Some(0)), Some(1));
    assert_eq!(back(&mut l, 1, Some(1)), Some(0));
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(wins(&l, 1).iter().map(|w| w.0).collect::<Vec<_>>(), vec![3, 4]);
}

#[test]
fn a_window_whose_column_is_gone_comes_back_at_the_foot_of_another() {
    let mut l = row();
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info());
    add(&mut l, 0, 1, None);
    add(&mut l, 1, 2, None);
    add(&mut l, 1, 3, None);
    stash(&mut l, 1, 0, &info());
    stash(&mut l, 1, 0, &info());
    rowclose(&mut l, 1, &info());
    assert_eq!(l.column_of(WindowId(2)), None);
    // nowhere asked for: it stays
    assert_eq!(back(&mut l, 2, None), None);
    assert!(l.is_stashed(WindowId(2)));
    assert_eq!(back(&mut l, 2, Some(0)), Some(0));
    assert_eq!(back(&mut l, 3, Some(0)), Some(0));
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(l.cols[0].wins.last().unwrap().r.y1, 700);
}

#[test]
fn a_window_leaving_hands_its_place_on() {
    let mut l = row();
    for w in 1..=3 {
        add(&mut l, 0, w, None);
    }
    // 2 away under 1; then 1 leaves (closed): 2 is now at the top
    stash(&mut l, 0, 1, &info());
    let above = stash_above(&l, 0, WindowId(1));
    colclose(&mut l, 0, 0, &info());
    left(&mut l, 0, WindowId(1), above);
    assert_eq!(l.stash[0].above, None);
    recall(&mut l, 0, None, &info());
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![2, 3]);
    // a stashed one taken out for good
    stash(&mut l, 0, 0, &info());
    let s = unstash(&mut l, 0);
    assert_eq!(s.window, WindowId(2));
    assert!(l.stash.is_empty());
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
    let warp = rowdragcol(&mut l, 1, 1, op, (300, 30), &info());
    assert_eq!(warp, Some(Warp::ColButton(ColumnId(2))));
    assert_eq!(l.cols[0].r.x1, 300);
    assert_eq!(l.cols[1].r.x0, 300 + BORDER);
    // never narrower than 80 + Scrollwid, short of half that
    rowdragcol(&mut l, 1, 1, (302, 30), (60, 30), &info());
    assert_eq!(l.cols[0].r.x1, 80 + SCROLLWID);
    // past it: the neighbour minimized where it stands
    rowdragcol(&mut l, 1, 1, (94, 30), (10, 30), &info());
    assert!(is_strip(l.cols[0].r) && !l.cols[0].stashed);
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
    let _ = Column { id: ColumnId(0), tag: BufferId(0), r: Rect::default(), full: None, restore: 0, stashed: false, after: None, wins: vec![Slot { window: w2, r: Rect::default(), body: Rect::default(), taglines: 1, nlines: 0, frmax: 0, maxlines: 0, extra: 0, share: 0, premax: 0 }] };
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
    // button 1 on w1's box until w2 is its tag, showing no lines
    for _ in 0..40 {
        if n.state.layout.slot(w2).unwrap().fr_maxlines(17) == 0 {
            break;
        }
        n.grow_window(&mut log, w1, 1).unwrap();
    }
    assert_eq!(n.state.layout.slot(w2).unwrap().fr_maxlines(17), 0);
    n.reveal(&mut log, w2).unwrap();
    assert!(n.state.layout.slot(w2).unwrap().fr_maxlines(17) >= 1);
    // a window already showing lines is left alone
    let before = n.state.layout.clone();
    n.reveal(&mut log, w2).unwrap();
    assert_eq!(n.state.layout, before);
    // stashed, showing it brings it back
    n.stash_window(&mut log, w2).unwrap();
    assert!(n.state.layout.is_stashed(w2));
    n.reveal(&mut log, w2).unwrap();
    assert!(!n.state.layout.is_stashed(w2));
    assert!(n.state.layout.slot(w2).unwrap().fr_maxlines(17) >= 1);
    // hidden behind one grown to the column (B3), showing it gives the
    // others back
    n.grow_window(&mut log, w1, 3).unwrap();
    assert!(n.state.layout.column(col).unwrap().hides(w2));
    n.reveal(&mut log, w2).unwrap();
    assert!(n.state.layout.column(col).unwrap().full.is_none());
    assert!(n.state.layout.slot(w2).unwrap().fr_maxlines(17) >= 1);
    // closing a stashed window takes it out of the stash
    n.stash_window(&mut log, w2).unwrap();
    n.delete_window(&mut log, w2).unwrap();
    assert!(n.state.layout.stash.is_empty());
    assert_eq!(n.state.layout.column(col).unwrap().wins.len(), 1);
}

#[test]
fn resizing_back_and_forth_keeps_the_windows_proportions() {
    // acme trims each window but the last to whole lines on a resize;
    // scaling from the trimmed heights handed the remainders down the
    // column, a few pixels a step, until the bottom window had it all
    let mut l = row();
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    add(&mut l, 0, 3, None);
    let before: Vec<i32> = wins(&l, 0).iter().map(|(_, y0, y1)| y1 - y0).collect();
    for i in 0..200 {
        let h = if i % 2 == 0 { 700 - 37 } else { 700 };
        tiling::rowresize(&mut l, Rect::new(0, 0, 1000, h), &info());
    }
    let after: Vec<i32> = wins(&l, 0).iter().map(|(_, y0, y1)| y1 - y0).collect();
    for (i, (a, b)) in before.iter().zip(after.iter()).enumerate() {
        assert!((a - b).abs() <= FONT, "window {i}: {a} -> {b} after 200 resizes; all {before:?} -> {after:?}");
    }
}

// ---- columns grown as windows are, on their side -------------------------------

/// A row of three columns, each with a window.
fn three() -> Layout {
    let mut l = row();
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info()).unwrap();
    rowadd(&mut l, AddingCol::New { id: ColumnId(3), tag: BufferId(3) }, None, &info()).unwrap();
    for ci in 0..3 {
        add(&mut l, ci, 10 + ci as u64, None);
    }
    l
}

fn widths(l: &Layout) -> Vec<i32> {
    l.cols.iter().map(|c| c.r.dx()).collect()
}

/// The columns' ids, left to right.
fn ids(l: &Layout) -> Vec<u64> {
    l.cols.iter().map(|c| c.id.0).collect()
}

/// Column `id`'s width, and its place.
fn wid(l: &Layout, id: u64) -> i32 {
    l.column(ColumnId(id)).unwrap().r.dx()
}

fn ix(l: &Layout, id: u64) -> usize {
    l.column_index(ColumnId(id)).unwrap()
}

/// Columns laid out across the row with a border between each and
/// nothing lost at either edge, their windows as wide as they are.
fn tiles(l: &Layout) {
    let n = l.cols.len();
    assert_eq!(l.cols[0].r.x0, l.r.x0, "{:?}", widths(l));
    assert_eq!(l.cols[n - 1].r.x1, l.r.x1, "{:?}", widths(l));
    for i in 1..n {
        assert_eq!(l.cols[i].r.x0, l.cols[i - 1].r.x1 + BORDER, "{:?}", widths(l));
    }
    for c in &l.cols {
        for s in &c.wins {
            assert_eq!((s.r.x0, s.r.x1), (c.r.x0, c.r.x1));
        }
    }
}

#[test]
fn button_1_on_a_columns_box_widens_it_at_its_neighbours_expense() {
    let mut l = three();
    tiles(&l);
    let before = widths(&l);
    rowgrow(&mut l, 1, 1, &info());
    tiles(&l);
    let after = widths(&l);
    assert!(after[1] > before[1], "{before:?} -> {after:?}");
    assert!(after[0] <= before[0] && after[2] <= before[2], "{before:?} -> {after:?}");
    assert_eq!(l.full, None);
    // a step, smaller than a window's: a fifth of its width or a twelfth
    // of the row, not half again
    let grown = after[1] - before[1];
    let row = l.r.dx();
    assert!(grown <= (before[1] / 5).max(row / 12) + 1, "{before:?} -> {after:?}");
    assert!(grown >= row / 20, "still a visible step: {before:?} -> {after:?}");
    // and again: another step of the same kind
    rowgrow(&mut l, 1, 1, &info());
    tiles(&l);
    let again = widths(&l);
    assert!(again[1] > after[1] && again[1] - after[1] <= (after[1] / 5).max(row / 12) + 1, "{after:?} -> {again:?}");
}

#[test]
fn button_2_maximizes_a_column_and_minimizes_the_others_where_they_stand() {
    let mut l = three();
    rowgrow(&mut l, 1, 2, &info());
    tiles(&l);
    // the others minimized in place, in their order: not stashed
    assert_eq!(ids(&l), vec![1, 2, 3]);
    assert_eq!(widths(&l), vec![STRIP, 1000 - 2 * STRIP - 2 * BORDER, STRIP]);
    assert!(l.cols.iter().all(|c| !c.stashed));
    assert!(is_maximized_col(&l, 1));
    // a strip is its box: its windows' tags are a line each
    assert!(l.cols[0].wins.iter().all(|s| s.taglines == 1));
    // a click on a minimized one brings it back where it stands
    rowgrow(&mut l, 0, 1, &info());
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3]);
    assert!(wid(&l, 1) > STRIP + 100, "{:?}", widths(&l));
}

#[test]
fn button_3_gives_a_column_the_row_and_again_gives_the_others_back_where_they_were() {
    let mut l = three();
    let before = widths(&l);
    let id = l.cols[1].id;
    rowgrow(&mut l, 1, 3, &info());
    assert_eq!(l.full, Some(id));
    assert_eq!((l.cols[1].r.x0, l.cols[1].r.x1), (0, 1000));
    assert!(l.shows(1) && !l.shows(0) && !l.shows(2));
    // the hidden columns are not there to be found, stale as they are
    assert_eq!(rowwhichcol(&l, (5, 300)), Some(1));
    assert_eq!(rowwhichcol(&l, (995, 300)), Some(1));
    // B3 again: every column back where it stood, at its width
    rowgrow(&mut l, 1, 3, &info());
    assert_eq!(l.full, None);
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3]);
    let after = widths(&l);
    for (a, b) in before.iter().zip(&after) {
        assert!(near(*a, *b), "{before:?} -> {after:?}");
    }
    assert!(l.cols.iter().all(|c| !c.stashed));
    // and B1 on it as well
    rowgrow(&mut l, 2, 3, &info());
    assert_eq!(l.full, Some(ColumnId(3)));
    rowgrow(&mut l, 2, 1, &info());
    assert_eq!(l.full, None);
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3]);
}
#[test]
fn a_click_on_a_columns_box_grows_it_and_a_drag_still_moves_it() {
    let mut l = three();
    let id = l.cols[2].id;
    let x = l.cols[2].r.x0 + 3;
    let before = widths(&l);
    // a click: under five pixels of movement
    assert_eq!(rowdragcol(&mut l, 2, 1, (x, 30), (x + 2, 31), &info()), Some(Warp::ColButton(id)));
    assert!(widths(&l)[2] > before[2]);
    // given the row (as B3 once did) then a drag: the row comes back
    // before the box is moved
    rowfull(&mut l, 2, &info());
    assert_eq!(l.full, Some(id));
    rowdragcol(&mut l, 2, 1, (3, 30), (600, 30), &info());
    assert_eq!(l.full, None);
    tiles(&l);
}

#[test]
fn a_row_with_a_full_column_resizes_it_and_lays_out_before_it_adds_or_closes() {
    let mut l = three();
    let id = l.cols[0].id;
    rowgrow(&mut l, 0, 3, &info());
    // the window grows: the full column is the row still
    rowresize(&mut l, Rect::new(0, 0, 1400, 900), &info());
    assert_eq!(l.full, Some(id));
    assert_eq!((l.cols[0].r.x0, l.cols[0].r.x1, l.cols[0].r.y1), (0, 1400, 900));
    // a new column: the row comes back first, every column with room
    let at = rowadd(&mut l, AddingCol::New { id: ColumnId(4), tag: BufferId(4) }, None, &info());
    assert!(at.is_some());
    assert_eq!(l.full, None);
    tiles(&l);
    assert!(widths(&l).iter().all(|&w| w > STRIP), "{:?}", widths(&l));
    // closing one, from a hidden row too
    rowgrow(&mut l, 0, 3, &info());
    rowclose(&mut l, 3, &info());
    assert_eq!(l.full, None);
    tiles(&l);
}
// ---- strips, and bringing them back ------------------------------------------------------

/// A row of four columns, each with a window, and their widths.
fn four() -> (Layout, Vec<i32>) {
    let mut l = three();
    rowadd(&mut l, AddingCol::New { id: ColumnId(4), tag: BufferId(4) }, None, &info()).unwrap();
    add(&mut l, 3, 13, None);
    let w = widths(&l);
    (l, w)
}

fn near(a: i32, b: i32) -> bool {
    (a - b).abs() <= 2
}

#[test]
fn strips_left_by_button_2_come_back_at_the_widths_they_had() {
    // minimized by B2 on another: back where it stands
    let (mut l, before) = four();
    rowgrow(&mut l, 2, 2, &info());
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    for id in [1, 2, 4] {
        assert_eq!(wid(&l, id), STRIP);
    }
    rowgrow(&mut l, 3, 1, &info());
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    assert!(near(wid(&l, 4), before[3]), "{before:?} -> {:?}", widths(&l));
}

#[test]
fn button_4_on_a_columns_box_does_nothing() {
    let (mut l, _) = four();
    let before = l.clone();
    rowgrow(&mut l, 1, 4, &info());
    assert_eq!(l, before);
}
#[test]
fn a_click_on_a_windows_box_in_a_strip_brings_the_column_back_and_the_next_grows_the_window() {
    let (mut l, before) = four();
    // a second window in the first column, then the column minimized
    // (B2 on its neighbour)
    add(&mut l, 0, 20, None);
    assert_eq!(l.cols[0].wins.len(), 2);
    rowgrow(&mut l, 1, 2, &info());
    assert!(is_strip(l.cols[0].r));
    let heights = |l: &Layout| l.cols[0].wins.iter().map(|s| s.r.dy()).collect::<Vec<_>>();
    // the first click on the second window's box: only the column, back
    // where it stands at its width
    let s = l.cols[0].wins[1].r;
    let at = (s.x0 + 3, s.y0 + 3);
    assert_eq!(coldragwin(&mut l, 0, 1, 2, at, at, &info()), Some(Warp::WinButton(WindowId(20))));
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    assert!(near(l.cols[0].r.dx(), before[0]), "{before:?} -> {:?}", widths(&l));
    let back = heights(&l);
    assert!(l.cols[0].wins[0].r.dy() > FONT, "the other window is not squeezed yet: {back:?}");
    // the second: B2 maximizes it, the other down to its tag
    let s = l.cols[0].wins[1].r;
    let at = (s.x0 + 3, s.y0 + 3);
    coldragwin(&mut l, 0, 1, 2, at, at, &info());
    assert_eq!(l.cols[0].wins.len(), 2, "{:?}", heights(&l));
    assert!(l.cols[0].wins[0].body.dy() <= 0);
    assert!(l.cols[0].wins[1].r.dy() > 600);
}
#[test]
fn uncovering_a_column_brings_it_out_of_a_strip_or_from_behind_a_full_one() {
    let (mut l, before) = four();
    rowgrow(&mut l, 1, 2, &info());
    assert!(is_strip(l.cols[0].r));
    uncover(&mut l, 0, &info());
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    assert!(near(wid(&l, 1), before[0]), "{before:?} -> {:?}", widths(&l));
    // a column with room is left as it is
    let now = widths(&l);
    uncover(&mut l, 1, &info());
    assert_eq!(widths(&l), now);
    // hidden behind a column given the row: the row as it was
    let (mut l, before) = four();
    rowgrow(&mut l, 1, 3, &info());
    uncover(&mut l, 3, &info());
    assert_eq!(l.full, None);
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    assert!(near(wid(&l, 4), before[3]), "{before:?} -> {:?}", widths(&l));
}
#[test]
fn the_line_between_columns_moves_only_the_widths() {
    let mut l = row();
    rowadd(&mut l, AddingCol::New { id: ColumnId(2), tag: BufferId(2) }, None, &info()).unwrap();
    add(&mut l, 0, 1, None);
    add(&mut l, 1, 2, None);
    let (a, b) = (l.cols[0].r, l.cols[1].r);
    rowmovecol(&mut l, 1, b.x0 - 100, &info());
    assert_eq!(l.cols[0].id, ColumnId(1));
    assert_eq!(l.cols[1].id, ColumnId(2));
    assert_eq!(l.cols[0].r.x1, b.x0 - 100);
    assert_eq!(l.cols[1].r.x0, b.x0 - 100 + BORDER);
    assert_eq!(l.cols[1].r.x1, b.x1);
    assert_eq!(l.cols[0].r.x0, a.x0);
    // never narrower than acme allows short of half that; past it, the
    // column is minimized where it stands, as a window dragged over goes
    // down to its tag
    rowmovecol(&mut l, 1, 80 + SCROLLWID - 10, &info());
    assert_eq!(l.cols[0].r.x1, 80 + SCROLLWID);
    rowmovecol(&mut l, 1, 10, &info());
    assert!(is_strip(l.cols[0].r) && !l.cols[0].stashed, "{:?}", widths(&l));
    // B1 brings it back at the width it had
    rowgrow(&mut l, 0, 1, &info());
    assert!(!is_strip(l.cols[0].r));
    assert_eq!(l.cols[0].r.x1, 80 + SCROLLWID);
    rowmovecol(&mut l, 1, 5000, &info());
    assert!(is_strip(l.cols[1].r) && !l.cols[1].stashed, "{:?}", widths(&l));
    assert_eq!(l.cols[1].r.x1, b.x1);
}

#[test]
fn a_drags_preview_is_where_the_drop_puts_it() {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "t");
    let mut n = Node::new(a);
    n.catch_up(&log).unwrap();
    let col = n.init_session(&mut log).unwrap();
    let w1 = n.new_window(&mut log, col, "a", "x\n").unwrap();
    let w2 = n.new_window(&mut log, col, "b", "y\n").unwrap();
    let r1 = n.state.layout.slot(w1).unwrap().r;
    let op = (r1.x0 + 3, r1.y0 + 3);
    // a click is no drag: nothing to preview
    assert_eq!(n.drag_window_preview(w1, 1, op, op), None);
    // taken below the other window: the preview is where the drop puts it
    let r2 = n.state.layout.slot(w2).unwrap().r;
    let p = (op.0, r2.y1 - 40);
    let shown = n.drag_window_preview(w1, 1, op, p).unwrap();
    let before = n.state.layout.clone();
    n.drag_window(&mut log, w1, 1, op, p).unwrap();
    assert_ne!(n.state.layout, before);
    assert_eq!(Some(shown), n.state.layout.slot(w1).map(|s| s.r));
}

#[test]
fn a_columns_box_answers_as_a_windows_does() {
    // B3: the whole row, the others hidden; B3 again: all back
    let (mut l, before) = four();
    rowgrow(&mut l, 1, 3, &info());
    assert_eq!(l.full, Some(ColumnId(2)));
    rowgrow(&mut l, 1, 3, &info());
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    for j in 0..4 {
        assert!(near(widths(&l)[j], before[j]), "{before:?} -> {:?}", widths(&l));
    }
    // B2: maximized, the others minimized where they stand; B2 again
    // changes nothing; B1 on it: all back
    let (mut l, before) = four();
    rowgrow(&mut l, 2, 2, &info());
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    assert!([1, 2, 4].iter().all(|&id| wid(&l, id) == STRIP));
    let max = l.clone();
    rowgrow(&mut l, 2, 2, &info());
    assert_eq!(l, max);
    rowgrow(&mut l, 2, 1, &info());
    tiles(&l);
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    for j in 0..3 {
        assert!(near(widths(&l)[j], before[j]), "{before:?} -> {:?}", widths(&l));
    }
    // B2 on a column given the row: the row back, then it maximized
    let (mut l, _) = four();
    rowgrow(&mut l, 1, 3, &info());
    rowgrow(&mut l, 1, 2, &info());
    assert_eq!(l.full, None);
    assert!(is_maximized_col(&l, 1));
    // no column is ever put away
    assert!(l.cols.iter().all(|c| !c.stashed));
}
#[test]
fn resizing_the_row_keeps_strips_strips() {
    // two minimized (B2 on another), the others sharing the rest
    let (mut l, _) = four();
    rowgrow(&mut l, 2, 2, &info());
    rowgrow(&mut l, 3, 1, &info());
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    assert_eq!((wid(&l, 1), wid(&l, 2)), (STRIP, STRIP));
    let (a, b) = (wid(&l, 3), wid(&l, 4));
    for r in [Rect::new(0, 0, 1600, 900), Rect::new(0, 0, 700, 900), Rect::new(0, 0, 1000, 900)] {
        rowresize(&mut l, r, &info());
        tiles(&l);
        assert_eq!((wid(&l, 1), wid(&l, 2)), (STRIP, STRIP), "{r:?}: {:?}", widths(&l));
        assert_eq!(l.cols[3].r.x1, r.x1);
        // the open ones in the proportion they had
        let (a2, b2) = (wid(&l, 3), wid(&l, 4));
        let share = |x: i32, y: i32| x as f64 / (x + y) as f64;
        assert!((share(a2, b2) - share(a, b)).abs() < 0.01, "{r:?}: {a}/{b} -> {a2}/{b2}");
    }
    // and each comes back where it stands
    rowgrow(&mut l, 0, 1, &info());
    assert_eq!(ids(&l), vec![1, 2, 3, 4]);
    assert!(wid(&l, 1) > STRIP);
}
#[test]
fn a_window_stashed_with_a_few_lines_comes_back_with_an_even_share() {
    let mut l = row();
    add(&mut l, 0, 1, None);
    add(&mut l, 0, 2, None);
    // made at the foot, as an +Errors window can be: a line or two
    add(&mut l, 0, 3, Some(700 - 3 * FONT));
    assert!(l.cols[0].wins[2].frmax < 5, "{:?}", wins(&l, 0));
    stash(&mut l, 0, 2, &info());
    recall(&mut l, 0, None, &info());
    // a third of the column, near enough, not the sliver it had
    let dy = l.cols[0].wins[2].r.dy();
    assert!(dy > 600 / 3 - 2 * FONT, "{:?}", wins(&l, 0));
}

#[test]
fn a_window_restashed_is_the_latest_and_still_comes_back_where_it_was() {
    let mut l = row();
    for w in 1..=4 {
        add(&mut l, 0, w, None);
    }
    stash(&mut l, 0, 1, &info());
    stash(&mut l, 0, 1, &info());
    assert_eq!(l.stash.iter().map(|s| s.slot.window.0).collect::<Vec<_>>(), vec![2, 3]);
    restash(&mut l, 0);
    assert_eq!(l.stash.iter().map(|s| s.slot.window.0).collect::<Vec<_>>(), vec![3, 2]);
    back(&mut l, 2, None);
    back(&mut l, 3, None);
    assert_eq!(wins(&l, 0).iter().map(|w| w.0).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
}
