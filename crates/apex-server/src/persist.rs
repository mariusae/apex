//! The daemon's sessions kept on disk (`apex_core::dump`), so they
//! outlive it: a dump of each is written as it changes, and read back
//! when the daemon starts (DESIGN.md §12). Each socket's daemon keeps
//! its own, in `~/.apex/sessions/` under the socket's path.

use std::path::{Path, PathBuf};

use apex_core::dump::{Dump, Host};
use apex_core::{Log, TermId};

use crate::Server;

/// The directory a daemon on `socket` keeps its sessions in: the
/// socket's path, its slashes as `%`, under `~/.apex/sessions/`.
pub fn dir_for(socket: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    let name = socket.to_string_lossy().trim_start_matches('/').replace('/', "%");
    Some(PathBuf::from(home).join(".apex/sessions").join(name))
}

/// Where session `id`'s dump is.
pub fn path_of(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// Write `d` as session `id`'s dump: whole, or not at all (a dump
/// half-written by a crash would lose what the last one had).
pub fn write(dir: &Path, id: &str, d: &Dump) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_vec(d).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!(".{id}.json.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path_of(dir, id))
}

/// Session `id` is over: its dump goes.
pub fn forget(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(path_of(dir, id));
}

/// Every dump in `dir`, oldest written first, each read or why not.
pub fn read_all(dir: &Path) -> Vec<(PathBuf, Result<Dump, String>)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, Result<Dump, String>)> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json") && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .map(|p| {
            let d = std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<Dump>(&b).map_err(|e| e.to_string()));
            (p, d)
        })
        .collect();
    out.sort_by_key(|(_, d)| d.as_ref().map(|d| d.saved).unwrap_or(0));
    out
}

/// A dump that would not load, put aside where it is (`id.json.bad`)
/// for its text to be had, and not tried again.
pub fn set_aside(path: &Path) {
    let mut bad = path.as_os_str().to_owned();
    bad.push(".bad");
    let _ = std::fs::rename(path, bad);
}

/// What a dump is loaded with: the server's files, and its terminals,
/// started as the session's settings say.
pub struct Loader<'a> {
    pub server: &'a mut Server,
    pub shell: Option<String>,
    pub scrollback: usize,
}

impl Host for Loader<'_> {
    fn read(&mut self, name: &str) -> Option<(String, String)> {
        self.server.read_path(Path::new(name)).ok()
    }

    fn term(&mut self, log: &mut Log, dir: &str, history: &str) -> Option<TermId> {
        self.server.restored_term(log, dir, history, self.shell.as_deref(), self.scrollback)
    }
}
