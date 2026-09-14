//! The windows: the pane, `DIR/-agents`, with a block an agent, and a
//! transcript window for each one opened, `AGENTDIR/-claude+ID`, named
//! for the agent's own directory so that a `path:line` in it is B3'd
//! from where the agent worked. Both are the tool's own (`set_owner`):
//! what is in them is the agents' doing and not a file's contents.
//!
//! Nothing here waits on anything: the logs' directory and each open
//! transcript's are watched (directories, not files, as the server's
//! own watcher does: what is appended to is one thing, what is renamed
//! into place another), a change wakes the loop, and what is new is
//! read and written; the window's events are taken in between. A slow
//! pass every few seconds asks after the agents' processes and catches
//! anything a watch let by.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use apex_tool::{Event, Range, Rule, RuleId, Tool, WindowId, END};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::agents::{self, Agents, State};
use crate::event::{self, Tail};
use crate::transcript::{self, Op, Parser, Writer};

pub struct Opts {
    pub cwd: PathBuf,
    /// Where the logs are.
    pub dir: PathBuf,
    pub thoughts: bool,
}

/// How often the slow pass runs: the agents' processes asked after,
/// and the logs and transcripts looked at whether or not a watch said
/// to, so that nothing is missed for longer than this.
const SLOW: Duration = Duration::from_secs(5);
/// How often the pane is written again with nothing new heard: the
/// minutes an agent has been quiet.
const MINUTES: Duration = Duration::from_secs(15);

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
}

pub struct Pane {
    t: Tool,
    w: WindowId,
    opts: Opts,
    agents: Agents,
    /// Each session's log, read as it grows.
    logs: HashMap<String, Tail>,
    /// What the window says: the header, the blocks and where they start.
    header: String,
    blocks: Vec<(String, String)>,
    starts: Vec<usize>,
    details: HashMap<WindowId, Detail>,
    open: RuleId,
    goto: RuleId,
    look: RuleId,
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
}

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
        let _ = t.set_tag(w, "Look Open Goto");
        // Open and Goto with dot in a block, or with the agent named
        // after them; B3 anywhere in a block
        let open = t.offer(Rule::verb("Open").window(w))?;
        let goto = t.offer(Rule::verb("Goto").window(w))?;
        let look = t.offer(Rule::plumb().window(w).priority(10))?;
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
        let mut pane = Pane { t, w, opts, agents: Agents::default(), logs: HashMap::new(), header: String::new(), blocks: Vec::new(), starts: Vec::new(), details: HashMap::new(), open, goto, look, pulsing: false, dirty: false, last_slow: Instant::now(), last_render: Instant::now(), home, watcher, woken, watched: BTreeMap::new() };
        // the directory must be there to be watched: a hook makes it
        // otherwise, and the watch would miss the making
        let _ = std::fs::create_dir_all(&pane.opts.dir);
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
                }
                // the transcripts first, so that a hook speaking of a
                // call finds its line already there
                self.tick_details()?;
                self.look()?;
                self.sweep();
                self.render()?;
            } else if self.last_render.elapsed() >= MINUTES {
                self.render()?;
            }
            match self.t.next_event(Some(Duration::from_millis(20)))? {
                None if !self.t.windows().iter().any(|x| x.id == self.w) => return Ok(()),
                None => {}
                Some(Event::Deleted { window }) if window == self.w => return Ok(()),
                Some(Event::Deleted { window }) => {
                    if let Some(d) = self.details.remove(&window) {
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
                    let taken = if p.rule == self.open {
                        self.open_verb(&p.text, p.at)?
                    } else if p.rule == self.goto {
                        self.goto_verb(&p.text, p.at)?
                    } else if p.rule == self.look {
                        self.open_at(p.sel.or(p.at))?
                    } else {
                        false
                    };
                    self.t.answer(&p, taken)?;
                }
                Some(_) => {}
            }
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
                self.agents.apply(&ev);
                self.heard(&ev)?;
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
    /// open: running, wanted, refused.
    fn heard(&mut self, ev: &event::Event) -> apex_tool::Result<()> {
        let Some(d) = self.details.values_mut().find(|d| d.session == ev.session) else { return Ok(()) };
        let glyph = match ev.event.as_str() {
            "PreToolUse" => Some("▶"),
            "PermissionRequest" => Some("?"),
            "PermissionDenied" | "PostToolUseFailure" => Some("✗"),
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
                // SAFETY: signal 0 delivers nothing; it asks whether the process is there
                if unsafe { libc::kill(pid as i32, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    a.state = State::Ended;
                }
            }
        }
    }

    // ---- the pane ----

    /// The pane as it should read now, written where it differs: a block
    /// that changed is written in place, and the whole only when the
    /// blocks are not the ones they were, in the order they were.
    fn render(&mut self) -> apex_tool::Result<()> {
        let now = event::now_ms();
        let (header, blocks) = agents::pane(&self.agents.ordered(), now, self.home.as_deref());
        let same_keys = header == self.header && blocks.len() == self.blocks.len() && blocks.iter().zip(&self.blocks).all(|(a, b)| a.0 == b.0);
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
            let (text, _) = agents::pane_text(&header, &blocks);
            self.t.replace(self.w, 0, END, &text)?;
            self.dirty = true;
        }
        let (_, starts) = agents::pane_text(&header, &blocks);
        self.header = header;
        self.blocks = blocks;
        self.starts = starts;
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

    // ---- the transcripts ----

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
        let kind = if a.kind.is_empty() { "agent" } else { &a.kind };
        let name = format!("{}/-{kind}+{}", a.cwd.trim_end_matches('/'), a.short());
        let w = self.t.new_window(&name)?;
        let _ = self.t.set_owner(w, true);
        let _ = self.t.set_live(w, true);
        let _ = self.t.set_tag(w, "Look");
        self.t.watch(w)?;
        let mut wr = Writer::new();
        let mut parser = transcript::parser(&a.kind, &a.cwd, self.opts.thoughts);
        let tail = match transcript::transcript_path(a.transcript.as_deref()) {
            Some(p) => Some(Tail::new(p.to_path_buf())),
            None => None,
        };
        let mut tail = tail;
        if let Some(dir) = tail.as_ref().and_then(|t| t.path.parent()).map(Path::to_path_buf) {
            self.watch(&dir);
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
        for (id, _) in &a.running {
            wr.glyph(id, "▶");
        }
        if let (State::Asking, Some(id)) = (a.state, &a.asking) {
            wr.glyph(id, "?");
        }
        // the whole of it at once, and the dot at the end so that what
        // comes next carries it along
        self.t.replace(w, 0, END, &wr.text)?;
        let _ = self.t.select(w, wr.len, wr.len);
        let _ = self.t.set_clean(w);
        self.details.insert(w, Detail { session: session.to_string(), w, tail, parser, wr, pulsing: false });
        Ok(())
    }

    /// What the transcripts say that they did not last time.
    fn tick_details(&mut self) -> apex_tool::Result<()> {
        let mut todo: Vec<(WindowId, Vec<Op>)> = Vec::new();
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
            todo.push((d.w, ops));
        }
        for (w, ops) in todo {
            for op in ops {
                apply(&mut self.t, w, op)?;
            }
            let _ = self.t.set_clean(w);
        }
        Ok(())
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

/// One change to a transcript window. Text goes at the end with the dot
/// following it, as a win's output does; a glyph is written over in
/// place and moves nothing.
fn apply(t: &mut Tool, w: WindowId, op: Op) -> apex_tool::Result<()> {
    match op {
        Op::Append(text) => {
            let len = t.read(w)?.chars().count();
            t.insert_following(w, len, &text)
        }
        Op::Glyph { at, glyph } => t.replace(w, at, at + 1, glyph),
    }
}
