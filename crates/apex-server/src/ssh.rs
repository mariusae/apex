//! Attaching to a session on another machine over ssh (§6, §7). The
//! host needs nothing but sshd: we carry an `apex` for its OS and
//! architecture, put it in `~/.apex/bin` (or update it when ours
//! differs), let it start the daemon, and bridge frames through
//! `apex attach --stdio` on the host.
//!
//! `APEX_SSH` names the ssh program to use; tests point it at a script
//! that runs the commands locally.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Where the host keeps our command.
pub const REMOTE_BIN: &str = "$HOME/.apex/bin/apex";

pub fn ssh_program() -> String {
    std::env::var("APEX_SSH").unwrap_or_else(|_| "ssh".into())
}

/// Run `cmd` on `host` through the login shell, with `stdin` fed to it.
/// Returns stdout; a failure carries stderr.
pub fn run(host: &str, cmd: &str, stdin: Option<&[u8]>) -> io::Result<String> {
    let mut child = Command::new(ssh_program())
        .arg(host)
        .arg(cmd)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes)?;
        drop(pipe);
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(io::Error::other(if err.is_empty() { format!("{host}: {cmd}: {}", out.status) } else { format!("{host}: {err}") }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The host's OS and architecture as we name binaries: `linux-amd64`,
/// `linux-arm64`, `darwin-arm64`, ...
pub fn remote_target(host: &str) -> io::Result<String> {
    let uname = run(host, "uname -sm", None)?;
    let mut parts = uname.split_whitespace();
    let os = match parts.next().unwrap_or("") {
        "Linux" => "linux",
        "Darwin" => "darwin",
        other => return Err(io::Error::other(format!("{host}: unsupported OS {other:?}"))),
    };
    let arch = match parts.next().unwrap_or("") {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => return Err(io::Error::other(format!("{host}: unsupported architecture {other:?}"))),
    };
    Ok(format!("{os}-{arch}"))
}

/// This machine, named the same way.
pub fn local_target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}-{arch}")
}

/// Our `apex` for `target`: in the app bundle under `Resources/remote/`,
/// beside a dev binary under `remote/`, in a cross-build's target
/// directory, or — for this machine's own kind — the `apex` beside us.
pub fn bundled_binary(target: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let mut candidates = Vec::new();
    if let Ok(d) = std::env::var("APEX_REMOTE_BINARIES") {
        candidates.push(PathBuf::from(d).join(target).join("apex"));
    }
    candidates.push(dir.join("../Resources/remote").join(target).join("apex"));
    candidates.push(dir.join("remote").join(target).join("apex"));
    if let Some(triple) = match target {
        "linux-amd64" => Some("x86_64-unknown-linux-musl"),
        "linux-arm64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    } {
        // a dev tree: target/<triple>/release/apex beside target/release
        if let Some(t) = dir.parent() {
            candidates.push(t.join(triple).join("release").join("apex"));
        }
    }
    if target == local_target() {
        candidates.push(dir.join("apex"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn sha256_of(path: &std::path::Path) -> io::Result<String> {
    let out = Command::new("shasum").arg("-a").arg("256").arg(path).output()?;
    if !out.status.success() {
        return Err(io::Error::other("shasum failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_string())
}

/// Make sure `host` has our `apex`, current with ours. Returns its path
/// there, and whether it was (re)installed.
pub fn deploy(host: &str) -> io::Result<(String, bool)> {
    let target = remote_target(host)?;
    let bin = bundled_binary(&target).ok_or_else(|| io::Error::other(format!("no apex for {target} in this build")))?;
    let want = sha256_of(&bin)?;
    let have = run(host, &format!("(sha256sum {REMOTE_BIN} 2>/dev/null || shasum -a 256 {REMOTE_BIN} 2>/dev/null) | cut -c1-64"), None)
        .unwrap_or_default()
        .trim()
        .to_string();
    if have == want {
        return Ok((REMOTE_BIN.to_string(), false));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&bin)?.read_to_end(&mut bytes)?;
    // written beside, then moved: a daemon still running the old one keeps it
    let install = format!(
        "mkdir -p $HOME/.apex/bin && cat > {REMOTE_BIN}.new && chmod +x {REMOTE_BIN}.new && mv {REMOTE_BIN}.new {REMOTE_BIN}"
    );
    run(host, &install, Some(&bytes))?;
    Ok((REMOTE_BIN.to_string(), true))
}

/// The command whose stdin and stdout carry the frames: `apex attach
/// --stdio` on the host, which starts the daemon there if it must.
pub fn attach_command(host: &str, session: &str) -> String {
    // the command is run through a local shell: the remote command is one
    // single-quoted word so `$HOME` is the host's, not ours
    let session: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).collect();
    format!("{} {} '{REMOTE_BIN} --session {session} attach --stdio'", ssh_program(), shell_quote(host))
}

/// The sessions on `host`'s daemon (started if it is not running).
pub fn list_sessions(host: &str) -> io::Result<Vec<String>> {
    let out = run(host, &format!("{REMOTE_BIN} --ensure-server ls"), None)?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

/// `host/session` from a name the user typed; `None` without a slash.
pub fn split_spec(spec: &str) -> Option<(&str, &str)> {
    let (host, session) = spec.split_once('/')?;
    if host.is_empty() {
        return None;
    }
    Some((host, if session.is_empty() { "local" } else { session }))
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.@:/".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
