//! A pty and the shell on it, as alacritty's `tty` module did it: a
//! pseudoterminal, a child in its own session with the slave for its
//! controlling terminal, and the master read and written here.
//!
//! Ghostty's VT library parses the stream and keeps the screen, but it
//! opens no pty and starts no program (it makes no threads at all), so
//! this is apex's.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// A shell on a pty: the master to read and write, and the child.
pub struct Pty {
    master: OwnedFd,
    child: Child,
    /// Set once the child has been waited for; its status is kept.
    exited: Option<i32>,
}

/// What to run, where, and with what in the environment.
pub struct Options {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub dir: PathBuf,
    pub env: HashMap<String, String>,
}

impl Pty {
    /// A pty of `cols` by `rows` with the program running on it.
    pub fn spawn(opts: &Options, cols: u16, rows: u16) -> io::Result<Pty> {
        let pty = rustix_openpty::openpty(None, None)?;
        let (master, slave) = (pty.controller, pty.user);
        // the reader never blocks: the loop polls and drains
        set_nonblocking(&master)?;
        set_size(&master, cols, rows)?;

        let mut cmd = Command::new(&opts.program);
        cmd.args(&opts.args)
            .current_dir(&opts.dir)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?))
            // apex is not its child's terminal: what the shell inherits
            // about a terminal is the pty's, and nothing of ours
            .env_remove("XDG_ACTIVATION_TOKEN")
            .env_remove("DESKTOP_STARTUP_ID");
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        let slave_fd = slave.as_raw_fd();
        // SAFETY: after fork, in the child, before exec: only these calls,
        // none of which allocate or touch our state.
        unsafe {
            cmd.pre_exec(move || {
                // its own session, with the slave as its controlling
                // terminal, so job control and ^C reach it and not us
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                // the signals a shell expects to handle itself
                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop(slave); // the child holds it now; ours would keep it open
        Ok(Pty { master, child, exited: None })
    }

    pub fn fd(&self) -> i32 {
        self.master.as_raw_fd()
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Bytes from the program; `Ok(0)` at end of file.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: a read of `buf.len()` into `buf`.
        let n = unsafe { libc::read(self.fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    /// Bytes to the program; as much as the pty took.
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: a write of `buf.len()` from `buf`.
        let n = unsafe { libc::write(self.fd(), buf.as_ptr() as *const libc::c_void, buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = set_size(&self.master, cols, rows);
    }

    /// The child's status, once it has ended.
    pub fn exit(&mut self) -> Option<i32> {
        if self.exited.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exited = Some(status.code().unwrap_or(0));
            }
        }
        self.exited
    }

    /// Hang up: the shell sees its terminal go, as closing a window does.
    pub fn hangup(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.exited = Some(0);
    }
}

fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: plain fcntl on a file descriptor we own.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The window size the program reads (`TIOCGWINSZ`), pixels included so
/// that a program asking gets the cell size apex draws with.
fn set_size(fd: &OwnedFd, cols: u16, rows: u16) -> io::Result<()> {
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: cols * 8, ws_ypixel: rows * 16 };
    // SAFETY: TIOCSWINSZ takes a winsize, which is what is passed.
    if unsafe { libc::ioctl(fd.as_raw_fd(), libc::TIOCSWINSZ as _, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// What every program on a pty of ours is told, as alacritty's
/// `tty::setup_env` does: the terminal apex draws.
pub fn setup_env() {
    std::env::set_var("TERM", "xterm-256color");
    std::env::set_var("COLORTERM", "truecolor");
    std::env::set_var("TERM_PROGRAM", "apex");
}

/// Where a program's `$SHELL` is, or a plain one.
pub fn shell_path(shell: Option<&str>) -> PathBuf {
    match shell.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => PathBuf::from(s),
        None => match std::env::var_os("SHELL") {
            Some(s) if !s.is_empty() => PathBuf::from(s),
            _ => PathBuf::from("/bin/sh"),
        },
    }
}

/// The name a window takes from the program it runs.
pub fn program_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}
