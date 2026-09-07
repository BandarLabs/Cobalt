use std::{env, fmt::Write, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("crate path")).join("../..");
    let mut paths = Vec::new();
    for group in ["apps", "examples"] {
        let folder = root.join(group);
        println!("cargo:rerun-if-changed={}", folder.display());
        for entry in fs::read_dir(folder).expect("app directories") {
            let path = entry.expect("app directory").path().join("cobalt-app.json");
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut source = String::from("const SOURCES: &[&str] = &[\n");
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        writeln!(
            source,
            "include_str!({:?}),",
            path.to_str().expect("UTF-8 manifest path")
        )
        .expect("write source");
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("output")).join("catalog.rs"),
        source,
    )
    .expect("write catalog sources");
}
