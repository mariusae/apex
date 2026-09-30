//! A daemon given SIGTERM ignored (by whatever started the app) must not
//! pass that on: what it runs is killed by Kill all the same. A test
//! binary of its own, since it changes the process's disposition.

use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::{perform, Server};

#[test]
fn what_the_server_runs_dies_of_kill_though_the_server_ignores_it() {
    // SAFETY: setting a signal disposition, in a process of this test's own
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    std::env::set_var("SHELL", "/bin/sh");
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "test");
    let mut node = Node::new(a);
    node.catch_up(&log).unwrap();
    let col = node.init_session(&mut log).unwrap();
    let (mut server, mut rx) = Server::new(&log);
    let w = node.new_window(&mut log, col, "scratch", "").unwrap();
    node.exec(&mut log, ExecCtx::Window(w), "sleep 30").unwrap();
    let props = server.poll_execs(&mut log, &node);
    perform(&mut node, &mut log, props);
    let running = |n: &Node| n.state.meta.procs.iter().find(|p| p.name == "sleep" && p.running()).cloned();
    let pump = |log: &mut Log, node: &mut Node, server: &mut Server, rx: &mut futures::channel::mpsc::UnboundedReceiver<apex_server::ServerEvent>, done: &dyn Fn(&Node) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(node) && Instant::now() < deadline {
            match rx.try_recv() {
                Ok(ev) => {
                    let props = server.pump(log, node, ev);
                    perform(node, log, props);
                    node.catch_up(log).unwrap();
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        done(node)
    };
    assert!(pump(&mut log, &mut node, &mut server, &mut rx, &|n| running(n).is_some()), "sleep starts");
    let pid = running(&node).unwrap().pid;
    assert_eq!(server.kill(&pid.to_string()), 1);
    assert!(pump(&mut log, &mut node, &mut server, &mut rx, &|n| running(n).is_none()), "sleep ignored the SIGTERM");
}
