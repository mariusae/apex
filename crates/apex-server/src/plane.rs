//! The I/O plane from a client's side, for threads (WEB.md §1, §2.3):
//! a `IoPlane` handle opens streams and receives their frames without
//! the link's owner pumping them; a localhost `CONNECT` proxy on it is
//! what a web view's traffic goes through; `fetch` is one request.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::proto::{ClientMsg, IoFrame};
use crate::remote::Outbound;

/// Stream ids a link hands out (odd, increasing), shared with threads.
#[derive(Clone, Default)]
pub struct IoIds(Arc<AtomicU32>);

impl IoIds {
    pub fn new() -> IoIds {
        IoIds(Arc::new(AtomicU32::new(1)))
    }

    pub fn next(&self) -> u32 {
        self.0.fetch_add(2, Ordering::Relaxed)
    }
}

/// Where a stream's frames go when a thread waits for them: the link's
/// reader delivers to a registered sink instead of the owner's queue.
#[derive(Clone, Default)]
pub struct IoSinks(Arc<Mutex<HashMap<u32, Sender<IoFrame>>>>);

impl IoSinks {
    pub fn new() -> IoSinks {
        IoSinks(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Deliver a frame to its sink; false if the stream has none.
    pub fn deliver(&self, stream: u32, frame: &IoFrame) -> bool {
        let mut m = self.0.lock().unwrap();
        match m.get(&stream) {
            Some(tx) => {
                if tx.send(frame.clone()).is_err() {
                    m.remove(&stream);
                }
                true
            }
            None => false,
        }
    }
}

/// A link's I/O plane as a thread sees it: open a stream, send on it,
/// receive its frames.
#[derive(Clone)]
pub struct IoPlane {
    out: Outbound,
    ids: IoIds,
    sinks: IoSinks,
}

impl IoPlane {
    pub fn new(out: Outbound, ids: IoIds, sinks: IoSinks) -> IoPlane {
        IoPlane { out, ids, sinks }
    }

    /// Open a stream with a request; the frames that come back arrive on
    /// the receiver, until `close`.
    pub fn open(&self, method: &str, url: &str, headers: &[(&str, &str)]) -> (u32, Receiver<IoFrame>) {
        let stream = self.ids.next();
        let (tx, rx) = channel();
        self.sinks.0.lock().unwrap().insert(stream, tx);
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let _ = self.out.send(&ClientMsg::Io { stream, frame: IoFrame::Request { method: method.to_string(), url: url.to_string(), headers } });
        (stream, rx)
    }

    pub fn send_body(&self, stream: u32, bytes: &[u8]) {
        for chunk in bytes.chunks(256 * 1024) {
            let _ = self.out.send(&ClientMsg::Io { stream, frame: IoFrame::Body(chunk.to_vec()) });
        }
    }

    pub fn end(&self, stream: u32) {
        let _ = self.out.send(&ClientMsg::Io { stream, frame: IoFrame::End });
    }

    /// Done with a stream: its sink goes (the server side is told with
    /// `end` or `reset` by the caller as fits).
    pub fn close(&self, stream: u32) {
        self.sinks.0.lock().unwrap().remove(&stream);
    }

    pub fn reset(&self, stream: u32, reason: &str) {
        let _ = self.out.send(&ClientMsg::Io { stream, frame: IoFrame::Reset { reason: reason.to_string() } });
        self.close(stream);
    }

    /// One whole request: the status, the headers and the body, or why not.
    pub fn fetch(&self, method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>, timeout: Duration) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
        let (stream, rx) = self.open(method, url, headers);
        if let Some(b) = body {
            self.send_body(stream, b);
            self.end(stream);
        } else if url.starts_with("http") {
            self.end(stream);
        }
        let r = Self::collect(&rx, timeout);
        self.close(stream);
        r
    }

    fn collect(rx: &Receiver<IoFrame>, timeout: Duration) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
        let deadline = std::time::Instant::now() + timeout;
        let mut head = None;
        let mut body = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("timed out waiting for the host".into());
            }
            match rx.recv_timeout(left) {
                Ok(IoFrame::Response { status, headers }) => head = Some((status, headers)),
                Ok(IoFrame::Body(b)) => body.extend_from_slice(&b),
                Ok(IoFrame::End) => break,
                Ok(IoFrame::Reset { reason }) => return Err(reason),
                Ok(IoFrame::Request { .. }) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err("timed out waiting for the host".into()),
                Err(_) => return Err("connection closed".into()),
            }
        }
        let (status, headers) = head.ok_or("no response")?;
        Ok((status, headers, body))
    }
}

/// A `CONNECT` proxy on localhost whose tunnels are `CONNECT` streams on
/// the plane: point a web view at it and its traffic goes out through
/// the host. The port; it serves until the plane's link closes.
pub fn start_connect_proxy(plane: IoPlane) -> io::Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    let port = l.local_addr()?.port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let plane = plane.clone();
            std::thread::spawn(move || proxy_one(plane, s));
        }
    });
    Ok(port)
}

fn proxy_one(plane: IoPlane, mut c: TcpStream) {
    // the request head, byte by byte to the blank line
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while c.read(&mut b).unwrap_or(0) == 1 {
        head.push(b[0]);
        if head.ends_with(b"\r\n\r\n") || head.len() > 64 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let line = text.lines().next().unwrap_or("").to_string();
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("").to_string());
    if method != "CONNECT" || target.is_empty() {
        let _ = c.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let (stream, rx) = plane.open("CONNECT", &target, &[]);
    // the answer: up, or not
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(IoFrame::Response { status: 200, .. }) => {}
        Ok(IoFrame::Response { status, .. }) => {
            let _ = c.write_all(format!("HTTP/1.1 502 Bad Gateway\r\nX-Apex-Status: {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
            plane.close(stream);
            return;
        }
        _ => {
            let _ = c.write_all(b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            plane.reset(stream, "no answer");
            return;
        }
    }
    if c.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").is_err() {
        plane.reset(stream, "client gone");
        return;
    }
    // bytes in: the socket to the stream, until the client is done
    let Ok(mut reader) = c.try_clone() else { return };
    let plane2 = plane.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => plane2.send_body(stream, &buf[..n]),
            }
        }
        plane2.end(stream);
    });
    // bytes out: the stream to the socket, until the far end is done
    loop {
        match rx.recv() {
            Ok(IoFrame::Body(b)) => {
                if c.write_all(&b).is_err() {
                    break;
                }
            }
            Ok(IoFrame::End) | Ok(IoFrame::Reset { .. }) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = c.shutdown(std::net::Shutdown::Both);
    plane.close(stream);
}

/// A content type from a path's extension, for what a web view fetches.
pub fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "xml" => "application/xml",
        "md" | "txt" | "" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
