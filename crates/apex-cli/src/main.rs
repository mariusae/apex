//! `apex`: the command. Scripts and tools never see the wire; they see
//! this. Every subcommand attaches to a session as a tool (a follower with
//! its own replica), reads from the replica, and proposes to the leader.
//!
//! ```text
//! apex [--socket P] [--session S] server               run the daemon (foreground)
//! apex ls                                              list sessions
//! apex new-session NAME
//! apex rename-session [FROM] TO
//! apex attach [DEST/]SESSION [--stdio] [FILE...]       a UI; --stdio bridges the socket to stdin/stdout
//!                                                      DEST/SESSION: on a destination (user@host, provider:name)
//!                                                      through its provider, installing apex there first
//! apex new FILE...                                     open files in the first column
//! apex win list | win del WIN
//! apex text read WIN [--addr ADDR]
//! apex edit WIN PROGRAM
//! apex sel WIN [Q0 Q1]
//! apex exec [WIN] COMMAND                              as if B2
//! apex events [--shard S]                              entries as JSON lines, forever
//! apex term new | term send TERM TEXT | term read TERM
//! apex plumb TEXT
//! apex env [KEY=VALUE ...]                              set the session's environment (none: show it)
//! apex label TEXT                                       name this terminal's window (plan9port's label)
//! apex awd [LABEL]                                      name it pwd/-LABEL (plan9port's awd)
//! ```
//!
//! `WIN` is a window id or a unique substring of a window's name.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::{default_socket, Daemon};
use apex_server::proto::{ClientMsg, ServerMsg};
use apex_server::remote::Remote;
use apex_server::Proposal;

const TIMEOUT: Duration = Duration::from_secs(10);

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut socket = std::env::var("APEX_SOCKET").map(PathBuf::from).unwrap_or_else(|_| default_socket());
    // in a terminal apex runs, `apexsession` names the session it is in
    let mut session = std::env::var("apexsession").or_else(|_| std::env::var("APEX_SESSION")).unwrap_or_else(|_| "default".into());
    let mut ensure = false;
    loop {
        if args.len() >= 2 && (args[0] == "--socket" || args[0] == "--session") {
            let v = args.remove(1);
            match args.remove(0).as_str() {
                "--socket" => socket = PathBuf::from(v),
                _ => session = v,
            }
        } else if args.first().is_some_and(|a| a == "--ensure-server") {
            // start the daemon first if it is not running (what a remote
            // `apex ls` wants)
            args.remove(0);
            ensure = true;
        } else {
            break;
        }
    }
    if ensure {
        if let Err(e) = ensure_server(&socket, &session) {
            eprintln!("apex: {e}");
            std::process::exit(1);
        }
    }
    let Some(cmd) = args.first().cloned() else { usage() };
    let rest = &args[1..];
    let r = match cmd.as_str() {
        "server" => server(&socket, &session),
        "ls" => ls(&socket, &session),
        "new-session" => new_session(&socket, &session, rest),
        "rename-session" => rename_session(&socket, &session, rest),
        "attach" => attach(&socket, &session, rest),
        "new" => new(&socket, &session, rest),
        "win" => win(&socket, &session, rest),
        "text" => text(&socket, &session, rest),
        "edit" => edit(&socket, &session, rest),
        "sel" => sel(&socket, &session, rest),
        "exec" => exec(&socket, &session, rest),
        "events" => events(&socket, &session, rest),
        "term" => term(&socket, &session, rest),
        "plumb" => plumb(&socket, &session, rest),
        "label" => label(&rest.join(" ")),
        "env" => env_cmd(&socket, &session, rest),
        "awd" => awd(rest),
        _ => usage(),
    };
    if let Err(e) = r {
        eprintln!("apex {cmd}: {e}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("usage: apex [--socket P] [--session S] server|ls|new-session|attach|new|win|text|edit|sel|exec|events|term|plumb|label|awd|env ...");
    std::process::exit(2);
}

type R = Result<(), String>;

fn tool(socket: &Path, session: &str) -> Result<Remote, String> {
    Remote::connect_as(socket, session, "apex-cli", AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))
}

/// Pump until `done` or the timeout.
fn wait(r: &mut Remote, mut done: impl FnMut(&Remote) -> bool) -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if done(r) {
            return Ok(());
        }
        match r.step(Duration::from_millis(20)) {
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("connection closed".into()),
        }
    }
    if done(r) {
        Ok(())
    } else {
        Err("timed out".into())
    }
}

// ---- server, sessions -----------------------------------------------------------

fn server(socket: &Path, session: &str) -> R {
    if let Some(d) = socket.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    eprintln!("apex server: session {session} on {}", socket.display());
    Daemon::run(socket, session).map_err(|e| e.to_string())
}

/// Start a daemon in the background if the socket does not answer.
fn ensure_server(socket: &Path, session: &str) -> R {
    if UnixStream::connect(socket).is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .args(["--socket", &socket.to_string_lossy(), "--session", session, "server"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("start server: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("server did not start".into())
}

// these three talk to the daemon without attaching to any session

fn ls(socket: &Path, _session: &str) -> R {
    for s in apex_server::remote::list_sessions(socket).map_err(|e| format!("{}: {e}", socket.display()))? {
        println!("{s}");
    }
    Ok(())
}

fn rename_session(socket: &Path, session: &str, args: &[String]) -> R {
    let (from, to) = match args {
        [a, b] => (a.clone(), b.clone()),
        [b] => (session.to_string(), b.clone()),
        _ => return Err("rename-session [FROM] TO".into()),
    };
    apex_server::remote::rename_session(socket, &from, &to).map_err(|e| e.to_string())
}

fn new_session(socket: &Path, session: &str, args: &[String]) -> R {
    let name = args.first().ok_or("new-session NAME")?;
    ensure_server(socket, session)?;
    apex_server::remote::new_session(socket, name, apex_server::remote::local_init()).map_err(|e| e.to_string())
}

// ---- attach -----------------------------------------------------------------------

fn attach(socket: &Path, session: &str, args: &[String]) -> R {
    let mut args = args.to_vec();
    let stdio = args.iter().position(|a| a == "--stdio").map(|i| args.remove(i)).is_some();
    let target = if args.first().is_some_and(|a| !Path::new(a).exists()) { args.remove(0) } else { session.to_string() };
    // a URL names the destination and the session in one
    let target = match apex_server::providers::SessionUrl::parse(&target) {
        Some(u) if u.is_local() => u.session,
        Some(u) => format!("{}/{}", u.dest().unwrap_or_default(), u.session),
        None => target,
    };
    if stdio {
        // the bridge on a host: the daemon there may need starting
        ensure_server(socket, &session)?;
        return bridge(socket);
    }
    let ui = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("apex-ui");
    let status = match apex_server::providers::split_spec(&target) {
        Some((host, sess)) => {
            // remote: the UI talks to `ssh host apex attach --stdio`,
            // after our apex is put on the host
            Command::new(&ui).arg("--remote").arg(host).arg("--session").arg(sess).args(&args).status()
        }
        None => {
            ensure_server(socket, &target)?;
            Command::new(&ui).arg("--attach").arg(socket).arg("--session").arg(&target).args(&args).status()
        }
    }
    .map_err(|e| format!("{}: {e}", ui.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("apex-ui exited with {status}"))
    }
}

/// Copy bytes between the socket and stdin/stdout, both ways, until one
/// side closes. Frames pass through untouched: this is the whole remote
/// story on the server side.
fn bridge(socket: &Path) -> R {
    let s = UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    let mut from_sock = s.try_clone().map_err(|e| e.to_string())?;
    let mut to_sock = s;
    let up = std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let _ = std::io::copy(&mut stdin, &mut to_sock);
        let _ = to_sock.shutdown(std::net::Shutdown::Write);
    });
    {
        let mut stdout = std::io::stdout().lock();
        let mut buf = [0u8; 64 * 1024];
        loop {
            match from_sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).and_then(|_| stdout.flush()).is_err() {
                        break;
                    }
                }
            }
        }
    }
    // stdin may still be open (the ssh session); we are done regardless
    drop(up);
    Ok(())
}

// ---- windows and text ---------------------------------------------------------------

fn find_window(c: &Remote, spec: &str) -> Result<WindowId, String> {
    if let Ok(n) = spec.parse::<u64>() {
        let w = WindowId(n);
        return c.node.state.window(w).map(|_| w).map_err(|e| e.to_string());
    }
    let hits: Vec<WindowId> = c.node.state.windows.keys().copied().filter(|w| c.node.window_name(*w).contains(spec)).collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(format!("no window matches {spec:?}")),
        n => Err(format!("{n} windows match {spec:?}")),
    }
}

fn body_text(c: &Remote, w: WindowId) -> Result<String, String> {
    let b = c.node.view_buffer(ViewId::Body(w)).map_err(|e| e.to_string())?;
    Ok(c.node.state.buffer(b).map_err(|e| e.to_string())?.text.to_string())
}

fn new(socket: &Path, session: &str, args: &[String]) -> R {
    if args.is_empty() {
        return Err("new FILE...".into());
    }
    let mut c = tool(socket, session)?;
    let col = c.node.state.layout.cols.first().ok_or("no column")?.id;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let before = c.node.state.windows.len();
    for f in args {
        let p = cwd.join(f);
        c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: p.to_string_lossy().to_string() });
    }
    // opening an already-open file adds no window; wait for what is new
    let want = args.len();
    let _ = wait(&mut c, |r| r.node.state.windows.len() >= before + want);
    for f in args {
        let p = cwd.join(f).to_string_lossy().to_string();
        if let Ok(w) = find_window(&c, &p) {
            println!("{}\t{}", w.0, c.node.window_name(w));
        }
    }
    Ok(())
}

fn win(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    match args.first().map(|s| s.as_str()) {
        Some("list") | None => {
            for col in &c.node.state.layout.cols {
                for slot in &col.wins {
                    let w = slot.window;
                    let dirty = c.node.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| c.node.state.buffer(b).ok()).is_some_and(|b| b.dirty());
                    let mut name = c.node.window_name(w);
                    if name.is_empty() {
                        // a terminal: its tag names it
                        let tag = c.node.state.window(w).ok().map(|x| x.tag);
                        name = tag.and_then(|b| c.node.state.buffer(b).ok()).map(|b| b.text.to_string().split(' ').next().unwrap_or("").to_string()).unwrap_or_default();
                    }
                    println!("{}\t{}{}", w.0, if dirty { "*" } else { " " }, name);
                }
            }
            Ok(())
        }
        Some("del") => {
            let w = find_window(&c, args.get(1).ok_or("win del WIN")?)?;
            c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() }, TIMEOUT)?;
            Ok(())
        }
        _ => Err("win list|del WIN".into()),
    }
}

fn text(socket: &Path, session: &str, args: &[String]) -> R {
    let c = tool(socket, session)?;
    match args.first().map(|s| s.as_str()) {
        Some("read") => {
            let w = find_window(&c, args.get(1).ok_or("text read WIN [--addr A]")?)?;
            let text = body_text(&c, w)?;
            let addr = match (args.get(2).map(|s| s.as_str()), args.get(3)) {
                (Some("--addr"), Some(a)) => Some(a.clone()),
                _ => None,
            };
            let out = match addr {
                None => text,
                Some(a) => {
                    let t = Text::new(&text);
                    let (q0, q1) = c.node.selection(ViewId::Body(w)).unwrap_or((0, 0));
                    let name = c.node.window_name(w);
                    let outcome = apex_edit::Edit::new().run(&t, (q0, q1), Some(&name), &format!("{a}p")).map_err(|e| e.to_string())?;
                    outcome.output_string()
                }
            };
            print!("{out}");
            let _ = std::io::stdout().flush();
            Ok(())
        }
        _ => Err("text read WIN [--addr A]".into()),
    }
}

fn edit(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    let w = find_window(&c, args.first().ok_or("edit WIN PROGRAM")?)?;
    let program = args.get(1).ok_or("edit WIN PROGRAM")?.clone();
    c.propose(Proposal::Edit { window: w, program }, TIMEOUT)?;
    Ok(())
}

fn sel(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    let w = find_window(&c, args.first().ok_or("sel WIN [Q0 Q1]")?)?;
    let v = ViewId::Body(w);
    match (args.get(1), args.get(2)) {
        (Some(q0), Some(q1)) => {
            let q0 = q0.parse().map_err(|_| "Q0 must be a number")?;
            let q1 = q1.parse().map_err(|_| "Q1 must be a number")?;
            c.propose(Proposal::Select { view: v, q0, q1 }, TIMEOUT)?;
            Ok(())
        }
        _ => {
            let (q0, q1) = c.node.selection(v).map_err(|e| e.to_string())?;
            println!("{q0} {q1}");
            Ok(())
        }
    }
}

fn exec(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    let (ctx, text) = match args {
        [cmd] => (ExecCtx::Top, cmd.clone()),
        [w, cmd] => (ExecCtx::Window(find_window(&c, w)?), cmd.clone()),
        _ => return Err("exec [WIN] COMMAND".into()),
    };
    c.propose(Proposal::Exec { ctx, text }, TIMEOUT)?;
    Ok(())
}

fn events(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    let only: Option<String> = match args {
        [flag, s] if flag == "--shard" => Some(s.clone()),
        _ => None,
    };
    let mut out = std::io::stdout().lock();
    loop {
        let m = match c.link.rx.recv() {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        if let ServerMsg::Entries { shard, entries } = &m {
            if only.as_ref().is_none_or(|o| shard.to_string().starts_with(o)) {
                for e in entries {
                    let line = serde_json::json!({ "shard": shard.to_string(), "seq": e.seq, "attachment": e.attachment.0, "epoch": e.epoch, "op": e.op });
                    if writeln!(out, "{line}").is_err() {
                        return Ok(());
                    }
                }
                let _ = out.flush();
            }
        }
        c.handle(m);
    }
}

fn term(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    let find_term = |c: &Remote, spec: &str| -> Result<TermId, String> {
        let n: u64 = spec.parse().map_err(|_| "TERM must be a number")?;
        if c.node.state.terms.contains_key(&TermId(n)) {
            Ok(TermId(n))
        } else {
            Err(format!("no terminal {n}"))
        }
    };
    match args.first().map(|s| s.as_str()) {
        Some("new") => {
            let before: Vec<TermId> = c.node.state.terms.keys().copied().collect();
            c.propose(Proposal::Exec { ctx: ExecCtx::Top, text: "Newterm".into() }, TIMEOUT)?;
            wait(&mut c, |r| r.node.state.terms.keys().any(|t| !before.contains(t)))?;
            let t = c.node.state.terms.keys().find(|t| !before.contains(t)).unwrap();
            println!("{}", t.0);
            Ok(())
        }
        Some("send") => {
            let t = find_term(&c, args.get(1).ok_or("term send TERM TEXT")?)?;
            // the text is pasted; a final newline is the Enter key, since
            // shells take a pasted newline literally (bracketed paste)
            let mut text = args[2..].join(" ");
            let enter = text.ends_with('\r') || text.ends_with('\n');
            if enter {
                text.pop();
            }
            if !text.is_empty() {
                c.send(&ClientMsg::TermPaste { term: t, text });
            }
            if enter {
                c.send(&ClientMsg::TermKey { term: t, key: apex_server::TermKey { key: "enter".into(), text: Some("\r".into()), shift: false, control: false, alt: false } });
            }
            // give the daemon a moment to take it before we hang up
            let _ = c.step(Duration::from_millis(50));
            Ok(())
        }
        Some("read") => {
            let t = find_term(&c, args.get(1).ok_or("term read TERM")?)?;
            let term = &c.node.state.terms[&t];
            for row in &term.grid {
                let line: String = row.iter().map(|c| c.ch).collect();
                println!("{}", line.trim_end());
            }
            Ok(())
        }
        _ => Err("term new|send TERM TEXT|read TERM".into()),
    }
}

fn plumb(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    let text = args.join(" ");
    let before = c.node.state.windows.len();
    c.send(&ClientMsg::Plumb { ctx: ExecCtx::Top, text });
    let _ = wait(&mut c, |r| r.node.state.windows.len() > before);
    Ok(())
}

/// plan9port's `label`: name the window this terminal shows, through the
/// sequence acme's win reads (`ESC ] ; text BEL`).
fn label(text: &str) -> R {
    use std::io::Write;
    let seq = format!("\x1b];{text}\x07");
    match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(mut f) => f.write_all(seq.as_bytes()).map_err(|e| e.to_string())?,
        Err(_) => {
            let mut out = std::io::stdout();
            out.write_all(seq.as_bytes()).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// plan9port's `awd [label]`: name the window `pwd/-label`, the label
/// being the host unless given.
fn awd(args: &[String]) -> R {
    let sys = match args {
        [] => apex_server::term::sysname(),
        [s] if !s.starts_with('-') => s.clone(),
        _ => return Err("usage: awd [label]".into()),
    };
    let p = std::env::current_dir().map_err(|e| e.to_string())?.display().to_string();
    label(&format!("{p}{}-{sys}", if p.ends_with('/') { "" } else { "/" }))
}

/// The session's environment: what its terminals and commands get beyond
/// the daemon's own. `KEY=VALUE` sets; nothing prints it.
fn env_cmd(socket: &Path, session: &str, args: &[String]) -> R {
    let mut c = tool(socket, session)?;
    let set: Vec<(String, String)> = args
        .iter()
        .map(|a| a.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).ok_or_else(|| format!("env: {a}: not KEY=VALUE")))
        .collect::<Result<_, _>>()?;
    let show = set.is_empty();
    let vars = c.env(set, TIMEOUT)?;
    if show {
        for (k, v) in vars {
            println!("{k}={v}");
        }
    }
    Ok(())
}
