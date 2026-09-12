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
//!     apex-acp [-agent CMD] [-cwd DIR] [-thoughts]
//!
//! The agent command defaults to `$APEX_ACP_AGENT`, else the SDK's
//! `npx -y @agentclientprotocol/claude-agent-acp@latest`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::mpsc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, FileSystemCapabilities, Implementation, InitializeRequest, NewSessionRequest,
    PermissionOption, PermissionOptionKind, Plan, PlanEntryStatus, PromptRequest, ReadTextFileRequest, ReadTextFileResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification,
    SessionUpdate, StopReason, TextContent, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, Agent, ConnectionTo, Responder};
use apex_tool::{Event, Rule, RuleId, Tool, WindowId};

/// What the agent side tells the window side.
enum Out {
    Ready { agent: String },
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
}

struct Opts {
    agent: Option<String>,
    cwd: PathBuf,
    thoughts: bool,
}

fn usage() -> ! {
    eprintln!("usage: apex-acp [-agent CMD] [-cwd DIR] [-thoughts]");
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
    let window = std::thread::spawn(move || match Win::run(cwd, thoughts, out_rx, in_tx) {
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

// ---- the agent side ----------------------------------------------------------

async fn agent_side(opts: Opts, out: mpsc::Sender<Out>, mut inbox: tokio::sync::mpsc::UnboundedReceiver<In>) -> Result<(), agent_client_protocol::Error> {
    let agent = match &opts.agent {
        Some(cmd) => AcpAgent::from_str(cmd)?,
        None => AcpAgent::claude_agent(),
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
            let agent = init.agent_info.map(|i| format!("{} {}", i.name, i.version)).unwrap_or_else(|| "agent".to_string());
            let session = conn.send_request(NewSessionRequest::new(cwd)).block_task().await?;
            let sid = session.session_id;
            let _ = out.send(Out::Ready { agent });

            let mut queue: VecDeque<String> = VecDeque::new();
            loop {
                let text = match queue.pop_front() {
                    Some(t) => t,
                    None => match inbox.recv().await {
                        Some(In::Prompt(t)) => t,
                        Some(In::Cancel) => continue,
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
    /// The plan block, replaced in place.
    plan: Option<(usize, usize)>,
    pending: VecDeque<Pending>,
    busy: bool,
    thoughts: bool,
    verbs: HashMap<RuleId, &'static str>,
    /// The session's directory, which paths are shown relative to.
    cwd: PathBuf,
    inbox: tokio::sync::mpsc::UnboundedSender<In>,
}

const VERBS: [&str; 6] = ["Send", "Cancel", "Allow", "Always", "Deny", "Never"];

impl Win {
    fn run(cwd: PathBuf, thoughts: bool, out: mpsc::Receiver<Out>, inbox: tokio::sync::mpsc::UnboundedSender<In>) -> apex_tool::Result<()> {
        let mut t = Tool::attach("acp")?;
        let name = format!("{}/+agent", cwd.display().to_string().trim_end_matches('/'));
        let w = t.new_window(&name)?;
        t.watch(w)?;
        let _ = t.set_live(w, true);
        let mut verbs = HashMap::new();
        for v in VERBS {
            verbs.insert(t.offer(Rule::verb(v).window(w))?, v);
        }
        let mut win = Win { t, w, len: 0, mark: 0, col0: true, anchors: HashMap::new(), titles: HashMap::new(), plan: None, pending: VecDeque::new(), busy: false, thoughts, verbs, cwd, inbox };
        win.out("starting the agent…\n")?;
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
        Ok(())
    }

    // ---- the agent's news ----

    fn take(&mut self, o: Out) -> apex_tool::Result<()> {
        match o {
            Out::Ready { agent } => {
                self.line_start()?;
                self.out(&format!("{agent} is ready.\n\n"))
            }
            Out::Update(u) => self.update(u),
            Out::Permission { req, responder } => {
                let id = req.tool_call.tool_call_id.0.to_string();
                let title = req.tool_call.fields.title.clone().or_else(|| self.titles.get(&id).cloned()).unwrap_or(id);
                let key = format!("perm:{}", self.pending.len() + self.anchors.len());
                self.line_start()?;
                self.anchors.insert(key.clone(), self.mark);
                self.out(&format!("? {title}: Allow Always Deny Never\n"))?;
                self.pending.push_back(Pending { key, options: req.options, responder });
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
                let _ = match std::fs::write(&req.path, req.content) {
                    Ok(()) => responder.respond(WriteTextFileResponse::new()),
                    Err(e) => responder.respond_with_internal_error(format!("{}: {e}", req.path.display())),
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
                self.locations(&tc.locations)?;
                self.contents(&tc.content)
            }
            SessionUpdate::ToolCallUpdate(up) => self.tool_update(&up),
            SessionUpdate::Plan(plan) => self.show_plan(&plan),
            SessionUpdate::CurrentModeUpdate(m) => {
                self.line_start()?;
                self.out(&format!("mode: {}\n", m.current_mode_id.0))
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
            self.locations(locs)?;
        }
        if let Some(c) = &f.content {
            self.contents(c)?;
        }
        Ok(())
    }

    /// A path as shown: relative to the session's directory when under it.
    fn shown(&self, p: &Path) -> String {
        p.strip_prefix(&self.cwd).unwrap_or(p).display().to_string()
    }

    fn locations(&mut self, locs: &[ToolCallLocation]) -> apex_tool::Result<()> {
        for l in locs {
            self.line_start()?;
            let p = self.shown(&l.path);
            match l.line {
                Some(n) => self.out(&format!("    {p}:{n}\n"))?,
                None => self.out(&format!("    {p}\n"))?,
            }
        }
        Ok(())
    }

    fn contents(&mut self, cs: &[ToolCallContent]) -> apex_tool::Result<()> {
        for c in cs {
            self.line_start()?;
            match c {
                ToolCallContent::Diff(d) => {
                    let p = self.shown(&d.path);
                    self.out(&format!("    edit {p}\n"))?
                }
                ToolCallContent::Terminal(t) => self.out(&format!("    terminal {}\n", t.terminal_id.0))?,
                ToolCallContent::Content(c) => {
                    let text = content_text(&c.content);
                    let lines: Vec<&str> = text.lines().collect();
                    let shown = lines.len().min(12);
                    let mut s = String::new();
                    for l in &lines[..shown] {
                        s.push_str("    ");
                        s.push_str(l);
                        s.push('\n');
                    }
                    if lines.len() > shown {
                        s.push_str(&format!("    … {} more lines\n", lines.len() - shown));
                    }
                    self.out(&s)?;
                }
                _ => {}
            }
        }
        Ok(())
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
