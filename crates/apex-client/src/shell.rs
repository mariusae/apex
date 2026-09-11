//! The application shell around the acme window: what the app does on
//! launch (which sessions to open, starting the daemon), the menu bar and
//! its actions, the title bar with the session button, and the session
//! selector that drops down from it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};


use gpui::{
    actions, anchored, deferred, div, point, prelude::*, px, rgb, size, App, Bounds, Context, KeyBinding, Menu, MenuItem, MouseButton, Pixels, WindowBounds,
    Window,
};

use apex_server::providers::SessionUrl;
use apex_server::proto::SessionInfo;
use apex_server::remote::{list_sessions, new_session};

use crate::app::{Acme, Backend};

actions!(apex, [Quit, HideApp, About, InstallCli, NewFile, NewWindow, CloseWindow, Sessions, PreviousSession, Profile, Tab1, Tab2, Tab3, Tab4, Tab5, Tab6, Tab7, Tab8, Tab9, Goto, NavBack, NavFwd, Reconnect, ToggleFullScreen, Put, Get, Del, Undo, Redo, Cut, Copy, Paste, SelectAll, ThemeLight, ThemeDark, ThemeSystem]);

/// The theme chosen in the View menu: kept, the menus remade with the
/// choice marked, every window redrawn.
pub fn set_theme(m: crate::theme::Mode, cx: &mut App) {
    crate::theme::set_mode(m);
    cx.set_menus(menus());
    // the daemons hear the new colours, for the programs that ask
    for w in cx.windows() {
        if let Some(h) = w.downcast::<crate::app::Acme>() {
            let _ = h.update(cx, |acme, _, _| {
                acme.send_config();
                acme.webs.restyle(); // pages from buffers take the colours
            });
        }
    }
    crate::pool::Pool::send_config(cx);
    cx.refresh_windows();
}

/// Set by the Quit action so closing windows on the way out does not
/// forget which sessions were open.
pub static QUITTING: AtomicBool = AtomicBool::new(false);

pub const TITLEBAR_HEIGHT: f32 = 34.;
/// The top row's background (acme's tag colour): what the selected tab is.
pub const BLINK: std::time::Duration = std::time::Duration::from_millis(500);
/// The system's UI font.
pub const UI_FONT: &str = ".AppleSystemUIFont";

pub fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Apex".into(),
            disabled: false,
            items: vec![
                MenuItem::action("About Apex", About),
                MenuItem::separator(),
                MenuItem::action("Install apex Command…", InstallCli),
                MenuItem::separator(),
                MenuItem::action("Hide Apex", HideApp),
                MenuItem::separator(),
                MenuItem::action("Quit Apex", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                MenuItem::action("New", NewFile),
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Sessions…", Sessions),
                MenuItem::action("Go to…", Goto),
                MenuItem::action("Back", NavBack),
                MenuItem::action("Forward", NavFwd),
                MenuItem::action("Reconnect", Reconnect),
                MenuItem::separator(),
                MenuItem::action("Enter Full Screen", ToggleFullScreen),
                MenuItem::separator(),
                MenuItem::action("Put", Put),
                MenuItem::action("Get", Get),
                MenuItem::action("Del", Del),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", Cut),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
                MenuItem::action("Select All", SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            disabled: false,
            items: {
                let m = crate::theme::mode();
                let mark = |name: &str, mine: crate::theme::Mode| if m == mine { format!("{name} ✓") } else { name.to_string() };
                vec![
                    MenuItem::action(mark("Light", crate::theme::Mode::Light), ThemeLight),
                    MenuItem::action(mark("Dark", crate::theme::Mode::Dark), ThemeDark),
                    MenuItem::action(mark("System", crate::theme::Mode::System), ThemeSystem),
                ]
            },
        },
    ]
}

pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", HideApp, None),
        // acme's commands on the text under the pointer, the Mac way
        KeyBinding::new("cmd-n", NewFile, None),
        KeyBinding::new("cmd-shift-n", NewWindow, None),
        KeyBinding::new("cmd-s", Put, None),
        KeyBinding::new("cmd-w", Del, None),
        KeyBinding::new("cmd-shift-w", CloseWindow, None),
        KeyBinding::new("cmd-k", Sessions, None),
        KeyBinding::new("cmd-shift-k", PreviousSession, None),
        KeyBinding::new("cmd-1", Tab1, None),
        KeyBinding::new("cmd-2", Tab2, None),
        KeyBinding::new("cmd-3", Tab3, None),
        KeyBinding::new("cmd-4", Tab4, None),
        KeyBinding::new("cmd-5", Tab5, None),
        KeyBinding::new("cmd-6", Tab6, None),
        KeyBinding::new("cmd-7", Tab7, None),
        KeyBinding::new("cmd-8", Tab8, None),
        KeyBinding::new("cmd-9", Tab9, None),
        KeyBinding::new("cmd-,", Profile, None),
        KeyBinding::new("cmd-r", Get, None),
        KeyBinding::new("cmd-shift-r", Reconnect, None),
        KeyBinding::new("cmd-p", Goto, None),
        KeyBinding::new("cmd-[", NavBack, None),
        KeyBinding::new("cmd-]", NavFwd, None),
        KeyBinding::new("cmd-ctrl-f", ToggleFullScreen, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
    ]
}

// ---- the login shell's environment ------------------------------------------------------

/// An app launched from the Finder gets LaunchServices' environment, whose
/// PATH is `/usr/bin:/bin:/usr/sbin:/sbin`: no `~/.local/bin`, no
/// providers, and commands run by the daemon we start would miss them
/// too. Ask the user's login shell for its environment and adopt it, as
/// Zed does. Returns what changed.
pub fn adopt_login_shell_environment() -> Vec<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = Command::new(&shell)
        .args(["-l", "-i", "-c", "env -0 2>/dev/null || env"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let entries: Vec<&str> = if text.contains('\0') { text.split('\0').collect() } else { text.lines().collect() };
    let mut changed = Vec::new();
    for e in entries {
        let Some((k, v)) = e.split_once('=') else { continue };
        if k.is_empty() || matches!(k, "_" | "SHLVL" | "PWD" | "OLDPWD" | "TERM" | "TERM_PROGRAM" | "TERM_SESSION_ID") {
            continue;
        }
        if std::env::var(k).ok().as_deref() != Some(v) {
            std::env::set_var(k, v);
            changed.push(k.to_string());
        }
    }
    changed
}

// ---- which sessions to open ----------------------------------------------------------

pub(crate) fn state_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(home).join("Library/Application Support/apex/last-sessions")
}

/// A window as remembered: its session, and where it was on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Remembered {
    pub url: String,
    pub frame: Option<Bounds<Pixels>>,
    pub fullscreen: bool,
}

/// The windows open when the app last ran: one line each, the session's
/// URL and (tab-separated) the frame's x, y, width, height.
pub fn remembered() -> Vec<Remembered> {
    std::fs::read_to_string(state_file())
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let mut f = l.split('\t');
                    let url = f.next().unwrap_or("").to_string();
                    let rest: Vec<&str> = f.collect();
                    let nums: Vec<f32> = rest.iter().take(4).filter_map(|x| x.parse().ok()).collect();
                    let frame = match nums.as_slice() {
                        [x, y, w, h] if *w > 0. && *h > 0. => Some(Bounds { origin: point(px(*x), px(*y)), size: size(px(*w), px(*h)) }),
                        _ => None,
                    };
                    let fullscreen = rest.get(4).is_some_and(|f| *f == "full");
                    Remembered { url, frame, fullscreen }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn remember(windows: &[Remembered]) {
    let p = state_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let text: String = windows
        .iter()
        .map(|r| match r.frame {
            Some(b) => format!("{}\t{}\t{}\t{}\t{}{}\n", r.url, f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height), if r.fullscreen { "\tfull" } else { "" }),
            None => format!("{}\n", r.url),
        })
        .collect();
    let _ = std::fs::write(p, text);
}

/// Record every open window, its session and its frame, unless the app
/// is quitting (then what was recorded is what we want back next time).
pub fn save_open(cx: &mut App) {
    if QUITTING.load(Ordering::Relaxed) {
        return;
    }
    let mut open = Vec::new();
    let debug = std::env::var_os("APEX_DEBUG").is_some();
    for w in cx.windows() {
        let Some(h) = w.downcast::<Acme>() else {
            if debug {
                eprintln!("apex-ui: save_open: a window that is not ours");
            }
            continue;
        };
        let Ok(a) = h.read(cx) else {
            if debug {
                eprintln!("apex-ui: save_open: cannot read a window");
            }
            continue;
        };
        if a.socket.is_none() || a.url.provider == "via" || a.chooser {
            if debug {
                eprintln!("apex-ui: save_open: skipping {} (socket {:?})", a.url, a.socket);
            }
            continue;
        }
        let url = a.url.to_string();
        let (frame, fullscreen) = h.update(cx, |_, window, _| (window.bounds(), window.is_fullscreen())).map(|(b, f)| (Some(b), f)).unwrap_or((None, false));
        open.push(Remembered { url, frame, fullscreen });
    }
    if debug {
        eprintln!("apex-ui: save_open: {} window(s): {:?}", open.len(), open.iter().map(|r| r.url.clone()).collect::<Vec<_>>());
    }
    log_line(&format!("saved {} of {} window(s): {:?}", open.len(), cx.windows().len(), open.iter().map(|r| r.url.clone()).collect::<Vec<_>>()));
    remember(&open);
}

/// A line in the log beside the state file, for when the app ran from
/// the Finder and stderr goes nowhere.
pub fn log_line(what: &str) {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("{stamp} pid {} {what}\n", std::process::id());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(state_file().with_file_name("log")) {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

/// The windows to open at launch: the remembered ones, each on its
/// session and at its frame (a session that is gone, after `apex stop`
/// say, is made again, empty: attaching creates it); else one on the
/// first existing local session; else one on a new `default`.
pub fn plan(socket: &Path) -> std::io::Result<Vec<(SessionUrl, Option<WindowBounds>)>> {
    let existing = list_sessions(socket)?;
    // one window per session: a second one could only fence the first
    let mut seen: Vec<SessionUrl> = Vec::new();
    let again: Vec<(SessionUrl, Option<WindowBounds>)> = remembered()
        .iter()
        .filter_map(|r| SessionUrl::parse(&r.url).map(|u| (u, r.frame.map(|b| if r.fullscreen { WindowBounds::Fullscreen(b) } else { WindowBounds::Windowed(b) }))))
        .filter(|(u, _)| {
            if seen.contains(u) {
                false
            } else {
                seen.push(u.clone());
                true
            }
        })
        .collect();
    if !again.is_empty() {
        return Ok(again);
    }
    if let Some(first) = existing.first() {
        return Ok(vec![(SessionUrl::local(&first.label).with_id(&first.id), None)]);
    }
    new_session(socket, apex_server::providers::DEFAULT_SESSION)?;
    Ok(vec![(SessionUrl::local(apex_server::providers::DEFAULT_SESSION), None)])
}

/// Make sure a daemon answers on `socket`: start one with the `apex`
/// command next to this executable (the app bundle) or on the PATH; as a
/// last resort run one inside this process, which then lives only as
/// long as the app.
pub fn ensure_daemon(socket: &Path) -> std::io::Result<()> {
    if list_sessions(socket).is_ok() {
        return Ok(());
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe.with_file_name("apex"));
    }
    candidates.push(PathBuf::from("apex"));
    let mut last = std::io::Error::other("no apex command");
    for apex in candidates {
        match apex_server::daemon::spawn_server(&apex, socket, apex_server::providers::DEFAULT_SESSION) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

// ---- the apex command ---------------------------------------------------------------

/// Where the `apex` command gets linked.
pub const CLI_LINK: &str = "/usr/local/bin/apex";

/// The `apex` command this app carries: the one beside the executable
/// (in the bundle, `Contents/MacOS/apex`; in a dev tree, `target/release/apex`).
pub fn bundled_cli() -> std::io::Result<PathBuf> {
    let p = std::env::current_exe()?.with_file_name("apex");
    if p.is_file() {
        Ok(p)
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("no apex command at {}", p.display())))
    }
}

/// Put a symlink at `link` pointing at `target`. A symlink, not a copy,
/// so rebuilding the app updates the command. Refuses to replace a real
/// file; replaces a symlink.
pub fn link_cli(target: &Path, link: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(link)?,
        Ok(_) => {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} exists and is not a symlink", link.display())));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(d) = link.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::os::unix::fs::symlink(target, link)
}

/// Install the command, asking for administrator rights through the
/// system dialog when `/usr/local/bin` is not ours to write.
pub fn install_cli() -> Result<String, String> {
    let target = bundled_cli().map_err(|e| e.to_string())?;
    let link = Path::new(CLI_LINK);
    match link_cli(&target, link) {
        Ok(()) => return Ok(format!("{} → {}", link.display(), target.display())),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(e) => return Err(format!("{}: {e}", link.display())),
    }
    let dir = link.parent().unwrap();
    let script = format!(
        "do shell script \"mkdir -p '{}' && ln -sfn '{}' '{}'\" with administrator privileges",
        dir.display(),
        target.display(),
        link.display()
    );
    let out = Command::new("osascript").arg("-e").arg(script).output().map_err(|e| format!("osascript: {e}"))?;
    if out.status.success() {
        Ok(format!("{} → {}", link.display(), target.display()))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-128") {
            Err("cancelled".into())
        } else {
            Err(format!("could not link {}: {}", link.display(), err.trim()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_shells_path_is_adopted() {
        std::env::set_var("PATH", "/usr/bin:/bin");
        let changed = adopt_login_shell_environment();
        let path = std::env::var("PATH").unwrap();
        assert!(changed.iter().any(|k| k == "PATH"), "changed: {changed:?}");
        assert!(path.split(':').count() > 2, "{path}");
    }

    #[test]
    fn links_and_relinks_but_never_clobbers_a_file() {
        let dir = std::env::temp_dir().join(format!("apex-cli-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target_a = dir.join("a/apex");
        let target_b = dir.join("b/apex");
        std::fs::create_dir_all(target_a.parent().unwrap()).unwrap();
        std::fs::create_dir_all(target_b.parent().unwrap()).unwrap();
        std::fs::write(&target_a, "a").unwrap();
        std::fs::write(&target_b, "b").unwrap();
        let link = dir.join("bin/apex");
        link_cli(&target_a, &link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), target_a);
        // a rebuilt app: the link moves
        link_cli(&target_b, &link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), target_b);
        // a real file in the way is left alone
        let file = dir.join("bin/real");
        std::fs::write(&file, "keep").unwrap();
        assert!(link_cli(&target_a, &file).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- the session selector -----------------------------------------------------------

/// The dropdown under the session name, styled after Zed's project
/// picker: a search field, then sections — this window's session, recent
/// ones, this machine's, the destination's — and actions below a
/// divider. Every session is a URL: `local:///name`,
/// `ssh://user@host/name`, `sprite://box/name`.
/// A tab held with B1 (`Acme::tab_drag`): which, whether it is the
/// current one, where the press was, and whether it has moved enough
/// to be a drag rather than a click.
pub struct TabDrag {
    pub url: SessionUrl,
    pub current: bool,
    pub start: gpui::Point<Pixels>,
    /// Where the pointer is now.
    pub pos: gpui::Point<Pixels>,
    /// Where in the tab it was grabbed, from its left edge, and the
    /// tab's width: the floating tab keeps that grip.
    pub grab: Pixels,
    pub width: Pixels,
    pub moved: bool,
}

pub struct Selector {
    pub filter: crate::field::LineEdit,
    /// Index into `rows()`; only pickable rows are ever landed on.
    pub cursor: usize,
    /// The user has moved the cursor or typed: it stays on its row as the
    /// hosts answer, rather than landing on this window's session.
    pub moved: bool,
    /// The hosts remembered, local first: what the picker is organised by.
    pub hosts: Vec<Host>,
    /// Each host's sessions, as they come in (asked for in the background:
    /// a host that is down must not hold the picker).
    pub sessions: HashMap<Host, Loading>,
    pub current: SessionUrl,
    /// Typing a new name for this window's session.
    pub renaming: bool,
    /// Typing the name of a new session on this host.
    pub naming: Option<Host>,
    /// The new-host form: a provider, a host.
    pub connect: Option<Connect>,
    /// Which opening of the picker this is: answers for an older one are
    /// dropped.
    pub epoch: u64,
    /// When the caret last became visible: it blinks, and a keystroke
    /// makes it show at once.
    pub caret_since: std::time::Instant,
}

/// A place sessions live: this machine's daemon (`local`), or a
/// destination through a provider.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Host {
    pub provider: String,
    pub arg: String,
}

impl Host {
    pub fn local() -> Host {
        Host { provider: "local".into(), arg: String::new() }
    }

    pub fn of(u: &SessionUrl) -> Host {
        Host { provider: u.provider.clone(), arg: if u.is_local() { String::new() } else { u.arg.clone() } }
    }

    pub fn is_local(&self) -> bool {
        self.provider == "local"
    }

    /// The URL of one of this host's sessions, identity and all.
    pub fn url_of(&self, s: &SessionInfo) -> SessionUrl {
        self.url(&s.label).with_id(&s.id)
    }

    pub fn url(&self, session: &str) -> SessionUrl {
        SessionUrl { provider: self.provider.clone(), arg: self.arg.clone(), session: session.to_string(), id: None }
    }

    /// How the host reads: `local`, or the host with the provider in
    /// parentheses.
    pub fn parts(&self) -> (String, String) {
        if self.is_local() { ("local".into(), String::new()) } else { (self.arg.clone(), format!("({})", self.provider)) }
    }

    /// The destination `providers.rs` works with.
    pub fn dest(&self) -> String {
        self.url("default").dest().unwrap_or_default()
    }
}

/// A host's sessions: as last seen while the host is asked (they are
/// mostly the same), here, or not to be had (the last seen still shown).
#[derive(Clone, Debug, PartialEq)]
pub enum Loading {
    Seeded(Vec<SessionInfo>),
    Ready(Vec<SessionInfo>),
    Failed(Vec<SessionInfo>, String),
}

impl Loading {
    pub fn sessions(&self) -> &[SessionInfo] {
        match self {
            Loading::Seeded(n) | Loading::Ready(n) | Loading::Failed(n, _) => n,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A host's section: its name, and the host, to forget it by.
    Header(Host),
    Divider,
    /// A session on a host.
    Open(SessionUrl),
    /// "+ new session" under a host: type its name next.
    NewSession(Host),
    /// Still asking, or could not.
    Note(String),
    /// A session named to be made: on a host after `NewSession`, or as
    /// a URL typed into the search.
    Create(SessionUrl),
    /// "+ new host…": the form next.
    NewHost,
    /// "Rename this session…": type the new name next.
    RenameThis,
    Rename(String),
}

/// The new-host form's fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    Provider,
    Host,
}

/// A new host: a provider chosen from what is available (`local`, `ssh`,
/// every `apex-remote-NAME` on the PATH), and the host it takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Connect {
    pub providers: Vec<String>,
    pub provider: usize,
    pub host: crate::field::LineEdit,
    pub field: Field,
}

impl Connect {
    pub fn new() -> Connect {
        let providers: Vec<String> = apex_server::providers::available().into_iter().filter(|p| p != "local").collect();
        Connect { providers, provider: 0, host: crate::field::LineEdit::new(), field: Field::Provider }
    }

    pub fn provider(&self) -> &str {
        self.providers.get(self.provider).map(String::as_str).unwrap_or("ssh")
    }

    pub fn next_field(&mut self, delta: i32) {
        self.field = if delta < 0 { Field::Provider } else { Field::Host };
    }

    pub fn next_provider(&mut self, delta: i32) {
        let n = self.providers.len() as i32;
        if n > 0 {
            self.provider = ((self.provider as i32 + delta).rem_euclid(n)) as usize;
        }
    }

    /// The host the form names, once it names one.
    pub fn host(&self) -> Option<Host> {
        let host = self.host.trim();
        if host.is_empty() || host.contains('/') {
            return None;
        }
        Some(Host { provider: self.provider().to_string(), arg: host.to_string() })
    }
}

// ---- known hosts ---------------------------------------------------------------------

fn hosts_file() -> PathBuf {
    state_file().with_file_name("known-hosts")
}

/// The hosts the picker shows: local, then those connected to, latest
/// first (the file, then what the recent sessions carry).
pub fn known_hosts() -> Vec<Host> {
    let mut out = vec![Host::local()];
    let listed: Vec<Host> = std::fs::read_to_string(hosts_file())
        .map(|s| s.lines().filter_map(|l| l.split_once('\t').map(|(p, h)| Host { provider: p.to_string(), arg: h.to_string() })).collect())
        .unwrap_or_default();
    for h in listed.into_iter().chain(recent().iter().map(Host::of)) {
        if !h.is_local() && !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

fn write_hosts(list: &[Host]) {
    let p = hosts_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let text: String = list.iter().filter(|h| !h.is_local()).take(50).map(|h| format!("{}\t{}\n", h.provider, h.arg)).collect();
    let _ = std::fs::write(p, text);
}

/// A host connected to: remembered first.
pub fn note_host(url: &SessionUrl) {
    let h = Host::of(url);
    if h.is_local() {
        return;
    }
    let mut list = known_hosts();
    list.retain(|x| *x != h);
    list.insert(1, h);
    write_hosts(&list);
}

fn sessions_file() -> PathBuf {
    state_file().with_file_name("known-sessions")
}

/// The sessions each host had when last asked (the file, a line per
/// session: provider, host, id, label), plus what the recent sessions
/// say: what the picker shows before a host answers.
pub fn known_sessions() -> HashMap<Host, Vec<SessionInfo>> {
    let mut out: HashMap<Host, Vec<SessionInfo>> = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(sessions_file()) {
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if let [p, h, id, label] = f.as_slice() {
                let host = Host { provider: p.to_string(), arg: h.to_string() };
                out.entry(host).or_default().push(SessionInfo { id: id.to_string(), label: label.to_string() });
            }
        }
    }
    for u in recent() {
        let h = Host::of(&u);
        let e = out.entry(h.clone()).or_default();
        if !e.iter().any(|s| h.url_of(s) == u) {
            e.push(SessionInfo { id: u.id.clone().unwrap_or_default(), label: u.session.clone() });
        }
    }
    out
}

/// A host answered: its sessions are what the picker shows for it next time.
pub fn note_sessions(h: &Host, sessions: &[SessionInfo]) {
    let mut all = known_sessions();
    all.insert(h.clone(), sessions.to_vec());
    let p = sessions_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut lines: Vec<String> = all.iter().flat_map(|(h, v)| v.iter().map(move |s| format!("{}\t{}\t{}\t{}", h.provider, h.arg, s.id, s.label))).collect();
    lines.sort();
    let _ = std::fs::write(p, lines.join("\n") + "\n");
}

/// A host forgotten (the × on its section): its sessions leave the
/// recent list too, so it does not come back from there.
pub fn forget_host(h: &Host) {
    let list: Vec<Host> = known_hosts().into_iter().filter(|x| x != h).collect();
    write_hosts(&list);
    let keep: Vec<SessionUrl> = recent().into_iter().filter(|u| Host::of(u) != *h).collect();
    write_recent(&keep);
}

/// How a session reads in the picker: its label first, then the host,
/// then the provider in parentheses (the host and provider dimmed).
pub fn session_parts(u: &SessionUrl) -> (String, String, String) {
    (u.session.clone(), u.arg.clone(), format!("({})", u.provider))
}

impl Row {
    pub fn pickable(&self) -> bool {
        !matches!(self, Row::Header(_) | Row::Divider | Row::Note(_))
    }
}

impl Selector {
    pub fn caret_visible(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / BLINK.as_millis()) % 2 == 0
    }

    pub fn rows(&self) -> Vec<Row> {
        let f = self.filter.trim();
        if self.connect.is_some() {
            return Vec::new(); // the form is the panel then
        }
        if self.renaming {
            return match (f.is_empty(), apex_server::providers::valid_label(f)) {
                (true, _) => Vec::new(),
                (false, Ok(())) => vec![Row::Rename(f.to_string())],
                (false, Err(_)) => vec![Row::Note("a label: a letter first, then lowercase letters, digits and -".into())],
            };
        }
        if let Some(h) = &self.naming {
            return match (f.is_empty(), apex_server::providers::valid_label(f)) {
                (true, _) => Vec::new(),
                (false, Ok(())) => vec![Row::Create(h.url(f))],
                (false, Err(_)) => vec![Row::Note("a label: a letter first, then lowercase letters, digits and -".into())],
            };
        }
        let fl = f.to_lowercase();
        let mut rows = Vec::new();
        let mut seen: Vec<SessionUrl> = Vec::new();
        for h in &self.hosts {
            let (name, prov) = h.parts();
            let host_matches = fl.is_empty() || format!("{name} {prov}").to_lowercase().contains(&fl);
            let mut section = Vec::new();
            let loading = self.sessions.get(h);
            for n in loading.map(|l| l.sessions()).unwrap_or(&[]) {
                let u = h.url_of(n);
                if (host_matches || u.to_string().to_lowercase().contains(&fl)) && !seen.contains(&u) {
                    seen.push(u.clone());
                    section.push(Row::Open(u));
                }
            }
            if host_matches {
                match loading {
                    Some(Loading::Failed(_, e)) => section.push(Row::Note(format!("unreachable: {e}"))),
                    Some(Loading::Ready(_)) => {}
                    _ => section.push(Row::Note("asking…".into())),
                }
                section.push(Row::NewSession(h.clone()));
            }
            if !section.is_empty() {
                rows.push(Row::Header(h.clone()));
                rows.extend(section);
            }
        }
        let mut actions = Vec::new();
        if !f.is_empty() {
            if let Some(u) = SessionUrl::parse(f) {
                if !seen.contains(&u) && apex_server::providers::valid_label(&u.session).is_ok() {
                    actions.push(Row::Create(u));
                }
            }
        } else {
            actions.push(Row::NewHost);
            actions.push(Row::RenameThis);
        }
        if !actions.is_empty() {
            if !rows.is_empty() {
                rows.push(Row::Divider);
            }
            rows.extend(actions);
        }
        rows
    }

    /// The first pickable row at or after `from`.
    fn pickable_from(&self, rows: &[Row], from: usize) -> Option<usize> {
        (from..rows.len()).find(|&i| rows[i].pickable())
    }

    /// Change the list and keep the cursor on the row it was on, wherever
    /// that row is now (a host answering fills its section in above).
    pub fn keeping(&mut self, change: impl FnOnce(&mut Selector)) {
        let under = self.rows().get(self.cursor).cloned();
        change(self);
        let rows = self.rows();
        match under.and_then(|u| rows.iter().position(|r| *r == u)) {
            Some(i) => self.cursor = i,
            None => self.settle(),
        }
    }

    pub fn move_cursor(&mut self, delta: i32) {
        self.moved = true;
        let rows = self.rows();
        let mut i = self.cursor as i32;
        loop {
            i += delta;
            if i < 0 || i as usize >= rows.len() {
                return;
            }
            if rows[i as usize].pickable() {
                self.cursor = i as usize;
                return;
            }
        }
    }

    pub fn settle(&mut self) {
        let rows = self.rows();
        self.cursor = self.pickable_from(&rows, self.cursor.min(rows.len().saturating_sub(1))).or_else(|| self.pickable_from(&rows, 0)).unwrap_or(0);
    }

    /// Put the cursor on this window's session, when it is listed.
    pub fn land_on_current(&mut self) {
        let rows = self.rows();
        if let Some(i) = rows.iter().position(|r| matches!(r, Row::Open(u) if *u == self.current)) {
            self.cursor = i;
        } else {
            self.settle();
        }
    }
}

// ---- recent sessions ----------------------------------------------------------------

fn recent_file() -> PathBuf {
    state_file().with_file_name("recent-sessions")
}

/// The sessions attached to lately, latest first.
pub fn recent() -> Vec<SessionUrl> {
    std::fs::read_to_string(recent_file()).map(|s| s.lines().filter_map(SessionUrl::parse).collect()).unwrap_or_default()
}

fn write_recent(list: &[SessionUrl]) {
    let p = recent_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let text: String = list.iter().take(12).map(|u| format!("{u}\n")).collect();
    let _ = std::fs::write(p, text);
}

pub fn note_recent(url: &SessionUrl) {
    let mut list = recent();
    list.retain(|u| u != url);
    list.insert(0, url.clone());
    write_recent(&list);
}

pub fn renamed_recent(old: &SessionUrl, new: &SessionUrl) {
    let list: Vec<SessionUrl> = recent().into_iter().map(|u| if u == *old { new.clone() } else { u }).collect();
    write_recent(&list);
    let last: Vec<Remembered> = remembered().into_iter().map(|mut r| {
        if SessionUrl::parse(&r.url).is_some_and(|u| u == *old) {
            r.url = new.to_string();
        }
        r
    }).collect();
    remember(&last);
}

impl Acme {
    pub fn open_selector(&mut self, cx: &mut Context<Self>) {
        let Some(socket) = self.socket.clone() else { return };
        static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
        let mut hosts = known_hosts();
        let here = Host::of(&self.url);
        if !hosts.contains(&here) {
            hosts.push(here);
        }
        let mut sel = Selector { filter: crate::field::LineEdit::new(), cursor: 0, moved: false, hosts: hosts.clone(), sessions: HashMap::new(), current: self.url.clone(), renaming: false, naming: None, connect: None, epoch, caret_since: std::time::Instant::now() };
        // what each host had last time, shown at once; the answers update it
        let mut known = known_sessions();
        for h in &hosts {
            let mut names = known.remove(h).unwrap_or_default();
            if *h == Host::of(&self.url) && !names.iter().any(|s| h.url_of(s) == self.url) {
                names.push(SessionInfo { id: self.url.id.clone().unwrap_or_default(), label: self.url.session.clone() });
            }
            sel.sessions.insert(h.clone(), Loading::Seeded(names));
        }
        sel.land_on_current();
        self.selector = Some(sel);
        // every host's sessions, asked for in the background: a host that
        // is down, or slow, holds nothing up
        for h in hosts {
            self.ask_host(h, socket.clone(), epoch, cx);
        }
        // blink the caret while the selector is open
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BLINK).await;
            let open = cx.update(|cx| this.update(cx, |acme, cx| {
                let open = acme.selector.is_some();
                if open {
                    cx.notify();
                }
                open
            }).unwrap_or(false));
            if !open {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    /// Ask a host for its sessions, off the UI thread; the answer lands
    /// in the picker of the same opening.
    fn ask_host(&mut self, h: Host, socket: PathBuf, epoch: u64, cx: &mut Context<Self>) {
        let host = h.clone();
        let asking = cx.background_executor().spawn(async move {
            if host.is_local() {
                return list_sessions(&socket).map_err(|e| e.to_string());
            }
            // a host that has no apex yet (just added) gets ours first, as
            // attaching would, and is asked again
            let dest = host.dest();
            match apex_server::providers::list_sessions(&dest) {
                Ok(names) => Ok(names),
                Err(first) => match apex_server::providers::deploy(&dest) {
                    Ok(_) => apex_server::providers::list_sessions(&dest).map_err(|e| e.to_string()),
                    Err(_) => Err(first.to_string()),
                },
            }
        });
        cx.spawn(async move |this, cx| {
            let r = asking.await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |acme, cx| {
                    if let Some(sel) = acme.selector.as_mut() {
                        if sel.epoch == epoch {
                            let before = sel.sessions.get(&h).map(|l| l.sessions().to_vec()).unwrap_or_default();
                            let loaded = match r {
                                Ok(names) => {
                                    note_sessions(&h, &names);
                                    Loading::Ready(names)
                                }
                                Err(e) => Loading::Failed(before, e),
                            };
                            // the cursor stays on its row; untouched, it
                            // lands on this window's session once listed
                            sel.keeping(|sel| {
                                sel.sessions.insert(h.clone(), loaded);
                            });
                            if !sel.moved {
                                sel.land_on_current();
                            }
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
    }

    /// The field that has the keyboard while an overlay is up: the
    /// new-host form's host, the picker's search, the finder's.
    fn overlay_field(&mut self) -> Option<&mut crate::field::LineEdit> {
        if let Some(sel) = self.selector.as_mut() {
            sel.caret_since = std::time::Instant::now();
            return match sel.connect.as_mut() {
                Some(form) => (form.field == Field::Host).then_some(&mut form.host),
                None => Some(&mut sel.filter),
            };
        }
        if let Some(f) = self.finder.as_mut() {
            f.caret_since = std::time::Instant::now();
            return Some(&mut f.filter);
        }
        None
    }

    /// The overlay's field changed: the list is filtered anew from the top.
    fn overlay_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(sel) = self.selector.as_mut() {
            if sel.connect.is_none() {
                sel.moved = true;
                sel.cursor = 0;
                sel.settle();
            }
        } else if let Some(f) = self.finder.as_mut() {
            f.cursor = 0;
        }
        cx.notify();
    }

    /// The Edit menu (its keys) with an overlay up: cut, copy, paste
    /// (the text's first line), select all and undo on its field.
    pub fn overlay_edit(&mut self, what: &str, cx: &mut Context<Self>) {
        let clip = cx.read_from_clipboard().and_then(|c| c.text());
        let Some(field) = self.overlay_field() else { return };
        let mut copied = None;
        let changed = match what {
            "paste" => match clip.as_deref().and_then(|t| t.lines().next()).map(str::trim).filter(|l| !l.is_empty()) {
                Some(line) => {
                    field.insert(line);
                    true
                }
                None => false,
            },
            "select-all" => {
                field.select_all();
                false
            }
            "copy" => {
                copied = field.selected();
                false
            }
            "cut" => {
                copied = field.cut();
                copied.is_some()
            }
            "undo" => field.undo(),
            _ => false,
        };
        if let Some(text) = copied {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
        if changed {
            self.overlay_changed(cx);
        } else {
            cx.notify();
        }
    }

    pub fn close_selector(&mut self, cx: &mut Context<Self>) {
        self.selector = None;
        if self.chooser {
            // a new window that never got a session: nothing to show
            self.close_requested = true;
        }
        cx.notify();
    }

    /// Keys while the selector is open. Returns true if it took the key.
    pub fn selector_key(&mut self, key: &str, ch: Option<&str>, mods: &gpui::Modifiers, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(sel) = self.selector.as_mut() else { return false };
        sel.caret_since = std::time::Instant::now();
        if let Some(form) = sel.connect.as_mut() {
            // the new-host form: ↑↓ between the two fields, ←→ (or a
            // letter) through the providers, enter adds the host, esc backs out
            match key {
                "escape" => {
                    sel.connect = None;
                    sel.settle();
                }
                "enter" => {
                    if let Some(h) = form.host() {
                        self.add_host(h, cx);
                        return true;
                    }
                }
                "up" => form.next_field(-1),
                "down" | "tab" => form.next_field(1),
                "left" if form.field == Field::Provider => form.next_provider(-1),
                "right" | "space" if form.field == Field::Provider => form.next_provider(1),
                _ => match form.field {
                    Field::Host => {
                        form.host.key(key, ch, mods);
                    }
                    Field::Provider => {
                        if let Some(c) = ch.filter(|c| !c.chars().any(char::is_control)) {
                            let lc = c.to_lowercase();
                            if let Some(i) = form.providers.iter().position(|p| p.to_lowercase().starts_with(&lc)) {
                                form.provider = i;
                            }
                        }
                    }
                },
            }
            cx.notify();
            return true;
        }
        match key {
            "escape" => {
                if sel.naming.is_some() || sel.renaming {
                    // back to the list
                    sel.naming = None;
                    sel.renaming = false;
                    sel.filter.clear();
                    sel.land_on_current();
                    cx.notify();
                } else {
                    self.close_selector(cx);
                }
            }
            "enter" => {
                let rows = sel.rows();
                if let Some(row) = rows.get(sel.cursor).filter(|r| r.pickable()).cloned() {
                    self.choose(row, window, cx);
                }
            }
            "up" => {
                sel.move_cursor(-1);
                cx.notify();
            }
            "down" => {
                sel.move_cursor(1);
                cx.notify();
            }
            _ => match sel.filter.key(key, ch, mods) {
                crate::field::Edited::Changed => self.overlay_changed(cx),
                crate::field::Edited::Moved => cx.notify(),
                crate::field::Edited::No => {}
            },
        }
        true
    }

    /// End a session from the picker: on this machine's daemon, or on the
    /// host through `apex end-session` there; off the UI thread, the
    /// host asked again after, a refusal shown in its section.
    fn end_session_from_picker(&mut self, url: SessionUrl, cx: &mut Context<Self>) {
        let Some(socket) = self.socket.clone() else { return };
        let Some(epoch) = self.selector.as_ref().map(|s| s.epoch) else { return };
        let host = Host::of(&url);
        let u = url.clone();
        let ending = cx.background_executor().spawn(async move {
            if u.is_local() {
                apex_server::remote::end_session(&socket, u.session_ref(), false).map_err(|e| e.to_string())
            } else {
                let dest = u.dest().unwrap_or_default();
                apex_server::providers::run(&dest, &format!("{} end-session {}", apex_server::providers::REMOTE_BIN, u.session_ref()), None).map(|_| ()).map_err(|e| e.to_string())
            }
        });
        let socket = self.socket.clone().unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let r = ending.await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |acme, cx| {
                    let Some(sel) = acme.selector.as_mut() else { return };
                    if sel.epoch != epoch {
                        return;
                    }
                    match r {
                        Ok(()) => {
                            // gone from what the host had; the host is asked again
                            sel.keeping(|sel| {
                                if let Some(l) = sel.sessions.get_mut(&host) {
                                    let names: Vec<SessionInfo> = l.sessions().iter().filter(|s| host.url_of(s) != url).cloned().collect();
                                    *l = Loading::Seeded(names);
                                }
                            });
                            acme.ask_host(host, socket, epoch, cx);
                        }
                        Err(e) => {
                            let names = sel.sessions.get(&host).map(|l| l.sessions().to_vec()).unwrap_or_default();
                            sel.keeping(|sel| {
                                sel.sessions.insert(host.clone(), Loading::Failed(names, e));
                            });
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// A host added from the form: remembered, listed, its sessions asked
    /// for, the cursor on it.
    fn add_host(&mut self, h: Host, cx: &mut Context<Self>) {
        let Some(socket) = self.socket.clone() else { return };
        note_host(&h.url("default"));
        let epoch = match self.selector.as_mut() {
            Some(sel) => {
                sel.connect = None;
                if !sel.hosts.contains(&h) {
                    sel.hosts.push(h.clone());
                }
                sel.sessions.insert(h.clone(), Loading::Seeded(Vec::new()));
                let rows = sel.rows();
                sel.cursor = rows.iter().position(|r| matches!(r, Row::NewSession(x) if *x == h)).unwrap_or(0);
                sel.settle();
                sel.epoch
            }
            None => return,
        };
        self.ask_host(h, socket, epoch, cx);
        cx.notify();
    }

    pub fn choose(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            Row::Header(_) | Row::Divider | Row::Note(_) => {}
            Row::NewHost => {
                if let Some(sel) = self.selector.as_mut() {
                    sel.connect = Some(Connect::new());
                    sel.filter.clear();
                }
                cx.notify();
            }
            Row::NewSession(h) => {
                if let Some(sel) = self.selector.as_mut() {
                    sel.naming = Some(h);
                    sel.filter.clear();
                    sel.cursor = 0;
                }
                cx.notify();
            }
            Row::RenameThis => {
                if let Some(sel) = self.selector.as_mut() {
                    sel.renaming = true;
                    sel.filter.clear();
                    sel.cursor = 0;
                }
                cx.notify();
            }
            Row::Rename(to) => {
                self.selector = None;
                self.rename_session(&to, window);
                cx.defer(|cx| save_open(cx)); // after this window's update, so it is read too
                cx.notify();
            }
            Row::Open(url) | Row::Create(url) => {
                self.selector = None;
                note_host(&url);
                if !self.chooser && url == self.url {
                    cx.notify();
                    return;
                }
                // a window already on that session: go there instead
                let me = window.window_handle().window_id();
                let elsewhere = cx.windows().into_iter().filter_map(|w| w.downcast::<Acme>()).find(|h| {
                    h.window_id() != me && h.read(cx).ok().is_some_and(|a| a.url == url)
                });
                if let Some(h) = elsewhere {
                    let _ = h.update(cx, |_, window, _| window.activate_window());
                    if self.chooser {
                        // the new window has no reason to be: that one shows it
                        self.close_requested = true;
                    }
                    cx.notify();
                    return;
                }
                self.switch_to(&url, window, cx);
                cx.defer(|cx| save_open(cx)); // after this window's update, so it is read too
                cx.notify();
            }
        }
    }

    /// The new-host form: the providers as a row of pills (the chosen one
    /// marked), then the host field with the caret.
    fn connect_form(&self, form: &Connect, caret_on: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = crate::theme::theme();
        let row = |label: &str, active: bool| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .px(px(14.))
                .py(px(8.))
                .text_size(px(14.))
                .font_family(UI_FONT)
                .when(active, |d| d.bg(rgb(t.tab_bg)))
                .child(div().w(px(80.)).text_color(rgb(t.panel_dim)).text_size(px(12.)).child(label.to_string()))
        };
        let field = |value: &crate::field::LineEdit, hint: &str, active: bool| crate::field::field_view(value, caret_on, hint, active);
        let mut pills = div().flex().flex_row().items_center().gap(px(6.));
        for (i, p) in form.providers.iter().enumerate() {
            let chosen = i == form.provider;
            pills = pills.child(
                div()
                    .id(("provider", i))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(t.panel_border))
                    .when(chosen, |d| d.bg(rgb(t.panel_chosen_bg)).text_color(rgb(t.panel_chosen_text)).border_color(rgb(t.panel_chosen_bg)))
                    .when(!chosen, |d| d.text_color(rgb(t.panel_text)).hover(|s| s.bg(rgb(t.panel_hover))))
                    .cursor_pointer()
                    .child(p.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if let Some(f) = this.selector.as_mut().and_then(|s| s.connect.as_mut()) {
                                f.provider = i;
                                f.field = Field::Host;
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            );
        }
        let mut el = div().flex().flex_col().py(px(6.));
        el = el.child(div().px(px(14.)).pt(px(6.)).pb(px(2.)).text_size(px(12.)).font_family(UI_FONT).text_color(rgb(t.panel_dim)).child("New host"));
        el = el.child(row("Provider", form.field == Field::Provider).child(pills));
        el = el.child(row("Host", form.field == Field::Host).child(field(&form.host, "user@host, a box name…", form.field == Field::Host)));
        let ready = form.host().is_some();
        let hint = if ready { "enter adds it and asks for its sessions  ·  ←→ providers  ·  esc back" } else { "type the host  ·  ←→ providers  ·  esc back" };
        el = el.child(div().px(px(14.)).pt(px(6.)).pb(px(4.)).text_size(px(12.)).font_family(UI_FONT).text_color(rgb(t.panel_dim)).child(hint));
        el.into_any_element()
    }

    /// The strip at the top: traffic lights live in its left margin; the
    /// session URL is a button, a tinted pill as in Zed.
    pub fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = match &self.backend {
            Backend::Local(_) => "in-process".to_string(),
            Backend::Remote(_) if self.fenced() => format!("{}  ·  fenced", self.url.describe()),
            Backend::Remote(_) => self.url.describe(),
        };
        let clickable = self.socket.is_some();
        let open = self.selector.is_some();
        // a tab per connected session (this one, and the parked ones), in
        // the order first shown; this one is the selected tab and toggles
        // the picker; another switches to it; its × lets a parked one go
        // the tabs sit on the strip's bottom edge; the selected one is the
        // colour of the row below it, rounded at the top, and its bottom
        // corners drape out into the strip (a square of its colour with
        // the strip's colour rounded away), so it flows into the window
        // the selected tab nearly fills the bar; its text sits on the
        // bar's centre line, with the other tabs' and the + beside it
        const TAB_H: f32 = 30.;
        const INSET: f32 = 4.;
        // one line box for every piece of text in a tab, whatever its
        // size, so centring them centres them on the same line
        const LINE: f32 = 18.;
        const DRAPE: f32 = 10.;
        let t = crate::theme::theme();
        let strip: u32 = t.strip;
        let mut tabs = div().id("tabs").h_full().flex().flex_row().items_end();
        let all = crate::pool::Pool::tabs(cx, &self.url);
        // a tab being dragged floats under the pointer, kept within the
        // strip's tabs (their bounds of last frame say where that is)
        let dragging = self.tab_drag.as_ref().filter(|d| d.moved).map(|d| d.url.clone());
        let ghost: Option<(SessionUrl, Pixels, Pixels)> = self.tab_drag.as_ref().filter(|d| d.moved).and_then(|d| {
            let bounds = self.tab_bounds.borrow();
            let mine = bounds.iter().find(|(u, _)| *u == d.url).map(|(_, b)| *b)?;
            let left = bounds.iter().map(|(_, b)| b.origin.x).fold(mine.origin.x, |a, x| if x < a { x } else { a });
            let right = bounds.iter().map(|(_, b)| b.origin.x + b.size.width).fold(mine.origin.x + mine.size.width, |a, x| if x > a { x } else { a });
            let mut x = d.pos.x - d.grab;
            if x > right - d.width {
                x = right - d.width;
            }
            if x < left {
                x = left;
            }
            // the face carries its own margin: the bounds are the face's
            Some((d.url.clone(), x - px(2.), mine.origin.y))
        });
        let mut floating: Option<gpui::Div> = None;
        // where each tab lands this frame, for a drag to reorder by
        self.tab_bounds.borrow_mut().clear();
        let others = all.len() > 1;
        for (i, u) in all.into_iter().enumerate() {
            let current = u == self.url;
            // the label; the host dimmed after it for a session elsewhere
            let text = if current && matches!(self.backend, Backend::Local(_)) { label.clone() } else { u.session.clone() };
            let host = (!u.is_local()).then(|| u.arg.clone());
            let fenced = current && self.fenced();
            let bg = if open { t.tab_open_bg } else { t.tab_bg };
            let closable = clickable && (!current || others);
            let drape = |left: bool| {
                let corner = div().size_full().bg(rgb(strip));
                let corner = if left { corner.rounded_br(px(DRAPE)) } else { corner.rounded_bl(px(DRAPE)) };
                let d = div().absolute().bottom(px(0.)).w(px(DRAPE)).h(px(DRAPE)).bg(rgb(bg)).child(corner);
                if left { d.left(px(-DRAPE)) } else { d.right(px(-DRAPE)) }
            };
            // the tab's face: its look and its words, made twice for a
            // tab being dragged (the placeholder in the row, the one
            // under the pointer)
            let face = |ghost: bool| {
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.))
                    .px(px(10.))
                    .mx(px(2.))
                    .text_size(px(13.))
                    .line_height(px(LINE))
                    .font_family(UI_FONT)
                    .when(current, |d| d.h(px(TAB_H)).pb(px(INSET)).rounded_t(px(DRAPE)).text_color(rgb(t.tab_current_text)).bg(rgb(bg)).child(drape(true)).child(drape(false)))
                    // fenced (another client leads, nothing here takes): the
                    // whole tab fades into the strip, its name greyed, and says so
                    .when(fenced, |d| d.opacity(0.4).text_color(rgb(t.tab_fenced_text)))
                    // the other tabs: the same centre line as the selected one,
                    // so the text stays put as the selection moves; hovered, a
                    // rounded rectangle, as a browser's (only the selected tab
                    // drapes); the one dragged shows as hovered
                    .when(!current, |d| d.h(px(TAB_H - INSET)).mb(px(INSET)).rounded(px(6.)).text_color(rgb(t.tab_text)).hover(|s| s.bg(rgb(t.tab_hover))))
                    .when(!current && ghost, |d| d.bg(rgb(t.tab_hover)))
                    .child(text.clone())
                    .when_some(host.clone(), |d, h| d.child(div().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(t.tab_dim)).child(h)))
                    .when(fenced, |d| d.child(div().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(t.tab_fenced_text)).child("fenced")))
                    .when(closable && ghost, |d| d.child(div().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(t.tab_dim)).child("×")))
            };
            if let Some((_, x, y)) = ghost.as_ref().filter(|(g, _, _)| *g == u) {
                // the tab under the pointer, over everything in the strip
                floating = Some(div().absolute().left(*x).top(*y).child(face(true)));
            }
            let bounds = self.tab_bounds.clone();
            let bounds_url = u.clone();
            let mut tab = face(false)
                .id(("tab", i))
                .child(div().absolute().top(px(0.)).left(px(0.)).size_full().child(gpui::canvas(
                    move |b, _, _| bounds.borrow_mut().push((bounds_url, b)),
                    |_, _, _, _| {},
                )))
                // dragged: its place in the row is kept, empty, as the
                // tabs around it slide; the tab itself is the floating one
                .when(dragging.as_ref() == Some(&u), |d| d.opacity(0.));
            if clickable {
                let url = u.clone();
                // held: a click on release unless it moved, a drag
                // reordering the tabs if it did (`mouse_move`, `mouse_up`)
                tab = tab.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        let (grab, width) = this
                            .tab_bounds
                            .borrow()
                            .iter()
                            .find(|(u, _)| *u == url)
                            .map(|(_, b)| (e.position.x - b.origin.x, b.size.width))
                            .unwrap_or((px(0.), px(80.)));
                        this.tab_drag = Some(TabDrag { url: url.clone(), current, start: e.position, pos: e.position, grab, width, moved: false });
                        cx.stop_propagation();
                    }),
                );
                // ×: a parked session let go; the current one let go
                // too, the window moving to the one parked last
                if closable {
                    let url = u.clone();
                    tab = tab.child(
                        div()
                            .id(("tab-close", i))
                            .text_size(px(11.))
                            .line_height(px(LINE))
                            .text_color(rgb(t.tab_dim))
                            .hover(|s| s.text_color(rgb(t.tab_close_hover)))
                            .child("×")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    if current {
                                        this.close_current_session(window, cx);
                                    } else {
                                        crate::pool::Pool::let_go(cx, &url);
                                    }
                                    cx.notify();
                                    cx.stop_propagation();
                                }),
                            ),
                    );
                }
            }
            tabs = tabs.child(tab);
        }
        // and one more: the picker, for a session not here yet
        // the +, sized as the × and on the same line as they are
        let mut plus = div().id("tab-new").h(px(TAB_H - INSET)).mb(px(INSET)).px(px(7.)).flex().items_center().rounded(px(6.)).text_size(px(11.)).line_height(px(LINE)).font_family(UI_FONT).text_color(rgb(0x8a8a8a)).child("+");
        if clickable {
            plus = plus.cursor_pointer().hover(|s| s.bg(rgb(t.tab_hover)).text_color(rgb(t.tab_current_text))).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selector.is_some() {
                        this.close_selector(cx);
                    } else {
                        this.open_selector(cx);
                    }
                    cx.stop_propagation();
                }),
            );
        }
        let button = tabs.child(plus);
        div()
            .id("titlebar")
            .relative()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .pl(px(78.))
            .bg(rgb(strip))
            .gap(px(6.))
            // no line under the strip: its grey meets the row below, and
            // the selected tab runs straight into it
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                    if this.selector.is_some() {
                        this.close_selector(cx);
                    }
                    if e.click_count >= 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                    cx.stop_propagation();
                }),
            )
            .child(button)
            .child(div().flex_1())
            .when_some(floating, |d, f| d.child(f))
            .when_some(self.latency(), |d, l| d.child(div().pr(px(10.)).text_size(px(11.)).font_family(UI_FONT).text_color(rgb(t.tab_dim)).child(l)))
            // the link to the daemon, at the right: bright while it is up, faded when gone
            .child(div().pr(px(12.)).text_size(px(13.)).opacity(if self.connected { 1.0 } else { 0.25 }).child("⚡"))
    }

    /// The dropdown, when open.
    pub fn selector_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let sel = self.selector.as_ref()?;
        let rows = sel.rows();
        let hint = if sel.renaming {
            "New name for this session…".to_string()
        } else if let Some(h) = &sel.naming {
            let (name, prov) = h.parts();
            format!("Name for the new session on {name} {prov}…")
        } else {
            "Search sessions and hosts, or type a URL to create one…".to_string()
        };
        // the field: the text typed, its selection and caret, or the hint
        let t = crate::theme::theme();
        let field = div().px(px(14.)).py(px(10.)).border_b_1().border_color(rgb(t.panel_divider)).text_size(px(14.)).font_family(UI_FONT).child(crate::field::field_view(&sel.filter, sel.caret_visible(), &hint, true));
        let mut list = div().flex().flex_col().py(px(6.)).px(px(6.));
        let row_style = |d: gpui::Stateful<gpui::Div>, picked: bool| {
            d.flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(14.))
                .font_family(UI_FONT)
                .cursor_pointer()
                .when(picked, |d| d.bg(rgb(t.panel_pick)))
                .when(!picked, |d| d.hover(|s| s.bg(rgb(t.panel_hover))))
        };
        for (i, row) in rows.iter().enumerate() {
            let picked = i == sel.cursor && row.pickable();
            let el = match row {
                Row::Header(h) => {
                    // the host: its name, the provider dimmed, and a × to forget it
                    let (name, prov) = h.parts();
                    let mut d = div().flex().flex_row().items_baseline().gap(px(8.)).px(px(10.)).pt(px(8.)).pb(px(4.)).text_size(px(12.)).font_family(UI_FONT).text_color(rgb(0x6f6f6f)).child(div().text_color(rgb(0x333333)).child(name));
                    if !prov.is_empty() {
                        d = d.child(div().child(prov));
                    }
                    if !h.is_local() {
                        let forget = h.clone();
                        d = d.child(div().flex_1()).child(
                            div()
                                .id(("forget", i))
                                .px(px(6.))
                                .rounded(px(4.))
                                .text_color(rgb(t.panel_dim))
                                .hover(|s| s.bg(rgb(t.panel_hover)).text_color(rgb(t.panel_text)))
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        forget_host(&forget);
                                        if let Some(s) = this.selector.as_mut() {
                                            s.hosts.retain(|x| *x != forget);
                                            s.sessions.remove(&forget);
                                            let mut all = known_sessions();
                                            all.remove(&forget);
                                            s.settle();
                                        }
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                ),
                        );
                    }
                    d.into_any_element()
                }
                Row::Divider => div().h(px(1.)).my(px(6.)).mx(px(4.)).bg(rgb(t.panel_divider)).into_any_element(),
                Row::Note(n) => div().px(px(22.)).py(px(4.)).text_size(px(13.)).font_family(UI_FONT).text_color(rgb(t.panel_dim)).child(n.clone()).into_any_element(),
                Row::Open(u) | Row::Create(u) => {
                    let is_current = matches!(row, Row::Open(u) if *u == self.url);
                    let create = matches!(row, Row::Create(_));
                    let (label, host, provider) = session_parts(u);
                    let mut text = div().flex().flex_row().items_baseline().gap(px(8.));
                    if create {
                        text = text.child(div().text_color(rgb(t.panel_accent)).child("Create"));
                    }
                    text = text.child(div().child(label));
                    if create {
                        // where it will be, since the section does not say
                        if !host.is_empty() {
                            text = text.child(div().text_color(rgb(t.panel_dim)).child(host));
                        }
                        text = text.child(div().text_color(rgb(t.panel_dim)).text_size(px(12.)).child(provider));
                    }
                    let r = row.clone();
                    let mut d = row_style(div().id(("row", i)), picked).pl(px(22.)).text_color(rgb(t.panel_text)).child(text);
                    if is_current {
                        d = d.child(div().text_color(rgb(t.panel_accent)).child("✓"));
                    }
                    // a session is ended from here: "end" at the right
                    if !create {
                        let end = u.clone();
                        d = d.child(div().flex_1()).child(
                            div()
                                .id(("end", i))
                                .px(px(6.))
                                .rounded(px(4.))
                                .text_size(px(12.))
                                .text_color(rgb(t.panel_dim))
                                .hover(|s| s.bg(rgb(t.panel_danger_hover)).text_color(rgb(t.panel_text)))
                                .child("end")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.end_session_from_picker(end.clone(), cx);
                                        cx.stop_propagation();
                                    }),
                                ),
                        );
                    }
                    d.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.choose(r.clone(), window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .into_any_element()
                }
                Row::NewSession(_) | Row::NewHost | Row::RenameThis | Row::Rename(_) => {
                    let (text, indent) = match row {
                        Row::NewSession(_) => ("+ new session".to_string(), 22.),
                        Row::NewHost => ("+ new host…".to_string(), 10.),
                        Row::Rename(n) => (format!("Rename to “{n}”"), 10.),
                        _ => ("Rename This Session…".to_string(), 10.),
                    };
                    let r = row.clone();
                    row_style(div().id(("row", i)), picked)
                        .pl(px(indent))
                        .text_color(rgb(t.panel_accent))
                        .child(text)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.choose(r.clone(), window, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .into_any_element()
                }
            };
            list = list.child(el);
        }
        if rows.is_empty() && sel.connect.is_none() {
            let what = if sel.renaming || sel.naming.is_some() { "Type a name" } else { "Nothing matches" };
            list = list.child(div().px(px(10.)).py(px(6.)).text_size(px(13.)).font_family(UI_FONT).text_color(rgb(t.panel_dim)).child(what));
        }
        let mut panel = div()
            .w(px(620.))
            .max_h(px(560.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .rounded(px(10.))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden();
        panel = match &sel.connect {
            Some(form) => panel.child(self.connect_form(form, sel.caret_visible(), cx)),
            None => panel.child(field).child(list),
        };
        Some(deferred(anchored().position(point(px(72.), px(self.top() - 2.))).child(panel)).with_priority(1))
    }
}

#[cfg(test)]
mod picker_tests {
    use super::*;

    fn picker() -> Selector {
        let local = Host::local();
        let box_ = Host { provider: "sprite".into(), arg: "devvm".into() };
        let down = Host { provider: "ssh".into(), arg: "gone".into() };
        let si = |l: &str| SessionInfo { id: format!("id-{l}"), label: l.to_string() };
        let mut sessions = HashMap::new();
        sessions.insert(local.clone(), Loading::Ready(vec![si("default"), si("notes")]));
        sessions.insert(box_.clone(), Loading::Seeded(vec![si("work")]));
        sessions.insert(down.clone(), Loading::Failed(vec![si("old")], "no route".into()));
        Selector { filter: crate::field::LineEdit::new(), cursor: 0, moved: false, hosts: vec![local, box_, down], sessions, current: SessionUrl::local("notes"), renaming: false, naming: None, connect: None, epoch: 1, caret_since: std::time::Instant::now() }
    }

    #[test]
    fn hosts_head_their_sessions_and_offer_a_new_one() {
        let sel = picker();
        let rows = sel.rows();
        let shape: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Row::Header(h) => format!("[{}]", h.parts().0),
                Row::Open(u) => u.session.clone(),
                Row::NewSession(_) => "+session".into(),
                Row::Note(t) => format!("note:{}", t.split(':').next().unwrap()),
                Row::Divider => "-".into(),
                Row::NewHost => "+host".into(),
                Row::RenameThis => "rename".into(),
                other => format!("{other:?}"),
            })
            .collect();
        // a host still asked shows what it had last time, then "asking…";
        // one that could not be reached keeps what it had, and says so
        assert_eq!(shape, vec!["[local]", "default", "notes", "+session", "[devvm]", "work", "note:asking…", "+session", "[gone]", "old", "note:unreachable", "+session", "-", "+host", "rename"]);
        // the cursor lands on this window's session
        let mut sel = picker();
        sel.land_on_current();
        assert!(matches!(sel.rows()[sel.cursor], Row::Open(ref u) if u.session == "notes"));
        // a filter narrows to sessions and hosts that carry it; a URL typed creates
        let mut sel = picker();
        sel.filter = "dev".into();
        let rows = sel.rows();
        assert!(matches!(rows[0], Row::Header(ref h) if h.arg == "devvm"), "{rows:?}");
        assert!(!rows.iter().any(|r| matches!(r, Row::Open(u) if u.is_local())));
        sel.filter = "ssh://new@host/x".into();
        assert!(sel.rows().iter().any(|r| matches!(r, Row::Create(u) if u.arg == "new@host" && u.session == "x")));
        // a host answering fills its section in above the cursor: the
        // cursor keeps its row (the first devvm session grows to three)
        let mut sel = picker();
        sel.cursor = sel.rows().iter().position(|r| matches!(r, Row::Open(u) if u.session == "old")).unwrap();
        sel.moved = true;
        let devvm = Host { provider: "sprite".into(), arg: "devvm".into() };
        sel.keeping(|s| {
            let si = |l: &str| SessionInfo { id: format!("id-{l}"), label: l.to_string() };
            s.sessions.insert(devvm.clone(), Loading::Ready(vec![si("a"), si("b"), si("work")]));
        });
        assert!(matches!(sel.rows()[sel.cursor], Row::Open(ref u) if u.session == "old"), "{:?}", sel.rows()[sel.cursor]);
        // its row gone, the cursor settles on the next pickable one
        sel.keeping(|s| {
            s.sessions.insert(Host { provider: "ssh".into(), arg: "gone".into() }, Loading::Ready(vec![]));
        });
        assert!(sel.rows()[sel.cursor].pickable());
        // naming a new session on a host
        sel.filter = "scratch".into();
        sel.naming = Some(Host { provider: "sprite".into(), arg: "devvm".into() });
        assert_eq!(sel.rows(), vec![Row::Create(SessionUrl::parse("sprite://devvm/scratch").unwrap())]);
        // the new-host form
        let mut f = Connect { providers: vec!["ssh".into(), "sprite".into()], provider: 0, host: crate::field::LineEdit::new(), field: Field::Provider };
        assert!(f.host().is_none());
        f.host = "me@box".into();
        assert_eq!(f.host().unwrap(), Host { provider: "ssh".into(), arg: "me@box".into() });
        f.next_provider(1);
        assert_eq!(f.host().unwrap().provider, "sprite");
        assert_eq!(Host::of(&SessionUrl::parse("sprite://devvm/x").unwrap()).parts(), ("devvm".to_string(), "(sprite)".to_string()));
        assert_eq!(Host::local().parts(), ("local".to_string(), String::new()));
    }
}
