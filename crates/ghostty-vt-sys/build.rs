//! Builds libghostty-vt from a pinned Ghostty commit and the shim over
//! it, and tells cargo to link both.
//!
//! Ghostty's VT library is Zig, and its C API lives only on main (the
//! tagged releases carry the OSC and SGR parsers alone), so a commit is
//! pinned here and built with Zig. The build needs `zig` (0.16) and, the
//! first time, the network. The source and the built library are kept
//! under a cache directory, keyed by the commit, so this happens once
//! per commit rather than once per build.
//!
//! - `APEX_GHOSTTY_SRC`: a Ghostty checkout to build instead of cloning.
//! - `APEX_GHOSTTY_LIB`: a directory holding `lib/libghostty-vt.a` and
//!   `include`, already built; nothing is cloned or built.
//! - `ZIG`: the Zig to build with (else `zig` on PATH, else the newest
//!   `~/.local/zig-*-0.16*/zig`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The Ghostty commit the shim is written against (main, 2026-09-21).
const GHOSTTY_COMMIT: &str = "4ff699343ad039bc73f970ae104cbcec42cc070c";
const GHOSTTY_REPO: &str = "https://github.com/ghostty-org/ghostty";

fn main() {
    println!("cargo:rerun-if-changed=src/shim.c");
    println!("cargo:rerun-if-changed=build.rs");
    for v in ["APEX_GHOSTTY_SRC", "APEX_GHOSTTY_LIB", "ZIG"] {
        println!("cargo:rerun-if-env-changed={v}");
    }

    let out = match std::env::var_os("APEX_GHOSTTY_LIB") {
        Some(d) => PathBuf::from(d),
        None => build_lib(),
    };
    let include = out.join("include");
    if !out.join("lib/libghostty-vt.a").is_file() {
        panic!("no libghostty-vt.a under {}", out.display());
    }

    cc::Build::new().file("src/shim.c").include(&include).flag_if_supported("-std=c11").warnings(true).compile("apex_vt_shim");

    // the archive alone, where the linker cannot find the dylib beside it
    // and link that instead
    let here = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("lib");
    std::fs::create_dir_all(&here).expect("link directory");
    std::fs::copy(out.join("lib/libghostty-vt.a"), here.join("libghostty-vt.a")).expect("the archive");
    println!("cargo:rustc-link-search=native={}", here.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:include={}", include.display());
}

/// The library, built once per commit under the cache, then reused.
fn build_lib() -> PathBuf {
    let cache = cache_dir().join(format!("ghostty-{}", &GHOSTTY_COMMIT[..12]));
    let prefix = cache.join("out");
    if prefix.join("lib/libghostty-vt.a").is_file() {
        return prefix;
    }
    let src = match std::env::var_os("APEX_GHOSTTY_SRC") {
        Some(d) => PathBuf::from(d),
        None => {
            let src = cache.join("src");
            fetch(&src);
            src
        }
    };
    let zig = zig().unwrap_or_else(|| panic!("libghostty-vt needs zig 0.16 to build: put it on PATH or set ZIG"));
    run(
        Command::new(&zig)
            .current_dir(&src)
            .arg("build")
            .arg("-Demit-lib-vt=true")
            // the xcframework wants xcodebuild, and we link the archive
            .arg("-Demit-xcframework=false")
            .arg("-Doptimize=ReleaseFast")
            .arg("--summary")
            .arg("none")
            .arg("--prefix")
            .arg(&prefix),
        "zig build",
    );
    prefix
}

/// The pinned commit alone, fetched shallow into `src`.
fn fetch(src: &Path) {
    std::fs::create_dir_all(src).expect("cache directory");
    if !src.join(".git").is_dir() {
        run(Command::new("git").arg("init").arg("-q").current_dir(src), "git init");
        run(Command::new("git").args(["remote", "add", "origin", GHOSTTY_REPO]).current_dir(src), "git remote add");
    }
    run(Command::new("git").args(["fetch", "-q", "--depth", "1", "origin", GHOSTTY_COMMIT]).current_dir(src), "git fetch");
    run(Command::new("git").args(["checkout", "-q", "FETCH_HEAD"]).current_dir(src), "git checkout");
}

/// Zig 0.16, as `ZIG` says, on PATH, or where a hand install puts it.
fn zig() -> Option<PathBuf> {
    if let Some(z) = std::env::var_os("ZIG") {
        return Some(PathBuf::from(z));
    }
    if Command::new("zig").arg("version").output().is_ok_and(|o| o.status.success()) {
        return Some(PathBuf::from("zig"));
    }
    let home = std::env::var_os("HOME")?;
    let mut found: Vec<PathBuf> = std::fs::read_dir(Path::new(&home).join(".local"))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("zig-") && n.contains("-0.16")))
        .map(|p| p.join("zig"))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop()
}

fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(d).join("apex");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".cache/apex")
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd.status().unwrap_or_else(|e| panic!("{what}: {e}"));
    if !status.success() {
        panic!("{what}: {status}");
    }
}
