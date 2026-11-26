//! Files on their way to the host over the I/O plane: paced bodies
//! (`pace.rs`), acknowledged as the daemon takes them.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use apex_server::daemon::Daemon;
use apex_server::proto::IoFrame;
use apex_server::remote::Remote;

/// The home the daemons here keep `~/.apex` in: one for this test
/// binary, set before any daemon starts, so `~/.apex/drops` is its own.
fn home() -> &'static PathBuf {
    static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let h = std::env::temp_dir().join(format!("apex-transfer-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(&h).unwrap();
        // SAFETY: before any daemon thread reads it, once
        unsafe { std::env::set_var("HOME", &h) };
        h
    })
}

fn daemon() -> PathBuf {
    home();
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("apex-transfer-test-{}-{n}.sock", std::process::id()));
    let p = path.clone();
    std::thread::spawn(move || Daemon::run_with(&p, "main", None).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    path
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("apex-transfer-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The answer on a stream: its status, headers and body.
fn answer(rx: &Receiver<IoFrame>) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut head, mut body) = (None, Vec::new());
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("an answer") {
            IoFrame::Response { status, headers } => head = Some((status, headers)),
            IoFrame::Body(b) => body.extend_from_slice(&b),
            IoFrame::End => break,
            IoFrame::Reset { reason } => panic!("reset: {reason}"),
            IoFrame::Request { .. } => {}
        }
    }
    let (status, headers) = head.expect("a response");
    (status, headers, body)
}

fn bytes(n: usize) -> Vec<u8> {
    (0..n as u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect()
}

#[test]
fn a_paced_body_goes_as_acknowledged_and_arrives_whole() {
    let sock = daemon();
    let r = Remote::connect(&sock, "main", "uploader").unwrap();
    let plane = r.io_plane();
    let dir = scratch("whole");
    let path = dir.join("big.bin");
    let data = bytes(3_000_000);
    let (stream, rx) = plane.open("PUT", &format!("file://{}", path.display()), &[("Paced", "1")]);
    let mut went = Vec::new();
    plane.send_paced(stream, &mut &data[..], data.len() as u64, &|| false, &mut |n| went.push(n)).unwrap();
    plane.end(stream);
    let (status, _, said) = answer(&rx);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&said));
    assert_eq!(std::fs::read(&path).unwrap(), data, "every byte, in order");
    // progress told as it went, ending at all of it
    assert!(went.len() > 10 && went.windows(2).all(|w| w[0] < w[1]) && *went.last().unwrap() == data.len() as u64);
    // it went only as acknowledged: had nothing come back, it would have
    // stopped at the starting budget, far short of three megabytes
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_paced_body_stops_when_told() {
    let sock = daemon();
    let r = Remote::connect(&sock, "main", "uploader").unwrap();
    let plane = r.io_plane();
    let dir = scratch("stop");
    let path = dir.join("never.bin");
    let data = bytes(4_000_000);
    let (stream, _rx) = plane.open("PUT", &format!("file://{}", path.display()), &[("Paced", "1")]);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let r = plane.send_paced(stream, &mut &data[..], data.len() as u64, &|| stop.load(std::sync::atomic::Ordering::Relaxed), &mut |n| {
        if n > 200_000 {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    assert_eq!(r, Err("stopped".to_string()));
    plane.reset(stream, "stopped");
    // a body ended short of its length is an error too
    let (stream, _rx) = plane.open("PUT", &format!("file://{}", dir.join("short.bin").display()), &[("Paced", "1")]);
    let r = plane.send_paced(stream, &mut &data[..1000], 5000, &|| false, &mut |_| {});
    assert!(r.is_err_and(|e| e.contains("ended at 1000")), "short");
    plane.reset(stream, "short");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The first answer on a stream that has more to come: its status and
/// headers.
fn head(rx: &Receiver<IoFrame>) -> (u16, Vec<(String, String)>) {
    match rx.recv_timeout(Duration::from_secs(10)).expect("an answer") {
        IoFrame::Response { status, headers } => (status, headers),
        f => panic!("not a response: {f:?}"),
    }
}

fn header<'a>(headers: &'a [(String, String)], k: &str) -> Option<&'a str> {
    headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.as_str())
}

#[test]
fn a_dropped_file_lands_in_the_drops_folder_by_its_name_once() {
    let sock = daemon();
    let r = Remote::connect(&sock, "main", "uploader").unwrap();
    let plane = r.io_plane();
    let data = bytes(1_200_000);
    let len = data.len().to_string();
    let want = home().join(".apex/drops/k7f3/Screenshot 2026-11-26 at 15.20.png");
    let (stream, rx) = plane.open("PUT", "apex-drop:Screenshot 2026-11-26 at 15.20.png", &[("Key", "k7f3"), ("Length", &len), ("Paced", "1")]);
    // where it will be, before a byte of it goes
    let (status, headers) = head(&rx);
    assert_eq!((status, header(&headers, "Location")), (100, Some(want.to_str().unwrap())));
    plane.send_paced(stream, &mut &data[..], data.len() as u64, &|| false, &mut |_| {}).unwrap();
    plane.end(stream);
    let (status, headers, said) = answer(&rx);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&said));
    assert_eq!(header(&headers, "Location"), Some(want.to_str().unwrap()));
    assert_eq!(std::fs::read(&want).unwrap(), data);
    // the same file again: it is there, nothing to send
    let (_, rx) = plane.open("PUT", "apex-drop:Screenshot 2026-11-26 at 15.20.png", &[("Key", "k7f3"), ("Length", &len), ("Paced", "1")]);
    let (status, headers, _) = answer(&rx);
    assert_eq!((status, header(&headers, "Have"), header(&headers, "Location")), (200, Some("1"), Some(want.to_str().unwrap())));
}

#[test]
fn a_drop_stopped_or_refused_leaves_nothing() {
    let sock = daemon();
    let r = Remote::connect(&sock, "main", "uploader").unwrap();
    let plane = r.io_plane();
    let data = bytes(2_000_000);
    let len = data.len().to_string();
    let dir = home().join(".apex/drops/stopped1");
    let (stream, rx) = plane.open("PUT", "apex-drop:big.bin", &[("Key", "stopped1"), ("Length", &len), ("Paced", "1")]);
    assert_eq!(head(&rx).0, 100);
    let _ = plane.send_paced(stream, &mut &data[..300_000], 300_000, &|| false, &mut |_| {});
    plane.reset(stream, "stopped");
    // the daemon has taken the reset once it answers what comes after it
    let _ = plane.fetch("GET", &format!("file://{}", home().display()), &[], None, Duration::from_secs(5));
    let left: Vec<_> = std::fs::read_dir(&dir).map(|r| r.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "nothing of it: {left:?}");
    // a name that would leave the folder, a missing key: refused
    for (url, hs) in [("apex-drop:../escape", vec![("Key", "k"), ("Length", "1")]), ("apex-drop:f", vec![("Length", "1")]), ("apex-drop:f", vec![("Key", "a/b"), ("Length", "1")])] {
        let (_, rx) = plane.open("PUT", url, &hs);
        let (status, _, said) = answer(&rx);
        assert_eq!(status, 400, "{url} {hs:?}: {}", String::from_utf8_lossy(&said));
    }
    assert!(!home().join(".apex/escape").exists());
}

#[test]
fn drop_file_sends_a_file_once_and_says_where_it_landed() {
    let sock = daemon();
    let r = Remote::connect(&sock, "main", "uploader").unwrap();
    let plane = r.io_plane();
    let src = scratch("dropfile").join("photo 1.jpg");
    let data = bytes(900_000);
    std::fs::write(&src, &data).unwrap();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut went = 0;
    let at = plane.drop_file(&src, data.len() as u64, "dropfile1", &stop, &mut |n| went = n).unwrap();
    assert_eq!(at, home().join(".apex/drops/dropfile1/photo 1.jpg").display().to_string());
    assert_eq!(went, data.len() as u64);
    assert_eq!(std::fs::read(&at).unwrap(), data);
    // again: there already, nothing sent
    let mut again = 0;
    assert_eq!(plane.drop_file(&src, data.len() as u64, "dropfile1", &stop, &mut |n| again = n), Ok(at));
    assert_eq!(again, 0, "not a byte the second time");
    // stopped as it goes: nothing there, and it says so
    let mut stopped_at = 0;
    let r = plane.drop_file(&src, data.len() as u64, "dropfile2", &stop, &mut |n| {
        stopped_at = n;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    assert_eq!(r, Err("stopped".into()));
    assert!(stopped_at > 0 && stopped_at < data.len() as u64);
    let _ = plane.fetch("GET", &format!("file://{}", home().display()), &[], None, Duration::from_secs(5));
    assert!(!home().join(".apex/drops/dropfile2/photo 1.jpg").exists());
}
