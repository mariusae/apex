
//! `apex tool agent ARGS`: the tool, its hooks, and the logs as text.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::agent::{self, Agents, State};
use crate::event::{self, Tail};
use crate::{hook, install, win};

fn usage() -> ! {
    eprintln!("usage: apex tool agent [-cwd DIR] [-a] [-all] [-thoughts] [-quiet]");
    eprintln!("       apex tool agent install|uninstall [claude|codex|muse]...");
    eprintln!("       apex tool agent ls | wait ID | events [-all]");
    eprintln!("       apex tool agent hook claude|codex|muse");
    std::process::exit(2);
}

/// `apex tool agent` with `args` (what follows `agent`), in session
/// `session` of the daemon at `socket`.
pub fn run(socket: &Path, session: &str, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("hook") => {
            let agent = args.get(1).map(String::as_str).unwrap_or_else(|| usage());
            std::process::exit(hook::run(agent));
        }
        Some(cmd @ ("install" | "uninstall")) => {
            let which: Vec<&str> = if args.len() > 1 { args[1..].iter().map(String::as_str).collect() } else { install::AGENTS.to_vec() };
            for a in &which {
                if !install::AGENTS.contains(a) {
                    return Err(format!("{a}: not an agent I know; claude, codex or muse"));
                }
            }
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| usage()));
            let res = match cmd {
                "install" => {
                    let exe = install::invoked_as(std::env::args_os().next(), std::env::current_dir().ok(), std::env::var_os("PATH")).or_else(|| std::env::current_exe().ok()).ok_or("where am I?")?;
                    install::install(&home, &exe, &which)
                }
                _ => install::uninstall(&home, &which),
            };
            for l in res? {
                println!("{l}");
            }
        }
        Some("ls") => {
            let agents = read_all(&event::dir());
            let home = std::env::var("HOME").ok();
            let (header, blocks, footer) = agent::pane(&agents.ordered(), event::now_ms(), home.as_deref(), None, None);
            let (text, _) = agent::pane_text(&header, &blocks, &footer);
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
            let opts = parse_pane(args);
            win::Pane::run(socket, session, opts).map_err(|e| e.0)?;
        }
    }
    Ok(())
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

/// `apex tool agent wait ID`: until the agent named (by its id, a prefix of
/// it, or its kind when there is one such) is no longer working. The
/// exit status is its state: 0 its turn is over and the next prompt is
/// yours, 1 it is asking something, 2 the turn failed, 3 it is gone, 4
/// no such agent. What a script chains on.
fn wait(dir: &std::path::Path, id: &str) -> i32 {
    let find = |agents: &Agents| -> Option<String> {
        if let Some(a) = agents.by_id(id) {
            return Some(a.session.clone());
        }
        let kinds: Vec<&agent::Agent> = agents.ordered().into_iter().filter(|a| a.kind == id).collect();
        match kinds.as_slice() {
            [one] => Some(one.session.clone()),
            _ => None,
        }
    };
    let agents = read_all(dir);
    let Some(session) = find(&agents) else {
        eprintln!("apex tool agent: wait {id}: no such agent");
        return 4;
    };
    let mut log = Tail::new(event::log_path(dir, &session));
    let mut agent = agent::Agent::new(&session);
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

/// `apex tool agent events`: the events as they land, a line each -- when,
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
                    let what = ev.title.as_deref().or(ev.kind.as_deref()).or(ev.text.as_deref()).map(crate::transcript::brief).unwrap_or_default();
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
