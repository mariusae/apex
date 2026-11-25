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

/// An agent's conversation as markdown, for its lens (`View`): each
/// exchange finished, what was asked quoted and then the answer, and the
/// turn going, if one is -- what it was asked, and what it is doing.
pub fn conversation(a: &crate::agent::Agent) -> String {
    let mut md = String::new();
    // each turn a heading, its prompt's first line: the page's outline is
    // the conversation's turns, to go to from its scrollbar
    let heading = |asked: &str| {
        let first = asked.trim().lines().next().unwrap_or("").trim();
        let short: String = first.chars().take(60).collect();
        let more = if first.chars().count() > 60 { "…" } else { "" };
        format!("### {}{more}\n\n", escape(&short).replace('#', "\\#"))
    };
    for x in &a.exchanges {
        md.push_str(&heading(&x.asked));
        md.push_str(&markdown(&x.asked, &x.said));
        md.push_str("\n---\n\n");
    }
    let going = matches!(a.state, crate::agent::State::Working | crate::agent::State::Asking | crate::agent::State::Starting);
    if going {
        let finished = a.exchanges.last().map(|x| x.asked.trim()) == a.prompt.as_deref().map(str::trim);
        if let Some(p) = a.prompt.as_deref().filter(|_| !finished) {
            md.push_str(&heading(p));
            md.push_str(&markdown(p, ""));
        }
        let doing = match (a.state, a.running.last(), a.asked.as_deref()) {
            (crate::agent::State::Asking, _, Some(call)) => format!("asks to run {call}"),
            (crate::agent::State::Asking, _, None) => "has a question".to_string(),
            (_, Some((_, line)), _) => line.clone(),
            _ => "working".to_string(),
        };
        md.push_str(&format!("*{}*\n", escape(doing.trim())));
    } else if a.exchanges.is_empty() {
        md.push_str("*nothing asked yet*\n");
    }
    md
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
}
