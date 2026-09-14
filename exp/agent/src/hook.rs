//! The hook itself: `apex-agent hook claude` (or `codex`), which the
//! agent runs at every event with the event's JSON on its standard
//! input. It says what happened in one line of the session's log and
//! exits 0 whatever else: a hook that fails or dawdles is the agent's
//! problem, and this one must never be.

use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::event::{self, Event};
use crate::transcript::call_title;

pub fn run(agent: &str) -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let Ok(v) = serde_json::from_str::<Value>(&input) else { return 0 };
    let mut ev = event_from(agent, &v);
    ev.ms = event::now_ms();
    let dir = event::dir();
    // the process is looked for once: when the session starts, or when
    // nothing has been written of it yet (the viewer may have cleaned
    // the log away, or the hooks may have been put in mid-session)
    if ev.event == "SessionStart" || !event::log_path(&dir, &ev.session).exists() {
        ev.pid = agent_pid(agent);
    }
    let _ = event::append(&dir, &ev);
    0
}

/// The event as the log keeps it: the fields every hook has, and the
/// one or two the pane wants of each kind. The tool's input is said in
/// words rather than kept.
pub fn event_from(agent: &str, v: &Value) -> Event {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    let event = s("hook_event_name").unwrap_or_default();
    let cwd = s("cwd").unwrap_or_default();
    let tool = s("tool_name");
    let title = tool.as_deref().map(|t| call_title(t, v.get("tool_input").unwrap_or(&Value::Null), &cwd));
    let text = match event.as_str() {
        "UserPromptSubmit" => s("prompt"),
        "Stop" | "SubagentStop" => s("last_assistant_message"),
        "Notification" => s("message"),
        "StopFailure" => s("message").or_else(|| s("error")),
        _ => None,
    }
    .map(|t| cut(&t, 2000));
    let kind = s("notification_type")
        .or_else(|| s("session_start_method"))
        .or_else(|| s("source"))
        .or_else(|| s("session_end_reason"))
        .or_else(|| s("reason"))
        .or_else(|| s("agent_type"))
        .or_else(|| s("trigger"));
    Event {
        ms: 0,
        agent: agent.to_string(),
        event,
        session: s("session_id").unwrap_or_else(|| "unknown".to_string()),
        cwd,
        transcript: s("transcript_path"),
        pid: None,
        sub: s("agent_id"),
        tool,
        call: s("tool_use_id"),
        title,
        text,
        kind,
        mode: s("permission_mode"),
    }
}

/// The first `n` characters, and a mark where the rest was.
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

/// The agent's process: the nearest ancestor of ours that is the agent
/// by name (or the node it runs in), else the nearest that is not a
/// shell. A hook is run through a shell more often than not, so the
/// parent is rarely it. What the viewer later asks `kill(pid, 0)`
/// about, since an agent killed outright sends no `SessionEnd`.
fn agent_pid(agent: &str) -> Option<u32> {
    const SHELLS: [&str; 8] = ["sh", "bash", "zsh", "fish", "dash", "rc", "ksh", "tcsh"];
    const RUNTIMES: [&str; 3] = ["node", "bun", "deno"];
    // SAFETY: getppid cannot fail
    let mut pid = unsafe { libc::getppid() } as u32;
    let mut fallback = None;
    for _ in 0..8 {
        if pid <= 1 {
            break;
        }
        let Some((ppid, comm)) = ps(pid) else { break };
        let name = Path::new(comm.trim_start_matches('-')).file_name().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        if name == agent || RUNTIMES.contains(&name.as_str()) {
            return Some(pid);
        }
        if !SHELLS.contains(&name.as_str()) && fallback.is_none() {
            fallback = Some(pid);
        }
        pid = ppid;
    }
    fallback
}

/// A process's parent and command, as `ps` says.
fn ps(pid: u32) -> Option<(u32, String)> {
    let out = std::process::Command::new("ps").args(["-o", "ppid=,comm=", "-p", &pid.to_string()]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.trim();
    let (ppid, comm) = line.split_once(char::is_whitespace)?;
    Some((ppid.trim().parse().ok()?, comm.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hooks_input_becomes_one_event_said_in_words() {
        let v: Value = serde_json::json!({
            "session_id": "abc", "transcript_path": "/t/abc.jsonl", "cwd": "/home/me/proj",
            "hook_event_name": "PreToolUse", "permission_mode": "auto",
            "tool_name": "Bash", "tool_use_id": "toolu_1",
            "tool_input": {"command": "cargo build\ncargo test", "description": "Build and test"}
        });
        let e = event_from("claude", &v);
        assert_eq!(e.event, "PreToolUse");
        assert_eq!(e.session, "abc");
        assert_eq!(e.call.as_deref(), Some("toolu_1"));
        assert_eq!(e.title.as_deref(), Some("Bash: Build and test"));
        assert_eq!(e.mode.as_deref(), Some("auto"));
        assert_eq!(e.text, None);
        let v: Value = serde_json::json!({"session_id": "abc", "hook_event_name": "Stop", "last_assistant_message": "Done.", "cwd": "/x"});
        let e = event_from("codex", &v);
        assert_eq!((e.agent.as_str(), e.text.as_deref()), ("codex", Some("Done.")));
        let v: Value = serde_json::json!({"session_id": "abc", "hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash", "cwd": "/x"});
        let e = event_from("claude", &v);
        assert_eq!(e.kind.as_deref(), Some("permission_prompt"));
        let v: Value = serde_json::json!({"session_id": "abc", "hook_event_name": "PostToolUse", "agent_id": "a1", "agent_type": "Explore", "tool_name": "Read", "tool_input": {"file_path": "/x/y.rs"}, "cwd": "/x"});
        let e = event_from("claude", &v);
        assert_eq!(e.sub.as_deref(), Some("a1"));
        assert_eq!(e.title.as_deref(), Some("Read: y.rs"));
    }

    #[test]
    fn a_long_text_is_cut_and_says_so() {
        let long = "x".repeat(3000);
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": long, "cwd": "/"});
        let e = event_from("claude", &v);
        assert_eq!(e.text.as_ref().map(|t| t.chars().count()), Some(2001));
    }
}
