//! apex-tuid: the view server behind the TermKit UI.
//!
//! It attaches to a session exactly as the gpui client does, keeps the
//! replica, and serves one JSON view model per change over its stdout,
//! taking the pointer and the keyboard back on its stdin. The UI itself
//! — every window, tag, terminal, markdown page and finder — is the
//! TermKit app in `swift/ApexTUI`.
//!
//! `apex-tuid [--session S] [--attach SOCKET] [--via CMD] [--remote DEST]`

use apex_tui::{input, model, ui, wire};

use std::io::{BufReader, Write};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use apex_core::AttachmentKind;
use apex_server::providers::SessionUrl;
use apex_server::remote::Link;

use input::Event;
use ui::Ui;

/// A wake the link's reader thread pulls, so the loop sleeps until
/// something happens rather than polling.
#[derive(Default)]
struct Bell {
    m: Mutex<bool>,
    cv: Condvar,
}

impl Bell {
    fn ring(&self) {
        *self.m.lock().unwrap() = true;
        self.cv.notify_all();
    }
    /// Wait for a ring, or for the timeout; true if one came.
    fn wait(&self, d: Duration) -> bool {
        let g = self.m.lock().unwrap();
        let (mut g, _) = self.cv.wait_timeout_while(g, d, |rung| !*rung).unwrap();
        std::mem::take(&mut *g)
    }
}

fn usage() -> ! {
    eprintln!("usage: apex-tuid [--session NAME] [--attach SOCKET] [--via CMD] [--remote DEST] [files...]");
    std::process::exit(2)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut session = String::new();
    let mut socket: Option<String> = None;
    let mut via: Option<String> = None;
    let mut remote: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--session" => session = args.next().unwrap_or_else(|| usage()),
            "--attach" => socket = args.next(),
            "--via" => via = Some(args.next().unwrap_or_else(|| usage())),
            "--remote" => remote = Some(args.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            _ if a.starts_with('-') => usage(),
            _ => files.push(a),
        }
    }
    if session.is_empty() {
        session = "local".into();
    }

    let bell = Arc::new(Bell::default());
    let wake: apex_server::remote::Wake = {
        let b = bell.clone();
        Arc::new(move || b.ring())
    };

    let (link, log, mut node) = match connect(&session, socket.as_deref(), via.as_deref(), remote.as_deref(), wake.clone()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("apex-tuid: attach: {e}");
            std::process::exit(1);
        }
    };
    let mut log = log;
    // a session with nothing in it gets its first column, as the gpui
    // client's attach does
    let col = match node.state.layout.cols.last() {
        Some(c) => c.id,
        None => match node.init_session(&mut log) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("apex-tuid: init: {e}");
                std::process::exit(1);
            }
        },
    };

    let mut ui = Ui::new(node, log, link, session);
    for f in files {
        ui.open(col, &f);
    }

    let events = wire::reader::<Event>(BufReader::new(std::io::stdin()), Some(wake));
    let out = wire::writer::<model::Frame>(std::io::stdout());

    ui.measure(80, 24);
    ui.sync();
    let mut last: Option<model::Frame> = None;
    loop {
        // everything waiting, then one frame
        loop {
            match events.recv_timeout(Duration::from_millis(0)) {
                Ok(ev) => {
                    let quit = ev == Event::Quit;
                    ui.event(ev);
                    if quit {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if !ui.poll() && ui.quit {
            break;
        }
        let f = ui.frame();
        // the seq always differs, so compare what is drawn
        let changed = match &last {
            Some(prev) => !same(prev, &f),
            None => true,
        };
        if changed {
            if out.send(f.clone()).is_err() {
                break;
            }
            last = Some(f);
        }
        if ui.quit {
            break;
        }
        bell.wait(Duration::from_millis(50));
    }
    let _ = std::io::stdout().flush();
}

/// Two frames draw the same thing (their sequence numbers aside).
fn same(a: &model::Frame, b: &model::Frame) -> bool {
    a.cols == b.cols
        && a.rows == b.rows
        && a.title == b.title
        && a.top == b.top
        && a.columns == b.columns
        && a.overlay == b.overlay
        && a.notification == b.notification
        && a.connected == b.connected
        && a.fenced == b.fenced
        && b.warp.is_none()
        && b.snarf.is_none()
}

/// Start the local daemon if nothing answers on its socket, as the gpui
/// client's `ensure_daemon` does.
fn ensure_daemon(socket: &std::path::Path) -> std::io::Result<()> {
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return Ok(());
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        candidates.push(exe.with_file_name("apex"));
    }
    candidates.push(std::path::PathBuf::from("apex"));
    let mut last = std::io::Error::other("no apex command");
    for apex in candidates {
        match apex_server::daemon::spawn_server(&apex, socket, apex_server::providers::DEFAULT_SESSION) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn connect(
    session: &str,
    socket: Option<&str>,
    via: Option<&str>,
    remote: Option<&str>,
    wake: apex_server::remote::Wake,
) -> std::io::Result<(Link, apex_core::Log, apex_core::Node)> {
    if let Some(cmd) = via {
        let (stdin, stdout, closer) = apex_server::remote::bridge_child(cmd)?;
        return Link::over_streams_creating(Box::new(stdout), Box::new(stdin), Some(closer), session, session, "apex", AttachmentKind::Ui, Some(wake));
    }
    if let Some(dest) = remote {
        let url = SessionUrl::parse(dest).ok_or_else(|| std::io::Error::other("bad destination"))?;
        let d = url.dest().ok_or_else(|| std::io::Error::other("no destination"))?;
        apex_server::providers::deploy(&d)?;
        let cmd = apex_server::providers::attach_command(&d, url.session_ref())?;
        let (stdin, stdout, closer) = apex_server::remote::bridge_child(&cmd)?;
        return Link::over_streams_creating(Box::new(stdout), Box::new(stdin), Some(closer), url.session_ref(), &url.session, "apex", AttachmentKind::Ui, Some(wake));
    }
    let path = socket.map(std::path::PathBuf::from).unwrap_or_else(apex_server::daemon::default_socket);
    ensure_daemon(&path)?;
    let stream = std::os::unix::net::UnixStream::connect(&path)?;
    let w = stream.try_clone()?;
    let closer = stream.try_clone()?;
    Link::over_streams_creating(
        Box::new(stream),
        Box::new(w),
        Some(Box::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        })),
        session,
        session,
        "apex",
        AttachmentKind::Ui,
        Some(wake),
    )
}
