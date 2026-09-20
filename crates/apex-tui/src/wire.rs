//! The UI link: one JSON value per line, each way.
//!
//! The UI is a separate process (TermKit is Swift), so the two halves
//! talk over a pipe rather than a function call. Line-delimited JSON is
//! enough: a frame is small, and the round trip is a keystroke's.

use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, Sender};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Read values from `r` onto a channel, until the stream ends.
pub fn reader<T: DeserializeOwned + Send + 'static>(r: impl BufRead + Send + 'static, wake: Option<apex_server::remote::Wake>) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in r.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<T>(&line) {
                Ok(v) => {
                    if tx.send(v).is_err() {
                        break;
                    }
                    if let Some(w) = &wake {
                        w();
                    }
                }
                Err(e) => eprintln!("apex-tuid: bad event: {e}: {line}"),
            }
        }
    });
    rx
}

/// Write values from a channel to `w`, one line each.
pub fn writer<T: Serialize + Send + 'static>(mut w: impl Write + Send + 'static) -> Sender<T> {
    let (tx, rx) = std::sync::mpsc::channel::<T>();
    std::thread::spawn(move || {
        while let Ok(v) = rx.recv() {
            let Ok(s) = serde_json::to_string(&v) else { continue };
            if w.write_all(s.as_bytes()).is_err() || w.write_all(b"\n").is_err() || w.flush().is_err() {
                break;
            }
        }
    });
    tx
}
