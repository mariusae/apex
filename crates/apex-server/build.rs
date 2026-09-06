//! The build id: a hash of every source file in the workspace, so a
//! daemon and a client can tell whether they are the same apex. The same
//! sources give the same id on every target, which is what lets a client
//! compare itself with the binary it carries for a remote host.

use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn main() {
    use sha2::Digest;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = root.join("crates");
    println!("cargo:rerun-if-changed={}", crates.display());
    println!("cargo:rerun-if-changed={}", root.join("Cargo.lock").display());
    let mut files = Vec::new();
    walk(&crates, &mut files);
    files.push(root.join("Cargo.lock"));
    files.sort();
    let mut h = sha2::Sha256::new();
    for f in &files {
        if let Ok(bytes) = std::fs::read(f) {
            h.update(f.strip_prefix(&root).unwrap_or(f).to_string_lossy().as_bytes());
            h.update(b"\0");
            h.update(&bytes);
            h.update(b"\0");
        }
    }
    let id = format!("{:x}", h.finalize());
    println!("cargo:rustc-env=APEX_BUILD_ID={}", &id[..12]);
}
