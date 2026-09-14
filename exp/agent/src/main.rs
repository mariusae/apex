//! apex-agent: an experiment. One window that says what every agent is
//! doing -- every Claude Code and Codex on the machine, whichever
//! terminal or editor it was started from -- fed by the hooks those
//! agents offer, and a transcript window for any of them, B3'd open.
//!
//!     apex-agent [-cwd DIR] [-thoughts]     the pane, DIR/-agents
//!     apex-agent install [claude|codex]...  put the hooks in (both, by default)
//!     apex-agent uninstall [claude|codex]...
//!     apex-agent hook claude|codex          what the agents run; not for typing
//!
//! The hook is this same program: each event the agent has is one line
//! appended to `~/.apex/agents/SESSION.jsonl`, and the pane reads those
//! logs, so nothing need be running when an agent starts and nothing
//! is lost when the pane is not. The pane is a block an agent, in the
//! order they want attention: `?` a permission or a question waiting on
//! you, `✗` a turn that failed, `~` a turn over and the next prompt
//! yours, `▶` at work. B3 anywhere in a block (or `Open`) opens the
//! agent's transcript beside it, named for the agent's own directory,
//! and read from the agent's own record as it grows.

use std::path::PathBuf;

use apex_agent::{event, hook, install, win};

fn usage() -> ! {
    eprintln!("usage: apex-agent [-cwd DIR] [-thoughts]");
    eprintln!("       apex-agent install|uninstall [claude|codex]...");
    eprintln!("       apex-agent hook claude|codex");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") => {
            let agent = args.get(1).map(String::as_str).unwrap_or_else(|| usage());
            std::process::exit(hook::run(agent));
        }
        Some(cmd @ ("install" | "uninstall")) => {
            let which: Vec<&str> = if args.len() > 1 { args[1..].iter().map(String::as_str).collect() } else { install::AGENTS.to_vec() };
            for a in &which {
                if !install::AGENTS.contains(a) {
                    eprintln!("apex-agent: {a}: not an agent I know; claude or codex");
                    std::process::exit(2);
                }
            }
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| usage()));
            let res = match cmd {
                "install" => {
                    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_else(|e| {
                        eprintln!("apex-agent: where am I? {e}");
                        std::process::exit(1)
                    });
                    install::install(&home, &exe, &which)
                }
                _ => install::uninstall(&home, &which),
            };
            match res {
                Ok(lines) => {
                    for l in lines {
                        println!("{l}");
                    }
                }
                Err(e) => {
                    eprintln!("apex-agent: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            let opts = parse_pane(&args);
            if let Err(e) = win::Pane::run(opts) {
                eprintln!("apex-agent: {}", e.0);
                std::process::exit(1);
            }
        }
    }
}

fn parse_pane(args: &[String]) -> win::Opts {
    let mut opts = win::Opts { cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")), dir: event::dir(), thoughts: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-cwd" | "--cwd" => {
                let d = PathBuf::from(it.next().unwrap_or_else(|| usage()));
                opts.cwd = if d.is_absolute() { d } else { opts.cwd.join(d) };
            }
            "-thoughts" | "--thoughts" => opts.thoughts = true,
            _ => usage(),
        }
    }
    opts
}
