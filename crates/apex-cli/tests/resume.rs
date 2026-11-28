//! `apex resume TICKET`: what a restored terminal types at its first
//! prompt to start a program again (program resume, OSC 7577).

use std::process::Command;

fn ticket(name: &str, argv: &[&str], cwd: Option<&str>) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("apex-resume-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let r = apex_core::dump::DumpResume { argv: argv.iter().map(|s| s.to_string()).collect(), cwd: cwd.map(String::from) };
    std::fs::write(&p, serde_json::to_vec(&r).unwrap()).unwrap();
    p
}

#[test]
fn the_program_runs_as_it_was_in_its_directory_and_the_ticket_goes() {
    let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let t = ticket("one", &["sh", "-c", "pwd; printf '%s|' \"$@\"", "x", "a b", "c;d", ""], Some(dir.to_str().unwrap()));
    let out = Command::new(env!("CARGO_BIN_EXE_apex")).args(["resume", t.to_str().unwrap()]).env_remove("OSC7577_TOKEN").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // in its directory, its arguments exactly as they were, none read by a shell
    assert_eq!(stdout, format!("{}\na b|c;d||", dir.display()));
    assert!(String::from_utf8_lossy(&out.stderr).contains("apex: resuming sh -c"));
    assert!(!t.exists(), "a ticket is good once");
}

#[test]
fn a_directory_gone_is_not_resumed_in() {
    let t = ticket("two", &["echo", "should not run"], Some("/nonexistent/apex-resume"));
    let out = Command::new(env!("CARGO_BIN_EXE_apex")).args(["resume", t.to_str().unwrap()]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("is gone: not resuming echo"), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.is_empty());
}
