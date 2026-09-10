//! Attaching over ssh, with `ssh` replaced by a script that runs the
//! command on this machine under a scratch HOME: the install of our
//! binary on the "host", the daemon started there, and frames bridged
//! through `apex attach -stdio`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::remote::Remote;
use apex_server::providers as ssh;

/// Both tests set process-wide environment variables: one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A fake ssh: `fake-ssh HOST CMD` runs CMD in a shell with HOME and the
/// socket set for the scratch host. Returns (script, home, socket).
fn fake_host() -> (PathBuf, PathBuf, PathBuf) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let home = std::env::temp_dir().join(format!("apex-ssh-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let socket = std::env::temp_dir().join(format!("apex-ssh-{}-{nanos}.sock", std::process::id()));
    let script = home.join("fake-ssh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n# $1 is the host; the rest is the command\nshift\nexport HOME={}\nexport APEX_SOCKET={}\nexec sh -c \"$*\"\n",
            home.display(),
            socket.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, home, socket)
}

/// Stop the daemon a test started on its scratch host (by its socket path,
/// which is this test's alone).
fn stop_host(socket: &PathBuf) {
    let _ = std::process::Command::new("pkill").arg("-f").arg("--").arg(format!("-socket={} ", socket.display())).status();
    let _ = std::fs::remove_file(socket);
}

/// Our binaries "for other machines": this machine's kind, which is the
/// apex built for these tests.
fn binaries() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apex-ssh-bins-{}", std::process::id()));
    let d = dir.join(ssh::local_target());
    std::fs::create_dir_all(&d).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_apex"), d.join("apex")).unwrap();
    // an rc to carry along (any executable will do for the install)
    std::fs::write(d.join("rc"), "#!/bin/sh\nexec sh \"$@\"\n").unwrap();
    dir
}

#[test]
fn deploy_installs_and_updates_our_binary_on_the_host() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (script, home, _sock) = fake_host();
    std::env::set_var("APEX_SSH", &script);
    std::env::set_var("APEX_REMOTE_BINARIES", binaries());
    assert_eq!(ssh::remote_target("box").unwrap(), ssh::local_target());
    let (path, installed) = ssh::deploy("box").unwrap();
    assert_eq!(path, ssh::REMOTE_BIN);
    assert!(installed);
    let there = home.join(".apex/bin/apex");
    assert!(there.is_file());
    assert!(home.join(".apex/bin/rc").is_file(), "rc carried along");
    // the same binary again: nothing to do
    let (_, installed) = ssh::deploy("box").unwrap();
    assert!(!installed);
    // a changed local binary is put there again
    let bins = PathBuf::from(std::env::var("APEX_REMOTE_BINARIES").unwrap());
    let mine = bins.join(ssh::local_target()).join("apex");
    let mut bytes = std::fs::read(&mine).unwrap();
    bytes.push(0);
    std::fs::write(&mine, bytes).unwrap();
    let (_, installed) = ssh::deploy("box").unwrap();
    assert!(installed);
    assert_eq!(std::fs::read(&there).unwrap().len(), std::fs::read(&mine).unwrap().len());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn attaching_over_ssh_bridges_to_a_daemon_on_the_host() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (script, home, sock) = fake_host();
    std::env::set_var("APEX_SSH", &script);
    std::env::set_var("APEX_REMOTE_BINARIES", binaries());
    ssh::deploy("box").unwrap();
    // the daemon on the host is started by the bridge; sessions listed
    assert_eq!(ssh::list_sessions("box").unwrap().into_iter().map(|s| s.label).collect::<Vec<_>>(), vec!["default".to_string()]);
    let cmd = ssh::attach_command("box", "default").unwrap();
    assert!(cmd.starts_with(&format!("{} box ", script.display())), "{cmd}");
    let mut c = Remote::via(&cmd, "default", "over-ssh", AttachmentKind::Ui).unwrap();
    let col = c.node.state.layout.cols[0].id;
    let w = c.node.new_window(&mut c.log, col, "remote-notes", "typed over ssh\n").unwrap();
    c.flush();
    let b = c.node.view_buffer(ViewId::Body(w)).unwrap();
    let want = c.log.last_seq(Shard::Buffer(b));
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.acked(Shard::Buffer(b)) < want && Instant::now() < deadline {
        let _ = c.step(Duration::from_millis(50));
    }
    assert_eq!(c.acked(Shard::Buffer(b)), want, "acked over the bridge");
    // another client on the same "host" sees it
    let again = Remote::via(&ssh::attach_command("box", "default").unwrap(), "default", "again", AttachmentKind::Tool).unwrap();
    assert!(again.node.state.windows.keys().any(|w| again.node.window_name(*w) == "remote-notes"));
    drop(again);
    drop(c);
    stop_host(&sock);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn a_provider_is_a_command_named_apex_provider_on_the_path() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // "sprite:box": the provider script is `apex-remote-sprite`, found on the PATH,
    // called as `apex-remote-sprite box COMMAND`
    let (script, home, sock) = fake_host();
    let bindir = home.join("providers");
    std::fs::create_dir_all(&bindir).unwrap();
    std::fs::copy(&script, bindir.join("apex-remote-sprite")).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bindir.display(), std::env::var("PATH").unwrap_or_default()));
    std::env::remove_var("APEX_SSH");
    std::env::set_var("APEX_REMOTE_BINARIES", binaries());
    let d = ssh::Dest::parse("sprite:box");
    assert_eq!(d, ssh::Dest { provider: "sprite".into(), name: "box".into() });
    assert_eq!(d.program().unwrap(), bindir.join("apex-remote-sprite").to_string_lossy());
    assert_eq!(ssh::Dest::parse("me@host").spec(), "me@host");
    assert_eq!(ssh::Dest::parse("sprite:box").spec(), "sprite:box");
    assert_eq!(ssh::split_spec("sprite:box/dev"), Some(("sprite:box", "dev")));
    assert!(ssh::Dest::parse("nosuch:thing").program().is_err());
    // the whole path through the provider
    let (_, installed) = ssh::deploy("sprite:box").unwrap();
    assert!(installed);
    assert_eq!(ssh::list_sessions("sprite:box").unwrap().into_iter().map(|s| s.label).collect::<Vec<_>>(), vec!["default".to_string()]);
    let c = Remote::via(&ssh::attach_command("sprite:box", "default").unwrap(), "default", "over-sprite", AttachmentKind::Ui).unwrap();
    assert_eq!(c.node.state.layout.cols.len(), 1);
    drop(c);
    stop_host(&sock);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_file(&sock);
}

#[test]
fn session_urls_name_a_provider_an_argument_and_a_session() {
    use ssh::SessionUrl;
    let u = SessionUrl::parse("sprite://box/dev").unwrap();
    assert_eq!(u, SessionUrl { provider: "sprite".into(), arg: "box".into(), session: "dev".into(), id: None });
    assert_eq!(u.to_string(), "sprite://box/dev");
    assert_eq!(u.dest().as_deref(), Some("sprite:box"));
    let l = SessionUrl::parse("local:///work").unwrap();
    assert!(l.is_local());
    assert_eq!(l.to_string(), "local:///work");
    assert_eq!(l.dest(), None);
    // older spellings still read
    assert_eq!(SessionUrl::parse("work").unwrap(), SessionUrl::local("work"));
    assert_eq!(SessionUrl::parse("me@host/dev").unwrap().to_string(), "ssh://me@host/dev");
    assert_eq!(SessionUrl::parse("sprite:box").unwrap().to_string(), "sprite://box/default");
    assert_eq!(SessionUrl::parse("ssh://me@host").unwrap().session, "default");
    assert!(SessionUrl::parse("sprite:///nothing").is_none());
    assert!(SessionUrl::parse("").is_none());
}
