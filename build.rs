//! Bakes the syntax set into the binary: syntect's own syntaxes plus the ones
//! vendored under `assets/syntaxes`, dumped here instead of parsed at run time.
//!
//! A file syntect cannot load is skipped with a warning, so one bad syntax
//! never costs the build.

use std::path::Path;

use syntect::dumps::dump_to_file;
use syntect::parsing::{SyntaxDefinition, SyntaxSet};

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");
    println!("cargo:rerun-if-changed=build.rs");

    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    let mut files: Vec<_> = walk(Path::new("assets/syntaxes"));
    files.sort();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("vendored syntax is readable");
        let name = file.to_string_lossy().to_string();
        match SyntaxDefinition::load_from_str(&text, true, Some(&name)) {
            Ok(syntax) => builder.add(syntax),
            Err(e) => println!("cargo:warning=skipped {name}: {e}"),
        }
    }

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("syntaxes.bin");
    dump_to_file(&builder.build(), out).expect("syntax dump");
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "sublime-syntax") {
            out.push(path);
        }
    }
    out
}
