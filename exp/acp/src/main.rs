//! apex-acp: an experiment. An Agent Client Protocol client that lives in
//! an apex window, built on the public tool API only.
//!
//! The agent (any ACP agent: `claude-agent-acp`, `codex-acp`, ...) is
//! started as a child; its session is a window named `DIR/-agent`,
//! with `+run` and `+transcript` beside it for what it puts out. What
//! the agent says streams into the window; what the user types at the
//! end of it is the next prompt, sent by `Send`, which the window puts
//! in its own tag: Enter is Enter, so a prompt is as many lines as it
//! wants, and a `Send` during a turn queues behind it. What the agent
//! says begins with a `•`, a message to a bullet, so that what the
//! window keeps is a line a step of the work; what the user sent is
//! unmarked and what apex itself has to say `–`; `~` ends a turn, and
//! what stands under it is the user's, so a prompt wants no mark of
//! its own. The cursor rides the output
//! (`insert_following`), so a prompt is typed without clicking and the
//! window follows without scrolling, while a draft keeps its cursor. What it does is one line
//! at the end saying what is going on now, written over as it changes
//! and gone when the reply comes -- the agent's own words for the
//! call, cut to the one line, and the only machinery this window
//! keeps.
//! `Transcript` (or `-transcript`) opens a window of its own holding
//! the whole of it: the tool calls ticked off in place, their file
//! locations plumbable and their edits shown as diffs, kept whether or
//! not a window is showing them. When the agent
//! asks permission, the line offers `Allow Always Deny Never` to B2,
//! and stays as the record of what was decided. `Interrupt`
//! ends the turn, and anything waiting behind it. The window's handle pulses while the agent
//! works, and rests while it waits for an answer. `Preview` toggles a
//! page beside it holding the last finished reply, rendered. The agent
//! reads files through apex when they are open (unsaved edits
//! included); its writes go back the same way, and the commands it runs
//! (§ terminals) go into a window of their own, `Stop` ending them.
//! `Resume` says what sessions this directory has had -- the id, when
//! it was last worked in, what it is about -- and B3 on an id (or
//! `Resume ID`, or `-resume` for the newest) takes that one up again:
//! the agent replays the whole of it, and the window is rebuilt from
//! what comes, since the agent's record is the one that outlived us.
//!
//!     apex-acp [claude|codex|CMD] [-cwd DIR] [-thoughts] [-transcript] [-resume]
//!
//! The agent is the argument, there being one thing to say: either of
//! those short names, or a command of its own. With none it is
//! `$APEX_ACP_AGENT`, else Anthropic's Claude adapter:
//! `claude-agent-acp` when it is installed, else npx fetching
//! `@agentclientprotocol/claude-agent-acp`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::mpsc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    AuthMethodId, AuthenticateRequest, CancelNotification, ClientCapabilities, ContentBlock, CreateTerminalRequest,
    CreateTerminalResponse, FileSystemCapabilities, Implementation, InitializeRequest, KillTerminalRequest, KillTerminalResponse,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PermissionOption, PermissionOptionKind, Plan, PlanEntryStatus,
    PromptRequest, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest, SelectedPermissionOutcome,
    MessageId, SessionId, SessionModeId, SessionModeState, SessionNotification, SessionUpdate, SetSessionModeRequest, StopReason,
    TerminalExitStatus, TerminalId, TerminalOutputRequest, TerminalOutputResponse, TextContent, ToolCallContent, ToolCallLocation,
    ToolCallStatus, ToolCallUpdate, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, Responder};
use apex_tool::{Event, Rule, RuleId, Tool, WindowId};

mod term;

/// What the agent side tells the window side.
enum Out {
    /// The agent answered `initialize`.
    Hello { agent: String, auth: Vec<(String, String)>, can: Can },
    /// A session could not be started: `Login` must come first.
    NeedsAuth(String),
    /// A session is going: its modes, and the one it is in.
    Ready { modes: Vec<(String, String)>, mode: Option<String> },
    /// The sessions this directory has had, newest first.
    Sessions(Vec<Past>),
    /// A past session is being taken up: what it says of itself is
    /// coming, and is gathered until the whole of it is here.
    Replay,
    /// It is here: the window is that session's now. `replayed` says
    /// whether the transcript came with it.
    Resumed { past: Past, replayed: bool, modes: Vec<(String, String)>, mode: Option<String> },
    /// It is not: the window keeps the session it had.
    NoResume(String),
    /// A mode we asked for was taken (agents do not all say so).
    Mode(String),
    /// Something worth a line of its own.
    Note(String),
    Update(SessionUpdate),
    Permission { req: RequestPermissionRequest, responder: Responder<RequestPermissionResponse> },
    Read { req: ReadTextFileRequest, responder: Responder<ReadTextFileResponse> },
    Write { req: WriteTextFileRequest, responder: Responder<WriteTextFileResponse> },
    /// The agent wants a command run, or news of one it is running.
    Term(TermReq),
    /// A turn ended, well or badly.
    Turn(Result<StopReason, String>),
    /// Prompts that were waiting their turn went with an `Interrupt`,
    /// and no turn of theirs is coming.
    Dropped(usize),
    /// The agent side is over.
    Gone(String),
}

/// The `terminal/*` methods, which the window side answers: it owns the
/// commands, since it is what shows them.
enum TermReq {
    Create { req: CreateTerminalRequest, responder: Responder<CreateTerminalResponse> },
    Output { req: TerminalOutputRequest, responder: Responder<TerminalOutputResponse> },
    /// Answered when the command is over, which may be a long time.
    Wait { req: WaitForTerminalExitRequest, responder: Responder<WaitForTerminalExitResponse> },
    Kill { req: KillTerminalRequest, responder: Responder<KillTerminalResponse> },
    Release { req: ReleaseTerminalRequest, responder: Responder<ReleaseTerminalResponse> },
}

/// What the window side tells the agent side.
enum In {
    Prompt(String),
    /// Stop the turn going, and drop whatever was queued behind it.
    Interrupt,
    Mode(String),
    Auth(String),
    /// What sessions this directory has had; the answer is `Out::Sessions`.
    List,
    /// Take one of them up again.
    Load(Which),
}

/// Which past session to take up.
enum Which {
    /// The one most lately worked in, which `-resume` means.
    Newest,
    Id(String),
}

/// A session the agent has had here before.
#[derive(Clone)]
struct Past {
    id: String,
    title: Option<String>,
    /// ISO 8601, when the agent keeps one.
    when: Option<String>,
}

/// What the agent said at `initialize` it can do with the sessions it
/// has had before.
#[derive(Clone, Copy, Default)]
struct Can {
    /// `session/list`.
    list: bool,
    /// `session/load`: the session and the whole of its transcript.
    load: bool,
    /// `session/resume`: the session, without being able to say it again.
    resume: bool,
}

struct Opts {
    agent: Option<String>,
    cwd: PathBuf,
    thoughts: bool,
    transcript: bool,
    resume: bool,
}

fn usage() -> ! {
    eprintln!("usage: apex-acp [claude|codex|CMD] [-cwd DIR] [-thoughts] [-transcript] [-resume]");
    std::process::exit(2);
}

fn parse_args() -> Opts {
    let mut opts = Opts { agent: std::env::var("APEX_ACP_AGENT").ok().filter(|s| !s.is_empty()), cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")), thoughts: false, transcript: false, resume: false };
    let mut args = std::env::args().skip(1);
    let mut said = false; // the agent is said once, or not at all
    while let Some(a) = args.next() {
        match a.as_str() {
            "-cwd" | "--cwd" => {
                let d = PathBuf::from(args.next().unwrap_or_else(|| usage()));
                opts.cwd = if d.is_absolute() { d } else { opts.cwd.join(d) };
            }
            "-thoughts" | "--thoughts" => opts.thoughts = true,
            "-transcript" | "--transcript" => opts.transcript = true,
            "-resume" | "--resume" => opts.resume = true,
            // the one thing this takes without a flag to name it
            _ if !a.starts_with('-') && !said => {
                said = true;
                opts.agent = Some(a);
            }
            _ => usage(),
        }
    }
    opts
}

fn main() {
    let opts = parse_args();
    let (out_tx, out_rx) = mpsc::channel::<Out>();
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<In>();

    // the window side: a thread of its own, since the tool API is synchronous
    let cwd = opts.cwd.clone();
    let (thoughts, transcript) = (opts.thoughts, opts.transcript);
    let name = label(opts.agent.as_deref());
    let window = std::thread::spawn(move || match Win::run(cwd, name, thoughts, transcript, out_rx, in_tx) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("apex-acp: {}", e.0);
            1
        }
    });

    // the agent side: tokio on the main thread
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
    let res = rt.block_on(agent_side(opts, out_tx.clone(), in_rx));
    let why = match res {
        Ok(()) => "the agent is gone".to_string(),
        Err(e) => format!("agent: {e}"),
    };
    let _ = out_tx.send(Out::Gone(why));
    // the window stays until it is deleted
    let code = window.join().unwrap_or(1);
    std::process::exit(code);
}

// ---- finding the agent -------------------------------------------------------

/// What a window of this agent's is called: `DIR/-claude`. A program's
/// window is named for it after a `-`, as win's are; `+X` is for
/// auxiliary output, which is what this one's `+run` and `+transcript`
/// beside it are.
fn label(spec: Option<&str>) -> String {
    match spec.map(str::trim) {
        None | Some("") | Some("claude") => "claude".to_string(),
        Some("codex") => "codex".to_string(),
        Some(cmd) => {
            // the agent, not what runs it: `python3 agent.py` is agent
            const RUNNERS: [&str; 10] = ["python", "python3", "node", "npx", "bun", "bunx", "deno", "uv", "uvx", "sh"];
            let word = cmd
                .split_whitespace()
                .find(|w| {
                    let stem = Path::new(w).file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    !w.starts_with('-') && !RUNNERS.contains(&stem)
                })
                .unwrap_or("agent");
            let word = word.rsplit('/').next().unwrap_or(word);
            let word = word.split('@').find(|p| !p.is_empty()).unwrap_or("agent");
            Path::new(word).file_stem().and_then(|s| s.to_str()).unwrap_or("agent").to_string()
        }
    }
}

/// The Claude adapter, and the others published the same way.
const CLAUDE: (&str, &str) = ("claude-agent-acp", "@agentclientprotocol/claude-agent-acp@latest");
const CODEX: (&str, &str) = ("codex-acp", "@agentclientprotocol/codex-acp@latest");

/// The agent to run: what was asked for, else the Claude adapter.
fn resolve(spec: Option<&str>) -> Result<(AcpAgent, String), String> {
    match spec.map(str::trim) {
        None | Some("") | Some("claude") => adapter(CLAUDE),
        Some("codex") => adapter(CODEX),
        Some(cmd) => AcpAgent::from_str(cmd).map(|a| (a, cmd.to_string())).map_err(|e| e.to_string()),
    }
}

/// An adapter published on npm: the command itself when it is installed,
/// else npx, which may be one someone else brought along.
fn adapter((bin, pkg): (&str, &str)) -> Result<(AcpAgent, String), String> {
    if let Some(p) = on_path(bin) {
        return Ok((AcpAgent::new(AcpAgentConfig::new(p)), bin.to_string()));
    }
    let (npx, node_bin) = npx().ok_or_else(|| format!("{bin} is not installed and there is no npx to fetch it with: install node, or name a command of your own"))?;
    let mut cfg = AcpAgentConfig::new(&npx).args(["-y", pkg]);
    if let Some(dir) = node_bin {
        // an npx that is not on PATH needs its node found by name
        let path = std::env::var("PATH").unwrap_or_default();
        cfg = cfg.env("PATH", format!("{}:{path}", dir.display()));
    }
    Ok((AcpAgent::new(cfg), format!("npx {pkg}")))
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// npx, and the bin directory to put on PATH when it is not there
/// already: `$APEX_ACP_NODE_BIN`, else a node another program brought
/// (Zed keeps one for its own agents).
fn npx() -> Option<(PathBuf, Option<PathBuf>)> {
    if let Some(p) = on_path("npx") {
        return Some((p, None));
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("APEX_ACP_NODE_BIN").map(|d| vec![PathBuf::from(d)]).unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        let zed = PathBuf::from(&home).join("Library/Application Support/Zed/node");
        if let Ok(rd) = std::fs::read_dir(&zed) {
            let mut found: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("bin")).filter(|p| p.join("npx").is_file()).collect();
            found.sort();
            dirs.extend(found.into_iter().rev());
        }
    }
    dirs.into_iter().find(|d| d.join("npx").is_file()).map(|d| (d.join("npx"), Some(d)))
}

// ---- the agent side ----------------------------------------------------------

async fn agent_side(opts: Opts, out: mpsc::Sender<Out>, mut inbox: tokio::sync::mpsc::UnboundedReceiver<In>) -> Result<(), agent_client_protocol::Error> {
    let (agent, how) = resolve(opts.agent.as_deref()).map_err(|e| agent_client_protocol::Error::into_internal_error(std::io::Error::other(e)))?;
    let _ = out.send(Out::Note(format!("starting {how}…")));
    let agent = match std::env::var_os("APEX_ACP_DEBUG") {
        Some(_) => agent.with_debug(|line, dir| eprintln!("acp {dir:?}: {line}")),
        None => agent,
    };
    let (o1, o2, o3, o4) = (out.clone(), out.clone(), out.clone(), out.clone());
    let (t1, t2, t3, t4, t5) = (out.clone(), out.clone(), out.clone(), out.clone(), out.clone());
    let cwd = opts.cwd.clone();
    let resume = opts.resume;
    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |n: SessionNotification, _cx| {
                let _ = o1.send(Out::Update(n.update));
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |req: RequestPermissionRequest, responder, _cx| {
                // answered from the window, when the user says
                let _ = o2.send(Out::Permission { req, responder });
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: ReadTextFileRequest, responder, _cx| {
                let _ = o3.send(Out::Read { req, responder });
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: WriteTextFileRequest, responder, _cx| {
                let _ = o4.send(Out::Write { req, responder });
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: CreateTerminalRequest, responder, _cx| {
                let _ = t1.send(Out::Term(TermReq::Create { req, responder }));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: TerminalOutputRequest, responder, _cx| {
                let _ = t2.send(Out::Term(TermReq::Output { req, responder }));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: WaitForTerminalExitRequest, responder, _cx| {
                // held until the command ends: the window side answers
                let _ = t3.send(Out::Term(TermReq::Wait { req, responder }));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: KillTerminalRequest, responder, _cx| {
                let _ = t4.send(Out::Term(TermReq::Kill { req, responder }));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: ReleaseTerminalRequest, responder, _cx| {
                let _ = t5.send(Out::Term(TermReq::Release { req, responder }));
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |conn: ConnectionTo<Agent>| async move {
            let caps = ClientCapabilities::new()
                .fs(FileSystemCapabilities::new().read_text_file(true).write_text_file(true))
                .terminal(true);
            let init = conn
                .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(caps).client_info(Implementation::new("apex-acp", env!("CARGO_PKG_VERSION"))))
                .block_task()
                .await?;
            let name = init.agent_info.map(|i| format!("{} {}", i.name, i.version)).unwrap_or_else(|| "the agent".to_string());
            let auth: Vec<(String, String)> = init.auth_methods.iter().map(|m| (m.id().0.to_string(), m.name().to_string())).collect();
            let sc = &init.agent_capabilities.session_capabilities;
            let can = Can { list: sc.list.is_some(), load: init.agent_capabilities.load_session, resume: sc.resume.is_some() };
            let _ = out.send(Out::Hello { agent: name, auth, can });
            // the newest session here when -resume asked for one, else
            // a new one, logging in first when the agent asks for it
            let mut taken = match resume {
                true => restore(&conn, &cwd, Which::Newest, can, &out).await,
                false => None,
            };
            if taken.is_none() {
                let session = loop {
                    match conn.send_request(NewSessionRequest::new(cwd.clone())).block_task().await {
                        Ok(s) => break s,
                        Err(e) => {
                            let _ = out.send(Out::NeedsAuth(e.to_string()));
                            loop {
                                match inbox.recv().await {
                                    Some(In::Auth(m)) => match conn.send_request(AuthenticateRequest::new(AuthMethodId::new(m))).block_task().await {
                                        Ok(_) => break,
                                        Err(e) => {
                                            let _ = out.send(Out::Note(format!("login: {e}")));
                                        }
                                    },
                                    Some(_) => {}
                                    None => return Ok(()),
                                }
                            }
                        }
                    }
                };
                let (modes, mode) = modes_of(&session.modes);
                let _ = out.send(Out::Ready { modes, mode });
                taken = Some(session.session_id);
            }
            // a resume replaces it, so it is the session of the moment
            let mut sid = taken.expect("a session");

            let mut queue: VecDeque<String> = VecDeque::new();
            loop {
                let text = match queue.pop_front() {
                    Some(t) => t,
                    None => match inbox.recv().await {
                        Some(In::Prompt(t)) => t,
                        Some(In::Mode(m)) => {
                            match conn.send_request(SetSessionModeRequest::new(sid.clone(), SessionModeId::new(m.clone()))).block_task().await {
                                Ok(_) => {
                                    let _ = out.send(Out::Mode(m));
                                }
                                Err(e) => {
                                    let _ = out.send(Out::Note(format!("mode: {e}")));
                                }
                            }
                            continue;
                        }
                        Some(In::List) => {
                            match sessions(&conn, &cwd, can).await {
                                Ok(v) => {
                                    let _ = out.send(Out::Sessions(v));
                                }
                                Err(e) => {
                                    let _ = out.send(Out::Note(format!("sessions: {e}")));
                                }
                            }
                            continue;
                        }
                        Some(In::Load(w)) => {
                            if let Some(s) = restore(&conn, &cwd, w, can, &out).await {
                                sid = s;
                            }
                            continue;
                        }
                        Some(In::Interrupt) | Some(In::Auth(_)) => continue,
                        None => return Ok(()),
                    },
                };
                let prompt = PromptRequest::new(sid.clone(), vec![ContentBlock::Text(TextContent::new(text))]);
                let mut sent = Box::pin(conn.send_request(prompt).block_task());
                let res = loop {
                    tokio::select! {
                        r = &mut sent => break r,
                        m = inbox.recv() => match m {
                            // the turn stops, and so does what was
                            // waiting to go after it: an interrupt is
                            // an end to the work, not to one turn of it
                            Some(In::Interrupt) => {
                                let _ = conn.send_notification(CancelNotification::new(sid.clone()));
                                if !queue.is_empty() {
                                    let _ = out.send(Out::Dropped(queue.len()));
                                    queue.clear();
                                }
                            }
                            Some(In::Prompt(t)) => queue.push_back(t),
                            // mid-turn: sent on a task of its own, so the
                            // turn's own response can still come in
                            Some(In::Mode(m)) => {
                                let (c, s, o) = (conn.clone(), sid.clone(), out.clone());
                                let _ = conn.spawn(async move {
                                    match c.send_request(SetSessionModeRequest::new(s, SessionModeId::new(m.clone()))).block_task().await {
                                        Ok(_) => {
                                            let _ = o.send(Out::Mode(m));
                                        }
                                        Err(e) => {
                                            let _ = o.send(Out::Note(format!("mode: {e}")));
                                        }
                                    }
                                    Ok(())
                                });
                            }
                            // the listing is only read, so it can be
                            // had mid-turn; taking a session up is not
                            Some(In::List) => {
                                let (c, d, o) = (conn.clone(), cwd.clone(), out.clone());
                                let _ = conn.spawn(async move {
                                    match sessions(&c, &d, can).await {
                                        Ok(v) => {
                                            let _ = o.send(Out::Sessions(v));
                                        }
                                        Err(e) => {
                                            let _ = o.send(Out::Note(format!("sessions: {e}")));
                                        }
                                    }
                                    Ok(())
                                });
                            }
                            Some(In::Load(_)) => {
                                let _ = out.send(Out::NoResume("the agent is working: Interrupt first".to_string()));
                            }
                            Some(In::Auth(_)) => {}
                            None => return Ok(()),
                        },
                    }
                };
                let _ = out.send(Out::Turn(res.map(|r| r.stop_reason).map_err(|e| e.to_string())));
            }
        })
        .await
}

/// The modes a session came with: the ones on offer, and the one it is in.
fn modes_of(m: &Option<SessionModeState>) -> (Vec<(String, String)>, Option<String>) {
    let modes = m.as_ref().map(|m| m.available_modes.iter().map(|x| (x.id.0.to_string(), x.name.clone())).collect()).unwrap_or_default();
    (modes, m.as_ref().map(|m| m.current_mode_id.0.to_string()))
}

/// The sessions the agent has had in this directory, newest first.
/// `session/list` filters by `cwd` itself, so nothing of a session need
/// be kept on our side: the agent already knows them all.
async fn sessions(conn: &ConnectionTo<Agent>, cwd: &Path, can: Can) -> Result<Vec<Past>, String> {
    if !can.list {
        return Err("this agent does not list the sessions it has had".to_string());
    }
    const PAGES: usize = 8;
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..PAGES {
        let req = ListSessionsRequest::new().cwd(cwd.to_path_buf()).cursor(cursor.take());
        let res = conn.send_request(req).block_task().await.map_err(|e| e.to_string())?;
        all.extend(res.sessions.iter().map(|s| Past { id: s.session_id.0.to_string(), title: s.title.clone(), when: s.updated_at.clone() }));
        match res.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    // newest first, whatever order they came in; the sort is stable, so
    // an agent that keeps no times keeps its own order
    all.sort_by(|a, b| b.when.cmp(&a.when));
    Ok(all)
}

/// Take up a past session. `session/load` replays the whole of it as
/// ordinary updates and the window rebuilds itself from them; an agent
/// that can only reattach (`session/resume`) hands back the session
/// without its transcript. Either way the news goes to the window,
/// which is told first that something is coming, so that what arrives
/// is gathered rather than added to a stale transcript.
async fn restore(conn: &ConnectionTo<Agent>, cwd: &Path, which: Which, can: Can, out: &mpsc::Sender<Out>) -> Option<SessionId> {
    if !can.load && !can.resume {
        let _ = out.send(Out::NoResume("this agent cannot take a session up again".to_string()));
        return None;
    }
    let past = match which {
        Which::Id(id) => Past { id, title: None, when: None },
        Which::Newest => match sessions(conn, cwd, can).await {
            Ok(v) => match v.into_iter().next() {
                Some(p) => p,
                None => {
                    let _ = out.send(Out::NoResume("no session has been here before".to_string()));
                    return None;
                }
            },
            Err(e) => {
                let _ = out.send(Out::NoResume(e));
                return None;
            }
        },
    };
    let sid = SessionId::new(past.id.clone());
    let _ = out.send(Out::Replay);
    let (dir, id) = (cwd.to_path_buf(), sid.clone());
    let modes = match can.load {
        true => conn.send_request(LoadSessionRequest::new(id, dir)).block_task().await.map(|r| r.modes).map_err(|e| e.to_string()),
        false => conn.send_request(ResumeSessionRequest::new(id, dir)).block_task().await.map(|r| r.modes).map_err(|e| e.to_string()),
    };
    match modes {
        Ok(m) => {
            let (modes, mode) = modes_of(&m);
            let _ = out.send(Out::Resumed { past, replayed: can.load, modes, mode });
            Some(sid)
        }
        Err(e) => {
            let _ = out.send(Out::NoResume(e));
            None
        }
    }
}

// ---- the window side ---------------------------------------------------------

/// A whole exchange: what was asked, and the agent's answer to it --
/// its last word on the turn. The page shows one.
struct Exchange {
    asked: String,
    said: String,
}

/// A permission the agent is waiting on.
struct Pending {
    /// Anchor of its line's glyph.
    key: String,
    /// Anchor of the words offered, and how many they are: the decision
    /// takes their place.
    offer: String,
    offered: usize,
    options: Vec<PermissionOption>,
    responder: Responder<RequestPermissionResponse>,
}

/// A command the agent is running, and who is waiting for it to end.
struct Term {
    run: term::Run,
    waiting: Vec<Responder<WaitForTerminalExitResponse>>,
    /// Whether its end has been told: to the window, and to the waiters.
    ended: bool,
}

/// The whole of what the session says, machinery and all: the tool
/// calls, where they worked, their diffs, the plans, the questions and
/// what was answered. It is kept whether or not a window is showing
/// it, so `Transcript` opens on the whole of it rather than on what
/// happens next, and a window closed and opened again says the same.
/// Nothing is shown at: it is written at the end and the window
/// follows its output by acme's rule, as a win does -- scrolled back
/// to read something, it holds still.
struct Trans {
    /// The window showing it, while `Transcript` has one open.
    w: Option<WindowId>,
    /// What it says. The window, when there is one, says the same.
    text: String,
    /// Characters in it, which the window counts in too.
    len: usize,
    col0: bool,
    blank: bool,
    /// Offsets of status glyphs, by tool call (or permission) id.
    anchors: HashMap<String, usize>,
    /// What each tool call has already put down, in order: an update
    /// carries the whole list again, and only what is new of it is
    /// worth a line.
    said: HashMap<String, Vec<String>>,
    /// The plan block, replaced in place.
    plan: Option<(usize, usize)>,
}

impl Default for Trans {
    /// Empty, and so at the start of a line: nothing stands before the
    /// first thing written.
    fn default() -> Trans {
        Trans { w: None, text: String::new(), len: 0, col0: true, blank: true, anchors: HashMap::new(), said: HashMap::new(), plan: None }
    }
}

struct Win {
    t: Tool,
    w: WindowId,
    /// Characters in the body, as far as we know.
    len: usize,
    /// Where our output goes; what follows is the user's draft.
    mark: usize,
    /// Whether our output stands at the start of a line, and whether
    /// the line before that is empty: what the end marker is set off by.
    col0: bool,
    blank: bool,
    /// Offsets of the status glyphs of questions asked here.
    anchors: HashMap<String, usize>,
    /// Titles of tool calls seen.
    titles: HashMap<String, String>,
    /// The plan block, replaced in place.
    plan: Option<(usize, usize)>,
    /// The action line, while there is one: the one line of machinery
    /// this window keeps, saying what is going on now.
    action: Option<(usize, usize)>,
    /// The line marking the end of what has been said, while there is
    /// one: only once the session is going, and not while it is
    /// answering. The blank line setting it off goes in with it, so
    /// that taking it away leaves what was there and no more.
    tail: Option<(usize, usize)>,
    tail_blank: bool,
    ready: bool,
    /// The whole of it, in a window of its own when `Transcript` opens
    /// one.
    tr: Trans,
    pending: VecDeque<Pending>,
    /// Prompts sent and not yet answered. More than one may be out: a
    /// `Send` during a turn queues behind it, and the agent takes them
    /// in order, so the window is busy until the last of them ends.
    sent: usize,
    thoughts: bool,
    verbs: HashMap<RuleId, &'static str>,
    /// The session's directory, which paths are shown relative to.
    cwd: PathBuf,
    /// The modes the agent offers (id, name), and the one it is in.
    modes: Vec<(String, String)>,
    mode: Option<String>,
    /// How the agent can be logged in to, when it is not.
    auth: Vec<(String, String)>,
    /// What the agent calls itself, so a window rebuilt on a resume can
    /// say it again, and what it can do with its past sessions.
    agent: String,
    can: Can,
    /// What a session being taken up has said of itself so far. The
    /// window is rebuilt from the whole of it at once: until then what
    /// stands here is the session we still have, and a resume that
    /// fails leaves it alone.
    replay: Option<Vec<SessionUpdate>>,
    /// Whether the transcript is being rebuilt from a replay: it is not
    /// a live turn, so it is not scrolled to a hundred times on the way.
    replaying: bool,
    /// Whether the last thing written was the user's own words, which a
    /// replay is the only source of.
    in_user: bool,
    /// Whether the agent's words are still the message they began in,
    /// and the message its chunks are of when the agent says which
    /// (ACP's `messageId`): a message begins with a bullet, so where
    /// one ends and the next starts has to be known.
    in_agent: bool,
    msg: Option<MessageId>,
    /// Whether the message going is still owed its bullet: it waits
    /// for something to stand in front of, since a message beginning
    /// with a blank line would else leave it on a line of its own.
    owed: bool,
    /// The agent's slash commands, as it last said.
    commands: Vec<(String, String)>,
    /// The past sessions as `Resume` last listed them, which is what
    /// an id B3'd in the window is looked up in.
    sessions: Vec<Past>,
    /// The rule by which B3 on a session's id takes it up.
    pick: RuleId,
    /// The message the agent is saying now: a new one takes the place
    /// of the last, since what the page shows is the answer and not the
    /// running commentary made on the way to it.
    reply: String,
    /// What is being answered: the prompts sent and not yet answered
    /// (the agent takes them in order), and, on a replay, the user's
    /// words as they come.
    prompts: VecDeque<String>,
    asked: String,
    /// The last exchange finished, which the page shows.
    last: Option<Exchange>,
    /// The page window, while `Preview` has one open.
    page: Option<WindowId>,
    /// What we last told the window about the work going on, and
    /// whether we have said it is clean since anything was written.
    pulsing: bool,
    settled: bool,
    /// The commands the agent is running, by the id we gave each.
    terms: HashMap<String, Term>,
    next_term: usize,
    /// The window every command's output goes to, and which command
    /// wrote to it last: another's output is headed afresh.
    run: Option<WindowId>,
    wrote: Option<String>,
    run_col0: bool,
    /// Whether the run window is pulsing.
    running: bool,
    inbox: tokio::sync::mpsc::UnboundedSender<In>,
}

/// The left margin: who is speaking. Three things are said in this
/// window and each is marked, so that the eye can tell them apart down
/// the column without reading a word: what apex itself has to say, what
/// the user sent, and what the agent answered. Three shapes rather than
/// three dots -- a bar, an angle, a bullet -- since nothing here has a
/// colour to tell it by, and all of a width, since a wide one would
/// stand out for no reason. The bar is an en dash rather than an em:
/// narrow enough to sit in the column with the others, and not the
/// hyphen a Markdown list begins with, which a reply is full of. What
/// the user sent wants no mark of its own: it is what stands under the
/// end of the last turn (§ `TAIL`) and above the bullets answering it.
const SYS: &str = "– ";
const AGENT: &str = "• ";

/// The end of a turn, and so the start of the next thing said: what
/// stands under it is the user's. Set off on both sides, with the
/// cursor left on the line below, which is where a prompt is typed.
/// At the end of the window it is the mark of where to type, put down
/// when the window settles and taken away by anything written after
/// it; a prompt sent under one leaves it where it is, and it is the
/// record of where that exchange began.
const TAIL: &str = "~\n\n";

/// The verbs in the window's tools menu: the ones you reach for with
/// nothing to click and nothing to say after them.
const VERBS: [&str; 5] = ["Send", "Interrupt", "Preview", "Transcript", "Stop"];

/// The rest, which work the same but are no words in the menu: some are
/// written into the window to be clicked where they stand (the four a
/// permission offers), the others mean nothing without what follows
/// them (`Mode plan`, `Resume ID`, `Login METHOD`) or are read once and
/// not wanted again (`Commands`).
const UNLISTED: [&str; 8] = ["Allow", "Always", "Deny", "Never", "Mode", "Commands", "Login", "Resume"];

impl Win {
    fn run(cwd: PathBuf, label: String, thoughts: bool, transcript: bool, out: mpsc::Receiver<Out>, inbox: tokio::sync::mpsc::UnboundedSender<In>) -> apex_tool::Result<()> {
        let mut t = Tool::attach("acp")?;
        let name = format!("{}/-{label}", cwd.display().to_string().trim_end_matches('/'));
        let w = t.new_window(&name)?;
        t.watch(w)?;
        // ours: what is in it is the agent's talk, not a file's text,
        // so the tag offers nothing to write it with and Del asks
        // nothing. `live` says something else here, and comes and goes
        let _ = t.set_owner(w, true);
        // Enter does not send here (a prompt is as many lines as it
        // wants), so the verb that does is in the tag, where B2 reaches
        // it without going by way of the menu
        let _ = t.set_tag(w, "Look Send");
        let mut verbs = HashMap::new();
        for v in VERBS {
            verbs.insert(t.offer(Rule::verb(v).window(w))?, v);
        }
        for v in UNLISTED {
            verbs.insert(t.offer(Rule::verb(v).window(w).unlisted())?, v);
        }
        // B3 on a session's id takes it up. A click stops at the
        // hyphens (they are not word characters), so a part of a uuid
        // must do as well as the whole of one; a hex word that names no
        // session of ours is handed back, and B3 does what it always
        // does with it
        let pick = t.offer(Rule::plumb().text(r"[0-9a-fA-F]{4,}(-[0-9a-fA-F]{4,})*").window(w))?;
        let mut win = Win { t, w, len: 0, mark: 0, col0: true, blank: true, anchors: HashMap::new(), titles: HashMap::new(), plan: None, action: None, tail: None, tail_blank: true, ready: false, tr: Trans::default(), pending: VecDeque::new(), sent: 0, thoughts, verbs, cwd, modes: Vec::new(), mode: None, auth: Vec::new(), agent: "the agent".to_string(), can: Can::default(), replay: None, replaying: false, in_user: false, in_agent: false, msg: None, owed: false, commands: Vec::new(), sessions: Vec::new(), pick, reply: String::new(), prompts: VecDeque::new(), asked: String::new(), last: None, page: None, pulsing: false, settled: false, terms: HashMap::new(), next_term: 1, run: None, wrote: None, run_col0: true, running: false, inbox };
        if transcript {
            win.toggle_transcript()?;
        }

        let r = win.serve(out);
        // the page is ours: it goes with us
        if let Some(p) = win.page.take() {
            let _ = win.t.delete(p);
        }
        // and so are the commands: nothing the agent started outlives
        // its window. The run window stays, as the record of what ran.
        win.terms.clear();
        r
    }

    /// Until the window goes, or the session does.
    fn serve(&mut self, out: mpsc::Receiver<Out>) -> apex_tool::Result<()> {
        let win = self;
        loop {
            // the agent's news first, then the window's
            while let Ok(o) = out.try_recv() {
                win.take(o)?;
            }
            win.tick_terms()?;
            // what the handle says follows what was written, by either
            // of us, and what is going on
            win.pulse();
            let ev = win.t.next_event(Some(Duration::from_millis(20)))?;
            if ev.is_some() && std::env::var_os("APEX_ACP_DEBUG").is_some() {
                eprintln!("acp: {ev:?} len={} mark={} busy={}", win.len, win.mark, win.busy());
            }
            match ev {
                None if !win.t.windows().iter().any(|x| x.id == win.w) => return Ok(()),
                None => {}
                Some(Event::Deleted { window }) if window == win.w => return Ok(()),
                Some(Event::Deleted { window }) if Some(window) == win.page => win.page = None,
                Some(Event::Deleted { window }) if Some(window) == win.tr.w => win.tr.w = None,
                // Del on the run window clears it; what is still running
                // goes on, into a fresh one
                Some(Event::Deleted { window }) if Some(window) == win.run => {
                    win.run = None;
                    win.wrote = None;
                    win.running = false;
                }
                Some(Event::Edit(e)) if e.window == win.w => win.edited(e.q0, e.nd, &e.text),
                Some(Event::Plumb(p)) => {
                    let taken = match win.verbs.get(&p.rule).copied() {
                        Some(v) => win.verb(v, &p.text)?,
                        None if p.rule == win.pick => win.pick_session(&p.text)?,
                        None => false,
                    };
                    win.t.answer(&p, taken)?;
                }
                Some(_) => {}
            }
        }
    }

    // ---- text bookkeeping ----

    /// Someone (we included) replaced `nd` characters at `q0` with `text`:
    /// move what we remember accordingly.
    fn edited(&mut self, q0: usize, nd: usize, text: &str) {
        let ni = text.chars().count();
        let q1 = q0 + nd;
        let shift = |p: usize| -> Option<usize> {
            if p <= q0 {
                Some(p)
            } else if p >= q1 {
                Some(p + ni - nd)
            } else {
                None
            }
        };
        // saturating: an edit may be heard of after the window has
        // been taken down and rebuilt, and means nothing then
        self.len = (self.len + ni).saturating_sub(nd);
        self.settled = false;
        self.mark = shift(self.mark).unwrap_or(q0.min(self.len));
        self.anchors.retain(|_, p| match shift(*p) {
            Some(n) => {
                *p = n;
                true
            }
            None => false,
        });
        self.plan = self.plan.and_then(|(a, b)| Some((shift(a)?, shift(b)?)));
        self.action = self.action.and_then(|(a, b)| Some((shift(a)?, shift(b)?)));
        self.tail = self.tail.and_then(|(a, b)| Some((shift(a)?, shift(b)?)));
    }

    fn splice(&mut self, q0: usize, q1: usize, text: &str) -> apex_tool::Result<()> {
        self.t.replace(self.w, q0, q1, text)?;
        self.edited(q0, q1 - q0, text);
        Ok(())
    }

    /// Output at `at`, with the cursor following it when it sits there:
    /// what is written at the end of the window carries the typing
    /// point along, so nothing has to be clicked to answer, and what is
    /// half-typed keeps its cursor while the agent writes before it.
    fn wrote(&mut self, at: usize, text: &str) -> apex_tool::Result<()> {
        self.t.insert_following(self.w, at, text)?;
        self.edited(at, 0, text);
        Ok(())
    }

    /// The cursor to the output point, whatever it was doing: where a
    /// prompt is typed. Said only where the window would otherwise
    /// leave it behind -- when the session is ready, and when what was
    /// typed has just gone -- since output alone carries it along.
    fn cursor_to_mark(&mut self) {
        let m = self.mark;
        let _ = self.t.select(self.w, m, m);
    }

    /// Output at the mark. An action line standing at the end goes
    /// first: it says what is going on now, and anything written
    /// outlives it. Code that remembers a position across a write must
    /// take it after that, so `drop_action` comes before the position,
    /// not after (see [`Win::show_plan`]).
    fn out(&mut self, s: &str) -> apex_tool::Result<()> {
        self.drop_tail()?;
        self.drop_action()?;
        self.put(s)
    }

    fn put(&mut self, s: &str) -> apex_tool::Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        let m = self.mark;
        self.wrote(m, s)?;
        self.mark = m + s.chars().count();
        self.blank = s.ends_with("\n\n") || (s == "\n" && self.col0);
        self.col0 = s.ends_with('\n');
        Ok(())
    }

    /// What the agent is doing now, in a line of its own at the end of
    /// the transcript: written over the last one, so the window does
    /// not grow by a line a tool call. The words are the agent's own
    /// for the call (ACP's `title`) -- what it would say it is doing.
    fn action(&mut self, text: &str) -> apex_tool::Result<()> {
        let line = format!("{text}\n");
        if let Some((a, b)) = self.action {
            // nothing is written after it without taking it away
            // first, so the mark is its end and stays so
            self.splice(a, b, &line)?;
            self.action = Some((a, self.mark));
            return Ok(());
        }
        self.line_start()?;
        let a = self.mark;
        self.put(&line)?;
        self.action = Some((a, self.mark));
        Ok(())
    }

    fn drop_action(&mut self) -> apex_tool::Result<()> {
        if let Some((a, b)) = self.action.take() {
            self.splice(a, b, "")?;
        }
        Ok(())
    }

    /// Mark the end of what has been said, once the session is going
    /// and while it is not answering. What is written next takes it
    /// away (`out`), so there is never more than the one, and it is
    /// never anywhere but the end of the window's own output: a draft
    /// being typed stands below it, which is the point of it.
    fn show_tail(&mut self) -> apex_tool::Result<()> {
        if self.tail.is_some() || !self.ready || self.busy() {
            return Ok(());
        }
        self.line_start()?;
        // set off by a blank line, as a turn's end leaves anyway; it
        // goes in with the marker, and comes away with it
        let a = self.mark;
        self.tail_blank = self.blank;
        if !self.blank {
            self.put("\n")?;
        }
        self.put(TAIL)?;
        self.tail = Some((a, self.mark));
        Ok(())
    }

    /// End the turn before a prompt goes in, in both: the marker the
    /// window settled with stands where it is and is the record of that
    /// exchange's beginning, and one is written where there is none --
    /// a prompt queued during a turn, or a replayed one, which never
    /// waited for the window to settle. Nothing is typed in the
    /// transcript, so it is only ever told.
    fn mark_turn(&mut self) -> apex_tool::Result<()> {
        if self.tail.take().is_none() {
            self.line_start()?;
            if !self.blank {
                self.out("\n")?;
            }
            self.out(TAIL)?;
        }
        self.tr_line_start()?;
        if !self.tr.blank {
            self.tr_out("\n")?;
        }
        self.tr_out(TAIL)
    }

    fn drop_tail(&mut self) -> apex_tool::Result<()> {
        if let Some((a, b)) = self.tail.take() {
            self.splice(a, b, "")?;
            // it began at a line start and took its blank line with it
            self.col0 = true;
            self.blank = self.tail_blank;
        }
        Ok(())
    }

    /// Output with each line begun by `prefix`.
    fn out_prefixed(&mut self, prefix: &str, s: &str) -> apex_tool::Result<()> {
        let mut buf = String::new();
        for c in s.chars() {
            if self.col0 {
                buf.push_str(prefix);
                self.col0 = false;
            }
            buf.push(c);
            if c == '\n' {
                self.col0 = true;
            }
        }
        let col0 = self.col0;
        self.out(&buf)?;
        self.col0 = col0;
        Ok(())
    }

    fn line_start(&mut self) -> apex_tool::Result<()> {
        if !self.col0 {
            self.out("\n")?;
        }
        Ok(())
    }

    // ---- the transcript ----

    /// Replace the characters `[q0, q1)` of the transcript, in what we
    /// keep of it and in the window showing it, which say the same.
    fn tr_splice(&mut self, q0: usize, q1: usize, text: &str) -> apex_tool::Result<()> {
        if let Some(w) = self.tr.w {
            // nothing is shown at: written at the end, the window
            // follows its own output and carries the dot with it, as
            // the session's window does; a rewrite in place (a status
            // glyph, the plan block) moves nothing
            match q0 == q1 {
                true => self.t.insert_following(w, q0, text)?,
                false => self.t.replace(w, q0, q1, text)?,
            }
        }
        let (a, b) = (byte_at(&self.tr.text, q0), byte_at(&self.tr.text, q1));
        self.tr.text.replace_range(a..b, text);
        // as [`Win::edited`] moves what the window remembers, and in
        // the same order: a place at the point written at stays where
        // it is, so writing at the end does not drag the end of the
        // plan block along behind it
        let (ni, nd) = (text.chars().count(), q1 - q0);
        let shift = |p: usize| -> Option<usize> {
            if p <= q0 {
                Some(p)
            } else if p >= q1 {
                Some(p + ni - nd)
            } else {
                None
            }
        };
        self.tr.len = (self.tr.len + ni).saturating_sub(nd);
        self.tr.anchors.retain(|_, p| match shift(*p) {
            Some(n) => {
                *p = n;
                true
            }
            None => false,
        });
        self.tr.plan = self.tr.plan.and_then(|(a, b)| Some((shift(a)?, shift(b)?)));
        Ok(())
    }

    /// Write at the end of the transcript.
    fn tr_out(&mut self, s: &str) -> apex_tool::Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        let end = self.tr.len;
        self.tr_splice(end, end, s)?;
        self.tr.blank = s.ends_with("\n\n") || (s == "\n" && self.tr.col0);
        self.tr.col0 = s.ends_with('\n');
        Ok(())
    }

    fn tr_line_start(&mut self) -> apex_tool::Result<()> {
        if !self.tr.col0 {
            self.tr_out("\n")?;
        }
        Ok(())
    }

    /// Write with each line begun by `prefix`.
    fn tr_prefixed(&mut self, prefix: &str, s: &str) -> apex_tool::Result<()> {
        let mut buf = String::new();
        let mut col0 = self.tr.col0;
        for c in s.chars() {
            if col0 {
                buf.push_str(prefix);
            }
            buf.push(c);
            col0 = c == '\n';
        }
        self.tr_out(&buf)?;
        self.tr.col0 = col0;
        Ok(())
    }

    /// Change a status glyph in the transcript in place.
    fn tr_glyph(&mut self, key: &str, g: &str) -> apex_tool::Result<()> {
        if let Some(&p) = self.tr.anchors.get(key) {
            self.tr_splice(p, p + 1, g)?;
        }
        Ok(())
    }

    /// Both at once: what the conversation itself says goes to the
    /// window and to the transcript, and only the machinery differs.
    fn both(&mut self, s: &str) -> apex_tool::Result<()> {
        self.out(s)?;
        self.tr_out(s)
    }

    /// `Transcript` opens a window beside this one holding the whole of
    /// what the session has said -- every tool call, where it worked,
    /// its diff -- and closes it again; Del on the window does the
    /// same. What it shows is kept either way, so it opens on the whole
    /// of the conversation and not on the rest of it.
    fn toggle_transcript(&mut self) -> apex_tool::Result<()> {
        match self.tr.w.take() {
            Some(w) => {
                let _ = self.t.delete(w);
                Ok(())
            }
            None => {
                let name = format!("{}+transcript", self.t.window_name(self.w).unwrap_or_else(|| "-agent".to_string()));
                let w = self.t.new_window(&name)?;
                // ours, as the run window is: Del does not ask about it
                let _ = self.t.set_owner(w, true);
                let _ = self.t.set_live(w, true);
                let text = std::mem::take(&mut self.tr.text);
                self.t.replace(w, 0, apex_tool::END, &text)?;
                self.tr.text = text;
                // at the end, so that what comes next carries it along
                let _ = self.t.select(w, self.tr.len, self.tr.len);
                self.tr.w = Some(w);
                Ok(())
            }
        }
    }

    /// Change a status glyph in place.
    fn glyph(&mut self, key: &str, g: &str) -> apex_tool::Result<()> {
        if let Some(&p) = self.anchors.get(key) {
            self.t.replace(self.w, p, p + 1, g)?;
        }
        Ok(())
    }

    /// The agent's message, if one was going, is over: what it says
    /// next is another, and begins with a bullet of its own.
    fn end_message(&mut self) {
        self.in_agent = false;
        self.owed = false;
    }

    /// Whether a turn of ours is still going.
    fn busy(&self) -> bool {
        self.sent > 0
    }

    /// The user's draft: what follows the mark.
    fn draft(&self) -> apex_tool::Result<String> {
        Ok(self.t.read(self.w)?.chars().skip(self.mark).collect())
    }

    // ---- the window's events ----

    fn verb(&mut self, v: &str, args: &str) -> apex_tool::Result<bool> {
        // whatever a verb writes outlives what the agent is doing now
        self.drop_action()?;
        match v {
            "Send" => self.send(Some(args.trim()).filter(|s| !s.is_empty()))?,
            "Interrupt" => {
                if self.busy() {
                    let _ = self.inbox.send(In::Interrupt);
                }
            }
            "Allow" => self.decide(PermissionOptionKind::AllowOnce)?,
            "Always" => self.decide(PermissionOptionKind::AllowAlways)?,
            "Deny" => self.decide(PermissionOptionKind::RejectOnce)?,
            "Never" => self.decide(PermissionOptionKind::RejectAlways)?,
            "Mode" => self.set_mode(args.trim())?,
            "Commands" => self.list_commands()?,
            "Login" => self.login(args.trim())?,
            "Preview" => self.toggle_page()?,
            "Transcript" => self.toggle_transcript()?,
            "Stop" => self.kill_terms()?,
            "Resume" => self.resume(args.trim())?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Send the draft (or `text`) as the next prompt. The draft stays,
    /// as the transcript's record of it. Nothing else sends: a prompt is
    /// as many lines as it wants, so Enter is Enter. During a turn this
    /// queues behind it, and the agent takes it when the turn ends.
    fn send(&mut self, text: Option<&str>) -> apex_tool::Result<()> {
        if self.replay.is_some() {
            return self.note("a session is being taken up: wait for it");
        }
        // the marker that ends what came before stays, and the prompt
        // stands under it: nothing else is needed to say whose it is
        self.mark_turn()?;
        let draft = self.draft()?;
        let prompt = match text {
            Some(t) => t.to_string(),
            None => draft.trim().to_string(),
        };
        if prompt.is_empty() {
            return Ok(());
        }
        if text.is_some() && draft.trim().is_empty() {
            // nothing typed: record the argument instead
            let m = self.mark;
            self.splice(m, self.len, &format!("{prompt}\n"))?;
        } else if !draft.ends_with('\n') {
            // the words are in the window already, typed there
            let l = self.len;
            self.splice(l, l, "\n")?;
        }
        self.mark = self.len;
        self.col0 = true;
        // the agent takes them in order, so the answers come in order
        self.prompts.push_back(prompt.clone());
        // what answers it begins after a blank line
        self.out("\n")?;
        // what was typed has gone: the cursor to the output point, so
        // that the answer carries it along rather than leaving it
        // behind in the prompt it came from
        self.cursor_to_mark();
        // the window has the prompt because it was typed there; the
        // transcript is only written to, so it is told
        self.tr_out(&format!("{prompt}\n\n"))?;
        self.end_message();
        self.sent += 1;
        self.pulse();
        let _ = self.inbox.send(In::Prompt(prompt));
        Ok(())
    }

    /// Answer the oldest pending permission with the option of this kind,
    /// or the nearest: any allow for an allow, any reject for a reject.
    fn decide(&mut self, kind: PermissionOptionKind) -> apex_tool::Result<()> {
        let Some(p) = self.pending.pop_front() else {
            return self.t.errors(None, "acp: no permission is pending\n");
        };
        let allow = matches!(kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways);
        let same = p.options.iter().find(|o| o.kind == kind);
        let near = p.options.iter().find(|o| matches!(o.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways) == allow);
        let outcome = match same.or(near) {
            Some(o) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(o.option_id.clone())),
            None => RequestPermissionOutcome::Cancelled,
        };
        let _ = p.responder.respond(RequestPermissionResponse::new(outcome));
        let glyph = if allow { "✓" } else { "✗" };
        let said = match kind {
            PermissionOptionKind::AllowOnce => "allowed",
            PermissionOptionKind::AllowAlways => "allowed, and not asked again",
            PermissionOptionKind::RejectOnce => "denied",
            _ => "denied, and not asked again",
        };
        // the decision takes the place of the words offered, in both:
        // it is worth keeping wherever the question was asked
        self.glyph(&p.key, glyph)?;
        if let Some(&at) = self.anchors.get(&p.offer) {
            self.splice(at, at + p.offered, said)?;
        }
        self.tr_glyph(&p.key, glyph)?;
        if let Some(&at) = self.tr.anchors.get(&p.offer) {
            self.tr_splice(at, at + p.offered, said)?;
        }
        for k in [&p.key, &p.offer] {
            self.anchors.remove(k);
            self.tr.anchors.remove(k);
        }
        self.pulse();
        Ok(())
    }

    /// The mode is `id` now, said once however many ways the agent
    /// says it.
    fn took_mode(&mut self, id: String) -> apex_tool::Result<()> {
        if self.mode.as_deref() == Some(id.as_str()) {
            return Ok(());
        }
        let name = self.mode_name(&id);
        self.mode = Some(id);
        self.note(&format!("mode: {name}"))
    }

    /// A mode by its name, when the agent named it.
    fn mode_name(&self, id: &str) -> String {
        self.modes.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone()).unwrap_or_else(|| id.to_string())
    }

    /// `Mode` alone says what the modes are; `Mode NAME` asks for one,
    /// by its id or its name, a unique prefix of either.
    fn set_mode(&mut self, want: &str) -> apex_tool::Result<()> {
        if self.modes.is_empty() {
            return self.note("this agent has no modes");
        }
        if want.is_empty() {
            let here = self.mode.clone().unwrap_or_default();
            let w = self.modes.iter().map(|(id, _)| id.chars().count()).max().unwrap_or(0);
            let mut s = String::from("modes:\n");
            for (id, name) in &self.modes {
                let mark = if *id == here { '*' } else { ' ' };
                s.push_str(&format!("  {mark} {id:<w$}  {name}\n", w = w));
            }
            self.line_start()?;
            return self.out(&s);
        }
        let want_low = want.to_lowercase();
        let hit = self
            .modes
            .iter()
            .find(|(id, name)| id.eq_ignore_ascii_case(want) || name.eq_ignore_ascii_case(want))
            .or_else(|| self.modes.iter().find(|(id, name)| id.to_lowercase().starts_with(&want_low) || name.to_lowercase().starts_with(&want_low)));
        match hit {
            Some((id, _)) => {
                let _ = self.inbox.send(In::Mode(id.clone()));
                Ok(())
            }
            None => self.note(&format!("no mode {want}")),
        }
    }

    fn list_commands(&mut self) -> apex_tool::Result<()> {
        if self.commands.is_empty() {
            return self.note("the agent has offered no commands");
        }
        let w = self.commands.iter().map(|(n, _)| n.chars().count()).max().unwrap_or(0);
        let mut s = String::from("commands:\n");
        for (n, d) in self.commands.clone() {
            s.push_str(&format!("  /{n:<w$}  {d}\n", w = w));
        }
        self.line_start()?;
        self.out(&s)
    }

    /// `Resume` says what sessions this directory has had, a line each:
    /// the id the agent knows it by, when it was last worked in, and
    /// what it is about. `Resume ID` takes one up, and so does B3 on
    /// an id, the listing being text like any other. The window is
    /// then rebuilt from what the agent replays: what stood in it
    /// outlived the program but not the session, and the agent's
    /// record is the one that is still true.
    fn resume(&mut self, want: &str) -> apex_tool::Result<()> {
        if self.replay.is_some() {
            return self.note("a session is being taken up already");
        }
        if self.busy() {
            return self.note("the agent is working: Interrupt first");
        }
        if want.is_empty() {
            if !self.can.list {
                return self.note("this agent does not list the sessions it has had");
            }
            let _ = self.inbox.send(In::List);
            return Ok(());
        }
        if !self.can.load && !self.can.resume {
            return self.note("this agent cannot take a session up again");
        }
        // as much of an id as was given, when the listing knows it;
        // else the word as it stands, for the agent to make of it what
        // it can
        let id = match self.named(want).as_slice() {
            [id] => id.clone(),
            [] => want.to_string(),
            many => return self.note(&format!("{} sessions here are named {want}", many.len())),
        };
        let _ = self.inbox.send(In::Load(Which::Id(id)));
        Ok(())
    }

    /// The sessions `want` names: the whole of an id, as much of one as
    /// it begins with, or one of the parts it is written in.
    fn named(&self, want: &str) -> Vec<String> {
        let want = want.trim().to_lowercase();
        if want.is_empty() {
            return Vec::new();
        }
        self.sessions
            .iter()
            .filter(|p| {
                let id = p.id.to_lowercase();
                id == want || id.starts_with(&want) || id.split('-').any(|part| part == want)
            })
            .map(|p| p.id.clone())
            .collect()
    }

    /// A session named by B3. The whole of an id is one word only when
    /// it was swept; a click takes the part of it that was clicked, so
    /// a part will do when it tells the sessions apart. A word that
    /// names none of them is not ours: it is handed back, and B3 goes
    /// on to do what it would have done.
    fn pick_session(&mut self, text: &str) -> apex_tool::Result<bool> {
        if self.named(text).is_empty() {
            return Ok(false);
        }
        self.resume(text.trim())?;
        Ok(true)
    }

    /// Take the window down to nothing, for a session being replayed to
    /// fill. What stood here was another session's, and is no more
    /// true of this one than of any other window's contents.
    fn reset(&mut self) -> apex_tool::Result<()> {
        // whatever the session we are leaving was waiting on is not
        // going to be answered
        for p in std::mem::take(&mut self.pending) {
            let _ = p.responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
        }
        self.t.replace(self.w, 0, apex_tool::END, "")?;
        self.len = 0;
        self.mark = 0;
        self.col0 = true;
        self.blank = true;
        self.anchors.clear();
        self.titles.clear();
        self.plan = None;
        self.action = None;
        self.tail = None;
        // the transcript was that session's too, and the replay says
        // the whole of this one
        if let Some(w) = self.tr.w {
            self.t.replace(w, 0, apex_tool::END, "")?;
        }
        self.tr = Trans { w: self.tr.w, ..Trans::default() };
        self.reply.clear();
        self.prompts.clear();
        self.asked.clear();
        self.last = None;
        self.in_user = false;
        self.end_message();
        self.msg = None;
        self.pulse();
        Ok(())
    }

    /// A session replayed, written down as it was said. Here the
    /// ninety tool calls of it collapse into the one action line, which
    /// the next thing said takes away, so what is rebuilt reads as the
    /// conversation and not as the machinery; the transcript takes the
    /// whole of it, and `Transcript` opens on the reconstruction
    /// whether or not a window was showing one at the time.
    fn rebuild(&mut self, buf: Vec<SessionUpdate>) -> apex_tool::Result<()> {
        self.replaying = true;
        let mut r = Ok(());
        for u in buf {
            r = self.update(u);
            if r.is_err() {
                break;
            }
        }
        self.replaying = false;
        r
    }

    /// `Login` with a method the agent named, or its only one.
    fn login(&mut self, want: &str) -> apex_tool::Result<()> {
        let pick = match (want.is_empty(), self.auth.as_slice()) {
            (_, []) => return self.note("the agent offers no way to log in from here"),
            (true, [(id, _)]) => id.clone(),
            (true, _) => {
                let list = self.auth.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>().join(", ");
                return self.note(&format!("Login with one of: {list}"));
            }
            (false, ms) => match ms.iter().find(|(id, name)| id.eq_ignore_ascii_case(want) || name.eq_ignore_ascii_case(want)) {
                Some((id, _)) => id.clone(),
                None => return self.note(&format!("no login method {want}")),
            },
        };
        self.note(&format!("logging in with {pick}…"))?;
        let _ = self.inbox.send(In::Auth(pick));
        Ok(())
    }

    /// `Preview` opens a page beside the window holding the last reply
    /// the agent finished, and closes it again. It stands until the
    /// next reply is whole: what is half-said is in the transcript.
    fn toggle_page(&mut self) -> apex_tool::Result<()> {
        match self.page.take() {
            Some(w) => {
                let _ = self.t.delete(w);
                Ok(())
            }
            None => {
                let name = format!("{}+Preview", self.t.window_name(self.w).unwrap_or_else(|| "-agent".to_string()));
                let html = self.rendered();
                let w = self.t.new_page(&name, &html)?;
                // ours, as the preview tool's page is: Del does not ask
                let _ = self.t.set_owner(w, true);
                let _ = self.t.set_live(w, true);
                self.page = Some(w);
                Ok(())
            }
        }
    }

    /// The page again, when one is open.
    fn show_page(&mut self) -> apex_tool::Result<()> {
        let Some(w) = self.page else { return Ok(()) };
        let html = self.rendered();
        self.t.replace(w, 0, apex_tool::END, &html)
    }

    /// The last reply as a page: through the converter the session names
    /// for markdown (`Preview.md`, `apex md` unless a setting says
    /// otherwise), so a page here looks like every other preview.
    fn rendered(&self) -> String {
        let Some(x) = self.last.as_ref() else {
            return "<p><em>no reply yet</em></p>".to_string();
        };
        // what was asked stands first, quoted, as the preamble to the
        // answer: the page is the exchange, and reads as one
        let mut md = String::new();
        for line in x.asked.trim().lines() {
            md.push_str("> ");
            md.push_str(line);
            md.push('\n');
        }
        if !md.is_empty() {
            md.push('\n');
        }
        md.push_str(x.said.trim());
        md.push('\n');
        let md = &md;
        let cmd = self.t.setting("Preview.md").unwrap_or_else(|| "apex md".to_string());
        match filter(&cmd, md, &self.cwd) {
            Ok(html) => html,
            Err(e) => format!("<pre>{}</pre>", escape(&format!("{cmd}: {e}\n\n{md}"))),
        }
    }

    /// What the handle says, which is what the window is waiting for.
    /// Work going on: it pulses, and is live while it does, so it
    /// breathes from that colour rather than from what the writing has
    /// made of the window. Nothing going on and nothing typed: clean,
    /// the answer is whole and there is nothing of anyone's left in it.
    /// A prompt typed and not sent: dirty, which is what dirty means --
    /// your text, not acted on. The pulse rests while the agent waits
    /// on an answer of ours: the window says whose turn it is.
    fn pulse(&mut self) {
        let want = self.busy() && self.pending.is_empty();
        if want != self.pulsing {
            self.pulsing = want;
            let _ = self.t.set_working(self.w, want);
            let _ = self.t.set_live(self.w, want);
            self.settled = false;
        }
        // the end of what has been said, once there is anything to end
        let _ = self.show_tail();
        // what follows the mark is a prompt not yet sent
        let drafted = self.len > self.mark;
        if !want && !drafted && !self.settled {
            self.settled = true;
            let _ = self.t.set_clean(self.w);
        }
    }

    /// A line of apex's own: what the agent is, what it is ready for,
    /// what was asked of it that could not be done. A block of them (a
    /// listing) is marked once, at its head, as a message is.
    fn note(&mut self, text: &str) -> apex_tool::Result<()> {
        self.line_start()?;
        let text = text.trim_end_matches('\n');
        self.out(&format!("{SYS}{text}\n"))
    }

    /// One the transcript keeps too: what the session itself is, rather
    /// than what the window was asked.
    fn note_both(&mut self, text: &str) -> apex_tool::Result<()> {
        self.line_start()?;
        self.tr_line_start()?;
        let text = text.trim_end_matches('\n');
        self.both(&format!("{SYS}{text}\n"))
    }

    // ---- terminals: the commands the agent runs ----

    /// The window every command's output goes to, made when the first
    /// one starts and again if it is deleted: acme's `+Errors` for what
    /// the agent runs. One window rather than one a command, so a turn
    /// that builds and tests and greps does not bury the column.
    fn run_window(&mut self) -> apex_tool::Result<WindowId> {
        if let Some(w) = self.run {
            if self.t.windows().iter().any(|x| x.id == w) {
                return Ok(w);
            }
            self.run = None;
            self.wrote = None;
        }
        let name = format!("{}+run", self.t.window_name(self.w).unwrap_or_else(|| "-agent".to_string()));
        let w = self.t.new_window(&name)?;
        // ours, as the page is: Del does not ask about the text in it
        let _ = self.t.set_owner(w, true);
        let _ = self.t.set_live(w, true);
        self.run = Some(w);
        self.run_col0 = true;
        self.running = false;
        Ok(w)
    }

    /// Output from the command `id` into the run window, headed by the
    /// command itself whenever another one wrote last. `line` asks for
    /// it to begin on a line of its own. Nothing is written here that
    /// the agent reads: what it asked for is the command's own bytes,
    /// which the head and the exit line are not part of.
    fn to_run(&mut self, id: &str, cmd: &str, text: &str, line: bool) -> apex_tool::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let w = self.run_window()?;
        let mut s = String::new();
        if self.wrote.as_deref() != Some(id) {
            if !self.run_col0 {
                s.push('\n');
            }
            if self.wrote.is_some() {
                s.push('\n');
            }
            s.push_str(&format!("$ {cmd}\n"));
            self.wrote = Some(id.to_string());
        } else if line && !self.run_col0 {
            s.push('\n');
        }
        s.push_str(text);
        self.run_col0 = s.ends_with('\n');
        self.t.insert_following(w, apex_tool::END, &s)
    }

    /// What the commands have written since the last turn round the
    /// loop, and which of them have ended. Their output arrives on
    /// threads of its own (`term`); this is where it reaches a window,
    /// and where an agent waiting on `terminal/wait_for_exit` is
    /// answered.
    fn tick_terms(&mut self) -> apex_tool::Result<()> {
        let ids: Vec<String> = self.terms.keys().cloned().collect();
        for id in ids {
            let Some(t) = self.terms.get_mut(&id) else { continue };
            let cmd = t.run.command.clone();
            let text = t.run.take_fresh();
            let ended = if t.ended { None } else { t.run.settled() };
            self.to_run(&id, &cmd, &text, false)?;
            let Some(e) = ended else { continue };
            // whatever the pipes still held when it went
            let last = self.terms.get(&id).map(|t| t.run.take_fresh()).unwrap_or_default();
            self.to_run(&id, &cmd, &last, false)?;
            self.to_run(&id, &cmd, &format!("({e})\n"), true)?;
            if let Some(t) = self.terms.get_mut(&id) {
                t.ended = true;
                for r in std::mem::take(&mut t.waiting) {
                    let _ = r.respond(WaitForTerminalExitResponse::new(exit_status(&e)));
                }
            }
        }
        self.pulse_run();
        Ok(())
    }

    /// The run window's handle pulses while a command is going, as the
    /// agent's does while it thinks.
    fn pulse_run(&mut self) {
        let want = self.terms.values().any(|t| !t.ended);
        if want != self.running {
            self.running = want;
            if let Some(w) = self.run {
                let _ = self.t.set_working(w, want);
            }
        }
    }

    /// `Stop` ends every command the agent has running. The agent is
    /// told as it would be of any other end: a killed command reports
    /// the signal that took it.
    fn kill_terms(&mut self) -> apex_tool::Result<()> {
        let mut killed = 0;
        for t in self.terms.values_mut() {
            if !t.ended {
                t.run.kill();
                killed += 1;
            }
        }
        if killed == 0 {
            return self.note("the agent is running nothing");
        }
        Ok(())
    }

    fn start_term(&mut self, id: &str, req: &CreateTerminalRequest) -> Result<(), String> {
        let cwd = req.cwd.clone().unwrap_or_else(|| self.cwd.clone());
        let env: Vec<(String, String)> = req.env.iter().map(|v| (v.name.clone(), v.value.clone())).collect();
        let run = term::Run::start(&req.command, &req.args, &env, &cwd, req.output_byte_limit).map_err(|e| format!("{}: {e}", req.command))?;
        self.terms.insert(id.to_string(), Term { run, waiting: Vec::new(), ended: false });
        // the window comes with the command, not with its first output,
        // so that starting one is visible at once
        let _ = self.run_window();
        self.pulse_run();
        Ok(())
    }

    fn term_req(&mut self, r: TermReq) -> apex_tool::Result<()> {
        match r {
            TermReq::Create { req, responder } => {
                let id = format!("t{}", self.next_term);
                self.next_term += 1;
                let _ = match self.start_term(&id, &req) {
                    Ok(()) => responder.respond(CreateTerminalResponse::new(TerminalId::new(id))),
                    Err(e) => responder.respond_with_internal_error(e),
                };
            }
            TermReq::Output { req, responder } => {
                let id = req.terminal_id.0.to_string();
                let _ = match self.terms.get_mut(&id) {
                    Some(t) => {
                        let (output, truncated) = t.run.output();
                        let exit = t.run.exit();
                        let res = TerminalOutputResponse::new(output, truncated);
                        responder.respond(match exit {
                            Some(e) => res.exit_status(exit_status(&e)),
                            None => res,
                        })
                    }
                    None => responder.respond_with_internal_error(gone(&id)),
                };
            }
            TermReq::Wait { req, responder } => {
                let id = req.terminal_id.0.to_string();
                match self.terms.get_mut(&id) {
                    // the tick answers it once the command is over and
                    // its output is all in
                    Some(t) if !t.ended => t.waiting.push(responder),
                    Some(t) => {
                        let e = t.run.exit().unwrap_or(term::Exit { code: None, signal: None });
                        let _ = responder.respond(WaitForTerminalExitResponse::new(exit_status(&e)));
                    }
                    None => {
                        let _ = responder.respond_with_internal_error(gone(&id));
                    }
                }
            }
            TermReq::Kill { req, responder } => {
                let id = req.terminal_id.0.to_string();
                let _ = match self.terms.get_mut(&id) {
                    Some(t) => {
                        t.run.kill();
                        responder.respond(KillTerminalResponse::new())
                    }
                    None => responder.respond_with_internal_error(gone(&id)),
                };
            }
            TermReq::Release { req, responder } => {
                let id = req.terminal_id.0.to_string();
                let _ = match self.terms.remove(&id) {
                    Some(mut t) => {
                        let (cmd, ended) = (t.run.command.clone(), t.ended);
                        let text = t.run.take_fresh();
                        for r in std::mem::take(&mut t.waiting) {
                            let _ = r.respond_with_internal_error("the terminal was released");
                        }
                        // the command goes with the terminal it ran in
                        drop(t);
                        self.to_run(&id, &cmd, &text, false)?;
                        if !ended {
                            self.to_run(&id, &cmd, "(released while running)\n", true)?;
                        }
                        self.pulse_run();
                        responder.respond(ReleaseTerminalResponse::new())
                    }
                    None => responder.respond_with_internal_error(gone(&id)),
                };
            }
        }
        Ok(())
    }

    // ---- the agent's news ----

    fn take(&mut self, o: Out) -> apex_tool::Result<()> {
        match o {
            Out::Hello { agent, auth, can } => {
                self.auth = auth;
                self.can = can;
                self.agent = agent;
                let line = self.agent.clone();
                self.note_both(&line)
            }
            Out::NeedsAuth(why) => {
                self.note(&format!("no session: {why}"))?;
                match self.auth.clone().as_slice() {
                    [] => self.note("the agent offers no way to log in from here"),
                    ms => {
                        let list = ms.iter().map(|(id, name)| format!("{id} ({name})")).collect::<Vec<_>>().join(", ");
                        self.note(&format!("Login to go on: {list}"))
                    }
                }
            }
            Out::Ready { modes, mode } => {
                self.modes = modes;
                self.mode = mode;
                let m = self.mode.clone().map(|m| format!(" [{}]", self.mode_name(&m))).unwrap_or_default();
                self.note_both(&format!("ready{m}"))?;
                self.both("\n")?;
                self.ready = true;
                // the window is ready to be typed in, so the cursor is
                // where it would be typed
                self.cursor_to_mark();
                // and what there is to take up again is the first thing
                // offered: a session here is usually a session resumed
                if self.can.list {
                    let _ = self.inbox.send(In::List);
                }
                Ok(())
            }
            // the agent announces the mode as well as answering for
            // it, and not all of them do either: whichever says first
            // is the one that says
            Out::Mode(id) => self.took_mode(id),
            Out::Note(n) => self.note(&n),
            Out::Sessions(v) => {
                self.sessions = v;
                if self.sessions.is_empty() {
                    return self.note("no session has been here before");
                }
                // the id first, because it is what B3 takes
                let now = now();
                let rows: Vec<(&str, String)> = self.sessions.iter().map(|p| (p.id.as_str(), when(p.when.as_deref(), now))).collect();
                let wide = |f: &dyn Fn(&(&str, String)) -> usize| rows.iter().map(f).max().unwrap_or(0);
                let (wi, wt) = (wide(&|r| r.0.chars().count()), wide(&|r| r.1.chars().count()));
                let mut s = String::from("sessions:\n");
                for (p, (id, at)) in self.sessions.iter().zip(&rows) {
                    s.push_str(&format!("  {id:<wi$}  {at:<wt$}  {}\n", p.title.as_deref().unwrap_or("(untitled)")));
                }
                self.note(&s)
            }
            Out::Replay => {
                self.replay = Some(Vec::new());
                Ok(())
            }
            Out::NoResume(why) => {
                self.replay = None;
                self.note(&format!("not resumed: {why}"))
            }
            Out::Resumed { past, replayed, modes, mode } => {
                // a session taken up by its id comes back nameless: the
                // listing here knows what the agent did not say again
                let past = match past.title.is_some() {
                    true => past,
                    false => self.sessions.iter().find(|p| p.id == past.id).cloned().unwrap_or(past),
                };
                let buf = self.replay.take().unwrap_or_default();
                self.modes = modes;
                self.mode = mode;
                self.reset()?;
                // the same line the listing wrote, so the window says
                // which session it is and B3 on the id still reaches it
                let mut line = past.id.clone();
                for part in [when(past.when.as_deref(), now()), past.title.clone().unwrap_or_default()] {
                    if !part.is_empty() {
                        line.push_str("  ");
                        line.push_str(&part);
                    }
                }
                let agent = self.agent.clone();
                self.note_both(&agent)?;
                self.note_both(&line)?;
                self.both("\n")?;
                self.rebuild(buf)?;
                // the reply the agent had got to is the last whole one:
                // nothing followed it to say otherwise
                if !self.reply.trim().is_empty() {
                    self.last = Some(Exchange { asked: std::mem::take(&mut self.asked), said: std::mem::take(&mut self.reply) });
                }
                self.in_user = false;
                if !replayed {
                    self.note("this agent keeps the transcript to itself: only the session came back")?;
                }
                self.line_start()?;
                self.tr_line_start()?;
                self.both("\n")?;
                let m = self.mode.clone().map(|m| format!(" [{}]", self.mode_name(&m))).unwrap_or_default();
                self.note_both(&format!("ready{m}"))?;
                self.both("\n")?;
                self.ready = true;
                self.show_page()?;
                // the whole of it went in at once: this is the one
                // place the window is brought to its end rather than
                // following there
                self.t.show(self.w, self.mark)?;
                self.cursor_to_mark();
                Ok(())
            }
            Out::Update(u) => match &mut self.replay {
                // gathered: the window is rebuilt from the whole of it
                Some(buf) => {
                    buf.push(u);
                    Ok(())
                }
                None => self.update(u),
            },
            Out::Term(r) => self.term_req(r),
            Out::Permission { req, responder } => {
                let id = req.tool_call.tool_call_id.0.to_string();
                let title = req.tool_call.fields.title.clone().or_else(|| self.titles.get(&id).cloned()).unwrap_or_else(|| id.clone());
                let n = self.pending.len() + self.anchors.len();
                let (key, offer) = (format!("perm:{n}"), format!("offer:{n}"));
                self.end_message();
                const OFFERED: &str = "Allow Always Deny Never";
                // here the call has no line standing, so the question
                // must say what it is about; before the anchors below
                // are taken, the action line goes
                self.drop_action()?;
                self.line_start()?;
                self.anchors.insert(key.clone(), self.mark);
                self.out(&format!("? {}: ", brief(&title)))?;
                self.anchors.insert(offer.clone(), self.mark);
                self.out(&format!("{OFFERED}\n"))?;
                // in the transcript the call has a line already, and
                // the question goes under it
                let under = self.tr.anchors.contains_key(&id);
                self.tr_line_start()?;
                if under {
                    self.tr_out("    ")?;
                }
                self.tr.anchors.insert(key.clone(), self.tr.len);
                self.tr_out("? ")?;
                if !under {
                    self.tr_out(&format!("{title}: "))?;
                }
                self.tr.anchors.insert(offer.clone(), self.tr.len);
                self.tr_out(&format!("{OFFERED}\n"))?;
                self.pending.push_back(Pending { key, offer, offered: OFFERED.chars().count(), options: req.options, responder });
                self.pulse();
                Ok(())
            }
            Out::Read { req, responder } => {
                let res = self.read_file(&req.path, req.line, req.limit);
                let _ = match res {
                    Ok(content) => responder.respond(ReadTextFileResponse::new(content)),
                    Err(e) => responder.respond_with_internal_error(e),
                };
                Ok(())
            }
            Out::Write { req, responder } => {
                let res = self.write_file(&req.path, &req.content);
                let _ = match res {
                    Ok(()) => responder.respond(WriteTextFileResponse::new()),
                    Err(e) => responder.respond_with_internal_error(e),
                };
                Ok(())
            }
            Out::Dropped(n) => {
                self.sent = self.sent.saturating_sub(n);
                self.pulse();
                self.note(&format!("{n} waiting to be sent went too"))
            }
            Out::Turn(res) => {
                self.sent = self.sent.saturating_sub(1);
                self.end_message();
                self.pulse();
                let asked = self.prompts.pop_front().unwrap_or_default();
                // only a turn the agent finished is worth showing: a
                // cancelled or failed one leaves the last one standing
                if matches!(res, Ok(StopReason::EndTurn)) && !self.reply.trim().is_empty() {
                    self.last = Some(Exchange { asked, said: std::mem::take(&mut self.reply) });
                    self.show_page()?;
                }
                let end = match res {
                    Ok(StopReason::EndTurn) => String::new(),
                    Ok(StopReason::Cancelled) => format!("{SYS}cancelled\n"),
                    Ok(r) => format!("{SYS}{r:?}\n"),
                    Err(e) => format!("{SYS}error: {e}\n"),
                };
                self.line_start()?;
                self.out(&end)?;
                self.out("\n")?;
                self.tr_line_start()?;
                self.tr_out(&end)?;
                self.tr_out("\n")?;
                Ok(())
            }
            Out::Gone(why) => {
                self.sent = 0;
                self.pulse();
                self.note(&why)
            }
        }
    }

    fn update(&mut self, u: SessionUpdate) -> apex_tool::Result<()> {
        // the user's own words end where anything else begins, and so
        // does the agent's message
        if self.in_user && !matches!(u, SessionUpdate::UserMessageChunk(_)) {
            self.in_user = false;
            self.line_start()?;
            self.tr_line_start()?;
            self.both("\n")?;
        }
        if !matches!(u, SessionUpdate::AgentMessageChunk(_)) {
            self.end_message();
        }
        match u {
            SessionUpdate::AgentMessageChunk(c) => {
                // a message of the agent's is one thing it did, and
                // begins with a bullet: the calls it made are taken
                // away again as they finish, and these lines are what
                // the work leaves behind. The agent says which message
                // its chunks are of; failing that, anything else
                // written ends the one going.
                let new = match &c.message_id {
                    Some(id) => self.msg.as_ref() != Some(id),
                    None => !self.in_agent,
                };
                if c.message_id.is_some() {
                    self.msg = c.message_id.clone();
                }
                if new {
                    self.in_agent = true;
                    self.owed = true;
                    // the page shows the answer, not the running
                    // commentary made on the way to it
                    self.reply.clear();
                }
                let text = content_text(&c.content);
                self.reply.push_str(&text);
                // the bullet is ours, not the agent's -- the page
                // shows what it said -- and it waits for the words it
                // goes in front of
                let text = match self.owed {
                    false => text,
                    true => match text.trim_start() {
                        "" => return Ok(()),
                        t => {
                            let t = t.to_string();
                            self.owed = false;
                            self.line_start()?;
                            self.tr_line_start()?;
                            self.both(AGENT)?;
                            t
                        }
                    },
                };
                self.both(&text)
            }
            SessionUpdate::AgentThoughtChunk(c) => {
                if self.thoughts {
                    let text = content_text(&c.content);
                    self.out_prefixed("  · ", &text)?;
                    self.tr_prefixed("  · ", &text)?;
                }
                Ok(())
            }
            SessionUpdate::UserMessageChunk(c) => {
                // live these are the words the user typed, and they are
                // in the window already because they typed them there.
                // On a replay they are the only source of their half.
                if !self.replaying {
                    return Ok(());
                }
                if !self.in_user {
                    self.in_user = true;
                    // a turn ends where the next prompt begins, and
                    // reads as a live one does: under the marker
                    if !self.reply.trim().is_empty() {
                        self.last = Some(Exchange { asked: std::mem::take(&mut self.asked), said: std::mem::take(&mut self.reply) });
                    }
                    self.asked.clear();
                    self.mark_turn()?;
                }
                let text = content_text(&c.content);
                self.asked.push_str(&text);
                self.both(&text)
            }
            SessionUpdate::ToolCall(tc) => {
                let id = tc.tool_call_id.0.to_string();
                self.titles.insert(id.clone(), tc.title.clone());
                // here, the one line saying what is going on now; in
                // the transcript, the call and all its workings
                self.action(&format!("{} {}", status_glyph(tc.status), brief(&tc.title)))?;
                self.tr_line_start()?;
                self.tr.anchors.insert(id.clone(), self.tr.len);
                self.tr_out(&format!("{} {}\n", status_glyph(tc.status), tc.title))?;
                self.locations(&id, &tc.locations)?;
                self.contents(&id, &tc.content)
            }
            SessionUpdate::ToolCallUpdate(up) => self.tool_update(&up),
            SessionUpdate::Plan(plan) => self.show_plan(&plan),
            SessionUpdate::CurrentModeUpdate(m) => self.took_mode(m.current_mode_id.0.to_string()),
            SessionUpdate::AvailableCommandsUpdate(u) => {
                self.commands = u.available_commands.iter().map(|c| (c.name.clone(), c.description.clone())).collect();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn tool_update(&mut self, up: &ToolCallUpdate) -> apex_tool::Result<()> {
        let id = up.tool_call_id.0.to_string();
        let f = &up.fields;
        let known = self.titles.contains_key(&id);
        if let Some(t) = &f.title {
            self.titles.insert(id.clone(), t.clone());
        }
        // the line here changes when the status does, and when a call
        // we never saw begin names itself; what it read or wrote is
        // detail, and belongs to the transcript
        let title = self.titles.get(&id).cloned().unwrap_or_else(|| id.clone());
        let said = brief(&title);
        match (f.status, f.title.is_some() || !known) {
            (Some(s), _) => self.action(&format!("{} {said}", status_glyph(s)))?,
            (None, true) => self.action(&format!("{} {said}", status_glyph(ToolCallStatus::default())))?,
            (None, false) => {}
        }
        if !self.tr.anchors.contains_key(&id) && !known {
            // news of a call we never saw begin
            self.tr_line_start()?;
            self.tr.anchors.insert(id.clone(), self.tr.len);
            self.tr_out(&format!("{} {}\n", status_glyph(f.status.unwrap_or_default()), title))?;
        } else if let Some(s) = f.status {
            self.tr_glyph(&id, status_glyph(s))?;
        }
        if let Some(locs) = &f.locations {
            self.locations(&id, locs)?;
        }
        if let Some(c) = &f.content {
            self.contents(&id, c)?;
        }
        Ok(())
    }

    /// A path as shown: relative to the session's directory when under it.
    fn shown(&self, p: &Path) -> String {
        p.strip_prefix(&self.cwd).unwrap_or(p).display().to_string()
    }

    /// Write the blocks of `key` that are not already down. Agents
    /// resend a tool call's whole content and locations with every
    /// update of it; what has not changed is already written.
    fn fresh(&mut self, key: &str, blocks: Vec<String>) -> apex_tool::Result<()> {
        let old = self.tr.said.entry(key.to_string()).or_default().clone();
        for (i, b) in blocks.iter().enumerate() {
            if old.get(i) == Some(b) {
                continue;
            }
            self.tr_line_start()?;
            self.tr_out(b)?;
        }
        self.tr.said.insert(key.to_string(), blocks);
        Ok(())
    }

    /// Where a call works, one line per file: agents name the same file
    /// again as they learn where in it they are, and the second naming
    /// is no news.
    fn locations(&mut self, id: &str, locs: &[ToolCallLocation]) -> apex_tool::Result<()> {
        let key = format!("{id}/where");
        for l in locs {
            let p = self.shown(&l.path);
            let seen = self.tr.said.entry(key.clone()).or_default();
            if seen.iter().any(|x| *x == p) {
                continue;
            }
            seen.push(p.clone());
            self.tr_line_start()?;
            match l.line {
                Some(n) => self.tr_out(&format!("    {p}:{n}\n"))?,
                None => self.tr_out(&format!("    {p}\n"))?,
            }
        }
        Ok(())
    }

    fn contents(&mut self, id: &str, cs: &[ToolCallContent]) -> apex_tool::Result<()> {
        let mut blocks = Vec::new();
        for c in cs {
            let mut b = String::new();
            match c {
                ToolCallContent::Diff(d) => {
                    let p = self.shown(&d.path);
                    let (lines, cut) = hunk(d.old_text.as_deref().unwrap_or(""), &d.new_text);
                    let (minus, plus) = (lines.iter().filter(|l| l.starts_with('-')).count(), lines.iter().filter(|l| l.starts_with('+')).count());
                    b.push_str(&format!("    {p}  +{plus} -{minus}\n"));
                    for l in &lines {
                        b.push_str(&format!("    {l}\n"));
                    }
                    if cut > 0 {
                        b.push_str(&format!("    … {cut} more lines\n"));
                    }
                }
                // the output itself is in the run window: the line
                // names it, so B3 opens it
                ToolCallContent::Terminal(t) => match self.run.and_then(|w| self.t.window_name(w)) {
                    Some(n) => b.push_str(&format!("    {}\n", self.shown(Path::new(&n)))),
                    None => b.push_str(&format!("    terminal {}\n", t.terminal_id.0)),
                },
                ToolCallContent::Content(c) => {
                    let text = content_text(&c.content);
                    let lines: Vec<&str> = text.lines().collect();
                    let shown = lines.len().min(12);
                    for l in &lines[..shown] {
                        b.push_str(&format!("    {l}\n"));
                    }
                    if lines.len() > shown {
                        b.push_str(&format!("    … {} more lines\n", lines.len() - shown));
                    }
                }
                _ => {}
            }
            blocks.push(b);
        }
        blocks.retain(|b| !b.is_empty());
        self.fresh(&format!("{id}/what"), blocks)
    }

    fn show_plan(&mut self, plan: &Plan) -> apex_tool::Result<()> {
        let mut s = String::from("plan:\n");
        for e in &plan.entries {
            let box_ = match e.status {
                PlanEntryStatus::Pending => "[ ]",
                PlanEntryStatus::InProgress => "[~]",
                PlanEntryStatus::Completed => "[x]",
                _ => "[?]",
            };
            s.push_str(&format!("  {box_} {}\n", e.content));
        }
        if let Some((a, b)) = self.plan.take() {
            self.splice(a, b, "")?;
        }
        // before the block's own place is taken down
        self.drop_action()?;
        self.line_start()?;
        let a = self.mark;
        self.out(&s)?;
        self.plan = Some((a, self.mark));
        if let Some((a, b)) = self.tr.plan.take() {
            self.tr_splice(a, b, "")?;
        }
        self.tr_line_start()?;
        let a = self.tr.len;
        self.tr_out(&s)?;
        self.tr.plan = Some((a, self.tr.len));
        Ok(())
    }

    /// A file written: through its window when one is open, so the
    /// change is an edit the user can undo and see, and then put, so
    /// what the agent builds and tests next is on disk too. With no
    /// window, straight to the disk, where the watcher (§9) brings it
    /// into any clean window later.
    fn write_file(&mut self, path: &Path, content: &str) -> Result<(), String> {
        let name = path.display().to_string();
        let open = self.t.windows().into_iter().find(|w| w.name == name).map(|w| w.id);
        match open {
            Some(w) => {
                self.t.replace(w, 0, apex_tool::END, content).map_err(|e| e.0)?;
                self.t.exec_in(Some(w), "Put").map_err(|e| e.0)
            }
            None => std::fs::write(path, content).map_err(|e| format!("{name}: {e}")),
        }
    }

    /// A file's text: the window's when one is open under that name,
    /// else the disk's. `line` is 1-based; `limit` counts lines.
    fn read_file(&self, path: &Path, line: Option<u32>, limit: Option<u32>) -> Result<String, String> {
        let name = path.display().to_string();
        let open = self.t.windows().into_iter().find(|w| w.name == name).map(|w| w.id);
        let text = match open {
            Some(w) => self.t.read(w).map_err(|e| e.0)?,
            None => std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?,
        };
        if line.is_none() && limit.is_none() {
            return Ok(text);
        }
        let skip = line.map(|l| l.saturating_sub(1) as usize).unwrap_or(0);
        let take = limit.map(|l| l as usize).unwrap_or(usize::MAX);
        let mut out = String::new();
        for l in text.split_inclusive('\n').skip(skip).take(take) {
            out.push_str(l);
        }
        Ok(out)
    }
}

/// The one changed stretch between two texts, as `-`/`+` lines, and how
/// many were left out. Agents edit a place at a time, so the common
/// prefix and suffix are enough to find it.
fn hunk(old: &str, new: &str) -> (Vec<String>, usize) {
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

/// Run `cmd` with `input` on its standard input, in `dir`, and take
/// what it writes.
fn filter(cmd: &str, input: &str, dir: &Path) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(shell())
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
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

/// Where the character at `ch` begins, in bytes; the end when it is
/// past the last.
fn byte_at(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map(|(i, _)| i).unwrap_or(s.len())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn exit_status(e: &term::Exit) -> TerminalExitStatus {
    TerminalExitStatus::new().exit_code(e.code).signal(e.signal.clone())
}

/// A terminal the agent names that we do not have: it was released, or
/// never made.
fn gone(id: &str) -> String {
    format!("no terminal {id}")
}

/// A timestamp the agent gave (ISO 8601) as seconds since the epoch.
/// Whatever zone it was written in, since agents differ: `Z`, an
/// offset, or none, which is taken as UTC.
fn epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |at: usize, n: usize| -> Option<i64> { std::str::from_utf8(b.get(at..at + n)?).ok()?.parse().ok() };
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hh, mm, ss) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&m) {
        return None;
    }
    // days from the civil calendar (Howard Hinnant's algorithm)
    let y = y - (m <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    // the zone it was written in: the date's own hyphens are behind us
    let off = match b.iter().rposition(|&c| c == b'+' || c == b'-').filter(|&i| i > 10) {
        Some(i) => {
            let n = num(i + 1, 2)? * 3600 + num(i + 4, 2)? * 60;
            if b[i] == b'+' {
                n
            } else {
                -n
            }
        }
        None => 0,
    };
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - off)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A time at the resolution that tells it apart: the time of day
/// today, the weekday within the week, and the date beyond it. In this
/// machine's zone, since that is the one the day is being had in.
fn when(at: Option<&str>, now: i64) -> String {
    let Some(t) = at.and_then(epoch) else { return String::new() };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let secs = t as libc::time_t;
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return String::new();
    }
    let clock = format!("{}:{:02}{}", if tm.tm_hour % 12 == 0 { 12 } else { tm.tm_hour % 12 }, tm.tm_min, if tm.tm_hour < 12 { "AM" } else { "PM" });
    match now - t {
        d if d > 7 * 24 * 3600 => format!("{}{}{:02}", tm.tm_mday, MONTHS[(tm.tm_mon as usize).min(11)], (tm.tm_year + 1900).rem_euclid(100)),
        d if d > 24 * 3600 => format!("{}{clock}", DAYS[(tm.tm_wday as usize).min(6)]),
        _ => clock,
    }
}

/// Now, in seconds since the epoch.
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// What a call is called, in one line and a short one. An agent's
/// title for a call is often the command itself, heredoc and all, and
/// this window keeps the summary of the work and not the code in it:
/// a title that runs on is cut, and says so. The transcript has it
/// whole.
fn brief(title: &str) -> String {
    const KEEP: usize = 76;
    let line = title.lines().next().unwrap_or("").trim_end();
    if line.chars().count() <= KEEP && title.lines().nth(1).is_none() {
        return line.to_string();
    }
    let cut: String = line.chars().take(KEEP).collect();
    format!("{}…", cut.trim_end())
}

fn status_glyph(s: ToolCallStatus) -> &'static str {
    match s {
        ToolCallStatus::Pending => "⋯",
        ToolCallStatus::InProgress => "▶",
        ToolCallStatus::Completed => "✓",
        ToolCallStatus::Failed => "✗",
        _ => "?",
    }
}

fn content_text(c: &ContentBlock) -> String {
    match c {
        ContentBlock::Text(t) => t.text.clone(),
        ContentBlock::ResourceLink(r) => format!("[{}]", r.uri),
        ContentBlock::Image(_) => "[image]".to_string(),
        ContentBlock::Audio(_) => "[audio]".to_string(),
        _ => "[resource]".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The civil calendar and the zone an agent wrote its times in.
    #[test]
    fn timestamps_are_read_whatever_zone_they_were_written_in() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2000-03-01T00:00:00Z"), Some(951868800));
        assert_eq!(epoch("2026-09-12T19:38:47.060Z"), Some(1789241927));
        // an offset says the same moment as the UTC it works out to
        assert_eq!(epoch("2026-09-12T21:38:47+02:00"), epoch("2026-09-12T19:38:47Z"));
        assert_eq!(epoch("2026-09-12T14:38:47-05:00"), epoch("2026-09-12T19:38:47Z"));
        // a leap day, and a century that is not a leap year
        assert_eq!(epoch("2024-02-29T12:00:00Z"), Some(1709208000));
        assert_eq!(epoch("1900-03-01T00:00:00Z"), Some(-2203891200));
        assert_eq!(epoch("not a time"), None);
        assert_eq!(epoch("2026-13-01T00:00:00Z"), None);
    }

    /// The resolution that tells one time from another: the clock
    /// today, the weekday within the week, the date beyond it.
    #[test]
    fn a_time_is_shown_at_the_resolution_that_tells_it_apart() {
        // the clock is this machine's, so the test names a zone
        extern "C" {
            fn tzset();
        }
        std::env::set_var("TZ", "UTC");
        unsafe { tzset() };
        let at = "2026-09-12T19:38:47Z";
        let t = epoch(at).unwrap();
        let f = |ago: i64| when(Some(at), t + ago);
        assert_eq!(f(0), "7:38PM");
        assert_eq!(f(3600), "7:38PM");
        assert_eq!(f(23 * 3600), "7:38PM");
        assert_eq!(f(25 * 3600), "Sat7:38PM");
        assert_eq!(f(6 * 24 * 3600), "Sat7:38PM");
        assert_eq!(f(8 * 24 * 3600), "12Sep26");
        // midnight and noon are 12, as Kitchen has them
        assert_eq!(when(Some("2026-09-12T00:04:00Z"), t), "12:04AM");
        assert_eq!(when(Some("2026-09-12T12:04:00Z"), t), "12:04PM");
        // no time, nothing to say
        assert_eq!(when(None, t), "");
        assert_eq!(when(Some("whenever"), t), "");
    }
}
