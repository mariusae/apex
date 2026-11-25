//! The host's ssh agent link, as Manifold keeps one (its DESIGN.md §8.6).
//!
//! A daemon on a host reached over ssh was started by the first `apex
//! attach -stdio` there, and has that connection's environment: its
//! SSH_AUTH_SOCK, sshd's socket for that login, dies with it, and every
//! terminal and command the daemon starts after -- or started before --
//! has a dead agent. So the daemon gives them none of its own: their
//! SSH_AUTH_SOCK is a link (`~/.apex/agent.sock`), pointed at the newest
//! agent that answers, of
//!
//! - those forwarded to bridges (`apex attach -stdio`): each notes its
//!   connection's as it starts, a file in `~/.apex/agents/` holding its
//!   pid, taken away as it ends;
//! - sshd's for the user's logins (`/tmp/ssh-*/agent.*`, theirs);
//! - those `apex env agent` names.
//!
//! When the connection whose agent it is ends, the link goes to the next,
//! under the shells already running. It is replaced by a rename, so a
//! shell never finds none; pointed at an agent that has gone, it is left
//! so until another comes. A socket's path is limited (104 bytes on
//! macOS) and the link's is what counts, not its target's: one that would
//! be too long goes in the home, else in /tmp.
//!
//! A daemon that no bridge started (`APEX_BRIDGED` unset), whose own
//! agent answers -- a Mac's, from launchd -- keeps that one.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// Set by a bridge for the daemon it starts: its environment is a
/// connection's, which will end -- its SSH_AUTH_SOCK with it.
pub const BRIDGED: &str = "APEX_BRIDGED";

/// How often the daemon looks again.
const EVERY: Duration = Duration::from_secs(5);

/// Where apex keeps what is the host's: `~/.apex`.
fn home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join(".apex")
}

/// The link: in `~/.apex` if its path can name a socket and has no
/// spaces; else in the home; else in a directory of the user's in /tmp.
pub fn link() -> PathBuf {
    let own = home().join("agent.sock");
    let fits = |p: &Path| p.as_os_str().len() <= 100 && !p.to_string_lossy().contains(' ');
    if fits(&own) {
        return own;
    }
    let home = home().with_file_name(".apex-agent.sock");
    if fits(&home) {
        return home;
    }
    // SAFETY: getuid cannot fail
    let dir = PathBuf::from(format!("/tmp/apex-{}", unsafe { libc::getuid() }));
    let _ = std::fs::create_dir_all(&dir);
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    dir.join("agent.sock")
}

/// Where agents are noted, a file each.
fn notes() -> PathBuf {
    home().join("agents")
}

/// An agent noted: by a bridge as it starts (`pid` its own, the note
/// gone with it), or named (`apex env agent`).
pub fn note(socket: &str, from: &str, pid: Option<u32>) {
    if Path::new(socket) == link() || !is_socket(Path::new(socket)) {
        return;
    }
    let dir = notes();
    let _ = std::fs::create_dir_all(&dir);
    let name = match pid {
        Some(p) => format!("bridge-{p}"),
        None => format!("named{}", socket.replace('/', "_")),
    };
    let text = format!("{socket}\n{from}\n{}\n", pid.map(|p| p.to_string()).unwrap_or_default());
    // written beside, then renamed: a reader sees all of it or none
    let tmp = dir.join(format!(".{name}.{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, dir.join(&name)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// A bridge's note taken away as it ends.
pub fn forget(pid: u32) {
    let _ = std::fs::remove_file(notes().join(format!("bridge-{pid}")));
}

/// An agent that answers: its socket, where it came from, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub socket: String,
    pub from: String,
    pub at: SystemTime,
}

/// Every agent that answers, newest first; the notes of those gone
/// deleted.
pub fn candidates() -> Vec<Candidate> {
    candidates_in(&notes(), Path::new("/tmp"))
}

fn candidates_in(notes: &Path, tmp: &Path) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let mut add = |c: Candidate| match found.iter_mut().find(|f| f.socket == c.socket) {
        // noted and found both: when it was noted, by whom
        Some(f) if f.at >= c.at => {}
        Some(f) => *f = c,
        None => found.push(c),
    };
    for e in std::fs::read_dir(notes).into_iter().flatten().flatten() {
        let path = e.path();
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut lines = text.lines();
        let (socket, from, pid) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""), lines.next().and_then(|p| p.trim().parse::<i32>().ok()));
        // SAFETY: kill with signal 0 only asks whether the process is there
        let alive = pid.is_none_or(|p| unsafe { libc::kill(p, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM));
        if socket.is_empty() || !alive || !answers(Path::new(socket)) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let at = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        add(Candidate { socket: socket.to_string(), from: from.to_string(), at });
    }
    // sshd's, for this user's logins with an agent forwarded
    // SAFETY: getuid cannot fail
    let uid = unsafe { libc::getuid() };
    for d in std::fs::read_dir(tmp).into_iter().flatten().flatten() {
        if !d.file_name().to_string_lossy().starts_with("ssh-") {
            continue;
        }
        let dir = d.path();
        let Ok(m) = std::fs::symlink_metadata(&dir) else { continue };
        if !m.is_dir() || m.uid() != uid {
            continue;
        }
        for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if !f.file_name().to_string_lossy().starts_with("agent.") {
                continue;
            }
            let s = f.path();
            let Ok(m) = std::fs::symlink_metadata(&s) else { continue };
            if m.uid() != uid || !answers(&s) {
                continue;
            }
            let at = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            add(Candidate { socket: s.display().to_string(), from: "forwarded to an ssh login".into(), at });
        }
    }
    found.sort_by(|a, b| b.at.cmp(&a.at));
    found
}

fn is_socket(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.file_type().is_socket())
}

/// Something listens at the socket.
pub fn answers(p: &Path) -> bool {
    is_socket(p) && std::os::unix::net::UnixStream::connect(p).is_ok()
}

/// Where the link points now, if it is there.
pub fn target() -> Option<String> {
    std::fs::read_link(link()).ok().map(|t| t.display().to_string())
}

/// The link pointed at `socket` at once: made beside it, renamed over.
fn point_at(link: &Path, socket: &str) {
    if std::fs::read_link(link).ok().is_some_and(|t| t == Path::new(socket)) {
        return;
    }
    if let Some(d) = link.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = link.with_extension(format!("sock.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    if std::os::unix::fs::symlink(socket, &tmp).is_ok() && std::fs::rename(&tmp, link).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The link pointed at the newest agent that answers, if any does; if
/// none, left as it was, for the next to come. Those that answer.
pub fn update() -> Vec<Candidate> {
    let all = candidates();
    if let Some(newest) = all.first() {
        point_at(&link(), &newest.socket);
    }
    all
}

/// How many keys the agent at the link holds (`ssh-add -l`); `None` when
/// it could not be asked.
pub fn keys() -> Option<usize> {
    let out = std::process::Command::new("ssh-add").arg("-l").env("SSH_AUTH_SOCK", link()).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    // 1: the agent has no keys; 2: it could not be reached
    match out.status.code() {
        Some(0) => Some(String::from_utf8_lossy(&out.stdout).lines().count()),
        Some(1) => Some(0),
        _ => None,
    }
}

/// Whether this daemon keeps the link (and gives it), decided as it
/// starts (`keep`); not decided, it gives nothing of its own.
static MANAGED: OnceLock<bool> = OnceLock::new();

/// The daemon starting: whether its own agent is the host's (not a
/// bridge's connection's, and answering) and so kept; if not, the link
/// kept from now on, looked at every few seconds.
pub fn keep() {
    let bridged = std::env::var(BRIDGED).is_ok_and(|v| v == "1");
    let inherited = std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
    let local = !bridged && inherited.as_deref().is_some_and(|p| p != link() && answers(p));
    if MANAGED.set(!local).is_err() || local {
        return;
    }
    std::thread::spawn(|| loop {
        update();
        std::thread::sleep(EVERY);
    });
}

/// What a terminal or command the daemon starts gets: SSH_AUTH_SOCK the
/// link, once there is one (even pointed at an agent gone: the next to
/// come is pointed at under it), when the daemon keeps it. Before the
/// session's own variables, which may set another.
pub fn spawn_vars() -> Vec<(String, String)> {
    if MANAGED.get() != Some(&true) {
        return Vec::new();
    }
    let l = link();
    if std::fs::symlink_metadata(&l).is_err() {
        return Vec::new();
    }
    vec![("SSH_AUTH_SOCK".into(), l.display().to_string())]
}

/// How the link stands, in a line: where it points, from where, whether
/// the agent answers and how many keys it has.
pub fn status() -> String {
    let l = link();
    let Some(t) = target() else { return format!("{}: no agent yet", l.display()) };
    let all = candidates();
    let from = all.iter().find(|c| c.socket == t).map(|c| c.from.clone());
    match from {
        Some(from) => {
            let keys = match keys() {
                Some(1) => ", 1 key".to_string(),
                Some(n) => format!(", {n} keys"),
                None => String::new(),
            };
            format!("{} -> {t} ({from}{keys})", l.display())
        }
        None => format!("{} -> {t} (gone; the next agent to come is pointed at)", l.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("apex-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_note(dir: &Path, name: &str, socket: &Path, pid: Option<u32>) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), format!("{}\nhere\n{}\n", socket.display(), pid.map(|p| p.to_string()).unwrap_or_default())).unwrap();
    }

    #[test]
    fn the_newest_live_agent_and_dead_notes_go() {
        let d = scratch("cand");
        let (notes, tmp) = (d.join("agents"), d.join("tmp"));
        std::fs::create_dir_all(&tmp).unwrap();
        let old = d.join("old.sock");
        let _l1 = UnixListener::bind(&old).unwrap();
        write_note(&notes, "named-old", &old, None);
        std::thread::sleep(Duration::from_millis(20));
        // sshd's, for this user, newer
        let ssh = tmp.join("ssh-abc");
        std::fs::create_dir_all(&ssh).unwrap();
        let login = ssh.join("agent.123");
        let _l2 = UnixListener::bind(&login).unwrap();
        // a socket nobody listens at, and a bridge that is gone
        let dead = d.join("dead.sock");
        drop(UnixListener::bind(&dead).unwrap());
        write_note(&notes, "named-dead", &dead, None);
        write_note(&notes, "bridge-999999", &old, Some(999_999));
        let all = candidates_in(&notes, &tmp);
        assert_eq!(all.iter().map(|c| c.socket.clone()).collect::<Vec<_>>(), vec![login.display().to_string(), old.display().to_string()]);
        assert!(!notes.join("named-dead").exists(), "a dead agent's note goes");
        assert!(!notes.join("bridge-999999").exists(), "a gone bridge's note goes");
        assert!(notes.join("named-old").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_link_is_replaced_in_place() {
        let d = scratch("link");
        let l = d.join("agent.sock");
        point_at(&l, "/tmp/a");
        assert_eq!(std::fs::read_link(&l).unwrap(), PathBuf::from("/tmp/a"));
        point_at(&l, "/tmp/b");
        assert_eq!(std::fs::read_link(&l).unwrap(), PathBuf::from("/tmp/b"));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "nothing left beside it");
        let _ = std::fs::remove_dir_all(&d);
    }
}
