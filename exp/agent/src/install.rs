//! `apex-agent install`: the hooks put where the agents read them.
//! Claude Code reads `~/.claude/settings.json`; Codex reads
//! `~/.codex/hooks.json`, in the same shape. Ours are the handlers whose
//! command is `apex-agent hook AGENT`, and they are known by that, so an
//! install over an install changes nothing, an install of a moved
//! binary replaces the old path, and `uninstall` takes ours away and
//! leaves everything else as it was.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The events Claude Code has that the pane cares about.
pub const CLAUDE_EVENTS: [&str; 15] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
];

/// Codex's, which are named the same where they are the same thing.
pub const CODEX_EVENTS: [&str; 12] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
    "Interrupt",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
];

pub const AGENTS: [&str; 2] = ["claude", "codex"];

/// Where an agent keeps its hooks, under `home`.
pub fn hooks_file(home: &Path, agent: &str) -> PathBuf {
    match agent {
        "claude" => home.join(".claude").join("settings.json"),
        _ => home.join(".codex").join("hooks.json"),
    }
}

fn events_of(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &CLAUDE_EVENTS,
        _ => &CODEX_EVENTS,
    }
}

/// The hook command: this binary, by its full path, so it is found
/// whatever the agent's PATH is.
pub fn command(exe: &Path, agent: &str) -> String {
    format!("{} hook {agent}", word(&exe.display().to_string()))
}

/// A shell word: quoted when it needs to be.
fn word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=+@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Whether a handler is one of ours, whatever path it was installed from.
fn ours(h: &Value) -> bool {
    h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains("apex-agent") && c.contains(" hook "))
}

/// Put our hooks into the agent's file (or take them out, with no
/// command). Everything else in the file stays as it was. Says whether
/// the file changed.
fn settle(file: &Path, events: &[&str], cmd: Option<&str>) -> Result<bool, String> {
    let before = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    let mut root: Value = match before.trim() {
        "" => json!({}),
        s => serde_json::from_str(s).map_err(|e| format!("{}: {e}", file.display()))?,
    };
    let obj = root.as_object_mut().ok_or_else(|| format!("{}: not a JSON object", file.display()))?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or_else(|| format!("{}: \"hooks\" is not an object", file.display()))?;
    // ours out of every event, whatever events an older install used
    for (_, groups) in hooks.iter_mut() {
        if let Some(groups) = groups.as_array_mut() {
            for g in groups.iter_mut() {
                if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    hs.retain(|h| !ours(h));
                }
            }
            groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_some_and(|hs| !hs.is_empty()));
        }
    }
    hooks.retain(|_, groups| groups.as_array().is_some_and(|g| !g.is_empty()));
    if let Some(cmd) = cmd {
        for ev in events {
            let groups = hooks.entry(*ev).or_insert_with(|| json!([]));
            if let Some(groups) = groups.as_array_mut() {
                // a question may wait on the pane for an answer; the
                // rest are a line written and done
                let timeout = if *ev == "PermissionRequest" { 120 } else { 5 };
                groups.push(json!({ "hooks": [{ "type": "command", "command": cmd, "timeout": timeout }] }));
            }
        }
    }
    if hooks.is_empty() {
        obj.remove("hooks");
    }
    let mut after = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    after.push('\n');
    if after == before {
        return Ok(false);
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = file.with_extension("json.apex-agent");
    std::fs::write(&tmp, after).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(true)
}

/// Install for each agent named; what was done, a line each.
pub fn install(home: &Path, exe: &Path, agents: &[&str]) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    for a in agents {
        let file = hooks_file(home, a);
        let changed = settle(&file, events_of(a), Some(&command(exe, a)))?;
        said.push(format!("{a}: {} {}", if changed { "hooks written to" } else { "hooks already in" }, file.display()));
        if *a == "codex" {
            said.push("codex: hooks are on by default in current versions; an older one wants `[features]` `codex_hooks = true` in ~/.codex/config.toml".to_string());
        }
    }
    Ok(said)
}

pub fn uninstall(home: &Path, agents: &[&str]) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    for a in agents {
        let file = hooks_file(home, a);
        let changed = settle(&file, events_of(a), None)?;
        said.push(format!("{a}: {} {}", if changed { "hooks taken out of" } else { "no hooks of ours in" }, file.display()));
    }
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys of a JSON object.
    fn keys(v: &Value) -> Vec<String> {
        v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
    }

    fn home() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("apex-agent-home-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_install_is_idempotent_and_leaves_the_rest_alone() {
        let home = home();
        let file = hooks_file(&home, "claude");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"theme":"auto","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#).unwrap();
        let exe = Path::new("/opt/apex agent/apex-agent");
        install(&home, exe, &["claude"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["theme"], "auto");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        assert_eq!(stop[1]["hooks"][0]["command"], "'/opt/apex agent/apex-agent' hook claude");
        assert_eq!(keys(&v["hooks"]).len(), CLAUDE_EVENTS.len());
        // again: nothing more
        let said = install(&home, exe, &["claude"]).unwrap();
        assert!(said[0].contains("already"), "{said:?}");
        let again = std::fs::read_to_string(&file).unwrap();
        // from elsewhere: the path is replaced, not added to
        install(&home, Path::new("/usr/local/bin/apex-agent"), &["claude"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], "/usr/local/bin/apex-agent hook claude");
        assert_ne!(again, std::fs::read_to_string(&file).unwrap());
        // out: theirs stays, ours goes, and the events we alone used go with us
        uninstall(&home, &["claude"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(keys(&v["hooks"]), vec!["Stop"]);
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_missing_file_is_made_and_an_empty_hooks_key_is_not_left() {
        let home = home();
        install(&home, Path::new("/bin/apex-agent"), &["codex"]).unwrap();
        let file = hooks_file(&home, "codex");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(keys(&v["hooks"]).len(), CODEX_EVENTS.len());
        assert_eq!(v["hooks"]["Interrupt"][0]["hooks"][0]["command"], "/bin/apex-agent hook codex");
        uninstall(&home, &["codex"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v, json!({}));
        let _ = std::fs::remove_dir_all(&home);
    }
}
