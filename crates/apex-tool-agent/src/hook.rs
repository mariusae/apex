//! The hook itself: `apex tool agent hook claude` (or `codex`, or
//! `muse`), which the agent runs at every event with the event's JSON
//! on its standard input. It says what happened in one line of the
//! session's log and exits 0 whatever else: a hook that fails or
//! dawdles is the agent's problem, and this one must never be.

use std::io::Read;
use std::path::{Path, PathBuf};
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
    // Muse's subagents keep sessions of their own, with no word of the
    // parent in what the hook is handed: their events are folded into
    // the parent's log, and its own machinery's are dropped outright
    if agent == "muse" {
        match route_subsession(&dir, ev) {
            Some(e) => ev = e,
            None => return 0,
        }
    }
    // the process is looked for once: when the session starts, or when
    // nothing has been written of it yet (the viewer may have cleaned
    // the log away, or the hooks may have been put in mid-session);
    // and so is where the repository stands
    if ev.event == "SessionStart" || !event::log_path(&dir, &ev.session).exists() {
        ev.pid = agent_pid(agent);
        (ev.apex, ev.win) = here(std::env::var("apexsession").ok().as_deref(), std::env::var("winid").ok().as_deref(), ev.pid);
        if !ev.cwd.is_empty() {
            ev.rev = crate::vcs::head(Path::new(&ev.cwd));
        }
    }
    let log = event::log_path(&dir, &ev.session);
    // a question is asked of the pane, when there is one: the answer
    // comes back as a `Decision` event in the log, and is the agent's
    // decision. None in time, or `ask`, and the agent's own prompt has it.
    // A subagent's question is never asked: the pane has nothing to say
    // to it, and waiting would hold the agent up for nothing
    let from = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    let _ = event::append(&dir, &ev);
    if ev.event == "PermissionRequest" && ev.sub.is_none() && event::pane_present(&dir) {
        if let Some(call) = ev.call.as_deref() {
            if let Some(d) = await_decision(&log, from, call, WAIT) {
                println!("{}", decision_json(&d));
            }
        }
    }
    0
}

/// Where a Muse subagent's session is remembered: a file a child,
/// named by it, saying its parent's session, `drop` for Muse's own
/// machinery, or nothing while the parent is not found yet.
fn subs_dir(dir: &Path) -> PathBuf {
    dir.join("subs")
}

/// A Muse event put where it belongs: its own session's log, or its
/// parent's when it is a subagent's, found on disk under the parent's
/// session (`subagent/CHILD`), since the hook is told nothing of the
/// parent. Muse's own observers (the `*-reminder` kind, and whatever
/// calls their tool) are not sessions to show, and their events are
/// dropped. None is nothing to log.
fn route_subsession(dir: &Path, mut ev: Event) -> Option<Event> {
    if ev.tool.as_deref() == Some("submit_reminder_decision") {
        return None;
    }
    let child = ev.session.clone();
    if !child.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some(ev);
    }
    let marker = subs_dir(dir).join(&child);
    match ev.event.as_str() {
        "SubagentStart" => {
            sweep_markers(dir);
            if ev.kind.as_deref().is_some_and(|k| k.ends_with("-reminder")) {
                write_marker(&marker, "drop");
                return None;
            }
            match crate::history::resolve_parent(&crate::history::muse_home(), &child) {
                Some(parent) => {
                    write_marker(&marker, &parent);
                    Some(sub_event(ev, &parent, &child))
                }
                None => {
                    write_marker(&marker, "");
                    None
                }
            }
        }
        "SubagentStop" => {
            let remembered = read_marker(&marker);
            let _ = std::fs::remove_file(&marker);
            if remembered.as_deref() == Some("drop") {
                return None;
            }
            remembered
                .filter(|p| !p.is_empty())
                .or_else(|| crate::history::resolve_parent(&crate::history::muse_home(), &child))
                .map(|parent| sub_event(ev, &parent, &child))
        }
        _ => match read_marker(&marker) {
            Some(parent) if !parent.is_empty() && parent != "drop" => {
                ev.session = parent;
                ev.sub = Some(child);
                Some(ev)
            }
            Some(dropped) if dropped == "drop" => None,
            // pending: the parent may have shown since, and is
            // remembered from now on
            Some(_) => match crate::history::resolve_parent(&crate::history::muse_home(), &child) {
                Some(parent) => {
                    write_marker(&marker, &parent);
                    ev.session = parent;
                    ev.sub = Some(child);
                    Some(ev)
                }
                None => None,
            },
            // the first word of a session never started in our hearing:
            // a subagent's, when a parent for it is on disk (the hooks
            // put in mid-session), else the session's own
            None if !event::log_path(dir, &child).exists() => match crate::history::resolve_parent(&crate::history::muse_home(), &child) {
                Some(parent) => {
                    write_marker(&marker, &parent);
                    ev.session = parent;
                    ev.sub = Some(child);
                    Some(ev)
                }
                None => Some(ev),
            },
            None => Some(ev),
        },
    }
}

/// A subagent's start or stop as its parent's log keeps it: the
/// parent's session, the child as the sub, and what kind it is. What
/// the pane wants of the parent -- its process, where it was started,
/// where its repository stands -- is the parent's own, and not said
/// again here.
fn sub_event(ev: Event, parent: &str, child: &str) -> Event {
    Event {
        session: parent.to_string(),
        sub: Some(child.to_string()),
        kind: ev.kind.filter(|k| !looks_like_id(k)),
        text: None,
        tool: None,
        call: None,
        title: None,
        transcript: None,
        pid: None,
        apex: None,
        win: None,
        rev: None,
        mode: None,
        plan: None,
        ..ev
    }
}

/// Whether a subagent's kind is no kind at all but an id (Muse names
/// its workflow children by one): the pane shows `subagent` then.
fn looks_like_id(kind: &str) -> bool {
    kind.len() >= 32 && kind.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn write_marker(marker: &Path, parent: &str) {
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker, parent);
}

fn read_marker(marker: &Path) -> Option<String> {
    std::fs::read_to_string(marker).ok().map(|s| s.trim().to_string())
}

/// Markers whose subagent never said it stopped (a session killed
/// outright says nothing) are not left forever: a week is longer than
/// any of them lives.
fn sweep_markers(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(subs_dir(dir)) else { return };
    let week_ago = std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(7 * 24 * 3600));
    for e in rd.flatten() {
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).zip(week_ago).is_some_and(|(t, ago)| t < ago);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
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
    // a plan set: its steps done, of how many -- how far along the agent
    // says it is
    let plan = match tool.as_deref() {
        Some("TodoWrite") | Some("write_todos") => v["tool_input"]["todos"].as_array(),
        Some("update_plan") => v["tool_input"]["plan"].as_array(),
        _ => None,
    }
    .map(|steps| (steps.iter().filter(|t| t["status"].as_str() == Some("completed")).count() as u32, steps.len() as u32));
    // a prompt and an answer are kept whole, within reason: the page
    // shows the whole of the last exchange. A notification is a line.
    // Nothing at all, of nothing said
    let text = match event.as_str() {
        "UserPromptSubmit" => s("prompt").map(|t| cut(&t, WHOLE)),
        "Stop" | "SubagentStop" => s("last_assistant_message").map(|t| cut(&t, WHOLE)),
        "Notification" => s("message").map(|t| cut(&t, LINE)),
        "StopFailure" => s("message").or_else(|| s("error")).map(|t| cut(&t, LINE)),
        _ => None,
    }
    .filter(|t| !t.trim().is_empty());
    let kind = s("notification_type")
        .or_else(|| s("session_start_method"))
        .or_else(|| s("source"))
        .or_else(|| s("session_end_reason"))
        .or_else(|| s("reason"))
        .or_else(|| s("agent_type"))
        .or_else(|| s("subagent_id"))
        .or_else(|| s("trigger"));
    Event {
        ms: 0,
        agent: agent.to_string(),
        event: event.clone(),
        session: s("session_id").unwrap_or_else(|| "unknown".to_string()),
        cwd,
        transcript: s("transcript_path"),
        pid: None,
        apex: None,
        win: None,
        rev: None,
        sub: s("agent_id"),
        tool: tool.clone(),
        call: s("tool_use_id").or_else(|| (agent == "muse" && event == "PermissionRequest").then(|| request_id(tool.as_deref(), v.get("tool_input").unwrap_or(&Value::Null)))),
        title,
        text,
        kind,
        mode: s("permission_mode"),
        plan,
    }
}

/// A question names no call, as Muse has it: one is made from the
/// request itself, so that the pane's answer finds its question.
fn request_id(tool: Option<&str>, input: &Value) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    tool.hash(&mut h);
    input.to_string().hash(&mut h);
    format!("muse-{:08x}", h.finish() as u32)
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
/// to its hooks. Muse hands its hooks neither, keeping its own
/// counsel about the environment; they are read from the agent's own
/// instead, which has them as it was started.
fn here(session: Option<&str>, win: Option<&str>, pid: Option<u32>) -> (Option<String>, Option<u64>) {
    let (mut session, mut win) = (session.map(str::trim).filter(|s| !s.is_empty()).map(String::from), win.and_then(|w| w.trim().parse::<u64>().ok()).filter(|&w| w != 0));
    if session.is_none() || win.is_none() {
        if let Some(pid) = pid {
            // from the end: what `ps` says has the command first, and
            // the environment after it
            let env = env_of_pid(pid);
            if session.is_none() {
                session = env.iter().rev().find(|(k, _)| k == "apexsession").map(|(_, v)| v.clone()).filter(|s| !s.is_empty());
            }
            if win.is_none() {
                win = env.iter().rev().find(|(k, _)| k == "winid").and_then(|(_, v)| v.parse::<u64>().ok()).filter(|&w| w != 0);
            }
        }
    }
    (session, win)
}

/// A process's environment: `/proc` has it exactly, and where there
/// is no `/proc` to ask, `ps` says it after the command.
fn env_of_pid(pid: u32) -> Vec<(String, String)> {
    if pid == 0 {
        return Vec::new();
    }
    if let Ok(env) = std::fs::read(format!("/proc/{pid}/environ")) {
        return env.split(|&b| b == 0).filter_map(|e| String::from_utf8_lossy(e).split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
    }
    for args in [["-E", "-p"], ["eww", "-p"]] {
        let out = std::process::Command::new("ps").arg(args[0]).arg(args[1]).arg(pid.to_string()).output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let env: Vec<(String, String)> = text.split_whitespace().filter_map(|t| t.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
        if !env.is_empty() {
            return env;
        }
    }
    Vec::new()
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
        if is_agent(&name, agent) || RUNTIMES.contains(&name.as_str()) {
            return Some(pid);
        }
        if !SHELLS.contains(&name.as_str()) && fallback.is_none() {
            fallback = Some(pid);
        }
        pid = ppid;
    }
    fallback
}

/// Whether the process is the agent by name: itself, or the binary
/// it runs as (`muse.real`, under a `muse` launcher).
fn is_agent(name: &str, agent: &str) -> bool {
    name == agent || name.strip_suffix(".real").is_some_and(|b| b == agent)
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
    fn a_plan_set_says_how_far_along_the_agent_is() {
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "TodoWrite", "cwd": "/x",
            "tool_input": {"todos": [{"content": "a", "status": "completed"}, {"content": "b", "status": "in_progress"}, {"content": "c", "status": "pending"}]}});
        assert_eq!(event_from("claude", &v).plan, Some((1, 3)));
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "update_plan", "cwd": "/x",
            "tool_input": {"plan": [{"step": "a", "status": "completed"}, {"step": "b", "status": "completed"}]}});
        assert_eq!(event_from("codex", &v).plan, Some((2, 2)));
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Read", "cwd": "/x", "tool_input": {"file_path": "/x/y"}});
        assert_eq!(event_from("claude", &v).plan, None);
    }

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
        assert_eq!(here(Some("9e21ab77-x"), Some("42"), None), (Some("9e21ab77-x".into()), Some(42)));
        assert_eq!(here(Some("9e21ab77-x"), Some("0"), None), (Some("9e21ab77-x".into()), None));
        assert_eq!(here(None, None, None), (None, None));
        assert_eq!(here(Some(""), Some("x"), None), (None, None));
        // what the hooks are not told, the agent's own environment has:
        // a child started with the variables, read while it sleeps
        let mut kid = std::process::Command::new("sleep").arg("30").env("apexsession", "9e21ab77-x").env("winid", "42").spawn().unwrap();
        assert_eq!(here(None, None, Some(kid.id())), (Some("9e21ab77-x".into()), Some(42)));
        assert_eq!(here(Some("elsewhere"), None, Some(kid.id())), (Some("elsewhere".into()), Some(42)));
        let _ = kid.kill();
        let _ = kid.wait();
        assert_eq!(here(None, None, Some(0)), (None, None));
    }

    #[test]
    fn the_agent_is_known_by_its_binary_too() {
        assert!(is_agent("muse", "muse"));
        assert!(is_agent("muse.real", "muse"));
        assert!(is_agent("claude", "claude"));
        assert!(!is_agent("muse.real", "claude"));
        assert!(!is_agent("sh", "muse"));
        assert!(!looks_like_id("skill-reminder"));
        assert!(!looks_like_id("Explore"));
        assert!(looks_like_id("01a1115c-7b7a-7c41-ade1-8a9ed1362f7a"));
    }

    #[test]
    fn a_muse_hooks_input_becomes_one_event_said_in_words() {
        let v: Value = serde_json::json!({
            "session_id": "01a10deb-6ac7", "transcript_path": null, "cwd": "/tmp/work",
            "hook_event_name": "PreToolUse", "permission_mode": "default",
            "tool_name": "write_file", "tool_use_id": "call_1",
            "tool_input": {"content": "hi\n", "path": "notes.txt"}
        });
        let e = event_from("muse", &v);
        assert_eq!(e.event, "PreToolUse");
        assert_eq!(e.session, "01a10deb-6ac7");
        assert_eq!(e.transcript, None);
        assert_eq!(e.call.as_deref(), Some("call_1"));
        assert_eq!(e.title.as_deref(), Some("write_file: notes.txt"));
        // its question names no call: one is made from the request
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "PermissionRequest", "cwd": "/x",
            "tool_name": "bash", "tool_input": {"command": "rm -rf target", "description": "Clean"}});
        let e = event_from("muse", &v);
        assert_eq!(e.title.as_deref(), Some("bash: Clean"));
        let call = e.call.clone().unwrap();
        assert!(call.starts_with("muse-"), "{call}");
        assert_eq!(event_from("muse", &v).call, Some(call.clone()));
        assert_ne!(event_from("muse", &serde_json::json!({"session_id": "s", "hook_event_name": "PermissionRequest", "cwd": "/x",
            "tool_name": "bash", "tool_input": {"command": "ls"}})).call, Some(call));
        // ...but only for muse: the others name theirs
        let e = event_from("claude", &v);
        assert_eq!(e.call, None);
        // its subagents say which by another name, and nothing of the parent
        let v: Value = serde_json::json!({"session_id": "child-1", "child_session_id": "child-1", "subagent_id": "Explore",
            "hook_event_name": "SubagentStart", "cwd": "/x"});
        let e = event_from("muse", &v);
        assert_eq!(e.kind.as_deref(), Some("Explore"));
        // a stop that says nothing says nothing
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "Stop", "last_assistant_message": "", "cwd": "/x"});
        assert_eq!(event_from("muse", &v).text, None);
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "Stop", "last_assistant_message": "Done.", "cwd": "/x"});
        assert_eq!(event_from("muse", &v).text.as_deref(), Some("Done."));
        // its todos are a plan, as the others' are
        let v: Value = serde_json::json!({"session_id": "s", "hook_event_name": "PostToolUse", "tool_name": "write_todos", "cwd": "/x",
            "tool_input": {"todos": [{"text": "a", "status": "completed"}, {"text": "b", "status": "pending"}]}});
        assert_eq!(event_from("muse", &v).plan, Some((1, 2)));
    }

    #[test]
    fn a_muse_subagents_events_are_its_parents_and_its_machinery_is_dropped() {
        let tmp = std::env::temp_dir().join(format!("apex-tool-agent-muse-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // the sessions as muse keeps them: the child's under its parent's
        let home = tmp.join("data").join("muse");
        std::fs::create_dir_all(home.join("sessions/2026/10/05/parent-1/subagent/child-1")).unwrap();
        std::fs::create_dir_all(home.join("sessions/2026/10/05/parent-1/subagent/child-2")).unwrap();
        std::fs::create_dir_all(home.join("sessions/2026/10/05/parent-1/subagent/child-3")).unwrap();
        let prev = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", tmp.join("data"));
        let dir = tmp.join("agent");
        let ev = |event: &str, session: &str| Event { ms: 1, agent: "muse".into(), event: event.into(), session: session.into(), cwd: "/x".into(), ..Event::default() };
        // the parent's own start is its own
        let e = route_subsession(&dir, ev("SessionStart", "parent-1")).unwrap();
        assert_eq!((e.session.as_str(), e.sub), ("parent-1", None));
        // a subagent's start is the parent's, with the child as the sub
        let e = route_subsession(&dir, Event { kind: Some("shell-probe".into()), ..ev("SubagentStart", "child-1") }).unwrap();
        assert_eq!(e.session, "parent-1");
        assert_eq!(e.sub.as_deref(), Some("child-1"));
        assert_eq!(e.kind.as_deref(), Some("shell-probe"));
        assert_eq!(read_marker(&subs_dir(&dir).join("child-1")).as_deref(), Some("parent-1"));
        // ...and so are its calls; an id for a kind is no kind
        let e = route_subsession(&dir, Event { kind: Some("01a1115c-7b7a-7c41-ade1-8a9ed1362f7a".into()), ..ev("SubagentStart", "child-2") }).unwrap();
        assert_eq!(e.kind, None);
        let e = route_subsession(&dir, Event { call: Some("call_9".into()), tool: Some("bash".into()), ..ev("PreToolUse", "child-2") }).unwrap();
        assert_eq!((e.session.as_str(), e.sub.as_deref(), e.call.as_deref()), ("parent-1", Some("child-2"), Some("call_9")));
        // its stop, and the marker goes with it
        let e = route_subsession(&dir, ev("SubagentStop", "child-2")).unwrap();
        assert_eq!((e.session.as_str(), e.sub.as_deref()), ("parent-1", Some("child-2")));
        assert!(!subs_dir(&dir).join("child-2").exists());
        // its own observers are dropped, start to stop
        assert_eq!(route_subsession(&dir, Event { kind: Some("skill-reminder".into()), ..ev("SubagentStart", "child-9") }), None);
        assert_eq!(read_marker(&subs_dir(&dir).join("child-9")).as_deref(), Some("drop"));
        assert_eq!(route_subsession(&dir, Event { tool: Some("submit_reminder_decision".into()), ..ev("PreToolUse", "child-9") }), None);
        // whatever else one says stays dropped, though a parent for it
        // is on disk
        std::fs::create_dir_all(home.join("sessions/2026/10/05/parent-1/subagent/child-9")).unwrap();
        assert_eq!(route_subsession(&dir, Event { tool: Some("bash".into()), ..ev("PreToolUse", "child-9") }), None);
        assert_eq!(route_subsession(&dir, ev("SubagentStop", "child-9")), None);
        assert!(!subs_dir(&dir).join("child-9").exists());
        // a start with no parent on disk yet waits for one: its next
        // word retries, and is its parent's once the parent has shown
        assert_eq!(route_subsession(&dir, Event { kind: Some("Explore".into()), ..ev("SubagentStart", "child-8") }), None);
        assert_eq!(read_marker(&subs_dir(&dir).join("child-8")).as_deref(), Some(""));
        assert_eq!(route_subsession(&dir, Event { tool: Some("bash".into()), ..ev("PreToolUse", "child-8") }), None);
        std::fs::create_dir_all(home.join("sessions/2026/10/05/parent-1/subagent/child-8")).unwrap();
        let e = route_subsession(&dir, Event { tool: Some("bash".into()), ..ev("PostToolUse", "child-8") }).unwrap();
        assert_eq!((e.session.as_str(), e.sub.as_deref()), ("parent-1", Some("child-8")));
        assert_eq!(read_marker(&subs_dir(&dir).join("child-8")).as_deref(), Some("parent-1"));
        // a session never heard starting, with a parent on disk, is a
        // subagent's (the hooks put in mid-session)
        event::append(&dir, &ev("SessionStart", "parent-1")).unwrap();
        let e = route_subsession(&dir, Event { call: Some("call_3".into()), ..ev("PreToolUse", "child-3") }).unwrap();
        assert_eq!((e.session.as_str(), e.sub.as_deref()), ("parent-1", Some("child-3")));
        assert_eq!(read_marker(&subs_dir(&dir).join("child-3")).as_deref(), Some("parent-1"));
        // ...and one with no parent for it is its own
        let e = route_subsession(&dir, ev("UserPromptSubmit", "parent-1")).unwrap();
        assert_eq!((e.session.as_str(), e.sub), ("parent-1", None));
        match prev {
            Some(p) => std::env::set_var("XDG_DATA_HOME", p),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn markers_outlive_no_subagent_by_a_week() {
        let dir = std::env::temp_dir().join(format!("apex-tool-agent-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let subs = subs_dir(&dir);
        std::fs::create_dir_all(&subs).unwrap();
        std::fs::write(subs.join("fresh"), "parent-1").unwrap();
        let old = subs.join("old");
        std::fs::write(&old, "parent-1").unwrap();
        let week = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(week).unwrap();
        sweep_markers(&dir);
        assert!(subs.join("fresh").exists());
        assert!(!old.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_decision_in_the_log_answers_the_question() {
        let dir = std::env::temp_dir().join(format!("apex-tool-agent-decide-{}", std::process::id()));
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
