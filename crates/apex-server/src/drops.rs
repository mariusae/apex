//! Files dropped on apex on another machine, given to this host
//! (`PUT apex-drop:NAME` on the I/O plane): each in `~/.apex/drops/KEY/NAME`,
//! the name its own, so a program shown the path sees what the file is
//! called (an agent takes `Screenshot … .png` for an image); the key the
//! client's, of the file (its size and time), so the same file dropped
//! again is the one already here and goes no second time.
//!
//! A file comes in under a name of its own beside where it goes (`.NAME.
//! N.part`) and takes its name only whole: a reader never sees half of
//! one. One that does not come whole -- stopped, cut off, short -- leaves
//! nothing. Drops a week old go.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// How long a drop is kept after it was last given.
pub const KEPT: Duration = Duration::from_secs(7 * 24 * 3600);

/// `~/.apex/drops`.
pub fn dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")).join(".apex").join("drops")
}

/// A file's name, as it may be one here: a single component, not `.`
/// or `..`, nothing a path or a C string would read otherwise.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0') && name.len() <= 255
}

/// A key: letters, digits, `-` and `_`, at most 64.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// What a drop asked for comes to: the file here already (the same key,
/// name and length), or one to write.
pub enum Arrival {
    Have(PathBuf),
    Part(Part),
}

/// A file coming in: written beside where it goes, renamed there whole
/// (`finish`); removed if it never is (dropped).
pub struct Part {
    part: PathBuf,
    path: PathBuf,
    file: Option<File>,
    len: u64,
    written: u64,
    error: Option<String>,
}

impl Arrival {
    /// A drop of `name`, `len` bytes, under `key`, in `root`.
    pub fn open(root: &Path, key: &str, name: &str, len: u64) -> Result<Arrival, String> {
        if !valid_key(key) {
            return Err(format!("{key:?}: not a key"));
        }
        if !valid_name(name) {
            return Err(format!("{name:?}: not a file's name"));
        }
        let dir = root.join(key);
        let path = dir.join(name);
        if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == len) {
            // given again: kept a week from now
            if let Ok(f) = File::options().write(true).open(&path) {
                let _ = f.set_modified(SystemTime::now());
            }
            return Ok(Arrival::Have(path));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        static N: AtomicU64 = AtomicU64::new(0);
        let part = dir.join(format!(".{name}.{}-{}.part", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let file = File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        Ok(Arrival::Part(Part { part, path, file: Some(file), len, written: 0, error: None }))
    }
}

impl Part {
    /// Where it goes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// More of it. The first error is kept for `finish` to say; what
    /// comes after it, or past the length, is not written.
    pub fn write(&mut self, bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        if self.written + bytes.len() as u64 > self.len {
            self.error = Some(format!("more than its {} bytes", self.len));
            return;
        }
        let Some(f) = self.file.as_mut() else { return };
        match f.write_all(bytes) {
            Ok(()) => self.written += bytes.len() as u64,
            Err(e) => self.error = Some(format!("{}: {e}", self.path.display())),
        }
    }

    /// The end of it: renamed where it goes if it came whole, its path;
    /// else why not (and nothing left of it).
    pub fn finish(mut self) -> Result<PathBuf, String> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        if self.written != self.len {
            return Err(format!("{}: {} bytes of {}", self.path.display(), self.written, self.len));
        }
        let f = self.file.take().expect("open until finished");
        f.sync_all().map_err(|e| format!("{}: {e}", self.path.display()))?;
        drop(f);
        std::fs::rename(&self.part, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
        Ok(self.path.clone())
    }
}

impl Drop for Part {
    fn drop(&mut self) {
        // not finished whole (or the rename failed): nothing of it stays
        let _ = std::fs::remove_file(&self.part);
    }
}

/// Drops not given for a week, gone: each key's folder whose files are
/// all older than `kept` (a part left by a daemon that died among them).
pub fn sweep(root: &Path, kept: Duration) {
    let Ok(keys) = std::fs::read_dir(root) else { return };
    let now = SystemTime::now();
    let old = |p: &Path| std::fs::symlink_metadata(p).and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > kept));
    for k in keys.flatten() {
        let p = k.path();
        if !k.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&p) else { continue };
        let all_old = files.flatten().all(|f| old(&f.path()));
        if all_old && old(&p) {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

/// `sweep` of `~/.apex/drops`, at most once an hour.
pub fn sweep_now_and_then() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < Duration::from_secs(3600)) {
        return;
    }
    *last = Some(Instant::now());
    drop(last);
    sweep(&dir(), KEPT);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("apex-drops-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn parts(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".part")).collect()).unwrap_or_default()
    }

    #[test]
    fn a_drop_takes_its_name_only_whole() {
        let r = root("whole");
        let Ok(Arrival::Part(mut p)) = Arrival::open(&r, "k1", "Screenshot 1.png", 6) else { panic!("a part") };
        p.write(b"abc");
        assert!(!r.join("k1/Screenshot 1.png").exists(), "not before it is whole");
        assert_eq!(parts(&r.join("k1")).len(), 1);
        p.write(b"def");
        assert_eq!(p.finish().unwrap(), r.join("k1/Screenshot 1.png"));
        assert_eq!(std::fs::read(r.join("k1/Screenshot 1.png")).unwrap(), b"abcdef");
        assert!(parts(&r.join("k1")).is_empty());
        // again: the one here, nothing to send
        assert!(matches!(Arrival::open(&r, "k1", "Screenshot 1.png", 6), Ok(Arrival::Have(p)) if p == r.join("k1/Screenshot 1.png")));
        // the same name and key, another length: written anew
        assert!(matches!(Arrival::open(&r, "k1", "Screenshot 1.png", 7), Ok(Arrival::Part(_))));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_drop_not_whole_leaves_nothing() {
        let r = root("short");
        // short
        let Ok(Arrival::Part(mut p)) = Arrival::open(&r, "k", "a.bin", 10) else { panic!() };
        p.write(b"12345");
        assert!(p.finish().is_err_and(|e| e.contains("5 bytes of 10")));
        // long
        let Ok(Arrival::Part(mut p)) = Arrival::open(&r, "k", "a.bin", 2) else { panic!() };
        p.write(b"123");
        assert!(p.finish().is_err_and(|e| e.contains("more than its 2")));
        // stopped, or its connection gone: dropped unfinished
        let Ok(Arrival::Part(mut p)) = Arrival::open(&r, "k", "a.bin", 10) else { panic!() };
        p.write(b"12");
        drop(p);
        assert!(!r.join("k/a.bin").exists());
        assert!(parts(&r.join("k")).is_empty(), "no part left: {:?}", parts(&r.join("k")));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn names_and_keys_stay_in_their_folder() {
        let r = root("names");
        for bad in ["", ".", "..", "a/b", "../x", "a\0b"] {
            assert!(Arrival::open(&r, "k", bad, 1).is_err(), "{bad:?}");
        }
        for bad in ["", "..", "a/b", "a.b", &"x".repeat(65)] {
            assert!(Arrival::open(&r, bad, "f", 1).is_err(), "{bad:?}");
        }
        assert!(valid_name("Screenshot 2026-11-26 at 15.20.png") && valid_name(".hidden") && valid_key("a1-B_2"));
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn a_week_old_drop_goes_and_a_new_one_stays() {
        let r = root("sweep");
        for key in ["old", "new"] {
            let Ok(Arrival::Part(mut p)) = Arrival::open(&r, key, "f", 1) else { panic!() };
            p.write(b"x");
            p.finish().unwrap();
        }
        let long_ago = SystemTime::now() - KEPT - Duration::from_secs(60);
        File::options().write(true).open(r.join("old/f")).unwrap().set_modified(long_ago).unwrap();
        File::open(r.join("old")).unwrap().set_modified(long_ago).unwrap();
        sweep(&r, KEPT);
        assert!(!r.join("old").exists() && r.join("new/f").exists());
        let _ = std::fs::remove_dir_all(&r);
    }
}
