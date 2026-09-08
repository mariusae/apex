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
/// The rule for a terminal's window name: `{osc7 path}/-{title}`. Once
/// OSC 7 has reported a directory, that is the path and nothing else
/// ever is; the title (an xterm title, plan9port's label) follows a
/// `-`. With a title but no directory reported, `-title`. Before either,
/// where the shell started and the host: `dir/-host`, win's naming.
pub fn compose_name(cwd: Option<&Path>, title: Option<&str>, initial_dir: &Path, initial_label: &str) -> String {
    let dir = |d: &Path| d.display().to_string().trim_end_matches('/').to_string();
    match (cwd, title) {
        (Some(c), Some(t)) => format!("{}/-{t}", dir(c)),
        (Some(c), None) => format!("{}/-{initial_label}", dir(c)),
        (None, Some(t)) => format!("-{t}"),
        (None, None) => format!("{}/-{initial_label}", dir(initial_dir)),
    }
}

/// A leading `~` or `~/` (a shell's short form of the home directory, as
/// in a title made with zsh's `%~`) becomes `$HOME`.
pub fn expand_tilde(s: &str) -> String {
    let home = || std::env::var("HOME").ok().filter(|h| !h.is_empty());
    if s == "~" {
        return home().unwrap_or_else(|| s.to_string());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = home() {
            return format!("{}/{rest}", h.trim_end_matches('/'));
        }
    }
    s.to_string()
}

/// The directory an OSC 7 report names: a `file://host/path` URL
/// (percent-encoded), or a plain path. Some shells report `~` for the
/// home directory; that becomes `$HOME`.
pub fn cwd_path(s: &str) -> Option<PathBuf> {
    let path = match s.strip_prefix("file://") {
        Some(rest) => &rest[rest.find('/')?..],
        None if s.starts_with('/') || s.starts_with('~') => s,
        None => return None,
    };
    // percent-decode
    let mut out = Vec::with_capacity(path.len());
    let b = path.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&path[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let path = String::from_utf8_lossy(&out).to_string();
    // `~` and `~/…` (also after the URL's slash) are the home directory
    let home = || std::env::var("HOME").ok().filter(|h| !h.is_empty());
    let tilde = path.strip_prefix('/').unwrap_or(&path);
    if tilde == "~" {
        return home().map(PathBuf::from);
    }
    if let Some(rest) = tilde.strip_prefix("~/") {
        return home().map(|h| PathBuf::from(h).join(rest));
    }
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_reports_become_paths() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(cwd_path("file://host/a/b%20c").unwrap(), PathBuf::from("/a/b c"));
        assert_eq!(cwd_path("/a/b").unwrap(), PathBuf::from("/a/b"));
        assert_eq!(cwd_path("~").unwrap(), PathBuf::from(&home));
        assert_eq!(cwd_path("~/src").unwrap(), PathBuf::from(&home).join("src"));
        assert_eq!(cwd_path("file://host/~/src").unwrap(), PathBuf::from(&home).join("src"));
        assert_eq!(cwd_path("file://host/%7E/x").unwrap(), PathBuf::from(&home).join("x"));
        assert!(cwd_path("nothing").is_none());
        assert_eq!(expand_tilde("~/src"), format!("{home}/src"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/a/~/b"), "/a/~/b");
    }
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
    /// The shell's process, for `ps` and `kill`: pid, its name (the
    /// shell's, or the command's), the command line, when it started.
    pub pid: u32,
    pub name: String,
    pub cmd: String,
    pub started: u64,
    /// What was last published: the viewport's top, its links, its rows,
    /// its cursor; the next publish carries only what differs.
    last: Option<Published>,
    /// What the shell reported: its directory (OSC 7) and its title (an
    /// xterm title, plan9port's label), the window's name being made of
    /// them (`compose_name`).
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
    /// Where the shell started: the name's directory until OSC 7 says.
    pub initial_dir: PathBuf,
}

struct Published {
    top: u64,
    links: Vec<String>,
    rows: Vec<Vec<Cell>>,
    cursor: (u16, u16, bool),
}

impl TermHost {
    /// The window's name under the rule, from what the shell reported.
    pub fn window_name(&self) -> String {
        compose_name(self.cwd.as_deref(), self.title.as_deref(), &self.initial_dir, &self.label)
    }

    /// Start the user's shell (`shell` when the session names one, the
    /// `Newterm.shell` setting; else $SHELL) as a login shell in `dir`, a
    /// truecolor xterm with `extra` (the session, the socket) in its
    /// environment; with `cmd`, the login shell runs that instead (acme's
    /// `win cmd`).
    pub fn spawn(id: TermId, dir: &Path, cols: u16, rows: u16, tx: UnboundedSender<(TermId, TermEvent)>, extra: &[(String, String)], cmd: Option<&str>, shell: Option<&str>) -> Result<TermHost, String> {
        tty::setup_env();
        let shell = match shell.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => PathBuf::from(s),
            None => match std::env::var_os("SHELL") {
                Some(s) if !s.is_empty() => PathBuf::from(s),
                _ => PathBuf::from("/bin/sh"),
            },
        };
        let args = match cmd {
            Some(c) => vec!["-l".to_string(), "-c".to_string(), c.to_string()],
            None => vec!["-l".to_string()],
        };
        // win's name for the window: the command's, else the host's
        let label = cmd.map(crate::command_name).filter(|n| !n.is_empty()).unwrap_or_else(sysname);
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "apex".to_string());
        for (k, v) in extra {
            env.insert(k.clone(), v.clone());
        }
        let options = Options {
            shell: Some(Shell::new(shell.to_string_lossy().to_string(), args)),
            working_directory: Some(dir.to_path_buf()),
            drain_on_exit: false,
            env,
        };
        let size = WindowSize { num_lines: rows, num_cols: cols, cell_width: 8, cell_height: 16 };
        let pty = tty::new(&options, size, id.0).map_err(|e| e.to_string())?;
        let pid = pty.child().id();
        let name = cmd.map(crate::command_name).filter(|n| !n.is_empty()).unwrap_or_else(|| shell.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let cmdline = match cmd {
            Some(c) => format!("{} -l -c {}", shell.display(), crate::shell_quote(c)),
            None => format!("{} -l", shell.display()),
        };
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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
        Ok(TermHost { term, notifier, cols, rows, exited: false, dir: dir.to_path_buf(), label, pid, name, cmd: cmdline, started, last: None, cwd: None, title: None, initial_dir: dir.to_path_buf() })
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

    /// The wheel as the program sees it, if it does: `delta` lines at
    /// cell `at`. With mouse reporting on, wheel buttons (64 up, 65
    /// down) in SGR or X10 form, one per line; on the alternate screen
    /// with alternate scroll (DECSET 1007, on by default), up and down
    /// arrows instead, as xterm sends them. False when the wheel is
    /// ours to scroll the display with.
    pub fn wheel(&self, delta: isize, at: Option<(u16, u16)>) -> bool {
        let mode = self.mode();
        let Some((col, row)) = at else { return false };
        let n = delta.unsigned_abs();
        if mode.intersects(TermMode::MOUSE_MODE) {
            let button = if delta < 0 { 64 } else { 65 };
            let mut out = Vec::new();
            for _ in 0..n {
                if mode.contains(TermMode::SGR_MOUSE) {
                    out.extend_from_slice(format!("\x1b[<{button};{};{}M", col + 1, row + 1).as_bytes());
                } else {
                    // X10: 32 + button, 32 + 1-based col and row, bytes
                    let (c, r) = ((col as usize + 33).min(255) as u8, (row as usize + 33).min(255) as u8);
                    out.extend_from_slice(&[0x1b, b'[', b'M', 32 + button as u8, c, r]);
                }
            }
            self.write(&out);
            return true;
        }
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let key = if delta < 0 { "up" } else { "down" };
            let k = TermKey { key: key.into(), text: None, shift: false, control: false, alt: false };
            let one = encode_key(&k, mode.contains(TermMode::APP_CURSOR));
            let mut out = Vec::new();
            for _ in 0..n {
                out.extend_from_slice(&one);
            }
            self.write(&out);
            return true;
        }
        false
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
        let out = encode_key(k, self.mode().contains(TermMode::APP_CURSOR));
        if !out.is_empty() {
            self.write(&out);
        }
    }
}

/// A keystroke as xterm sends it, with option as meta (ESC before the
/// key, as Terminal.app and iTerm2's "Esc+" do): so opt-b, opt-f,
/// opt-backspace are zsh's and bash's word keys, and opt-left/right
/// send ESC b / ESC f (Terminal.app's defaults, what the shells bind)
/// rather than xterm's modified arrows, which they do not. Other
/// modified keys are xterm's `CSI 1;m` forms.
pub fn encode_key(k: &TermKey, app_cursor: bool) -> Vec<u8> {
    {
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
        let alt_only = k.alt && !k.shift && !k.control;
        match k.key.as_str() {
            "enter" => out.push(b'\r'),
            "backspace" => {
                if k.alt {
                    out.push(0x1b);
                }
                out.push(0x7f);
            }
            "left" if alt_only => out.extend_from_slice(b"\x1bb"),
            "right" if alt_only => out.extend_from_slice(b"\x1bf"),
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
            key if k.alt => {
                // option as meta: ESC, then the key itself, not the
                // character the option layer composes (∫ for opt-b)
                let base = if key == "space" { " ".to_string() } else { key.to_string() };
                if base.chars().count() == 1 {
                    out.push(0x1b);
                    let c = base.chars().next().unwrap();
                    let c = if k.shift && c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c };
                    let mut b = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                } else if let Some(t) = &k.text {
                    out.push(0x1b);
                    out.extend_from_slice(t.as_bytes());
                }
            }
            _ => {
                if let Some(t) = &k.text {
                    if !t.is_empty() {
                        out.extend_from_slice(t.as_bytes());
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    fn k(key: &str, text: Option<&str>, shift: bool, control: bool, alt: bool) -> TermKey {
        TermKey { key: key.into(), text: text.map(String::from), shift, control, alt }
    }

    #[test]
    fn option_is_meta_and_arrows_are_what_the_shells_bind() {
        // opt-b: ESC b, not the ∫ the option layer composes
        assert_eq!(encode_key(&k("b", Some("∫"), false, false, true), false), b"\x1bb");
        assert_eq!(encode_key(&k("b", Some("ı"), true, false, true), false), b"\x1bB");
        assert_eq!(encode_key(&k("space", Some(" "), false, false, true), false), b"\x1b ");
        // opt-left/right are ESC b / ESC f; with shift too, xterm's form
        assert_eq!(encode_key(&k("left", None, false, false, true), false), b"\x1bb");
        assert_eq!(encode_key(&k("right", None, false, false, true), false), b"\x1bf");
        assert_eq!(encode_key(&k("left", None, true, false, true), false), b"\x1b[1;4D");
        // opt-up/down are xterm's meta arrows
        assert_eq!(encode_key(&k("up", None, false, false, true), false), b"\x1b[1;3A");
        assert_eq!(encode_key(&k("down", None, false, false, true), false), b"\x1b[1;3B");
        // opt-backspace deletes a word: ESC DEL
        assert_eq!(encode_key(&k("backspace", None, false, false, true), false), b"\x1b\x7f");
        assert_eq!(encode_key(&k("backspace", None, false, false, false), false), b"\x7f");
        // plain and application-cursor arrows, control letters, text
        assert_eq!(encode_key(&k("up", None, false, false, false), false), b"\x1b[A");
        assert_eq!(encode_key(&k("up", None, false, false, false), true), b"\x1bOA");
        assert_eq!(encode_key(&k("c", None, false, true, false), false), b"\x03");
        assert_eq!(encode_key(&k("c", None, false, true, true), false), b"\x1b\x03");
        assert_eq!(encode_key(&k("a", Some("a"), false, false, false), false), b"a");
    }
}

impl TermHost {

    /// The viewport's changes since the last publish as term-shard ops:
    /// the top when it moved, the links when they differ, runs of rows
    /// that differ (all of them the first time, or after a resize), the
    /// cursor when it moved. Nothing when nothing changed.
    pub fn changed_ops(&mut self) -> Vec<TermOp> {
        let all = self.snapshot_ops();
        let (mut top, mut links, mut rows, mut cursor) = (0u64, Vec::new(), Vec::new(), (0u16, 0u16, true));
        for op in all {
            match op {
                TermOp::View { top: t } => top = t,
                TermOp::Links { links: l } => links = l,
                TermOp::Rows { rows: r, .. } => rows = r,
                TermOp::Cursor { col, row, visible } => cursor = (col, row, visible),
                _ => {}
            }
        }
        let mut out = Vec::new();
        match &self.last {
            Some(p) if p.rows.len() == rows.len() && p.rows.iter().zip(rows.iter()).all(|(a, b)| a.len() == b.len()) => {
                if p.top != top {
                    out.push(TermOp::View { top });
                }
                if p.links != links {
                    out.push(TermOp::Links { links: links.clone() });
                }
                // runs of rows that differ
                let mut i = 0;
                while i < rows.len() {
                    if p.rows[i] == rows[i] {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < rows.len() && p.rows[i] != rows[i] {
                        i += 1;
                    }
                    out.push(TermOp::Rows { first: start as u16, rows: rows[start..i].to_vec() });
                }
                if p.cursor != cursor {
                    out.push(TermOp::Cursor { col: cursor.0, row: cursor.1, visible: cursor.2 });
                }
            }
            _ => {
                out.push(TermOp::View { top });
                out.push(TermOp::Links { links: links.clone() });
                out.push(TermOp::Rows { first: 0, rows: rows.clone() });
                out.push(TermOp::Cursor { col: cursor.0, row: cursor.1, visible: cursor.2 });
            }
        }
        self.last = Some(Published { top, links, rows, cursor });
        out
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
        let blank = Cell { ch: ' ', fg: 0, bg: 0, flags: 0, link: 0 };
        let mut rows: Vec<Vec<Cell>> = vec![vec![blank; cols_n]; rows_n];
        let mut links: Vec<String> = Vec::new();
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
            // OSC 8: the link's index, the table shared by the viewport
            let link = match cell.hyperlink() {
                Some(h) => {
                    let uri = h.uri();
                    let i = links.iter().position(|u| u == uri).unwrap_or_else(|| {
                        links.push(uri.to_string());
                        links.len() - 1
                    });
                    (i + 1).min(u16::MAX as usize) as u16
                }
                None => 0,
            };
            rows[row as usize][col] = Cell { ch, fg: pack(fg), bg: bg.map(pack).unwrap_or(0), flags: f, link };
        }
        let cursor = content.cursor;
        let visible = cursor.shape != CursorShape::Hidden;
        let crow = (cursor.point.line.0 + off).max(0) as u16;
        let top = t.grid().history_size().saturating_sub(content.display_offset) as u64;
        drop(t);
        vec![
            TermOp::View { top },
            TermOp::Links { links },
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
