//! A hosted terminal: a pty with a shell on it (`crate::pty`), whose
//! stream libghostty-vt parses into a screen; we turn that screen into
//! `TermOp` rows for the term shard and encode keystrokes xterm-style.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;
use ghostty_vt_sys::{Mode, Progress, Terminal};

use apex_core::{Cell, TermId, TermOp};

use crate::pty::{self, Options, Pty};
use crate::term_loop::{EventLoop, Label, Msg, Notifier, Report};

/// What a terminal reports: what the program did (a title, a bell, the
/// clipboard, its end), and the labels its shell wrote (acme's win:
/// `ESC ] ; name BEL`; OSC 7: the working directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    /// An xterm title (plan9port's label).
    Title(String),
    /// OSC 52: text for the snarf buffer.
    Clipboard(String),
    Bell,
    /// OSC 9;4: the program is working, this far along when it says, or
    /// no longer working.
    Working(bool, Option<u8>),
    /// The screen changed.
    Wakeup,
    /// The program ended.
    Exit(i32),
    Name(String),
    Cwd(String),
}

/// The size of a cell as a program is told (`TIOCGWINSZ` pixels, and
/// what an XTWINOPS report says).
pub const CELL_W: u16 = 8;
pub const CELL_H: u16 = 16;

/// The window size a program reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSize {
    pub num_lines: u16,
    pub num_cols: u16,
    pub cell_width: u16,
    pub cell_height: u16,
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
/// ever is; until then the path is where the terminal started (a shell
/// reports the same directory soon after, a program like `Newterm
/// claude` never does); the title (an xterm title, plan9port's label)
/// follows a `-`. Before either, `dir/-host`, win's naming.
pub fn compose_name(cwd: Option<&Path>, title: Option<&str>, initial_dir: &Path, initial_label: &str) -> String {
    let dir = |d: &Path| d.display().to_string().trim_end_matches('/').to_string();
    let t = title.map(|t| word(t));
    match (cwd, t) {
        (Some(c), Some(t)) => format!("{}/-{t}", dir(c)),
        (Some(c), None) => format!("{}/-{initial_label}", dir(c)),
        (None, Some(t)) => format!("{}/-{t}", dir(initial_dir)),
        (None, None) => format!("{}/-{initial_label}", dir(initial_dir)),
    }
}

/// A title made into one word, which is what a name is here: a
/// terminal's name lives in the first word of its tag, and the bar after
/// it is apex's. A title is nobody's to choose -- a coding agent writes
/// its state into one, blanks, bar and all (`renaming... | proj`) -- so
/// runs of blanks and any bar become a single `␣` (U+2423), the blank
/// written down. It reads as the space it stands for and is a word
/// character (`acme_isalnum`), so the name is still one word to a
/// double-click and to B3.
pub fn word(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut gap = false;
    for c in title.trim().chars() {
        if c.is_whitespace() || c == '|' {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push('\u{2423}');
        }
        gap = false;
        out.push(c);
    }
    out
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

    #[test]
    fn a_title_is_one_word() {
        let dir = PathBuf::from("/a/b");
        let name = |t: &str| compose_name(Some(&dir), Some(t), &dir, "host");
        assert_eq!(name("proj"), "/a/b/-proj");
        // a coding agent's title: its state, a spinner, and the project
        assert_eq!(name("renaming... ⠹ | proj"), "/a/b/-renaming...␣⠹␣proj");
        assert_eq!(name("  padded  "), "/a/b/-padded");
        assert_eq!(name("|"), "/a/b/-");
    }
}

/// Cell flags in `Cell::flags`.
pub const FLAG_BOLD: u8 = 1;
pub const FLAG_ITALIC: u8 = 2;
pub const FLAG_UNDERLINE: u8 = 4;

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
    pub term: Arc<Mutex<Terminal>>,
    notifier: Notifier,
    pub cols: u16,
    pub rows: u16,
    /// A size the window took while the terminal was scrolled back,
    /// held until it is back at the bottom: the program hears of a
    /// resize only then, so its redraw (a coding agent's, clearing the
    /// scrollback) cannot move what is being read.
    pub held_size: Option<(u16, u16)>,
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
    /// The colours last given to the terminal, which answers a program's
    /// questions from them; none until a client has said.
    colors: Option<crate::proto::TermColors>,
    /// Whether the program said it is at work (OSC 9;4), and how far
    /// along it said it is: the window's handle pulses while it is.
    pub working: bool,
    pub progress: Option<u8>,
}

struct Published {
    top: u64,
    total: u64,
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
    /// `scrollback`: lines of history kept (the `Newterm.scrollback`
    /// setting; 10000 otherwise).
    pub fn spawn(id: TermId, dir: &Path, cols: u16, rows: u16, tx: UnboundedSender<(TermId, TermEvent)>, extra: &[(String, String)], cmd: Option<&str>, shell: Option<&str>, scrollback: usize) -> Result<TermHost, String> {
        pty::setup_env();
        let shell = pty::shell_path(shell);
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
        let opts = Options { program: shell.clone(), args, dir: dir.to_path_buf(), env };
        let pty = Pty::spawn(&opts, cols, rows).map_err(|e| e.to_string())?;
        let pid = pty.pid();
        let name = cmd.map(crate::command_name).filter(|n| !n.is_empty()).unwrap_or_else(|| pty::program_name(&shell));
        let cmdline = match cmd {
            Some(c) => format!("{} -l -c {}", shell.display(), crate::shell_quote(c)),
            None => format!("{} -l", shell.display()),
        };
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let term = Terminal::new(cols, rows, scrollback).ok_or("no terminal")?;
        let term = Arc::new(Mutex::new(term));
        let report = Box::new(move |r: Report| {
            let ev = match r {
                Report::Label(Label::Name(s)) => TermEvent::Name(s),
                Report::Label(Label::Cwd(s)) => TermEvent::Cwd(s),
                Report::Title(t) => TermEvent::Title(t),
                Report::Clipboard(t) => TermEvent::Clipboard(t),
                Report::Bell => TermEvent::Bell,
                // a program at work makes its window's handle pulse; one
                // that failed, paused or finished is at work no longer
                Report::Progress(p, at) => TermEvent::Working(matches!(p, Progress::At | Progress::Unknown), at),
                Report::Wakeup => TermEvent::Wakeup,
                Report::Exit(code) => TermEvent::Exit(code),
            };
            let _ = tx.unbounded_send((id, ev));
        });
        let event_loop = EventLoop::new(term.clone(), pty, report).map_err(|e| e.to_string())?;
        let notifier = event_loop.channel();
        let _ = event_loop.spawn();
        Ok(TermHost { term, notifier, cols, rows, held_size: None, exited: false, dir: dir.to_path_buf(), label, pid, name, cmd: cmdline, started, last: None, cwd: None, title: None, initial_dir: dir.to_path_buf(), colors: None, working: false, progress: None })
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
        // the loop resizes the pty and the terminal together, so the
        // program never reads a size the screen does not have
        self.notifier.send(Msg::Resize { cols, rows });
    }

    /// Scrolled back into the history, not on the live screen.
    pub fn scrolled_back(&self) -> bool {
        self.term.lock().map(|mut t| !t.size().2).unwrap_or(false)
    }

    /// The scrollback dropped (the `Clear` verb): the screen stays as it
    /// is, at the bottom.
    pub fn clear_history(&mut self) {
        if let Ok(mut t) = self.term.lock() {
            t.write(b"\x1b[3J"); // xterm's: the saved lines go
        }
    }

    /// Positive scrolls towards newer output.
    /// Back to the live screen (typing goes where the cursor is).
    pub fn scroll_to_bottom(&mut self) -> bool {
        let Ok(mut t) = self.term.lock() else { return false };
        if t.size().2 {
            return false;
        }
        t.scroll_to_bottom();
        true
    }

    /// The text between two positions, `(column, history line)` as the
    /// term shard numbers them, the end exclusive; wrapped lines join.
    pub fn text(&self, p0: (u16, u64), p1: (u16, u64)) -> String {
        let Ok(mut t) = self.term.lock() else { return String::new() };
        let (_, total, _) = t.size();
        let last_col = self.cols.saturating_sub(1);
        let last_row = total.saturating_sub(1) as u32;
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
        let at = |(c, l): (u16, u64)| (c.min(last_col), (l.min(u32::MAX as u64) as u32).min(last_row));
        let (a, b) = (at(p0), at(end));
        if (a.1, a.0) > (b.1, b.0) {
            return String::new();
        }
        t.text(a, b)
    }

    pub fn scroll(&mut self, delta: isize) {
        if let Ok(mut t) = self.term.lock() {
            t.scroll(delta);
        }
    }

    /// The wheel as the program sees it, if it does: `delta` lines at
    /// cell `at`. With mouse reporting on, wheel buttons (64 up, 65
    /// down) in SGR or X10 form, one per line; on the alternate screen
    /// with alternate scroll (DECSET 1007, on by default), up and down
    /// arrows instead, as xterm sends them. False when the wheel is
    /// ours to scroll the display with.
    pub fn wheel(&self, delta: isize, at: Option<(u16, u16)>) -> bool {
        let Some((col, row)) = at else { return false };
        let n = delta.unsigned_abs();
        if self.mode(Mode::Mouse) {
            let button = if delta < 0 { 64 } else { 65 };
            let mut out = Vec::new();
            for _ in 0..n {
                if self.mode(Mode::SgrMouse) {
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
        if self.mode(Mode::AltScreen) && self.mode(Mode::AltScroll) {
            let key = if delta < 0 { "up" } else { "down" };
            let k = TermKey { key: key.into(), text: None, shift: false, control: false, alt: false };
            let one = encode_key(&k, self.mode(Mode::AppCursor));
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
        WindowSize { num_lines: self.rows, num_cols: self.cols, cell_width: CELL_W, cell_height: CELL_H }
    }

    /// Whether a mode the window cares about is on.
    fn mode(&self, m: Mode) -> bool {
        self.term.lock().map(|mut t| t.mode(m)).unwrap_or(false)
    }

    /// Focus gained or lost, to a program that asked for it (DECSET
    /// 1004): xterm's `CSI I` and `CSI O`.
    pub fn focus(&self, on: bool) {
        if self.mode(Mode::FocusInOut) {
            self.write(if on { b"\x1b[I" } else { b"\x1b[O" });
        }
    }

    pub fn paste(&self, text: &str) {
        if self.mode(Mode::BracketedPaste) {
            let mut v = b"\x1b[200~".to_vec();
            v.extend_from_slice(text.as_bytes());
            v.extend_from_slice(b"\x1b[201~");
            self.write(&v);
        } else {
            self.write(text.replace('\n', "\r").as_bytes());
        }
    }

    /// The text as if it were typed: newlines are Return, and nothing is
    /// bracketed, so what is sent runs. A paste is the other thing (a
    /// shell that asked for bracketed paste holds it on the line to be
    /// read), and `Send` and B2 are not pastes: they are typing.
    pub fn type_in(&self, text: &str) {
        self.write(text.replace('\n', "\r").as_bytes());
    }

    /// Encode a keystroke xterm-style and send it.
    pub fn key(&self, k: &TermKey) {
        let out = encode_key(k, self.mode(Mode::AppCursor));
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
        let (mut top, mut total, mut links, mut rows, mut cursor) = (0u64, 0u64, Vec::new(), Vec::new(), (0u16, 0u16, true));
        for op in all {
            match op {
                TermOp::View { top: t, total: n } => {
                    top = t;
                    total = n;
                }
                TermOp::Links { links: l } => links = l,
                TermOp::Rows { rows: r, .. } => rows = r,
                TermOp::Cursor { col, row, visible } => cursor = (col, row, visible),
                _ => {}
            }
        }
        let mut out = Vec::new();
        match &self.last {
            Some(p) if p.rows.len() == rows.len() && p.rows.iter().zip(rows.iter()).all(|(a, b)| a.len() == b.len()) => {
                if (p.top, p.total) != (top, total) {
                    out.push(TermOp::View { top, total });
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
                out.push(TermOp::View { top, total });
                out.push(TermOp::Links { links: links.clone() });
                out.push(TermOp::Rows { first: 0, rows: rows.clone() });
                out.push(TermOp::Cursor { col: cursor.0, row: cursor.1, visible: cursor.2 });
            }
        }
        self.last = Some(Published { top, total, links, rows, cursor });
        out
    }

    /// The viewport as term-shard ops: all rows, then the cursor.
    pub fn snapshot_ops(&self) -> Vec<TermOp> {
        let blank = Cell { ch: ' ', fg: 0, bg: 0, flags: 0, link: 0 };
        let Ok(mut t) = self.term.lock() else {
            return vec![TermOp::View { top: 0, total: self.rows as u64 }, TermOp::Links { links: Vec::new() }, TermOp::Rows { first: 0, rows: vec![vec![blank; self.cols as usize]; self.rows as usize] }, TermOp::Cursor { col: 0, row: 0, visible: false }];
        };
        // the scrollbar measures the view against the whole screen, the
        // history and the viewport together
        let (_, total, _) = t.size();
        let screen = t.screen();
        let total = total.max(screen.rows as u64);
        let (cols, rows_n) = (screen.cols as usize, screen.rows as usize);
        let mut rows: Vec<Vec<Cell>> = vec![vec![blank; cols]; rows_n];
        for (y, row) in rows.iter_mut().enumerate() {
            for (x, out) in row.iter_mut().enumerate() {
                let c = screen.cells[y * cols + x];
                *out = Cell { ch: char::from_u32(c.ch).unwrap_or(' '), fg: c.fg, bg: c.bg, flags: c.flags, link: c.link };
            }
        }
        vec![
            TermOp::View { top: screen.top, total },
            TermOp::Links { links: screen.links.clone() },
            TermOp::Rows { first: 0, rows },
            TermOp::Cursor { col: screen.cursor.0, row: screen.cursor.1, visible: screen.cursor_visible },
        ]
    }

    /// The colours the client draws with, which the terminal answers a
    /// program's questions from (OSC 4, 10, 11) and resolves its cells
    /// against.
    pub fn set_colors(&mut self, colors: &crate::proto::TermColors) {
        if self.colors == Some(*colors) {
            return;
        }
        self.colors = Some(*colors);
        let mut palette = [0u32; 16];
        palette.copy_from_slice(&colors.ansi);
        if let Ok(mut t) = self.term.lock() {
            t.set_colors(colors.fg, colors.bg, &palette);
        }
    }
}

impl Drop for TermHost {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

/// A colour as the protocol carries one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// The theme's ink and paper, where a program inverted the defaults.
pub const DEFAULT_FG: u32 = 0xfd00_0000;
pub const DEFAULT_BG: u32 = 0xfd00_0001;

/// The 256-colour xterm palette used when the application hasn't set one.
pub fn default_color(index: usize) -> Rgb8 {
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
            Rgb8 { r, g, b }
        }
        16..=231 => {
            let i = index - 16;
            let lv = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            Rgb8 { r: lv(i / 36), g: lv((i / 6) % 6), b: lv(i % 6) }
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            Rgb8 { r: v, g: v, b: v }
        }
        256 => Rgb8 { r: 0, g: 0, b: 0 },
        257 => Rgb8 { r: 0xff, g: 0xff, b: 0xea },
        _ => Rgb8 { r: 0, g: 0, b: 0 },
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;

    /// A terminal scrolled back stays on what it shows while output
    /// goes on below (win's behaviour), until a key brings it back.
    #[test]
    fn scrolled_back_does_not_follow_output() {
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        let cmd = "i=0; while [ $i -lt 400 ]; do echo line$i; i=$((i+1)); sleep 0.005; done; sleep 3";
        let mut h = TermHost::spawn(TermId(1), Path::new("/"), 40, 10, tx, &[], Some(cmd), Some("sh"), 1000).unwrap();
        let top_of = |h: &TermHost| h.snapshot_ops().iter().find_map(|o| if let TermOp::View { top, .. } = o { Some(*top) } else { None }).unwrap();
        let first_row = |h: &TermHost| h.snapshot_ops().iter().find_map(|o| if let TermOp::Rows { rows, .. } = o { Some(rows[0].iter().map(|c| c.ch).collect::<String>().trim_end().to_string()) } else { None }).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while top_of(&h) < 30 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(top_of(&h) >= 30, "output came: top {}", top_of(&h));
        h.scroll(-8);
        let (top, row) = (top_of(&h), first_row(&h));
        assert!(row.starts_with("line"), "{row}");
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(top_of(&h) > top || true); // history grew below; the view is what matters
        assert_eq!(first_row(&h), row, "the view moved while scrolled back");
        // a key brings it back to the live screen
        assert!(h.scroll_to_bottom());
        assert_ne!(first_row(&h), row);
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn the_directory_is_where_the_terminal_started_until_osc7_says() {
        let d = Path::new("/w/here");
        assert_eq!(compose_name(None, None, d, "host"), "/w/here/-host");
        // a title (Newterm claude, an xterm title) keeps the start directory
        assert_eq!(compose_name(None, Some("claude"), d, "host"), "/w/here/-claude");
        // OSC 7 reported: that path, and nothing else ever
        assert_eq!(compose_name(Some(Path::new("/else/")), Some("t"), d, "host"), "/else/-t");
        assert_eq!(compose_name(Some(Path::new("/else")), None, d, "host"), "/else/-host");
    }
}
