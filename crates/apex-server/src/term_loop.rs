//! The pty event loop: the bytes a program writes, read and given to
//! the terminal, with acme's `win` label scan in front of it, and the
//! keys and answers written back.
//!
//! plan9port's `win` watches its shell's output for `ESC ] ; label BEL`
//! (what its `label` and `awd` commands emit) and names the window after
//! it; we also take OSC 7, the working-directory report shells commonly
//! emit. Neither is for the parser, so they are cut out of the stream
//! here, before it sees them.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ghostty_vt_sys::{Event as VtEvent, Terminal};

use crate::pty::Pty;

/// Read from the pty at once.
const READ_BUFFER_SIZE: usize = 0x10_000;
/// An unterminated OSC longer than this is not a label: it goes through.
const MAX_LABEL: usize = 4096;

/// What the scan cut out of the shell's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Label {
    /// `ESC ] ; text BEL`: the window's name (acme's win).
    Name(String),
    /// OSC 7: the shell's working directory, a `file://` URL or a path.
    Cwd(String),
}

/// Cut labels out of `new` (with what an earlier call held back in
/// `carry`): the bytes for the parser, and the labels found. An OSC that
/// has not ended yet is held back, so a label split across reads is still
/// one label.
pub fn scan(carry: &mut Vec<u8>, new: &[u8]) -> (Vec<u8>, Vec<Label>) {
    let mut data = std::mem::take(carry);
    data.extend_from_slice(new);
    let mut out = Vec::with_capacity(data.len());
    let mut labels = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if data[i] != 0x1b {
            out.push(data[i]);
            i += 1;
            continue;
        }
        if i + 1 == data.len() {
            carry.push(0x1b); // a lone ESC: the rest is still to come
            break;
        }
        if data[i + 1] != b']' {
            out.push(data[i]);
            i += 1;
            continue;
        }
        // an OSC: it ends at BEL or ESC \ (an ESC before anything else aborts it)
        let start = i + 2;
        let mut j = start;
        let mut end = None; // (body end, next byte)
        let mut aborted = false;
        while j < data.len() {
            match data[j] {
                7 => {
                    end = Some((j, j + 1));
                    break;
                }
                0x1b if j + 1 == data.len() => break, // unterminated so far
                0x1b if data[j + 1] == b'\\' => {
                    end = Some((j, j + 2));
                    break;
                }
                0x1b => {
                    aborted = true;
                    break;
                }
                _ => j += 1,
            }
        }
        match end {
            Some((body_end, next)) => {
                let body = &data[start..body_end];
                let lossy = |b: &[u8]| String::from_utf8_lossy(b).to_string();
                if let Some(t) = body.strip_prefix(b";") {
                    labels.push(Label::Name(lossy(t)));
                } else if let Some(t) = body.strip_prefix(b"7;") {
                    labels.push(Label::Cwd(lossy(t)));
                } else {
                    out.extend_from_slice(&data[i..next]);
                }
                i = next;
            }
            None if !aborted && data.len() - i <= MAX_LABEL => {
                carry.extend_from_slice(&data[i..]);
                break;
            }
            None => {
                out.extend_from_slice(&data[i..j]);
                i = j;
            }
        }
    }
    (out, labels)
}

/// What the loop is told to do.
#[derive(Debug)]
pub enum Msg {
    /// Bytes for the program.
    Input(Vec<u8>),
    /// The window is a different size.
    Resize { cols: u16, rows: u16 },
    /// Stop the loop (the terminal is closing).
    Shutdown,
}

/// What the loop reports: the labels the scan took, what the program
/// asked for, and the end of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    Label(Label),
    /// The title (OSC 0, OSC 2).
    Title(String),
    /// OSC 52: text for the snarf buffer.
    Clipboard(String),
    Bell,
    /// The screen changed: what is drawn is behind it.
    Wakeup,
    /// The program ended.
    Exit(i32),
}

/// Sends to a loop, and wakes it from its wait.
#[derive(Clone)]
pub struct Notifier(pub Sender<Msg>, Arc<Waker>);

impl Notifier {
    pub fn send(&self, msg: Msg) {
        if self.0.send(msg).is_ok() {
            self.1.wake();
        }
    }

    pub fn notify(&self, bytes: Vec<u8>) {
        self.send(Msg::Input(bytes));
    }
}

/// A pipe the loop waits on beside the pty, so a message interrupts the
/// wait (the pty itself may be silent for hours).
struct Waker {
    read: i32,
    write: i32,
}

impl Waker {
    fn new() -> io::Result<Waker> {
        let mut fds = [0i32; 2];
        // SAFETY: pipe fills two descriptors.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in fds {
            // SAFETY: plain fcntl on our own descriptors.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL, 0);
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
        Ok(Waker { read: fds[0], write: fds[1] })
    }

    fn wake(&self) {
        // SAFETY: one byte into the pipe; a full pipe is a wake already sent.
        unsafe { libc::write(self.write, [1u8].as_ptr() as *const libc::c_void, 1) };
    }

    fn drain(&self) {
        let mut buf = [0u8; 64];
        // SAFETY: a read into our own buffer, non-blocking.
        while unsafe { libc::read(self.read, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) } > 0 {}
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        // SAFETY: ours to close, once.
        unsafe {
            libc::close(self.read);
            libc::close(self.write);
        }
    }
}

/// The pty, the terminal it feeds, and whoever hears about it.
pub struct EventLoop {
    pty: Pty,
    terminal: Arc<Mutex<Terminal>>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    waker: Arc<Waker>,
    report: Box<dyn FnMut(Report) + Send>,
    carry: Vec<u8>,
}

impl EventLoop {
    pub fn new(terminal: Arc<Mutex<Terminal>>, pty: Pty, report: Box<dyn FnMut(Report) + Send>) -> io::Result<EventLoop> {
        let (tx, rx) = mpsc::channel();
        Ok(EventLoop { pty, terminal, rx, tx, waker: Arc::new(Waker::new()?), report, carry: Vec::new() })
    }

    pub fn channel(&self) -> Notifier {
        Notifier(self.tx.clone(), self.waker.clone())
    }

    pub fn spawn(mut self) -> JoinHandle<()> {
        std::thread::Builder::new().name("pty".into()).spawn(move || self.run()).expect("the pty thread")
    }

    fn run(&mut self) {
        let mut buf = vec![0u8; READ_BUFFER_SIZE];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            if !self.messages(&mut pending) {
                self.pty.hangup();
                return;
            }
            self.flush(&mut pending);
            let mut fds = [
                libc::pollfd { fd: self.pty.fd(), events: libc::POLLIN | if pending.is_empty() { 0 } else { libc::POLLOUT }, revents: 0 },
                libc::pollfd { fd: self.waker.read, events: libc::POLLIN, revents: 0 },
            ];
            // SAFETY: two descriptors of ours, waited on until one is ready.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if fds[1].revents != 0 {
                self.waker.drain();
            }
            if fds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 && !self.read(&mut buf) {
                break;
            }
            // the program ended: say so, once what it last wrote is read
            if let Some(status) = self.pty.exit() {
                let mut buf2 = vec![0u8; READ_BUFFER_SIZE];
                let _ = self.read(&mut buf2);
                (self.report)(Report::Exit(status));
                return;
            }
        }
        (self.report)(Report::Exit(0));
    }

    /// What was asked of the loop; false once it is to stop.
    fn messages(&mut self, pending: &mut Vec<u8>) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(Msg::Input(bytes)) => pending.extend_from_slice(&bytes),
                Ok(Msg::Resize { cols, rows }) => {
                    self.pty.resize(cols, rows);
                    if let Ok(mut t) = self.terminal.lock() {
                        t.resize(cols, rows);
                    }
                    (self.report)(Report::Wakeup);
                }
                Ok(Msg::Shutdown) => return false,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    /// As much of what is waiting as the pty takes.
    fn flush(&mut self, pending: &mut Vec<u8>) {
        while !pending.is_empty() {
            match self.pty.write(pending) {
                Ok(0) => return,
                Ok(n) => {
                    pending.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted => return,
                Err(_) => {
                    pending.clear();
                    return;
                }
            }
        }
    }

    /// Everything the program has written: the labels out of it, the rest
    /// to the terminal, and what it asked for answered.
    fn read(&mut self, buf: &mut [u8]) -> bool {
        let mut any = false;
        loop {
            let got = match self.pty.read(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted => break,
                Err(_) => return false,
            };
            any = true;
            let (bytes, labels) = scan(&mut self.carry, &buf[..got]);
            for l in labels {
                (self.report)(Report::Label(l));
            }
            let events = match self.terminal.lock() {
                Ok(mut t) => {
                    t.write(&bytes);
                    t.events()
                }
                Err(_) => return false,
            };
            let mut answers: Vec<u8> = Vec::new();
            for e in events {
                match e {
                    VtEvent::WritePty(b) => answers.extend_from_slice(&b),
                    VtEvent::Title(t) => (self.report)(Report::Title(t)),
                    VtEvent::Clipboard(t) => (self.report)(Report::Clipboard(t)),
                    VtEvent::Bell => (self.report)(Report::Bell),
                }
            }
            if !answers.is_empty() {
                self.flush(&mut answers);
            }
            if got < buf.len() {
                break;
            }
        }
        if any {
            (self.report)(Report::Wakeup);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> (Vec<u8>, Vec<Label>) {
        let mut carry = Vec::new();
        let mut out = Vec::new();
        let mut labels = Vec::new();
        for c in chunks {
            let (o, l) = scan(&mut carry, c);
            out.extend(o);
            labels.extend(l);
        }
        out.extend(carry);
        (out, labels)
    }

    #[test]
    fn labels_leave_the_stream() {
        let (out, labels) = run(&[b"hi \x1b];/tmp/-x\x07there"]);
        assert_eq!(out, b"hi there");
        assert_eq!(labels, vec![Label::Name("/tmp/-x".into())]);
        let (out, labels) = run(&[b"\x1b]7;file://h/tmp\x1b\\ok"]);
        assert_eq!(out, b"ok");
        assert_eq!(labels, vec![Label::Cwd("file://h/tmp".into())]);
    }

    #[test]
    fn other_sequences_pass_through() {
        let (out, labels) = run(&[b"\x1b]2;title\x07\x1b[31mred\x1b[0m\x1b"]);
        assert_eq!(out, b"\x1b]2;title\x07\x1b[31mred\x1b[0m\x1b");
        assert!(labels.is_empty());
    }

    #[test]
    fn a_label_split_across_reads_is_one_label() {
        let (out, labels) = run(&[b"a\x1b];/tmp", b"/-x", b"\x07b"]);
        assert_eq!(out, b"ab");
        assert_eq!(labels, vec![Label::Name("/tmp/-x".into())]);
        // an ESC that aborts the sequence lets everything through
        let (out, labels) = run(&[b"\x1b];oops\x1b[0m"]);
        assert_eq!(out, b"\x1b];oops\x1b[0m");
        assert!(labels.is_empty());
    }
}
