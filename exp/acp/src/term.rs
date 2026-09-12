//! Running a command for the agent: ACP's terminal capability.
//!
//! A terminal here is a child of ours with its output captured, not a
//! pty. What the protocol asks of a client is to start a command, keep
//! what it writes, say how it ended, and kill it on request; there is
//! nothing interactive in it. Nothing in this file knows about apex:
//! where the output is shown is the caller's business.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How a command ended.
#[derive(Clone, Debug)]
pub struct Exit {
    pub code: Option<u32>,
    pub signal: Option<String>,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.signal, self.code) {
            (Some(s), _) => write!(f, "killed by {s}"),
            (None, Some(c)) => write!(f, "exit {c}"),
            (None, None) => write!(f, "gone"),
        }
    }
}

/// What a command has written: what the agent will be given, and what
/// the window has not been shown yet. The two are kept apart so that
/// dropping the oldest output (the agent's limit) never disturbs what
/// is on its way to the window.
#[derive(Default)]
struct Buf {
    /// The output kept for the agent, to the byte limit.
    kept: String,
    /// Whether anything was dropped from the front of `kept`.
    truncated: bool,
    /// What has arrived since the window was last written.
    fresh: String,
}

/// A command the agent is running.
pub struct Run {
    /// The command as it was asked for, for the window to show.
    pub command: String,
    child: Child,
    buf: Arc<Mutex<Buf>>,
    /// Pipes that have reached their end.
    ends: Arc<AtomicUsize>,
    /// How it ended, and when we first saw that.
    exit: Option<(Exit, Instant)>,
}

/// Output kept when the agent names no limit of its own.
const LIMIT: usize = 1 << 20;

/// How long the output is still waited for once the child is gone: the
/// pipes are read by threads of their own, a moment behind it, and a
/// grandchild may hold them open for good, so the wait is bounded.
const DRAIN: Duration = Duration::from_millis(250);

impl Run {
    /// Start `command`, its output captured. `env` is added to the one
    /// we were started with, as a shell's `VAR=x cmd` would.
    pub fn start(command: &str, args: &[String], env: &[(String, String)], cwd: &Path, limit: Option<u64>) -> std::io::Result<Run> {
        let mut cmd = Command::new(command);
        cmd.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        // a group of its own, so that killing the command kills what it
        // started too: a shell's `cargo build` is a grandchild of ours,
        // and killing the shell alone would leave the build running
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn()?;
        let limit = limit.map(|l| l as usize).unwrap_or(LIMIT).max(1);
        let buf = Arc::new(Mutex::new(Buf::default()));
        let ends = Arc::new(AtomicUsize::new(0));
        // stdout and stderr into the one buffer, as a terminal shows them
        if let Some(o) = child.stdout.take() {
            read_into(o, buf.clone(), ends.clone(), limit);
        }
        if let Some(e) = child.stderr.take() {
            read_into(e, buf.clone(), ends.clone(), limit);
        }
        let mut shown = vec![command.to_string()];
        shown.extend(args.iter().cloned());
        Ok(Run { command: shown.join(" "), child, buf, ends, exit: None })
    }

    /// The output so far, and whether the oldest of it was dropped.
    pub fn output(&self) -> (String, bool) {
        let b = lock(&self.buf);
        (b.kept.clone(), b.truncated)
    }

    /// What has arrived since this was last asked, for the window.
    pub fn take_fresh(&self) -> String {
        std::mem::take(&mut lock(&self.buf).fresh)
    }

    /// How the command ended, once it has; `None` while it runs.
    pub fn exit(&mut self) -> Option<Exit> {
        if let Some((e, _)) = &self.exit {
            return Some(e.clone());
        }
        let e = status(self.child.try_wait().ok()??);
        self.exit = Some((e.clone(), Instant::now()));
        Some(e)
    }

    /// The command is over and its output is all in: what the agent's
    /// `terminal/wait_for_exit` waits for.
    pub fn settled(&mut self) -> Option<Exit> {
        let e = self.exit()?;
        let seen = self.exit.as_ref().map(|(_, t)| *t)?;
        (self.ends.load(Ordering::SeqCst) >= 2 || seen.elapsed() > DRAIN).then_some(e)
    }

    pub fn kill(&mut self) {
        kill(&mut self.child);
    }
}

/// Kill the command and everything it started. The child leads a group
/// of its own (`start`), so the group is what is signalled; `kill` on
/// the child alone would leave its own children behind.
fn kill(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // a reaped child's pid could be another process by now: only
        // signal one we have not waited for
        if pid > 0 && matches!(child.try_wait(), Ok(None)) {
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
    let _ = child.kill();
}

impl Drop for Run {
    /// A command goes with the terminal it runs in: nothing the agent
    /// started outlives it.
    fn drop(&mut self) {
        kill(&mut self.child);
    }
}

/// A poisoned lock is no reason to lose the output: the threads here
/// only ever push text.
fn lock(buf: &Mutex<Buf>) -> std::sync::MutexGuard<'_, Buf> {
    buf.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read a pipe until its end, into the buffer, on a thread of its own.
fn read_into<R: Read + Send + 'static>(mut r: R, buf: Arc<Mutex<Buf>>, ends: Arc<AtomicUsize>, limit: usize) {
    std::thread::spawn(move || {
        let mut pending = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            pending.extend_from_slice(&chunk[..n]);
            add(&buf, &decode(&mut pending, false), limit);
        }
        add(&buf, &decode(&mut pending, true), limit);
        ends.fetch_add(1, Ordering::SeqCst);
    });
}

/// Add to the output, dropping the oldest when it would grow past the
/// limit. The protocol has the client truncate from the front, and at a
/// character boundary. What is on its way to the window is held to our
/// own limit instead: how much the agent wants to read is no reason to
/// show the user less.
fn add(buf: &Mutex<Buf>, text: &str, limit: usize) {
    if text.is_empty() {
        return;
    }
    let b = &mut *lock(buf);
    b.kept.push_str(text);
    b.fresh.push_str(text);
    b.truncated |= trim(&mut b.kept, limit);
    trim(&mut b.fresh, LIMIT);
}

/// Keep the last `limit` bytes, ending at a character boundary. Says
/// whether anything was dropped.
fn trim(s: &mut String, limit: usize) -> bool {
    if s.len() <= limit {
        return false;
    }
    let mut at = s.len() - limit;
    while at < s.len() && !s.is_char_boundary(at) {
        at += 1;
    }
    s.drain(..at);
    true
}

/// Decode what is whole of `pending`, leaving an unfinished character
/// for the next read. Bytes that are no character at all become U+FFFD,
/// as a terminal shows them: a command may write anything.
fn decode(pending: &mut Vec<u8>, end: bool) -> String {
    let mut out = String::new();
    loop {
        let e = match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                return out;
            }
            Err(e) => e,
        };
        let good = e.valid_up_to();
        out.push_str(std::str::from_utf8(&pending[..good]).unwrap_or(""));
        match e.error_len() {
            // junk: one replacement character, and on with the rest
            Some(n) => {
                out.push('\u{fffd}');
                pending.drain(..good + n);
            }
            // the tail is an unfinished character: keep it for the next
            // read, unless there will be none
            None => {
                pending.drain(..good);
                if end {
                    out.push('\u{fffd}');
                    pending.clear();
                }
                return out;
            }
        }
    }
}

fn status(s: std::process::ExitStatus) -> Exit {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(n) = s.signal() {
            return Exit { code: None, signal: Some(signal(n)) };
        }
    }
    Exit { code: s.code().map(|c| c as u32), signal: None }
}

/// The name of a signal a command is killed by, for the ones POSIX
/// numbers the same everywhere.
#[cfg(unix)]
fn signal(n: i32) -> String {
    match n {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return format!("signal {n}"),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cmd: &str, args: &[&str]) -> Run {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Run::start(cmd, &args, &[], Path::new("/"), None).expect("spawn")
    }

    /// Wait for the command to be over, as the window's tick does.
    fn settle(r: &mut Run) -> Exit {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(e) = r.settled() {
                return e;
            }
            assert!(Instant::now() < deadline, "the command never ended");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn output_and_exit_code() {
        let mut r = run("sh", &["-c", "echo out; echo err >&2; exit 3"]);
        let e = settle(&mut r);
        assert_eq!(e.code, Some(3));
        assert_eq!(e.signal, None);
        let (text, truncated) = r.output();
        assert!(text.contains("out"), "{text:?}");
        assert!(text.contains("err"), "{text:?}");
        assert!(!truncated);
    }

    #[test]
    fn fresh_is_taken_once() {
        let mut r = run("sh", &["-c", "echo hello"]);
        settle(&mut r);
        assert_eq!(r.take_fresh(), "hello\n");
        assert_eq!(r.take_fresh(), "");
        // what the agent gets is untouched by what the window took
        assert_eq!(r.output().0, "hello\n");
    }

    #[test]
    fn the_oldest_output_is_dropped_at_the_limit() {
        let args: Vec<String> = ["-c", "printf 'abcdefghij'"].iter().map(|s| s.to_string()).collect();
        let mut r = Run::start("sh", &args, &[], Path::new("/"), Some(4)).expect("spawn");
        settle(&mut r);
        let (text, truncated) = r.output();
        assert_eq!(text, "ghij");
        assert!(truncated);
    }

    #[test]
    fn a_killed_command_reports_its_signal() {
        let mut r = run("sh", &["-c", "sleep 30"]);
        assert!(r.exit().is_none(), "it should still be running");
        r.kill();
        let e = settle(&mut r);
        assert_eq!(e.signal.as_deref(), Some("SIGKILL"));
        assert_eq!(e.code, None);
    }

    #[test]
    fn the_environment_is_added_to_ours() {
        let env = vec![("APEX_ACP_TEST".to_string(), "yes".to_string())];
        let args: Vec<String> = ["-c", "echo $APEX_ACP_TEST $HOME"].iter().map(|s| s.to_string()).collect();
        let mut r = Run::start("sh", &args, &env, Path::new("/"), None).expect("spawn");
        settle(&mut r);
        assert!(r.output().0.starts_with("yes "), "{:?}", r.output().0);
    }

    #[test]
    fn killing_a_command_kills_what_it_started() {
        // the child is a shell; the `sleep` it forks is a grandchild,
        // and only a kill of the whole group reaches it
        let mark = std::env::temp_dir().join(format!("apex-acp-group-{}", std::process::id()));
        let _ = std::fs::remove_file(&mark);
        let script = format!("sleep 1; : > {}", mark.display());
        let mut r = run("sh", &["-c", &script]);
        r.kill();
        settle(&mut r);
        std::thread::sleep(Duration::from_millis(2000));
        assert!(!mark.exists(), "the grandchild outlived the kill");
    }

    #[test]
    fn a_command_that_is_not_there_is_an_error() {
        assert!(Run::start("apex-acp-no-such-command", &[], &[], Path::new("/"), None).is_err());
    }

    #[test]
    fn text_split_across_reads_survives() {
        // a character whose bytes arrive apart
        let mut pending = vec![0xe2, 0x82];
        assert_eq!(decode(&mut pending, false), "");
        pending.push(0xac);
        assert_eq!(decode(&mut pending, false), "€");
        // and bytes that are no character at all
        let mut junk = vec![b'a', 0xff, b'b'];
        assert_eq!(decode(&mut junk, true), "a\u{fffd}b");
    }
}
