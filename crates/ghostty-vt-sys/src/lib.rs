//! libghostty-vt, as much of it as a terminal window needs.
//!
//! Ghostty's VT library parses the stream and keeps the screen; the pty,
//! the keys and the windows stay apex's (`apex-server`'s `term` and
//! `term_loop`). The C shim beside this file does the walking of the
//! grid, so what crosses here is a cell of the term shard and nothing
//! of the library's own shape; see `shim.c`.

/// A cell as the term shard carries it (`apex_core::Cell`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: u32,
    pub fg: u32,
    pub bg: u32,
    pub flags: u8,
    pub link: u16,
}

/// What a program did that the window answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Bytes for the pty: an answer to a query the program made.
    WritePty(Vec<u8>),
    /// The title (OSC 0, OSC 2).
    Title(String),
    /// OSC 52: text for the snarf buffer.
    Clipboard(String),
    Bell,
}

/// The modes a window asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    AppCursor = 1,
    BracketedPaste = 2,
    FocusInOut = 3,
    AltScreen = 4,
    Mouse = 5,
    SgrMouse = 6,
    AltScroll = 7,
}

#[repr(C)]
struct ApexVt {
    _private: [u8; 0],
}

extern "C" {
    fn apex_vt_new(cols: u16, rows: u16, scrollback: usize) -> *mut ApexVt;
    fn apex_vt_free(vt: *mut ApexVt);
    fn apex_vt_write(vt: *mut ApexVt, data: *const u8, len: usize);
    fn apex_vt_resize(vt: *mut ApexVt, cols: u16, rows: u16);
    fn apex_vt_scroll(vt: *mut ApexVt, delta: isize);
    fn apex_vt_scroll_bottom(vt: *mut ApexVt);
    #[allow(clippy::too_many_arguments)]
    fn apex_vt_snapshot(
        vt: *mut ApexVt,
        cells: *mut Cell,
        cap: usize,
        cols: *mut u16,
        rows: *mut u16,
        cursor_x: *mut u16,
        cursor_y: *mut u16,
        cursor_visible: *mut i32,
        top: *mut u64,
        links: *mut u32,
    ) -> i32;
    fn apex_vt_link(vt: *mut ApexVt, i: u32) -> *const std::os::raw::c_char;
    fn apex_vt_mode(vt: *mut ApexVt, which: i32) -> bool;
    fn apex_vt_size(vt: *mut ApexVt, scrollback: *mut u64, total: *mut u64, at_bottom: *mut i32);
    fn apex_vt_text(vt: *mut ApexVt, x0: u16, y0: u32, x1: u16, y1: u32, out: *mut u8, cap: usize) -> isize;
    fn apex_vt_next_event(vt: *mut ApexVt, data: *mut *const u8, len: *mut usize) -> i32;
}

/// A terminal: the stream so far, and the screen it made.
pub struct Terminal {
    vt: *mut ApexVt,
    cells: Vec<Cell>,
    cols: u16,
    rows: u16,
}

// The terminal is a plain owned allocation; the library makes no threads
// and keeps nothing global (every call takes the handle).
unsafe impl Send for Terminal {}

/// The viewport as the shard shows it.
pub struct Screen<'a> {
    pub cols: u16,
    pub rows: u16,
    /// `cols * rows` cells, the first row first.
    pub cells: &'a [Cell],
    pub cursor: (u16, u16),
    pub cursor_visible: bool,
    /// The row the viewport starts at, counted from the first row the
    /// history holds.
    pub top: u64,
    /// The links the cells name, by their one-based index.
    pub links: Vec<String>,
}

impl Terminal {
    /// A terminal of `cols` by `rows`, keeping `scrollback` lines.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Option<Terminal> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // SAFETY: the library allocates and owns the terminal; null on failure.
        let vt = unsafe { apex_vt_new(cols, rows, scrollback) };
        if vt.is_null() {
            return None;
        }
        Some(Terminal { vt, cells: vec![Cell { ch: b' ' as u32, fg: 0, bg: 0, flags: 0, link: 0 }; cols as usize * rows as usize], cols, rows })
    }

    /// Bytes from the program.
    pub fn write(&mut self, data: &[u8]) {
        // SAFETY: the slice is borrowed for the call alone.
        unsafe { apex_vt_write(self.vt, data.as_ptr(), data.len()) }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        // SAFETY: the handle is ours.
        unsafe { apex_vt_resize(self.vt, cols, rows) }
        self.cols = cols;
        self.rows = rows;
        self.cells.resize(cols as usize * rows as usize, Cell { ch: b' ' as u32, fg: 0, bg: 0, flags: 0, link: 0 });
    }

    /// The viewport moved by `delta` rows, up into the history when
    /// negative.
    pub fn scroll(&mut self, delta: isize) {
        // SAFETY: the handle is ours.
        unsafe { apex_vt_scroll(self.vt, delta) }
    }

    pub fn scroll_to_bottom(&mut self) {
        // SAFETY: the handle is ours.
        unsafe { apex_vt_scroll_bottom(self.vt) }
    }

    /// The history's rows, the whole screen's rows, and whether the
    /// viewport is at the bottom.
    pub fn size(&mut self) -> (u64, u64, bool) {
        let (mut sb, mut total, mut bottom) = (0u64, 0u64, 0i32);
        // SAFETY: three outs of the stated types.
        unsafe { apex_vt_size(self.vt, &mut sb, &mut total, &mut bottom) }
        (sb, total, bottom != 0)
    }

    pub fn mode(&mut self, m: Mode) -> bool {
        // SAFETY: the handle is ours.
        unsafe { apex_vt_mode(self.vt, m as i32) }
    }

    /// The screen as it stands. The cells live until the next call.
    pub fn screen(&mut self) -> Screen<'_> {
        let (mut cols, mut rows) = (0u16, 0u16);
        let (mut cx, mut cy, mut visible, mut top, mut links) = (0u16, 0u16, 0i32, 0u64, 0u32);
        // the library may have been resized under us by the program
        // (DECCOLM): ask, grow, ask again
        for _ in 0..2 {
            // SAFETY: the buffer holds `cap` cells; the outs are of the stated types.
            let r = unsafe { apex_vt_snapshot(self.vt, self.cells.as_mut_ptr(), self.cells.len(), &mut cols, &mut rows, &mut cx, &mut cy, &mut visible, &mut top, &mut links) };
            if r == 0 {
                break;
            }
            self.cells.resize(cols as usize * rows as usize, Cell { ch: b' ' as u32, fg: 0, bg: 0, flags: 0, link: 0 });
        }
        self.cols = cols;
        self.rows = rows;
        let links = (1..=links)
            .map(|i| {
                // SAFETY: the shim keeps the strings until the next snapshot.
                let p = unsafe { apex_vt_link(self.vt, i) };
                if p.is_null() {
                    String::new()
                } else {
                    unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().to_string()
                }
            })
            .collect();
        Screen { cols, rows, cells: &self.cells[..cols as usize * rows as usize], cursor: (cx, cy), cursor_visible: visible != 0, top, links }
    }

    /// The text between two places on the whole screen (the history
    /// first), the end exclusive.
    pub fn text(&mut self, from: (u16, u32), to: (u16, u32)) -> String {
        let mut buf = vec![0u8; 4096];
        loop {
            // SAFETY: the buffer holds `cap` bytes.
            let n = unsafe { apex_vt_text(self.vt, from.0, from.1, to.0, to.1, buf.as_mut_ptr(), buf.len()) };
            if n < 0 {
                // it says how much room it wants, or -1 for no text at all
                let want = (-n) as usize;
                if want <= buf.len() || want > 1 << 28 {
                    return String::new();
                }
                buf.resize(want, 0);
                continue;
            }
            buf.truncate(n as usize);
            return String::from_utf8_lossy(&buf).to_string();
        }
    }

    /// What the program did while its bytes were being read.
    pub fn events(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            let (mut p, mut len) = (std::ptr::null(), 0usize);
            // SAFETY: the bytes are borrowed until the next call, and copied here.
            let kind = unsafe { apex_vt_next_event(self.vt, &mut p, &mut len) };
            if kind == 0 {
                return out;
            }
            // SAFETY: `len` bytes at `p` while this event is held.
            let bytes = if p.is_null() || len == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(p, len) }.to_vec() };
            out.push(match kind {
                1 => Event::WritePty(bytes),
                2 => Event::Title(String::from_utf8_lossy(&bytes).to_string()),
                3 => Event::Clipboard(String::from_utf8_lossy(&bytes).to_string()),
                _ => Event::Bell,
            });
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: ours to free, once.
        unsafe { apex_vt_free(self.vt) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &Screen<'_>, y: u16) -> String {
        s.cells[y as usize * s.cols as usize..][..s.cols as usize].iter().map(|c| char::from_u32(c.ch).unwrap_or(' ')).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn a_terminal_takes_a_stream_and_shows_a_screen() {
        let mut t = Terminal::new(20, 4, 100).unwrap();
        t.write(b"hello\r\nworld");
        let s = t.screen();
        assert_eq!((s.cols, s.rows), (20, 4));
        assert_eq!(line(&s, 0), "hello");
        assert_eq!(line(&s, 1), "world");
        assert_eq!(s.cursor, (5, 1));
        assert!(s.cursor_visible);
    }

    #[test]
    fn colours_and_attributes_come_as_the_shard_carries_them() {
        let mut t = Terminal::new(20, 2, 100).unwrap();
        // bold, one of the sixteen; then an exact colour; then plain
        t.write(b"\x1b[1;31mA\x1b[0m\x1b[38;2;10;20;30mB\x1b[0mC");
        let s = t.screen();
        let c = |x: usize| s.cells[x];
        assert_eq!(c(0).flags & 1, 1, "bold");
        assert_eq!(c(0).fg, 0xfe00_0001, "red is the palette's, for the theme to colour");
        assert_eq!(c(1).fg, 0xff0a_141e, "an exact colour is exact");
        assert_eq!(c(2).fg, 0, "the default ink");
        assert_eq!(c(2).flags, 0);
    }

    #[test]
    fn the_history_grows_and_the_viewport_moves_over_it() {
        let mut t = Terminal::new(10, 3, 100).unwrap();
        for i in 0..10 {
            t.write(format!("line{i}\r\n").as_bytes());
        }
        let (sb, total, bottom) = t.size();
        assert!(sb >= 7, "the history has the lines that went by: {sb}");
        assert_eq!(total, sb + 3, "the history and the screen");
        assert!(bottom);
        let was = t.screen().top;
        assert_eq!(was, sb, "at the bottom the viewport starts after the history");
        t.scroll(-3);
        let (_, _, bottom) = t.size();
        assert!(!bottom, "scrolled up into the history");
        let s = t.screen();
        assert_eq!(s.top, was - 3, "three rows up");
        // and it shows the text of the row it starts at
        let at_top = line(&s, 0);
        assert_eq!(t.text((0, (was - 3) as u32), (4, (was - 3) as u32)), at_top);
        t.scroll_to_bottom();
        assert!(t.size().2);
        assert_eq!(t.screen().top, was);
    }

    #[test]
    fn text_comes_back_from_anywhere_in_the_history() {
        let mut t = Terminal::new(10, 3, 100).unwrap();
        for i in 0..8 {
            t.write(format!("line{i}\r\n").as_bytes());
        }
        let (sb, _, _) = t.size();
        assert!(sb >= 5);
        // both ends are taken, as a sweep takes the cell under the pointer
        assert_eq!(t.text((0, 0), (4, 0)), "line0");
        assert_eq!(t.text((1, 0), (2, 0)), "in");
        assert_eq!(t.text((0, 0), (4, 1)), "line0\nline1");
    }

    #[test]
    fn modes_are_the_ones_a_window_asks_about() {
        let mut t = Terminal::new(10, 3, 100).unwrap();
        assert!(!t.mode(Mode::AppCursor) && !t.mode(Mode::BracketedPaste) && !t.mode(Mode::AltScreen));
        t.write(b"\x1b[?1h\x1b[?2004h\x1b[?1049h\x1b[?1006h\x1b[?1000h\x1b[?1004h\x1b[?1007h");
        assert!(t.mode(Mode::AppCursor));
        assert!(t.mode(Mode::BracketedPaste));
        assert!(t.mode(Mode::AltScreen));
        assert!(t.mode(Mode::SgrMouse));
        assert!(t.mode(Mode::Mouse));
        assert!(t.mode(Mode::FocusInOut));
        assert!(t.mode(Mode::AltScroll));
        t.write(b"\x1b[?1l\x1b[?1049l");
        assert!(!t.mode(Mode::AppCursor) && !t.mode(Mode::AltScreen));
    }

    #[test]
    fn a_program_that_asks_is_answered_and_what_it_says_is_heard() {
        let mut t = Terminal::new(10, 3, 100).unwrap();
        // a title, a bell, a device status report, and OSC 52
        t.write(b"\x1b]2;the title\x07\x07\x1b[6n\x1b]52;c;aGVsbG8=\x07");
        let evs = t.events();
        assert!(evs.contains(&Event::Title("the title".into())), "{evs:?}");
        assert!(evs.contains(&Event::Bell), "{evs:?}");
        assert!(evs.iter().any(|e| matches!(e, Event::WritePty(b) if b.starts_with(b"\x1b["))), "the report is answered: {evs:?}");
        assert!(evs.iter().any(|e| matches!(e, Event::Clipboard(s) if s == "hello")), "{evs:?}");
        assert!(t.events().is_empty(), "drained");
    }

    #[test]
    fn a_link_is_named_once_and_its_cells_point_at_it() {
        let mut t = Terminal::new(20, 2, 100).unwrap();
        t.write(b"\x1b]8;;https://apex.test/\x07link\x1b]8;;\x07 plain");
        let s = t.screen();
        assert_eq!(s.links, vec!["https://apex.test/".to_string()]);
        assert_eq!(s.cells[0].link, 1);
        assert_eq!(s.cells[3].link, 1);
        assert_eq!(s.cells[5].link, 0);
    }
}
