//! Files on their way to the host over the I/O plane: paced bodies
//! (`pace.rs`), acknowledged as the daemon takes them.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use apex_server::daemon::Daemon;
use apex_server::proto::IoFrame;
use apex_server::remote::Remote;

fn daemon() -> PathBuf {
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
