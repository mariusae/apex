//! apex-acp: an experiment. An Agent Client Protocol client that lives in
//! an apex window, built on the public tool API only.
//!
//! The agent (any ACP agent: `claude-agent-acp`, `codex-acp`, ...) is
//! started as a child; its session is a window named `DIR/+agent`. What
//! the agent says streams into the window; what the user types at the
//! end of it is the next prompt, sent by Enter on the last line, or by
//! the `Send` verb. Tool calls are lines the agent's progress ticks off
//! in place; their file locations are plumbable. When the agent asks
//! permission, the line offers `Allow Always Deny Never` to B2. `Cancel`
//! interrupts the turn. The agent reads files through apex when they
//! are open (unsaved edits included); its writes go to disk, where
//! apex's watcher brings them into clean windows.
//!
//!     apex-acp [-agent claude|codex|CMD] [-cwd DIR] [-thoughts]
//!
//! The agent defaults to `$APEX_ACP_AGENT`, else Anthropic's Claude
//! adapter: `claude-agent-acp` when it is installed, else npx fetching
//! `@agentclientprotocol/claude-agent-acp`. `-agent` takes either of
//! those short names or a command of its own.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::mpsc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    AuthMethodId, AuthenticateRequest, CancelNotification, ClientCapabilities, ContentBlock, FileSystemCapabilities, Implementation,
    InitializeRequest, NewSessionRequest, PermissionOption, PermissionOptionKind, Plan, PlanEntryStatus, PromptRequest,
    ReadTextFileRequest, ReadTextFileResponse, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionModeId, SessionNotification, SessionUpdate, SetSessionModeRequest, StopReason, TextContent,
    ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, Responder};
use apex_tool::{Event, Rule, RuleId, Tool, WindowId};

/// What the agent side tells the window side.
enum Out {
    /// The agent answered `initialize`.
    Hello { agent: String, auth: Vec<(String, String)> },
    /// A session could not be started: `Login` must come first.
    NeedsAuth(String),
    /// A session is going: its modes, and the one it is in.
    Ready { modes: Vec<(String, String)>, mode: Option<String> },
    /// A mode we asked for was taken (agents do not all say so).
    Mode(String),
    /// Something worth a line of its own.
    Note(String),
    Update(SessionUpdate),
    Permission { req: RequestPermissionRequest, responder: Responder<RequestPermissionResponse> },
    Read { req: ReadTextFileRequest, responder: Responder<ReadTextFileResponse> },
    Write { req: WriteTextFileRequest, responder: Responder<WriteTextFileResponse> },
    /// A turn ended, well or badly.
    Turn(Result<StopReason, String>),
    /// The agent side is over.
    Gone(String),
}

/// What the window side tells the agent side.
enum In {
    Prompt(String),
    Cancel,
    Mode(String),
    Auth(String),
}

struct Opts {
    agent: Option<String>,
    cwd: PathBuf,
    thoughts: bool,
}

fn usage() -> ! {
    eprintln!("usage: apex-acp [-agent claude|codex|CMD] [-cwd DIR] [-thoughts]");
    std::process::exit(2);
}

fn parse_args() -> Opts {
    let mut opts = Opts { agent: std::env::var("APEX_ACP_AGENT").ok().filter(|s| !s.is_empty()), cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")), thoughts: false };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-agent" | "--agent" => opts.agent = Some(args.next().unwrap_or_else(|| usage())),
            "-cwd" | "--cwd" => {
                let d = PathBuf::from(args.next().unwrap_or_else(|| usage()));
                opts.cwd = if d.is_absolute() { d } else { opts.cwd.join(d) };
            }
            "-thoughts" | "--thoughts" => opts.thoughts = true,
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
    let thoughts = opts.thoughts;
    let name = label(opts.agent.as_deref());
    let window = std::thread::spawn(move || match Win::run(cwd, name, thoughts, out_rx, in_tx) {
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

/// What a window of this agent's is called: `DIR/+claude`, as `+Errors`
/// is called.
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
    let (npx, node_bin) = npx().ok_or_else(|| format!("{bin} is not installed and there is no npx to fetch it with: install node, or -agent CMD"))?;
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
    let cwd = opts.cwd.clone();
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
        .connect_with(agent, |conn: ConnectionTo<Agent>| async move {
            let caps = ClientCapabilities::new().fs(FileSystemCapabilities::new().read_text_file(true).write_text_file(true));
            let init = conn
                .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(caps).client_info(Implementation::new("apex-acp", env!("CARGO_PKG_VERSION"))))
                .block_task()
                .await?;
            let name = init.agent_info.map(|i| format!("{} {}", i.name, i.version)).unwrap_or_else(|| "the agent".to_string());
            let auth: Vec<(String, String)> = init.auth_methods.iter().map(|m| (m.id().0.to_string(), m.name().to_string())).collect();
            let _ = out.send(Out::Hello { agent: name, auth });
            // a session, logging in first when the agent asks for it
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
            let sid = session.session_id;
            let modes = session.modes.as_ref().map(|m| m.available_modes.iter().map(|x| (x.id.0.to_string(), x.name.clone())).collect()).unwrap_or_default();
            let mode = session.modes.as_ref().map(|m| m.current_mode_id.0.to_string());
            let _ = out.send(Out::Ready { modes, mode });

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
                        Some(In::Cancel) | Some(In::Auth(_)) => continue,
                        None => return Ok(()),
                    },
                };
                let prompt = PromptRequest::new(sid.clone(), vec![ContentBlock::Text(TextContent::new(text))]);
                let mut sent = Box::pin(conn.send_request(prompt).block_task());
                let res = loop {
                    tokio::select! {
                        r = &mut sent => break r,
                        m = inbox.recv() => match m {
                            Some(In::Cancel) => { let _ = conn.send_notification(CancelNotification::new(sid.clone())); }
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

// ---- the window side ---------------------------------------------------------

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

struct Win {
    t: Tool,
    w: WindowId,
    /// Characters in the body, as far as we know.
    len: usize,
    /// Where our output goes; what follows is the user's draft.
    mark: usize,
    /// Whether our output stands at the start of a line.
    col0: bool,
    /// Offsets of status glyphs, by tool call (or permission) id.
    anchors: HashMap<String, usize>,
    /// Titles of tool calls seen.
    titles: HashMap<String, String>,
    /// What each tool call has already put in the window, in order:
    /// an update carries the whole list again, and only what is new of
    /// it is worth a line.
    said: HashMap<String, Vec<String>>,
    /// The plan block, replaced in place.
    plan: Option<(usize, usize)>,
    pending: VecDeque<Pending>,
    busy: bool,
    thoughts: bool,
    verbs: HashMap<RuleId, &'static str>,
    /// The session's directory, which paths are shown relative to.
    cwd: PathBuf,
    /// The modes the agent offers (id, name), and the one it is in.
    modes: Vec<(String, String)>,
    mode: Option<String>,
    /// How the agent can be logged in to, when it is not.
    auth: Vec<(String, String)>,
    /// The agent's slash commands, as it last said.
    commands: Vec<(String, String)>,
    inbox: tokio::sync::mpsc::UnboundedSender<In>,
}

const VERBS: [&str; 9] = ["Send", "Cancel", "Allow", "Always", "Deny", "Never", "Mode", "Commands", "Login"];

impl Win {
    fn run(cwd: PathBuf, label: String, thoughts: bool, out: mpsc::Receiver<Out>, inbox: tokio::sync::mpsc::UnboundedSender<In>) -> apex_tool::Result<()> {
        let mut t = Tool::attach("acp")?;
        let name = format!("{}/+{label}", cwd.display().to_string().trim_end_matches('/'));
        let w = t.new_window(&name)?;
        t.watch(w)?;
        let _ = t.set_live(w, true);
        let mut verbs = HashMap::new();
        for v in VERBS {
            verbs.insert(t.offer(Rule::verb(v).window(w))?, v);
        }
        let mut win = Win { t, w, len: 0, mark: 0, col0: true, anchors: HashMap::new(), titles: HashMap::new(), said: HashMap::new(), plan: None, pending: VecDeque::new(), busy: false, thoughts, verbs, cwd, modes: Vec::new(), mode: None, auth: Vec::new(), commands: Vec::new(), inbox };

        loop {
            // the agent's news first, then the window's
            while let Ok(o) = out.try_recv() {
                win.take(o)?;
            }
            let ev = win.t.next_event(Some(Duration::from_millis(20)))?;
            if ev.is_some() && std::env::var_os("APEX_ACP_DEBUG").is_some() {
                eprintln!("acp: {ev:?} len={} mark={} busy={}", win.len, win.mark, win.busy);
            }
            match ev {
                None if !win.t.windows().iter().any(|x| x.id == win.w) => return Ok(()),
                None => {}
                Some(Event::Deleted { window }) if window == win.w => return Ok(()),
                Some(Event::Edit(e)) if e.window == win.w => win.edit(e.q0, e.nd, &e.text)?,
                Some(Event::Plumb(p)) => {
                    let taken = match win.verbs.get(&p.rule).copied() {
                        Some(v) => win.verb(v, &p.text)?,
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
        self.len = self.len + ni - nd;
        self.mark = shift(self.mark).unwrap_or(q0.min(self.len));
        self.anchors.retain(|_, p| match shift(*p) {
            Some(n) => {
                *p = n;
                true
            }
            None => false,
        });
        self.plan = self.plan.and_then(|(a, b)| Some((shift(a)?, shift(b)?)));
    }

    fn splice(&mut self, q0: usize, q1: usize, text: &str) -> apex_tool::Result<()> {
        self.t.replace(self.w, q0, q1, text)?;
        self.edited(q0, q1 - q0, text);
        Ok(())
    }

    /// Output at the mark.
    fn out(&mut self, s: &str) -> apex_tool::Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        let m = self.mark;
        self.splice(m, m, s)?;
        self.mark = m + s.chars().count();
        self.col0 = s.ends_with('\n');
        self.t.show(self.w, self.mark)
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

    /// Change a status glyph in place.
    fn glyph(&mut self, key: &str, g: &str) -> apex_tool::Result<()> {
        if let Some(&p) = self.anchors.get(key) {
            self.t.replace(self.w, p, p + 1, g)?;
        }
        Ok(())
    }

    /// The user's draft: what follows the mark.
    fn draft(&self) -> apex_tool::Result<String> {
        Ok(self.t.read(self.w)?.chars().skip(self.mark).collect())
    }

    // ---- the window's events ----

    fn edit(&mut self, q0: usize, nd: usize, text: &str) -> apex_tool::Result<()> {
        let at_end = q0 + nd == self.len;
        let in_draft = q0 >= self.mark;
        self.edited(q0, nd, text);
        // Enter on the last line sends, as in win
        if at_end && in_draft && text.ends_with('\n') && !self.busy {
            self.send(None)?;
        }
        Ok(())
    }

    fn verb(&mut self, v: &str, args: &str) -> apex_tool::Result<bool> {
        match v {
            "Send" => self.send(Some(args.trim()).filter(|s| !s.is_empty()))?,
            "Cancel" => {
                if self.busy {
                    let _ = self.inbox.send(In::Cancel);
                }
            }
            "Allow" => self.decide(PermissionOptionKind::AllowOnce)?,
            "Always" => self.decide(PermissionOptionKind::AllowAlways)?,
            "Deny" => self.decide(PermissionOptionKind::RejectOnce)?,
            "Never" => self.decide(PermissionOptionKind::RejectAlways)?,
            "Mode" => self.set_mode(args.trim())?,
            "Commands" => self.list_commands()?,
            "Login" => self.login(args.trim())?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Send the draft (or `text`) as the next prompt. The draft stays,
    /// as the transcript's record of it.
    fn send(&mut self, text: Option<&str>) -> apex_tool::Result<()> {
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
            let l = self.len;
            self.splice(l, l, "\n")?;
        }
        self.mark = self.len;
        self.col0 = true;
        self.busy = true;
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
        self.glyph(&p.key, if allow { "✓" } else { "✗" })?;
        self.anchors.remove(&p.key);
        let said = match kind {
            PermissionOptionKind::AllowOnce => "allowed",
            PermissionOptionKind::AllowAlways => "allowed, and not asked again",
            PermissionOptionKind::RejectOnce => "denied",
            _ => "denied, and not asked again",
        };
        if let Some(&at) = self.anchors.get(&p.offer) {
            self.splice(at, at + p.offered, said)?;
        }
        self.anchors.remove(&p.offer);
        Ok(())
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

    fn note(&mut self, text: &str) -> apex_tool::Result<()> {
        self.line_start()?;
        self.out(&format!("{text}\n"))
    }

    // ---- the agent's news ----

    fn take(&mut self, o: Out) -> apex_tool::Result<()> {
        match o {
            Out::Hello { agent, auth } => {
                self.auth = auth;
                self.line_start()?;
                self.out(&format!("{agent}\n"))
            }
            Out::NeedsAuth(why) => {
                self.line_start()?;
                self.out(&format!("no session: {why}\n"))?;
                match self.auth.clone().as_slice() {
                    [] => self.out("the agent offers no way to log in from here\n"),
                    ms => {
                        let list = ms.iter().map(|(id, name)| format!("{id} ({name})")).collect::<Vec<_>>().join(", ");
                        self.out(&format!("Login to go on: {list}\n"))
                    }
                }
            }
            Out::Ready { modes, mode } => {
                self.modes = modes;
                self.mode = mode;
                self.line_start()?;
                let m = self.mode.clone().map(|m| format!(" [{}]", self.mode_name(&m))).unwrap_or_default();
                self.out(&format!("ready{m}\n\n"))
            }
            Out::Mode(id) => {
                let name = self.mode_name(&id);
                self.mode = Some(id);
                self.note(&format!("mode: {name}"))
            }
            Out::Note(n) => {
                self.line_start()?;
                self.out(&format!("{n}\n"))
            }
            Out::Update(u) => self.update(u),
            Out::Permission { req, responder } => {
                let id = req.tool_call.tool_call_id.0.to_string();
                let seen = self.titles.contains_key(&id);
                let title = req.tool_call.fields.title.clone().or_else(|| self.titles.get(&id).cloned()).unwrap_or_else(|| id.clone());
                let n = self.pending.len() + self.anchors.len();
                let (key, offer) = (format!("perm:{n}"), format!("offer:{n}"));
                self.line_start()?;
                // the call has a line of its own already: this goes under it
                if seen {
                    self.out("    ")?;
                }
                self.anchors.insert(key.clone(), self.mark);
                self.out("? ")?;
                if !seen {
                    self.out(&format!("{title}: "))?;
                }
                self.anchors.insert(offer.clone(), self.mark);
                const OFFERED: &str = "Allow Always Deny Never";
                self.out(&format!("{OFFERED}\n"))?;
                self.pending.push_back(Pending { key, offer, offered: OFFERED.chars().count(), options: req.options, responder });
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
            Out::Turn(res) => {
                self.busy = false;
                self.line_start()?;
                match res {
                    Ok(StopReason::EndTurn) => {}
                    Ok(StopReason::Cancelled) => self.out("(cancelled)\n")?,
                    Ok(r) => self.out(&format!("({r:?})\n"))?,
                    Err(e) => self.out(&format!("(error: {e})\n"))?,
                }
                self.out("\n")?;
                // type-ahead: a line finished while the agent was busy goes next
                let draft = self.draft()?;
                if draft.ends_with('\n') && !draft.trim().is_empty() {
                    self.send(None)?;
                }
                Ok(())
            }
            Out::Gone(why) => {
                self.busy = false;
                self.line_start()?;
                self.out(&format!("{why}\n"))
            }
        }
    }

    fn update(&mut self, u: SessionUpdate) -> apex_tool::Result<()> {
        match u {
            SessionUpdate::AgentMessageChunk(c) => self.out(&content_text(&c.content)),
            SessionUpdate::AgentThoughtChunk(c) => {
                if self.thoughts {
                    self.out_prefixed("  · ", &content_text(&c.content))?;
                }
                Ok(())
            }
            SessionUpdate::UserMessageChunk(_) => Ok(()),
            SessionUpdate::ToolCall(tc) => {
                let id = tc.tool_call_id.0.to_string();
                self.titles.insert(id.clone(), tc.title.clone());
                self.line_start()?;
                self.anchors.insert(id.clone(), self.mark);
                self.out(&format!("{} {}\n", status_glyph(tc.status), tc.title))?;
                self.locations(&id, &tc.locations)?;
                self.contents(&id, &tc.content)
            }
            SessionUpdate::ToolCallUpdate(up) => self.tool_update(&up),
            SessionUpdate::Plan(plan) => self.show_plan(&plan),
            SessionUpdate::CurrentModeUpdate(m) => {
                let id = m.current_mode_id.0.to_string();
                let name = self.mode_name(&id);
                self.mode = Some(id);
                self.note(&format!("mode: {name}"))
            }
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
        if !self.anchors.contains_key(&id) && !self.titles.contains_key(&id) {
            // news of a call we never saw begin
            let title = f.title.clone().unwrap_or_else(|| id.clone());
            self.titles.insert(id.clone(), title.clone());
            self.line_start()?;
            self.anchors.insert(id.clone(), self.mark);
            self.out(&format!("{} {}\n", status_glyph(f.status.unwrap_or_default()), title))?;
        } else if let Some(s) = f.status {
            self.glyph(&id, status_glyph(s))?;
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

    /// Write the blocks of `key` that are not already there. Agents
    /// resend a tool call's whole content and locations with every
    /// update of it; what has not changed is already on the screen.
    fn fresh(&mut self, key: &str, blocks: Vec<String>) -> apex_tool::Result<()> {
        let old = self.said.entry(key.to_string()).or_default().clone();
        for (i, b) in blocks.iter().enumerate() {
            if old.get(i) == Some(b) {
                continue;
            }
            self.line_start()?;
            self.out(b)?;
        }
        self.said.insert(key.to_string(), blocks);
        Ok(())
    }

    /// Where a call works, one line per file: agents name the same file
    /// again as they learn where in it they are, and the second naming
    /// is no news.
    fn locations(&mut self, id: &str, locs: &[ToolCallLocation]) -> apex_tool::Result<()> {
        let key = format!("{id}/where");
        for l in locs {
            let p = self.shown(&l.path);
            let seen = self.said.entry(key.clone()).or_default();
            if seen.iter().any(|x| *x == p) {
                continue;
            }
            seen.push(p.clone());
            self.line_start()?;
            match l.line {
                Some(n) => self.out(&format!("    {p}:{n}\n"))?,
                None => self.out(&format!("    {p}\n"))?,
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
                ToolCallContent::Terminal(t) => b.push_str(&format!("    terminal {}\n", t.terminal_id.0)),
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
        self.line_start()?;
        let a = self.mark;
        self.out(&s)?;
        self.plan = Some((a, self.mark));
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
