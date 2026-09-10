//! `apex tool bridge NAME`: the session as a tool in any language sees
//! it. The bridge attaches as the tool named NAME and speaks JSON, one
//! object per line, on its standard input and output; a program that
//! starts it gets windows, rules and plumbs without the wire protocol
//! or the replicated state behind them. It is a client of `apex-tool`,
//! and its commands and events are that crate's methods and events one
//! for one.
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
//! - `show {window, at}` or `show {window, line}`: the text there brought
//!   into view if it is off screen, dot and the mouse left alone (where
//!   `open` jumps the user there); `line {window, line}` → `q0, q1`
//! - `rename {window, name}`, `live {window, on}`, `delete {window}`
//! - `exec {window?, text}` (B2 there), `errors {dir?, text}` (+Errors)
//! - `switch {session, window?}`: another session shown (by id, a prefix
//!   or label), at a window there
//! - `rule {verb?, text?, file?, kind?, window?, priority?}` → `rule`: a
//!   rule answered by this tool (`plumb` events); `unrule {rule}`
//! - `ack {plumb, ok}`: the answer to a `plumb` event (within a second)
//! - `watch {window}` / `unwatch {window}`: `edit` events for its body
//! - `set {key, value}` / `setting {key}` → `value`
//!
//! **Events** go out as `{"event": "...", ...}`: `hello {attachment,
//! session}` first; `plumb {plumb, rule, verb, text, dir, window?,
//! groups, at?, sel?}` when a rule of ours matched (`rule` says which;
//! `at` and `sel` are `{q0, q1}` in the window's body); `edit {window,
//! q0, nd, text}` for a watched window, edits by others only; `renamed
//! {window, name}` and `deleted {window}` for windows we made, opened
//! or watched; `bye` when the session or the link ends, after which the
//! bridge exits. Offsets count characters, as apex does throughout.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

use apex_tool::{Event, Plumb, Rule, RuleId, Tool, WinKind, WindowId, END};

/// Run the bridge for the session at `socket`, as the tool `name`,
/// until stdin closes or the link ends.
pub fn run(socket: &Path, session: &str, name: &str) -> Result<(), String> {
    let tool = Tool::attach_to(socket, session, name).map_err(|e| e.to_string())?;
    let mut b = Bridge { tool, out: std::io::stdout() };
    b.emit(json!({ "event": "hello", "session": session, "tool": name }));
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

struct Bridge {
    tool: Tool,
    out: std::io::Stdout,
}

impl Bridge {
    fn emit(&mut self, v: Value) {
        let mut o = self.out.lock();
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    }

    fn main_loop(&mut self, rx: &Receiver<Option<String>>) -> Result<(), String> {
        loop {
            // what happened, without waiting
            loop {
                match self.tool.next_event(Some(Duration::ZERO)) {
                    Ok(Some(ev)) => self.event(ev),
                    Ok(None) => break,
                    Err(e) => return Err(e.to_string()),
                }
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

    fn event(&mut self, ev: Event) {
        let v = match ev {
            Event::Plumb(p) => plumb_json(&p),
            Event::Edit(e) => json!({ "event": "edit", "window": e.window.0, "q0": e.q0, "nd": e.nd, "text": e.text }),
            Event::Renamed { window, name } => json!({ "event": "renamed", "window": window.0, "name": name }),
            Event::Deleted { window } => json!({ "event": "deleted", "window": window.0 }),
        };
        self.emit(v);
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
        let window = |v: &Value| -> Result<WindowId, String> { v["window"].as_u64().map(WindowId).ok_or_else(|| "window: a window id".into()) };
        let e = |e: apex_tool::Error| e.to_string();
        match cmd {
            "windows" => {
                let list: Vec<Value> = self.tool.windows().into_iter().map(|w| json!({ "id": w.id.0, "name": w.name, "kind": w.kind.name(), "live": w.live })).collect();
                Ok(json!({ "windows": list }))
            }
            "new" => {
                let name = v["name"].as_str().ok_or("name")?;
                Ok(json!({ "window": self.tool.new_window(name).map_err(e)?.0 }))
            }
            "open" => {
                let name = v["name"].as_str().ok_or("name")?;
                let line = v["line"].as_u64().map(|l| l as usize);
                Ok(json!({ "window": self.tool.open(name, line).map_err(e)?.0 }))
            }
            "read" => Ok(json!({ "text": self.tool.read(window(v)?).map_err(e)? })),
            "selection" => {
                let r = self.tool.selection(window(v)?).map_err(e)?;
                Ok(json!({ "q0": r.q0, "q1": r.q1 }))
            }
            "write" => {
                let at = |x: &Value| -> usize {
                    match x.as_i64() {
                        Some(n) if n >= 0 => n as usize,
                        _ => END,
                    }
                };
                self.tool.replace(window(v)?, at(&v["q0"]), at(&v["q1"]), v["text"].as_str().unwrap_or("")).map_err(e)?;
                Ok(json!({}))
            }
            "select" => {
                let (q0, q1) = (v["q0"].as_u64().ok_or("q0")? as usize, v["q1"].as_u64().ok_or("q1")? as usize);
                self.tool.select(window(v)?, q0, q1).map_err(e)?;
                Ok(json!({}))
            }
            "show" => {
                let w = window(v)?;
                match (v["at"].as_u64(), v["line"].as_u64()) {
                    (Some(at), _) => self.tool.show(w, at as usize).map_err(e)?,
                    (None, Some(n)) => self.tool.show_line(w, n as usize).map_err(e)?,
                    _ => return Err("at or line".into()),
                }
                Ok(json!({}))
            }
            "line" => {
                let r = self.tool.line(window(v)?, v["line"].as_u64().ok_or("line")? as usize).map_err(e)?;
                Ok(json!({ "q0": r.q0, "q1": r.q1 }))
            }
            "rename" => {
                self.tool.rename(window(v)?, v["name"].as_str().ok_or("name")?).map_err(e)?;
                Ok(json!({}))
            }
            "live" => {
                self.tool.set_live(window(v)?, v["on"].as_bool().unwrap_or(true)).map_err(e)?;
                Ok(json!({}))
            }
            "delete" => {
                self.tool.delete(window(v)?).map_err(e)?;
                Ok(json!({}))
            }
            "exec" => {
                let text = v["text"].as_str().ok_or("text")?;
                let w = if v["window"].is_null() { None } else { Some(window(v)?) };
                self.tool.exec_in(w, text).map_err(e)?;
                Ok(json!({}))
            }
            "errors" => {
                self.tool.errors(v["dir"].as_str(), v["text"].as_str().ok_or("text")?).map_err(e)?;
                Ok(json!({}))
            }
            "switch" => {
                let session = v["session"].as_str().ok_or("session")?;
                let w = v["window"].as_u64().map(WindowId);
                self.tool.switch(session, w).map_err(e)?;
                Ok(json!({}))
            }
            "rule" => {
                let mut r = match v["verb"].as_str() {
                    Some(verb) => Rule::verb(verb),
                    None => Rule::plumb(),
                };
                if let Some(t) = v["text"].as_str() {
                    r = r.text(t);
                }
                if let Some(f) = v["file"].as_str() {
                    r = r.file(f);
                }
                if let Some(k) = v["kind"].as_str() {
                    r = r.kind(WinKind::parse(k).ok_or_else(|| format!("kind {k}: file, dir, term, errors or web"))?);
                }
                if let Some(w) = v["window"].as_u64() {
                    r = r.window(WindowId(w));
                }
                if let Some(p) = v["priority"].as_i64() {
                    r = r.priority(p as i32);
                }
                Ok(json!({ "rule": self.tool.offer(r).map_err(e)?.0 }))
            }
            "unrule" => {
                self.tool.withdraw(RuleId(v["rule"].as_u64().ok_or("rule")?));
                Ok(json!({}))
            }
            "ack" => {
                // the plumb by id alone: enough to answer it
                let id = v["plumb"].as_u64().ok_or("plumb")?;
                let p = Plumb { id, rule: RuleId(0), verb: String::new(), text: String::new(), dir: String::new(), window: None, groups: Vec::new(), at: None, sel: None };
                self.tool.answer(&p, v["ok"].as_bool().unwrap_or(true)).map_err(e)?;
                Ok(json!({}))
            }
            "watch" => {
                self.tool.watch(window(v)?).map_err(e)?;
                Ok(json!({}))
            }
            "unwatch" => {
                self.tool.unwatch(window(v)?);
                Ok(json!({}))
            }
            "set" => {
                self.tool.set(v["key"].as_str().ok_or("key")?, v["value"].as_str().unwrap_or(""));
                Ok(json!({}))
            }
            "setting" => Ok(json!({ "value": self.tool.setting(v["key"].as_str().ok_or("key")?) })),
            "" => Err("cmd: which command".into()),
            other => Err(format!("{other}: no such command")),
        }
    }
}

fn plumb_json(p: &Plumb) -> Value {
    let range = |r: Option<apex_tool::Range>| r.map(|r| json!({ "q0": r.q0, "q1": r.q1 }));
    json!({
        "event": "plumb", "plumb": p.id, "rule": p.rule.0, "verb": p.verb, "text": p.text, "dir": p.dir,
        "window": p.window.map(|w| w.0), "groups": p.groups, "at": range(p.at), "sel": range(p.sel),
    })
}
