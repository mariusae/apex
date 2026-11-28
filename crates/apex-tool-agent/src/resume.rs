//! Program resume (OSC 7577) for the agents apex hooks: at an agent's
//! session start its hook tells the terminal the command that brings
//! that session back -- the agent as it was started, its options kept,
//! with what resumes this session in place of whatever started it -- and
//! at an end the user asked for, that there is nothing to bring back.
//! What the terminal does with it is its own (`apex_server::resume`).
//!
//! Claude Code: `claude [options] --resume SESSION`. Codex: `codex resume
//! [options] SESSION`. A prompt given on the command line, a session to
//! resume or fork, a worktree to make, files or images to send: once, not
//! again. A run that is no interactive session (print mode, Codex's exec
//! and its kin, one kept off disk) is not reported.

use std::io::Write;

use apex_server::resume::{self, Ask};

/// How an option takes its value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Takes {
    /// A flag.
    Nothing,
    /// The next argument.
    One,
    /// The next, unless it is an option.
    Maybe,
    /// Every argument after, up to the next option.
    Many,
}

/// What becomes of an option in the resume command.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fate {
    Keep,
    Drop,
    /// No session to resume: not reported at all.
    NotASession,
}

struct Opt {
    names: &'static [&'static str],
    takes: Takes,
    fate: Fate,
}

const fn opt(names: &'static [&'static str], takes: Takes, fate: Fate) -> Opt {
    Opt { names, takes, fate }
}

use Fate::{Drop, Keep, NotASession};
use Takes::{Many, Maybe, Nothing, One};

/// Claude Code's options (2.1), as `claude --help` has them: those that
/// take a value and how, and those not to be kept. One not here is a
/// flag, kept.
const CLAUDE: &[Opt] = &[
    opt(&["-p", "--print"], Nothing, NotASession),
    opt(&["--bg", "--background", "--desktop", "--no-session-persistence"], Nothing, NotASession),
    opt(&["-c", "--continue", "--fork-session", "--tmux"], Nothing, Drop),
    opt(&["-r", "--resume", "--from-pr", "--teleport", "--cloud", "-w", "--worktree"], Maybe, Drop),
    opt(&["--session-id", "--environment", "--input-format", "--output-format", "--json-schema", "--max-budget-usd"], One, Drop),
    opt(&["--file"], Many, Drop),
    opt(&["-d", "--debug", "--prompt-suggestions", "--remote-control"], Maybe, Keep),
    opt(
        &[
            "--agent",
            "--agents",
            "--append-system-prompt",
            "--autocompact",
            "--debug-file",
            "--effort",
            "--fallback-model",
            "--model",
            "-n",
            "--name",
            "--permission-mode",
            "--permission-prompts",
            "--plugin-dir",
            "--plugin-url",
            "--remote-control-session-name-prefix",
            "--setting-sources",
            "--settings",
            "--system-prompt",
            "--system-prompt-snapshot",
        ],
        One,
        Keep,
    ),
    opt(&["--add-dir", "--allowedTools", "--allowed-tools", "--betas", "--disallowedTools", "--disallowed-tools", "--mcp-config", "--tools"], Many, Keep),
];

/// Codex's options (0.166), the same before `resume` as after.
const CODEX: &[Opt] = &[
    opt(&["--last", "--all", "--include-non-interactive", "--worktree"], Nothing, Drop),
    opt(&["-C", "--cd"], One, Drop),
    opt(&["-i", "--image"], Many, Drop),
    opt(&["-c", "--config", "--enable", "--disable", "--remote", "--remote-auth-token-env", "-m", "--model", "--local-provider", "-p", "--profile", "-s", "--sandbox", "-a", "--ask-for-approval"], One, Keep),
];

/// Codex's subcommands that are still an interactive session.
const CODEX_SESSIONS: [&str; 2] = ["resume", "fork"];

/// The options of `args` (the agent's arguments, its program gone) kept
/// by `table`; None when they are no session to resume. Positional
/// arguments -- a prompt, a subcommand's words -- go, but `positional`
/// says, of each, whether it ends things (a subcommand that is no
/// session).
fn kept(table: &[Opt], args: &[String], positional: &mut dyn FnMut(&str, bool) -> bool) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut first = true;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if a == "--" {
            break; // the rest is a prompt
        }
        if !a.starts_with('-') || a == "-" {
            if !positional(a, first) {
                return None;
            }
            first = false;
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, _)) if n.starts_with("--") => (n, true),
            _ => (a.as_str(), false),
        };
        let o = table.iter().find(|o| o.names.contains(&name));
        let (takes, fate) = o.map_or((Nothing, Keep), |o| (o.takes, o.fate));
        if fate == NotASession {
            return None;
        }
        let mut words = vec![a.clone()];
        if !inline {
            let value = |s: &String| !s.starts_with('-');
            match takes {
                Nothing => {}
                One => {
                    if let Some(v) = args.get(i) {
                        words.push(v.clone());
                        i += 1;
                    }
                }
                Maybe => {
                    if args.get(i).is_some_and(value) {
                        words.push(args[i].clone());
                        i += 1;
                    }
                }
                Many => {
                    while args.get(i).is_some_and(value) {
                        words.push(args[i].clone());
                        i += 1;
                    }
                }
            }
        }
        if fate == Keep {
            out.extend(words);
        }
    }
    Some(out)
}

/// The arguments after the program: `node cli.js ...` without the
/// runtime and its script, else all but the first.
fn after_program(argv: &[String]) -> &[String] {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    match argv.first().map(|a| base(a)) {
        Some(r) if ["node", "bun", "deno"].contains(&r.as_str()) => argv.get(2..).unwrap_or(&[]),
        Some(_) => &argv[1..],
        None => &[],
    }
}

/// The command that resumes session `session` of `agent` (claude,
/// codex), started as `argv`; None for an agent not known here, or a
/// run that is no session to resume.
pub fn command(agent: &str, argv: &[String], session: &str) -> Option<Vec<String>> {
    if session.is_empty() {
        return None;
    }
    let args = after_program(argv);
    match agent {
        "claude" => {
            let mut out = vec!["claude".to_string()];
            out.extend(kept(CLAUDE, args, &mut |_, _| true)?);
            out.extend(["--resume".to_string(), session.to_string()]);
            Some(out)
        }
        "codex" => {
            // a first positional is a subcommand, or the prompt; a session
            // only without one, or with resume or fork
            let mut sub_ok = |w: &str, first: bool| !first || CODEX_SESSIONS.contains(&w) || !is_codex_subcommand(w);
            let mut out = vec!["codex".to_string(), "resume".to_string()];
            out.extend(kept(CODEX, args, &mut sub_ok)?);
            out.push(session.to_string());
            Some(out)
        }
        _ => None,
    }
}

/// Codex's subcommands that are not a session: the rest of what a first
/// word can be is a prompt.
fn is_codex_subcommand(w: &str) -> bool {
    [
        "agents", "exec", "e", "review", "login", "logout", "mcp", "plugin", "app-server", "remote-control", "app", "completion", "update", "doctor", "sandbox", "debug", "apply", "queue", "archive", "delete",
        "migrate-rollouts", "migrate-nfs-db", "unarchive", "cloud", "exec-server", "features", "help",
    ]
    .contains(&w)
}

/// The agent's session `session` started (its hook, at SessionStart):
/// the terminal told how to resume it, when the agent is one apex knows
/// how to resume, runs in a terminal that takes reports (one that gave
/// it a token), and is a session. `pid`: the agent's process.
pub fn report_start(agent: &str, pid: u32, session: &str, cwd: &str) {
    let Ok(token) = std::env::var(resume::TOKEN_ENV) else { return };
    let Some(argv) = resume::proc_argv(pid) else { return };
    let Some(argv) = command(agent, &argv, session) else { return };
    let cwd = Some(cwd.to_string()).filter(|c| c.starts_with('/'));
    tell(&resume::sequence(pid, resume::now(), &Ask::Resume { argv, cwd }, &token));
}

/// The agent's session ended (its hook, at SessionEnd) for `reason`:
/// when the user ended it, nothing is to be resumed. Not when it ended
/// for any other reason -- its terminal going, a signal -- which is what
/// resume is for.
pub fn report_end(agent: &str, pid: u32, reason: Option<&str>) {
    let ended_by_user = match agent {
        "claude" => matches!(reason, Some("prompt_input_exit" | "logout")),
        "codex" => matches!(reason, Some("exit" | "quit" | "logout" | "prompt_input_exit")),
        _ => false,
    };
    if !ended_by_user {
        return;
    }
    let Ok(token) = std::env::var(resume::TOKEN_ENV) else { return };
    tell(&resume::sequence(pid, resume::now(), &Ask::Clear, &token));
}

/// A report to the terminal: written to it directly (`/dev/tty`), as the
/// hook's own output is the agent's to read.
fn tell(seq: &str) {
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(seq.as_bytes());
        let _ = tty.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn claude_keeps_its_options_and_resumes_the_session() {
        let argv = v(&["claude", "--model", "opus", "--dangerously-skip-permissions", "--add-dir", "/a", "/b", "--permission-mode=plan", "fix the bug"]);
        assert_eq!(command("claude", &argv, "s1"), Some(v(&["claude", "--model", "opus", "--dangerously-skip-permissions", "--add-dir", "/a", "/b", "--permission-mode=plan", "--resume", "s1"])));
    }

    #[test]
    fn claude_drops_what_was_for_once() {
        // a resume or continue it was started with, its prompt, a worktree
        assert_eq!(command("claude", &v(&["claude", "--resume", "old", "--model", "opus"]), "new"), Some(v(&["claude", "--model", "opus", "--resume", "new"])));
        assert_eq!(command("claude", &v(&["claude", "-c", "-w", "feature", "go on"]), "s"), Some(v(&["claude", "--resume", "s"])));
        assert_eq!(command("claude", &v(&["claude", "--session-id", "u-1", "--", "-not an option"]), "s"), Some(v(&["claude", "--resume", "s"])));
        // a resume with no session named is an option still
        assert_eq!(command("claude", &v(&["claude", "-r", "--verbose"]), "s"), Some(v(&["claude", "--verbose", "--resume", "s"])));
    }

    #[test]
    fn claude_in_print_mode_or_kept_off_disk_is_no_session() {
        assert_eq!(command("claude", &v(&["claude", "-p", "hello"]), "s"), None);
        assert_eq!(command("claude", &v(&["claude", "--no-session-persistence"]), "s"), None);
    }

    #[test]
    fn claude_under_node_is_claude() {
        assert_eq!(command("claude", &v(&["/usr/bin/node", "/x/cli.js", "--verbose"]), "s"), Some(v(&["claude", "--verbose", "--resume", "s"])));
    }

    #[test]
    fn codex_resumes_with_its_options_and_none_of_its_prompt() {
        let argv = v(&["codex", "-m", "o3", "-c", "model_reasoning_effort=\"high\"", "--dangerously-bypass-approvals-and-sandbox", "-i", "a.png", "b.png", "do it"]);
        assert_eq!(command("codex", &argv, "u-2"), Some(v(&["codex", "resume", "-m", "o3", "-c", "model_reasoning_effort=\"high\"", "--dangerously-bypass-approvals-and-sandbox", "u-2"])));
        // started as a resume or a fork: resumed as this session
        assert_eq!(command("codex", &v(&["codex", "resume", "--last", "-s", "workspace-write"]), "u-3"), Some(v(&["codex", "resume", "-s", "workspace-write", "u-3"])));
        assert_eq!(command("codex", &v(&["codex", "fork", "old-id"]), "u-4"), Some(v(&["codex", "resume", "u-4"])));
    }

    #[test]
    fn codex_exec_and_its_kin_are_no_session() {
        assert_eq!(command("codex", &v(&["codex", "exec", "do it"]), "u"), None);
        assert_eq!(command("codex", &v(&["codex", "review"]), "u"), None);
    }

    #[test]
    fn an_agent_not_known_here_is_not_reported() {
        assert_eq!(command("muse", &v(&["muse"]), "s"), None);
        assert_eq!(command("claude", &v(&["claude"]), ""), None);
    }

    #[test]
    fn what_a_hook_reports_the_terminal_reads() {
        let argv = command("claude", &v(&["claude", "--model", "opus"]), "s1").unwrap();
        let ask = Ask::Resume { argv: argv.clone(), cwd: Some("/p".into()) };
        let seq = resume::sequence(77, 1_800_000_000, &ask, "a-token-of-twenty-two-chars");
        let body = seq.strip_prefix("\x1b]7577;").and_then(|s| s.strip_suffix("\x1b\\")).unwrap();
        let r = resume::parse(body, "a-token-of-twenty-two-chars", 1_800_000_000).unwrap();
        assert_eq!((r.pid, r.ask), (77, Ask::Resume { argv, cwd: Some("/p".into()) }));
    }
}
