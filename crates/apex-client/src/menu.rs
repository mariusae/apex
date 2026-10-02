//! The tools menu on B4: libdraw's `menuhit` (plan9port
//! `src/libdraw/menuhit.c`), as acme in mariusae/plan9port pops it up on
//! a window -- in menuhit's ways: up while the button is held, the item
//! under the pointer highlighted and none outside, run when the button
//! comes up over it; the last one chosen remembered, and the menu opened
//! so that it sits under the pointer, which is landed on it, so a click
//! alone repeats it; past 25 items, a part of them and a lane to scroll
//! them by. What it looks like is a tag's: a card as a tag is, its rows
//! a tag's lines in a tag's face, the item under the pointer as B2
//! sweeping it in a tag would show it (choosing one is running it), the
//! remembered one marked by a dot in the margin.

use apex_core::tiling::Rect;
use apex_core::WindowId;

/// A row's height where no tag says (the tests'): the client gives a
/// tag's line (`Menu::place`'s `ih`).
#[cfg(test)]
pub const IH: i32 = 22;
/// Above the first row and below the last.
pub const PAD_Y: i32 = 4;
/// The highlight, in from the menu's sides.
pub const INSET: i32 = 4;
/// From the menu's edge to an item's text: the remembered one's dot's
/// column.
pub const LEAD: i32 = 18;
/// From an item's text to the menu's far edge.
pub const TRAIL: i32 = 14;
/// No narrower than this.
pub const MIN_W: i32 = 110;
pub const MAXUNSCROLL: i32 = 25; // maximum #entries before scrolling turns on
pub const NSCROLL: i32 = 20; // number entries in scrolling part
/// The scrolling lane, down the right while there is one.
pub const SCROLLWID: i32 = 12;

pub struct Menu {
    pub window: WindowId,
    pub items: Vec<String>,
    /// The whole menu (row coordinates).
    pub menur: Rect,
    /// The rows: the highlight's width, the rows' height.
    pub textr: Rect,
    pub scrollr: Rect,
    pub scrolling: bool,
    pub nitemdrawn: i32,
    /// The first item drawn (scrolling).
    pub off: i32,
    /// The highlighted item, relative to `off`; -1 for none.
    pub lasti: i32,
    pub ih: i32,
    /// The item chosen last time, which the checkmark marks.
    pub checked: Option<usize>,
}

impl Menu {
    /// The menu for `items` (the widest `maxwid` across), its rows `ih`
    /// high, `checked` the one chosen last, opened at `(mx, my)` in
    /// `screen`: that item under the pointer, the menu centred across it
    /// and kept on the screen.
    pub fn place(window: WindowId, items: Vec<String>, checked: Option<usize>, maxwid: i32, ih: i32, (mx, my): (i32, i32), screen: Rect) -> Menu {
        let ih = ih.max(1);
        let nitem = items.len() as i32;
        let lasthit = checked.unwrap_or(0) as i32;
        let screenitem = ((screen.dy() - 2 * PAD_Y - 10) / ih).max(1);
        let (scrolling, nitemdrawn, off) = if nitem > MAXUNSCROLL || nitem > screenitem {
            let n = NSCROLL.min(screenitem).max(1);
            (true, n, (lasthit - n / 2).clamp(0, (nitem - n).max(0)))
        } else {
            (false, nitem, 0)
        };
        let lasti = lasthit - off;
        let w = (LEAD + maxwid + TRAIL).max(MIN_W) + if scrolling { SCROLLWID } else { 0 };
        let h = 2 * PAD_Y + nitemdrawn * ih;
        let x0 = (mx - w / 2).min(screen.x1 - w).max(screen.x0);
        let y0 = (my - (PAD_Y + lasti * ih + ih / 2)).min(screen.y1 - h).max(screen.y0);
        let menur = Rect::new(x0, y0, x0 + w, y0 + h);
        let lane = if scrolling { SCROLLWID } else { 0 };
        let textr = Rect::new(menur.x0 + INSET, menur.y0 + PAD_Y, menur.x1 - INSET - lane, menur.y0 + PAD_Y + nitemdrawn * ih);
        let scrollr = if scrolling { Rect::new(menur.x1 - SCROLLWID - 2, menur.y0 + PAD_Y, menur.x1 - 2, menur.y1 - PAD_Y) } else { Rect::new(0, 0, 0, 0) };
        Menu { window, items, menur, textr, scrollr, scrolling, nitemdrawn, off, lasti, ih, checked }
    }

    /// The rectangle of drawn row `i`: what its highlight fills.
    pub fn item_rect(&self, i: i32) -> Rect {
        if i < 0 {
            return Rect::new(0, 0, 0, 0);
        }
        let y0 = self.textr.y0 + self.ih * i;
        Rect::new(self.textr.x0, y0, self.textr.x1, y0 + self.ih)
    }

    /// menusel: the drawn row containing (x, y), -1 outside the rows.
    pub fn sel(&self, x: i32, y: i32) -> i32 {
        if !self.textr.contains(x, y) {
            return -1;
        }
        ((y - self.textr.y0) / self.ih).min(self.nitemdrawn - 1)
    }

    /// The scroll lane's thumb (menuscrollpaint).
    pub fn thumb(&self) -> Rect {
        let nitem = self.items.len() as i32;
        let dy = self.scrollr.dy();
        let mut r = Rect::new(self.scrollr.x0, self.scrollr.y0 + (dy * self.off) / nitem.max(1), self.scrollr.x1, self.scrollr.y0 + (dy * (self.off + self.nitemdrawn)) / nitem.max(1));
        if r.y1 < r.y0 + 14 {
            r.y1 = r.y0 + 14;
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("item{i}")).collect()
    }

    const SCREEN: Rect = Rect { x0: 0, y0: 0, x1: 1200, y1: 800 };

    #[test]
    fn the_remembered_item_opens_under_the_pointer() {
        let m = Menu::place(WindowId(1), items(6), Some(3), 60, IH, (500, 400), SCREEN);
        let r = m.item_rect(m.lasti);
        assert_eq!(m.lasti, 3);
        assert!(r.contains(500, 400), "{r:?}");
        assert_eq!(m.sel(500, 400), 3, "a click alone takes it again");
        // and the first when nothing is remembered
        let m = Menu::place(WindowId(1), items(6), None, 60, IH, (500, 400), SCREEN);
        assert_eq!(m.sel(500, 400), 0);
        // outside the rows, nothing
        assert_eq!(m.sel(m.menur.x0 + 1, m.menur.y0 + 1), -1);
    }

    #[test]
    fn a_menu_is_as_wide_as_its_widest_item_and_stays_on_the_screen() {
        let m = Menu::place(WindowId(1), items(3), None, 400, IH, (10, 790), SCREEN);
        assert_eq!(m.menur.dx(), LEAD + 400 + TRAIL);
        assert!(m.menur.x0 >= 0 && m.menur.y1 <= 800, "{:?}", m.menur);
        // a narrow one is the least width
        assert_eq!(Menu::place(WindowId(1), items(3), None, 10, IH, (500, 400), SCREEN).menur.dx(), MIN_W);
    }

    #[test]
    fn past_twenty_five_items_a_part_shows_and_scrolls() {
        let m = Menu::place(WindowId(1), items(40), Some(30), 60, IH, (500, 400), SCREEN);
        assert!(m.scrolling);
        assert_eq!(m.nitemdrawn, NSCROLL);
        assert_eq!(m.off + m.lasti, 30, "the remembered one among those drawn");
        assert!(m.item_rect(m.lasti).contains(500, 400));
        assert!(m.scrollr.x0 > m.textr.x1 - 1, "the lane is right of the rows");
    }
}
