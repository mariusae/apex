//! The sessions a directory has had, as the agents keep them on disk:
//! Claude Code's under `~/.claude/projects/DIR-AS-A-NAME/`, Codex's
//! rollouts under `~/.codex/sessions/`, each saying which directory it
//! was in. Listed as apex-acp's `Resume` lists them -- the id, when it
//! was last worked in, what it is about -- so B3 on an id opens the
//! transcript and `Resume ID` takes the session up again.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A session an agent has had here before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Past {
    pub id: String,
    /// `claude`, `codex`.
    pub kind: String,
    /// Its transcript.
    pub path: PathBuf,
    pub cwd: String,
    /// When it was last written, in seconds since the epoch.
    pub when: i64,
    pub title: Option<String>,
}

/// Claude Code's name for a directory: every character that is not a
/// letter or a digit becomes a `-`, the leading `/` included.
pub fn claude_project(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The sessions had in `cwd`, newest first: Claude Code's from
/// `claude_home` (`~/.claude`) and Codex's from `codex_home`
/// (`~/.codex`).
pub fn sessions(claude_home: &Path, codex_home: &Path, cwd: &str) -> Vec<Past> {
    let mut out = Vec::new();
    let dir = claude_home.join("projects").join(claude_project(cwd));
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let path = e.path();
            let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".jsonl")) else { continue };
            let Some((title, ok)) = claude_title(&path) else { continue };
            if !ok {
                continue;
            }
            out.push(Past { id: id.to_string(), kind: "claude".into(), path: path.clone(), cwd: cwd.to_string(), when: mtime(&path), title });
        }
    }
    let mut rollouts = Vec::new();
    walk(&codex_home.join("sessions"), &mut rollouts, 0);
    for path in rollouts {
        if let Some(p) = codex_meta(&path, cwd) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| b.when.cmp(&a.when).then(a.id.cmp(&b.id)));
    out
}

/// Every `rollout-*.jsonl` under `dir`, a few levels down.
fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                walk(&p, out, depth + 1);
            }
        } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl")) {
            out.push(p);
        }
    }
}

fn mtime(p: &Path) -> i64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// What a Claude session is about -- its title when it gave itself
/// one, else its first prompt -- and whether it had a conversation at
/// all (a file of bookkeeping alone is no session).
fn claude_title(path: &Path) -> Option<(Option<String>, bool)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut title = None;
    let mut first = None;
    let mut talked = false;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("ai-title") => title = v.get("aiTitle").and_then(Value::as_str).map(String::from),
            Some("user") if first.is_none() && v.get("isMeta").and_then(Value::as_bool) != Some(true) => {
                let t = match v.get("message").and_then(|m| m.get("content")) {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Array(a)) => a.iter().find_map(|b| b.get("text").and_then(Value::as_str)).map(String::from),
                    _ => None,
                };
                if let Some(t) = t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !t.starts_with('<') && !t.starts_with('[')) {
                    first = Some(t);
                    talked = true;
                }
            }
            Some("assistant") => talked = true,
            _ => {}
        }
    }
    Some((title.or(first), talked))
}

/// A Codex rollout's session, when it was had in `cwd`.
fn codex_meta(path: &Path, cwd: &str) -> Option<Past> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).ok()?;
    let mut lines = std::io::BufReader::new(f).lines();
    let first = lines.next()?.ok()?;
    let v: Value = serde_json::from_str(&first).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let p = v.get("payload")?;
    if p.get("cwd").and_then(Value::as_str) != Some(cwd) {
        return None;
    }
    let id = p.get("id").and_then(Value::as_str)?.to_string();
    // the first thing the user said, for a title
    let mut title = None;
    for line in lines.take(200).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(p) = v.get("payload") else { continue };
        if p.get("type").and_then(Value::as_str) == Some("message") && p.get("role").and_then(Value::as_str) == Some("user") {
            let t = p.get("content").and_then(Value::as_array).and_then(|a| a.iter().find_map(|b| b.get("text").and_then(Value::as_str))).unwrap_or("").trim();
            if !t.is_empty() && !t.starts_with('<') && !t.starts_with('#') {
                title = Some(t.to_string());
                break;
            }
        }
    }
    Some(Past { id, kind: "codex".into(), path: path.to_path_buf(), cwd: cwd.to_string(), when: mtime(path), title })
}

/// The sessions as a listing, apex-acp's way: the id, when, and what
/// it is about, a line each, with a first line of apex's own.
pub fn listing(dir: &str, past: &[Past], now: i64) -> String {
    if past.is_empty() {
        return format!("– no session has been had in {dir}\n");
    }
    let mut s = format!("– sessions in {dir}, newest first; B3 an id for its transcript, Resume ID to take it up\n\n");
    for p in past {
        let title = p.title.as_deref().map(crate::transcript::brief).unwrap_or_default();
        s.push_str(&format!("  {}  {:<10}  {}  {}\n", p.id, when(p.when, now), p.kind, title));
    }
    s
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A time at the resolution that tells it apart: the time of day
/// today, the weekday within the week, and the date beyond it. In this
/// machine's zone, since that is the one the day is being had in.
pub fn when(t: i64, now: i64) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let secs = t as libc::time_t;
    // SAFETY: tm is a plain struct localtime_r fills in
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return String::new();
    }
    let clock = format!("{}:{:02}{}", if tm.tm_hour % 12 == 0 { 12 } else { tm.tm_hour % 12 }, tm.tm_min, if tm.tm_hour < 12 { "AM" } else { "PM" });
    match now - t {
        d if d > 7 * 24 * 3600 => format!("{}{}{:02}", tm.tm_mday, MONTHS[(tm.tm_mon as usize).min(11)], (tm.tm_year + 1900).rem_euclid(100)),
        d if d > 24 * 3600 => format!("{}{clock}", DAYS[(tm.tm_wday as usize).min(6)]),
        _ => clock,
    }
}

/// Now, in seconds since the epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directorys_sessions_are_found_and_listed_newest_first() {
        assert_eq!(claude_project("/Users/me/src/apex"), "-Users-me-src-apex");
        assert_eq!(claude_project("/Users/me/src/cmd/.claude/worktrees/x-1"), "-Users-me-src-cmd--claude-worktrees-x-1");
        let home = std::env::temp_dir().join(format!("apex-agent-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let claude = home.join(".claude");
        let codex = home.join(".codex");
        let proj = claude.join("projects").join(claude_project("/work/proj"));
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("aaaa-1.jsonl"), "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"fix the build\"}}\n{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n{\"type\":\"ai-title\",\"aiTitle\":\"Build fix\"}\n").unwrap();
        std::fs::write(proj.join("bbbb-2.jsonl"), "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"<local-command-caveat>x</local-command-caveat>\"},\"isMeta\":true}\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"port the shell\"}]}}\n").unwrap();
        // bookkeeping alone is no session
        std::fs::write(proj.join("cccc-3.jsonl"), "{\"type\":\"cost-state\"}\n").unwrap();
        let day = codex.join("sessions/2026/09/14");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-2026-09-14T10-00-00-dddd-4.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"dddd-4\",\"cwd\":\"/work/proj\"}}\n{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<environment_context>x</environment_context>\"}]}}\n{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"run the tests\"}]}}\n").unwrap();
        std::fs::write(day.join("rollout-2026-09-14T11-00-00-eeee-5.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"eeee-5\",\"cwd\":\"/elsewhere\"}}\n").unwrap();
        let got = sessions(&claude, &codex, "/work/proj");
        let ids: Vec<&str> = got.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(got.len(), 3, "{ids:?}");
        assert!(ids.contains(&"aaaa-1") && ids.contains(&"bbbb-2") && ids.contains(&"dddd-4"), "{ids:?}");
        let by = |id: &str| got.iter().find(|p| p.id == id).unwrap();
        assert_eq!(by("aaaa-1").title.as_deref(), Some("Build fix"));
        assert_eq!(by("bbbb-2").title.as_deref(), Some("port the shell"));
        assert_eq!((by("dddd-4").kind.as_str(), by("dddd-4").title.as_deref()), ("codex", Some("run the tests")));
        let text = listing("/work/proj", &got, now());
        assert!(text.starts_with("– sessions in /work/proj"), "{text}");
        assert!(text.contains("  aaaa-1  "), "{text}");
        assert!(text.contains("  codex  run the tests\n"), "{text}");
        assert_eq!(listing("/x", &[], 0), "– no session has been had in /x\n");
        let _ = std::fs::remove_dir_all(&home);
    }
}
