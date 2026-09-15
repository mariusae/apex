//! The changes an agent made: what its repository says differs from
//! where things stood when the session began. Git, Sapling and
//! Mercurial are known by the directory they keep at the root of a
//! repository, and asked in their own words; what they say is written
//! as it is, with one thing added -- each hunk's header is followed by
//! the `path:line` it lands at, so B3 on it opens the file there.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A repository's kind, and its root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub kind: Kind,
    pub root: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Git,
    Sapling,
    Mercurial,
}

impl Kind {
    /// The command.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Git => "git",
            Kind::Sapling => "sl",
            Kind::Mercurial => "hg",
        }
    }
}

/// The repository `dir` is in, by the nearest of `.git`, `.sl`, `.hg`
/// up the tree. A `.git` may be a file (a worktree), so either will do.
pub fn repo(dir: &Path) -> Option<Repo> {
    let mut d = Some(dir);
    while let Some(p) = d {
        for (mark, kind) in [(".git", Kind::Git), (".sl", Kind::Sapling), (".hg", Kind::Mercurial)] {
            if p.join(mark).exists() {
                return Some(Repo { kind, root: p.to_path_buf() });
            }
        }
        d = p.parent();
    }
    None
}

fn run(kind: Kind, dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(kind.name()).args(args).current_dir(dir).output().map_err(|e| format!("{}: {e}", kind.name()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("{} {}: {}", kind.name(), args.join(" "), if err.is_empty() { out.status.to_string() } else { err }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Where things stand: the commit the working copy is on, in the
/// repository's own words. Recorded when a session begins, so that
/// `Changes` can say what has happened since, commits included.
pub fn head(dir: &Path) -> Option<String> {
    let r = repo(dir)?;
    let out = match r.kind {
        Kind::Git => run(Kind::Git, dir, &["rev-parse", "HEAD"]),
        Kind::Sapling => run(Kind::Sapling, dir, &["whereami"]),
        Kind::Mercurial => run(Kind::Mercurial, dir, &["id", "-i", "--debug"]),
    }
    .ok()?;
    let id = out.split_whitespace().next()?.trim_end_matches('+').to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

/// What has changed in `dir`'s repository since `base` (a commit
/// `head` gave), or since the last commit when there is no base: the
/// status, a line a file, and then the diff without its `a/` `b/`
/// prefixes, each hunk's header followed by where it lands.
pub fn changes(dir: &Path, base: Option<&str>) -> Result<String, String> {
    let r = repo(dir).ok_or_else(|| format!("{}: not in a git, sapling or mercurial repository", dir.display()))?;
    // asked at the root, so that every path is relative to it, which
    // is where the window showing this is named
    let root = &r.root;
    let (status, diff) = match r.kind {
        Kind::Git => {
            let status = run(Kind::Git, root, &["status", "--short"])?;
            let mut args = vec!["diff", "--no-prefix", "--no-color"];
            match base {
                Some(b) => args.push(b),
                None => args.push("HEAD"),
            }
            (status, run(Kind::Git, root, &args)?)
        }
        Kind::Sapling | Kind::Mercurial => {
            let status = run(r.kind, root, &["status"])?;
            let mut args = vec!["diff", "--noprefix", "--color=never"];
            if let Some(b) = base {
                args.push("-r");
                args.push(b);
            }
            (status, run(r.kind, root, &args)?)
        }
    };
    let mut out = String::new();
    let status = status.trim_end();
    if !status.is_empty() {
        out.push_str(status);
        out.push_str("\n\n");
    }
    out.push_str(&located(&diff));
    if out.trim().is_empty() {
        out.push_str("nothing changed\n");
    }
    Ok(out)
}

/// The diff with each hunk header followed by the `path:line` its new
/// side starts at, which B3 opens.
pub fn located(diff: &str) -> String {
    let mut out = String::new();
    let mut file: Option<String> = None;
    for line in diff.lines() {
        if let Some(p) = line.strip_prefix("+++ ") {
            let p = p.split('\t').next().unwrap_or(p).trim();
            file = if p == "/dev/null" { None } else { Some(p.to_string()) };
        }
        if line.starts_with("@@ ") {
            if let (Some(f), Some(n)) = (&file, new_side_line(line)) {
                out.push_str(line);
                out.push_str(&format!("  {f}:{n}\n"));
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The line a hunk's new side begins at: the `+N` of `@@ -a,b +N,c @@`.
fn new_side_line(header: &str) -> Option<usize> {
    let plus = header.split_whitespace().find(|w| w.starts_with('+'))?;
    plus[1..].split(',').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hunk_header_says_where_it_lands() {
        let diff = "diff --git src/a.rs src/a.rs\n--- src/a.rs\n+++ src/a.rs\n@@ -1,3 +1,4 @@\n a\n+b\n c\n@@ -10,2 +11,3 @@\n x\n+y\n--- /dev/null\n+++ new.txt\n@@ -0,0 +1 @@\n+hi\n";
        let got = located(diff);
        assert!(got.contains("@@ -1,3 +1,4 @@  src/a.rs:1\n"), "{got}");
        assert!(got.contains("@@ -10,2 +11,3 @@  src/a.rs:11\n"), "{got}");
        assert!(got.contains("@@ -0,0 +1 @@  new.txt:1\n"), "{got}");
        assert_eq!(new_side_line("@@ -5 +7 @@"), Some(7));
    }

    #[test]
    fn a_git_repository_is_found_and_its_changes_said() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("apex-agent-vcs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(&dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("src/a.rs"), "a\nb\nc\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        let sub = dir.join("src");
        assert_eq!(repo(&sub).map(|r| r.kind), Some(Kind::Git));
        let base = head(&sub).unwrap();
        assert_eq!(base.len(), 40);
        assert_eq!(changes(&sub, Some(&base)).unwrap(), "nothing changed\n");
        // a commit and an edit since: both are the session's changes
        std::fs::write(dir.join("src/a.rs"), "a\nB\nc\n").unwrap();
        git(&["commit", "-q", "-am", "two"]);
        std::fs::write(dir.join("src/a.rs"), "a\nB\nc\nd\n").unwrap();
        std::fs::write(dir.join("new.txt"), "hi\n").unwrap();
        let got = changes(&sub, Some(&base)).unwrap();
        assert!(got.starts_with(" M src/a.rs\n?? new.txt\n\n"), "{got}");
        assert!(got.contains("+++ src/a.rs\n"), "{got}");
        assert!(got.contains("-b\n+B\n"), "{got}");
        assert!(got.contains("+d\n"), "{got}");
        assert!(got.contains("  src/a.rs:1\n"), "{got}");
        // no base: since the last commit only
        let got = changes(&sub, None).unwrap();
        assert!(!got.contains("+B\n"), "{got}");
        assert!(got.contains("+d\n"), "{got}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
