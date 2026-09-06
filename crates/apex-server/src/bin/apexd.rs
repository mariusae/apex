//! `apexd [--socket PATH] [--session NAME]`: run one session's daemon.

use std::path::PathBuf;

use apex_server::daemon::{default_socket, Daemon};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut socket = default_socket();
    let mut session = "local".to_string();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--socket" => socket = PathBuf::from(args.next().expect("--socket PATH")),
            "--session" => session = args.next().expect("--session NAME"),
            other => {
                eprintln!("apexd: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    if let Some(d) = socket.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    eprintln!("apexd: session {session} on {}", socket.display());
    if let Err(e) = Daemon::run(&socket, &session) {
        eprintln!("apexd: {e}");
        std::process::exit(1);
    }
}
