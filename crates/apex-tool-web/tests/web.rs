//! The Web tool against a daemon: Web and Newweb anywhere, its pages
//! owned and their history kept, a page nobody owns taken over.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::WindowEvent;
use apex_server::remote::Remote;
use apex_server::Proposal;

fn daemon() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-web-test-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

/// The session with the Web tool attached (its rules in), and a client.
fn session() -> (PathBuf, Remote) {
    let sock = daemon();
    let s = sock.clone();
    std::thread::spawn(move || apex_tool_web::run(&s, "main"));
    let mut c = Remote::connect_as(&sock, "main", "client", AttachmentKind::Tool).unwrap();
    assert!(until(&mut c, |c| c.node.state.meta.rules.values().any(|r| r.rule.verb == "Newweb")), "the tool's rules");
    (sock, c)
}

fn until(c: &mut Remote, mut done: impl FnMut(&Remote) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done(c) {
            return true;
        }
        let _ = c.step(Duration::from_millis(20));
    }
    done(c)
}

fn pages(c: &Remote) -> Vec<(WindowId, String)> {
    c.node.state.windows.values().filter(|w| w.body == Body::Page(Source::Url)).map(|w| (w.id, w.path.clone())).collect()
}

fn web_owned(c: &Remote, w: WindowId) -> bool {
    let owner = c.node.state.window(w).ok().and_then(|x| x.owner);
    owner.is_some_and(|a| c.node.state.meta.attachments.get(&a).is_some_and(|x| x.name == "web"))
}

#[test]
fn web_opens_a_page_on_the_url_given_or_selected_and_owns_it() {
    let (_sock, mut c) = session();
    // typed after the word, in the top row: a URL as it is
    c.propose(Proposal::Exec { ctx: ExecCtx::Top, text: "Web https://example.com/".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |c| pages(c).iter().any(|(w, p)| p == "https://example.com/" && web_owned(c, *w))), "{:?}", pages(&c));
    // selected in a text window: a bare path is the host's file there
    let col = c.node.state.layout.cols[0].id;
    let here = std::env::temp_dir().join(format!("apex-web-here-{}", std::process::id()));
    std::fs::create_dir_all(&here).unwrap();
    let here = here.display().to_string();
    let t = c.propose(Proposal::NewWindow { col, name: format!("{here}/notes.txt"), scratch: false, label: None, diagnostic: false }, Duration::from_secs(5)).unwrap().unwrap();
    let b = c.node.state.window(t).unwrap().body_buffer().unwrap();
    let version = c.node.state.buffer(b).unwrap().version;
    c.propose(Proposal::ReplaceRange { select: false, dir: None, buffer: b, version, q0: 0, q1: 0, text: "see doc.html\n".into() }, Duration::from_secs(5)).unwrap();
    c.propose(Proposal::Select { view: ViewId::Body(t), q0: 4, q1: 12 }, Duration::from_secs(5)).unwrap();
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(t), text: "Web".into() }, Duration::from_secs(5)).unwrap();
    let want = format!("apexfile://{here}/doc.html");
    assert!(until(&mut c, |c| pages(c).iter().any(|(_, p)| *p == want)), "{:?}", pages(&c));
    let _ = std::fs::remove_dir_all(&here);
    // Newweb: the address it is given
    c.propose(Proposal::Exec { ctx: ExecCtx::Top, text: "Newweb https://example.org/".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |c| pages(c).iter().any(|(_, p)| p == "https://example.org/")), "{:?}", pages(&c));
}

#[test]
fn a_pages_history_is_the_tools_back_and_fwd_answered_there() {
    let (_sock, mut c) = session();
    c.propose(Proposal::Exec { ctx: ExecCtx::Top, text: "Newweb https://example.com/a".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |c| pages(c).iter().any(|(w, _)| web_owned(c, *w))));
    let (w, _) = pages(&c)[0].clone();
    // the page goes to b (a link followed: the client says so, the log follows)
    c.link.window_event(w, WindowEvent::Navigated { url: "https://example.com/b".into() });
    c.propose(Proposal::Navigate { window: w, url: "https://example.com/b".into() }, Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // Back in its tag: the tool's word, the page back at a
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Back".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |c| c.node.window_path(w) == "https://example.com/a"), "{}", c.node.window_path(w));
    c.link.window_event(w, WindowEvent::Navigated { url: "https://example.com/a".into() });
    std::thread::sleep(Duration::from_millis(200));
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Fwd".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |c| c.node.window_path(w) == "https://example.com/b"), "{}", c.node.window_path(w));
    // Get: a reload every client follows
    let before = c.node.state.window(w).unwrap().reload;
    c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Get".into() }, Duration::from_secs(5)).unwrap();
    assert!(until(&mut c, |c| c.node.state.window(w).unwrap().reload > before));
}

#[test]
fn a_page_nobody_owns_is_taken_over() {
    let (_sock, mut c) = session();
    let col = c.node.state.layout.cols[0].id;
    let w = c.propose(Proposal::open_url(col, "https://example.net/"), Duration::from_secs(5)).unwrap().unwrap();
    assert!(until(&mut c, |c| web_owned(c, w)));
}
