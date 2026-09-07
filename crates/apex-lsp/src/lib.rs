//! `apex lsp`: language servers as an apex tool. It is not privileged: it
//! attaches like any tool, keeps a replica, reads buffer edits off the
//! entry stream to feed servers incrementally, proposes what they answer,
//! and installs plumbing rules that name it. Servers come from settings
//! (`lsp.go gopls`), with defaults for the usual languages; one runs per
//! workspace root.
//!
//! What it offers, through rules owned by its attachment: B3 on an
//! identifier in a source file goes to its definition (NACK when there
//! is none, so the walk goes on); the verbs `Def Refs Type Hov Sig Fmt Rn`
//! in the tools menu of source windows. Diagnostics go to `root/+lsp`.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use apex_core::text::Text;
use apex_core::*;
use apex_server::proto::{ClientMsg, ServerMsg};
use apex_server::remote::{Remote, ToolPlumb};
use apex_server::Proposal;

pub mod pos;

const TIMEOUT: Duration = Duration::from_secs(10);
/// The verbs offered in source windows.
pub const VERBS: [&str; 7] = ["Def", "Refs", "Type", "Hov", "Sig", "Fmt", "Rn"];

/// A language the tool knows: how to recognise its files, what to run,
/// and what marks a workspace root.
#[derive(Clone, Debug)]
pub struct Language {
    pub id: &'static str,
    pub exts: &'static [&'static str],
    pub default_server: &'static str,
    pub roots: &'static [&'static str],
}

pub const LANGUAGES: &[Language] = &[
    Language { id: "go", exts: &["go"], default_server: "gopls", roots: &["go.work", "go.mod"] },
    Language { id: "rust", exts: &["rs"], default_server: "rust-analyzer", roots: &["Cargo.toml"] },
    Language { id: "python", exts: &["py"], default_server: "pyright-langserver --stdio", roots: &["pyproject.toml", "setup.py", "requirements.txt"] },
    Language { id: "typescript", exts: &["ts", "tsx", "js", "jsx"], default_server: "typescript-language-server --stdio", roots: &["package.json"] },
    Language { id: "c", exts: &["c", "h", "cc", "cpp", "hpp"], default_server: "clangd", roots: &["compile_commands.json", "Makefile"] },
];

pub fn language_of(path: &str) -> Option<&'static Language> {
    let ext = Path::new(path).extension()?.to_str()?;
    LANGUAGES.iter().find(|l| l.exts.contains(&ext))
}

/// The workspace root of `path` for `lang`: the nearest directory up
/// from it with one of the language's markers, else `.git`, else the
/// file's directory.
pub fn root_of(path: &Path, lang: &Language) -> PathBuf {
    let dir = path.parent().unwrap_or(path);
    let mut d = dir;
    loop {
        if lang.roots.iter().any(|m| d.join(m).exists()) {
            return d.to_path_buf();
        }
        match d.parent() {
            Some(p) => d = p,
            None => break,
        }
    }
    let mut d = dir;
    loop {
        if d.join(".git").exists() {
            return d.to_path_buf();
        }
        match d.parent() {
            Some(p) => d = p,
            None => return dir.to_path_buf(),
        }
    }
}

// ---- JSON-RPC over stdio ---------------------------------------------------

/// One language server process.
struct Server {
    key: (String, PathBuf),
    child: Child,
    stdin: Arc<Mutex<Box<dyn Write + Send>>>,
    next_id: u64,
    initialized: bool,
    /// Notifications to send once initialized.
    queued: Vec<(String, Value)>,
    /// Buffers open in this server, by uri: our version counter.
    open: HashMap<String, i64>,
    /// Diagnostics per file, as last published.
    diagnostics: BTreeMap<String, Vec<String>>,
}

impl Server {
    fn spawn(key: (String, PathBuf), cmd: &str, tx: Sender<Event>) -> Result<Server, String> {
        let mut words = cmd.split_whitespace();
        let prog = words.next().ok_or("empty server command")?;
        let mut child = Command::new(prog)
            .args(words)
            .current_dir(&key.1)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{cmd}: {e}"))?;
        let stdin: Box<dyn Write + Send> = Box::new(child.stdin.take().expect("piped"));
        let stdout = child.stdout.take().expect("piped");
        let k = key.clone();
        std::thread::spawn(move || {
            let mut r = std::io::BufReader::new(stdout);
            while let Some(v) = read_message(&mut r) {
                if debug() {
                    eprintln!("apex lsp < {v}");
                }
                if tx.send(Event::Lsp(k.clone(), v)).is_err() {
                    break;
                }
            }
            let _ = tx.send(Event::LspGone(k));
        });
        Ok(Server { key, child, stdin: Arc::new(Mutex::new(stdin)), next_id: 1, initialized: false, queued: Vec::new(), open: HashMap::new(), diagnostics: BTreeMap::new() })
    }

    fn send(&self, v: &Value) {
        let body = v.to_string();
        if debug() {
            eprintln!("apex lsp > {body}");
        }
        let mut w = self.stdin.lock().unwrap();
        let _ = write!(w, "Content-Length: {}\r\n\r\n{}", body.len(), body);
        let _ = w.flush();
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        if !self.initialized && method != "initialized" {
            self.queued.push((method.to_string(), params));
            return;
        }
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }
}

fn read_message<R: BufRead>(r: &mut R) -> Option<Value> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse().ok();
        }
    }
    let len = len?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// `APEX_LSP_DEBUG` set: the JSON-RPC traffic on stderr.
fn debug() -> bool {
    std::env::var_os("APEX_LSP_DEBUG").is_some()
}

fn uri_of(path: &Path) -> String {
    let mut s = String::from("file://");
    for c in path.to_string_lossy().chars() {
        if c.is_ascii_alphanumeric() || "/-_.~".contains(c) {
            s.push(c);
        } else {
            for b in c.to_string().as_bytes() {
                s.push_str(&format!("%{b:02X}"));
            }
        }
    }
    s
}

fn path_of_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut out = Vec::new();
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 1 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).to_string()))
}

// ---- the tool ----------------------------------------------------------------

enum Event {
    Lsp((String, PathBuf), Value),
    LspGone((String, PathBuf)),
}

/// What a request to a server is for, so its answer can be acted on.
enum Waiting {
    Initialize,
    /// A plumb (B3 or a verb): answer it with ACK/NACK.
    Plumb { plumb: u64, verb: String, buffer: BufferId, ctx: ExecCtx, dir: String },
}

pub struct Tool {
    remote: Remote,
    servers: HashMap<(String, PathBuf), Server>,
    /// Open buffers this tool told a server about: buffer → (server key, uri).
    docs: HashMap<BufferId, ((String, PathBuf), String)>,
    waiting: HashMap<((String, PathBuf), u64), Waiting>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    /// Languages whose servers could not start, so we stop trying.
    failed: Vec<(String, PathBuf)>,
}

/// Run the tool on the session at `socket`, until the link ends.
pub fn run(socket: &Path, session: &str) -> Result<(), String> {
    let remote = Remote::connect_as(socket, session, "lsp", AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
    let (tx, rx) = channel();
    let mut t = Tool { remote, servers: HashMap::new(), docs: HashMap::new(), waiting: HashMap::new(), tx, rx, failed: Vec::new() };
    t.install_rules()?;
    t.main_loop()
}

impl Tool {
    fn setting(&self, key: &str) -> Option<String> {
        self.remote.node.state.meta.setting(self.remote.attachment(), key).map(String::from)
    }

    /// The rules that name us: the verbs in source windows, and B3 on an
    /// identifier there, ahead of the path rules.
    fn install_rules(&mut self) -> Result<(), String> {
        let exts: Vec<&str> = LANGUAGES.iter().flat_map(|l| l.exts.iter().copied()).collect();
        let file = format!(r"\.({})$", exts.join("|"));
        let rule = |verb: &str, text: Option<&str>| PlumbRule {
            verb: verb.into(),
            text: text.map(String::from),
            file: Some(file.clone()),
            kind: Some(WinKind::File),
            isfile: None,
            isdir: None,
            action: RuleAction::Tool("lsp".into()),
            to: None,
        };
        self.remote.rule_add(rule("plumb", Some(r"[A-Za-z_][A-Za-z0-9_]*")), 10, true, TIMEOUT)?;
        for v in VERBS {
            self.remote.rule_add(rule(v, None), 0, true, TIMEOUT)?;
        }
        Ok(())
    }

    fn main_loop(&mut self) -> Result<(), String> {
        loop {
            let mut busy = false;
            // the session: edits go to servers before they land in the replica
            while let Ok(m) = self.remote.link.rx.try_recv() {
                busy = true;
                self.before(&m);
                if !self.remote.handle(m) {
                    return Ok(()); // the link ended
                }
            }
            let plumbs: Vec<ToolPlumb> = std::mem::take(&mut self.remote.link.plumbs);
            for p in plumbs {
                busy = true;
                self.on_plumb(p);
            }
            while let Ok(ev) = self.rx.try_recv() {
                busy = true;
                match ev {
                    Event::Lsp(key, v) => self.on_lsp(key, v),
                    Event::LspGone(key) => {
                        self.servers.remove(&key);
                        self.docs.retain(|_, (k, _)| *k != key);
                    }
                }
            }
            self.sync_docs();
            if !busy {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// An entry stream message about to be applied: edits to documents
    /// a server knows become `didChange`, computed against the text as it
    /// still is.
    fn before(&mut self, m: &ServerMsg) {
        let ServerMsg::Entries { shard: Shard::Buffer(b), entries } = m else { return };
        let Some((key, uri)) = self.docs.get(b).cloned() else { return };
        let Ok(buf) = self.remote.node.state.buffer(*b) else { return };
        // several edits in one message: each against the text after the
        // previous, so keep a shadow
        let mut shadow = buf.text.clone();
        for e in entries {
            let Op::Buffer(BufferOp::Edit { q0, nd, text, .. }) = &e.op else { continue };
            let start = pos::position(&shadow, *q0);
            let end = pos::position(&shadow, q0 + nd);
            shadow.replace(*q0, *nd, text);
            if let Some(s) = self.servers.get_mut(&key) {
                let v = s.open.entry(uri.clone()).or_insert(1);
                *v += 1;
                let version = *v;
                s.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": version },
                        "contentChanges": [{ "range": { "start": start, "end": end }, "text": text }]
                    }),
                );
            }
        }
    }

    /// Buffers of known languages get a server and a `didOpen`; buffers
    /// gone get `didClose`.
    fn sync_docs(&mut self) {
        let present: Vec<(BufferId, String)> = self.remote.node.state.buffers.values().filter(|b| b.name.starts_with('/')).map(|b| (b.id, b.name.clone())).collect();
        for (b, name) in &present {
            if self.docs.contains_key(b) {
                continue;
            }
            let Some(lang) = language_of(name) else { continue };
            let path = PathBuf::from(name);
            let root = root_of(&path, lang);
            let key = (lang.id.to_string(), root.clone());
            if self.failed.contains(&key) {
                continue;
            }
            if !self.servers.contains_key(&key) {
                let cmd = self.setting(&format!("lsp.{}", lang.id)).unwrap_or_else(|| lang.default_server.to_string());
                match Server::spawn(key.clone(), &cmd, self.tx.clone()) {
                    Ok(mut s) => {
                        let id = s.request(
                            "initialize",
                            json!({
                                "processId": std::process::id(),
                                "rootUri": uri_of(&root),
                                "capabilities": {
                                    "textDocument": {
                                        "synchronization": { "didSave": false },
                                        "hover": { "contentFormat": ["plaintext", "markdown"] },
                                        "publishDiagnostics": {}
                                    }
                                },
                                "workspaceFolders": [{ "uri": uri_of(&root), "name": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() }]
                            }),
                        );
                        self.waiting.insert((key.clone(), id), Waiting::Initialize);
                        self.servers.insert(key.clone(), s);
                    }
                    Err(e) => {
                        if debug() {
                            eprintln!("apex lsp: start {cmd}: {e}");
                        }
                        self.failed.push(key.clone());
                        self.errors(Some(&root.display().to_string()), &format!("lsp: {}: {e}\n", lang.id));
                        continue;
                    }
                }
            }
            let uri = uri_of(&path);
            let text = self.remote.node.state.buffer(*b).map(|x| x.text.to_string()).unwrap_or_default();
            let s = self.servers.get_mut(&key).unwrap();
            s.open.insert(uri.clone(), 1);
            s.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": lang.id, "version": 1, "text": text } }));
            self.docs.insert(*b, (key, uri));
        }
        let gone: Vec<BufferId> = self.docs.keys().copied().filter(|b| !present.iter().any(|(p, _)| p == b)).collect();
        for b in gone {
            if let Some((key, uri)) = self.docs.remove(&b) {
                if let Some(s) = self.servers.get_mut(&key) {
                    s.open.remove(&uri);
                    s.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }));
                }
            }
        }
    }

    fn errors(&mut self, dir: Option<&str>, text: &str) {
        let _ = self.remote.propose(Proposal::Errors { dir: dir.map(String::from), text: text.to_string() }, TIMEOUT);
    }

    /// A message from a server: an answer to something we asked, or a
    /// notification (diagnostics).
    fn on_lsp(&mut self, key: (String, PathBuf), v: Value) {
        if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
            if v.get("method").is_some() {
                // a request from the server: answer what we can, refuse the rest
                let method = v["method"].as_str().unwrap_or("");
                let result = match method {
                    "workspace/configuration" => json!(v["params"]["items"].as_array().map(|a| vec![Value::Null; a.len()]).unwrap_or_default()),
                    "client/registerCapability" | "workspace/workspaceFolders" => Value::Null,
                    _ => Value::Null,
                };
                if let Some(s) = self.servers.get(&key) {
                    s.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                }
                return;
            }
            let Some(w) = self.waiting.remove(&(key.clone(), id)) else { return };
            match w {
                Waiting::Initialize => {
                    if let Some(s) = self.servers.get_mut(&key) {
                        s.initialized = true;
                        s.notify("initialized", json!({}));
                        for (m, p) in std::mem::take(&mut s.queued) {
                            s.notify(&m, p);
                        }
                    }
                }
                Waiting::Plumb { plumb, verb, buffer, ctx, dir } => {
                    let ok = self.answer(&key, &verb, buffer, ctx, &dir, &v);
                    self.remote.plumb_ack(plumb, ok);
                }
            }
            return;
        }
        match v.get("method").and_then(|m| m.as_str()) {
            Some("textDocument/publishDiagnostics") => {
                let uri = v["params"]["uri"].as_str().unwrap_or("").to_string();
                let path = path_of_uri(&uri).map(|p| p.display().to_string()).unwrap_or(uri);
                let lines: Vec<String> = v["params"]["diagnostics"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|d| {
                                let (l, c) = (d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1, d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1);
                                let sev = match d["severity"].as_u64() {
                                    Some(1) => "error",
                                    Some(2) => "warning",
                                    Some(3) => "info",
                                    _ => "hint",
                                };
                                format!("{path}:{l}:{c}: {sev}: {}", d["message"].as_str().unwrap_or("").lines().next().unwrap_or(""))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(s) = self.servers.get_mut(&key) {
                    if lines.is_empty() {
                        s.diagnostics.remove(&path);
                    } else {
                        s.diagnostics.insert(path, lines);
                    }
                }
                self.show_diagnostics(&key);
            }
            _ => {}
        }
    }

    /// `root/+lsp`: every diagnostic the server has, one per line, the
    /// window made when there is something to say.
    fn show_diagnostics(&mut self, key: &(String, PathBuf)) {
        let Some(s) = self.servers.get(key) else { return };
        let text: String = s.diagnostics.values().flat_map(|v| v.iter()).map(|l| format!("{l}\n")).collect();
        let name = format!("{}/+lsp", key.1.display().to_string().trim_end_matches('/'));
        let node = &self.remote.node;
        let existing = node.state.windows.keys().copied().find(|w| node.window_name(*w) == name);
        let w = match existing {
            Some(w) => w,
            None => {
                if text.is_empty() {
                    return;
                }
                let Some(col) = node.state.layout.cols.last().map(|c| c.id) else { return };
                match self.remote.propose(Proposal::NewWindow { col, name: name.clone() }, TIMEOUT) {
                    Ok(Some(w)) => {
                        // its entries may still be on their way
                        let deadline = std::time::Instant::now() + TIMEOUT;
                        while self.remote.node.state.window(w).is_err() && std::time::Instant::now() < deadline {
                            let _ = self.remote.step(Duration::from_millis(20));
                        }
                        w
                    }
                    _ => return,
                }
            }
        };
        let Some(buffer) = self.remote.node.state.window(w).ok().and_then(|x| x.body_buffer()) else { return };
        let hash = Text::new(&text).content_hash();
        let _ = self.remote.propose(Proposal::SetContent { buffer, version: None, text, hash }, TIMEOUT);
    }

    /// A rule named us: B3 on an identifier, or a verb from the menu.
    fn on_plumb(&mut self, p: ToolPlumb) {
        let span = p.at.or(p.sel);
        let Some(span) = span else {
            self.remote.plumb_ack(p.id, false);
            return;
        };
        let Some((key, uri)) = self.docs.get(&span.buffer).cloned() else {
            self.remote.plumb_ack(p.id, false);
            return;
        };
        let Ok(buf) = self.remote.node.state.buffer(span.buffer) else {
            self.remote.plumb_ack(p.id, false);
            return;
        };
        let at = pos::position(&buf.text, span.q0);
        let td = json!({ "textDocument": { "uri": uri }, "position": at });
        let (method, params) = match p.verb.as_str() {
            "plumb" | "Def" => ("textDocument/definition", td),
            "Type" => ("textDocument/typeDefinition", td),
            "Refs" => ("textDocument/references", json!({ "textDocument": { "uri": uri }, "position": at, "context": { "includeDeclaration": true } })),
            "Hov" => ("textDocument/hover", td),
            "Sig" => ("textDocument/signatureHelp", td),
            "Fmt" => ("textDocument/formatting", json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 8, "insertSpaces": false } })),
            "Rn" => {
                let new = p.text.trim();
                if new.is_empty() {
                    self.errors(Some(&p.dir), "Rn needs the new name: Rn name\n");
                    self.remote.plumb_ack(p.id, false);
                    return;
                }
                ("textDocument/rename", json!({ "textDocument": { "uri": uri }, "position": at, "newName": new }))
            }
            _ => {
                self.remote.plumb_ack(p.id, false);
                return;
            }
        };
        let Some(s) = self.servers.get_mut(&key) else {
            self.remote.plumb_ack(p.id, false);
            return;
        };
        if !s.initialized {
            self.remote.plumb_ack(p.id, false);
            return;
        }
        let id = s.request(method, params);
        self.waiting.insert((key, id), Waiting::Plumb { plumb: p.id, verb: p.verb, buffer: span.buffer, ctx: p.ctx, dir: p.dir });
    }

    /// Act on a server's answer to a plumb; true when it was taken.
    fn answer(&mut self, key: &(String, PathBuf), verb: &str, buffer: BufferId, _ctx: ExecCtx, dir: &str, v: &Value) -> bool {
        let result = &v["result"];
        if result.is_null() {
            return false;
        }
        match verb {
            "plumb" | "Def" | "Type" => {
                let loc = match result {
                    Value::Array(a) => a.first().cloned(),
                    o => Some(o.clone()),
                };
                let Some(loc) = loc else { return false };
                let (uri, range) = if loc.get("targetUri").is_some() { (loc["targetUri"].clone(), loc["targetSelectionRange"].clone()) } else { (loc["uri"].clone(), loc["range"].clone()) };
                let Some(path) = uri.as_str().and_then(path_of_uri) else { return false };
                self.open_at(&path, &range)
            }
            "Refs" => {
                let Some(a) = result.as_array() else { return false };
                let text: String = a
                    .iter()
                    .filter_map(|l| {
                        let p = path_of_uri(l["uri"].as_str()?)?;
                        Some(format!("{}:{}:{}\n", p.display(), l["range"]["start"]["line"].as_u64()? + 1, l["range"]["start"]["character"].as_u64()? + 1))
                    })
                    .collect();
                if text.is_empty() {
                    return false;
                }
                self.errors(Some(dir), &text);
                true
            }
            "Hov" => {
                let c = &result["contents"];
                let text = match c {
                    Value::String(s) => s.clone(),
                    Value::Object(o) => o.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    Value::Array(a) => a.iter().map(|x| x.as_str().map(String::from).or_else(|| x.get("value").and_then(|v| v.as_str()).map(String::from)).unwrap_or_default()).collect::<Vec<_>>().join("\n"),
                    _ => String::new(),
                };
                if text.trim().is_empty() {
                    return false;
                }
                self.errors(Some(dir), &format!("{}\n", text.trim_end()));
                true
            }
            "Sig" => {
                let Some(sigs) = result["signatures"].as_array() else { return false };
                let text: String = sigs.iter().filter_map(|s| s["label"].as_str()).map(|l| format!("{l}\n")).collect();
                if text.is_empty() {
                    return false;
                }
                self.errors(Some(dir), &text);
                true
            }
            "Fmt" => {
                let Some(edits) = result.as_array() else { return false };
                let Ok(buf) = self.remote.node.state.buffer(buffer) else { return false };
                let new = pos::apply_edits(&buf.text, edits);
                if new == buf.text.to_string() {
                    return true;
                }
                let len = buf.text.len();
                let version = buf.version;
                self.remote.propose(Proposal::ReplaceRange { dir: Some(dir.to_string()), buffer, version, q0: 0, q1: len, text: new }, TIMEOUT).is_ok()
            }
            "Rn" => {
                // the workspace edit: open buffers through the session, the
                // rest on disk
                let mut changes: Vec<(String, Vec<Value>)> = Vec::new();
                if let Some(o) = result["changes"].as_object() {
                    for (uri, edits) in o {
                        changes.push((uri.clone(), edits.as_array().cloned().unwrap_or_default()));
                    }
                }
                if let Some(a) = result["documentChanges"].as_array() {
                    for dc in a {
                        if let Some(uri) = dc["textDocument"]["uri"].as_str() {
                            changes.push((uri.to_string(), dc["edits"].as_array().cloned().unwrap_or_default()));
                        }
                    }
                }
                if changes.is_empty() {
                    return false;
                }
                let mut report = String::new();
                for (uri, edits) in changes {
                    let Some(path) = path_of_uri(&uri) else { continue };
                    let name = path.display().to_string();
                    let open = self.remote.node.state.buffers.values().find(|b| b.name == name).map(|b| (b.id, b.version, b.text.clone(), b.text.len()));
                    match open {
                        Some((b, version, text, len)) => {
                            let new = pos::apply_edits(&text, &edits);
                            let _ = self.remote.propose(Proposal::ReplaceRange { dir: Some(dir.to_string()), buffer: b, version, q0: 0, q1: len, text: new }, TIMEOUT);
                        }
                        None => {
                            if let Ok(s) = std::fs::read_to_string(&path) {
                                let new = pos::apply_edits(&Text::new(&s), &edits);
                                let _ = std::fs::write(&path, new);
                            }
                        }
                    }
                    report.push_str(&format!("{name}: renamed\n"));
                }
                self.errors(Some(dir), &report);
                let _ = key;
                true
            }
            _ => false,
        }
    }

    /// Show `path` at an LSP range: the window if it is open, else opened
    /// through the session; then the range selected.
    fn open_at(&mut self, path: &Path, range: &Value) -> bool {
        let name = path.display().to_string();
        let find = |node: &Node| node.state.windows.keys().copied().find(|w| node.window_name(*w) == name);
        let mut w = find(&self.remote.node);
        if w.is_none() {
            let Some(col) = self.remote.node.state.layout.cols.first().map(|c| c.id) else { return false };
            self.remote.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: name.clone() });
            let deadline = std::time::Instant::now() + TIMEOUT;
            while w.is_none() && std::time::Instant::now() < deadline {
                let _ = self.remote.step(Duration::from_millis(50));
                w = find(&self.remote.node);
            }
        }
        let Some(w) = w else { return false };
        let Some(b) = self.remote.node.state.window(w).ok().and_then(|x| x.body_buffer()) else { return false };
        let Ok(buf) = self.remote.node.state.buffer(b) else { return false };
        let q0 = pos::offset(&buf.text, &range["start"]);
        let q1 = pos::offset(&buf.text, &range["end"]);
        self.remote.propose(Proposal::Select { view: ViewId::Body(w), q0, q1: q1.max(q0) }, TIMEOUT).is_ok()
    }
}

impl Drop for Tool {
    fn drop(&mut self) {
        for (_, mut s) in self.servers.drain() {
            let _ = s.child.kill();
        }
    }
}
