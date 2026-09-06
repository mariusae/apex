//! Watching the files behind buffers (§9). Parent directories are watched,
//! not files: editors and `git checkout` replace files by rename, which
//! breaks per-file watches. Events name paths; the server decides what
//! they mean by hashing.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub struct Watches {
    watcher: Option<RecommendedWatcher>,
    dirs: BTreeSet<PathBuf>,
    /// Canonical directory → the directory as buffers name it. FSEvents
    /// reports `/private/var/...` for a file opened as `/var/...`.
    canonical: HashMap<PathBuf, PathBuf>,
    /// What we last wrote to each path, so our own `Put` is not a change.
    pub written: HashMap<PathBuf, String>,
}

impl Watches {
    /// `on_change` is called from the watcher's thread with each changed
    /// path.
    pub fn new(on_change: impl Fn(PathBuf) + Send + 'static) -> Watches {
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    on_change(p);
                }
            }
        })
        .ok();
        Watches { watcher, dirs: BTreeSet::new(), canonical: HashMap::new(), written: HashMap::new() }
    }

    /// Watch exactly the parent directories of `files`.
    pub fn sync<'a>(&mut self, files: impl Iterator<Item = &'a Path>) {
        let want: BTreeSet<PathBuf> = files.filter_map(|f| f.parent().map(Path::to_path_buf)).filter(|d| d.is_dir()).collect();
        let Some(w) = self.watcher.as_mut() else { return };
        for d in self.dirs.difference(&want) {
            let _ = w.unwatch(d);
        }
        for d in want.difference(&self.dirs) {
            let _ = w.watch(d, RecursiveMode::NonRecursive);
        }
        self.canonical = want.iter().filter_map(|d| std::fs::canonicalize(d).ok().map(|c| (c, d.clone()))).collect();
        self.dirs = want;
    }

    /// The path of an event, as the buffers name it.
    pub fn as_named(&self, p: &Path) -> PathBuf {
        if let (Some(parent), Some(file)) = (p.parent(), p.file_name()) {
            if let Some(d) = self.canonical.get(parent) {
                return d.join(file);
            }
        }
        p.to_path_buf()
    }
}
