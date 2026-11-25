//! The page: the last exchange an agent finished, rendered as
//! apex-acp's `Preview` renders its own -- what was asked, quoted as
//! markdown quotes it, and then the answer, through the converter the
//! session names for markdown (`Preview.md`, `apex md` unless a setting
//! says otherwise), so a page here looks like every other preview.

use std::path::Path;

use apex_tool::Tool;

/// The exchange as markdown: the prompt quoted, then the answer.
pub fn markdown(asked: &str, said: &str) -> String {
    let mut md = String::new();
    for line in asked.trim().lines() {
        md.push_str("> ");
        md.push_str(line);
        md.push('\n');
    }
    if !md.is_empty() {
        md.push('\n');
    }
    md.push_str(said.trim());
    md.push('\n');
    md
}

/// How a conversation looks, as Manifold's does: what you asked in a
/// bubble at the right, in the page's face (what it quotes as code in
/// the code face); what the agent did on the way
/// folded to a line that opens; what it said in between quieter; its
/// answer as the page's own text; what it is doing now beside dots.
const STYLE: &str = r#"<style>
.ax-turn { margin: 0 0 2.2em; }
.ax-turn + .ax-turn { padding-top: 0.6em; }
h4.ax-mark { height: 0; margin: 0; padding: 0; overflow: hidden; opacity: 0; }
pre.ax-prompt { font: inherit; line-height: 1.45;
  white-space: pre-wrap; word-wrap: break-word; width: fit-content; max-width: 82%; box-sizing: border-box;
  margin: 0 0 1.1em auto; background: var(--apex-code-bg); border: none; border-radius: 14px; padding: 0.55em 0.9em; }
p.ax-event, details.ax-steps, p.ax-standing, p.ax-earlier { font-family: var(--apex-mono, Menlo, monospace);
  font-size: 11.5px; color: var(--apex-dim); margin: 0 0 1em; }
details.ax-steps summary { cursor: default; list-style: none; }
details.ax-steps summary::-webkit-details-marker { display: none; }
details.ax-steps summary::before { content: "›"; display: inline-block; width: 1em; transition: transform 0.12s; }
details.ax-steps[open] summary::before { transform: rotate(90deg); }
details.ax-steps[open] { padding-bottom: 0.4em; border-bottom: 1px solid var(--apex-rule); }
details.ax-steps ul { margin: 0.5em 0 0.5em 1em; padding-left: 1em; }
details.ax-steps li.ax-failed { text-decoration: line-through; }
.ax-aside { color: var(--apex-dim); }
.ax-aside > :first-child { margin-top: 0; }
.ax-dots { display: inline-flex; gap: 3px; margin-right: 0.55em; vertical-align: calc(0.34em - 2px); }
.ax-dots i { width: 4px; height: 4px; border-radius: 50%; background: currentColor; opacity: 0.35; animation: ax-dot 1.2s infinite linear; }
.ax-dots i:nth-child(2) { animation-delay: 0.2s; } .ax-dots i:nth-child(3) { animation-delay: 0.4s; }
@keyframes ax-dot { 0% { opacity: 0.35; } 17% { opacity: 1; } 45%, 100% { opacity: 0.35; } }
@media (prefers-reduced-motion: reduce) { .ax-dots i { animation: none; opacity: 0.7; } }
pre.ax-prompt code { font-family: var(--apex-mono, Menlo, monospace); font-size: 0.92em; background: none; padding: 0; }
pre.ax-prompt code.ax-block { display: block; margin: 0.3em 0; }
</style>

"#;

/// A turn: what was asked, and what came of it, in order (a call's
/// title, its tool's name first, and whether it failed; the agent's words).
struct Turn<'a> {
    asked: Option<&'a str>,
    parts: Vec<Part<'a>>,
}

enum Part<'a> {
    Said(&'a str),
    Call { title: &'a str, failed: bool },
    Note(&'a str),
}

/// An agent's conversation from its transcript (`transcript::Item`s),
/// for its lens: a section a turn, the latest last.
pub fn told(items: &[crate::transcript::Item], a: &crate::agent::Agent) -> String {
    use crate::transcript::Item;
    let failed: std::collections::HashSet<&str> = items.iter().filter_map(|i| match i {
        Item::Result { id, ok: false, .. } => Some(id.as_str()),
        _ => None,
    }).collect();
    let mut turns: Vec<Turn> = Vec::new();
    for it in items {
        match it {
            Item::User(t) => turns.push(Turn { asked: Some(t), parts: Vec::new() }),
            Item::Agent(t) | Item::Call { title: t, .. } | Item::Note(t) if turns.is_empty() => {
                let _ = t;
                turns.push(Turn { asked: None, parts: Vec::new() });
            }
            _ => {}
        }
        let Some(turn) = turns.last_mut() else { continue };
        match it {
            Item::Agent(t) => turn.parts.push(Part::Said(t)),
            Item::Call { id, title, .. } => turn.parts.push(Part::Call { title, failed: failed.contains(id.as_str()) }),
            Item::Note(t) => turn.parts.push(Part::Note(t)),
            _ => {}
        }
    }
    page(&turns, a)
}

/// An agent's conversation as its hooks told it (no transcript to read):
/// what was asked and its last word, a turn each.
pub fn conversation(a: &crate::agent::Agent) -> String {
    let mut turns: Vec<Turn> = a.exchanges.iter().map(|x| Turn { asked: Some(&x.asked), parts: vec![Part::Said(&x.said)] }).collect();
    let finished = a.exchanges.last().map(|x| x.asked.trim()) == a.prompt.as_deref().map(str::trim);
    if let Some(p) = a.prompt.as_deref().filter(|_| !finished && going(a)) {
        turns.push(Turn { asked: Some(p), parts: Vec::new() });
    }
    page(&turns, a)
}

/// How many turns are shown, the latest: the rest said in a line.
const MOST: usize = 40;

fn page(turns: &[Turn], a: &crate::agent::Agent) -> String {
    let mut out = String::from(STYLE);
    let shown = &turns[turns.len().saturating_sub(MOST)..];
    if turns.len() > shown.len() {
        out.push_str(&format!("<p class=\"ax-earlier\">{} earlier turns</p>\n\n", turns.len() - shown.len()));
    }
    if shown.is_empty() {
        out.push_str("<p class=\"ax-standing\">Nothing asked yet: what you write below is its first prompt.</p>\n\n");
    }
    let last = shown.len().saturating_sub(1);
    for (i, t) in shown.iter().enumerate() {
        out.push_str(&format!("<section class=\"ax-turn\" id=\"turn-{}\">\n\n", turns.len() - shown.len() + i));
        if let Some(asked) = t.asked {
            // a mark for the page's outline: its turns
            out.push_str(&format!("<h4 class=\"ax-mark\">{}</h4>\n\n", escape(&title(asked))));
            match notification(asked) {
                Some(note) => out.push_str(&format!("<p class=\"ax-event\">{}</p>\n\n", escape(&note))),
                None => out.push_str(&format!("<pre class=\"ax-prompt\">{}</pre>\n\n", prompt_html(asked.trim()))),
            }
        }
        // what it did on the way (up to its last call), and its answer
        let cut = t.parts.iter().rposition(|p| matches!(p, Part::Call { .. })).map_or(0, |i| i + 1);
        out.push_str(&along(&t.parts[..cut]));
        for p in &t.parts[cut..] {
            match p {
                Part::Said(s) => {
                    out.push_str(s.trim());
                    out.push_str("\n\n");
                }
                Part::Note(s) => out.push_str(&format!("<p class=\"ax-event\">{}</p>\n\n", escape(s.trim()))),
                Part::Call { .. } => {}
            }
        }
        if i == last {
            out.push_str(&standing(a, t));
        }
        out.push_str("</section>\n\n");
    }
    out
}

/// What a turn did on the way: what it said as it went, quieter, and
/// each run of calls between folded to a line that opens to them.
fn along(parts: &[Part]) -> String {
    let mut out = String::new();
    let mut run: Vec<(&str, bool)> = Vec::new();
    let flush = |run: &mut Vec<(&str, bool)>, out: &mut String| {
        if run.is_empty() {
            return;
        }
        out.push_str(&format!("<details class=\"ax-steps\"><summary>{}</summary>\n<ul>\n", escape(&summary(run))));
        for (title, failed) in run.iter() {
            let class = if *failed { " class=\"ax-failed\"" } else { "" };
            out.push_str(&format!("<li{class}>{}</li>\n", escape(title)));
        }
        out.push_str("</ul>\n</details>\n\n");
        run.clear();
    };
    for p in parts {
        match p {
            Part::Call { title, failed } => run.push((title, *failed)),
            Part::Said(s) => {
                flush(&mut run, &mut out);
                out.push_str(&format!("<div class=\"ax-aside\">\n\n{}\n\n</div>\n\n", s.trim()));
            }
            Part::Note(s) => {
                flush(&mut run, &mut out);
                out.push_str(&format!("<p class=\"ax-event\">{}</p>\n\n", escape(s.trim())));
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

/// What was asked, as typed, for its bubble: the text as it is, line
/// for line, and what it quotes as code -- a `span`, a fenced block --
/// in the code face. (A `pre`, so that a blank line in it does not end
/// it as markdown's HTML block.)
fn prompt_html(text: &str) -> String {
    let mut out = String::new();
    let mut parts = text.split("```").enumerate().peekable();
    while let Some((i, part)) = parts.next() {
        if i % 2 == 1 && parts.peek().is_some() {
            // a fenced block: its first line, the language, left out
            let body = part.split_once('\n').map_or(part, |(first, rest)| if first.trim().contains(' ') { part } else { rest });
            out.push_str(&format!("<code class=\"ax-block\">{}</code>", escape(body.trim_end_matches('\n'))));
            continue;
        }
        if i % 2 == 1 {
            out.push_str("```");
        }
        // `spans` in what is between
        let mut inline = part.split('`').enumerate().peekable();
        while let Some((j, bit)) = inline.next() {
            if j % 2 == 1 && inline.peek().is_some() && !bit.is_empty() && !bit.contains('\n') {
                out.push_str(&format!("<code>{}</code>", escape(bit)));
            } else if j % 2 == 1 {
                // no span: its backticks as typed
                out.push('`');
                out.push_str(&escape(bit));
                if inline.peek().is_some() {
                    out.push('`');
                }
            } else {
                out.push_str(&escape(bit));
            }
        }
    }
    out
}

/// A call's tool, as its title begins (`Bash: make`).
fn tool_of(title: &str) -> &str {
    title.split_once(':').map_or(title, |(t, _)| t).trim()
}

/// What a run of calls did, in a line: "Read 6 files · ran 3 commands".
fn summary(run: &[(&str, bool)]) -> String {
    let mut order: Vec<&str> = Vec::new();
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (title, _) in run {
        let k = match tool_of(title) {
            "Read" | "NotebookRead" | "read_file" | "read" | "view_file" => "read",
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "edit_file" | "write_file" | "edit" | "write" | "apply_patch" => "edit",
            "Bash" | "bash" | "BashOutput" | "KillShell" | "shell" | "shell_command" | "exec_command" | "local_shell" | "container.exec" => "run",
            "Grep" | "Glob" | "LS" | "grep" | "find" | "search" | "ls" => "search",
            "WebFetch" | "WebSearch" => "web",
            "Task" | "Agent" | "subagent_spawn" => "agent",
            "TodoWrite" | "update_plan" | "write_todos" => "todo",
            _ => "other",
        };
        if !counts.contains_key(k) {
            order.push(k);
        }
        *counts.entry(k).or_insert(0) += 1;
    }
    let n = |k: &str, one: &str, many: &str| match counts.get(k).copied().unwrap_or(0) {
        1 => one.to_string(),
        c => many.replace("%d", &c.to_string()),
    };
    let words: Vec<String> = order
        .iter()
        .map(|k| match *k {
            "read" => n(k, "read a file", "read %d files"),
            "edit" => n(k, "made an edit", "made %d edits"),
            "run" => n(k, "ran a command", "ran %d commands"),
            "search" => n(k, "searched once", "searched %d times"),
            "web" => n(k, "looked on the web", "looked on the web %d times"),
            "agent" => n(k, "asked a subagent", "asked %d subagents"),
            "todo" => "kept its to-do list".to_string(),
            _ => n(k, "used another tool", "used %d other tools"),
        })
        .collect();
    let line = words.join(" · ");
    let mut c = line.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "Thought it through".to_string(),
    }
}

/// A prompt that was not asked but is the agent's harness telling it
/// something (Claude Code's `<task-notification>`), in a line.
fn notification(prompt: &str) -> Option<String> {
    let p = prompt.trim();
    if !p.starts_with("<task-notification>") {
        return None;
    }
    let tag = |name: &str| {
        let a = p.find(&format!("<{name}>"))? + name.len() + 2;
        let b = p[a..].find(&format!("</{name}>"))? + a;
        Some(p[a..b].trim().to_string())
    };
    Some(tag("summary").filter(|s| !s.is_empty()).unwrap_or_else(|| format!("A background task {}", tag("status").unwrap_or_else(|| "finished".into()))))
}

/// A turn's prompt in a line, for the outline.
fn title(asked: &str) -> String {
    if let Some(n) = notification(asked) {
        return n;
    }
    let first = asked.trim().lines().next().unwrap_or("").trim();
    let short: String = first.chars().take(60).collect();
    if first.chars().count() > 60 { format!("{short}…") } else { short }
}

fn going(a: &crate::agent::Agent) -> bool {
    use crate::agent::State;
    matches!(a.state, State::Working | State::Asking | State::Starting)
}

/// How the agent stands, at the foot of its last turn while it is at it:
/// what it is doing beside its dots, or that it needs you.
fn standing(a: &crate::agent::Agent, t: &Turn) -> String {
    use crate::agent::State;
    if !going(a) {
        return String::new();
    }
    if a.state == State::Asking {
        let what = a.asked.as_deref().map(|c| format!(": {c}")).unwrap_or_default();
        return format!("<p class=\"ax-standing\">Needs you{} — answer in its terminal (⌘↩)</p>\n\n", escape(&what));
    }
    let now = a.running.last().map(|(_, line)| line.as_str()).or_else(|| t.parts.iter().rev().find_map(|p| match p {
        Part::Call { title, .. } => Some(*title),
        _ => None,
    }));
    let now = now.map(activity).unwrap_or_else(|| "Working".to_string());
    format!("<p class=\"ax-standing\"><span class=\"ax-dots\"><i></i><i></i><i></i></span>{}…</p>\n\n", escape(&now))
}

/// What a call is doing, in a few words: "Running make", "Reading main.rs".
fn activity(title: &str) -> String {
    let what = title.split_once(':').map(|(_, w)| w.trim()).unwrap_or("");
    let file = what.rsplit('/').next().unwrap_or(what);
    let cut = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    match tool_of(title) {
        "Read" | "read_file" | "read" | "view_file" => if what.is_empty() { "Reading".into() } else { format!("Reading {file}") },
        "Edit" | "MultiEdit" | "Write" | "edit_file" | "write_file" | "edit" | "write" | "apply_patch" => if what.is_empty() { "Editing".into() } else { format!("Editing {file}") },
        "Bash" | "bash" | "shell" | "shell_command" | "exec_command" | "local_shell" => if what.is_empty() { "Running a command".into() } else { format!("Running {}", cut(what, 80)) },
        "Grep" | "Glob" | "LS" | "grep" | "find" | "search" | "ls" => if what.is_empty() { "Searching".into() } else { format!("Searching for {}", cut(what, 60)) },
        "WebFetch" | "WebSearch" => "Looking on the web".into(),
        "Task" | "Agent" | "subagent_spawn" => if what.is_empty() { "Asking a subagent".into() } else { format!("Asking a subagent: {}", cut(what, 60)) },
        "TodoWrite" | "update_plan" | "write_todos" => "Updating its to-do list".into(),
        t => format!("Using {t}"),
    }
}

/// Markdown as a page, through the session's converter.
pub fn render_md(t: &Tool, cwd: &Path, md: &str) -> String {
    let cmd = t.setting("Preview.md").unwrap_or_else(|| "apex md".to_string());
    match filter(&cmd, md, cwd) {
        Ok(html) => html,
        Err(e) => format!("<pre>{}</pre>", escape(&format!("{cmd}: {e}\n\n{md}"))),
    }
}

/// The exchange as a page, or a word when there is none yet.
pub fn render(t: &Tool, cwd: &Path, exchange: Option<(&str, &str)>) -> String {
    let Some((asked, said)) = exchange else {
        return "<p><em>no reply yet</em></p>".to_string();
    };
    let md = markdown(asked, said);
    let cmd = t.setting("Preview.md").unwrap_or_else(|| "apex md".to_string());
    match filter(&cmd, &md, cwd) {
        Ok(html) => html,
        Err(e) => format!("<pre>{}</pre>", escape(&format!("{cmd}: {e}\n\n{md}"))),
    }
}

/// Run `cmd` with `input` on its standard input, in `dir`, and take
/// what it writes.
pub fn filter(cmd: &str, input: &str, dir: &Path) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = if dir.is_dir() { dir } else { Path::new("/") };
    let mut child = Command::new(shell()).arg("-c").arg(cmd).current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let text = input.to_string();
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    // a big reply would fill the pipe before the child reads it
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(text.as_bytes());
    });
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".to_string())
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exchange_is_the_prompt_quoted_and_then_the_answer() {
        assert_eq!(markdown("what is in hosts?\n\nsay it briefly\n", "It names localhost.\n"), "> what is in hosts?\n> \n> say it briefly\n\nIt names localhost.\n");
        assert_eq!(markdown("", "Hello."), "Hello.\n");
    }

    #[test]
    fn a_filter_takes_stdin_and_says_what_went_wrong() {
        assert_eq!(filter("cat", "a\nb\n", Path::new("/")).unwrap(), "a\nb\n");
        assert_eq!(filter("tr a-z A-Z", "abc", Path::new("/nonesuch")).unwrap(), "ABC");
        let e = filter("echo bad >&2; exit 3", "", Path::new("/")).unwrap_err();
        assert_eq!(e, "bad");
        assert_eq!(escape("<a & b>"), "&lt;a &amp; b&gt;");
    }

    #[test]
    fn a_prompt_is_its_text_its_code_in_the_code_face() {
        assert_eq!(prompt_html("fix `make` <now>"), "fix <code>make</code> &lt;now&gt;");
        assert_eq!(prompt_html("run\n```sh\nmake test\n```\nthen"), "run\n<code class=\"ax-block\">make test</code>\nthen");
        // a lone backtick, or one across lines, is only a backtick
        assert_eq!(prompt_html("a ` b"), "a ` b");
        assert_eq!(prompt_html("a `b\nc` d"), "a `b\nc` d");
    }
}
