//! The windows: the pane, `DIR/-agents`, with a block an agent, and
//! beside it what each verb opens -- a transcript window,
//! `AGENTDIR/-claude+ID`, named for the agent's own directory so that a
//! `path:line` in it is B3'd from where the agent worked; its page,
//! `+Preview`; its changes, `+diff`, at the root of its repository; and
//! the pane's own `+history`. All are the tool's own (`set_owner`):
//! what is in them is the agents' doing and not a file's contents.
//!
//! Nothing here waits on anything: the logs' directory and each open
//! transcript's are watched (directories, not files, as the server's
//! own watcher does: what is appended to is one thing, what is renamed
//! into place another), a change wakes the loop, and what is new is
//! read and written; the window's events are taken in between. A slow
//! pass every few seconds asks after the agents' processes and catches
//! anything a watch let by.
//!
//! What the pane says back to an agent goes the way apex already has:
//! a decision is an event in the agent's log, which its hook is
//! waiting to read; a prompt is typed into the agent's terminal by
//! `apex term send`; an agent is started by `Newterm`.
//!
//! An agent running in a terminal of this very session is known by
//! the session and window its hooks recorded, and the pane offers its
//! verbs on that window too -- `Transcript`, `Preview`, `Changes` in
//! the terminal's own tools menu while the agent runs, and `Allow
//! Deny Ask` while it asks -- so the agent's window is the place to
//! answer it from, as apex-acp's is, with nothing added to the agent.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use apex_tool::{Event, Range, Rule, RuleId, Tool, WindowId, END};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::agents::{self, Agents, State};
use crate::event::{self, Tail};
use crate::history::{self, Past};
use crate::page;
use crate::transcript::{self, Op, Parser, Writer};
use crate::vcs;

pub struct Opts {
    pub cwd: PathBuf,
    /// Where the logs are.
    pub dir: PathBuf,
    pub thoughts: bool,
    /// Every agent, wherever it is, rather than those under `cwd`.
    pub all: bool,
    /// Only the agents started in this apex session (`-s`).
    pub session_only: bool,
    /// No notes in +Errors when an agent asks or fails.
    pub quiet: bool,
    /// Where the agents keep their sessions: `~/.claude`, `~/.codex`.
    pub claude_home: PathBuf,
    pub codex_home: PathBuf,
}

impl Opts {
    pub fn new(cwd: PathBuf, dir: PathBuf) -> Opts {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
        Opts { cwd, dir, thoughts: false, all: false, session_only: false, quiet: false, claude_home: home.join(".claude"), codex_home: home.join(".codex") }
    }
}

/// How often the slow pass runs: the agents' processes asked after,
/// and the logs and transcripts looked at whether or not a watch said
/// to, so that nothing is missed for longer than this.
const SLOW: Duration = Duration::from_secs(5);
/// How often the pane is written again with nothing new heard: the
/// minutes an agent has been quiet.
const MINUTES: Duration = Duration::from_secs(15);

/// The verbs in the pane's tools menu, in the order they are reached
/// for. The rest are unlisted: two of the three that answer a question
/// are written into the block to be B2'd where they stand (`Ask` is
/// listed, being the way to hand a question back with nothing to
/// click), and `Resume` means nothing without an id.
const VERBS: [&str; 8] = ["Open", "Goto", "Preview", "Changes", "Send", "Start", "History", "Ask"];
const UNLISTED: [&str; 3] = ["Allow", "Deny", "Resume"];

/// A transcript window.
struct Detail {
    session: String,
    w: WindowId,
    /// The file, while there is one to read.
    tail: Option<Tail>,
    parser: Box<dyn Parser>,
    wr: Writer,
    /// What we last told the window about the work behind it.
    pulsing: bool,
    /// `Send` in its tag: what is typed after the end is the prompt.
    send: RuleId,
}

pub struct Pane {
    t: Tool,
    w: WindowId,
    opts: Opts,
    agents: Agents,
    /// Each session's log, read as it grows.
    logs: HashMap<String, Tail>,
    /// What the window says: the header, the blocks and where they
    /// start, the footer.
    header: String,
    blocks: Vec<(String, String)>,
    starts: Vec<usize>,
    footer: String,
    details: HashMap<WindowId, Detail>,
    /// The pages open, each showing an agent's last exchange.
    pages: HashMap<WindowId, String>,
    /// The diff windows open, by agent.
    diffs: HashMap<WindowId, String>,
    /// The history window, while `History` has one open, and the
    /// sessions it last listed, by id, which is what an id B3'd
    /// anywhere is looked up in.
    hist: Option<WindowId>,
    past: HashMap<String, Past>,
    verbs: HashMap<RuleId, &'static str>,
    look: RuleId,
    /// B3 on a session's id anywhere: its transcript.
    look_any: RuleId,
    pulsing: bool,
    /// Whether we have written the pane since we last said it was
    /// clean.
    dirty: bool,
    last_slow: Instant,
    last_render: Instant,
    home: Option<String>,
    /// The watcher, and each directory watched with how many
    /// transcripts want it (the logs' directory is wanted always).
    watcher: Option<RecommendedWatcher>,
    woken: mpsc::Receiver<()>,
    watched: BTreeMap<PathBuf, usize>,
    /// Our presence file, which says to the hooks that there is a pane
    /// to ask.
    presence: PathBuf,
    /// This session's id: an agent whose hooks recorded it runs here.
    session: String,
    /// The agents' own windows in this session, with the verbs offered
    /// on each: the ones for as long as the agent runs, and the ones
    /// for as long as it asks.
    targets: HashMap<WindowId, Target>,
}

/// An agent's terminal in this session, and what is offered on it.
struct Target {
    session: String,
    rules: Vec<RuleId>,
    asking: Vec<RuleId>,
}

/// The verbs on an agent's own window: the transcript, named as
/// apex-acp names its own; the page; the changes.
const TARGET_VERBS: [&str; 3] = ["Transcript", "Preview", "Changes"];
const ASKING_VERBS: [&str; 3] = ["Allow", "Deny", "Ask"];

impl Pane {
    /// The pane in the session this program was started in.
    pub fn run(opts: Opts) -> apex_tool::Result<()> {
        let t = Tool::attach("agents")?;
        Pane::start(t, opts)?.serve()
    }

    /// The pane, made and written once; `serve` keeps it.
    pub fn start(mut t: Tool, opts: Opts) -> apex_tool::Result<Pane> {
        let name = format!("{}/-agents", opts.cwd.display().to_string().trim_end_matches('/'));
        let w = t.new_window(&name)?;
        let _ = t.set_owner(w, true);
        let _ = t.set_tag(w, &format!("Look {}", VERBS.join(" ")));
        // the verbs, with dot in a block or the agent named after them;
        // B3 anywhere in a block; and B3 on a session's id anywhere
        let mut verbs = HashMap::new();
        for v in VERBS {
            verbs.insert(t.offer(Rule::verb(v).window(w))?, v);
        }
        for v in UNLISTED {
            verbs.insert(t.offer(Rule::verb(v).window(w).unlisted())?, v);
        }
        let look = t.offer(Rule::plumb().window(w).priority(10))?;
        // a hex word with the shape of an id, wherever it is: one that
        // names no session of ours is handed back, and B3 does what it
        // always does with it
        let look_any = t.offer(Rule::plumb().text(r"[0-9a-fA-F]{6,}(-[0-9a-fA-F]{4,})*").priority(-1))?;
        let home = std::env::var("HOME").ok();
        // a change under a watched directory wakes the loop; what it
        // was is not said, since looking is cheap and the watch is
        // only ever the reason to
        let (tx, woken) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if is_change(&ev.kind) {
                    let _ = tx.send(());
                }
            }
        })
        .ok();
        // the directory must be there to be watched: a hook makes it
        // otherwise, and the watch would miss the making. And we say we
        // are here, for the hooks that have a question
        let _ = std::fs::create_dir_all(event::panes_dir(&opts.dir));
        let presence = event::panes_dir(&opts.dir).join(std::process::id().to_string());
        let _ = std::fs::write(&presence, b"");
        let (session, _) = t.session();
        let mut pane = Pane { t, w, opts, agents: Agents::default(), logs: HashMap::new(), header: String::new(), blocks: Vec::new(), starts: Vec::new(), footer: String::new(), details: HashMap::new(), pages: HashMap::new(), diffs: HashMap::new(), hist: None, past: HashMap::new(), verbs, look, look_any, pulsing: false, dirty: false, last_slow: Instant::now(), last_render: Instant::now(), home, watcher, woken, watched: BTreeMap::new(), presence, session, targets: HashMap::new() };
        let dir = pane.opts.dir.clone();
        pane.watch(&dir);
        pane.look()?;
        pane.sweep();
        pane.render()?;
        Ok(pane)
    }

    /// Until the pane is deleted, or the session is over.
    pub fn serve(&mut self) -> apex_tool::Result<()> {
        loop {
            let woken = self.woken.try_iter().count() > 0;
            let slow = self.last_slow.elapsed() >= SLOW;
            if woken || slow {
                if slow {
                    self.last_slow = Instant::now();
                    self.check_alive();
                    let _ = std::fs::write(&self.presence, b"");
                }
                // the transcripts first, so that a hook speaking of a
                // call finds its line already there
                self.tick_details()?;
                self.look()?;
                self.sweep();
                self.render()?;
                self.sync_targets()?;
            } else if self.last_render.elapsed() >= MINUTES {
                self.render()?;
            }
            match self.t.next_event(Some(Duration::from_millis(20)))? {
                None if !self.t.windows().iter().any(|x| x.id == self.w) => return Ok(()),
                None => {}
                Some(Event::Deleted { window }) if window == self.w => return Ok(()),
                Some(Event::Deleted { window }) => {
                    self.pages.remove(&window);
                    self.diffs.remove(&window);
                    if self.hist == Some(window) {
                        self.hist = None;
                    }
                    if let Some(d) = self.details.remove(&window) {
                        self.t.withdraw(d.send);
                        if let Some(dir) = d.tail.as_ref().and_then(|t| t.path.parent()).map(Path::to_path_buf) {
                            self.unwatch(&dir);
                        }
                    }
                }
                Some(Event::Edit(e)) => {
                    if let Some(d) = self.details.get_mut(&e.window) {
                        d.wr.shift(e.q0, e.nd, e.text.chars().count());
                    }
                }
                Some(Event::Plumb(p)) => {
                    // in an agent's own window, the verb means that agent
                    let target = p.window.and_then(|w| self.targets.get(&w).map(|t| t.session.clone()));
                    let taken = match (self.verbs.get(&p.rule).copied(), target) {
                        (Some(v), Some(key)) => self.verb_for(v, &key)?,
                        (Some(v), None) => self.verb(v, &p.text, p.at)?,
                        (None, _) if p.rule == self.look => self.open_at(p.sel.or(p.at))?,
                        (None, _) if p.rule == self.look_any => self.open_id(&p.text)?,
                        (None, _) => match p.window.and_then(|w| self.details.get(&w).map(|d| (d.send, w))) {
                            Some((send, w)) if send == p.rule => self.send_from_detail(w, &p.text)?,
                            _ => false,
                        },
                    };
                    self.t.answer(&p, taken)?;
                }
                Some(_) => {}
            }
        }
    }

    /// A verb in an agent's own window: about that agent, whatever dot
    /// or the words say.
    fn verb_for(&mut self, v: &str, key: &str) -> apex_tool::Result<bool> {
        match v {
            "Transcript" => {
                self.open_detail(key)?;
                Ok(true)
            }
            "Preview" => self.preview_for(key),
            "Changes" => self.changes_for(key),
            "Allow" | "Deny" | "Ask" => self.decide_for(v, key),
            _ => Ok(false),
        }
    }

    /// The agents running in terminals of this session, and what is
    /// offered on their windows: verbs for as long as the agent runs,
    /// and the three that answer a question for as long as it asks.
    /// A window gone, or an agent gone, takes its verbs away.
    fn sync_targets(&mut self) -> apex_tool::Result<()> {
        let windows: Vec<WindowId> = self.t.windows().iter().map(|x| x.id).collect();
        // what should be targeted now
        let want: Vec<(WindowId, String, bool)> = self
            .agents
            .ordered()
            .iter()
            .filter(|a| a.apex.as_deref() == Some(self.session.as_str()) && !self.session.is_empty())
            .filter_map(|a| a.win.map(|w| (WindowId(w), a.session.clone(), a.state == State::Asking && a.decided.is_none())))
            .filter(|(w, _, _)| windows.contains(w) && *w != self.w)
            .collect();
        // gone: the window, or the agent, or another agent in its place
        let stale: Vec<WindowId> = self.targets.iter().filter(|(w, t)| !want.iter().any(|(ww, s, _)| ww == *w && *s == t.session)).map(|(w, _)| *w).collect();
        for w in stale {
            if let Some(t) = self.targets.remove(&w) {
                for r in t.rules.into_iter().chain(t.asking) {
                    self.verbs.remove(&r);
                    self.t.withdraw(r);
                }
            }
        }
        for (w, session, asking) in want {
            if !self.targets.contains_key(&w) {
                let mut rules = Vec::new();
                for v in TARGET_VERBS {
                    let r = self.t.offer(Rule::verb(v).window(w).priority(5))?;
                    self.verbs.insert(r, v);
                    rules.push(r);
                }
                self.targets.insert(w, Target { session: session.clone(), rules, asking: Vec::new() });
            }
            let t = self.targets.get_mut(&w).expect("just made");
            if asking && t.asking.is_empty() {
                for v in ASKING_VERBS {
                    let r = self.t.offer(Rule::verb(v).window(w).priority(5))?;
                    self.verbs.insert(r, v);
                    t.asking.push(r);
                }
            } else if !asking && !t.asking.is_empty() {
                for r in std::mem::take(&mut t.asking) {
                    self.verbs.remove(&r);
                    self.t.withdraw(r);
                }
            }
        }
        Ok(())
    }

    /// A verb of the pane's.
    fn verb(&mut self, v: &str, args: &str, at: Option<Range>) -> apex_tool::Result<bool> {
        match v {
            "Open" => self.open_verb(args, at),
            "Goto" => self.goto_verb(args, at),
            "Preview" => self.preview_verb(args, at),
            "Changes" => self.changes_verb(args, at),
            "Send" => self.send_verb(args, at),
            "Start" => self.start_verb(args),
            "Resume" => self.resume_verb(args),
            "History" => self.toggle_history(),
            "Allow" | "Deny" | "Ask" => self.decide(v, at),
            _ => Ok(false),
        }
    }

    // ---- the watches ----

    /// Watch a directory, one more wanting it.
    fn watch(&mut self, dir: &Path) {
        let n = self.watched.entry(dir.to_path_buf()).or_insert(0);
        *n += 1;
        if *n == 1 {
            if let Some(w) = self.watcher.as_mut() {
                let _ = w.watch(dir, RecursiveMode::NonRecursive);
            }
        }
    }

    /// One fewer wanting it; unwatched when none does.
    fn unwatch(&mut self, dir: &Path) {
        let Some(n) = self.watched.get_mut(dir) else { return };
        *n = n.saturating_sub(1);
        if *n == 0 {
            self.watched.remove(dir);
            if let Some(w) = self.watcher.as_mut() {
                let _ = w.unwatch(dir);
            }
        }
    }

    // ---- the logs ----

    /// What the logs say that they did not last time.
    fn look(&mut self) -> apex_tool::Result<()> {
        let Ok(rd) = std::fs::read_dir(&self.opts.dir) else { return Ok(()) };
        let mut seen = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            if !path.is_file() {
                continue;
            }
            let Some(session) = event::session_of(&path) else { continue };
            seen.push(session.clone());
            let tail = self.logs.entry(session.clone()).or_insert_with(|| Tail::new(path));
            let lines = tail.lines();
            if lines.is_empty() {
                continue;
            }
            for ev in event::events(&lines) {
                if std::env::var_os("APEX_AGENT_DEBUG").is_some() {
                    eprintln!("agents: {} {} {:?}", ev.session, ev.event, ev.title);
                }
                let was = self.agents.get(&ev.session).map(|a| a.state);
                self.agents.apply(&ev);
                self.heard(&ev, was)?;
            }
        }
        // a log taken away is an agent gone
        let gone: Vec<String> = self.logs.keys().filter(|s| !seen.contains(s)).cloned().collect();
        for s in gone {
            self.logs.remove(&s);
            self.ended(&s)?;
        }
        Ok(())
    }

    /// A hook's word about a call, in the transcript window when one is
    /// open: running, wanted, refused. A turn's end, in the page. An
    /// agent come to want you, or to grief, in +Errors, where its id
    /// is B3'd for the transcript.
    fn heard(&mut self, ev: &event::Event, was: Option<State>) -> apex_tool::Result<()> {
        if ev.event == "Stop" {
            self.show_page(&ev.session)?;
        }
        if !self.opts.quiet {
            if let Some(a) = self.agents.get(&ev.session) {
                let note = match (was, a.state) {
                    (Some(State::Asking), State::Asking) | (Some(State::Failed), State::Failed) => None,
                    (_, State::Asking) => Some(format!("{} {} asks: {}\n", a.kind, a.short(), a.asked.as_deref().map(transcript::brief).unwrap_or_default())),
                    (_, State::Failed) => Some(format!("{} {} failed: {}\n", a.kind, a.short(), a.why.as_deref().unwrap_or("the turn failed"))),
                    _ => None,
                };
                if let Some(n) = note {
                    self.t.errors(None, &n)?;
                }
            }
        }
        let Some(d) = self.details.values_mut().find(|d| d.session == ev.session) else { return Ok(()) };
        let glyph = match ev.event.as_str() {
            "PreToolUse" => Some("▶"),
            "PermissionRequest" => Some("?"),
            "PermissionDenied" | "PostToolUseFailure" => Some("✗"),
            "Decision" if ev.kind.as_deref() == Some("deny") => Some("✗"),
            "Decision" if ev.kind.as_deref() == Some("allow") => Some("▶"),
            _ => None,
        };
        if let (Some(id), Some(g)) = (&ev.call, glyph) {
            if let Some(op) = d.wr.glyph(id, g) {
                apply(&mut self.t, d.w, op)?;
            }
        }
        Ok(())
    }

    /// An agent gone: its block goes, its transcript window says so.
    fn ended(&mut self, session: &str) -> apex_tool::Result<()> {
        self.agents.remove(session);
        if let Some(d) = self.details.values_mut().find(|d| d.session == session) {
            let ops = d.wr.item(&transcript::Item::Note("the agent is gone".to_string()));
            for op in ops {
                apply(&mut self.t, d.w, op)?;
            }
            if d.pulsing {
                d.pulsing = false;
                let _ = self.t.set_working(d.w, false);
            }
        }
        Ok(())
    }

    /// Sessions over are cleaned away, log and all; those whose
    /// process is gone were found so by `check_alive`.
    fn sweep(&mut self) {
        let over: Vec<String> = self.agents.map.values().filter(|a| a.state == State::Ended).map(|a| a.session.clone()).collect();
        for s in over {
            let _ = std::fs::remove_file(event::log_path(&self.opts.dir, &s));
            self.logs.remove(&s);
            let _ = self.ended(&s);
        }
    }

    /// An agent killed outright sends no `SessionEnd`: its process is
    /// asked after instead.
    fn check_alive(&mut self) {
        for a in self.agents.map.values_mut() {
            if a.state == State::Ended {
                continue;
            }
            if let Some(pid) = a.pid {
                if !event::alive(pid as i32) {
                    a.state = State::Ended;
                }
            }
        }
    }

    // ---- the pane ----

    /// The directory the pane's name filters by, unless `-all`.
    fn under(&self) -> Option<String> {
        if self.opts.all {
            return None;
        }
        let d = self.opts.cwd.display().to_string();
        let home = self.home.as_deref().unwrap_or("");
        // a pane in the home directory, or the root, is one of everything
        if d.trim_end_matches('/') == home.trim_end_matches('/') || d == "/" {
            return None;
        }
        Some(d)
    }

    /// The pane as it should read now, written where it differs: a block
    /// that changed is written in place, and the whole only when the
    /// blocks are not the ones they were, in the order they were.
    fn render(&mut self) -> apex_tool::Result<()> {
        let now = event::now_ms();
        let under = self.under();
        let session = (self.opts.session_only && !self.opts.all).then_some(self.session.as_str());
        let (header, blocks, footer) = agents::pane(&self.agents.ordered(), now, self.home.as_deref(), under.as_deref(), session);
        let same_keys = header == self.header && footer == self.footer && blocks.len() == self.blocks.len() && blocks.iter().zip(&self.blocks).all(|(a, b)| a.0 == b.0);
        if same_keys {
            // last first, so that what comes before keeps its offsets
            for i in (0..blocks.len()).rev() {
                if blocks[i].1 != self.blocks[i].1 {
                    let (q0, q1) = (self.starts[i], self.starts[i] + self.blocks[i].1.chars().count());
                    self.t.replace(self.w, q0, q1, &blocks[i].1)?;
                    self.dirty = true;
                }
            }
        } else {
            let (text, _) = agents::pane_text(&header, &blocks, &footer);
            self.t.replace(self.w, 0, END, &text)?;
            self.dirty = true;
        }
        let (_, starts) = agents::pane_text(&header, &blocks, &footer);
        self.header = header;
        self.blocks = blocks;
        self.starts = starts;
        self.footer = footer;
        self.last_render = Instant::now();
        // the handle pulses while any agent works, as the agent's own
        // window would
        let working = self.agents.working();
        if working != self.pulsing {
            self.pulsing = working;
            let _ = self.t.set_working(self.w, working);
        }
        // and is clean while none does: what it says is whole, and
        // nothing is going on behind it. Written while agents work, it
        // is dirty as any window a program is writing is, and the
        // pulse says why
        if !working && self.dirty {
            self.dirty = false;
            let _ = self.t.set_clean(self.w);
        }
        // and each transcript's, while its agent does
        let mut pulse = Vec::new();
        for d in self.details.values_mut() {
            let on = self.agents.get(&d.session).is_some_and(|a| a.state == State::Working);
            if on != d.pulsing {
                d.pulsing = on;
                pulse.push((d.w, on));
            }
        }
        for (w, on) in pulse {
            let _ = self.t.set_working(w, on);
        }
        Ok(())
    }

    /// The block the character at `at` is in.
    fn block_at(&self, at: usize) -> Option<&str> {
        for (i, (key, text)) in self.blocks.iter().enumerate() {
            let q0 = self.starts[i];
            if at >= q0 && at < q0 + text.chars().count() {
                return Some(key);
            }
        }
        None
    }

    /// B3 in the pane: the agent under the pointer, opened. Outside any
    /// block it is handed back, and B3 does what it always does.
    fn open_at(&mut self, at: Option<Range>) -> apex_tool::Result<bool> {
        let Some(r) = at else { return Ok(false) };
        let Some(key) = self.block_at(r.q0).map(String::from) else { return Ok(false) };
        self.open_detail(&key)?;
        Ok(true)
    }

    /// B3 on an id anywhere: an agent's transcript, or a past
    /// session's. A word that names neither is handed back.
    fn open_id(&mut self, text: &str) -> apex_tool::Result<bool> {
        let want = text.trim();
        if let Some(key) = self.agents.by_id(want).map(|a| a.session.clone()) {
            self.open_detail(&key)?;
            return Ok(true);
        }
        let hits: Vec<Past> = self.past.values().filter(|p| p.id.starts_with(want)).cloned().collect();
        if let [one] = hits.as_slice() {
            self.open_past(one.clone())?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The agent a verb means: with words after it, the one whose id
    /// begins so (or whose kind or directory is named); without, the
    /// one dot is in, or the only one there is. `None` when there is no
    /// such one, and +Errors says why, unless dot is simply outside any
    /// block with nothing to choose from.
    fn meant(&mut self, verb: &str, args: &str, at: Option<Range>) -> apex_tool::Result<Option<String>> {
        let want = args.trim();
        if want.is_empty() {
            if let Some(key) = at.and_then(|r| self.block_at(r.q0).map(String::from)) {
                return Ok(Some(key));
            }
            return match self.blocks.len() {
                0 => Ok(None),
                1 => Ok(Some(self.blocks[0].0.clone())),
                n => {
                    self.t.errors(None, &format!("{verb}: which of the {n}? B2 it with dot in the agent's block, or say its id: {verb} ID\n"))?;
                    Ok(None)
                }
            };
        }
        let keys: Vec<String> = self.agents.ordered().iter().filter(|a| a.session.starts_with(want) || a.kind == want || a.cwd.ends_with(want)).map(|a| a.session.clone()).collect();
        match keys.as_slice() {
            [one] => Ok(Some(one.clone())),
            [] => {
                self.t.errors(None, &format!("{verb} {want}: no such agent\n"))?;
                Ok(None)
            }
            many => {
                self.t.errors(None, &format!("{verb} {want}: {} agents match; say more of the id\n", many.len()))?;
                Ok(None)
            }
        }
    }

    /// `Open`: the agent's transcript.
    fn open_verb(&mut self, args: &str, at: Option<Range>) -> apex_tool::Result<bool> {
        match self.meant("Open", args, at)? {
            Some(key) => {
                self.open_detail(&key)?;
                Ok(true)
            }
            None => Ok(!args.trim().is_empty()),
        }
    }

    /// `Goto`: the window the agent was started in, in whatever session
    /// that was -- the hooks carry the `apexsession` and `winid` apex
    /// put in its environment -- so the pane is a way straight to any
    /// agent. One started outside apex has nowhere to go to, and
    /// +Errors says so.
    fn goto_verb(&mut self, args: &str, at: Option<Range>) -> apex_tool::Result<bool> {
        let Some(key) = self.meant("Goto", args, at)? else { return Ok(!args.trim().is_empty()) };
        let Some(a) = self.agents.get(&key) else { return Ok(true) };
        let (kind, short) = (a.kind.clone(), a.short());
        match (a.apex.clone(), a.win) {
            (Some(session), Some(win)) => self.t.switch(&session, Some(WindowId(win)))?,
            _ => self.t.errors(None, &format!("Goto {short}: {kind} was not started in an apex window\n"))?,
        }
        Ok(true)
    }

    // ---- answering ----

    /// `Allow`, `Deny`, `Ask`: the pane's answer to the question an
    /// agent is asking, written into its log as a `Decision`, which is
    /// the record of it and what the hook, waiting, reads. `Ask` hands
    /// the question to the agent's own prompt. With dot outside any
    /// block, the one agent asking is meant.
    fn decide(&mut self, word: &str, at: Option<Range>) -> apex_tool::Result<bool> {
        let key = match at.and_then(|r| self.block_at(r.q0).map(String::from)) {
            Some(k) => k,
            None => {
                let asking: Vec<String> = self.agents.ordered().into_iter().filter(|a| a.state == State::Asking && a.decided.is_none()).map(|a| a.session.clone()).collect();
                match asking.as_slice() {
                    [one] => one.clone(),
                    [] => {
                        self.t.errors(None, &format!("{word}: no agent is asking\n"))?;
                        return Ok(true);
                    }
                    _ => {
                        self.t.errors(None, &format!("{word}: which? B2 it in the agent's block\n"))?;
                        return Ok(true);
                    }
                }
            }
        };
        self.decide_for(word, &key)
    }

    /// The answer, for the agent named.
    fn decide_for(&mut self, word: &str, key: &str) -> apex_tool::Result<bool> {
        let Some(a) = self.agents.get(key) else { return Ok(true) };
        let (Some(call), State::Asking) = (a.asking.clone(), a.state) else {
            self.t.errors(None, &format!("{word}: {} {} is not asking anything\n", a.kind, a.short()))?;
            return Ok(true);
        };
        let ev = event::Event { ms: event::now_ms(), agent: a.kind.clone(), event: "Decision".into(), session: a.session.clone(), call: Some(call), kind: Some(word.to_ascii_lowercase()), ..event::Event::default() };
        if let Err(e) = event::append(&self.opts.dir, &ev) {
            self.t.errors(None, &format!("{word}: {e}\n"))?;
        }
        Ok(true)
    }

    // ---- starting and sending ----

    /// `Start [claude|codex] [DIR]`: the agent in a new terminal
    /// beside the pane, in the pane's directory or the one named, which
    /// the hooks then pick up. `Newterm` is what apex starts terminals
    /// with, so that is what this is.
    fn start_verb(&mut self, args: &str) -> apex_tool::Result<bool> {
        let words: Vec<&str> = args.split_whitespace().collect();
        let (agent, dir) = match words.as_slice() {
            [] => ("claude", None),
            [a] if crate::install::AGENTS.contains(a) => (*a, None),
            [d] => ("claude", Some(*d)),
            [a, d] if crate::install::AGENTS.contains(a) => (*a, Some(*d)),
            [d, a] if crate::install::AGENTS.contains(a) => (*a, Some(*d)),
            _ => {
                self.t.errors(None, "Start: usage: Start [claude|codex] [DIR]\n")?;
                return Ok(true);
            }
        };
        self.start_agent(agent, dir, "")
    }

    /// `Resume ID`: the session taken up again, in a new terminal in
    /// the directory it was had in; the agent replays it there.
    fn resume_verb(&mut self, args: &str) -> apex_tool::Result<bool> {
        let want = args.trim();
        if want.is_empty() {
            self.t.errors(None, "Resume: which? Resume ID, from History\n")?;
            return Ok(true);
        }
        let hits: Vec<Past> = self.past.values().filter(|p| p.id.starts_with(want)).cloned().collect();
        let Some(p) = hits.first() else {
            self.t.errors(None, &format!("Resume {want}: no such session in the history; History lists them\n"))?;
            return Ok(true);
        };
        let (kind, cwd, id) = (p.kind.clone(), p.cwd.clone(), p.id.clone());
        let extra = match kind.as_str() {
            "codex" => format!("resume {id}"),
            _ => format!("--resume {id}"),
        };
        self.start_agent(&kind, Some(&cwd), &extra)
    }

    /// A terminal running `agent` in `dir`, by Newterm from the pane's
    /// window; a directory not the pane's is gone to first.
    fn start_agent(&mut self, agent: &str, dir: Option<&str>, extra: &str) -> apex_tool::Result<bool> {
        let here = self.opts.cwd.display().to_string();
        let dir = match dir {
            Some(d) if d.starts_with('/') => d.to_string(),
            Some(d) if d.starts_with("~/") => format!("{}/{}", self.home.as_deref().unwrap_or(""), &d[2..]),
            Some(d) => format!("{}/{d}", here.trim_end_matches('/')),
            None => here.clone(),
        };
        let cmd = if extra.is_empty() { agent.to_string() } else { format!("{agent} {extra}") };
        let text = if dir.trim_end_matches('/') == here.trim_end_matches('/') { format!("Newterm {cmd}") } else { format!("Newterm cd '{}' && exec {cmd}", dir.replace('\'', "'\\''")) };
        self.t.exec_in(Some(self.w), &text)?;
        Ok(true)
    }

    /// `Send TEXT` in the pane: typed into the agent's terminal, Enter
    /// after it, by `apex term send`, which reaches a terminal in any
    /// session. With no text, the snarf buffer is not ours to read:
    /// +Errors says to say it.
    fn send_verb(&mut self, args: &str, at: Option<Range>) -> apex_tool::Result<bool> {
        // the agent first, when the first word names one and more follows
        let (target, text) = match args.split_once(char::is_whitespace) {
            Some((first, rest)) if self.agents.by_id(first).is_some() || crate::install::AGENTS.contains(&first) => (first, rest.trim()),
            _ => ("", args.trim()),
        };
        if text.is_empty() {
            self.t.errors(None, "Send: say what to send: Send TEXT, or type it under the transcript and Send there\n")?;
            return Ok(true);
        }
        let Some(key) = self.meant("Send", target, at)? else { return Ok(true) };
        self.send_to(&key, text)
    }

    /// `Send` in a transcript window: what is typed after the end of
    /// the transcript is the prompt, or the words after `Send`. What
    /// was typed is taken away, since it comes back as the agent's
    /// record of it.
    fn send_from_detail(&mut self, w: WindowId, args: &str) -> apex_tool::Result<bool> {
        let Some(d) = self.details.get(&w) else { return Ok(false) };
        let (session, end) = (d.session.clone(), d.wr.len);
        let draft: String = self.t.read(w)?.chars().skip(end).collect();
        let text = if args.trim().is_empty() { draft.trim().to_string() } else { args.trim().to_string() };
        if text.is_empty() {
            self.t.errors(None, "Send: type the prompt after the end of the transcript, or say it: Send TEXT\n")?;
            return Ok(true);
        }
        if self.send_to(&session, &text)? && args.trim().is_empty() {
            self.t.replace(w, end, END, "")?;
            let _ = self.t.select(w, end, end);
        }
        Ok(true)
    }

    /// Text into the agent's terminal; whether it went.
    fn send_to(&mut self, key: &str, text: &str) -> apex_tool::Result<bool> {
        let Some(a) = self.agents.get(key) else { return Ok(false) };
        let (kind, short) = (a.kind.clone(), a.short());
        let (Some(session), Some(win)) = (a.apex.clone(), a.win) else {
            self.t.errors(None, &format!("Send: {kind} {short} was not started in an apex terminal; there is nowhere to type\n"))?;
            return Ok(false);
        };
        match term_send(&session, win, text) {
            Ok(()) => Ok(true),
            Err(e) => {
                self.t.errors(None, &format!("Send: {e}\n"))?;
                Ok(false)
            }
        }
    }

    // ---- the pages ----

    /// `Preview`: the agent's last exchange as a page beside the pane,
    /// what was asked and then the answer, rendered as apex-acp's is;
    /// again, to close it. It is written afresh as each turn ends, so
    /// it always shows the latest answer whole.
    fn preview_verb(&mut self, args: &str, at: Option<Range>) -> apex_tool::Result<bool> {
        let Some(key) = self.meant("Preview", args, at)? else { return Ok(!args.trim().is_empty()) };
        self.preview_for(&key)
    }

    /// The page for the agent named, or closed again.
    fn preview_for(&mut self, key: &str) -> apex_tool::Result<bool> {
        let key = key.to_string();
        if let Some((&w, _)) = self.pages.iter().find(|(_, s)| **s == key) {
            let _ = self.t.delete(w);
            self.pages.remove(&w);
            return Ok(true);
        }
        let Some(a) = self.agents.get(&key).cloned() else { return Ok(true) };
        let kind = if a.kind.is_empty() { "agent" } else { &a.kind };
        let name = format!("{}/-{kind}+{}+Preview", a.cwd.trim_end_matches('/'), a.short());
        let html = page::render(&self.t, Path::new(&a.cwd), a.exchange.as_ref().map(|x| (x.asked.as_str(), x.said.as_str())));
        let w = self.t.new_page(&name, &html)?;
        // ours, as the preview tool's page is: Del does not ask
        let _ = self.t.set_owner(w, true);
        let _ = self.t.set_live(w, true);
        self.pages.insert(w, key);
        Ok(true)
    }

    /// The agent's page again, when one is open.
    fn show_page(&mut self, session: &str) -> apex_tool::Result<()> {
        let Some((&w, _)) = self.pages.iter().find(|(_, s)| *s == session) else { return Ok(()) };
        let Some(a) = self.agents.get(session) else { return Ok(()) };
        let html = page::render(&self.t, Path::new(&a.cwd), a.exchange.as_ref().map(|x| (x.asked.as_str(), x.said.as_str())));
        self.t.replace(w, 0, END, &html)
    }

    // ---- the changes ----

    /// `Changes`: what the agent's repository says has changed since
    /// the session began -- the status, then the diff, each hunk's
    /// header followed by the `path:line` it lands at -- in a window at
    /// the root of the repository, `ROOT/-claude+ID+diff`, so that the
    /// paths in it are B3'd from where they are relative to. Again,
    /// and it is written afresh.
    fn changes_verb(&mut self, args: &str, at: Option<Range>) -> apex_tool::Result<bool> {
        let Some(key) = self.meant("Changes", args, at)? else { return Ok(!args.trim().is_empty()) };
        self.changes_for(&key)
    }

    /// The changes of the agent named, written afresh.
    fn changes_for(&mut self, key: &str) -> apex_tool::Result<bool> {
        let key = key.to_string();
        let Some(a) = self.agents.get(&key).cloned() else { return Ok(true) };
        let kind = if a.kind.is_empty() { "agent" } else { &a.kind };
        let cwd = Path::new(&a.cwd);
        let root = vcs::repo(cwd).map(|r| r.root).unwrap_or_else(|| cwd.to_path_buf());
        let since = match &a.rev {
            Some(r) => format!("since the session began ({})", r.chars().take(12).collect::<String>()),
            None => "since the last commit (where the session began was not recorded)".to_string(),
        };
        let body = match vcs::changes(cwd, a.rev.as_deref()) {
            Ok(text) => format!("– changes in {} {since}\n\n{text}", root.display()),
            Err(e) => format!("– {e}\n"),
        };
        let w = match self.diffs.iter().find(|(_, s)| **s == key).map(|(w, _)| *w) {
            Some(w) => w,
            None => {
                let name = format!("{}/-{kind}+{}+diff", root.display().to_string().trim_end_matches('/'), a.short());
                let w = self.t.new_window(&name)?;
                let _ = self.t.set_owner(w, true);
                let _ = self.t.set_live(w, true);
                let _ = self.t.set_tag(w, "Look Changes");
                let again = self.t.offer(Rule::verb("Changes").window(w))?;
                self.verbs.insert(again, "Changes");
                self.diffs.insert(w, key.clone());
                w
            }
        };
        self.t.replace(w, 0, END, &body)?;
        let _ = self.t.select(w, 0, 0);
        let _ = self.t.set_clean(w);
        Ok(true)
    }

    // ---- the history ----

    /// `History`: the sessions the pane's directory has had, newest
    /// first, in `DIR/-agents+history`; again, to close it. B3 on an id
    /// there (or anywhere) opens the transcript; `Resume ID` takes the
    /// session up.
    fn toggle_history(&mut self) -> apex_tool::Result<bool> {
        if let Some(w) = self.hist.take() {
            let _ = self.t.delete(w);
            return Ok(true);
        }
        let dir = self.opts.cwd.display().to_string();
        let past = history::sessions(&self.opts.claude_home, &self.opts.codex_home, &dir);
        let text = history::listing(&dir, &past, history::now());
        self.past = past.into_iter().map(|p| (p.id.clone(), p)).collect();
        let name = format!("{}/-agents+history", dir.trim_end_matches('/'));
        let w = self.t.new_window(&name)?;
        let _ = self.t.set_owner(w, true);
        let _ = self.t.set_tag(w, "Look Resume");
        let resume = self.t.offer(Rule::verb("Resume").window(w).unlisted())?;
        self.verbs.insert(resume, "Resume");
        self.t.replace(w, 0, END, &text)?;
        let _ = self.t.set_clean(w);
        self.hist = Some(w);
        Ok(true)
    }

    // ---- the transcripts ----

    /// A window for a transcript, live or past.
    #[allow(clippy::too_many_arguments)]
    fn detail_window(&mut self, session: &str, kind: &str, cwd: &str, short: &str, transcript: Option<&Path>, note: Option<String>, running: &[(String, String)], asking: Option<&str>) -> apex_tool::Result<()> {
        let kind = if kind.is_empty() { "agent" } else { kind };
        let name = format!("{}/-{kind}+{short}", cwd.trim_end_matches('/'));
        let w = self.t.new_window(&name)?;
        let _ = self.t.set_owner(w, true);
        let _ = self.t.set_live(w, true);
        // Enter does not send here (a prompt is as many lines as it
        // wants), so the verb that does is in the tag
        let _ = self.t.set_tag(w, "Look Send");
        let send = self.t.offer(Rule::verb("Send").window(w))?;
        self.t.watch(w)?;
        let mut wr = Writer::new();
        let mut parser = transcript::parser(kind, cwd, self.opts.thoughts);
        let mut tail = transcript.map(|p| Tail::new(p.to_path_buf()));
        if let Some(dir) = tail.as_ref().and_then(|t| t.path.parent()).map(Path::to_path_buf) {
            self.watch(&dir);
        }
        if let Some(n) = note {
            wr.item(&transcript::Item::Note(n));
        }
        match &mut tail {
            Some(t) => {
                for line in t.lines() {
                    for it in parser.line(&line) {
                        wr.item(&it);
                    }
                }
                if wr.text.is_empty() {
                    wr.item(&transcript::Item::Note(format!("nothing in {} yet", t.path.display())));
                }
            }
            None => {
                wr.item(&transcript::Item::Note(format!("{kind} did not say where it keeps this session's transcript")));
            }
        }
        // what the hooks have said of the calls going: the transcript
        // knows nothing of a call's running or being asked about
        for (id, _) in running {
            wr.glyph(id, "▶");
        }
        if let Some(id) = asking {
            wr.glyph(id, "?");
        }
        // the whole of it at once, and the dot at the end so that what
        // comes next carries it along
        self.t.replace(w, 0, END, &wr.text)?;
        let _ = self.t.select(w, wr.len, wr.len);
        let _ = self.t.set_clean(w);
        self.details.insert(w, Detail { session: session.to_string(), w, tail, parser, wr, pulsing: false, send });
        Ok(())
    }

    /// The agent's transcript in a window of its own, or the one it has
    /// brought forward.
    fn open_detail(&mut self, session: &str) -> apex_tool::Result<()> {
        if let Some(d) = self.details.values().find(|d| d.session == session) {
            let w = d.w;
            if let Some(name) = self.t.window_name(w) {
                let _ = self.t.open(&name, None);
            }
            return Ok(());
        }
        let Some(a) = self.agents.get(session).cloned() else { return Ok(()) };
        let asking = match (a.state, a.asking.as_deref()) {
            (State::Asking, Some(id)) => Some(id),
            _ => None,
        };
        self.detail_window(session, &a.kind, &a.cwd, &a.short(), transcript::transcript_path(a.transcript.as_deref()), None, &a.running, asking)
    }

    /// A past session's transcript: the same window, read from the
    /// agent's record, with a word at the top saying it is over.
    fn open_past(&mut self, p: Past) -> apex_tool::Result<()> {
        if let Some(d) = self.details.values().find(|d| d.session == p.id) {
            let w = d.w;
            if let Some(name) = self.t.window_name(w) {
                let _ = self.t.open(&name, None);
            }
            return Ok(());
        }
        let short: String = p.id.chars().take(8).collect();
        let when = history::when(p.when, history::now());
        self.detail_window(&p.id, &p.kind, &p.cwd, &short, Some(&p.path), Some(format!("a past session, last worked in {when}; Resume {short} takes it up")), &[], None)
    }

    /// What the transcripts say that they did not last time.
    fn tick_details(&mut self) -> apex_tool::Result<()> {
        let mut todo: Vec<(WindowId, Vec<Op>, usize)> = Vec::new();
        for d in self.details.values_mut() {
            let Some(t) = &mut d.tail else { continue };
            let lines = t.lines();
            if lines.is_empty() {
                continue;
            }
            let mut ops = Vec::new();
            for line in lines {
                for it in d.parser.line(&line) {
                    ops.extend(d.wr.item(&it));
                }
            }
            todo.push((d.w, ops, d.wr.len));
        }
        for (w, ops, len) in todo {
            for op in ops {
                apply(&mut self.t, w, op)?;
            }
            // clean, unless a prompt is being typed under it: that is
            // theirs, and not acted on, which is what dirty means
            if self.t.read(w)?.chars().count() <= len {
                let _ = self.t.set_clean(w);
            }
        }
        Ok(())
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.presence);
    }
}

/// An event that may have changed a file's contents or existence: a
/// write, a creation, a removal, a rename; not an open or a read,
/// which on Linux every read of ours would be.
fn is_change(kind: &notify::EventKind) -> bool {
    use notify::EventKind::*;
    match kind {
        Access(_) | Other => false,
        Any | Create(_) | Modify(_) | Remove(_) => true,
    }
}

/// One change to a transcript window. Text goes at the end of the
/// transcript with the dot following it, as a win's output does, and
/// before any draft typed after it; a glyph is written over in place
/// and moves nothing.
fn apply(t: &mut Tool, w: WindowId, op: Op) -> apex_tool::Result<()> {
    match op {
        Op::Append { at, text } => t.insert_following(w, at, &text),
        Op::Glyph { at, glyph } => t.replace(w, at, at + 1, glyph),
    }
}

/// The `apex` command: on PATH, else beside this program, else where a
/// remote install puts it.
pub fn apex_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("apex")).find(|p| p.is_file())) {
        return Some(p);
    }
    if let Some(p) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("apex"))).filter(|p| p.is_file()) {
        return Some(p);
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".apex/bin/apex")).filter(|p| p.is_file())
}

/// Text typed into the terminal `win` of `session`, Enter after it:
/// `apex term send`, which finds the session by its id and the terminal
/// by its window.
pub fn term_send(session: &str, win: u64, text: &str) -> Result<(), String> {
    let apex = apex_bin().ok_or("no apex command to type with: put apex on PATH")?;
    let out = std::process::Command::new(&apex).arg(format!("-session={session}")).args(["term", "send", &win.to_string(), &format!("{text}\n")]).output().map_err(|e| format!("{}: {e}", apex.display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("apex term send: {}", out.status) } else { err });
    }
    Ok(())
}
