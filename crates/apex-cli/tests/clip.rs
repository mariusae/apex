//! apex as xclip and wl-paste in a session's terminal, end to end: the
//! binary run by those names, the daemon, and an app (a UI attachment
//! of the test's) whose clipboard they read and write.

use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::ClientMsg;
use apex_server::remote::Remote;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR-not-really-but-binary\n\x00\xff";

fn daemon() -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-clip-test-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

/// The tools, as links to the binary named as each.
fn tools() -> PathBuf {
    let d = std::env::temp_dir().join(format!("apex-clip-tools-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    for t in ["xclip", "wl-paste", "wl-copy", "xsel"] {
        let _ = symlink(env!("CARGO_BIN_EXE_apex"), d.join(t));
    }
    d
}

fn run(sock: &PathBuf, tool: &str, args: &[&str], stdin: &[u8]) -> Output {
    let mut c = Command::new(tools().join(tool))
        .args(args)
        .env("APEX_SOCKET", sock)
        .env("apexsession", "main")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    c.stdin.take().unwrap().write_all(stdin).unwrap();
    c.wait_with_output().unwrap()
}

/// What the app was told, as it heard it.
#[derive(Debug, PartialEq)]
enum Heard {
    Text(String),
    Image(String, Vec<u8>),
}

/// The app: a UI attached to the session, its clipboard an image and some
/// text, answering the daemon's reads as apex's app does (on a paced
/// stream of its own) and saying what programs put there. It sends a key
/// to a terminal when told (`key`), as the user pressing ^V would.
fn app(sock: &PathBuf) -> (Sender<()>, Receiver<Heard>) {
    let (key_tx, key_rx) = channel::<()>();
    let (heard_tx, heard_rx) = channel();
    let mut ui = Remote::connect_as(sock, "main", "app", AttachmentKind::Ui).unwrap();
    std::thread::spawn(move || loop {
        let _ = ui.step(Duration::from_millis(10));
        if key_rx.try_recv().is_ok() {
            let key = apex_server::term::TermKey { key: "v".into(), text: Some("\u{16}".into()), shift: false, control: true, alt: false };
            let _ = ui.link.send(&ClientMsg::TermKey { term: TermId(1), key });
        }
        for (id, want) in std::mem::take(&mut ui.link.clip_asks) {
            let (status, body): (&str, Vec<u8>) = match want.as_str() {
                "TARGETS" => ("200", b"image/png\ntext".to_vec()),
                "image/png" => ("200", PNG.to_vec()),
                "text" => ("200", b"from the mac".to_vec()),
                _ => ("404", Vec::new()),
            };
            let plane = ui.io_plane();
            std::thread::spawn(move || {
                let _ = plane.put_paced("PUT", &format!("apex-clip-answer:{id}"), &[("Status", status)], &body, Duration::from_secs(10));
            });
        }
        for t in std::mem::take(&mut ui.link.clips) {
            let _ = heard_tx.send(Heard::Text(t));
        }
        for (w, b) in std::mem::take(&mut ui.link.clip_puts) {
            let _ = heard_tx.send(Heard::Image(w, b));
        }
    });
    (key_tx, heard_rx)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn claude_codes_paste_reads_the_macs_image_just_after_a_key() {
    let sock = daemon();
    let (key, _) = app(&sock);
    // no key yet: not read, and said so
    let o = run(&sock, "xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"], b"");
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("just after a key"), "{}", text(&o.stderr));
    // ^V pressed in a terminal: Claude Code's two calls, as it makes them
    key.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let o = run(&sock, "xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"], b"");
    assert_eq!((o.status.code(), text(&o.stdout)), (Some(0), "TARGETS\nimage/png\nUTF8_STRING\nTEXT\nSTRING\ntext/plain;charset=utf-8\ntext/plain\n".into()), "{}", text(&o.stderr));
    let o = run(&sock, "xclip", &["-selection", "clipboard", "-t", "image/png", "-o"], b"");
    assert_eq!((o.status.code(), o.stdout.as_slice()), (Some(0), PNG), "{}", text(&o.stderr));
    // and through wl-paste, as on Wayland
    let o = run(&sock, "wl-paste", &["--list-types"], b"");
    assert!(text(&o.stdout).starts_with("image/png\ntext/plain;charset=utf-8\n"), "{}", text(&o.stdout));
    let o = run(&sock, "wl-paste", &["--type", "image/png"], b"");
    assert_eq!(o.stdout, PNG);
    let o = run(&sock, "wl-paste", &[], b"");
    assert_eq!(o.stdout, b"from the mac\n");
    // a target there is none of
    let o = run(&sock, "xclip", &["-selection", "clipboard", "-t", "image/jpeg", "-o"], b"");
    assert_eq!((o.status.code(), text(&o.stderr)), (Some(1), "Error: target image/jpeg not available\n".into()));
}

#[test]
fn what_a_program_copies_goes_to_the_mac() {
    let sock = daemon();
    let (_, heard) = app(&sock);
    let o = run(&sock, "xclip", &["-selection", "clipboard"], b"copied on the host");
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(heard.recv_timeout(Duration::from_secs(5)).unwrap(), Heard::Text("copied on the host".into()));
    let o = run(&sock, "wl-copy", &[], PNG);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(heard.recv_timeout(Duration::from_secs(5)).unwrap(), Heard::Image("image/png".into(), PNG.to_vec()));
}
