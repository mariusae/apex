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

use apex_server::remote::{list_sessions, new_session};

use crate::app::{Acme, Backend};

actions!(apex, [Quit, HideApp, About, InstallCli, NewWindow, CloseWindow, Sessions, Undo, Redo, Cut, Copy, Paste, SelectAll]);

/// Set by the Quit action so closing windows on the way out does not
/// forget which sessions were open.
pub static QUITTING: AtomicBool = AtomicBool::new(false);

pub const TITLEBAR_HEIGHT: f32 = 30.;

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
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Sessions…", Sessions),
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
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-k", Sessions, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
    ]
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
                if a.socket.is_some() && !open.contains(&a.session) {
                    open.push(a.session.clone());
                }
            }
        }
    }
    remember(&open);
}

/// The sessions to open at launch: the remembered ones that still exist;
/// else the first existing one; else a new `local`.
pub fn plan(socket: &Path) -> std::io::Result<Vec<String>> {
    let existing = list_sessions(socket)?;
    let again: Vec<String> = remembered().into_iter().filter(|s| existing.contains(s)).collect();
    if !again.is_empty() {
        return Ok(again);
    }
    if let Some(first) = existing.first() {
        return Ok(vec![first.clone()]);
    }
    new_session(socket, "local")?;
    Ok(vec!["local".to_string()])
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
            .args(["--socket", &socket.to_string_lossy(), "--session", "local", "server"])
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
            let _ = apex_server::daemon::Daemon::run(&p, "local");
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

/// The dropdown under the session name: a filter that is also the name
/// of a session to create, and the list.
pub struct Selector {
    pub filter: String,
    pub sessions: Vec<String>,
    pub cursor: usize,
}

impl Selector {
    /// Rows in order: matching sessions, then "create" if the filter
    /// names no existing session.
    pub fn rows(&self) -> Vec<Row> {
        let f = self.filter.trim().to_lowercase();
        let mut rows: Vec<Row> = self
            .sessions
            .iter()
            .filter(|s| f.is_empty() || s.to_lowercase().contains(&f))
            .map(|s| Row::Session(s.clone()))
            .collect();
        if !f.is_empty() && !self.sessions.iter().any(|s| s.to_lowercase() == f) {
            rows.push(Row::Create(self.filter.trim().to_string()));
        }
        rows
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Session(String),
    Create(String),
}

impl Acme {
    pub fn open_selector(&mut self, cx: &mut Context<Self>) {
        let Some(socket) = &self.socket else { return };
        let sessions = list_sessions(socket).unwrap_or_default();
        let cursor = sessions.iter().position(|s| *s == self.session).unwrap_or(0);
        self.selector = Some(Selector { filter: String::new(), sessions, cursor });
        cx.notify();
    }

    pub fn close_selector(&mut self, cx: &mut Context<Self>) {
        self.selector = None;
        cx.notify();
    }

    /// Keys while the selector is open. Returns true if it took the key.
    pub fn selector_key(&mut self, key: &str, ch: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(sel) = self.selector.as_mut() else { return false };
        match key {
            "escape" => self.close_selector(cx),
            "enter" => {
                let rows = sel.rows();
                if let Some(row) = rows.get(sel.cursor.min(rows.len().saturating_sub(1))).cloned() {
                    self.choose(row, window, cx);
                }
            }
            "up" => {
                sel.cursor = sel.cursor.saturating_sub(1);
                cx.notify();
            }
            "down" => {
                let n = sel.rows().len();
                sel.cursor = (sel.cursor + 1).min(n.saturating_sub(1));
                cx.notify();
            }
            "backspace" => {
                sel.filter.pop();
                sel.cursor = 0;
                cx.notify();
            }
            _ => {
                if let Some(c) = ch {
                    if !c.chars().any(char::is_control) {
                        sel.filter.push_str(c);
                        sel.cursor = 0;
                        cx.notify();
                    }
                }
            }
        }
        true
    }

    pub fn choose(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        self.selector = None;
        let name = match row {
            Row::Session(s) | Row::Create(s) => s,
        };
        if name == self.session {
            cx.notify();
            return;
        }
        if let Err(e) = self.switch_session(&name, window) {
            eprintln!("apex-ui: switch to {name}: {e}");
        }
        save_open(cx);
        cx.notify();
    }

    /// The strip at the top: traffic lights live in its left margin; the
    /// session name is a button.
    pub fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = match &self.backend {
            Backend::Local(_) => "in-process".to_string(),
            Backend::Remote(_) => self.session.clone(),
        };
        let clickable = self.socket.is_some();
        let mut button = div()
            .id("session")
            .px(px(8.))
            .py(px(2.))
            .rounded(px(5.))
            .text_size(px(13.))
            .font_family("Lucida Grande")
            .text_color(rgb(0x222222))
            .child(if clickable { format!("{label}  ▾") } else { label });
        if clickable {
            button = button
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0xdcdcdc)))
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
    }

    /// The dropdown, when open.
    pub fn selector_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let sel = self.selector.as_ref()?;
        let rows = sel.rows();
        let cursor = sel.cursor.min(rows.len().saturating_sub(1));
        let filter_row = div()
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(rgb(0xdddddd))
            .text_size(px(13.))
            .font_family("Lucida Grande")
            .child(if sel.filter.is_empty() {
                div().text_color(rgb(0x888888)).child("Search sessions, or type a new name…")
            } else {
                div().text_color(rgb(0x111111)).child(format!("{}▏", sel.filter))
            });
        let mut panel = div()
            .w(px(360.))
            .bg(gpui::white())
            .border_1()
            .border_color(rgb(0xbbbbbb))
            .rounded(px(8.))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(filter_row);
        for (i, row) in rows.iter().enumerate() {
            let (text, dim) = match row {
                Row::Session(s) if *s == self.session => (format!("{s}   (this window)"), false),
                Row::Session(s) => (s.clone(), false),
                Row::Create(s) => (format!("Create session “{s}”"), true),
            };
            let row = row.clone();
            let item = div()
                .id(("row", i))
                .px(px(12.))
                .py(px(6.))
                .text_size(px(13.))
                .font_family("Lucida Grande")
                .text_color(if dim { rgb(0x0000aa) } else { rgb(0x111111) })
                .cursor_pointer()
                .when(i == cursor, |d| d.bg(rgb(0xeaffff)))
                .hover(|s| s.bg(rgb(0xd8f8f8)))
                .child(text)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.choose(row.clone(), window, cx);
                        cx.stop_propagation();
                    }),
                );
            panel = panel.child(item);
        }
        if rows.is_empty() {
            panel = panel.child(div().px(px(12.)).py(px(6.)).text_size(px(13.)).text_color(rgb(0x888888)).child("no sessions"));
        }
        Some(deferred(anchored().position(point(px(72.), px(TITLEBAR_HEIGHT))).child(panel)).with_priority(1))
    }
}
