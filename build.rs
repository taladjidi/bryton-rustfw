//! The ANT network keys, if `ant_key.bin` / `di2_key.bin` exist (tools/ant_key.py copies them from your stock image).
//! They are not ours to publish (the ANT+ key is licensed; the second is Shimano's private network's, for their Di2
//! channel), so they never enter the repository: the build reads them from those gitignored files, and main.rs gets
//! `ANT_KEY` and `DI2_KEY`, each an `Option<[u8; 8]>` (None: no ANT+, or no Di2).

fn key(file: &str, name: &str, doc: &str) -> String {
    println!("cargo::rerun-if-changed={file}");
    let key = match std::fs::read(file) {
        Ok(k) if k.len() == 8 => format!("Some({k:?})"),
        Ok(_) => panic!("{file} must be 8 bytes (tools/ant_key.py)"),
        Err(_) => "None".into(),
    };
    format!("/// {doc} (build.rs).\npub const {name}: Option<[u8; 8]> = {key};\n")
}

fn main() {
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("ant_key.rs");
    let text = key("ant_key.bin", "ANT_KEY", "The ANT+ network key")
        + &key("di2_key.bin", "DI2_KEY", "Shimano's private ANT network key, for the Di2 channel");
    std::fs::write(out, text).unwrap();
}
