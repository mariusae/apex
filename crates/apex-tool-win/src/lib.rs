//! `apex tool win`: acme's win (plan9port `src/cmd/9term/win.c`) as an
//! apex tool. A shell (`$acmeshell`, rc by default) runs on a pty; its
//! output is inserted at the output point of a text window named
//! `dir/-host`; what is typed after that point is sent to the shell
//! line by line, at each newline (or ^D), and the point moves past it;
//! ^C or DEL typed interrupts and drops the typing. The pty stays in
//! cooked, echoing mode, and the echo is cancelled against what was sent
//! (win's `echocancel`); a program that turns echo off (a password) is
//! served raw: keys go at once and leave the window. Labels in the
//! output name the window. `Send` (the builtin) appends the snarf buffer
//! and a newline, which is typing like any other.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use apex_core::*;
use apex_server::proto::ServerMsg;
use apex_server::remote::{Remote, ToolPlumb};
use apex_server::term_loop::{scan, Label};
use apex_server::Proposal;

const TIMEOUT: Duration = Duration::from_secs(10);

/// `APEX_WIN_DEBUG` set: what win sees and sends, on stderr.
fn debug() -> bool {
    std::env::var_os("APEX_WIN_DEBUG").is_some()
}

enum Event {
    Output(Vec<u8>),
    Exited,
}

/// The shell on its pty.
struct Shell {
    master: OwnedFd,
    /// The slave side, kept open here for its termios (the master's do
    /// not say whether the program turned echo off), as win keeps `tfd`.
    slave: OwnedFd,
    pid: libc::pid_t,
}

impl Shell {
    /// Start `cmd` (the shell, `-i`) on a pty in `dir`, the tty set as
    /// win's `stty` sets it: cooked, echoing, erase ^H, intr DEL, no
    /// output CR.
    fn spawn(cmd: &[String], dir: &Path, env: &[(String, String)], tx: Sender<Event>) -> Result<Shell, String> {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        // SAFETY: openpty with valid out-pointers.
        if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) } != 0 {
            return Err(format!("openpty: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: fresh descriptors from openpty.
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave_fd = slave;
        let mut c = std::process::Command::new(&cmd[0]);
        // as win sets them, except TERM_PROGRAM: rc's rcmain (ours) does
        // awd on cd for apex, plan9port's 9 awd for win
        c.args(&cmd[1..]).current_dir(dir).env("TERM", "dumb").env("termprog", "win").env("TERM_PROGRAM", "apex");
        for (k, v) in env {
            c.env(k, v);
        }
        use std::os::unix::process::CommandExt;
        // SAFETY: only async-signal-safe calls before exec: setsid, ioctl,
        // dup2, tcsetattr, close.
        unsafe {
            c.pre_exec(move || {
                libc::setsid();
                libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0);
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(slave_fd, &mut t) == 0 {
                    t.c_lflag |= libc::ICANON | libc::ECHO;
                    t.c_oflag &= !libc::ONLCR;
                    t.c_cc[libc::VERASE] = 0x08;
                    t.c_cc[libc::VINTR] = 0x7f;
                    libc::tcsetattr(slave_fd, libc::TCSANOW, &t);
                }
                libc::dup2(slave_fd, 0);
                libc::dup2(slave_fd, 1);
                libc::dup2(slave_fd, 2);
                if slave_fd > 2 {
                    libc::close(slave_fd);
                }
                Ok(())
            });
        }
        let child = c.spawn().map_err(|e| format!("{}: {e}", cmd[0]))?;
        // SAFETY: the slave descriptor from openpty, ours from here on.
        let slave = unsafe { OwnedFd::from_raw_fd(slave_fd) };
        let pid = child.id() as libc::pid_t;
        let reader = master.try_clone().map_err(|e| e.to_string())?;
        std::thread::spawn(move || {
            let mut f = std::fs::File::from(reader);
            let mut buf = [0u8; 8192];
            loop {
                match f.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Event::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = tx.send(Event::Exited);
        });
        Ok(Shell { master, slave, pid })
    }

    fn write(&self, bytes: &[u8]) {
        let mut f = std::fs::File::from(self.master.try_clone().expect("dup"));
        let _ = f.write_all(bytes);
    }

    /// win's `isecho`: is the tty echoing (cooked), or has a program
    /// turned that off (a password)?
    fn echoing(&self) -> bool {
        use std::os::fd::AsRawFd;
        // SAFETY: tcgetattr on our own descriptor.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(self.slave.as_raw_fd(), &mut t) != 0 {
                return true;
            }
            t.c_lflag & libc::ECHO != 0
        }
    }

    fn intr(&self) -> u8 {
        use std::os::fd::AsRawFd;
        // SAFETY: as above.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(self.slave.as_raw_fd(), &mut t) != 0 {
                return 0x7f;
            }
            t.c_cc[libc::VINTR]
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // SAFETY: a signal to the process we started.
        unsafe {
            libc::kill(self.pid, libc::SIGHUP);
        }
    }
}

/// The window's state, as win keeps it: the output point `p` (chars),
/// the typing after it, and the echo to cancel.
struct Win {
    remote: Remote,
    window: WindowId,
    buffer: BufferId,
    shell: Shell,
    /// Where the shell's next output goes; typing lives after it.
    p: usize,
    /// What was typed after `p` and not yet sent, in chars.
    typing: String,
    /// Newlines (and ^D) in `typing`, not yet sent.
    breaks: usize,
    /// Bytes sent that the pty will echo back, to be cancelled.
    echo: VecDeque<u8>,
    /// Inserts of ours on their way through the leader: (q0, text), so
    /// the entries that come back are told from typing.
    ours: VecDeque<(usize, String)>,
    /// A label cut across reads.
    carry: Vec<u8>,
    rx: Receiver<Event>,
    /// Typed a newline once: cooked from here (win's `cook`).
    cook: bool,
    /// Ranges to take out of the window once the entries at hand have
    /// landed (raw mode's keys, DEL): `before` runs ahead of the replica.
    to_remove: Vec<(usize, usize)>,
}

/// Run win: a window on the session, the shell (`cmd`, else
/// `$acmeshell`/rc `-i`) in `dir`, until the shell exits.
pub fn run(socket: &Path, session: &str, dir: &Path, cmd: &[String]) -> Result<(), String> {
    let name = format!("win-{}", std::process::id());
    let mut remote = Remote::connect_as(socket, session, &name, AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
    // the window: dir/-host, or dir/-cmd, as win names it
    let label = match cmd.first() {
        Some(c) => apex_server::command_name(c),
        None => apex_server::term::sysname(),
    };
    let wname = format!("{}/-{label}", dir.display().to_string().trim_end_matches('/'));
    let col = remote.node.state.layout.cols.last().map(|c| c.id).ok_or("no column")?;
    let window = match remote.propose(Proposal::NewWindow { col, name: wname.clone() }, TIMEOUT)? {
        Some(w) => w,
        None => return Err("no window made".into()),
    };
    let deadline = std::time::Instant::now() + TIMEOUT;
    while remote.node.state.window(window).is_err() && std::time::Instant::now() < deadline {
        let _ = remote.step(Duration::from_millis(20));
    }
    let buffer = remote.node.state.window(window).map_err(|e| e.to_string())?.body_buffer().ok_or("not a text window")?;
    // the shell, $acmeshell (rc) -i unless a command was given
    let argv: Vec<String> = if cmd.is_empty() { vec![apex_server::command_shell(), "-i".into()] } else { cmd.to_vec() };
    let env = vec![("winid".to_string(), window.0.to_string())];
    let (tx, rx) = channel();
    let shell = Shell::spawn(&argv, dir, &env, tx)?;
    // the verbs in the tools menu of this window
    for verb in ["Interrupt", "EOF"] {
        let rule = PlumbRule {
            verb: verb.into(),
            text: None,
            file: Some(format!("^{}$", regex_escape(&wname))),
            kind: Some(WinKind::File),
            isfile: None,
            isdir: None,
            action: RuleAction::Tool(name.clone()),
            to: None,
        };
        remote.rule_add(rule, 0, true, TIMEOUT)?;
    }
    let me = remote.attachment();
    let mut w = Win { remote, window, buffer, shell, p: 0, typing: String::new(), breaks: 0, echo: VecDeque::new(), ours: VecDeque::new(), carry: Vec::new(), rx, cook: false, to_remove: Vec::new() };
    // live while the shell is: the handle says so, Del does not ask
    let _ = w.propose(Proposal::Live { window, by: Some(me) }, TIMEOUT);
    let r = w.main_loop();
    if w.remote.node.state.window(window).is_ok() {
        let _ = w.propose(Proposal::Live { window, by: None }, TIMEOUT);
    }
    r
}

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() && c != '/' && c != '-' && c != '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Win {

    /// One message, through `before` first; false when the link ended.
    fn step(&mut self, timeout: Duration) -> bool {
        match self.remote.link.rx.recv_timeout(timeout) {
            Ok(m) => {
                self.before(&m);
                let alive = self.remote.handle(m);
                self.settle();
                alive
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => true,
            Err(_) => false,
        }
    }

    /// What `before` could not do while the replica was behind.
    fn settle(&mut self) {
        let removes = std::mem::take(&mut self.to_remove);
        for (q0, q1) in removes.into_iter().rev() {
            self.remove(q0, q1);
        }
    }

    /// Propose and wait for the answer, every message on the way seen
    /// by `before` (Remote::propose would apply them behind our back).
    fn propose(&mut self, p: Proposal, timeout: Duration) -> Result<Option<WindowId>, String> {
        let id = self.remote.link.propose(p);
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(r) = self.remote.link.applied.remove(&id) {
                return r;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("timed out waiting for the leader".into());
            }
            if !self.step(left.min(Duration::from_millis(50))) {
                return Err("connection closed".into());
            }
        }
    }
    fn main_loop(&mut self) -> Result<(), String> {
        loop {
            let mut busy = false;
            while let Ok(m) = self.remote.link.rx.try_recv() {
                busy = true;
                self.before(&m);
                if !self.remote.handle(m) {
                    if debug() {
                        eprintln!("win: link ended");
                    }
                    return Ok(());
                }
                self.settle();
            }
            if self.remote.node.state.window(self.window).is_err() {
                if debug() {
                    eprintln!("win: window gone");
                }
                return Ok(()); // the window was deleted: we are done
            }
            let plumbs: Vec<ToolPlumb> = std::mem::take(&mut self.remote.link.plumbs);
            for p in plumbs {
                busy = true;
                self.on_plumb(p);
            }
            while let Ok(ev) = self.rx.try_recv() {
                busy = true;
                match ev {
                    Event::Output(bytes) => self.output(bytes),
                    Event::Exited => {
                        if debug() {
                            eprintln!("win: shell exited");
                        }
                        return Ok(());
                    }
                }
            }
            if !busy {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// win's event loop, over the entries of our buffer before they land:
    /// our own writes move the output point; others' edits before it
    /// shift it; edits in the typing are typing.
    fn before(&mut self, m: &ServerMsg) {
        let ServerMsg::Entries { shard, entries } = m else { return };
        if *shard != Shard::Buffer(self.buffer) {
            return;
        }
        for e in entries {
            let Op::Buffer(BufferOp::Edit { q0, nd, text, .. }) = &e.op else { continue };
            let (q0, nd) = (*q0, *nd);
            let n = text.chars().count();
            if debug() {
                eprintln!("win: edit q0={q0} nd={nd} text={text:?} p={} typing={:?} ours={:?}", self.p, self.typing, self.ours);
            }
            if nd == 0 && n > 0 {
                if let Some((oq, ot)) = self.ours.front() {
                    if *oq == q0 && ot == text {
                        // 'E': ours; the point moves past it
                        self.ours.pop_front();
                        self.p += n;
                        continue;
                    }
                }
            }
            // a delete first
            if nd > 0 {
                let shift = self.delete(q0, q0 + nd);
                self.p -= shift;
                if self.raw() && q0 + nd >= self.p + shift {
                    self.shell.write(&vec![0x08; shift]);
                }
            }
            if n == 0 {
                continue;
            }
            // 'K'/'M' insert
            if n == 1 && text == "\u{7f}" {
                // DEL: gone from the window, an interrupt to the shell
                self.to_remove.push((q0, q0 + 1));
                self.shell.write(&[self.shell.intr()]);
                continue;
            }
            if q0 < self.p {
                self.p += n;
            } else if q0 <= self.p + self.typing.chars().count() {
                self.typed(q0, text);
            }
        }
    }

    /// win's `type`: text typed at `q0` in the typing region.
    fn typed(&mut self, q0: usize, text: &str) {
        let at = q0 - self.p;
        if !self.addtype(at, text) {
            return; // an interrupt tossed the typing
        }
        if self.raw() {
            // raw: keys leave the window at once, the shell shows what it wants
            let n = text.chars().count();
            self.to_remove.push((q0, q0 + n));
            self.p -= n;
        }
        self.sendtype();
        if text.ends_with('\n') {
            self.cook = true;
        }
    }

    /// win's `addtype`: into the typing at char `at`; false when ^C or
    /// DEL was in it (sent as the interrupt, typing dropped).
    fn addtype(&mut self, at: usize, text: &str) -> bool {
        if text.chars().any(|c| c == '\u{7f}' || c == '\u{3}') {
            self.shell.write(&[self.shell.intr()]);
            // toss all typing
            let n = self.typing.chars().count() + text.chars().count();
            self.p += n;
            self.typing.clear();
            self.breaks = 0;
            return false;
        }
        self.breaks += text.chars().filter(|c| *c == '\n' || *c == '\u{4}').count();
        let byte_at = self.typing.char_indices().nth(at).map(|(i, _)| i).unwrap_or(self.typing.len());
        self.typing.insert_str(byte_at, text);
        true
    }

    /// win's `sendtype`: every complete line (through its newline or ^D)
    /// goes to the shell, echo remembered; raw, everything goes.
    fn sendtype(&mut self) {
        let raw = self.raw();
        while self.breaks > 0 || (raw && !self.typing.is_empty()) {
            let chars: Vec<char> = self.typing.chars().collect();
            let mut cut = None;
            for (i, c) in chars.iter().enumerate() {
                if *c == '\n' || *c == '\u{4}' || (raw && i == chars.len() - 1) {
                    cut = Some(i + 1);
                    if *c == '\n' || *c == '\u{4}' {
                        self.breaks = self.breaks.saturating_sub(1);
                    }
                    break;
                }
            }
            let Some(cut) = cut else {
                self.breaks = 0;
                break;
            };
            let line: String = chars[..cut].iter().collect();
            if debug() {
                eprintln!("win: send {line:?} raw={raw}");
            }
            if !raw {
                self.echo.extend(line.as_bytes());
            }
            self.shell.write(line.as_bytes());
            self.p += cut;
            self.typing = chars[cut..].iter().collect();
        }
    }

    /// win's `delete`: a deletion [q0,q1) of the buffer, applied to the
    /// typing; returns how far the point moves back.
    fn delete(&mut self, q0: usize, q1: usize) -> usize {
        let ntyper = self.typing.chars().count();
        if q1 <= self.p {
            return q1 - q0;
        }
        if q0 >= self.p + ntyper {
            return 0;
        }
        let (mut a, deltap) = if q0 < self.p { (0, self.p - q0) } else { (q0 - self.p, 0) };
        let mut b = if q1 > self.p + ntyper { ntyper } else { q1 - self.p };
        a = a.min(ntyper);
        b = b.min(ntyper);
        let chars: Vec<char> = self.typing.chars().collect();
        self.breaks = self.breaks.saturating_sub(chars[a..b].iter().filter(|c| **c == '\n' || **c == '\u{4}').count());
        self.typing = chars[..a].iter().chain(chars[b..].iter()).collect();
        deltap
    }

    fn raw(&self) -> bool {
        !self.cook && !self.shell.echoing() || !self.shell.echoing()
    }

    /// Output from the shell: echo cancelled, CR LF and backspace runs
    /// dropped, labels taken, the rest inserted at the output point.
    fn output(&mut self, bytes: Vec<u8>) {
        if debug() {
            eprintln!("win: output {:?} echo={:?}", String::from_utf8_lossy(&bytes), String::from_utf8_lossy(&self.echo.iter().copied().collect::<Vec<u8>>()));
        }
        let (bytes, labels) = scan(&mut self.carry, &bytes);
        for l in labels {
            if let Label::Name(t) = l {
                let name = apex_server::term::labelled(&apex_server::term::expand_tilde(&t), &apex_server::term::sysname());
                let _ = self.propose(Proposal::Rename { buffer: self.buffer, window: self.window, name }, TIMEOUT);
            }
        }
        let bytes = self.echocancel(&bytes);
        let bytes = dropcrnl(&bytes);
        if bytes.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&bytes).to_string();
        self.insert_output(text);
    }

    /// win's `echocancel`: what the pty echoes of what we sent is not
    /// output.
    fn echocancel(&mut self, p: &[u8]) -> Vec<u8> {
        let mut i = 0;
        while i < p.len() {
            match self.echo.front() {
                Some(&e) if e == p[i] => {
                    self.echo.pop_front();
                    i += 1;
                }
                Some(b'\n') if p[i] == b'\r' => i += 1,
                Some(_) if p[i] == 0x08 => {
                    if i + 2 < p.len() && p[i + 1] == b' ' && p[i + 2] == 0x08 {
                        i += 2;
                    }
                    i += 1;
                }
                _ => {
                    self.echo.clear();
                    break;
                }
            }
        }
        p[i..].to_vec()
    }

    /// Insert at the output point, against the buffer as it is now;
    /// retried from the replica when the version moved.
    fn insert_output(&mut self, text: String) {
        for _ in 0..20 {
            let Ok(buf) = self.remote.node.state.buffer(self.buffer) else { return };
            let p = self.p.min(buf.text.len());
            let version = buf.version;
            self.ours.push_back((p, text.clone()));
            match self.propose(Proposal::ReplaceRange { dir: None, buffer: self.buffer, version, q0: p, q1: p, text: text.clone() }, TIMEOUT) {
                Ok(_) => return,
                Err(_) => {
                    // the buffer moved on (typing, say): the entries in
                    // between went through `before` while we waited and
                    // moved the point; forget this attempt and try again
                    self.ours.pop_back();
                    let _ = self.step(Duration::from_millis(20));
                }
            }
        }
    }

    /// Remove [q0,q1) from the window (raw mode, DEL): ours, so the point
    /// is not shifted for it.
    fn remove(&mut self, q0: usize, q1: usize) {
        let Ok(buf) = self.remote.node.state.buffer(self.buffer) else { return };
        let version = buf.version;
        let _ = self.propose(Proposal::ReplaceRange { dir: None, buffer: self.buffer, version, q0, q1, text: String::new() }, TIMEOUT);
    }

    /// The menu's verbs: Interrupt, EOF.
    fn on_plumb(&mut self, p: ToolPlumb) {
        if p.ctx != ExecCtx::Window(self.window) {
            self.remote.plumb_ack(p.id, false); // another win's window
            return;
        }
        match p.verb.as_str() {
            "Interrupt" => {
                self.shell.write(&[self.shell.intr()]);
                self.remote.plumb_ack(p.id, true);
            }
            "EOF" => {
                self.shell.write(&[0x04]);
                self.remote.plumb_ack(p.id, true);
            }
            _ => self.remote.plumb_ack(p.id, false),
        }
    }
}

/// win's `dropcrnl`: CR before LF, and backspace-space-backspace runs.
fn dropcrnl(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        if p[i] == b'\r' && i + 1 < p.len() && p[i + 1] == b'\n' {
            i += 1;
            continue;
        }
        if p[i] == 0x08 {
            if i + 2 < p.len() && p[i + 1] == b' ' && p[i + 2] == 0x08 {
                i += 2;
            }
            i += 1;
            continue;
        }
        out.push(p[i]);
        i += 1;
    }
    out
}
