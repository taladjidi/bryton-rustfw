//! The ANT network keys, if `ant_key.bin` / `di2_key.bin` exist (tools/ant_key.py copies them from your stock image).
//! They are not ours to publish (the ANT+ key is licensed; the second is Shimano's private network's, for their Di2
//! channel), so they never enter the repository: the build reads them from those gitignored files, and main.rs gets
//! `ANT_KEY` and `DI2_KEY`, each an `Option<[u8; 8]>` (None: no ANT+, or no Di2).
//! Also the commit the build is made from, for ABOUT and the dev link's `info`: `GIT_DESCRIBE` (`git describe
//! --always --dirty`: the short hash, "-dirty" when the tree has changes) and `GIT_DATE` (the commit's date, so a
//! build of the same commit says the same); "unknown" outside a git checkout.

fn key(file: &str, name: &str, doc: &str) -> String {
    println!("cargo::rerun-if-changed={file}");
    let key = match std::fs::read(file) {
        Ok(k) if k.len() == 8 => format!("Some({k:?})"),
        Ok(_) => panic!("{file} must be 8 bytes (tools/ant_key.py)"),
        Err(_) => "None".into(),
    };
    format!("/// {doc} (build.rs).\npub const {name}: Option<[u8; 8]> = {key};\n")
}

/// The output of `git args`, trimmed; "unknown" if git fails.
fn git(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".into(), |s| s.trim().into())
}

fn main() {
    // the commit moves with HEAD and with what it points to; -dirty with the index (a worktree's .git is a file:
    // these paths are then missing, which only means fewer rebuilds)
    for p in [".git/HEAD", ".git/index", ".git/refs/heads"] {
        println!("cargo::rerun-if-changed={p}");
    }
    println!("cargo::rustc-env=GIT_DESCRIBE={}", git(&["describe", "--always", "--dirty", "--abbrev=7"]));
    println!("cargo::rustc-env=GIT_DATE={}", git(&["log", "-1", "--format=%cs"]));
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("ant_key.rs");
    let text = key("ant_key.bin", "ANT_KEY", "The ANT+ network key")
        + &key("di2_key.bin", "DI2_KEY", "Shimano's private ANT network key, for the Di2 channel");
    std::fs::write(out, text).unwrap();
}
