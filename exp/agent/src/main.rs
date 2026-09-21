//! apex-agent: an experiment. What every agent in the session is
//! doing -- every Claude Code and Codex started in one of its terminals
//! -- fed by the hooks those agents offer, and for any of them its
//! transcript, its last answer as a page, its changes as a diff.
//!
//!     apex-agent [-cwd DIR] [-a] [-all] [-thoughts] [-quiet]
//!     apex-agent install [claude|codex]...  put the hooks in (both, by default)
//!     apex-agent uninstall [claude|codex]...
//!     apex-agent ls                         every agent, as text
//!     apex-agent wait ID                    until the agent's turn ends; the exit status is its state
//!     apex-agent events [-all]              the events as they come, a line each
//!     apex-agent hook claude|codex          what the agents run; not for typing
//!
//! The hook is this same program: each event the agent has is one line
//! appended to `~/.apex/agents/SESSION.jsonl`, and apex-agent reads
//! those logs, so nothing need be running when an agent starts and
//! nothing is lost when it is not.
//!
//! It has no window of its own by default: its job is the terminals of
//! its own session that are running agents, and what it does with them
//! it does on their windows. While an agent runs, `Transcript`,
//! `Preview` and `Changes` are in its terminal's tools menu -- the
//! transcript, named for the agent's own directory and read from the
//! agent's own record as it grows; a page with the last exchange it
//! finished, written afresh as each turn ends; its repository's diff
//! since the session began -- and while it asks something, `Allow Deny
//! Ask` are there too, the answer going into the log for the hook that
//! asked to read. An agent that wants you -- its turn over and the next
//! prompt yours, a question to answer, a turn that failed -- raises a
//! notification on its terminal, so the session's square says someone
//! is waiting and a click takes you to them, one agent a click;
//! it goes as soon as the agent is back at work.
//!
//! `-a` adds the overview window, `DIR/-agents`, a block an agent in
//! the order they want attention: `?` a permission or a question
//! waiting on you, `✗` a turn that failed, `~` a turn over and the next
//! prompt yours, `▶` at work. A question is answered where it stands,
//! `Allow Deny Ask`. B3 anywhere in a block (or `Open`) opens the
//! agent's transcript beside it; `Preview` its page; `Changes` its
//! diff; `Goto` goes to the agent itself, the window it was started in,
//! in whatever session that was; `Send TEXT` types into that window;
//! `CopyContext` in any text window copies its selection, or the current
//! line when there is none, headed by the file and line it came from;
//! `Start` and `Resume` make terminals running agents; `History` lists
//! the directory's past sessions.
//!
//! `-all` widens both from this session to every agent on the machine,
//! whichever terminal or editor it was started from. `ls`, `wait` and
//! `events` are the same logs as text, for scripts.

use std::path::PathBuf;
use std::time::Duration;

use apex_agent::agents::{self, Agents, State};
use apex_agent::event::{self, Tail};
use apex_agent::{hook, install, win};

fn usage() -> ! {
    eprintln!("usage: apex-agent [-cwd DIR] [-a] [-all] [-thoughts] [-quiet]");
    eprintln!("       apex-agent install|uninstall [claude|codex]...");
    eprintln!("       apex-agent ls | wait ID | events [-all]");
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
                    let exe = install::invoked_as(std::env::args_os().next(), std::env::current_dir().ok(), std::env::var_os("PATH")).or_else(|| std::env::current_exe().ok()).unwrap_or_else(|| {
                        eprintln!("apex-agent: where am I?");
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
        Some("ls") => {
            let agents = read_all(&event::dir());
            let home = std::env::var("HOME").ok();
            let (header, blocks, footer) = agents::pane(&agents.ordered(), event::now_ms(), home.as_deref(), None, None);
            let (text, _) = agents::pane_text(&header, &blocks, &footer);
            print!("{text}");
        }
        Some("wait") => {
            let id = args.get(1).map(String::as_str).unwrap_or_else(|| usage());
            std::process::exit(wait(&event::dir(), id));
        }
        Some("events") => {
            let all = args.iter().skip(1).any(|a| a == "-all" || a == "--all");
            events(&event::dir(), all);
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
    let mut opts = win::Opts::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")), event::dir());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-cwd" | "--cwd" => {
                let d = PathBuf::from(it.next().unwrap_or_else(|| usage()));
                opts.cwd = if d.is_absolute() { d } else { opts.cwd.join(d) };
            }
            "-a" | "--a" => opts.pane = true,
            "-thoughts" | "--thoughts" => opts.thoughts = true,
            "-all" | "--all" => opts.all = true,
            "-quiet" | "--quiet" => opts.quiet = true,
            _ => usage(),
        }
    }
    opts
}

/// Every session's log, read whole: the agents as they stand.
fn read_all(dir: &std::path::Path) -> Agents {
    let mut agents = Agents::default();
    let Ok(rd) = std::fs::read_dir(dir) else { return agents };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && event::session_of(p).is_some()).collect();
    paths.sort();
    for p in paths {
        let mut t = Tail::new(p);
        for ev in event::events(&t.lines()) {
            agents.apply(&ev);
        }
    }
    agents
}

/// `apex-agent wait ID`: until the agent named (by its id, a prefix of
/// it, or its kind when there is one such) is no longer working. The
/// exit status is its state: 0 its turn is over and the next prompt is
/// yours, 1 it is asking something, 2 the turn failed, 3 it is gone, 4
/// no such agent. What a script chains on.
fn wait(dir: &std::path::Path, id: &str) -> i32 {
    let find = |agents: &Agents| -> Option<String> {
        if let Some(a) = agents.by_id(id) {
            return Some(a.session.clone());
        }
        let kinds: Vec<&agents::Agent> = agents.ordered().into_iter().filter(|a| a.kind == id).collect();
        match kinds.as_slice() {
            [one] => Some(one.session.clone()),
            _ => None,
        }
    };
    let agents = read_all(dir);
    let Some(session) = find(&agents) else {
        eprintln!("apex-agent: wait {id}: no such agent");
        return 4;
    };
    let mut log = Tail::new(event::log_path(dir, &session));
    let mut agent = agents::Agent::new(&session);
    for ev in event::events(&log.lines()) {
        agent.apply(&ev);
    }
    loop {
        match agent.state {
            State::Idle => return 0,
            State::Asking => return 1,
            State::Failed => return 2,
            State::Ended => return 3,
            State::Working | State::Starting => {}
        }
        if !log.path.exists() || agent.pid.is_some_and(|p| !event::alive(p as i32)) {
            return 3;
        }
        std::thread::sleep(Duration::from_millis(200));
        for ev in event::events(&log.lines()) {
            agent.apply(&ev);
        }
    }
}

/// `apex-agent events`: the events as they land, a line each -- when,
/// the session, the agent, what happened, and the words for it -- from
/// now, or from the start of every log with `-all`. Until killed.
fn events(dir: &std::path::Path, all: bool) {
    use std::collections::HashMap;
    use std::io::Write;
    let mut tails: HashMap<String, Tail> = HashMap::new();
    let mut first = true;
    let out = std::io::stdout();
    loop {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && event::session_of(p).is_some()).collect();
            paths.sort();
            for p in paths {
                let session = event::session_of(&p).unwrap_or_default();
                let t = tails.entry(session).or_insert_with(|| {
                    let mut t = Tail::new(p.clone());
                    if first && !all {
                        t.read = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    }
                    t
                });
                for ev in event::events(&t.lines()) {
                    let what = ev.title.as_deref().or(ev.kind.as_deref()).or(ev.text.as_deref()).map(apex_agent::transcript::brief).unwrap_or_default();
                    let mut o = out.lock();
                    if writeln!(o, "{}\t{}\t{}\t{}\t{}", ev.ms, ev.session.chars().take(8).collect::<String>(), ev.agent, ev.event, what).is_err() {
                        return;
                    }
                }
            }
        }
        first = false;
        std::thread::sleep(Duration::from_millis(200));
    }
}
