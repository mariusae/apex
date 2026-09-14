//! The transcript: what an agent's own record of the session says,
//! read as it is written, and put into words the way apex-acp's
//! transcript window puts them -- `~` where a prompt begins, `•` where
//! the agent speaks, a line a tool call with its status ticked off in
//! place, the files it touched as `path:line` for B3, an edit as its
//! diff, a result cut to a dozen lines. Claude Code keeps a JSONL of
//! its own shape and Codex another; each is read into the same items,
//! and one writer renders them.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

/// One thing the transcript says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A prompt: what the user sent.
    User(String),
    /// A message of the agent's.
    Agent(String),
    /// Its thinking, when shown.
    Thought(String),
    /// A tool call: its id (what the result names), its title in one
    /// line, and what more there is to say of it, a line each.
    Call { id: String, title: String, detail: Vec<String> },
    /// The tool's answer.
    Result { id: String, ok: bool, text: String },
    /// Apex's own remark.
    Note(String),
}

// ---- words for a tool call ---------------------------------------------------

/// A path as shown: relative to the session's directory when under it.
pub fn shown(path: &str, cwd: &str) -> String {
    let cwd = cwd.trim_end_matches('/');
    if !cwd.is_empty() {
        if let Some(rest) = path.strip_prefix(cwd) {
            if let Some(rest) = rest.strip_prefix('/') {
                return rest.to_string();
            }
        }
    }
    path.to_string()
}

/// What a call is called, in one line and a short one: the agent's own
/// words for it when it gave some (Claude's `description`), else the
/// thing itself -- the command, the file, the pattern. The pane shows
/// this; the transcript shows it and the detail under it.
pub fn call_title(tool: &str, input: &Value, cwd: &str) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let path = |k: &str| s(k).map(|p| shown(p, cwd));
    let words = match tool {
        "Bash" => s("description").map(String::from).or_else(|| s("command").map(String::from)),
        "Edit" | "Write" | "Read" | "NotebookEdit" | "MultiEdit" => path("file_path").or_else(|| path("notebook_path")),
        "Glob" => s("pattern").map(|p| match path("path") {
            Some(d) => format!("{p} in {d}"),
            None => p.to_string(),
        }),
        "Grep" => s("pattern").map(|p| match path("path") {
            Some(d) => format!("{p} in {d}"),
            None => p.to_string(),
        }),
        "Agent" | "Task" => s("description").map(String::from).or_else(|| s("subagent_type").map(String::from)),
        "WebFetch" => s("url").map(String::from),
        "WebSearch" => s("query").map(String::from),
        "Skill" => s("skill").map(String::from),
        "TodoWrite" | "update_plan" => Some("plan".to_string()),
        "shell" | "shell_command" | "exec_command" | "local_shell" | "container.exec" => command_of(input),
        "apply_patch" => s("patch").or_else(|| s("input")).map(patched_files),
        _ => ["description", "command", "cmd", "file_path", "path", "pattern", "query", "prompt", "url", "name"].iter().find_map(|k| s(k)).map(String::from).or_else(|| command_of(input)),
    };
    let words = words.unwrap_or_default();
    let line = words.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        tool.to_string()
    } else {
        format!("{tool}: {line}")
    }
}

/// A command as Codex gives one: a list of words, or a string.
fn command_of(input: &Value) -> Option<String> {
    match input.get("command").or_else(|| input.get("cmd")) {
        Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// The files an `apply_patch` patch touches.
fn patched_files(patch: &str) -> String {
    let files: Vec<&str> = patch
        .lines()
        .filter_map(|l| ["*** Update File: ", "*** Add File: ", "*** Delete File: "].iter().find_map(|h| l.strip_prefix(h)))
        .map(str::trim)
        .collect();
    files.join(", ")
}

/// What more the transcript says of a call, under its title: the whole
/// command where the title was the description of it, an edit as its
/// diff, a write as its size, a plan as its steps.
pub fn call_detail(tool: &str, input: &Value, cwd: &str) -> Vec<String> {
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    let mut out = Vec::new();
    match tool {
        "Bash" => {
            if let (Some(_), Some(cmd)) = (s("description"), s("command")) {
                out.extend(cmd.lines().map(String::from));
            } else if let Some(cmd) = s("command") {
                out.extend(cmd.lines().skip(1).map(String::from));
            }
        }
        "Edit" => {
            let (lines, cut) = hunk(s("old_string").unwrap_or(""), s("new_string").unwrap_or(""));
            let (minus, plus) = (lines.iter().filter(|l| l.starts_with('-')).count(), lines.iter().filter(|l| l.starts_with('+')).count());
            out.push(format!("+{plus} -{minus}"));
            out.extend(lines);
            if cut > 0 {
                out.push(format!("… {cut} more lines"));
            }
        }
        "Write" => {
            let n = s("content").map(|c| c.lines().count()).unwrap_or(0);
            out.push(format!("+{n} lines"));
        }
        "TodoWrite" => {
            if let Some(todos) = input.get("todos").and_then(Value::as_array) {
                for t in todos {
                    let box_ = match t.get("status").and_then(Value::as_str) {
                        Some("completed") => "[x]",
                        Some("in_progress") => "[~]",
                        _ => "[ ]",
                    };
                    out.push(format!("{box_} {}", t.get("content").and_then(Value::as_str).unwrap_or("")));
                }
            }
        }
        "update_plan" => {
            if let Some(plan) = input.get("plan").and_then(Value::as_array) {
                for t in plan {
                    let box_ = match t.get("status").and_then(Value::as_str) {
                        Some("completed") => "[x]",
                        Some("in_progress") => "[~]",
                        _ => "[ ]",
                    };
                    out.push(format!("{box_} {}", t.get("step").and_then(Value::as_str).unwrap_or("")));
                }
            }
        }
        "apply_patch" => {
            const KEEP: usize = 24;
            let patch = s("patch").or_else(|| s("input")).unwrap_or("");
            let lines: Vec<&str> = patch.lines().filter(|l| !l.starts_with("*** Begin Patch") && !l.starts_with("*** End Patch")).collect();
            let shown = lines.len().min(KEEP);
            out.extend(lines[..shown].iter().map(|l| l.to_string()));
            if lines.len() > shown {
                out.push(format!("… {} more lines", lines.len() - shown));
            }
        }
        "Agent" | "Task" => {
            if let Some(p) = s("prompt") {
                out.extend(p.lines().take(6).map(String::from));
            }
        }
        _ => {
            if let Some(cmd) = command_of(input) {
                let first = cmd.lines().next().unwrap_or("");
                if cmd.lines().nth(1).is_some() || first.chars().count() > 76 {
                    out.extend(cmd.lines().map(String::from));
                }
            }
        }
    }
    let _ = cwd;
    out
}

/// A change as lines: what went and what came, the unchanged head and
/// tail of each trimmed off, cut at a dozen lines a side.
pub fn hunk(old: &str, new: &str) -> (Vec<String>, usize) {
    const KEEP: usize = 12;
    let (o, n): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let mut a = 0;
    while a < o.len() && a < n.len() && o[a] == n[a] {
        a += 1;
    }
    let mut b = 0;
    while a + b < o.len() && a + b < n.len() && o[o.len() - 1 - b] == n[n.len() - 1 - b] {
        b += 1;
    }
    let (gone, come) = (&o[a..o.len() - b], &n[a..n.len() - b]);
    let mut lines = Vec::new();
    let mut cut = 0;
    for (mark, part) in [('-', gone), ('+', come)] {
        let shown = part.len().min(KEEP);
        for l in &part[..shown] {
            lines.push(format!("{mark}{l}"));
        }
        cut += part.len() - shown;
    }
    (lines, cut)
}

/// One line and a short one: the first line of `title`, cut at 76
/// characters with a `…` where the rest was.
pub fn brief(title: &str) -> String {
    const KEEP: usize = 76;
    let line = title.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() <= KEEP && title.trim().lines().nth(1).is_none() {
        return line.to_string();
    }
    let cut: String = line.chars().take(KEEP).collect();
    format!("{}…", cut.trim_end())
}

// ---- writing it --------------------------------------------------------------

/// A change to the window: text at the end, or a glyph written over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Append(String),
    Glyph { at: usize, glyph: &'static str },
}

/// The text of a transcript, and what it remembers of it: the length
/// in characters (what the window counts in), the offsets of the status
/// glyphs by call id, and whether it stands at the start of a line.
#[derive(Default)]
pub struct Writer {
    pub text: String,
    pub len: usize,
    col0: bool,
    blank: bool,
    anchors: HashMap<String, usize>,
    /// A glyph for a call not seen yet: hooks can speak of a call
    /// before its line is in the transcript.
    pending: HashMap<String, &'static str>,
}

/// Results are cut to this many lines.
const RESULT_LINES: usize = 12;

impl Writer {
    pub fn new() -> Writer {
        Writer { col0: true, blank: true, ..Writer::default() }
    }

    fn put(&mut self, s: &str, ops: &mut Vec<Op>) {
        if s.is_empty() {
            return;
        }
        self.text.push_str(s);
        self.len += s.chars().count();
        self.blank = s.ends_with("\n\n") || (s == "\n" && self.col0);
        self.col0 = s.ends_with('\n');
        match ops.last_mut() {
            Some(Op::Append(t)) => t.push_str(s),
            _ => ops.push(Op::Append(s.to_string())),
        }
    }

    fn line_start(&mut self, ops: &mut Vec<Op>) {
        if !self.col0 {
            self.put("\n", ops);
        }
    }

    fn blank_line(&mut self, ops: &mut Vec<Op>) {
        self.line_start(ops);
        if !self.blank {
            self.put("\n", ops);
        }
    }

    /// Each line begun by `prefix`.
    fn prefixed(&mut self, prefix: &str, s: &str, ops: &mut Vec<Op>) {
        let mut buf = String::new();
        let mut col0 = self.col0;
        for c in s.chars() {
            if col0 {
                buf.push_str(prefix);
            }
            buf.push(c);
            col0 = c == '\n';
        }
        self.put(&buf, ops);
    }

    /// Say one item: the ops that put it in the window.
    pub fn item(&mut self, it: &Item) -> Vec<Op> {
        let mut ops = Vec::new();
        match it {
            Item::User(t) => {
                // a turn ends where the next prompt begins: set off on
                // both sides, as the session's window has it
                self.blank_line(&mut ops);
                self.put("~\n\n", &mut ops);
                self.put(t.trim_end(), &mut ops);
                self.put("\n\n", &mut ops);
            }
            Item::Agent(t) => {
                self.line_start(&mut ops);
                self.put("• ", &mut ops);
                self.put(t.trim(), &mut ops);
                self.put("\n", &mut ops);
            }
            Item::Thought(t) => {
                self.line_start(&mut ops);
                self.prefixed("  · ", t.trim(), &mut ops);
                self.put("\n", &mut ops);
            }
            Item::Call { id, title, detail } => {
                self.line_start(&mut ops);
                let glyph = self.pending.remove(id).unwrap_or("⋯");
                self.anchors.insert(id.clone(), self.len);
                self.put(&format!("{glyph} {title}\n"), &mut ops);
                for l in detail {
                    self.put(&format!("    {l}\n"), &mut ops);
                }
            }
            Item::Result { id, ok, text } => {
                if let Some(op) = self.glyph(id, if *ok { "✓" } else { "✗" }) {
                    ops.push(op);
                }
                let lines: Vec<&str> = text.lines().collect();
                let shown = lines.len().min(RESULT_LINES);
                if shown > 0 {
                    self.line_start(&mut ops);
                }
                for l in &lines[..shown] {
                    self.put(&format!("    {l}\n"), &mut ops);
                }
                if lines.len() > shown {
                    self.put(&format!("    … {} more lines\n", lines.len() - shown), &mut ops);
                }
            }
            Item::Note(t) => {
                self.line_start(&mut ops);
                self.put(&format!("– {t}\n"), &mut ops);
            }
        }
        ops
    }

    /// A call's status glyph, written over in place; kept for the call
    /// when its line has not been written yet.
    pub fn glyph(&mut self, id: &str, glyph: &'static str) -> Option<Op> {
        match self.anchors.get(id) {
            Some(&at) => {
                let b = byte_at(&self.text, at);
                let old = self.text[b..].chars().next().map(char::len_utf8).unwrap_or(0);
                self.text.replace_range(b..b + old, glyph);
                Some(Op::Glyph { at, glyph })
            }
            None => {
                self.pending.insert(id.to_string(), glyph);
                None
            }
        }
    }

    /// Someone else replaced `nd` characters at `q0` with `ni`: what we
    /// remember moves along.
    pub fn shift(&mut self, q0: usize, nd: usize, ni: usize) {
        let q1 = q0 + nd;
        self.len = (self.len + ni).saturating_sub(nd);
        self.anchors.retain(|_, p| {
            if *p < q0 {
                true
            } else if *p >= q1 {
                *p = *p + ni - nd;
                true
            } else {
                false
            }
        });
    }
}

fn byte_at(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map(|(i, _)| i).unwrap_or(s.len())
}

// ---- reading it --------------------------------------------------------------

/// A reader of one agent's transcript format.
pub trait Parser: Send {
    /// The items one line of the file says.
    fn line(&mut self, line: &str) -> Vec<Item>;
}

pub fn parser(kind: &str, cwd: &str, thoughts: bool) -> Box<dyn Parser> {
    match kind {
        "codex" => Box::new(Codex { cwd: cwd.to_string(), thoughts }),
        _ => Box::new(Claude { cwd: cwd.to_string(), thoughts }),
    }
}

/// Claude Code's `~/.claude/projects/PROJ/SESSION.jsonl`: a line an
/// entry, `type` `user`, `assistant` or `system` with a `message` whose
/// content is a string or blocks (`text`, `thinking`, `tool_use`,
/// `tool_result`), and a good many entries of other types that are its
/// own bookkeeping. Subagents' entries (`isSidechain`) are theirs.
pub struct Claude {
    cwd: String,
    thoughts: bool,
}

impl Parser for Claude {
    fn line(&mut self, line: &str) -> Vec<Item> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return Vec::new();
        }
        let mut out = Vec::new();
        match v.get("type").and_then(Value::as_str) {
            Some("user") => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    return out;
                }
                match v.get("message").and_then(|m| m.get("content")) {
                    Some(Value::String(s)) => {
                        if let Some(t) = user_text(s) {
                            out.push(Item::User(t));
                        }
                    }
                    Some(Value::Array(blocks)) => {
                        for b in blocks {
                            match b.get("type").and_then(Value::as_str) {
                                Some("text") => {
                                    if let Some(t) = b.get("text").and_then(Value::as_str).and_then(user_text) {
                                        out.push(Item::User(t));
                                    }
                                }
                                Some("tool_result") => {
                                    let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string();
                                    let ok = b.get("is_error").and_then(Value::as_bool) != Some(true);
                                    out.push(Item::Result { id, ok, text: blocks_text(b.get("content")) });
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                let Some(blocks) = v.get("message").and_then(|m| m.get("content")).and_then(Value::as_array) else { return out };
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                                out.push(Item::Agent(t.to_string()));
                            }
                        }
                        Some("thinking") if self.thoughts => {
                            if let Some(t) = b.get("thinking").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                                out.push(Item::Thought(t.to_string()));
                            }
                        }
                        Some("tool_use") => {
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            out.push(Item::Call { id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(), title: call_title(name, &input, &self.cwd), detail: call_detail(name, &input, &self.cwd) });
                        }
                        _ => {}
                    }
                }
            }
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("api_error") => {
                let e = v.get("error");
                let what = e.and_then(|e| e.get("formatted").or_else(|| e.get("message"))).and_then(Value::as_str).unwrap_or("API error");
                out.push(Item::Note(format!("error: {what}")));
            }
            _ => {}
        }
        out
    }
}

/// The user's words with the machinery Claude Code wraps around them
/// taken off: the reminders it appends, the caveats it prepends, the
/// commands and images it stands in for. None when nothing is left.
fn user_text(s: &str) -> Option<String> {
    let t = s.trim();
    for skip in ["<local-command-stdout>", "<local-command-caveat>", "<command-name>", "<command-message>", "[Image", "<bash-input>", "<bash-stdout>", "<bash-stderr>", "<task-notification>", "<ci-monitor-event>"] {
        if t.starts_with(skip) {
            return None;
        }
    }
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find("<system-reminder>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</system-reminder>") {
            Some(j) => rest = &rest[i + j + "</system-reminder>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    let out = out.trim();
    if out.is_empty() {
        None
    } else {
        Some(out.to_string())
    }
}

/// The text of a result: a string, or blocks of text.
fn blocks_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Codex's `~/.codex/sessions/.../rollout-*.jsonl`: a line an entry
/// with a `type` and a `payload`. The `response_item`s are the
/// conversation as the model saw it -- messages, reasoning, function
/// calls and their outputs -- and the rest (`event_msg`,
/// `turn_context`, `session_meta`) says the same things again or
/// nothing the transcript wants.
pub struct Codex {
    cwd: String,
    thoughts: bool,
}

impl Parser for Codex {
    fn line(&mut self, line: &str) -> Vec<Item> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        let mut out = Vec::new();
        if v.get("type").and_then(Value::as_str) != Some("response_item") {
            return out;
        }
        let Some(p) = v.get("payload") else { return out };
        let s = |k: &str| p.get(k).and_then(Value::as_str);
        match s("type") {
            Some("message") => {
                let texts: Vec<&str> = p.get("content").and_then(Value::as_array).map(|a| a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect()).unwrap_or_default();
                let text = texts.join("\n");
                match s("role") {
                    Some("user") => {
                        if let Some(t) = codex_user_text(&text) {
                            out.push(Item::User(t));
                        }
                    }
                    Some("assistant") if !text.trim().is_empty() => out.push(Item::Agent(text)),
                    _ => {}
                }
            }
            Some("reasoning") if self.thoughts => {
                let texts: Vec<&str> = p.get("summary").and_then(Value::as_array).map(|a| a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect()).unwrap_or_default();
                let text = texts.join("\n");
                if !text.trim().is_empty() {
                    out.push(Item::Thought(text));
                }
            }
            Some("function_call") => {
                let name = s("name").unwrap_or("tool");
                let args: Value = s("arguments").and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
                out.push(Item::Call { id: s("call_id").unwrap_or("").to_string(), title: call_title(name, &args, &self.cwd), detail: call_detail(name, &args, &self.cwd) });
            }
            Some("custom_tool_call") => {
                let name = s("name").unwrap_or("tool");
                let args = serde_json::json!({ "input": s("input").unwrap_or("") });
                out.push(Item::Call { id: s("call_id").unwrap_or("").to_string(), title: call_title(name, &args, &self.cwd), detail: call_detail(name, &args, &self.cwd) });
            }
            Some("local_shell_call") => {
                let args = p.get("action").cloned().unwrap_or(Value::Null);
                let id = s("call_id").or_else(|| s("id")).unwrap_or("").to_string();
                out.push(Item::Call { id, title: call_title("shell", &args, &self.cwd), detail: call_detail("shell", &args, &self.cwd) });
            }
            Some("function_call_output") | Some("custom_tool_call_output") => {
                let raw = s("output").unwrap_or("");
                // a shell's output comes wrapped, with how it ended
                let (ok, text) = match serde_json::from_str::<Value>(raw) {
                    Ok(o) if o.is_object() => {
                        let code = o.get("metadata").and_then(|m| m.get("exit_code")).and_then(Value::as_i64);
                        (code.map_or(true, |c| c == 0), o.get("output").and_then(Value::as_str).unwrap_or(raw).to_string())
                    }
                    _ => (true, raw.to_string()),
                };
                out.push(Item::Result { id: s("call_id").unwrap_or("").to_string(), ok, text });
            }
            _ => {}
        }
        out
    }
}

/// Codex puts its own instructions and the environment before the
/// user in messages of the user's role.
fn codex_user_text(s: &str) -> Option<String> {
    let t = s.trim();
    for skip in ["<environment_context>", "<user_instructions>", "<permissions instructions>", "# AGENTS.md", "<turn_aborted>", "<INSTRUCTIONS>", "<system_message>"] {
        if t.starts_with(skip) {
            return None;
        }
    }
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Where the transcript of an agent's session is: what its hooks said,
/// as a path.
pub fn transcript_path(p: Option<&str>) -> Option<&Path> {
    p.filter(|p| !p.is_empty()).map(Path::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_titled_in_the_agents_words_or_by_the_thing_itself() {
        let cwd = "/home/me/proj";
        assert_eq!(call_title("Bash", &serde_json::json!({"command": "ls\npwd", "description": "List"}), cwd), "Bash: List");
        assert_eq!(call_title("Bash", &serde_json::json!({"command": "ls -la\npwd"}), cwd), "Bash: ls -la");
        assert_eq!(call_title("Edit", &serde_json::json!({"file_path": "/home/me/proj/src/a.rs"}), cwd), "Edit: src/a.rs");
        assert_eq!(call_title("Read", &serde_json::json!({"file_path": "/etc/hosts"}), cwd), "Read: /etc/hosts");
        assert_eq!(call_title("Grep", &serde_json::json!({"pattern": "fn main", "path": "/home/me/proj/src"}), cwd), "Grep: fn main in src");
        assert_eq!(call_title("shell", &serde_json::json!({"command": ["bash", "-lc", "cargo test"]}), cwd), "shell: bash -lc cargo test");
        assert_eq!(call_title("apply_patch", &serde_json::json!({"patch": "*** Begin Patch\n*** Update File: a.rs\n-x\n+y\n*** Add File: b.rs\n+z\n*** End Patch"}), cwd), "apply_patch: a.rs, b.rs");
        assert_eq!(call_title("mcp__x__y", &serde_json::json!({"query": "q"}), cwd), "mcp__x__y: q");
        assert_eq!(call_title("Weird", &serde_json::json!({}), cwd), "Weird");
        let d = call_detail("Edit", &serde_json::json!({"old_string": "a\nb\nc", "new_string": "a\nB\nc"}), cwd);
        assert_eq!(d, vec!["+1 -1", "-b", "+B"]);
        let d = call_detail("Bash", &serde_json::json!({"command": "ls\npwd", "description": "List"}), cwd);
        assert_eq!(d, vec!["ls", "pwd"]);
        assert_eq!(call_detail("Bash", &serde_json::json!({"command": "ls"}), cwd), Vec::<String>::new());
    }

    #[test]
    fn a_title_is_one_line_and_a_short_one() {
        assert_eq!(brief("hello"), "hello");
        assert_eq!(brief("one\ntwo"), "one…");
        assert_eq!(brief("\n\nfirst real line\nmore"), "first real line…");
        let long = "x".repeat(100);
        assert_eq!(brief(&long).chars().count(), 77);
    }

    #[test]
    fn a_claude_transcript_reads_as_the_conversation() {
        let mut p = Claude { cwd: "/home/me/proj".into(), thoughts: false };
        let mut w = Writer::new();
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"what is in hosts?\n\n<system-reminder>\nsecret\n</system-reminder>"},"uuid":"u1"}"#,
            r#"{"type":"attachment","attachment":{"type":"environment"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll take a look."}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/etc/hosts"}}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"a subagent's"}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"127.0.0.1 localhost\n::1 localhost","is_error":false}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"It names localhost.\n"}]}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Continue from where you left off."}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"thanks"}]}}"#,
        ];
        let mut ops = Vec::new();
        for l in lines {
            for it in p.line(l) {
                ops.extend(w.item(&it));
            }
        }
        assert_eq!(w.text, "~\n\nwhat is in hosts?\n\n• I'll take a look.\n✓ Read: /etc/hosts\n    127.0.0.1 localhost\n    ::1 localhost\n• It names localhost.\n\n~\n\nthanks\n\n");
        assert_eq!(w.len, w.text.chars().count());
        // the glyph was written over in place, once the result came
        assert!(ops.iter().any(|o| matches!(o, Op::Glyph { glyph: "✓", .. })), "{ops:?}");
        // the appended text, put together, is the text
        let appended: String = ops.iter().filter_map(|o| match o {
            Op::Append(t) => Some(t.replace('✓', "⋯")),
            _ => None,
        }).collect();
        assert_eq!(appended, w.text.replace('✓', "⋯"));
    }

    #[test]
    fn a_glyph_for_a_call_not_yet_written_waits_for_it() {
        let mut w = Writer::new();
        assert_eq!(w.glyph("t1", "▶"), None);
        let ops = w.item(&Item::Call { id: "t1".into(), title: "Bash: ls".into(), detail: vec![] });
        assert_eq!(ops, vec![Op::Append("▶ Bash: ls\n".into())]);
        assert_eq!(w.glyph("t1", "?"), Some(Op::Glyph { at: 0, glyph: "?" }));
        assert_eq!(w.text, "? Bash: ls\n");
        // an edit before it by someone else moves the anchor
        w.shift(0, 0, 3);
        assert_eq!(w.glyph("t1", "✗"), Some(Op::Glyph { at: 3, glyph: "✗" }));
    }

    #[test]
    fn a_codex_rollout_reads_as_the_conversation() {
        let mut p = Codex { cwd: "/home/me/proj".into(), thoughts: true };
        let mut w = Writer::new();
        let lines = [
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"x","cwd":"/home/me/proj"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>...</environment_context>"}]}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"run the tests"}]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"user_message","message":"run the tests"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Running cargo test"}]}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}","call_id":"c1"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"{\"output\":\"test result: ok\",\"metadata\":{\"exit_code\":0}}"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"All green."}]}}"#,
        ];
        for l in lines {
            for it in p.line(l) {
                w.item(&it);
            }
        }
        assert_eq!(w.text, "~\n\nrun the tests\n\n  · Running cargo test\n✓ shell: bash -lc cargo test\n    test result: ok\n• All green.\n");
    }

    #[test]
    fn a_hunk_shows_what_changed_and_no_more() {
        let (lines, cut) = hunk("a\nb\nc\nd", "a\nB\nc\nd");
        assert_eq!((lines, cut), (vec!["-b".to_string(), "+B".to_string()], 0));
        let big: Vec<String> = (0..30).map(|i| format!("l{i}")).collect();
        let (lines, cut) = hunk("", &big.join("\n"));
        assert_eq!((lines.len(), cut), (12, 18));
    }
}
