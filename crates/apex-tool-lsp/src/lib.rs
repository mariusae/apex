//! `apex tool lsp`: language servers as an apex tool. It is not privileged: it
//! attaches like any tool, keeps a replica, reads buffer edits off the
//! entry stream to feed servers incrementally, proposes what they answer,
//! and installs plumbing rules that name it. Servers come from settings
//! (`lsp.go gopls`), with defaults for the usual languages; one runs per
//! workspace root. `lsp.root` or `lsp.LANG.root` can name a marker that
//! overrides the built-in workspace-root discovery.
//!
//! What it offers, through rules owned by its attachment: the verbs
//! `Def Refs Type Hov Sig Fmt Rn` in the tools menu of source windows
//! (cmd-B3 on an identifier is `Def` at the pointer; B3 itself stays
//! acme's look), and `Back`/`Fwd` everywhere. Diagnostics go to
//! `root/+lsp`.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
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
/// Offered everywhere: the session's navigation stack (Goto records
/// every jump; these pop it).
pub const NAV_VERBS: [&str; 2] = ["Back", "Fwd"];

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
    Language { id: "python", exts: &["py", "pyi"], default_server: "pyright-langserver --stdio", roots: &["pyproject.toml", "setup.py", "requirements.txt"] },
    Language { id: "typescript", exts: &["ts", "tsx", "js", "jsx"], default_server: "typescript-language-server --stdio", roots: &["package.json"] },
    Language { id: "c", exts: &["c", "h", "cc", "cpp", "cxx", "hh", "hpp", "hxx"], default_server: "clangd", roots: &["compile_commands.json", "Makefile"] },
];

pub fn language_of(path: &str) -> Option<&'static Language> {
    let ext = Path::new(path).extension()?.to_str()?;
    LANGUAGES.iter().find(|l| l.exts.contains(&ext))
}

/// The workspace root of `path` for `lang`: the nearest directory up
/// from it with one of the language's markers, else `.git`, else the
/// file's directory.
pub fn root_of(path: &Path, lang: &Language) -> PathBuf {
    root_of_with_marker(path, lang, None)
}

/// As [`root_of`], except that a configured marker takes precedence over
/// language markers. This is useful for monorepos whose language servers need
/// the repository root even when nested packages have their own manifests.
fn root_of_with_marker(path: &Path, lang: &Language, marker: Option<&str>) -> PathBuf {
    let dir = path.parent().unwrap_or(path);
    if let Some(marker) = marker.filter(|m| !m.is_empty()) {
        let mut d = dir;
        loop {
            if d.join(marker).exists() {
                return d.to_path_buf();
            }
            match d.parent() {
                Some(p) => d = p,
                None => break,
            }
        }
    }
    let mut d = dir;
    let mut nearest: Option<PathBuf> = None;
    loop {
        if lang.roots.iter().any(|m| d.join(m).exists()) {
            // a Cargo workspace (or Go workspace) above a crate is the root
            let outer = lang.id == "rust" && std::fs::read_to_string(d.join("Cargo.toml")).is_ok_and(|s| s.contains("[workspace]"));
            if outer || lang.id != "rust" {
                return d.to_path_buf();
            }
            nearest.get_or_insert_with(|| d.to_path_buf());
        }
        match d.parent() {
            Some(p) => d = p,
            None => break,
        }
    }
    if let Some(n) = nearest {
        return n;
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

#[cfg(test)]
mod root_tests {
    use super::*;

    #[test]
    fn configured_marker_overrides_nested_language_marker() {
        let root = std::env::temp_dir().join(format!("apex-lsp-marker-{}", std::process::id()));
        let package = root.join("nested/package");
        std::fs::create_dir_all(root.join(".hg")).unwrap();
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(package.join("Cargo.toml"), "[package]\nname = \"nested\"\n").unwrap();
        let file = package.join("src/lib.rs");

        let rust = LANGUAGES.iter().find(|lang| lang.id == "rust").unwrap();
        assert_eq!(root_of(&file, rust), package);
        assert_eq!(root_of_with_marker(&file, rust, Some(".hg")), root);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recognizes_fblsp_file_extensions() {
        for (file, want) in [("types.pyi", "python"), ("impl.cxx", "c"), ("api.hh", "c"), ("more.hxx", "c")] {
            assert_eq!(language_of(file).map(|lang| lang.id), Some(want), "{file}");
        }
    }
}

// ---- JSON-RPC over stdio ---------------------------------------------------

/// One language server process.
struct Server {
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
            .stderr(if debug() { Stdio::inherit() } else { Stdio::null() })
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
        Ok(Server { child, stdin: Arc::new(Mutex::new(stdin)), next_id: 1, initialized: false, queued: Vec::new(), open: HashMap::new(), diagnostics: BTreeMap::new() })
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
    /// `-v`: every request, answer and progress report on stderr.
    verbose: bool,
    /// The verb rules installed per language, once a server of it is
    /// ready; gone with its last server.
    rules: HashMap<String, Vec<RuleId>>,
    servers: HashMap<(String, PathBuf), Server>,
    /// Open buffers this tool told a server about: buffer → (server key, uri).
    docs: HashMap<BufferId, ((String, PathBuf), String)>,
    waiting: HashMap<((String, PathBuf), u64), Waiting>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    /// Languages whose servers could not start, so we stop trying.
    failed: Vec<(String, PathBuf)>,
}

/// Run the tool on the session at `socket`, until the link ends;
/// `verbose` says what goes on between it and the servers on stderr.
pub fn run(socket: &Path, session: &str, verbose: bool) -> Result<(), String> {
    let remote = Remote::connect_as(socket, session, "lsp", AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
    // `apex tool lsp` is called lsp, not apex, in the top row and ps
    remote.announce("lsp");
    let (tx, rx) = channel();
    let mut t = Tool { remote, verbose: verbose || debug(), rules: HashMap::new(), servers: HashMap::new(), docs: HashMap::new(), waiting: HashMap::new(), tx, rx, failed: Vec::new() };
    t.log(&format!("attached to session {session}; servers start as files of known languages open"));
    t.install_rules()?;
    t.main_loop()
}

impl Tool {
    /// What matters on stderr: servers starting, indexing, ready, gone.
    fn log(&self, msg: &str) {
        eprintln!("apex lsp: {msg}");
    }

    /// With -v: requests, answers, progress, diagnostics.
    fn vlog(&self, msg: &str) {
        if self.verbose {
            eprintln!("apex lsp: {msg}");
        }
    }

    /// A server of `lang` is ready: its verbs are offered in that
    /// language's windows from now on (until its last server goes), so
    /// their appearing is the sign the server can be used.
    fn install_verbs(&mut self, lang: &str) -> Result<(), String> {
        if self.rules.contains_key(lang) {
            return Ok(());
        }
        let Some(l) = LANGUAGES.iter().find(|l| l.id == lang) else { return Ok(()) };
        let file = format!(r"\.({})$", l.exts.join("|"));
        let mut ids = Vec::new();
        for v in VERBS {
            let rule = PlumbRule { verb: v.to_string(), text: None, file: Some(file.clone()), kind: Some(WinKind::File), isfile: None, isdir: None, action: RuleAction::Tool("lsp".into()), win: None, to: None };
            ids.push(self.remote.rule_add(rule, 0, true, TIMEOUT)?);
        }
        self.rules.insert(lang.to_string(), ids);
        Ok(())
    }

    fn remove_verbs(&mut self, lang: &str) {
        if self.servers.keys().any(|(l, _)| l == lang) {
            return; // another server of the language is still up
        }
        if let Some(ids) = self.rules.remove(lang) {
            for id in ids {
                self.remote.send(&ClientMsg::RuleRm { id });
            }
        }
    }

    /// One message, through `before` first; false when the link ended.
    fn step(&mut self, timeout: Duration) -> bool {
        match self.remote.link.rx.recv_timeout(timeout) {
            Ok(m) => {
                self.before(&m);
                self.remote.handle(m)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => true,
            Err(_) => false,
        }
    }

    /// Propose and wait for the answer, every message on the way seen
    /// by `before` (Remote::propose would apply them behind our back).
    fn propose(&mut self, p: Proposal, timeout: Duration) -> Result<Option<WindowId>, String> {
        let id = self.remote.link.propose(p);
        let deadline = std::time::Instant::now() + timeout;
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
    fn setting(&self, key: &str) -> Option<String> {
        self.remote.node.state.meta.setting(self.remote.attachment(), key).map(String::from)
    }

    /// The rules that name us from the start: Back and Fwd, the
    /// session's stack, which need no server. The verbs (cmd-B3 is `Def`
    /// at the pointer; B3 itself stays acme's look) come with each
    /// language's server, once it is ready (`install_verbs`).
    fn install_rules(&mut self) -> Result<(), String> {
        for v in NAV_VERBS {
            let r = PlumbRule { verb: v.into(), text: None, file: None, kind: None, isfile: None, isdir: None, action: RuleAction::Tool("lsp".into()), win: None, to: None };
            // a priority below the verbs', so the menu lists them after
            self.remote.rule_add(r, -1, true, TIMEOUT)?;
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
                        // it died: say so, its verbs go, and it is not started again
                        self.servers.remove(&key);
                        self.docs.retain(|_, (k, _)| *k != key);
                        self.failed.push(key.clone());
                        self.remove_verbs(&key.0);
                        self.log(&format!("the {} server for {} exited (APEX_LSP_DEBUG=1 shows its stderr)", key.0, key.1.display()));
                        let msg = format!("lsp: the {} server for {} exited (APEX_LSP_DEBUG=1 shows why)\n", key.0, key.1.display());
                        self.errors(Some(&key.1.display().to_string()), &msg);
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
            let root_marker = self.setting(&format!("lsp.{}.root", lang.id)).or_else(|| self.setting("lsp.root"));
            let root = root_of_with_marker(&path, lang, root_marker.as_deref());
            let key = (lang.id.to_string(), root.clone());
            if self.failed.contains(&key) {
                continue;
            }
            if !self.servers.contains_key(&key) {
                let cmd = self.setting(&format!("lsp.{}", lang.id)).unwrap_or_else(|| lang.default_server.to_string());
                self.log(&format!("starting {cmd} for {} in {}", lang.id, root.display()));
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
                        self.log(&format!("start {cmd}: {e}"));
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
        let _ = self.propose(Proposal::Errors { dir: dir.map(String::from), text: text.to_string() }, TIMEOUT);
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
                    let mut queued = 0;
                    if let Some(s) = self.servers.get_mut(&key) {
                        s.initialized = true;
                        s.notify("initialized", json!({}));
                        let q = std::mem::take(&mut s.queued);
                        queued = q.len();
                        for (m, p) in q {
                            s.notify(&m, p);
                        }
                    }
                    let name = v["result"]["serverInfo"]["name"].as_str().unwrap_or(&key.0).to_string();
                    let version = v["result"]["serverInfo"]["version"].as_str().map(|s| format!(" {s}")).unwrap_or_default();
                    self.log(&format!("{name}{version} ready for {} ({queued} queued notification{} sent); its verbs are offered now", key.1.display(), if queued == 1 { "" } else { "s" }));
                    if let Err(e) = self.install_verbs(&key.0.clone()) {
                        self.log(&format!("rules for {}: {e}", key.0));
                    }
                }
                Waiting::Plumb { plumb, verb, buffer, ctx, dir } => {
                    let ok = self.answer(&key, &verb, buffer, ctx, &dir, &v);
                    self.vlog(&format!("{verb}: {}", if ok { "answered" } else { "nothing" }));
                    self.remote.plumb_ack(plumb, ok);
                }
            }
            return;
        }
        match v.get("method").and_then(|m| m.as_str()) {
            Some("$/progress") => {
                // indexing and the like: begin and end always, the
                // reports in between with -v
                let val = &v["params"]["value"];
                let title = val["title"].as_str().unwrap_or("").to_string();
                let message = val["message"].as_str().map(|m| format!(": {m}")).unwrap_or_default();
                let pct = val["percentage"].as_u64().map(|p| format!(" {p}%")).unwrap_or_default();
                match val["kind"].as_str() {
                    Some("begin") => self.log(&format!("{}: {title}{message}", key.0)),
                    Some("end") => self.log(&format!("{}: {title} done{message}", key.0)),
                    _ => self.vlog(&format!("{}: {title}{pct}{message}", key.0)),
                }
            }
            Some("window/showMessage") => {
                let m = v["params"]["message"].as_str().unwrap_or("");
                self.log(&format!("{}: {m}", key.0));
            }
            Some("window/logMessage") => {
                let m = v["params"]["message"].as_str().unwrap_or("");
                self.vlog(&format!("{}: {m}", key.0));
            }
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
                self.vlog(&format!("{}: {} diagnostic{} for {path}", key.0, lines.len(), if lines.len() == 1 { "" } else { "s" }));
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
                match self.propose(Proposal::NewWindow { col, name: name.clone() }, TIMEOUT) {
                    Ok(Some(w)) => {
                        // its entries may still be on their way
                        let deadline = std::time::Instant::now() + TIMEOUT;
                        while self.remote.node.state.window(w).is_err() && std::time::Instant::now() < deadline {
                            let _ = self.step(Duration::from_millis(20));
                        }
                        w
                    }
                    _ => return,
                }
            }
        };
        let Some(buffer) = self.remote.node.state.window(w).ok().and_then(|x| x.body_buffer()) else { return };
        let hash = Text::new(&text).content_hash();
        let _ = self.propose(Proposal::SetContent { buffer, version: None, text, hash }, TIMEOUT);
    }

    /// A rule named us: a verb from the menu, or cmd-B3 (`Def`).
    fn on_plumb(&mut self, p: ToolPlumb) {
        if p.verb == "Back" || p.verb == "Fwd" {
            // the session's stack: pop it, and the leader lands there
            match self.propose(Proposal::Nav { back: p.verb == "Back" }, TIMEOUT) {
                Ok(_) => {}
                Err(e) => self.errors(Some(&p.dir), &format!("{}: {e}\n", p.verb)),
            }
            self.remote.plumb_ack(p.id, true);
            return;
        }
        let span = p.at.or(p.sel);
        let Some(span) = span else {
            self.remote.plumb_ack(p.id, false);
            return;
        };
        // A rule can become visible just before the main loop's first document
        // sync. Catch that small startup window rather than declining the verb.
        if !self.docs.contains_key(&span.buffer) {
            self.sync_docs();
        }
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
        let Some(initialized) = self.servers.get(&key).map(|s| s.initialized) else {
            self.remote.plumb_ack(p.id, false);
            return;
        };
        if !initialized {
            self.remote.plumb_ack(p.id, true);
            self.errors(Some(&p.dir), &format!("{}: language server is still initializing\n", p.verb));
            return;
        }
        let s = self.servers.get_mut(&key).unwrap();
        let id = s.request(method, params);
        self.vlog(&format!("{}: {method} for {}", key.0, p.verb));
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
                self.propose(Proposal::ReplaceRange { select: false, dir: Some(dir.to_string()), buffer, version, q0: 0, q1: len, text: new }, TIMEOUT).is_ok()
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
                            let _ = self.propose(Proposal::ReplaceRange { select: false, dir: Some(dir.to_string()), buffer: b, version, q0: 0, q1: len, text: new }, TIMEOUT);
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

    /// Go to `path` at an LSP range: one jump (`Goto`), which records
    /// where the user left from; the leader opens the file if it must.
    /// The range is a selection when the buffer is here to count in, a
    /// line and column otherwise.
    fn open_at(&mut self, path: &Path, range: &Value) -> bool {
        let name = path.display().to_string();
        let open = self.remote.node.state.buffers.values().find(|b| b.name == name).map(|b| b.text.clone());
        let pos = match open {
            Some(text) => {
                let q0 = pos::offset(&text, &range["start"]);
                let q1 = pos::offset(&text, &range["end"]);
                Pos::Chars(q0, q1.max(q0))
            }
            None => Pos::LineCol(range["start"]["line"].as_u64().unwrap_or(0) as usize, range["start"]["character"].as_u64().unwrap_or(0) as usize),
        };
        self.propose(Proposal::Goto { loc: Loc { name, pos } }, TIMEOUT).is_ok()
    }
}

impl Drop for Tool {
    fn drop(&mut self) {
        for (_, mut s) in self.servers.drain() {
            let _ = s.child.kill();
        }
    }
}
