//! The ANT+ network key, if `ant_key.bin` exists (tools/ant_key.py copies it from your stock image). It is
//! licensed, so it never enters the repository: the build reads it from that gitignored file, and main.rs gets
//! `ANT_KEY: Option<[u8; 8]>` (None: no ANT+).

fn main() {
    println!("cargo::rerun-if-changed=ant_key.bin");
    let key = match std::fs::read("ant_key.bin") {
        Ok(k) if k.len() == 8 => format!("Some({k:?})"),
        Ok(_) => panic!("ant_key.bin must be 8 bytes (tools/ant_key.py)"),
        Err(_) => "None".into(),
    };
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("ant_key.rs");
    std::fs::write(out, format!("/// The ANT+ network key (build.rs).\npub const ANT_KEY: Option<[u8; 8]> = {key};\n"))
        .unwrap();
}
