//! `apex-bench [--socket PATH] [--via CMD]`: measure the attach protocol.
//! Without `--socket` it starts a daemon on a thread of its own, so the
//! numbers are for two threads of one process talking through the kernel;
//! with one, it talks to a running `apexd`. `--via CMD` attaches through
//! CMD's stdin/stdout instead — `apex attach --stdio` locally stands in for
//! `ssh host apex attach --stdio`, adding the two pipe hops ssh would.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::Daemon;
use apex_server::proto::{ClientMsg, ServerMsg};
use apex_server::remote::Remote;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut socket: Option<PathBuf> = None;
    let mut via: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(args.next().expect("--socket PATH"))),
            "--via" => via = Some(args.next().expect("--via CMD")),
            other => {
                eprintln!("apex-bench: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let socket = socket.unwrap_or_else(|| {
        let path = std::env::temp_dir().join(format!("apex-bench-{}.sock", std::process::id()));
        let p = path.clone();
        std::thread::spawn(move || Daemon::run(&p, "main").unwrap());
        while !path.exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
        path
    });

    let connect = |name: &str| -> Remote {
        match &via {
            Some(cmd) => Remote::via(cmd, "main", name, AttachmentKind::Ui).unwrap(),
            None => Remote::connect(&socket, "main", name).unwrap(),
        }
    };
    match &via {
        Some(cmd) => println!("via: {cmd}"),
        None => println!("socket: {}", socket.display()),
    }
    println!();

    // ---- protocol floor: ping round trip ----------------------------------------
    let mut c = connect("bench");
    let col = c.node.state.layout.cols[0].id;
    c.flush();
    let mut pings = Vec::new();
    for i in 0..2000u64 {
        let t = Instant::now();
        c.send(&ClientMsg::Ping { t: i });
        loop {
            match c.link.rx.recv().unwrap() {
                ServerMsg::Pong { t: got } if got == i => break,
                m => {
                    c.handle(m);
                }
            }
        }
        pings.push(t.elapsed());
    }
    report("ping round trip (2000)", &pings);

    // ---- keystroke round trip: one Edit entry, flush, wait for its Ack ----------
    let w = c.node.new_window(&mut c.log, col, "bench", "").unwrap();
    let v = ViewId::Body(w);
    let b = c.node.view_buffer(v).unwrap();
    c.flush();
    wait_ack(&mut c, Shard::Buffer(b));
    let mut rtts = Vec::new();
    for _ in 0..5000 {
        let t = Instant::now();
        c.node.insert(&mut c.log, v, "x").unwrap();
        c.flush();
        wait_ack(&mut c, Shard::Buffer(b));
        rtts.push(t.elapsed());
    }
    report("keystroke → ack (5000)", &rtts);

    // ---- throughput: many entries in one flush ------------------------------------
    for &n in &[10_000usize, 100_000] {
        let t = Instant::now();
        for _ in 0..n {
            c.node.insert(&mut c.log, v, "y").unwrap();
        }
        let local = t.elapsed();
        c.flush();
        wait_ack(&mut c, Shard::Buffer(b));
        let total = t.elapsed();
        println!(
            "{n} keystrokes, one flush: local {:.1} ms, acked {:.1} ms → {:.0} entries/s",
            local.as_secs_f64() * 1e3,
            total.as_secs_f64() * 1e3,
            n as f64 / total.as_secs_f64()
        );
    }
    println!();

    // ---- attach: connect, get the snapshot, decode it ----------------------------
    for &kb in &[0usize, 100, 1000, 10_000] {
        if kb > 0 {
            let text: String = "0123456789abcdef".repeat(kb * 1024 / 16);
            let w = c.node.new_window(&mut c.log, col, &format!("big-{kb}k"), &text).unwrap();
            let b = c.node.view_buffer(ViewId::Body(w)).unwrap();
            c.flush();
            wait_ack(&mut c, Shard::Buffer(b));
        }
        let bytes: usize = c.node.state.buffers.values().map(|b| b.text.len()).sum();
        let mut ts = Vec::new();
        let mut snap = 0;
        for _ in 0..10 {
            let t = Instant::now();
            let r = connect("probe");
            ts.push(t.elapsed());
            snap = r.node.state.to_snapshot().len();
        }
        // the probes took the leases; take them back for the next round
        c = connect("bench");
        report(&format!("attach with {bytes} runes of text ({} KB snapshot, 10 runs)", snap / 1024), &ts);
    }
    println!();

    // ---- terminal: key → rows arriving ------------------------------------------
    c.node.exec(&mut c.log, ExecCtx::Top, "Newterm").unwrap();
    c.flush();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut term = None;
    while Instant::now() < deadline && term.is_none() {
        let _ = c.step(Duration::from_millis(50));
        term = c.node.state.windows.values().find_map(|w| match w.body {
            Body::Term(t) => Some(t),
            _ => None,
        });
    }
    let Some(t) = term else {
        println!("no terminal window appeared");
        return;
    };
    // let the shell settle
    let settle = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settle {
        let _ = c.step(Duration::from_millis(50));
    }
    let mut ks = Vec::new();
    for i in 0..200 {
        let ch = if i % 2 == 0 { "a" } else { "\x7f" }; // type, then backspace
        let t0 = Instant::now();
        c.send(&ClientMsg::TermKey {
            term: t,
            key: apex_server::TermKey { key: ch.to_string(), text: Some(ch.to_string()), shift: false, control: false, alt: false },
        });
        loop {
            let m = c.link.rx.recv_timeout(Duration::from_secs(2)).expect("term rows");
            let rows = matches!(&m, ServerMsg::Entries { shard, .. } if *shard == Shard::Term(t));
            c.handle(m);
            if rows {
                break;
            }
        }
        ks.push(t0.elapsed());
    }
    report("terminal key → rows on the client (200)", &ks);
}

fn wait_ack(c: &mut Remote, shard: Shard) {
    let want = c.log.last_seq(shard);
    while c.acked(shard) < want {
        c.step(Duration::from_secs(10)).expect("ack");
    }
}

fn report(what: &str, ts: &[Duration]) {
    let mut v: Vec<Duration> = ts.to_vec();
    v.sort();
    let pct = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
    println!("{what}: p50 {:>8}  p90 {:>8}  p99 {:>8}  max {:>8}", fmt(pct(0.5)), fmt(pct(0.9)), fmt(pct(0.99)), fmt(*v.last().unwrap()));
}

fn fmt(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us < 1000.0 {
        format!("{us:.0} µs")
    } else {
        format!("{:.2} ms", us / 1000.0)
    }
}
