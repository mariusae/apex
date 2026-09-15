//! What each agent is up to, as its hooks have said, and the pane that
//! shows them all: a block an agent, the left margin saying its state
//! so that a glance down the column finds the one that wants you.

use std::collections::BTreeMap;

use crate::event::Event;
use crate::transcript::brief;

/// An agent's state, in the order the pane lists them: what wants an
/// answer first, then what is finished and waiting on a prompt, then
/// what is working and needs nothing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum State {
    /// A permission, or a question: it cannot go on until you say.
    Asking,
    /// The turn ended badly (rate limit, server error).
    Failed,
    /// The turn is over and the next prompt is yours to give.
    Idle,
    Working,
    /// Nothing has been said of it yet but that it exists.
    Starting,
    /// `SessionEnd` came, or the process is gone.
    Ended,
}

/// A whole exchange: what was asked, and the agent's answer to it --
/// its last word on the turn. The page shows one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exchange {
    pub asked: String,
    pub said: String,
}

/// A subagent out on the agent's behalf: what kind, and the call it
/// is making.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sub {
    pub kind: String,
    pub running: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Agent {
    pub session: String,
    /// `claude`, `codex`.
    pub kind: String,
    pub cwd: String,
    pub transcript: Option<String>,
    pub pid: Option<u32>,
    /// The apex session and window it was started in, when it was.
    pub apex: Option<String>,
    pub win: Option<u64>,
    /// Where its repository stood when the session began.
    pub rev: Option<String>,
    pub started: i64,
    /// When it last said anything, in milliseconds.
    pub last: i64,
    pub state: State,
    /// What it was asked, the turn going or the last one.
    pub prompt: Option<String>,
    /// The tool calls going, oldest first: more than one at once when
    /// the agent runs them in parallel. The last is the line.
    pub running: Vec<(String, String)>,
    /// The call it is asking permission for, and its id.
    pub asked: Option<String>,
    pub asking: Option<String>,
    /// Its last word on the turn.
    pub said: Option<String>,
    /// The last exchange finished, which the page shows: it stands
    /// while the next turn is going, until that one is whole.
    pub exchange: Option<Exchange>,
    /// Why the turn failed.
    pub why: Option<String>,
    pub subagents: usize,
    /// The subagents out, by id, with what each is doing.
    pub subs: BTreeMap<String, Sub>,
    /// What the pane decided about the call it is asking for, once it
    /// has: `allow`, `deny`, or `ask` (the terminal's prompt has it).
    pub decided: Option<String>,
    pub mode: Option<String>,
}

impl Agent {
    pub fn new(session: &str) -> Agent {
        Agent { session: session.to_string(), kind: String::new(), cwd: String::new(), transcript: None, pid: None, apex: None, win: None, rev: None, started: 0, last: 0, state: State::Starting, prompt: None, running: Vec::new(), asked: None, asking: None, said: None, exchange: None, why: None, subagents: 0, subs: BTreeMap::new(), decided: None, mode: None }
    }

    /// Enough of the id to tell it apart, and to B3.
    pub fn short(&self) -> String {
        self.session.chars().take(8).collect()
    }

    pub fn apply(&mut self, e: &Event) {
        self.last = e.ms;
        if self.started == 0 {
            self.started = e.ms;
        }
        if self.kind.is_empty() && !e.agent.is_empty() {
            self.kind = e.agent.clone();
        }
        if !e.cwd.is_empty() {
            self.cwd = e.cwd.clone();
        }
        if e.transcript.is_some() {
            self.transcript = e.transcript.clone();
        }
        if e.pid.is_some() {
            self.pid = e.pid;
        }
        if e.apex.is_some() {
            self.apex = e.apex.clone();
            self.win = e.win;
        }
        if e.mode.is_some() {
            self.mode = e.mode.clone();
        }
        if e.rev.is_some() {
            self.rev = e.rev.clone();
        }
        match e.event.as_str() {
            "SubagentStart" => {
                self.subagents += 1;
                if let Some(id) = &e.sub {
                    self.subs.insert(id.clone(), Sub { kind: e.kind.clone().unwrap_or_default(), running: Vec::new() });
                }
                return;
            }
            "SubagentStop" => {
                self.subagents = self.subagents.saturating_sub(1);
                if let Some(id) = &e.sub {
                    self.subs.remove(id);
                }
                return;
            }
            _ => {}
        }
        // a subagent's calls are its own, shown under it
        if let Some(id) = &e.sub {
            let sub = self.subs.entry(id.clone()).or_insert_with(|| Sub { kind: e.kind.clone().unwrap_or_default(), running: Vec::new() });
            match e.event.as_str() {
                "PreToolUse" => {
                    if let (Some(c), Some(t)) = (&e.call, &e.title) {
                        sub.running.retain(|(i, _)| i != c);
                        sub.running.push((c.clone(), t.clone()));
                    }
                }
                "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
                    if let Some(c) = &e.call {
                        sub.running.retain(|(i, _)| i != c);
                    }
                }
                _ => {}
            }
            return;
        }
        match e.event.as_str() {
            "SessionStart" => {
                // a compaction restarts the session mid-turn
                if self.state != State::Working {
                    self.state = State::Idle;
                }
            }
            "UserPromptSubmit" => {
                self.state = State::Working;
                self.prompt = e.text.clone();
                self.running.clear();
                self.asked = None;
                self.asking = None;
                self.said = None;
                self.why = None;
            }
            "PreToolUse" => {
                self.state = State::Working;
                self.asked = None;
                self.asking = None;
                if let (Some(id), Some(t)) = (&e.call, &e.title) {
                    self.running.retain(|(i, _)| i != id);
                    self.running.push((id.clone(), t.clone()));
                }
            }
            "PermissionRequest" => {
                self.state = State::Asking;
                self.asked = e.title.clone();
                self.asking = e.call.clone();
                self.decided = None;
            }
            // the pane's answer, written into the log as the record of it
            "Decision" if e.call.is_some() && e.call == self.asking => {
                self.decided = e.kind.clone();
                match e.kind.as_deref() {
                    Some("allow") => self.state = State::Working,
                    Some("deny") => {
                        self.state = State::Working;
                        if let Some(id) = &e.call {
                            self.running.retain(|(i, _)| i != id);
                        }
                    }
                    _ => {}
                }
            }
            "PermissionDenied" | "PostToolUse" | "PostToolUseFailure" => {
                self.state = State::Working;
                self.asked = None;
                self.asking = None;
                if let Some(id) = &e.call {
                    self.running.retain(|(i, _)| i != id);
                }
            }
            "PostToolBatch" => self.running.clear(),
            "Notification" => match e.kind.as_deref() {
                Some("permission_prompt") => {
                    self.state = State::Asking;
                    if self.asked.is_none() {
                        self.asked = e.text.clone();
                    }
                }
                Some("elicitation_dialog") | Some("elicitation_url_dialog") => {
                    self.state = State::Asking;
                    self.asked = e.text.clone();
                }
                Some("idle_prompt") | Some("agent_needs_input") | Some("agent_completed") => {
                    self.state = State::Idle;
                    self.running.clear();
                }
                _ => {}
            },
            "Stop" => {
                self.state = State::Idle;
                self.running.clear();
                self.asked = None;
                self.asking = None;
                self.said = e.text.clone();
                if let Some(said) = e.text.as_deref().filter(|t| !t.trim().is_empty()) {
                    self.exchange = Some(Exchange { asked: self.prompt.clone().unwrap_or_default(), said: said.to_string() });
                }
            }
            "Interrupt" => {
                self.state = State::Idle;
                self.running.clear();
                self.asked = None;
                self.asking = None;
            }
            "StopFailure" => {
                self.state = State::Failed;
                self.running.clear();
                self.why = e.kind.clone().or_else(|| e.text.clone());
            }
            "SessionEnd" => self.state = State::Ended,
            "PreCompact" => {
                self.running.retain(|(i, _)| i != "compact");
                self.running.push(("compact".to_string(), "compacting the context".to_string()));
            }
            "PostCompact" => self.running.retain(|(i, _)| i != "compact"),
            _ => {}
        }
    }

    /// The mark in the margin.
    pub fn glyph(&self) -> &'static str {
        match self.state {
            State::Asking => "?",
            State::Failed => "✗",
            State::Idle => "~",
            State::Working => "▶",
            State::Starting => "⋯",
            State::Ended => "–",
        }
    }

    /// The line under the prompt: what it is doing, what it wants, or
    /// what it last said, as its state has it. None when there is
    /// nothing to say. A question not yet answered offers the words
    /// that answer it, to be B2'd where they stand; one handed to the
    /// terminal says so.
    pub fn doing(&self) -> Option<String> {
        match self.state {
            State::Asking => self.asked.as_deref().map(|a| match self.decided.as_deref() {
                None => format!("? {}  Allow Deny Ask", brief(a)),
                Some("ask") => format!("? {}  asked at the terminal", brief(a)),
                Some(d) => format!("? {}  {d}", brief(a)),
            }),
            State::Working | State::Starting => self.running.last().map(|(_, t)| format!("▶ {}", brief(t))),
            State::Idle => self.said.as_deref().map(|s| format!("• {}", brief(s))),
            State::Failed => Some(format!("✗ {}", self.why.as_deref().unwrap_or("the turn failed"))),
            State::Ended => Some("– ended".to_string()),
        }
    }
}

/// Every agent heard from, by session.
#[derive(Default)]
pub struct Agents {
    pub map: BTreeMap<String, Agent>,
}

impl Agents {
    pub fn apply(&mut self, e: &Event) {
        self.map.entry(e.session.clone()).or_insert_with(|| Agent::new(&e.session)).apply(e);
    }

    pub fn remove(&mut self, session: &str) -> Option<Agent> {
        self.map.remove(session)
    }

    pub fn get(&self, session: &str) -> Option<&Agent> {
        self.map.get(session)
    }

    /// Those that are still there, in the order the pane shows them:
    /// by state, and the longest-running first within it, so that a
    /// block keeps its place as the work goes on.
    pub fn ordered(&self) -> Vec<&Agent> {
        let mut v: Vec<&Agent> = self.map.values().filter(|a| a.state != State::Ended).collect();
        v.sort_by(|a, b| a.state.cmp(&b.state).then(a.started.cmp(&b.started)).then(a.session.cmp(&b.session)));
        v
    }

    /// Whether any of them is working: what the pane's handle says.
    pub fn working(&self) -> bool {
        self.map.values().any(|a| a.state == State::Working)
    }

    /// The agent an id names: the whole of it, or a prefix that names
    /// one and no other.
    pub fn by_id(&self, id: &str) -> Option<&Agent> {
        let hits: Vec<&Agent> = self.map.values().filter(|a| a.session.starts_with(id)).collect();
        match hits.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }
}

/// Whether `cwd` is `dir` or under it.
pub fn under(cwd: &str, dir: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    dir.is_empty() || cwd == dir || cwd.strip_prefix(dir).is_some_and(|r| r.starts_with('/'))
}

/// How long since `then`, coarsely: nothing under a minute, since that
/// is live, and then minutes, hours, days.
pub fn ago(then: i64, now: i64) -> String {
    let s = (now - then).max(0) / 1000;
    match s {
        s if s < 60 => String::new(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

/// A directory as shown: the home directory as `~`.
pub fn shown_dir(dir: &str, home: Option<&str>) -> String {
    match home.filter(|h| !h.is_empty() && *h != "/") {
        Some(h) if dir == h => "~".to_string(),
        Some(h) => match dir.strip_prefix(h) {
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => dir.to_string(),
        },
        None => dir.to_string(),
    }
}

/// The pane: its first line, and then a block an agent, keyed by
/// session so that a block that changed can be written in place.
///
/// ```text
/// – 2 agents
///
/// ? claude  ~/src/apex  0b1c1425  2m
///   build an experimental tool, apex-agent
///   ? Bash: Remove the build directory
///
/// ▶ codex  ~/src/cmd  9e21ab77
///   port the rc shell
///   ▶ shell: cargo test
/// ```
///
/// The first line is apex's own (`–`); each block's margin is the
/// agent's state, its first line the agent, where, its id, and how long
/// it has been quiet; its second what it was asked; its third what it is
/// doing about it, wanting, or last said; and under that a line a
/// subagent, with what each is doing. The pane's name is its filter:
/// with `under` given, only agents in that directory are blocks, and
/// the last line counts the rest; with `session` given (`-s`), only
/// agents started in that apex session are. With no agents the first
/// line is a guide: `Start claude` and `Start codex` are verbs, to be
/// B2'd there.
pub fn pane(agents: &[&Agent], now: i64, home: Option<&str>, under_dir: Option<&str>, session: Option<&str>) -> (String, Vec<(String, String)>, String) {
    let (here, elsewhere): (Vec<&Agent>, Vec<&Agent>) = agents.iter().partition(|a| match session {
        Some(s) => a.apex.as_deref() == Some(s),
        None => under_dir.is_none_or(|d| under(&a.cwd, d)),
    });
    let header = match (here.len(), elsewhere.len()) {
        (0, 0) => "– no agents yet: Start claude, Start codex, or `apex-agent install` first\n".to_string(),
        (0, _) if session.is_some() => "– no agents in this session\n".to_string(),
        (0, _) => "– no agents here\n".to_string(),
        (1, _) => "– 1 agent\n".to_string(),
        (n, _) => format!("– {n} agents\n"),
    };
    let footer = match (elsewhere.len(), session.is_some()) {
        (0, _) => String::new(),
        (1, true) => "– 1 in another session: -all shows all\n".to_string(),
        (n, true) => format!("– {n} in other sessions: -all shows all\n"),
        (1, false) => "– 1 elsewhere: a pane in ~ shows all\n".to_string(),
        (n, false) => format!("– {n} elsewhere: a pane in ~ shows all\n"),
    };
    let mut blocks = Vec::new();
    for a in here {
        let mut b = format!("{} {}  {}  {}", a.glyph(), if a.kind.is_empty() { "agent" } else { &a.kind }, shown_dir(&a.cwd, home), a.short());
        let since = ago(a.last, now);
        if !since.is_empty() {
            b.push_str(&format!("  {since}"));
        }
        match a.subagents {
            0 => {}
            1 => b.push_str("  1 subagent"),
            n => b.push_str(&format!("  {n} subagents")),
        }
        b.push('\n');
        if let Some(p) = &a.prompt {
            let p = brief(p);
            if !p.is_empty() {
                b.push_str(&format!("  {p}\n"));
            }
        }
        if let Some(d) = a.doing() {
            b.push_str(&format!("  {d}\n"));
        }
        for s in a.subs.values() {
            let kind = if s.kind.is_empty() { "subagent" } else { &s.kind };
            match s.running.last() {
                Some((_, t)) => b.push_str(&format!("    ▶ {kind}: {}\n", brief(t))),
                None => b.push_str(&format!("    ⋯ {kind}\n")),
            }
        }
        blocks.push((a.session.clone(), b));
    }
    (header, blocks, footer)
}

/// The whole text of the pane, and where each block starts in it (in
/// characters): the header, a blank line, then the blocks set off from
/// one another by a blank line, and the footer, when there is one, set
/// off the same way.
pub fn pane_text(header: &str, blocks: &[(String, String)], footer: &str) -> (String, Vec<usize>) {
    let mut text = header.to_string();
    text.push('\n');
    let mut starts = Vec::new();
    for (i, (_, b)) in blocks.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        starts.push(text.chars().count());
        text.push_str(b);
    }
    if !footer.is_empty() {
        if !blocks.is_empty() {
            text.push('\n');
        }
        text.push_str(footer);
    }
    (text, starts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(session: &str, event: &str, ms: i64) -> Event {
        Event { ms, agent: "claude".into(), event: event.into(), session: session.into(), cwd: "/home/me/src/apex".into(), ..Event::default() }
    }

    #[test]
    fn an_agent_goes_through_its_states_and_the_pane_says_so() {
        let mut ag = Agents::default();
        ag.apply(&Event { kind: Some("startup".into()), ..ev("0b1c1425-aaaa", "SessionStart", 1000) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().state, State::Idle);
        ag.apply(&Event { text: Some("build an experimental tool, apex-agent\n\nit should use hooks".into()), ..ev("0b1c1425-aaaa", "UserPromptSubmit", 2000) });
        ag.apply(&Event { call: Some("t1".into()), title: Some("Bash: Build it".into()), ..ev("0b1c1425-aaaa", "PreToolUse", 3000) });
        ag.apply(&Event { call: Some("t2".into()), title: Some("Read: src/main.rs".into()), ..ev("0b1c1425-aaaa", "PreToolUse", 3100) });
        let (h, b, _) = pane(&ag.ordered(), 4000, Some("/home/me"), None, None);
        assert_eq!(h, "– 1 agent\n");
        // a prompt of many lines is its first, and says there is more
        assert_eq!(b[0].1, "▶ claude  ~/src/apex  0b1c1425\n  build an experimental tool, apex-agent…\n  ▶ Read: src/main.rs\n");
        // the first call ends: the other is still the line
        ag.apply(&Event { call: Some("t2".into()), ..ev("0b1c1425-aaaa", "PostToolUse", 3200) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().doing().as_deref(), Some("▶ Bash: Build it"));
        ag.apply(&Event { call: Some("t1".into()), ..ev("0b1c1425-aaaa", "PostToolUse", 3300) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().doing(), None);
        // a permission wanted goes to the head of the list, over one working
        ag.apply(&Event { text: Some("port the rc shell".into()), ..ev("9e21ab77-bbbb", "UserPromptSubmit", 5000) });
        ag.apply(&Event { call: Some("t3".into()), title: Some("Bash: Remove the build directory".into()), ..ev("0b1c1425-aaaa", "PermissionRequest", 6000) });
        let (h, b, _) = pane(&ag.ordered(), 6000 + 150_000, Some("/home/me"), None, None);
        assert_eq!(h, "– 2 agents\n");
        assert_eq!(b[0].0, "0b1c1425-aaaa");
        assert_eq!(b[0].1, "? claude  ~/src/apex  0b1c1425  2m\n  build an experimental tool, apex-agent…\n  ? Bash: Remove the build directory  Allow Deny Ask\n");
        // the pane's answer is an event, and the line says it
        ag.apply(&Event { call: Some("t3".into()), kind: Some("ask".into()), ..ev("0b1c1425-aaaa", "Decision", 6500) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().doing().as_deref(), Some("? Bash: Remove the build directory  asked at the terminal"));
        ag.apply(&Event { call: Some("t3".into()), kind: Some("allow".into()), ..ev("0b1c1425-aaaa", "Decision", 6600) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().state, State::Working);
        assert_eq!(b[1].1, "▶ claude  ~/src/apex  9e21ab77  2m\n  port the rc shell\n");
        // allowed and run: working again
        ag.apply(&Event { call: Some("t3".into()), ..ev("0b1c1425-aaaa", "PostToolUse", 7000) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().state, State::Working);
        // the turn ends: its last word is the line, and it sits after the one still working
        ag.apply(&Event { text: Some("Done: the tool is built.\n\nMore below.".into()), ..ev("0b1c1425-aaaa", "Stop", 8000) });
        let (_, b, _) = pane(&ag.ordered(), 8000, Some("/home/me"), None, None);
        assert_eq!(b[0].1, "~ claude  ~/src/apex  0b1c1425\n  build an experimental tool, apex-agent…\n  • Done: the tool is built.…\n");
        // the exchange is kept whole for the page, and stands through the next turn
        let x = ag.get("0b1c1425-aaaa").unwrap().exchange.clone().unwrap();
        assert_eq!((x.asked.as_str(), x.said.as_str()), ("build an experimental tool, apex-agent\n\nit should use hooks", "Done: the tool is built.\n\nMore below."));
        ag.apply(&Event { text: Some("and now?".into()), ..ev("0b1c1425-aaaa", "UserPromptSubmit", 8500) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().exchange.as_ref().map(|x| x.said.as_str()), Some("Done: the tool is built.\n\nMore below."));
        assert_eq!(b[1].0, "9e21ab77-bbbb");
        // subagents are counted, and each is a line of its own under the block
        ag.apply(&Event { sub: Some("a1".into()), kind: Some("Explore".into()), ..ev("9e21ab77-bbbb", "SubagentStart", 9000) });
        ag.apply(&Event { sub: Some("a1".into()), call: Some("t9".into()), title: Some("Grep: foo".into()), ..ev("9e21ab77-bbbb", "PreToolUse", 9100) });
        let a = ag.get("9e21ab77-bbbb").unwrap();
        assert_eq!((a.subagents, a.doing()), (1, None));
        let (_, b, _) = pane(&ag.ordered(), 9100, Some("/home/me"), None, None);
        assert_eq!(b[1].1, "▶ claude  ~/src/apex  9e21ab77  1 subagent\n  port the rc shell\n    ▶ Explore: Grep: foo\n");
        ag.apply(&Event { sub: Some("a1".into()), ..ev("9e21ab77-bbbb", "SubagentStop", 9200) });
        assert!(ag.get("9e21ab77-bbbb").unwrap().subs.is_empty());
        // the pane's name is its filter
        let (h, b, f) = pane(&ag.ordered(), 9200, Some("/home/me"), Some("/home/me/src/apex"), None);
        assert_eq!((h.as_str(), b.len(), f.as_str()), ("– 2 agents\n", 2, ""));
        let (h, b, f) = pane(&ag.ordered(), 9200, Some("/home/me"), Some("/home/me/src/cmd"), None);
        assert_eq!((h.as_str(), b.len(), f.as_str()), ("– no agents here\n", 0, "– 2 elsewhere: a pane in ~ shows all\n"));
        // -s: the agents started in this apex session, wherever they are
        ag.apply(&Event { apex: Some("sess-1".into()), win: Some(7), ..ev("0b1c1425-aaaa", "PreToolUse", 9300) });
        let (h, b, f) = pane(&ag.ordered(), 9300, Some("/home/me"), Some("/home/me/src/cmd"), Some("sess-1"));
        assert_eq!((h.as_str(), b.len(), f.as_str()), ("– 1 agent\n", 1, "– 1 in another session: -all shows all\n"));
        let (h, b, f) = pane(&ag.ordered(), 9300, Some("/home/me"), None, Some("sess-2"));
        assert_eq!((h.as_str(), b.len(), f.as_str()), ("– no agents in this session\n", 0, "– 2 in other sessions: -all shows all\n"));
        assert_eq!(ag.by_id("0b1c").map(|a| a.session.as_str()), Some("0b1c1425-aaaa"));
        assert!(ag.by_id("zz").is_none());
        // gone
        ag.apply(&ev("9e21ab77-bbbb", "SessionEnd", 9500));
        assert_eq!(ag.ordered().len(), 1);
        ag.apply(&Event { kind: Some("rate_limit".into()), ..ev("0b1c1425-aaaa", "StopFailure", 9600) });
        assert_eq!(ag.get("0b1c1425-aaaa").unwrap().doing().as_deref(), Some("✗ rate_limit"));
    }

    #[test]
    fn the_pane_text_knows_where_its_blocks_start() {
        let blocks = vec![("a".to_string(), "▶ a\n  one\n".to_string()), ("b".to_string(), "~ b\n".to_string())];
        let (text, starts) = pane_text("– 2 agents\n", &blocks, "");
        assert_eq!(text, "– 2 agents\n\n▶ a\n  one\n\n~ b\n");
        assert_eq!(starts, vec![12, 23]);
        assert_eq!(text.chars().nth(12), Some('▶'));
        assert_eq!(text.chars().nth(23), Some('~'));
        let (text, _) = pane_text("– 1 agent\n", &blocks[..1], "– 1 elsewhere\n");
        assert_eq!(text, "– 1 agent\n\n▶ a\n  one\n\n– 1 elsewhere\n");
        assert!(under("/a/b", "/a") && under("/a", "/a") && !under("/ab", "/a") && under("/x", ""));
    }

    #[test]
    fn time_since_is_coarse() {
        assert_eq!(ago(0, 59_000), "");
        assert_eq!(ago(0, 60_000), "1m");
        assert_eq!(ago(0, 7_200_000), "2h");
        assert_eq!(ago(0, 3 * 86_400_000), "3d");
        assert_eq!(shown_dir("/home/me/src", Some("/home/me")), "~/src");
        assert_eq!(shown_dir("/home/me", Some("/home/me")), "~");
        assert_eq!(shown_dir("/home/meh/src", Some("/home/me")), "/home/meh/src");
    }
}
