//! Attaching to a session on another machine (§6, §7) through a
//! *provider*: a command that runs a shell command line on a destination
//! with stdin and stdout connected, exactly as `ssh HOST COMMAND` does.
//! `ssh` is the built-in provider; any other is an executable named
//! `apex-remote-<provider>` on the PATH, called as
//!
//! ```text
//! apex-remote-<provider> DESTINATION COMMAND
//! ```
//!
//! where COMMAND is one argument, a shell command line for the
//! destination (it uses `&&`, redirections and the destination's
//! `$HOME`). A provider over an argv-style tool wraps it: `apex-remote-sprite`
//! is `exec sprite exec -s "$1" -- sh -c "$2"`. There is no other
//! configuration: the provider script is it.
//!
//! A destination is written `provider:name`, or just `name` for ssh
//! (`user@host`). The destination needs nothing but the provider's
//! access: we carry an `apex` for its OS and architecture, put it in
//! `~/.apex/bin` there (or update it when ours differs), let it start
//! the daemon, and bridge frames through `apex attach --stdio`.
//!
//! `APEX_PROVIDER_<NAME>` (and `APEX_SSH` for ssh) name the program to
//! use instead; tests point them at a script that runs the commands
//! locally.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Where the host keeps our command.
pub const REMOTE_BIN: &str = "$HOME/.apex/bin/apex";

/// A destination: which provider reaches it, and its name there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dest {
    pub provider: String,
    pub name: String,
}

impl Dest {
    /// `provider:name`, or `name` for ssh (`user@host`).
    pub fn parse(spec: &str) -> Dest {
        match spec.split_once(':') {
            Some((p, n)) if !p.is_empty() && !n.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') => {
                Dest { provider: p.to_string(), name: n.to_string() }
            }
            _ => Dest { provider: "ssh".into(), name: spec.to_string() },
        }
    }

    /// The spec as the user writes it: ssh destinations are bare.
    pub fn spec(&self) -> String {
        if self.provider == "ssh" {
            self.name.clone()
        } else {
            format!("{}:{}", self.provider, self.name)
        }
    }

    /// The program for this provider: `APEX_PROVIDER_<NAME>` if set
    /// (`APEX_SSH` for ssh), else `apex-remote-<provider>` on the PATH, else
    /// `ssh` itself for ssh.
    pub fn program(&self) -> io::Result<String> {
        let var = format!("APEX_PROVIDER_{}", self.provider.to_ascii_uppercase().replace('-', "_"));
        if let Ok(p) = std::env::var(&var) {
            return Ok(p);
        }
        if self.provider == "ssh" {
            if let Ok(p) = std::env::var("APEX_SSH") {
                return Ok(p);
            }
        }
        let name = format!("apex-remote-{}", self.provider);
        if let Some(p) = on_path(&name) {
            return Ok(p.to_string_lossy().to_string());
        }
        if self.provider == "ssh" {
            return Ok("ssh".into());
        }
        Err(io::Error::new(io::ErrorKind::NotFound, format!("no {name} command on the PATH for provider {}", self.provider)))
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Run `cmd` (a shell command line) on the destination `spec`, with
/// `stdin` fed to it. Returns stdout; a failure carries stderr.
pub fn run(spec: &str, cmd: &str, stdin: Option<&[u8]>) -> io::Result<String> {
    let dest = Dest::parse(spec);
    let host = dest.name.as_str();
    let mut child = Command::new(dest.program()?)
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

/// The destination's OS and architecture as we name binaries:
/// `linux-amd64`, `linux-arm64`, `darwin-arm64`, ...
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

/// Make sure the destination has our `apex`, current with ours. Returns
/// its path there, and whether it was (re)installed.
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
/// --stdio` on the destination, which starts the daemon there if it
/// must. Run it through a local shell.
pub fn attach_command(spec: &str, session: &str) -> io::Result<String> {
    let dest = Dest::parse(spec);
    // the remote command is one single-quoted word so `$HOME` is the
    // destination's, not ours
    let session: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).collect();
    Ok(format!("{} {} '{REMOTE_BIN} --session {session} attach --stdio'", shell_quote(&dest.program()?), shell_quote(&dest.name)))
}

/// The sessions on the destination's daemon (started if it is not running).
pub fn list_sessions(host: &str) -> io::Result<Vec<String>> {
    let out = run(host, &format!("{REMOTE_BIN} --ensure-server ls"), None)?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

/// `destination/session` from a name the user typed (`provider:name` or
/// `user@host` before the slash); `None` without a slash.
pub fn split_spec(spec: &str) -> Option<(&str, &str)> {
    let (dest, session) = spec.split_once('/')?;
    if dest.is_empty() {
        return None;
    }
    Some((dest, if session.is_empty() { "local" } else { session }))
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.@:/".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
