//! `apex tool agent install`: the hooks put where the agents read them.
//! Claude Code reads `~/.claude/settings.json`; Codex reads
//! `~/.codex/hooks.json`; Muse reads `~/.config/muse/settings.json`;
//! all in the same shape. Ours are the handlers whose
//! command is `apex tool agent hook AGENT`, and they are known by that, so an
//! install over an install changes nothing, an install of a moved
//! binary replaces the old path, and `uninstall` takes ours away and
//! leaves everything else as it was.
//!
//! pi has no hooks of that kind, only extensions: TypeScript it loads
//! from `~/.pi/agent/extensions/`. Ours is one file there, `apex.ts`,
//! which runs the same hook at each of pi's events with the event said
//! as Claude Code's hooks say theirs; it is ours by what it runs, and is
//! written whole, or taken away whole.

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

/// Muse's: the same shape as the others', but `PermissionDenied` is
/// not an event it has, and `Interrupt` wants a handler that runs
/// without waiting, which ours is not, so neither is installed.
pub const MUSE_EVENTS: [&str; 14] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
];

pub const AGENTS: [&str; 4] = ["claude", "codex", "muse", "pi"];

/// Where Muse reads its configuration: `$XDG_CONFIG_HOME`, else
/// `~/.config`, as Muse itself has it.
pub fn config_dir(home: &Path) -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME").ok().filter(|d| !d.trim().is_empty()) {
        Some(d) => PathBuf::from(d),
        None => home.join(".config"),
    }
}

/// Where pi keeps what is its own: `$PI_CODING_AGENT_DIR`, else
/// `~/.pi/agent`, as pi itself has it.
pub fn pi_dir(home: &Path) -> PathBuf {
    match std::env::var("PI_CODING_AGENT_DIR").ok().filter(|d| !d.trim().is_empty()) {
        Some(d) if d == "~" => home.to_path_buf(),
        Some(d) => match d.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(d),
        },
        None => home.join(".pi").join("agent"),
    }
}

/// Where an agent keeps its hooks, under `home`.
pub fn hooks_file(home: &Path, agent: &str) -> PathBuf {
    match agent {
        "claude" => home.join(".claude").join("settings.json"),
        "muse" => config_dir(home).join("muse").join("settings.json"),
        "pi" => pi_dir(home).join("extensions").join("apex.ts"),
        _ => home.join(".codex").join("hooks.json"),
    }
}

fn events_of(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &CLAUDE_EVENTS,
        "muse" => &MUSE_EVENTS,
        _ => &CODEX_EVENTS,
    }
}

/// apex as it was invoked, not as it resolves: a path as given
/// (made absolute), a bare name as PATH finds it -- the first match,
/// symlinks and all left in place. A launcher that runs a binary from a
/// cache (dotslash, a version manager) keeps the launcher's path, so
/// the hooks outlive the cache's next version. None when it cannot say.
pub fn invoked_as(argv0: Option<std::ffi::OsString>, cwd: Option<PathBuf>, path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let a = PathBuf::from(argv0?);
    if a.as_os_str().is_empty() {
        return None;
    }
    if a.components().count() > 1 || a.is_absolute() {
        return Some(if a.is_absolute() { a } else { cwd?.join(a) });
    }
    std::env::split_paths(&path?).map(|d| d.join(&a)).find(|p| p.is_file())
}

/// The hook command: `apex tool agent hook`, apex by its full path as
/// invoked, so it is found whatever the agent's PATH is.
pub fn command(exe: &Path, agent: &str) -> String {
    format!("{} tool agent hook {agent}", word(&exe.display().to_string()))
}

/// A shell word: quoted when it needs to be.
fn word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=+@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Whether a handler is one of ours, whatever path it was installed
/// from: `apex tool agent hook`, or the `apex tool agent hook` of before.
fn ours(h: &Value) -> bool {
    h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(" tool agent hook ") || (c.contains("apex-agent") && c.contains(" hook ")))
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
    let tmp = file.with_extension("json.apex-tool-agent");
    std::fs::write(&tmp, after).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(true)
}

/// pi's extension: its events, said as Claude Code's hooks say theirs,
/// handed to the hook one at a time and each waited for, so the log has
/// them in the order they happened. pi asks no permissions, so there is
/// no question for the pane to answer; and a hook that fails is let be.
const PI_EXTENSION: &str = r#"// apex tool agent's hooks for pi: each event of pi's that the pane
// cares about, said as Claude Code's hooks say theirs, handed to
// `apex tool agent hook pi`. Written by `apex tool agent install pi`;
// `apex tool agent uninstall pi` takes it away.
import { spawn } from "node:child_process";

const APEX = @APEX@;

function hook(ctx: any, ev: Record<string, unknown>): Promise<void> {
  const sm = ctx.sessionManager;
  const body = { session_id: sm.getSessionId(), transcript_path: sm.getSessionFile() ?? null, cwd: ctx.cwd, ...ev };
  return new Promise((resolve) => {
    let child: any;
    try {
      child = spawn(APEX, ["tool", "agent", "hook", "pi"], { stdio: ["pipe", "ignore", "ignore"] });
    } catch {
      return resolve();
    }
    const timer = setTimeout(() => child.kill(), 5000);
    const done = () => {
      clearTimeout(timer);
      resolve();
    };
    child.on("error", done);
    child.on("close", done);
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify(body));
  });
}

function text(content: any): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.filter((b: any) => b?.type === "text").map((b: any) => b.text).join("\n");
}

export default function (pi: any) {
  pi.on("session_start", (e: any, ctx: any) => hook(ctx, { hook_event_name: "SessionStart", source: e.reason }));
  pi.on("session_shutdown", (e: any, ctx: any) => {
    // a reload is the same session going on
    if (e.reason === "reload") return;
    return hook(ctx, { hook_event_name: "SessionEnd", reason: e.reason });
  });
  pi.on("before_agent_start", (e: any, ctx: any) => hook(ctx, { hook_event_name: "UserPromptSubmit", prompt: e.prompt }));
  pi.on("tool_execution_start", (e: any, ctx: any) =>
    hook(ctx, { hook_event_name: "PreToolUse", tool_name: e.toolName, tool_input: e.args, tool_use_id: e.toolCallId }));
  pi.on("tool_execution_end", (e: any, ctx: any) =>
    hook(ctx, { hook_event_name: e.isError ? "PostToolUseFailure" : "PostToolUse", tool_name: e.toolName, tool_use_id: e.toolCallId }));
  pi.on("agent_end", (e: any, ctx: any) => {
    const last = [...(e.messages ?? [])].reverse().find((m: any) => m?.role === "assistant");
    if (last?.stopReason === "error") return hook(ctx, { hook_event_name: "StopFailure", message: last.errorMessage ?? "error" });
    if (last?.stopReason === "aborted") return hook(ctx, { hook_event_name: "Interrupt" });
    return hook(ctx, { hook_event_name: "Stop", last_assistant_message: text(last?.content) });
  });
  pi.on("session_before_compact", (_e: any, ctx: any) => hook(ctx, { hook_event_name: "PreCompact" }));
  pi.on("session_compact", (_e: any, ctx: any) => hook(ctx, { hook_event_name: "PostCompact" }));
}
"#;

/// pi's extension, running apex from `exe`.
fn pi_extension(exe: &Path) -> String {
    PI_EXTENSION.replace("@APEX@", &Value::String(exe.display().to_string()).to_string())
}

/// Put our extension in pi's (or take it out, with none). A file there
/// that is not ours is not touched. Says whether anything changed.
fn settle_pi(file: &Path, ext: Option<&str>) -> Result<bool, String> {
    let before = match std::fs::read_to_string(file) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    if before.as_deref().is_some_and(|b| !b.contains(r#""tool", "agent", "hook", "pi""#)) {
        return Err(format!("{}: there already, and not ours", file.display()));
    }
    match ext {
        Some(ext) if before.as_deref() == Some(ext) => Ok(false),
        Some(ext) => {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let tmp = file.with_extension("ts.apex-tool-agent");
            std::fs::write(&tmp, ext).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))?;
            Ok(true)
        }
        None if before.is_none() => Ok(false),
        None => std::fs::remove_file(file).map(|_| true).map_err(|e| format!("{}: {e}", file.display())),
    }
}

/// Install for each agent named; what was done, a line each.
pub fn install(home: &Path, exe: &Path, agents: &[&str]) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    for a in agents {
        let file = hooks_file(home, a);
        let changed = match *a {
            "pi" => settle_pi(&file, Some(&pi_extension(exe)))?,
            _ => settle(&file, events_of(a), Some(&command(exe, a)))?,
        };
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
        let changed = match *a {
            "pi" => settle_pi(&file, None)?,
            _ => settle(&file, events_of(a), None)?,
        };
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
        let d = std::env::temp_dir().join(format!("apex-tool-agent-home-{}-{n}", std::process::id()));
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
        let exe = Path::new("/opt/apex app/apex");
        install(&home, exe, &["claude"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["theme"], "auto");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        assert_eq!(stop[1]["hooks"][0]["command"], "'/opt/apex app/apex' tool agent hook claude");
        assert_eq!(keys(&v["hooks"]).len(), CLAUDE_EVENTS.len());
        // again: nothing more
        let said = install(&home, exe, &["claude"]).unwrap();
        assert!(said[0].contains("already"), "{said:?}");
        let again = std::fs::read_to_string(&file).unwrap();
        // from elsewhere: the path is replaced, not added to
        install(&home, Path::new("/usr/local/bin/apex"), &["claude"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], "/usr/local/bin/apex tool agent hook claude");
        assert_ne!(again, std::fs::read_to_string(&file).unwrap());
        // out: theirs stays, ours goes, and the events we alone used go with us
        uninstall(&home, &["claude"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(keys(&v["hooks"]), vec!["Stop"]);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_hooks_of_apex_agent_before_it_was_apex_tool_agent_are_replaced() {
        let home = home();
        let file = hooks_file(&home, "claude");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/bin/apex-agent hook claude"}]}]}}"#).unwrap();
        install(&home, Path::new("/bin/apex"), &["claude"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "/bin/apex tool agent hook claude");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_missing_file_is_made_and_an_empty_hooks_key_is_not_left() {
        let home = home();
        install(&home, Path::new("/bin/apex"), &["codex"]).unwrap();
        let file = hooks_file(&home, "codex");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(keys(&v["hooks"]).len(), CODEX_EVENTS.len());
        assert_eq!(v["hooks"]["Interrupt"][0]["hooks"][0]["command"], "/bin/apex tool agent hook codex");
        uninstall(&home, &["codex"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v, json!({}));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn muse_hooks_go_in_its_settings_wherever_it_reads_them() {
        let home = home();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", home.join("xdg"));
        // under $XDG_CONFIG_HOME when it says
        assert_eq!(hooks_file(&home, "muse"), home.join("xdg").join("muse").join("settings.json"));
        install(&home, Path::new("/bin/apex"), &["muse"]).unwrap();
        let file = home.join("xdg").join("muse").join("settings.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(keys(&v["hooks"]).len(), MUSE_EVENTS.len());
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "/bin/apex tool agent hook muse");
        // the question may wait on the pane; the rest are a line and done
        assert_eq!(v["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"], 120);
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 5);
        assert!(v["hooks"].get("PermissionDenied").is_none());
        assert!(v["hooks"].get("Interrupt").is_none());
        uninstall(&home, &["muse"]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v, json!({}));
        // under ~/.config when it says nothing
        std::env::remove_var("XDG_CONFIG_HOME");
        assert_eq!(hooks_file(&home, "muse"), home.join(".config").join("muse").join("settings.json"));
        match prev {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn pis_hooks_are_an_extension_of_ours_and_only_ours() {
        let home = home();
        let prev = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::remove_var("PI_CODING_AGENT_DIR");
        let file = hooks_file(&home, "pi");
        assert_eq!(file, home.join(".pi/agent/extensions/apex.ts"));
        let said = install(&home, Path::new("/opt/apex app/apex"), &["pi"]).unwrap();
        assert!(said[0].contains("written"), "{said:?}");
        let ext = std::fs::read_to_string(&file).unwrap();
        assert!(ext.contains("const APEX = \"/opt/apex app/apex\";"), "{ext}");
        assert!(!ext.contains("@APEX@"));
        // again: nothing more; from elsewhere: the path is replaced
        assert!(install(&home, Path::new("/opt/apex app/apex"), &["pi"]).unwrap()[0].contains("already"));
        install(&home, Path::new("/bin/apex"), &["pi"]).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("const APEX = \"/bin/apex\";"));
        // out, and out again is nothing
        assert!(uninstall(&home, &["pi"]).unwrap()[0].contains("taken out"));
        assert!(!file.exists());
        assert!(uninstall(&home, &["pi"]).unwrap()[0].contains("no hooks"));
        // a file of that name that is not ours is left alone
        std::fs::write(&file, "export default function () {}\n").unwrap();
        assert!(install(&home, Path::new("/bin/apex"), &["pi"]).is_err());
        assert!(uninstall(&home, &["pi"]).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "export default function () {}\n");
        // wherever pi is told its directory is
        std::env::set_var("PI_CODING_AGENT_DIR", "~/elsewhere");
        assert_eq!(hooks_file(&home, "pi"), home.join("elsewhere/extensions/apex.ts"));
        match prev {
            Some(p) => std::env::set_var("PI_CODING_AGENT_DIR", p),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn apex_is_named_as_invoked_not_as_it_resolves() {
        use std::ffi::OsString;
        let tmp = std::env::temp_dir().join(format!("apex-tool-agent-invoked-{}", std::process::id()));
        let bin = tmp.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("apex"), "#!/bin/sh\n").unwrap();
        // a bare name: where PATH finds it, not where that leads
        let path = std::env::join_paths([tmp.join("none"), bin.clone()]).unwrap();
        assert_eq!(invoked_as(Some(OsString::from("apex")), None, Some(path.clone())), Some(bin.join("apex")));
        assert_eq!(invoked_as(Some(OsString::from("nowhere")), None, Some(path)), None);
        // a path: as given, made absolute against where it was run
        assert_eq!(invoked_as(Some(OsString::from("/opt/x/apex")), None, None), Some(PathBuf::from("/opt/x/apex")));
        assert_eq!(invoked_as(Some(OsString::from("bin/apex")), Some(tmp.clone()), None), Some(tmp.join("bin/apex")));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
