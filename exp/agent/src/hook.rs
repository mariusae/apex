//! The hook itself: `apex-agent hook claude` (or `codex`), which the
//! agent runs at every event with the event's JSON on its standard
//! input. It says what happened in one line of the session's log and
//! exits 0 whatever else: a hook that fails or dawdles is the agent's
//! problem, and this one must never be.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::event::{self, Event, Tail};
use crate::transcript::call_title;

/// How long a permission request waits on the pane for an answer
/// before it is left to the agent's own prompt.
const WAIT: Duration = Duration::from_secs(90);

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
    // the log away, or the hooks may have been put in mid-session);
    // and so is where the repository stands
    if ev.event == "SessionStart" || !event::log_path(&dir, &ev.session).exists() {
        ev.pid = agent_pid(agent);
        (ev.apex, ev.win) = here(std::env::var("apexsession").ok().as_deref(), std::env::var("winid").ok().as_deref());
        if !ev.cwd.is_empty() {
            ev.rev = crate::vcs::head(Path::new(&ev.cwd));
        }
    }
    let log = event::log_path(&dir, &ev.session);
    // a question is asked of the pane, when there is one: the answer
    // comes back as a `Decision` event in the log, and is the agent's
    // decision. None in time, or `ask`, and the agent's own prompt has it
    let from = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    let _ = event::append(&dir, &ev);
    if ev.event == "PermissionRequest" && event::pane_present(&dir) {
        if let Some(call) = ev.call.as_deref() {
            if let Some(d) = await_decision(&log, from, call, WAIT) {
                println!("{}", decision_json(&d));
            }
        }
    }
    0
}

/// Wait for a `Decision` event about `call` to land in the log after
/// `from`; the pane's word, `allow` or `deny`. `None` when it is `ask`
/// (the terminal's prompt is wanted) or none comes in time.
pub fn await_decision(log: &Path, from: u64, call: &str, wait: Duration) -> Option<String> {
    let mut tail = Tail::new(log.to_path_buf());
    tail.read = from;
    let deadline = Instant::now() + wait;
    loop {
        for e in event::events(&tail.lines()) {
            if e.event == "Decision" && e.call.as_deref() == Some(call) {
                return match e.kind.as_deref() {
                    Some(d @ ("allow" | "deny")) => Some(d.to_string()),
                    _ => None,
                };
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What the agent is told: Claude Code's shape for a permission
/// decision, which Codex shares.
pub fn decision_json(behavior: &str) -> String {
    serde_json::json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": { "behavior": behavior, "message": format!("{behavior}ed in apex") } } }).to_string()
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
    // a prompt and an answer are kept whole, within reason: the page
    // shows the whole of the last exchange. A notification is a line
    let text = match event.as_str() {
        "UserPromptSubmit" => s("prompt").map(|t| cut(&t, WHOLE)),
        "Stop" | "SubagentStop" => s("last_assistant_message").map(|t| cut(&t, WHOLE)),
        "Notification" => s("message").map(|t| cut(&t, LINE)),
        "StopFailure" => s("message").or_else(|| s("error")).map(|t| cut(&t, LINE)),
        _ => None,
    };
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
        apex: None,
        win: None,
        rev: None,
        sub: s("agent_id"),
        tool,
        call: s("tool_use_id"),
        title,
        text,
        kind,
        mode: s("permission_mode"),
    }
}

/// How much of a prompt or an answer is kept, and of anything else.
const WHOLE: usize = 64 * 1024;
const LINE: usize = 2000;

/// The first `n` characters, and a mark where the rest was.
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

/// Where the agent was started, when it was started from apex: the
/// session (`apexsession`) and the window (`winid`, `0` for none),
/// which apex puts in a command's environment and the agent passes on
/// to its hooks.
fn here(session: Option<&str>, win: Option<&str>) -> (Option<String>, Option<u64>) {
    let session = session.map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let win = win.and_then(|w| w.trim().parse::<u64>().ok()).filter(|&w| w != 0);
    (session, win)
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
    fn where_the_agent_was_started_is_kept_when_apex_says() {
        assert_eq!(here(Some("9e21ab77-x"), Some("42")), (Some("9e21ab77-x".into()), Some(42)));
        assert_eq!(here(Some("9e21ab77-x"), Some("0")), (Some("9e21ab77-x".into()), None));
        assert_eq!(here(None, None), (None, None));
        assert_eq!(here(Some(""), Some("x")), (None, None));
    }

    #[test]
    fn a_decision_in_the_log_answers_the_question() {
        let dir = std::env::temp_dir().join(format!("apex-agent-decide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ev = |event: &str, call: &str, kind: Option<&str>| Event { ms: 1, agent: "claude".into(), event: event.into(), session: "s".into(), call: Some(call.into()), kind: kind.map(String::from), ..Event::default() };
        event::append(&dir, &ev("PermissionRequest", "t1", None)).unwrap();
        let log = event::log_path(&dir, "s");
        let from = std::fs::metadata(&log).unwrap().len();
        // nothing said: none, once the wait is up
        assert_eq!(await_decision(&log, from, "t1", Duration::from_millis(50)), None);
        // another call's answer is not this one's; then this one's comes
        event::append(&dir, &ev("Decision", "t0", Some("deny"))).unwrap();
        event::append(&dir, &ev("Decision", "t1", Some("allow"))).unwrap();
        assert_eq!(await_decision(&log, from, "t1", Duration::from_millis(50)), Some("allow".into()));
        event::append(&dir, &ev("Decision", "t2", Some("ask"))).unwrap();
        assert_eq!(await_decision(&log, from, "t2", Duration::from_millis(50)), None);
        assert!(decision_json("allow").contains("\"behavior\":\"allow\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_text_is_cut_and_says_so() {
        let long = "x".repeat(3000);
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "Notification", "message": long, "cwd": "/"});
        let e = event_from("claude", &v);
        assert_eq!(e.text.as_ref().map(|t| t.chars().count()), Some(2001));
        // a prompt is kept whole: the page shows it
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": "x".repeat(3000), "cwd": "/"});
        let e = event_from("claude", &v);
        assert_eq!(e.text.as_ref().map(|t| t.chars().count()), Some(3000));
    }
}
