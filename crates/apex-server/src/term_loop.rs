//! The pty event loop: alacritty's, with acme's `win` label scan in front
//! of the parser. plan9port's `win` watches its shell's output for
//! `ESC ] ; label BEL` (what its `label` and `awd` commands emit) and
//! names the window after it; we also take OSC 7, the working-directory
//! report shells commonly emit. alacritty drops both, so they are cut out
//! of the stream here, before it parses.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use alacritty_terminal::event::{self, Event, EventListener, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi;
use polling::{Event as PollingEvent, Events, PollMode};

/// Max bytes to read from the pty before forced terminal synchronization.
const READ_BUFFER_SIZE: usize = 0x10_0000;
/// Max bytes to read from the pty while the terminal is locked.
const MAX_LOCKED_READ: usize = u16::MAX as usize;
/// An unterminated OSC longer than this is not a label: it goes through.
const MAX_LABEL: usize = 4096;
/// The pty's polling tokens (alacritty's tty module keeps them private).
const PTY_CHILD_EVENT_TOKEN: usize = 1;
const PTY_READ_WRITE_TOKEN: usize = 0;

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

/// Messages that may be sent to the `EventLoop`.
#[derive(Debug)]
pub enum Msg {
    /// Data that should be written to the pty.
    Input(Cow<'static, [u8]>),
    /// Stop the loop (the terminal is closing).
    Shutdown,
    /// Resize the pty.
    Resize(WindowSize),
}

/// Handles all the pty I/O and runs the parser which updates terminal state.
pub struct EventLoop<T: tty::EventedPty, U: EventListener> {
    poll: Arc<polling::Poller>,
    pty: T,
    rx: PeekableReceiver<Msg>,
    tx: Sender<Msg>,
    terminal: Arc<FairMutex<Term<U>>>,
    event_proxy: U,
    drain_on_exit: bool,
    on_label: Box<dyn FnMut(Label) + Send>,
    carry: Vec<u8>,
}

impl<T, U> EventLoop<T, U>
where
    T: tty::EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Send + 'static,
{
    pub fn new(
        terminal: Arc<FairMutex<Term<U>>>,
        event_proxy: U,
        pty: T,
        drain_on_exit: bool,
        on_label: Box<dyn FnMut(Label) + Send>,
    ) -> io::Result<EventLoop<T, U>> {
        let (tx, rx) = mpsc::channel();
        let poll = polling::Poller::new()?.into();
        Ok(EventLoop { poll, pty, tx, rx: PeekableReceiver::new(rx), terminal, event_proxy, drain_on_exit, on_label, carry: Vec::new() })
    }

    pub fn channel(&self) -> EventLoopSender {
        EventLoopSender { sender: self.tx.clone(), poller: self.poll.clone() }
    }

    /// Drain the channel; `false` once a shutdown message arrived.
    fn drain_recv_channel(&mut self, state: &mut State) -> bool {
        while let Some(msg) = self.rx.recv() {
            match msg {
                Msg::Input(input) => state.write_list.push_back(input),
                Msg::Resize(window_size) => self.pty.on_resize(window_size),
                Msg::Shutdown => return false,
            }
        }
        true
    }

    #[inline]
    fn pty_read(&mut self, state: &mut State, buf: &mut [u8]) -> io::Result<()> {
        let mut unprocessed = 0;
        let mut processed = 0;

        // Reserve the next terminal lock for pty reading.
        let _terminal_lease = Some(self.terminal.lease());
        let mut terminal = None;

        loop {
            match self.pty.reader().read(&mut buf[unprocessed..]) {
                // macOS: no more data is readable from the pty
                Ok(0) if unprocessed == 0 => break,
                Ok(got) => unprocessed += got,
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted | ErrorKind::WouldBlock => {
                        if unprocessed == 0 {
                            break;
                        }
                    }
                    _ => return Err(err),
                },
            }

            let terminal = match &mut terminal {
                Some(terminal) => terminal,
                None => terminal.insert(match self.terminal.try_lock_unfair() {
                    // Force block if we are at the buffer size limit.
                    None if unprocessed >= READ_BUFFER_SIZE => self.terminal.lock_unfair(),
                    None => continue,
                    Some(terminal) => terminal,
                }),
            };

            // acme's win: labels leave the stream before the parser sees it
            let (bytes, labels) = scan(&mut self.carry, &buf[..unprocessed]);
            for l in labels {
                (self.on_label)(l);
            }
            state.parser.advance(&mut **terminal, &bytes);

            processed += unprocessed;
            unprocessed = 0;

            if processed >= MAX_LOCKED_READ {
                break;
            }
        }

        // Queue a redraw unless all processed bytes were synchronized.
        if state.parser.sync_bytes_count() < processed && processed > 0 {
            self.event_proxy.send_event(Event::Wakeup);
        }
        Ok(())
    }

    #[inline]
    fn pty_write(&mut self, state: &mut State) -> io::Result<()> {
        state.ensure_next();
        'write_many: while let Some(mut current) = state.take_current() {
            'write_one: loop {
                match self.pty.writer().write(current.remaining_bytes()) {
                    Ok(0) => {
                        state.set_current(Some(current));
                        break 'write_many;
                    }
                    Ok(n) => {
                        current.advance(n);
                        if current.finished() {
                            state.goto_next();
                            break 'write_one;
                        }
                    }
                    Err(err) => {
                        state.set_current(Some(current));
                        match err.kind() {
                            ErrorKind::Interrupted | ErrorKind::WouldBlock => break 'write_many,
                            _ => return Err(err),
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn spawn(mut self) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("pty reader".into())
            .spawn(move || {
                let mut state = State::default();
                let mut buf = vec![0u8; READ_BUFFER_SIZE];

                let poll_opts = PollMode::Level;
                let mut interest = PollingEvent::readable(0);

                // SAFETY: the pty outlives its registration; it is deregistered below.
                if let Err(err) = unsafe { self.pty.register(&self.poll, interest, poll_opts) } {
                    eprintln!("apex: terminal: event loop registration: {err}");
                    return;
                }

                let mut events = Events::with_capacity(NonZeroUsize::new(1024).unwrap());

                'event_loop: loop {
                    // Wake up when a synchronized update timeout was reached.
                    let handler = state.parser.sync_timeout();
                    let timeout = handler.sync_timeout().map(|st| st.saturating_duration_since(Instant::now()));

                    events.clear();
                    if let Err(err) = self.poll.wait(&mut events, timeout) {
                        match err.kind() {
                            ErrorKind::Interrupted => continue,
                            _ => {
                                eprintln!("apex: terminal: polling: {err}");
                                break 'event_loop;
                            }
                        }
                    }

                    // Handle synchronized update timeout.
                    if events.is_empty() && self.rx.peek().is_none() {
                        state.parser.stop_sync(&mut *self.terminal.lock());
                        self.event_proxy.send_event(Event::Wakeup);
                        continue;
                    }

                    if !self.drain_recv_channel(&mut state) {
                        break;
                    }

                    for event in events.iter() {
                        match event.key {
                            PTY_CHILD_EVENT_TOKEN => {
                                if let Some(tty::ChildEvent::Exited(code)) = self.pty.next_child_event() {
                                    if let Some(code) = code {
                                        self.event_proxy.send_event(Event::ChildExit(code));
                                    }
                                    if self.drain_on_exit {
                                        let _ = self.pty_read(&mut state, &mut buf);
                                    }
                                    self.terminal.lock().exit();
                                    self.event_proxy.send_event(Event::Wakeup);
                                    break 'event_loop;
                                }
                            }
                            PTY_READ_WRITE_TOKEN => {
                                if event.is_interrupt() {
                                    continue; // no I/O on a dead pty
                                }
                                if event.readable {
                                    if let Err(err) = self.pty_read(&mut state, &mut buf) {
                                        // Linux: a read on the master side fails with EIO when
                                        // the client side hangs up; the Exited event follows.
                                        #[cfg(target_os = "linux")]
                                        if err.raw_os_error() == Some(libc::EIO) {
                                            continue;
                                        }
                                        eprintln!("apex: terminal: reading the pty: {err}");
                                        break 'event_loop;
                                    }
                                }
                                if event.writable {
                                    if let Err(err) = self.pty_write(&mut state) {
                                        eprintln!("apex: terminal: writing the pty: {err}");
                                        break 'event_loop;
                                    }
                                }
                            }
                            _ => (),
                        }
                    }

                    // Register write interest if necessary.
                    let needs_write = state.needs_write();
                    if needs_write != interest.writable {
                        interest.writable = needs_write;
                        self.pty.reregister(&self.poll, interest, poll_opts).unwrap();
                    }
                }

                let _ = self.pty.deregister(&self.poll);
            })
            .expect("thread spawn works")
    }
}

/// Tracks how much of a buffer has been written.
struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

pub struct Notifier(pub EventLoopSender);

impl event::Notify for Notifier {
    fn notify<B>(&self, bytes: B)
    where
        B: Into<Cow<'static, [u8]>>,
    {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return; // the terminal hangs on 0 bytes
        }
        let _ = self.0.send(Msg::Input(bytes));
    }
}

impl event::OnResize for Notifier {
    fn on_resize(&mut self, window_size: WindowSize) {
        let _ = self.0.send(Msg::Resize(window_size));
    }
}

#[derive(Clone)]
pub struct EventLoopSender {
    sender: Sender<Msg>,
    poller: Arc<polling::Poller>,
}

impl EventLoopSender {
    pub fn send(&self, msg: Msg) -> io::Result<()> {
        self.sender.send(msg).map_err(|e| io::Error::other(e.to_string()))?;
        self.poller.notify()
    }
}

/// The mutable state of the loop: what is to be written, and the parser.
#[derive(Default)]
struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
    parser: ansi::Processor,
}

impl State {
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }
    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(|c| Writing { source: c, written: 0 });
    }
    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }
    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }
    fn set_current(&mut self, new: Option<Writing>) {
        self.writing = new;
    }
}

impl Writing {
    fn advance(&mut self, n: usize) {
        self.written += n;
    }
    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }
    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}

struct PeekableReceiver<T> {
    rx: Receiver<T>,
    peeked: Option<T>,
}

impl<T> PeekableReceiver<T> {
    fn new(rx: Receiver<T>) -> Self {
        Self { rx, peeked: None }
    }
    fn peek(&mut self) -> Option<&T> {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }
        self.peeked.as_ref()
    }
    fn recv(&mut self) -> Option<T> {
        if self.peeked.is_some() {
            self.peeked.take()
        } else {
            match self.rx.try_recv() {
                Err(TryRecvError::Disconnected) => None,
                res => res.ok(),
            }
        }
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
