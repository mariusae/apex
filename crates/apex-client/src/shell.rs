//! The application shell around the acme window: what the app does on
//! launch (which sessions to open, starting the daemon), the menu bar and
//! its actions, the title bar with the session button, and the session
//! selector that drops down from it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};


use gpui::{
    actions, deferred, div, point, prelude::*, px, rgb, size, App, Bounds, Context, KeyBinding, Menu, MenuItem, MouseButton, Pixels, WindowBounds,
    Window,
};

use apex_server::providers::SessionUrl;
use apex_server::proto::SessionInfo;
use apex_server::remote::{list_sessions, new_session};

use crate::app::{Acme, Backend};

actions!(apex, [Quit, HideApp, About, InstallCli, NewFile, CloseWindow, NewTab, CloseTab, PreviousSession, Profile, Tab1, Tab2, Tab3, Tab4, Tab5, Tab6, Tab7, Tab8, Tab9, PrevTab, NextTab, Goto, GotoAll, NextNotification, NavBack, NavFwd, Reconnect, ToggleFullScreen, Put, Get, Del, Undo, Redo, Cut, Copy, Paste, SelectAll, ThemeLight, ThemeDark, ThemeSystem, ToggleFullscreenTabs, ToggleContrast]);

/// View ▸ Always Show Tabs in Full Screen toggled: kept, the menus
/// remade with the mark, every window laid out again.
pub fn toggle_fullscreen_tabs(cx: &mut App) {
    crate::theme::set_fullscreen_tabs(!crate::theme::fullscreen_tabs());
    cx.set_menus(menus());
    cx.refresh_windows();
}

/// View ▸ Correct Terminal Contrast toggled: kept, the menus remade
/// with the mark, every terminal painted again.
pub fn toggle_contrast(cx: &mut App) {
    crate::theme::set_contrast(!crate::theme::contrast());
    cx.set_menus(menus());
    cx.refresh_windows();
}

/// The theme chosen in the View menu: kept, the menus remade with the
/// choice marked, every window redrawn.
pub fn set_theme(m: crate::theme::Mode, cx: &mut App) {
    crate::theme::set_mode(m);
    apply_theme(cx);
}

/// The theme in effect changed (chosen, or the system's appearance
/// under System): the menus remade with the mark, every link told the
/// colours (a link made before the system's appearance was known was
/// told light's), pages restyled, every window redrawn.
pub fn apply_theme(cx: &mut App) {
    cx.set_menus(menus());
    // the daemons hear the new colours, for the programs that ask: after
    // this update, since a menu's action arrives while the focused window
    // is mid-update and cannot be reached (its link, the one the shown
    // session's terminals answer from, was left on the old colours)
    cx.defer(|cx| {
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
    });
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
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Previous Tab", PrevTab),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::action("Go to…", Goto),
                MenuItem::action("Go to in All Tabs…", GotoAll),
                MenuItem::action("Next Notification", NextNotification),
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
                let tabs = if crate::theme::fullscreen_tabs() { "Always Show Tabs in Full Screen ✓" } else { "Always Show Tabs in Full Screen" };
                let contrast = if crate::theme::contrast() { "Correct Terminal Contrast ✓" } else { "Correct Terminal Contrast" };
                vec![
                    MenuItem::action(mark("Light", crate::theme::Mode::Light), ThemeLight),
                    MenuItem::action(mark("Dark", crate::theme::Mode::Dark), ThemeDark),
                    MenuItem::action(mark("System", crate::theme::Mode::System), ThemeSystem),
                    MenuItem::separator(),
                    MenuItem::action(tabs, ToggleFullscreenTabs),
                    MenuItem::action(contrast, ToggleContrast),
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
        KeyBinding::new("cmd-s", Put, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", Del, None),
        KeyBinding::new("cmd-shift-w", CloseTab, None),
        KeyBinding::new("cmd-shift-k", PreviousSession, None),
        KeyBinding::new("cmd-g", NextNotification, None),
        KeyBinding::new("cmd-1", Tab1, None),
        KeyBinding::new("cmd-2", Tab2, None),
        KeyBinding::new("cmd-3", Tab3, None),
        KeyBinding::new("cmd-4", Tab4, None),
        KeyBinding::new("cmd-5", Tab5, None),
        KeyBinding::new("cmd-6", Tab6, None),
        KeyBinding::new("cmd-7", Tab7, None),
        KeyBinding::new("cmd-8", Tab8, None),
        KeyBinding::new("cmd-9", Tab9, None),
        // ⌘⇧[ and ⌘⇧]: macOS hands the shifted character over, so the
        // binding is on what the key makes -- { and } -- as Zed's is
        KeyBinding::new("cmd-{", PrevTab, None),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-,", Profile, None),
        KeyBinding::new("cmd-r", Get, None),
        KeyBinding::new("cmd-shift-r", Reconnect, None),
        KeyBinding::new("cmd-p", Goto, None),
        KeyBinding::new("cmd-shift-p", GotoAll, None),
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

/// Record the window, its session and its frame, unless the app is
/// quitting (then what was recorded is what we want back next time).
/// apex has one window; the loop is over the one there is.
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
        if a.socket.is_none() || a.url.provider == "via" {
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

/// The window to open at launch: the one remembered, on the session it
/// showed and at the frame it had (a session that is gone, after `apex
/// stop` say, is made again, empty: attaching creates it); else the
/// first existing local session; else a new `default`. The sessions
/// beside it come back as its tabs, from what the pool remembers.
pub fn plan(socket: &Path) -> std::io::Result<(SessionUrl, Option<WindowBounds>)> {
    let existing = list_sessions(socket)?;
    let again = remembered()
        .iter()
        .find_map(|r| SessionUrl::parse(&r.url).map(|u| (u, r.frame.map(|b| if r.fullscreen { WindowBounds::Fullscreen(b) } else { WindowBounds::Windowed(b) }))));
    if let Some(one) = again {
        return Ok(one);
    }
    if let Some(first) = existing.first() {
        return Ok((SessionUrl::local(&first.label).with_id(&first.id), None));
    }
    new_session(socket, apex_server::providers::DEFAULT_SESSION)?;
    Ok((SessionUrl::local(apex_server::providers::DEFAULT_SESSION), None))
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
/// How long the pointer rests on a tab before its card shows.
const CARD_DELAY: std::time::Duration = std::time::Duration::from_millis(450);

/// A tab's status card, beneath the tab while the pointer rests on it:
/// label and value lines. Records its bounds so the web views cut a
/// hole for it.
pub fn tab_card(lines: &[(String, String)], mark: std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<Pixels>>>>) -> gpui::Div {
    {
        let t = crate::theme::theme();
        let mut card = div()
            .relative()
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .rounded(px(8.))
            .shadow_md()
            .px(px(12.))
            .py(px(8.))
            .text_size(px(12.))
            .line_height(px(17.))
            .font_family(UI_FONT)
            .text_color(rgb(t.panel_text))
            .child(div().absolute().top(px(0.)).left(px(0.)).size_full().child(gpui::canvas(
                move |b, _, _| mark.borrow_mut().push(b),
                |_, _, _, _| {},
            ).size_full()));
        for (k, v) in lines {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(10.))
                    .child(div().w(px(56.)).text_color(rgb(t.panel_dim)).child(k.clone()))
                    .child(div().child(v.clone())),
            );
        }
        card
    }
}

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

/// What the picker is for: a new tab (the sessions not open here, the
/// hosts, a session to create, a host to add: ⌘T and the `+`), or the
/// tabs (the open ones and the recently closed, searched: ⌘⇧A and the
/// ▾ before the tabs, a browser's tab search).
pub struct Selector {
    /// The tabs open now, in the strip's order (left out of a new tab's
    /// list; the first section of the tabs' list).
    pub tabs: Vec<SessionUrl>,
    /// Tabs closed lately, the latest first (the tabs' second section).
    pub closed: Vec<SessionUrl>,
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
    /// Typing a new name for this session (a tab, right-clicked).
    pub renaming: Option<SessionUrl>,
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
    /// A session this window has open (a tab): going there.
    GoTo(SessionUrl),
    /// A session that is there but not open here: opening it.
    Open(SessionUrl),
    /// A session to make, named by what was typed, on this host.
    Create(SessionUrl, Host),
    /// "Add a host…": the form next.
    NewHost,
    /// Nothing to offer, or a host that could not be asked.
    Note(String),
    /// The new name typed for the session being renamed.
    Rename(String),
}

impl Row {
    /// The words at the right of the row: what picking it does.
    pub fn action(&self) -> String {
        match self {
            Row::GoTo(_) => "Go to session".into(),
            Row::Open(_) => "Open session".into(),
            Row::Create(_, h) => {
                let (name, prov) = h.parts();
                if prov.is_empty() {
                    format!("Create on {name}")
                } else {
                    format!("Create on {name} {prov}")
                }
            }
            Row::NewHost => "Add a host".into(),
            Row::Rename(_) => "Rename".into(),
            Row::Note(_) => String::new(),
        }
    }

    /// The glyph at the left: an arrow for somewhere to go, a plus for
    /// something to make.
    pub fn glyph(&self) -> &'static str {
        match self {
            Row::GoTo(_) | Row::Open(_) => "→",
            Row::Create(..) | Row::NewHost => "+",
            Row::Rename(_) => "✓",
            Row::Note(_) => "",
        }
    }

    /// What the row says: the session's name, or the text typed.
    pub fn title(&self) -> String {
        match self {
            Row::GoTo(u) | Row::Open(u) | Row::Create(u, _) => u.session.clone(),
            Row::NewHost => "Add a host…".into(),
            Row::Note(t) | Row::Rename(t) => t.clone(),
        }
    }

    /// The host under the name, dimmed, for a session that is elsewhere.
    pub fn where_(&self) -> Option<String> {
        match self {
            Row::GoTo(u) | Row::Open(u) | Row::Create(u, _) => (!u.is_local()).then(|| u.arg.clone()),
            _ => None,
        }
    }
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
        !matches!(self, Row::Note(_))
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
        if self.renaming.is_some() {
            return match (f.is_empty(), apex_server::providers::valid_label(f)) {
                (true, _) => Vec::new(),
                (false, Ok(())) => vec![Row::Rename(f.to_string())],
                (false, Err(_)) => vec![Row::Note("a label: a letter first, then lowercase letters, digits and -".into())],
            };
        }
        // one list, whatever the way in was: the sessions this window has
        // open, then the ones that are there to open, then the making of
        // one by the name typed on each host, then a host to add. A name
        // typed narrows all of it at once, so `another` offers the tab
        // `anotherfoobaz`, the session `anotherfoobar`, and `another` on
        // every host.
        let fl = f.to_lowercase();
        let matches = |u: &SessionUrl| fl.is_empty() || u.session.to_lowercase().contains(&fl) || u.arg.to_lowercase().contains(&fl);
        let mut rows = Vec::new();
        let mut seen: Vec<SessionUrl> = vec![self.current.clone()];
        for u in self.tabs.iter().filter(|u| **u != self.current && matches(u)) {
            seen.push(u.clone());
            rows.push(Row::GoTo(u.clone()));
        }
        // the sessions the hosts have, and the ones lately closed: there
        // to open, and not open here
        let mut elsewhere: Vec<SessionUrl> = Vec::new();
        for h in &self.hosts {
            for n in self.sessions.get(h).map(|l| l.sessions()).unwrap_or(&[]) {
                elsewhere.push(h.url_of(n));
            }
        }
        elsewhere.extend(self.closed.iter().cloned());
        for u in elsewhere {
            if !seen.contains(&u) && matches(&u) {
                seen.push(u.clone());
                rows.push(Row::Open(u));
            }
        }
        // a name to make: on each host that has no session by that name
        if apex_server::providers::valid_label(f).is_ok() {
            for h in &self.hosts {
                let u = h.url(f);
                if !seen.iter().any(|s| s.provider == u.provider && s.arg == u.arg && s.session == u.session) {
                    rows.push(Row::Create(u, h.clone()));
                }
            }
        }
        // and a host, at the very bottom
        rows.push(Row::NewHost);
        if rows.len() == 1 && !f.is_empty() && apex_server::providers::valid_label(f).is_err() {
            rows.insert(0, Row::Note("a label: a letter first, then lowercase letters, digits and -".into()));
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

    /// The cursor on the first row worth landing on.
    pub fn land_on_current(&mut self) {
        self.settle();
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
    /// ⌘T, the `+`: the picker, which is every way into a session.
    pub fn open_selector(&mut self, cx: &mut Context<Self>) {
        self.open_picker(cx);
    }

    /// A tab right-clicked: its session renamed (the picker's field, on
    /// that session).
    pub fn open_rename(&mut self, url: SessionUrl, cx: &mut Context<Self>) {
        self.open_picker(cx);
        if let Some(sel) = self.selector.as_mut() {
            sel.renaming = Some(url);
            sel.filter.clear();
            sel.cursor = 0;
        }
        cx.notify();
    }

    /// A session other than this window's renamed, on its host.
    pub fn rename_other(&mut self, url: &SessionUrl, to: &str, cx: &mut Context<Self>) {
        let (url, to) = (url.clone(), to.to_string());
        let socket = apex_server::daemon::default_socket();
        let renaming = cx.background_executor().spawn({
            let (url, to) = (url.clone(), to.clone());
            async move {
                if url.is_local() {
                    apex_server::remote::rename_session(&socket, url.session_ref(), &to).map_err(|e| e.to_string())
                } else {
                    let dest = url.dest().unwrap_or_default();
                    apex_server::providers::run(&dest, &format!("{} rename-session {} {}", apex_server::providers::REMOTE_BIN, url.session_ref(), to), None).map(|_| ()).map_err(|e| e.to_string())
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let r = renaming.await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |acme, cx| {
                    match r {
                        Ok(()) => renamed_recent(&url, &url.with_session(&to)),
                        Err(e) => acme.notice(&format!("rename: {e}\n")),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn open_picker(&mut self, cx: &mut Context<Self>) {
        let Some(socket) = self.socket.clone() else { return };
        static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
        let mut hosts = known_hosts();
        let here = Host::of(&self.url);
        if !hosts.contains(&here) {
            hosts.push(here);
        }
        let tabs = crate::pool::Pool::tabs(cx, &self.url);
        // recently closed: the recent sessions not open here, one per
        // place and label (a session made anew under an old label is
        // the same tab to the eye)
        let same = |a: &SessionUrl, b: &SessionUrl| a.provider == b.provider && a.arg == b.arg && a.session == b.session;
        let mut closed: Vec<SessionUrl> = Vec::new();
        for u in recent() {
            if !tabs.iter().any(|t| same(t, &u)) && !closed.iter().any(|c| same(c, &u)) {
                closed.push(u);
            }
        }
        let mut sel = Selector { tabs, closed, filter: crate::field::LineEdit::new(), cursor: 0, moved: false, hosts: hosts.clone(), sessions: HashMap::new(), current: self.url.clone(), renaming: None, connect: None, epoch, caret_since: std::time::Instant::now() };
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
                if sel.renaming.is_some() {
                    // back to the list
                    sel.renaming = None;
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
                sel.cursor = rows.iter().position(|r| matches!(r, Row::Create(_, x) if *x == h)).unwrap_or(0);
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
            Row::Note(_) => {}
            Row::NewHost => {
                if let Some(sel) = self.selector.as_mut() {
                    sel.connect = Some(Connect::new());
                    sel.filter.clear();
                }
                cx.notify();
            }
            Row::Rename(to) => {
                let target = self.selector.take().and_then(|s| s.renaming).unwrap_or_else(|| self.url.clone());
                if target == self.url {
                    self.rename_session(&to, window);
                } else {
                    self.rename_other(&target, &to, cx);
                }
                cx.defer(|cx| save_open(cx)); // after this window's update, so it is read too
                cx.notify();
            }
            Row::GoTo(url) | Row::Open(url) | Row::Create(url, _) => {
                self.selector = None;
                note_host(&url);
                if url == self.url {
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
                .when(active, |d| d.bg(rgb(t.tag_bg)))
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
        // a tab per session the app has open, in the order first shown,
        // whatever its link is doing: shown, parked, still coming up or
        // down, each saying so after its name. This one is the selected
        // tab and toggles the picker; another switches to it; its ×
        // closes it. The text sits on the bar's centre line.
        /// A tab is a pill inset in the bar, as ghostty's are, and the
        /// tabs share the bar between them: every one the same width,
        /// the row reaching the far right, with `+` after it.
        const INSET: f32 = 5.;
        const TAB_H: f32 = TITLEBAR_HEIGHT - INSET * 2.;
        /// One line box for every piece of text in a tab, whatever its
        /// size, so centring them centres them on the same line.
        const LINE: f32 = 18.;
        let t = crate::theme::theme();
        let strip: u32 = t.strip;
        // the bar is acme's paper; a tab lies a step off it, the pointer
        // on one lifts it another, and the one in front lies a step
        // beyond that (`theme::step`, toward the ink on light paper and
        // toward the light on dark)
        let idle_bg = crate::theme::step(strip, 1);
        let hover_bg = crate::theme::step(strip, 2);
        let front_bg = crate::theme::step(strip, 3);
        // no chevron in the strip and no key of its own for searching the
        // tabs: ⌘T opens the picker, which is that and every other way
        // into a session besides, and the bar is the tabs' room
        let all = crate::pool::Pool::tabs(cx, &self.url);
        // one tab is no tab: the bar is then a plain title bar, with the
        // session's name in the middle of it and the + at its right, as
        // ghostty's and Terminal's are
        let lone = (all.len() == 1).then(|| all[0].clone());
        let all = if lone.is_some() { Vec::new() } else { all };
        // the tabs fill the bar, each the same width as the rest; with
        // no tab in it, the + keeps to the right end all the same
        let mut tabs = div().id("tabs").flex_1().min_w_0().h_full().flex().flex_row().items_center().gap(px(2.));
        if lone.is_some() {
            tabs = tabs.justify_end();
        }
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
            Some((d.url.clone(), x, mine.origin.y))
        });
        let mut floating: Option<gpui::Div> = None;
        // where the hovered tab sits, for its card to hang under: read
        // with the drag's, before the bounds are cleared, since what the
        // tabs record they record at paint, a frame behind this one
        let hovered: Option<(SessionUrl, gpui::Bounds<Pixels>)> = self
            .tab_hovered
            .as_ref()
            // no card while a tab is held: it would hang in the drag's way
            .filter(|_| self.tab_drag.is_none())
            .filter(|(_, since)| since.elapsed() >= CARD_DELAY)
            .and_then(|(u, _)| self.tab_bounds.borrow().iter().find(|(x, _)| x == u).map(|(_, b)| (u.clone(), *b)));
        // where each tab lands this frame, for a drag to reorder by
        self.tab_bounds.borrow_mut().clear();
        let others = all.len() > 1;
        // the tab under the pointer, floating, keeps the width it had
        let drag_w = self.tab_drag.as_ref().map(|d| d.width);
        for (i, u) in all.into_iter().enumerate() {
            let current = u == self.url;
            // the label; the host dimmed after it for a session elsewhere
            let text = if current && matches!(self.backend, Backend::Local(_)) { label.clone() } else { u.session.clone() };
            let host = (!u.is_local()).then(|| u.arg.clone());
            // what the tab is doing, when it is anything but simply up:
            // "connecting…", "restoring…", "fenced", "offline"
            let word = self.tab_word(&u, cx);
            // a tool in that session wants the user: a bell before the
            // name says so, and nothing else changes -- a tab is not a
            // place to shout from
            let notified = self.tab_notified(&u, cx);
            // ⌘1 to ⌘9 reach the first nine tabs: each says which it is
            let key = (i < 9).then(|| format!("⌘{}", i + 1));
            // the × shows while the pointer is on the tab (a tab held for
            // a drag is not rested on, and shows none)
            let on_it = self.tab_hovered.as_ref().is_some_and(|(h, _)| *h == u) && self.tab_drag.is_none();
            let bg = if open { hover_bg } else { front_bg };
            let dim = t.tab_dim;
            let closable = clickable && (!current || others);
            // the tab's face: its look and its words, made twice for a
            // tab being dragged (the placeholder in the row, the one
            // under the pointer)
            let face = |ghost: bool| {
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    // the tabs share the bar: each takes the same part of
                    // it, and the one floating over a drag keeps the width
                    // its place in the row had
                    .when(!ghost, |d| d.flex_1().min_w_0())
                    .when(ghost, |d| d.w(drag_w.unwrap_or(px(160.))))
                    .h(px(TAB_H))
                    .px(px(10.))
                    .rounded(px(8.))
                    .text_size(px(13.))
                    .line_height(px(LINE))
                    .font_family(UI_FONT)
                    // the one in front is a pill of the colour of the row
                    // it shows; the others are the bare strip until the
                    // pointer is on one, as ghostty's are
                    .when(current, |d| d.bg(rgb(bg)).text_color(rgb(t.tab_current_text)))
                    .when(!current, |d| d.bg(rgb(idle_bg)).text_color(rgb(t.tab_text)).hover(|s| s.bg(rgb(hover_bg))))
                    .when(!current && ghost, |d| d.bg(rgb(hover_bg)))
                    // not simply up (fenced: another client leads and
                    // nothing here takes; coming up; down): the whole tab
                    // fades into the strip, its name greyed, and says so
                    .when(word.is_some(), |d| d.opacity(0.4).text_color(rgb(t.tab_fenced_text)))
                    // the name, centred in what room is left of the tab
                    // and cut short when there is not enough of it, with
                    // the bell before it when a tool in that session wants
                    // the user -- the two centred together, so the bell
                    // reads as part of the name and nothing else changes
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_baseline()
                            .justify_center()
                            .gap(px(5.))
                            .overflow_hidden()
                            .when(notified, |d| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).child("🔔")))
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(text.clone()))
                            .when_some(host.clone(), |d, h| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(dim)).child(h)))
                            .when_some(word.clone(), |d, w| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(t.tab_fenced_text)).child(w))),
                    )
                    // the key that reaches it, at the tab's right end
                    .when_some(key.clone(), |d, k| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(dim)).child(k)))
                    .when(closable && ghost, |d| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(dim)).child("×")))
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
                // the pointer resting on a tab: its status card beneath it
                // after a moment (`tab_hovered`, drawn below)
                let url = u.clone();
                tab = tab.on_hover(cx.listener(move |this, on: &bool, _, cx| {
                    // a tab passed over while one is dragged is not rested on
                    if this.tab_drag.is_some() {
                        return;
                    }
                    if *on {
                        if this.tab_hovered.as_ref().map(|(u, _)| u) != Some(&url) {
                            this.tab_hovered = Some((url.clone(), std::time::Instant::now()));
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(CARD_DELAY).await;
                                let _ = cx.update(|cx| this.update(cx, |_, cx| cx.notify()));
                            })
                            .detach();
                        }
                    } else if this.tab_hovered.as_ref().map(|(u, _)| u) == Some(&url) {
                        this.tab_hovered = None;
                        cx.notify();
                    }
                }));
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
                        // held, not rested on: the card goes, and comes back
                        // only for a pointer that comes to rest on a tab again
                        this.tab_hovered = None;
                        cx.stop_propagation();
                    }),
                );
                // right-clicked: the session renamed
                let url = u.clone();
                tab = tab.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.open_rename(url.clone(), cx);
                        cx.stop_propagation();
                    }),
                );
                // ×: a parked session let go; the current one let go
                // too, the window moving to the one parked last. It shows
                // while the pointer is on the tab, over the tab's own
                // contents (on the paper it lies on, so the name behind it
                // does not show through) and taking no room of its own, so
                // a tab is the same width whether the pointer is on it or
                // not and the tabs do not shift as it passes
                if closable && on_it {
                    let url = u.clone();
                    let over = if current { bg } else { hover_bg };
                    tab = tab.child(
                        div()
                            .id(("tab-close", i))
                            .absolute()
                            .left(px(6.))
                            .top(px(0.))
                            .h(px(TAB_H))
                            .flex()
                            .items_center()
                            .px(px(3.))
                            .rounded(px(4.))
                            .bg(rgb(over))
                            .text_size(px(14.))
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
        let mut plus = div()
            .id("tab-new")
            .flex_none()
            .h(px(TAB_H))
            .ml(px(2.))
            .w(px(TAB_H))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_1()
            .border_color(rgb(t.tab_outline_dim))
            .text_size(px(13.))
            .line_height(px(LINE))
            .font_family(UI_FONT)
            .text_color(rgb(t.tab_dim))
            .child("+");
        if clickable {
            plus = plus.cursor_pointer().hover(|s| s.bg(rgb(t.tab_hover)).border_color(rgb(t.tab_outline)).text_color(rgb(t.tab_current_text))).on_mouse_down(
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
        // the lone tab's name, in the middle of the bar: the whole width
        // of it, so the name sits where a title bar's title sits, and
        // under the + (added after it), which keeps its clicks
        let title = lone.map(|u| {
            let text = if matches!(self.backend, Backend::Local(_)) { label.clone() } else { u.session.clone() };
            let host = (!u.is_local()).then(|| u.arg.clone());
            let word = self.tab_word(&u, cx);
            let notified = self.tab_notified(&u, cx);
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_baseline()
                        .gap(px(5.))
                        .px(px(90.))
                        .overflow_hidden()
                        .text_size(px(13.))
                        .line_height(px(LINE))
                        .font_family(UI_FONT)
                        .text_color(rgb(t.tab_current_text))
                        .when(word.is_some(), |d| d.text_color(rgb(t.tab_fenced_text)))
                        .when(notified, |d| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).child("🔔")))
                        .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(text))
                        .when_some(host, |d, h| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(t.tab_dim)).child(h)))
                        .when_some(word, |d, w| d.child(div().flex_none().text_size(px(11.)).line_height(px(LINE)).text_color(rgb(t.tab_fenced_text)).child(w))),
                )
        });
        // the hovered tab's card, beneath it, once the pointer has rested
        // the card hangs below the strip, under the tab the pointer rests
        // on: it hangs from the strip's own bottom edge, not from the
        // tab's, so whatever the tab's shape and however the layers fall
        // it can never cover the tab it belongs to. Its layer is under
        // the strip's too (the title bar is deferred at 1 in full
        // screen), and over the window, which it is a card on.
        let card = hovered.map(|(u, b)| {
            let lines = self.tab_status(&u, cx);
            let el = tab_card(&lines, self.overlay_bounds.clone());
            gpui::deferred(gpui::anchored().position(gpui::point(b.origin.x, px(TITLEBAR_HEIGHT + 4.))).child(el)).with_priority(0)
        });
        div()
            .id("titlebar")
            .relative()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            // room for the traffic lights, which full screen has none of
            .pl(px(if self.fullscreen { 12. } else { 78. }))
            // the `+` is not flush with the edge: the bar keeps a margin
            // on the right, as ghostty's does
            .pr(px(10.))
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
            .when_some(title, |d, el| d.child(el))
            .child(button)
            .when_some(card, |d, c| d.child(c))
            .when_some(floating, |d, f| d.child(f))

    }

    /// The picker, when open: the window dimmed behind it and the dialog
    /// in the middle -- a field, and under it the rows, each a thing to
    /// do with the words for it at the right.
    pub fn selector_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let sel = self.selector.as_ref()?;
        let rows = sel.rows();
        let t = crate::theme::theme();
        let hint = if let Some(u) = &sel.renaming {
            format!("New name for {}…", u.session)
        } else {
            "Search a session, or type a name to make one…".to_string()
        };
        let caret_on = sel.caret_visible();
        // the field: a glass at the left, then what is typed
        let field = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(12.))
            .text_size(px(15.))
            .font_family(UI_FONT)
            .child(div().flex_none().text_color(rgb(t.panel_dim)).child("⌕"))
            .child(div().flex_1().min_w_0().child(crate::field::field_view(&sel.filter, caret_on, &hint, true)));
        let mut list = div().id("picker-rows").flex().flex_col().px(px(8.)).pb(px(8.)).max_h(px(420.)).overflow_y_scroll();
        for (i, row) in rows.iter().enumerate() {
            let picked = i == sel.cursor && row.pickable();
            let dim = if picked { t.panel_chosen_text } else { t.panel_dim };
            let ink = if picked { t.panel_chosen_text } else { t.panel_text };
            let r = row.clone();
            let mut el = div()
                .id(("picker-row", i))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(7.))
                .text_size(px(14.))
                .font_family(UI_FONT)
                .text_color(rgb(ink))
                .when(picked, |d| d.bg(rgb(t.panel_chosen_bg)))
                .when(!picked && row.pickable(), |d| d.hover(|s| s.bg(rgb(t.panel_hover))))
                // the glyph, in a ring as the screenshots have it
                .child(
                    div()
                        .flex_none()
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(dim))
                        .text_size(px(11.))
                        .text_color(rgb(dim))
                        .child(row.glyph()),
                )
                .child(div().flex_none().overflow_hidden().text_ellipsis().whitespace_nowrap().child(row.title()))
                .when_some(row.where_(), |d, w| d.child(div().flex_none().text_size(px(12.)).text_color(rgb(dim)).child(w)))
                // what picking it does, in the dim words at the right of
                // the name, as the screenshots have it
                .child(div().flex_none().text_size(px(12.)).text_color(rgb(dim)).child(row.action()))
                .child(div().flex_1());
            if row.pickable() {
                el = el.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.choose(r.clone(), window, cx);
                        cx.stop_propagation();
                    }),
                );
            }
            list = list.child(el);
        }
        if rows.is_empty() && sel.connect.is_none() {
            let what = if sel.renaming.is_some() { "Type a name" } else { "Nothing matches" };
            list = list.child(div().px(px(10.)).py(px(6.)).text_size(px(13.)).font_family(UI_FONT).text_color(rgb(t.panel_dim)).child(what));
        }
        let mut panel = div()
            .w(px(760.))
            .max_w_full()
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .rounded(px(12.))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.overlay_mark())
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()));
        panel = match &sel.connect {
            Some(form) => panel.child(self.connect_form(form, caret_on, cx)),
            None => panel.child(field).child(list),
        };
        // the window behind it goes quiet: the dialog is the whole of
        // what there is to do while it is up
        let veil = div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .bg(gpui::rgba(if crate::theme::is_dark() { 0x00000099 } else { 0x33332899 }))
            .justify_center()
            .child(panel)
            // a little above the middle, where the eye goes first
            .child(div().flex_none().h(px(110.)));
        Some(deferred(veil).with_priority(2))
    }
}

#[cfg(test)]
mod picker_tests {
    use super::*;

    fn picker() -> Selector {
        let local = Host::local();
        let box_ = Host { provider: "sprite".into(), arg: "devvm".into() };
        let si = |l: &str| SessionInfo { id: format!("id-{l}"), label: l.to_string() };
        let mut sessions = HashMap::new();
        sessions.insert(local.clone(), Loading::Ready(vec![si("default"), si("notes"), si("anotherfoobaz")]));
        sessions.insert(box_.clone(), Loading::Seeded(vec![si("work")]));
        Selector {
            tabs: Vec::new(),
            closed: Vec::new(),
            filter: crate::field::LineEdit::new(),
            cursor: 0,
            moved: false,
            hosts: vec![local, box_],
            sessions,
            current: SessionUrl::local("notes"),
            renaming: None,
            connect: None,
            epoch: 1,
            caret_since: std::time::Instant::now(),
        }
    }

    /// What the rows say, as the eye reads them: the name and what
    /// picking it does.
    fn shape(sel: &Selector) -> Vec<String> {
        sel.rows().iter().map(|r| format!("{} {}", r.title(), r.action())).collect()
    }

    #[test]
    fn the_picker_offers_every_way_into_a_session() {
        // nothing typed: the tabs to go to, the sessions to open, a host
        // to add at the bottom; this window's own session is not offered
        let mut sel = picker();
        sel.tabs = vec![SessionUrl::local("default"), SessionUrl::local("notes")];
        assert_eq!(
            shape(&sel),
            vec![
                "default Go to session",
                "anotherfoobaz Open session",
                "work Open session",
                "Add a host… Add a host",
            ]
        );
        // a name typed: what it matches, then the making of it on every
        // host that has no session by that name, then the host row
        sel.filter = "another".into();
        assert_eq!(
            shape(&sel),
            vec![
                "anotherfoobaz Open session",
                "another Create on local",
                "another Create on devvm (sprite)",
                "Add a host… Add a host",
            ]
        );
        // a name a session already has on one host: only the other hosts
        // offer to make it
        sel.filter = "work".into();
        assert_eq!(shape(&sel), vec!["work Open session", "work Create on local", "Add a host… Add a host"]);
        // one closed lately is there to open, and is not offered twice
        let mut sel = picker();
        sel.closed = vec![SessionUrl::local("default"), SessionUrl::local("gone")];
        assert!(shape(&sel).contains(&"gone Open session".to_string()));
        assert_eq!(shape(&sel).iter().filter(|r| r.starts_with("default ")).count(), 1);
        // a name that is no label: said, and nothing is offered to make
        let mut sel = picker();
        sel.filter = "Not A Label".into();
        let rows = sel.rows();
        assert!(matches!(rows[0], Row::Note(_)), "{rows:?}");
        assert!(!rows.iter().any(|r| matches!(r, Row::Create(..))));
        // the cursor lands on the first row worth landing on, and a host
        // answering leaves it on the row it was on
        let mut sel = picker();
        sel.land_on_current();
        assert!(sel.rows()[sel.cursor].pickable());
        sel.cursor = sel.rows().iter().position(|r| matches!(r, Row::Open(u) if u.session == "work")).unwrap();
        sel.moved = true;
        let devvm = Host { provider: "sprite".into(), arg: "devvm".into() };
        sel.keeping(|s| {
            let si = |l: &str| SessionInfo { id: format!("id-{l}"), label: l.to_string() };
            s.sessions.insert(devvm, Loading::Ready(vec![si("a"), si("b"), si("work")]));
        });
        assert!(matches!(sel.rows()[sel.cursor], Row::Open(ref u) if u.session == "work"), "{:?}", sel.rows()[sel.cursor]);
        // renaming: the field is the new name, and the only row is it
        let mut sel = picker();
        sel.renaming = Some(SessionUrl::local("notes"));
        sel.filter = "scratch".into();
        assert_eq!(sel.rows(), vec![Row::Rename("scratch".into())]);
    }

    #[test]
    fn the_new_host_form_reads_its_fields() {
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
