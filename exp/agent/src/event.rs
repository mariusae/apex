//! What a hook leaves behind. Each run of one is a line of JSON appended
//! to the session's log, `~/.apex/agents/SESSION.jsonl`, and the viewer
//! reads the logs and nothing else: no socket, no daemon. A hook writes
//! its line and exits, so the agent is held up for as long as that
//! takes and no longer; the viewer watches the directory and reads what
//! is new the moment it lands; a viewer started late sees what
//! happened before it; and two viewers see the same.
//!
//! A line is small. What the hook was handed is not kept -- a `Write`'s
//! input is the file -- but said in words (`title`), which is what the
//! pane shows anyway; the transcript has the rest.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// When, in milliseconds since the epoch.
    pub ms: i64,
    /// Which program: `claude`, `codex`.
    pub agent: String,
    /// The hook's name: `PreToolUse`, `Stop`, ...
    pub event: String,
    pub session: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// The agent's process, found once at the start of the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Where the agent was started, when it was started from apex: the
    /// session and the window, so that `Goto` can go there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub win: Option<u64>,
    /// Where the repository stood when the session began, in its own
    /// words (a commit), so that `Changes` can say what happened since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Set when the hook fired inside a subagent: which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The tool call's id, the same one the transcript uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// The call in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The prompt, the last message, the notification, the failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// What kind of the event it was: the notification's type, how the
    /// session started or ended, why a turn failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// Where the logs are: `$APEX_AGENT_DIR`, else `~/.apex/agents`, beside
/// the rest of what apex keeps under `~/.apex`.
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("APEX_AGENT_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home).join(".apex").join("agents")
}

/// The session's log. Its id is the file's name, so only what is safe
/// in one is kept of it.
pub fn log_path(dir: &Path, session: &str) -> PathBuf {
    let safe: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("{safe}.jsonl"))
}

/// Where the panes say they are: a file a pane, named by its process,
/// under the logs. A hook that has a question looks here before it
/// waits for an answer, since with no pane there is nobody to give one.
pub fn panes_dir(dir: &Path) -> PathBuf {
    dir.join("panes")
}

/// Whether a pane is there to answer: a presence file whose process is
/// alive. Dead ones are cleaned away as they are found.
pub fn pane_present(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(panes_dir(dir)) else { return false };
    let mut present = false;
    for e in rd.flatten() {
        let alive = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()).is_some_and(alive);
        if alive {
            present = true;
        } else {
            let _ = std::fs::remove_file(e.path());
        }
    }
    present
}

/// Whether a process is there.
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 delivers nothing; it asks whether the process is there
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) }
}

/// The session a log file is of, from its name.
pub fn session_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".jsonl").map(String::from)
}

/// Append one event to its session's log. One write of one line, so
/// two hooks of the same session running at once (parallel tool calls)
/// do not tear each other's lines.
pub fn append(dir: &Path, ev: &Event) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut line = serde_json::to_string(ev).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log_path(dir, &ev.session))?;
    f.write_all(line.as_bytes())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A file read as it grows: whole lines since last time, and nothing
/// twice. What a log and a transcript both are.
pub struct Tail {
    pub path: PathBuf,
    /// Bytes taken so far.
    pub read: u64,
    /// A line still being written, kept until the rest of it comes.
    carry: Vec<u8>,
}

impl Tail {
    pub fn new(path: PathBuf) -> Tail {
        Tail { path, read: 0, carry: Vec::new() }
    }

    /// The lines added since last time; none when the file has not
    /// grown. A file shorter than it was is read again from the start.
    pub fn lines(&mut self) -> Vec<String> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(meta) = std::fs::metadata(&self.path) else { return Vec::new() };
        let len = meta.len();
        if len < self.read {
            self.read = 0;
            self.carry.clear();
        }
        if len == self.read {
            return Vec::new();
        }
        let Ok(mut f) = std::fs::File::open(&self.path) else { return Vec::new() };
        if f.seek(SeekFrom::Start(self.read)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if f.take(len - self.read).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        self.read += buf.len() as u64;
        self.carry.extend_from_slice(&buf);
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(i) = self.carry[from..].iter().position(|&b| b == b'\n') {
            let line = &self.carry[from..from + i];
            out.push(String::from_utf8_lossy(line).into_owned());
            from += i + 1;
        }
        self.carry.drain(..from);
        out
    }
}

/// The events in `lines`, the lines that are not events passed over.
pub fn events(lines: &[String]) -> Vec<Event> {
    lines.iter().filter_map(|l| serde_json::from_str::<Event>(l).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_process_is_alive() {
        assert!(alive(std::process::id() as i32));
    }

    #[test]
    fn a_log_is_read_as_it_grows_and_a_half_line_waits() {
        let dir = std::env::temp_dir().join(format!("apex-agent-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ev = Event { ms: 1, agent: "claude".into(), event: "SessionStart".into(), session: "s1".into(), cwd: "/x".into(), ..Event::default() };
        append(&dir, &ev).unwrap();
        let mut t = Tail::new(log_path(&dir, "s1"));
        let got = events(&t.lines());
        assert_eq!(got, vec![ev.clone()]);
        assert!(t.lines().is_empty());
        // half a line is not a line yet
        let mut f = std::fs::OpenOptions::new().append(true).open(&t.path).unwrap();
        f.write_all(b"{\"ms\":2,\"agent\":\"claude\",\"event\":\"Stop\"").unwrap();
        assert!(t.lines().is_empty());
        f.write_all(b",\"session\":\"s1\"}\n").unwrap();
        let got = events(&t.lines());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event, "Stop");
        assert_eq!(session_of(&t.path).as_deref(), Some("s1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
