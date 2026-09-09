//! `apex tool bridge NAME`: the session as a tool in any language sees
//! it. The bridge attaches as the tool named NAME and speaks JSON, one
//! object per line, on its standard input and output; a program that
//! starts it gets windows, rules and plumbs without the wire protocol
//! or the replicated state behind them.
//!
//! **Commands** come in on stdin as `{"id": N, "cmd": "...", ...}` and
//! are answered, in order, with `{"id": N, "ok": true, ...}` or
//! `{"id": N, "ok": false, "error": "..."}`:
//!
//! - `windows` → `windows: [{id, name, kind, live}]`
//! - `new {name}` → `window` (a new, empty window of that name)
//! - `open {name, line?}` → `window` (a file, opened or shown, at a line)
//! - `read {window}` → `text`; `selection {window}` → `q0, q1`
//! - `write {window, q0, q1, text}` (a range replaced; `q0`/`q1` of -1
//!   mean the end, so `q0: -1, q1: -1` appends), `select {window, q0, q1}`
//! - `rename {window, name}`, `live {window, on}`, `delete {window}`
//! - `exec {window?, text}` (B2 there), `errors {dir?, text}` (+Errors)
//! - `rule {verb?, text?, file?, kind?, window?, priority?}` → `rule`: a
//!   rule answered by this tool (`plumb` events); `unrule {rule}`
//! - `ack {plumb, ok}`: the answer to a `plumb` event (within a second)
//! - `watch {window}` / `unwatch {window}`: `edit` events for its body
//! - `set {key, value}` / `setting {key}` → `value`
//!
//! **Events** go out as `{"event": "...", ...}`: `hello {attachment,
//! session}` first; `plumb {plumb, rule, verb, text, dir, window?,
//! groups, at?, sel?}` when a rule of ours matched (`rule` says which) (`at` and `sel` are `{q0,
//! q1}` in the window's body); `edit {window, q0, nd, text}` for a
//! watched window, edits by others only; `renamed {window, name}` and
//! `deleted {window}` for windows we made, opened or watched; `bye`
//! when the session or the link ends, after which the bridge exits.
//! Offsets count characters, as apex does throughout.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

use apex_core::*;
use apex_server::proto::{ClientMsg, ServerMsg};
use apex_server::remote::{Remote, ToolPlumb};
use apex_server::Proposal;

const TIMEOUT: Duration = Duration::from_secs(10);

struct Bridge {
    remote: Remote,
    /// Windows whose body edits (by others) are reported.
    watched: BTreeSet<WindowId>,
    /// Windows we made, opened or watch, with their names as last seen:
    /// renames and deletions are reported for these.
    ours: BTreeMap<WindowId, String>,
    out: std::io::Stdout,
}

/// Run the bridge for the session at `socket`, as the tool `name`,
/// until stdin closes or the link ends.
pub fn run(socket: &Path, session: &str, name: &str) -> Result<(), String> {
    let remote = Remote::connect_as(socket, session, name, AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
    remote.announce(name);
    let mut b = Bridge { remote, watched: BTreeSet::new(), ours: BTreeMap::new(), out: std::io::stdout() };
    b.emit(json!({ "event": "hello", "attachment": b.remote.attachment().0, "session": session, "tool": name }));
    // stdin on a thread: a line at a time, ended by EOF
    let (tx, rx) = channel::<Option<String>>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(Some(l)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(None);
    });
    let r = b.main_loop(&rx);
    b.emit(json!({ "event": "bye" }));
    r
}

impl Bridge {
    fn emit(&mut self, v: Value) {
        let mut o = self.out.lock();
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    }

    /// One message from the link, seen for edits first; false when the
    /// link ended.
    fn step(&mut self, timeout: Duration) -> bool {
        match self.remote.link.rx.recv_timeout(timeout) {
            Ok(m) => {
                self.before(&m);
                let alive = self.remote.handle(m);
                self.after();
                alive
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => true,
            Err(_) => false,
        }
    }

    /// Propose and wait for the answer, every message on the way seen
    /// here (Remote::propose would apply them behind our back).
    fn propose(&mut self, p: Proposal) -> Result<Option<WindowId>, String> {
        let id = self.remote.link.propose(p);
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            if let Some(r) = self.remote.link.applied.remove(&id) {
                return r;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("timed out waiting for the leader".into());
            }
            if !self.step(left.min(Duration::from_millis(50))) {
                return Err("connection closed".into());
            }
        }
    }

    /// Edits by others to a watched window's body, before they land in
    /// the replica (so `q0` is against the text as the tool last saw it).
    fn before(&mut self, m: &ServerMsg) {
        let ServerMsg::Entries { shard, entries } = m else { return };
        let Shard::Buffer(b) = shard else { return };
        let me = self.remote.attachment();
        let Some(w) = self.watched.iter().copied().find(|w| self.remote.node.state.window(*w).ok().and_then(|x| x.body_buffer()) == Some(*b)) else { return };
        let mut events = Vec::new();
        for e in entries {
            if e.attachment == me {
                continue;
            }
            if let Op::Buffer(BufferOp::Edit { q0, nd, text, .. }) = &e.op {
                events.push(json!({ "event": "edit", "window": w.0, "q0": q0, "nd": nd, "text": text }));
            }
        }
        for ev in events {
            self.emit(ev);
        }
    }

    /// After messages: our windows renamed or gone.
    fn after(&mut self) {
        let mut events = Vec::new();
        let mut gone = Vec::new();
        for (w, name) in self.ours.iter_mut() {
            match self.remote.node.state.window(*w) {
                Ok(_) => {
                    let now = self.remote.node.window_name(*w);
                    if now != *name {
                        *name = now.clone();
                        events.push(json!({ "event": "renamed", "window": w.0, "name": now }));
                    }
                }
                Err(_) => gone.push(*w),
            }
        }
        for w in gone {
            self.ours.remove(&w);
            self.watched.remove(&w);
            events.push(json!({ "event": "deleted", "window": w.0 }));
        }
        for ev in events {
            self.emit(ev);
        }
    }

    fn main_loop(&mut self, rx: &Receiver<Option<String>>) -> Result<(), String> {
        loop {
            // the link, without blocking
            loop {
                match self.remote.link.rx.try_recv() {
                    Ok(m) => {
                        self.before(&m);
                        if !self.remote.handle(m) {
                            return Ok(());
                        }
                        self.after();
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => return Ok(()),
                }
            }
            let plumbs: Vec<ToolPlumb> = std::mem::take(&mut self.remote.link.plumbs);
            for p in plumbs {
                self.on_plumb(p);
            }
            // a command, or a moment
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let reply = match serde_json::from_str::<Value>(&line) {
                        Ok(v) => self.command(&v),
                        Err(e) => json!({ "ok": false, "error": format!("not JSON: {e}") }),
                    };
                    self.emit(reply);
                }
                Ok(None) => return Ok(()), // stdin closed: the tool is done
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Ok(()),
            }
        }
    }

    /// A rule of ours matched: the tool decides, and acks.
    fn on_plumb(&mut self, p: ToolPlumb) {
        let window = match p.ctx {
            ExecCtx::Window(w) => Some(w.0),
            _ => None,
        };
        let span = |s: Option<Span>| s.map(|s| json!({ "q0": s.q0, "q1": s.q1 }));
        self.emit(json!({
            "event": "plumb", "plumb": p.id, "rule": p.rule.0, "verb": p.verb, "text": p.text, "dir": p.dir, "window": window,
            "groups": p.groups, "at": span(p.at), "sel": span(p.sel),
        }));
    }

    fn window_of(&self, v: &Value) -> Result<WindowId, String> {
        let id = v["window"].as_u64().ok_or("window: a window id")?;
        let w = WindowId(id);
        self.remote.node.state.window(w).map_err(|_| format!("no window {id}"))?;
        Ok(w)
    }

    fn body_of(&self, w: WindowId) -> Result<BufferId, String> {
        self.remote.node.state.window(w).map_err(|e| e.to_string())?.body_buffer().ok_or_else(|| "not a text window".into())
    }

    fn command(&mut self, v: &Value) -> Value {
        let id = v["id"].clone();
        let cmd = v["cmd"].as_str().unwrap_or("").to_string();
        match self.run_command(&cmd, v) {
            Ok(mut result) => {
                if let Value::Object(m) = &mut result {
                    m.insert("id".into(), id);
                    m.insert("ok".into(), json!(true));
                }
                result
            }
            Err(e) => json!({ "id": id, "ok": false, "error": e }),
        }
    }

    fn run_command(&mut self, cmd: &str, v: &Value) -> Result<Value, String> {
        let node = &self.remote.node;
        match cmd {
            "windows" => {
                let mut list: Vec<Value> = node
                    .state
                    .windows
                    .keys()
                    .copied()
                    .map(|w| json!({ "id": w.0, "name": node.window_name(w), "kind": node.window_kind(w).name(), "live": node.window_live(w) }))
                    .collect();
                list.sort_by_key(|w| w["id"].as_u64());
                Ok(json!({ "windows": list }))
            }
            "new" => {
                let name = v["name"].as_str().ok_or("name")?.to_string();
                let col = node.state.layout.cols.last().map(|c| c.id).ok_or("no column")?;
                let w = self.propose(Proposal::NewWindow { col, name: name.clone() })?.ok_or("no window made")?;
                self.remember(w);
                Ok(json!({ "window": w.0 }))
            }
            "open" => {
                let name = v["name"].as_str().ok_or("name")?.to_string();
                let pos = match v["line"].as_u64() {
                    Some(l) => Pos::Line(l as usize),
                    None => Pos::Keep,
                };
                self.propose(Proposal::Goto { loc: Loc { name: name.clone(), pos } })?;
                // the file may be on its way: wait for its window
                let deadline = std::time::Instant::now() + TIMEOUT;
                loop {
                    if let Some(w) = self.remote.node.state.windows.keys().copied().find(|w| self.remote.node.window_name(*w) == name) {
                        self.remember(w);
                        return Ok(json!({ "window": w.0 }));
                    }
                    if std::time::Instant::now() > deadline {
                        return Err(format!("{name}: not opened"));
                    }
                    if !self.step(Duration::from_millis(50)) {
                        return Err("connection closed".into());
                    }
                }
            }
            "read" => {
                let w = self.window_of(v)?;
                let b = self.body_of(w)?;
                let text = node.state.buffer(b).map_err(|e| e.to_string())?.text.to_string();
                Ok(json!({ "text": text }))
            }
            "selection" => {
                let w = self.window_of(v)?;
                let (q0, q1) = node.selection(ViewId::Body(w)).map_err(|e| e.to_string())?;
                Ok(json!({ "q0": q0, "q1": q1 }))
            }
            "write" => {
                let w = self.window_of(v)?;
                let b = self.body_of(w)?;
                let text = v["text"].as_str().unwrap_or("").to_string();
                let buf = node.state.buffer(b).map_err(|e| e.to_string())?;
                let (len, version) = (buf.text.len(), buf.version);
                let at = |x: &Value| -> usize {
                    match x.as_i64() {
                        Some(n) if n >= 0 => (n as usize).min(len),
                        _ => len,
                    }
                };
                let (q0, q1) = (at(&v["q0"]), at(&v["q1"]));
                if q1 < q0 {
                    return Err("q1 before q0".into());
                }
                self.propose(Proposal::ReplaceRange { dir: None, buffer: b, version, q0, q1, text })?;
                Ok(json!({}))
            }
            "select" => {
                let w = self.window_of(v)?;
                let (q0, q1) = (v["q0"].as_u64().ok_or("q0")? as usize, v["q1"].as_u64().ok_or("q1")? as usize);
                self.propose(Proposal::Select { view: ViewId::Body(w), q0, q1 })?;
                Ok(json!({}))
            }
            "rename" => {
                let w = self.window_of(v)?;
                let b = self.body_of(w)?;
                let name = v["name"].as_str().ok_or("name")?.to_string();
                self.propose(Proposal::Rename { buffer: b, window: w, name: name.clone() })?;
                self.ours.entry(w).and_modify(|n| *n = name);
                Ok(json!({}))
            }
            "live" => {
                let w = self.window_of(v)?;
                let by = v["on"].as_bool().unwrap_or(true).then_some(self.remote.attachment());
                self.propose(Proposal::Live { window: w, by })?;
                Ok(json!({}))
            }
            "delete" => {
                let w = self.window_of(v)?;
                self.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() })?;
                Ok(json!({}))
            }
            "exec" => {
                let text = v["text"].as_str().ok_or("text")?.to_string();
                let ctx = match v["window"].as_u64() {
                    Some(_) => ExecCtx::Window(self.window_of(v)?),
                    None => ExecCtx::Top,
                };
                self.propose(Proposal::Exec { ctx, text })?;
                Ok(json!({}))
            }
            "errors" => {
                let text = v["text"].as_str().ok_or("text")?.to_string();
                let dir = v["dir"].as_str().map(String::from);
                self.propose(Proposal::Errors { dir, text })?;
                Ok(json!({}))
            }
            "rule" => {
                let kind = match v["kind"].as_str() {
                    Some(k) => Some(WinKind::parse(k).ok_or_else(|| format!("kind {k}: file, dir, term, errors or web"))?),
                    None => None,
                };
                let me = node.state.meta.attachments.get(&self.remote.attachment()).map(|a| a.name.clone()).ok_or("not attached")?;
                let rule = PlumbRule {
                    verb: v["verb"].as_str().unwrap_or("plumb").to_string(),
                    text: v["text"].as_str().map(String::from),
                    file: v["file"].as_str().map(String::from),
                    kind,
                    win: v["window"].as_u64().map(WindowId),
                    isfile: None,
                    isdir: None,
                    action: RuleAction::Tool(me),
                    to: None,
                };
                rule.check()?;
                let priority = v["priority"].as_i64().unwrap_or(0) as i32;
                let id = self.remote.rule_add(rule, priority, true, TIMEOUT)?;
                Ok(json!({ "rule": id.0 }))
            }
            "unrule" => {
                let id = v["rule"].as_u64().ok_or("rule")?;
                self.remote.send(&ClientMsg::RuleRm { id: RuleId(id) });
                Ok(json!({}))
            }
            "ack" => {
                let plumb = v["plumb"].as_u64().ok_or("plumb")?;
                self.remote.plumb_ack(plumb, v["ok"].as_bool().unwrap_or(true));
                Ok(json!({}))
            }
            "watch" => {
                let w = self.window_of(v)?;
                self.body_of(w)?;
                self.watched.insert(w);
                self.remember(w);
                Ok(json!({}))
            }
            "unwatch" => {
                let w = WindowId(v["window"].as_u64().ok_or("window")?);
                self.watched.remove(&w);
                Ok(json!({}))
            }
            "set" => {
                let key = v["key"].as_str().ok_or("key")?.to_string();
                let value = v["value"].as_str().unwrap_or("").to_string();
                self.remote.send(&ClientMsg::Set { key, value, attachment: None });
                Ok(json!({}))
            }
            "setting" => {
                let key = v["key"].as_str().ok_or("key")?;
                let value = node.state.meta.setting(self.remote.attachment(), key).map(String::from);
                Ok(json!({ "value": value }))
            }
            "" => Err("cmd: which command".into()),
            other => Err(format!("{other}: no such command")),
        }
    }

    fn remember(&mut self, w: WindowId) {
        let name = self.remote.node.window_name(w);
        self.ours.insert(w, name);
    }
}
