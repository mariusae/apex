//! A hosted terminal: alacritty_terminal owns the pty, its reader thread
//! and the grid; we turn the grid into `TermOp` rows for the term shard
//! and encode keystrokes xterm-style.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::tty::{self, Options, Shell};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use alacritty_terminal::Term as AlacTerm;
use futures::channel::mpsc::UnboundedSender;

use apex_core::{Cell, TermId, TermOp};

use crate::term_loop::{EventLoop, Label, Msg, Notifier};

/// What a terminal reports: alacritty's events, and the labels its shell
/// wrote (acme's win: `ESC ] ; name BEL`; OSC 7: the working directory).
#[derive(Debug)]
pub enum TermEvent {
    Alac(Event),
    Name(String),
    Cwd(String),
}

/// plan9port's `sysname`: `$sysname`, else the host's name up to the
/// first dot; `gnot` if all else fails (win.c).
pub fn sysname() -> String {
    if let Ok(s) = std::env::var("sysname") {
        if !s.is_empty() {
            return s;
        }
    }
    let mut buf = [0u8; 256];
    // SAFETY: a plain gethostname into a buffer of the stated size.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    let host = if rc == 0 { std::ffi::CStr::from_bytes_until_nul(&buf).map(|c| c.to_string_lossy().to_string()).unwrap_or_default() } else { String::new() };
    let host = host.split('.').next().unwrap_or("").to_string();
    if host.is_empty() {
        "gnot".into()
    } else {
        host
    }
}

/// win's `label`: the window's name for a label, with `/-name` added when
/// the label does not end in a `-` component of its own.
pub fn labelled(text: &str, name: &str) -> String {
    let last = text.rsplit('/').next().unwrap_or("");
    if text.contains('/') && last.starts_with('-') {
        return text.to_string();
    }
    format!("{text}{}-{name}", if text.ends_with('/') { "" } else { "/" })
}

/// The directory an OSC 7 report names: a `file://host/path` URL
/// (percent-encoded), or a plain path.
pub fn cwd_path(s: &str) -> Option<PathBuf> {
    let path = match s.strip_prefix("file://") {
        Some(rest) => &rest[rest.find('/')?..],
        None if s.starts_with('/') => s,
        None => return None,
    };
    let mut out = Vec::with_capacity(path.len());
    let b = path.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&path[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).to_string()))
}

/// Cell flags in `Cell::flags`.
pub const FLAG_BOLD: u8 = 1;
pub const FLAG_ITALIC: u8 = 2;
pub const FLAG_UNDERLINE: u8 = 4;

/// Forwards alacritty's events to the server as (term, event).
#[derive(Clone)]
pub struct Listener {
    id: TermId,
    tx: UnboundedSender<(TermId, TermEvent)>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.tx.unbounded_send((self.id, TermEvent::Alac(event)));
    }
}

#[derive(Clone, Copy)]
struct Size {
    cols: u16,
    rows: u16,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// A keystroke as the client reports it; encoded here because the
/// encoding depends on terminal modes only the server knows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TermKey {
    /// gpui-style key name: "a", "enter", "left", "f1", ...
    pub key: String,
    /// the text the key would type, if printable
    pub text: Option<String>,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

pub struct TermHost {
    pub term: Arc<FairMutex<AlacTerm<Listener>>>,
    notifier: Notifier,
    pub cols: u16,
    pub rows: u16,
    pub exited: bool,
    /// Where the shell is, as far as its labels have told us (acme's win
    /// resolves relative names there).
    pub dir: PathBuf,
    /// The `-name` the window carries after its directory (the host, until
    /// a label brings its own).
    pub label: String,
}

impl TermHost {
    /// Start the user's shell as a login shell in `dir`, a truecolor
    /// xterm with `extra` (the session, the socket) in its environment.
    pub fn spawn(id: TermId, dir: &Path, cols: u16, rows: u16, tx: UnboundedSender<(TermId, TermEvent)>, extra: &[(String, String)]) -> Result<TermHost, String> {
        tty::setup_env();
        let shell = match std::env::var_os("SHELL") {
            Some(s) if !s.is_empty() => PathBuf::from(s),
            _ => PathBuf::from("/bin/sh"),
        };
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "apex".to_string());
        for (k, v) in extra {
            env.insert(k.clone(), v.clone());
        }
        let options = Options {
            shell: Some(Shell::new(shell.to_string_lossy().to_string(), vec!["-l".to_string()])),
            working_directory: Some(dir.to_path_buf()),
            drain_on_exit: false,
            env,
        };
        let size = WindowSize { num_lines: rows, num_cols: cols, cell_width: 8, cell_height: 16 };
        let pty = tty::new(&options, size, id.0).map_err(|e| e.to_string())?;
        let listener = Listener { id, tx: tx.clone() };
        let term = AlacTerm::new(Config::default(), &Size { cols, rows }, listener.clone());
        let term = Arc::new(FairMutex::new(term));
        let label_tx = tx;
        let on_label = Box::new(move |l: Label| {
            let ev = match l {
                Label::Name(s) => TermEvent::Name(s),
                Label::Cwd(s) => TermEvent::Cwd(s),
            };
            let _ = label_tx.unbounded_send((id, ev));
        });
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, on_label).map_err(|e| e.to_string())?;
        let notifier = Notifier(event_loop.channel());
        let _ = event_loop.spawn();
        Ok(TermHost { term, notifier, cols, rows, exited: false, dir: dir.to_path_buf(), label: sysname() })
    }

    pub fn write(&self, data: &[u8]) {
        self.notifier.notify(data.to_vec());
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.lock().resize(Size { cols, rows });
        self.notifier.on_resize(WindowSize { num_lines: rows, num_cols: cols, cell_width: 8, cell_height: 16 });
    }

    /// Positive scrolls towards newer output.
    /// Back to the live screen (typing goes where the cursor is).
    pub fn scroll_to_bottom(&mut self) -> bool {
        let mut t = self.term.lock();
        if t.grid().display_offset() == 0 {
            return false;
        }
        t.scroll_display(Scroll::Bottom);
        true
    }

    /// The text between two positions, `(column, history line)` as the
    /// term shard numbers them, the end exclusive; wrapped lines join.
    pub fn text(&self, p0: (u16, u64), p1: (u16, u64)) -> String {
        let t = self.term.lock();
        let hist = t.grid().history_size() as i64;
        let last_col = t.grid().columns().saturating_sub(1) as u16;
        let last_line = t.grid().screen_lines() as i64 - 1;
        let (p0, p1) = if (p0.1, p0.0) <= (p1.1, p1.0) { (p0, p1) } else { (p1, p0) };
        // an exclusive end, as an inclusive one on the cell before it
        let end = if p1.0 == 0 {
            if p1.1 <= p0.1 {
                return String::new();
            }
            (last_col, p1.1 - 1)
        } else {
            (p1.0 - 1, p1.1)
        };
        let point = |(c, l): (u16, u64)| Point::new(Line((l as i64 - hist).clamp(-hist, last_line) as i32), Column(c.min(last_col) as usize));
        let (a, b) = (point(p0), point(end));
        if a > b {
            return String::new();
        }
        t.bounds_to_string(a, b)
    }

    pub fn scroll(&mut self, delta: isize) {
        self.term.lock().scroll_display(Scroll::Delta(-(delta as i32)));
    }

    pub fn window_size(&self) -> WindowSize {
        WindowSize { num_lines: self.rows, num_cols: self.cols, cell_width: 8, cell_height: 16 }
    }

    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn paste(&self, text: &str) {
        if self.mode().contains(TermMode::BRACKETED_PASTE) {
            let mut v = b"\x1b[200~".to_vec();
            v.extend_from_slice(text.as_bytes());
            v.extend_from_slice(b"\x1b[201~");
            self.write(&v);
        } else {
            self.write(text.replace('\n', "\r").as_bytes());
        }
    }

    /// Encode a keystroke xterm-style and send it.
    pub fn key(&self, k: &TermKey) {
        let app_cursor = self.mode().contains(TermMode::APP_CURSOR);
        let mut out: Vec<u8> = Vec::new();
        let modp = 1 + u8::from(k.shift) + 2 * u8::from(k.alt) + 4 * u8::from(k.control);
        let csi_mod = |out: &mut Vec<u8>, f: &str| {
            if modp > 1 {
                out.extend_from_slice(format!("\x1b[1;{modp}{f}").as_bytes());
            } else if app_cursor {
                out.extend_from_slice(format!("\x1bO{f}").as_bytes());
            } else {
                out.extend_from_slice(format!("\x1b[{f}").as_bytes());
            }
        };
        let tilde = |out: &mut Vec<u8>, n: u8| {
            if modp > 1 {
                out.extend_from_slice(format!("\x1b[{n};{modp}~").as_bytes());
            } else {
                out.extend_from_slice(format!("\x1b[{n}~").as_bytes());
            }
        };
        match k.key.as_str() {
            "enter" => out.push(b'\r'),
            "backspace" => out.push(if k.alt { 0x1b } else { 0x7f }),
            "tab" => {
                if k.shift {
                    out.extend_from_slice(b"\x1b[Z");
                } else {
                    out.push(b'\t');
                }
            }
            "escape" => out.push(0x1b),
            "up" => csi_mod(&mut out, "A"),
            "down" => csi_mod(&mut out, "B"),
            "right" => csi_mod(&mut out, "C"),
            "left" => csi_mod(&mut out, "D"),
            "home" => csi_mod(&mut out, "H"),
            "end" => csi_mod(&mut out, "F"),
            "delete" => tilde(&mut out, 3),
            "pageup" => tilde(&mut out, 5),
            "pagedown" => tilde(&mut out, 6),
            "f1" => out.extend_from_slice(b"\x1bOP"),
            "f2" => out.extend_from_slice(b"\x1bOQ"),
            "f3" => out.extend_from_slice(b"\x1bOR"),
            "f4" => out.extend_from_slice(b"\x1bOS"),
            "f5" => tilde(&mut out, 15),
            "f6" => tilde(&mut out, 17),
            "f7" => tilde(&mut out, 18),
            "f8" => tilde(&mut out, 19),
            "f9" => tilde(&mut out, 20),
            "f10" => tilde(&mut out, 21),
            "f11" => tilde(&mut out, 23),
            "f12" => tilde(&mut out, 24),
            key if k.control => {
                let c = key.chars().next().unwrap_or('\0');
                let byte = match c {
                    'a'..='z' => Some(c as u8 & 0x1f),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '-' | '7' => Some(0x1f),
                    '?' | '8' => Some(0x7f),
                    '@' | '2' | ' ' => Some(0x00),
                    _ => None,
                };
                if key == "space" {
                    out.push(0);
                } else if let Some(b) = byte {
                    if k.alt {
                        out.push(0x1b);
                    }
                    out.push(b);
                }
            }
            _ => {
                if let Some(t) = &k.text {
                    if !t.is_empty() {
                        if k.alt {
                            out.push(0x1b);
                        }
                        out.extend_from_slice(t.as_bytes());
                    }
                }
            }
        }
        if !out.is_empty() {
            self.write(&out);
        }
    }

    /// The viewport as term-shard ops: all rows, then the cursor.
    pub fn snapshot_ops(&self) -> Vec<TermOp> {
        let t = self.term.lock();
        let content = t.renderable_content();
        let colors = content.colors;
        let fg_default = resolve(Color::Named(NamedColor::Foreground), colors).unwrap_or(Rgb { r: 0, g: 0, b: 0 });
        let bg_default = Rgb { r: 0xff, g: 0xff, b: 0xea };
        let rows_n = t.grid().screen_lines();
        let cols_n = t.grid().columns();
        let blank = Cell { ch: ' ', fg: 0, bg: 0, flags: 0 };
        let mut rows: Vec<Vec<Cell>> = vec![vec![blank; cols_n]; rows_n];
        let off = content.display_offset as i32;
        for cell in content.display_iter {
            let row = cell.point.line.0 + off;
            if row < 0 || row >= rows_n as i32 {
                continue;
            }
            let col = cell.point.column.0;
            if col >= cols_n {
                continue;
            }
            let flags = cell.flags;
            if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let mut fg = resolve(cell.fg, colors).unwrap_or(fg_default);
            let mut bg = resolve_bg(cell.bg, colors);
            if flags.contains(Flags::INVERSE) {
                let b = bg.unwrap_or(bg_default);
                bg = Some(fg);
                fg = b;
            }
            if flags.contains(Flags::DIM) {
                fg = Rgb { r: 0x77, g: 0x77, b: 0x77 };
            }
            let mut f = 0u8;
            if flags.contains(Flags::BOLD) {
                f |= FLAG_BOLD;
            }
            if flags.contains(Flags::ITALIC) {
                f |= FLAG_ITALIC;
            }
            if flags.intersects(Flags::ALL_UNDERLINES) {
                f |= FLAG_UNDERLINE;
            }
            let ch = if flags.contains(Flags::HIDDEN) || cell.c == '\0' { ' ' } else { cell.c };
            rows[row as usize][col] = Cell { ch, fg: pack(fg), bg: bg.map(pack).unwrap_or(0), flags: f };
        }
        let cursor = content.cursor;
        let visible = cursor.shape != CursorShape::Hidden;
        let crow = (cursor.point.line.0 + off).max(0) as u16;
        let top = t.grid().history_size().saturating_sub(content.display_offset) as u64;
        drop(t);
        vec![
            TermOp::View { top },
            TermOp::Rows { first: 0, rows },
            TermOp::Cursor { col: cursor.point.column.0 as u16, row: crow, visible },
        ]
    }
}

impl Drop for TermHost {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

fn pack(c: Rgb) -> u32 {
    0xff00_0000 | ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

fn resolve(c: Color, colors: &alacritty_terminal::term::color::Colors) -> Option<Rgb> {
    match c {
        Color::Spec(rgb) => Some(rgb),
        Color::Named(NamedColor::Background) => colors[NamedColor::Background],
        Color::Named(n) => Some(colors[n].unwrap_or_else(|| default_color(n as usize))),
        Color::Indexed(i) => Some(colors[i as usize].unwrap_or_else(|| default_color(i as usize))),
    }
}

/// Background: `None` means the window's own background.
fn resolve_bg(c: Color, colors: &alacritty_terminal::term::color::Colors) -> Option<Rgb> {
    match c {
        Color::Named(NamedColor::Background) => colors[NamedColor::Background],
        other => resolve(other, colors),
    }
}

/// The 256-colour xterm palette used when the application hasn't set one.
pub fn default_color(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcc, 0x24, 0x1d),
        (0x3c, 0x8a, 0x2a),
        (0xb0, 0x8a, 0x00),
        (0x1c, 0x4f, 0xd6),
        (0x9a, 0x2d, 0x9a),
        (0x0f, 0x8a, 0x8a),
        (0xbb, 0xbb, 0xbb),
        (0x55, 0x55, 0x55),
        (0xff, 0x55, 0x55),
        (0x55, 0xc0, 0x55),
        (0xd6, 0xc0, 0x00),
        (0x55, 0x80, 0xff),
        (0xdd, 0x55, 0xdd),
        (0x33, 0xc0, 0xc0),
        (0xff, 0xff, 0xff),
    ];
    match index {
        0..=15 => {
            let (r, g, b) = ANSI[index];
            Rgb { r, g, b }
        }
        16..=231 => {
            let i = index - 16;
            let lv = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            Rgb { r: lv(i / 36), g: lv((i / 6) % 6), b: lv(i % 6) }
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            Rgb { r: v, g: v, b: v }
        }
        256 => Rgb { r: 0, g: 0, b: 0 },
        257 => Rgb { r: 0xff, g: 0xff, b: 0xea },
        _ => Rgb { r: 0, g: 0, b: 0 },
    }
}
