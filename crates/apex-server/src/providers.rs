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
//! the daemon, and bridge frames through `apex attach -stdio`.
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

/// The providers a session can be reached through: `local` (this
/// machine's daemon), `ssh` (built in), and every `apex-remote-NAME` on
/// the PATH, as NAME.
pub fn available() -> Vec<String> {
    let mut out = vec!["local".to_string(), "ssh".to_string()];
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(p) = name.strip_prefix("apex-remote-") {
                    if !p.is_empty() && e.path().is_file() && !out.iter().any(|x| x == p) {
                        out.push(p.to_string());
                    }
                }
            }
        }
    }
    out
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

/// What we carry for other machines: our command, and rc to run
/// commands with.
pub const CARRIED: [&str; 2] = ["apex", "rc"];

/// Our `apex` for `target` (see [`bundled`]).
pub fn bundled_binary(target: &str) -> Option<PathBuf> {
    bundled(target, "apex")
}

/// Our `name` (`apex` or `rc`) for `target`: in the app bundle under
/// `Resources/remote/`, beside a dev binary under `remote/`, in a
/// cross-build's target directory, or — for this machine's own kind —
/// beside us.
pub fn bundled(target: &str, name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let mut candidates = Vec::new();
    if let Ok(d) = std::env::var("APEX_REMOTE_BINARIES") {
        candidates.push(PathBuf::from(d).join(target).join(name));
    }
    candidates.push(dir.join("../Resources/remote").join(target).join(name));
    candidates.push(dir.join("remote").join(target).join(name));
    let triple = match target {
        "linux-amd64" => Some("x86_64-unknown-linux-musl"),
        "linux-arm64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    };
    // a dev tree: target/<triple>/release/apex and target/rc-<target>/bin/rc
    // beside target/release
    if let Some(t) = dir.parent() {
        if let Some(triple) = triple {
            candidates.push(t.join(triple).join("release").join(name));
        }
        candidates.push(t.join(format!("rc-{target}")).join("bin").join(name));
        if target == local_target() {
            candidates.push(t.join("rc-host").join("bin").join(name));
        }
    }
    if target == local_target() {
        candidates.push(dir.join(name));
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

/// Make sure the destination has our `apex` and `rc`, current with
/// ours, in `~/.apex/bin`. Returns apex's path there, and whether
/// anything was (re)installed. A missing `rc` for the target is not an
/// error: commands there run with `sh` then.
pub fn deploy(host: &str) -> io::Result<(String, bool)> {
    let target = remote_target(host)?;
    let mut installed = false;
    for name in CARRIED {
        let Some(bin) = bundled(&target, name) else {
            if name == "apex" {
                return Err(io::Error::other(format!("no apex for {target} in this build")));
            }
            continue;
        };
        let there = format!("$HOME/.apex/bin/{name}");
        let want = sha256_of(&bin)?;
        let have = run(host, &format!("(sha256sum {there} 2>/dev/null || shasum -a 256 {there} 2>/dev/null) | cut -c1-64"), None)
            .unwrap_or_default()
            .trim()
            .to_string();
        if have == want {
            continue;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&bin)?.read_to_end(&mut bytes)?;
        // written beside, then moved: a daemon still running the old one keeps it
        let install = format!("mkdir -p $HOME/.apex/bin && cat > {there}.new && chmod +x {there}.new && mv {there}.new {there}");
        run(host, &install, Some(&bytes))?;
        installed = true;
    }
    Ok((REMOTE_BIN.to_string(), installed))
}

/// The command whose stdin and stdout carry the frames: `apex attach
/// -stdio` on the destination, which starts the daemon there if it
/// must. Run it through a local shell.
pub fn attach_command(spec: &str, session: &str) -> io::Result<String> {
    let dest = Dest::parse(spec);
    // the remote command is one single-quoted word so `$HOME` is the
    // destination's, not ours
    let session: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).collect();
    Ok(format!("{} {} '{REMOTE_BIN} -session={session} attach -stdio'", shell_quote(&dest.program()?), shell_quote(&dest.name)))
}

/// The sessions on the destination's daemon (started if it is not running).
pub fn list_sessions(host: &str) -> io::Result<Vec<String>> {
    let out = run(host, &format!("{REMOTE_BIN} -ensure-server ls"), None)?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

/// `destination/session` from a name the user typed (`provider:name` or
/// `user@host` before the slash); `None` without a slash.
pub fn split_spec(spec: &str) -> Option<(&str, &str)> {
    let (dest, session) = spec.split_once('/')?;
    if dest.is_empty() {
        return None;
    }
    Some((dest, if session.is_empty() { DEFAULT_SESSION } else { session }))
}

/// A daemon's first session.
pub const DEFAULT_SESSION: &str = "default";

/// A session anywhere, as a URL: `local:///name` (this machine's daemon,
/// the pseudo-provider that takes no argument), `ssh://user@host/name`,
/// `sprite://box/name`. The scheme is the provider, the authority its
/// argument, the path the session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionUrl {
    pub provider: String,
    pub arg: String,
    pub session: String,
}

impl SessionUrl {
    pub fn local(session: &str) -> SessionUrl {
        SessionUrl { provider: "local".into(), arg: String::new(), session: session.to_string() }
    }

    /// A URL, or what older files and hands write: a bare name is a local
    /// session, `dest/name` a destination through its provider.
    pub fn parse(s: &str) -> Option<SessionUrl> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let Some((scheme, rest)) = s.split_once("://") {
            if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return None;
            }
            let (arg, session) = rest.split_once('/').unwrap_or((rest, ""));
            let session = if session.is_empty() { DEFAULT_SESSION } else { session };
            if scheme != "local" && arg.is_empty() {
                return None;
            }
            if session.contains('/') {
                return None;
            }
            return Some(SessionUrl { provider: scheme.to_string(), arg: if scheme == "local" { String::new() } else { arg.to_string() }, session: session.to_string() });
        }
        match split_spec(s) {
            Some((dest, session)) => {
                let d = Dest::parse(dest);
                Some(SessionUrl { provider: d.provider, arg: d.name, session: session.to_string() })
            }
            // a bare destination (`user@host`, `sprite:box`) is its default
            // session; anything else is a local session's name
            None if s.contains('@') || s.contains(':') => {
                let d = Dest::parse(s);
                Some(SessionUrl { provider: d.provider, arg: d.name, session: DEFAULT_SESSION.to_string() })
            }
            None => Some(SessionUrl::local(s)),
        }
    }

    pub fn is_local(&self) -> bool {
        self.provider == "local"
    }

    /// How a session is spoken of: its label, with the host in
    /// parentheses; the default session is just its host. `notes
    /// (blah.host.com)`, `blah.host.com`, `notes`, `local`.
    pub fn describe(&self) -> String {
        let host = if self.is_local() { "local".to_string() } else { self.arg.clone() };
        if self.session == DEFAULT_SESSION {
            host
        } else if self.is_local() {
            self.session.clone()
        } else {
            format!("{} ({host})", self.session)
        }
    }

    /// The destination for `providers.rs`'s functions (`None` for local).
    pub fn dest(&self) -> Option<String> {
        if self.is_local() {
            None
        } else {
            Some(Dest { provider: self.provider.clone(), name: self.arg.clone() }.spec())
        }
    }

    pub fn with_session(&self, session: &str) -> SessionUrl {
        SessionUrl { session: session.to_string(), ..self.clone() }
    }
}

impl std::fmt::Display for SessionUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}/{}", self.provider, self.arg, self.session)
    }
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.@:/".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod available_tests {
    #[test]
    fn providers_are_the_built_in_ones_and_what_the_path_carries() {
        let dir = std::env::temp_dir().join(format!("apex-providers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("apex-remote-zed"), "#!/bin/sh\n").unwrap();
        let old = std::env::var_os("PATH").unwrap_or_default();
        let mut paths: Vec<std::path::PathBuf> = std::env::split_paths(&old).collect();
        paths.insert(0, dir.clone());
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        let got = super::available();
        std::env::set_var("PATH", old);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(&got[..2], &["local".to_string(), "ssh".to_string()]);
        assert!(got.contains(&"zed".to_string()), "{got:?}");
    }
}

#[cfg(test)]
mod describe_tests {
    use super::SessionUrl;

    #[test]
    fn a_session_is_its_label_and_host() {
        assert_eq!(SessionUrl::parse("ssh://blah.host.com/testingsomething").unwrap().describe(), "testingsomething (blah.host.com)");
        assert_eq!(SessionUrl::parse("ssh://blah.host.com/default").unwrap().describe(), "blah.host.com");
        assert_eq!(SessionUrl::parse("sprite://apex-test/notes").unwrap().describe(), "notes (apex-test)");
        assert_eq!(SessionUrl::local("notes").describe(), "notes");
        assert_eq!(SessionUrl::local("default").describe(), "local");
    }
}
