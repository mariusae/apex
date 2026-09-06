//! The application shell around the acme window: what the app does on
//! launch (which sessions to open, starting the daemon), the menu bar and
//! its actions, the title bar with the session button, and the session
//! selector that drops down from it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{
    actions, anchored, deferred, div, point, prelude::*, px, rgb, App, Context, KeyBinding, Menu, MenuItem, MouseButton,
    Window,
};

use apex_server::providers::SessionUrl;
use apex_server::remote::{list_sessions, new_session};

use crate::app::{Acme, Backend};

actions!(apex, [Quit, HideApp, About, InstallCli, NewFile, NewWindow, CloseWindow, Sessions, Reconnect, Put, Del, Undo, Redo, Cut, Copy, Paste, SelectAll]);

/// Set by the Quit action so closing windows on the way out does not
/// forget which sessions were open.
pub static QUITTING: AtomicBool = AtomicBool::new(false);

pub const TITLEBAR_HEIGHT: f32 = 30.;
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
                MenuItem::action("Reconnect", Reconnect),
                MenuItem::separator(),
                MenuItem::action("Put", Put),
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
        KeyBinding::new("cmd-r", Reconnect, None),
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

fn state_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(home).join("Library/Application Support/apex/last-sessions")
}

/// Sessions that had windows when the app last ran.
pub fn remembered() -> Vec<String> {
    std::fs::read_to_string(state_file())
        .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

pub fn remember(sessions: &[String]) {
    let p = state_file();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, sessions.join("\n") + "\n");
}

/// Record the sessions of every open window, unless the app is quitting
/// (then the list as it was is what we want back next time).
pub fn save_open(cx: &App) {
    if QUITTING.load(Ordering::Relaxed) {
        return;
    }
    let mut open = Vec::new();
    for w in cx.windows() {
        if let Some(h) = w.downcast::<Acme>() {
            if let Ok(a) = h.read(cx) {
                if a.socket.is_some() && a.url.provider != "via" {
                    let spec = a.url.to_string();
                    if !open.contains(&spec) {
                        open.push(spec);
                    }
                }
            }
        }
    }
    remember(&open);
}

/// The sessions to open at launch: the remembered ones (local ones only
/// if they still exist; remote ones are tried); else the first existing
/// local one; else a new `default`.
pub fn plan(socket: &Path) -> std::io::Result<Vec<SessionUrl>> {
    let existing = list_sessions(socket)?;
    let again: Vec<SessionUrl> = remembered()
        .iter()
        .filter_map(|s| SessionUrl::parse(s))
        .filter(|u| !u.is_local() || existing.contains(&u.session))
        .collect();
    if !again.is_empty() {
        return Ok(again);
    }
    if let Some(first) = existing.first() {
        return Ok(vec![SessionUrl::local(first)]);
    }
    new_session(socket, apex_server::providers::DEFAULT_SESSION)?;
    Ok(vec![SessionUrl::local(apex_server::providers::DEFAULT_SESSION)])
}

/// Make sure a daemon answers on `socket`: start one with the `apex`
/// command next to this executable (the app bundle) or on the PATH; as a
/// last resort run one inside this process, which then lives only as
/// long as the app.
pub fn ensure_daemon(socket: &Path) -> std::io::Result<()> {
    if list_sessions(socket).is_ok() {
        return Ok(());
    }
    if let Some(d) = socket.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe.with_file_name("apex"));
    }
    candidates.push(PathBuf::from("apex"));
    let mut started = false;
    for apex in candidates {
        let spawned = Command::new(&apex)
            .args(["--socket", &socket.to_string_lossy(), "--session", apex_server::providers::DEFAULT_SESSION, "server"])
            .current_dir(&home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if spawned.is_ok() {
            started = true;
            break;
        }
    }
    if !started {
        eprintln!("apex-ui: no apex command found; running the daemon in-process (sessions end with the app)");
        let p = socket.to_path_buf();
        std::thread::spawn(move || {
            let _ = std::env::set_current_dir(&home);
            let _ = apex_server::daemon::Daemon::run(&p, apex_server::providers::DEFAULT_SESSION);
        });
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if list_sessions(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(std::io::Error::other("the daemon did not start"))
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
pub struct Selector {
    pub filter: String,
    /// Index into `rows()`; only pickable rows are ever landed on.
    pub cursor: usize,
    pub recent: Vec<SessionUrl>,
    pub local: Vec<SessionUrl>,
    /// The destination of a remote window, and its sessions.
    pub remote: Option<(String, Vec<SessionUrl>)>,
    pub current: SessionUrl,
    /// Typing a new name for this window's session.
    pub renaming: bool,
    /// When the caret last became visible: it blinks, and a keystroke
    /// makes it show at once.
    pub caret_since: std::time::Instant,
}

/// The caret's period.
const BLINK: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header(String),
    Divider,
    Open(SessionUrl),
    Create(SessionUrl),
    /// "Rename this session…": type the new name next.
    RenameThis,
    Rename(String),
}

impl Row {
    pub fn pickable(&self) -> bool {
        !matches!(self, Row::Header(_) | Row::Divider)
    }
}

impl Selector {
    pub fn caret_visible(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / BLINK.as_millis()) % 2 == 0
    }

    pub fn rows(&self) -> Vec<Row> {
        let f = self.filter.trim();
        if self.renaming {
            return if f.is_empty() || f.contains('/') { Vec::new() } else { vec![Row::Rename(f.to_string())] };
        }
        let mut rows = Vec::new();
        let mut seen: Vec<SessionUrl> = Vec::new();
        let fl = f.to_lowercase();
        let matches = |u: &SessionUrl| fl.is_empty() || u.to_string().to_lowercase().contains(&fl);
        let section = |rows: &mut Vec<Row>, seen: &mut Vec<SessionUrl>, title: String, urls: &[SessionUrl]| {
            let fresh: Vec<&SessionUrl> = urls.iter().filter(|u| matches(u) && !seen.contains(u)).collect();
            if fresh.is_empty() {
                return;
            }
            if fl.is_empty() {
                rows.push(Row::Header(title));
            }
            for u in fresh {
                seen.push(u.clone());
                rows.push(Row::Open(u.clone()));
            }
        };
        section(&mut rows, &mut seen, "This Window".into(), std::slice::from_ref(&self.current));
        section(&mut rows, &mut seen, "Recent".into(), &self.recent);
        section(&mut rows, &mut seen, "This Machine".into(), &self.local);
        if let Some((dest, urls)) = &self.remote {
            section(&mut rows, &mut seen, dest.clone(), urls);
        }
        let mut actions = Vec::new();
        if !f.is_empty() {
            if let Some(u) = SessionUrl::parse(f) {
                if !seen.contains(&u) {
                    actions.push(Row::Create(u));
                }
            }
        } else {
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

    pub fn move_cursor(&mut self, delta: i32) {
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
    let last: Vec<String> = remembered().into_iter().map(|s| if s == old.to_string() { new.to_string() } else { s }).collect();
    remember(&last);
}

impl Acme {
    pub fn open_selector(&mut self, cx: &mut Context<Self>) {
        let Some(socket) = self.socket.clone() else { return };
        let local: Vec<SessionUrl> = list_sessions(&socket).unwrap_or_default().iter().map(|s| SessionUrl::local(s)).collect();
        let remote = self.url.dest().map(|dest| {
            let urls = apex_server::providers::list_sessions(&dest).unwrap_or_default().iter().map(|s| self.url.with_session(s)).collect();
            (dest, urls)
        });
        let mut sel = Selector { filter: String::new(), cursor: 0, recent: recent(), local, remote, current: self.url.clone(), renaming: false, caret_since: std::time::Instant::now() };
        sel.settle();
        self.selector = Some(sel);
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

    pub fn close_selector(&mut self, cx: &mut Context<Self>) {
        self.selector = None;
        cx.notify();
    }

    /// Keys while the selector is open. Returns true if it took the key.
    pub fn selector_key(&mut self, key: &str, ch: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(sel) = self.selector.as_mut() else { return false };
        sel.caret_since = std::time::Instant::now();
        match key {
            "escape" => self.close_selector(cx),
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
            "backspace" => {
                sel.filter.pop();
                sel.cursor = 0;
                sel.settle();
                cx.notify();
            }
            _ => {
                if let Some(c) = ch {
                    if !c.chars().any(char::is_control) {
                        sel.filter.push_str(c);
                        sel.cursor = 0;
                        sel.settle();
                        cx.notify();
                    }
                }
            }
        }
        true
    }

    pub fn choose(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            Row::Header(_) | Row::Divider => {}
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
                save_open(cx);
                cx.notify();
            }
            Row::Open(url) | Row::Create(url) => {
                self.selector = None;
                if url == self.url {
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
                    cx.notify();
                    return;
                }
                if let Err(e) = self.reattach(&url, window) {
                    eprintln!("apex-ui: attach {url}: {e}");
                    self.notice(&format!("{url}: {e}\n"));
                }
                save_open(cx);
                cx.notify();
            }
        }
    }

    /// The strip at the top: traffic lights live in its left margin; the
    /// session URL is a button, a tinted pill as in Zed.
    pub fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = match &self.backend {
            Backend::Local(_) => "in-process".to_string(),
            Backend::Remote(_) if self.fenced() => format!("{}  ·  fenced", self.url),
            Backend::Remote(_) => self.url.to_string(),
        };
        let clickable = self.socket.is_some();
        let open = self.selector.is_some();
        let mut button = div()
            .id("session")
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .text_size(px(13.))
            .font_family(UI_FONT)
            .text_color(rgb(0x000099))
            .when(open, |d| d.bg(rgb(0xeaffff)))
            .child(label);
        if clickable {
            button = button
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0xeaffff)))
                .on_mouse_down(
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
        div()
            .id("titlebar")
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .pl(px(78.))
            .bg(rgb(0xececec))
            .border_b_1()
            .border_color(rgb(0xc8c8c8))
            .gap(px(6.))
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
            // the link to the daemon, at the right: bright while it is up, faded when gone
            .child(div().pr(px(12.)).text_size(px(13.)).opacity(if self.connected { 1.0 } else { 0.25 }).child("⚡"))
    }

    /// The dropdown, when open.
    pub fn selector_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let sel = self.selector.as_ref()?;
        let rows = sel.rows();
        let hint = if sel.renaming { "New name for this session…" } else { "Search sessions, or type a name or URL to create one…" };
        // the field: the text typed and a caret, or the hint after a caret
        let caret = div().w(px(1.5)).h(px(16.)).flex_none().when(sel.caret_visible(), |d| d.bg(rgb(0x000099)));
        let field = div()
            .px(px(14.))
            .py(px(10.))
            .border_b_1()
            .border_color(rgb(0xdddddd))
            .text_size(px(14.))
            .font_family(UI_FONT)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(1.))
            .when(!sel.filter.is_empty(), |d| d.child(div().text_color(rgb(0x111111)).child(sel.filter.clone())))
            .child(caret)
            .when(sel.filter.is_empty(), |d| d.child(div().pl(px(4.)).text_color(rgb(0x8a8a8a)).child(hint)));
        let mut list = div().flex().flex_col().py(px(6.)).px(px(6.));
        for (i, row) in rows.iter().enumerate() {
            let picked = i == sel.cursor && row.pickable();
            let el = match row {
                Row::Header(t) => div().px(px(10.)).pt(px(8.)).pb(px(4.)).text_size(px(12.)).font_family(UI_FONT).text_color(rgb(0x6f6f6f)).child(t.clone()).into_any_element(),
                Row::Divider => div().h(px(1.)).my(px(6.)).mx(px(4.)).bg(rgb(0xdddddd)).into_any_element(),
                Row::Open(u) | Row::Create(u) => {
                    let is_current = matches!(row, Row::Open(u) if *u == self.url);
                    let text = match row {
                        Row::Create(u) => format!("Create {u}"),
                        _ => u.to_string(),
                    };
                    let create = matches!(row, Row::Create(_));
                    let r = row.clone();
                    let mut d = div()
                        .id(("row", i))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .text_size(px(14.))
                        .font_family(UI_FONT)
                        .text_color(if create { rgb(0x000099) } else { rgb(0x111111) })
                        .cursor_pointer()
                        .when(picked, |d| d.bg(rgb(0x9eeeee)))
                        .when(!picked, |d| d.hover(|s| s.bg(rgb(0xe4e4e4))))
                        .child(div().child(text));
                    if is_current {
                        d = d.child(div().text_color(rgb(0x000099)).child("✓"));
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
                Row::RenameThis | Row::Rename(_) => {
                    let text = match row {
                        Row::Rename(n) => format!("Rename to “{n}”"),
                        _ => "Rename This Session…".to_string(),
                    };
                    let r = row.clone();
                    div()
                        .id(("row", i))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .text_size(px(14.))
                        .font_family(UI_FONT)
                        .text_color(rgb(0x111111))
                        .cursor_pointer()
                        .when(picked, |d| d.bg(rgb(0x9eeeee)))
                        .when(!picked, |d| d.hover(|s| s.bg(rgb(0xe4e4e4))))
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
        if rows.is_empty() {
            list = list.child(div().px(px(10.)).py(px(6.)).text_size(px(13.)).font_family(UI_FONT).text_color(rgb(0x8a8a8a)).child(if sel.renaming { "Type a name" } else { "No sessions" }));
        }
        let panel = div()
            .w(px(620.))
            .bg(rgb(0xf4f4f4))
            .border_1()
            .border_color(rgb(0xc8c8c8))
            .rounded(px(10.))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(field)
            .child(list);
        Some(deferred(anchored().position(point(px(72.), px(TITLEBAR_HEIGHT - 2.))).child(panel)).with_priority(1))
    }
}
