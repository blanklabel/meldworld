//! DG-2 codegen: compile authored dungeons into the binary, with the build as the
//! correctness gate.
//!
//! Every `content/**/*.dungeon.toml` is run through the *real* parser + validator
//! ([`meld_dungeon::parse_and_validate`], including the entrance→exit solvability
//! search). If any file is malformed or unsolvable, the build **fails** with a
//! located error — an agent iterating on a dungeon gets a hard compile error, not a
//! broken map that ships. The validated defs are serialized to `$OUT_DIR/dungeons.json`
//! and embedded by `lib.rs`.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let content = manifest.join("content");
    println!("cargo:rerun-if-changed=content");
    println!("cargo:rerun-if-changed=build.rs");

    let mut files = Vec::new();
    collect(&content, &mut files);
    files.sort();

    let mut defs = Vec::new();
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let src = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("dungeon content: cannot read {}: {e}", path.display()));
        let rel = path.strip_prefix(&manifest).unwrap_or(path);
        match meld_dungeon::parse_and_validate(&src) {
            Ok(def) => defs.push(def),
            Err(errs) => {
                // One hard, located compile error per broken dungeon.
                let mut msg = format!("\n\nauthored dungeon {} is invalid:\n", rel.display());
                for e in &errs {
                    msg.push_str(&format!("  - {e}\n"));
                }
                panic!("{msg}");
            }
        }
    }

    // Deterministic order (files were sorted; keep defs stable by name too).
    defs.sort_by(|a, b| a.name.cmp(&b.name));

    // Reject two dungeons claiming the same name (the runtime keys on it).
    for w in defs.windows(2) {
        if w[0].name == w[1].name {
            panic!("two authored dungeons share the name {:?}", w[0].name);
        }
    }

    let json = serde_json::to_string(&defs).expect("serialize dungeon registry");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("dungeons.json");
    std::fs::write(&out, json).expect("write embedded dungeon registry");
    // Note: build-time validation is silent on success and a hard `panic!` (compile
    // error) on failure — we deliberately don't emit a `cargo:warning` on success,
    // which would show as a spurious warning on every build.
}

/// Recursively collect every `*.dungeon.toml` under `dir` (missing dir = none).
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".dungeon.toml")) {
            out.push(path);
        }
    }
}
