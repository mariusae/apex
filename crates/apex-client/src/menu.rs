//! The tools menu on B4: libdraw's `menuhit` (plan9port
//! `src/libdraw/menuhit.c`), as acme in mariusae/plan9port pops it up on
//! a window. The numbers and the colours are menuhit's.

use apex_core::tiling::Rect;
use apex_core::WindowId;

pub const MARGIN: i32 = 4; // outside to text
pub const BORDER: i32 = 2; // outside to selection boxes
pub const BLACKBORDER: i32 = 2; // width of outlining border
pub const VSPACING: i32 = 2; // extra spacing between lines of text
pub const MAXUNSCROLL: i32 = 25; // maximum #entries before scrolling turns on
pub const NSCROLL: i32 = 20; // number entries in scrolling part
pub const SCROLLWID: i32 = 14; // width of scroll bar
pub const GAP: i32 = 4; // between text and scroll bar

/// menuhit's colours: "main tone is greenish, with negative selection"
pub const BACK: u32 = 0xD4FFD4; // allocimagemix(DPalegreen, DWhite)
pub const HIGH: u32 = 0x448844; // DDarkgreen
pub const BORD: u32 = 0x88CC88; // DMedgreen
pub const TEXT: u32 = 0x000000;
pub const HTEXT: u32 = BACK;

pub struct Menu {
    pub window: WindowId,
    pub items: Vec<String>,
    /// The whole menu, with its border (row coordinates).
    pub menur: Rect,
    /// The text elements.
    pub textr: Rect,
    pub scrollr: Rect,
    pub scrolling: bool,
    pub nitemdrawn: i32,
    /// The first item drawn (scrolling).
    pub off: i32,
    /// The highlighted item, relative to `off`; -1 for none.
    pub lasti: i32,
    /// Item height: the font's height plus `VSPACING`.
    pub ih: i32,
}

impl Menu {
    /// menurect: the rectangle, including its edge, of drawn item `i`.
    pub fn item_rect(&self, i: i32) -> Rect {
        if i < 0 {
            return Rect::new(0, 0, 0, 0);
        }
        let y0 = self.textr.y0 + self.ih * i;
        let r = Rect::new(self.textr.x0, y0, self.textr.x1, y0 + self.ih);
        // insetrect(r, Border-Margin): grows by Margin-Border
        let g = MARGIN - BORDER;
        Rect::new(r.x0 - g, r.y0 - g, r.x1 + g, r.y1 + g)
    }

    /// menusel: the drawn item containing (x, y), -1 outside the text.
    pub fn sel(&self, x: i32, y: i32) -> i32 {
        let r = Rect::new(self.textr.x0 + MARGIN, self.textr.y0 + MARGIN, self.textr.x1 - MARGIN, self.textr.y1 - MARGIN);
        if !r.contains(x, y) {
            return -1;
        }
        ((y - r.y0) / self.ih).min(self.nitemdrawn - 1)
    }

    /// The scroll bar's thumb (menuscrollpaint).
    pub fn thumb(&self) -> Rect {
        let nitem = self.items.len() as i32;
        let dy = self.scrollr.dy();
        let mut r = Rect::new(self.scrollr.x0, self.scrollr.y0 + (dy * self.off) / nitem.max(1), self.scrollr.x1, self.scrollr.y0 + (dy * (self.off + self.nitemdrawn)) / nitem.max(1));
        if r.y1 < r.y0 + 2 {
            r.y1 = r.y0 + 2;
        }
        r
    }
}
