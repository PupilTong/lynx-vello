//! Every JavaScript file under the repository's `node_modules` the fork can
//! compile passes the verifier: the rules are not stricter than the
//! compiler's habits. Ignored by default (it takes minutes); run with
//! `cargo test -p quickjs-rust-bridge wide -- --ignored --nocapture`.
//! `BYTECODE_WIDE_LIMIT` caps the number of files (default 20000).

use std::path::{Path, PathBuf};

use super::{assert_round_trip, check};
use crate::Runtime;
use crate::bytecode::read_program;

fn javascript_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            // Symbolic links (pnpm's layout) are not followed: every
            // package is reached once through `.pnpm`.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file()
                && path.extension().is_some_and(|extension| {
                    matches!(extension.to_str(), Some("js" | "mjs" | "cjs"))
                })
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files.truncate(limit);
    files
}

#[test]
#[ignore = "walks node_modules; takes minutes"]
fn every_compilable_file_in_node_modules_verifies() {
    let limit = std::env::var("BYTECODE_WIDE_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_000);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node_modules");
    let files = javascript_files(&root, limit);
    let (mut scripts, mut modules, mut skipped, mut too_large) = (0usize, 0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    let mut runtime = Runtime::new().unwrap();
    for (index, path) in files.iter().enumerate() {
        let Ok(source) = std::fs::read_to_string(path) else {
            skipped += 1;
            continue;
        };
        if source.len() > 4 << 20 {
            too_large += 1;
            continue;
        }
        let name = path.to_string_lossy();
        let compiler = runtime.create_context().unwrap();
        let outcome = compiler
            .compile_script_bytecode(&name, &source)
            .map(|bytes| (bytes, false))
            .or_else(|_| {
                compiler
                    .compile_module_bytecode(&format!("app:///{index}"), &source)
                    .map(|bytes| (bytes, true))
            });
        let Ok((bytes, is_module)) = outcome else {
            skipped += 1;
            continue;
        };
        drop(compiler);
        if is_module {
            modules += 1;
        } else {
            scripts += 1;
        }
        assert_round_trip(&bytes, &name);
        let program = read_program(&bytes).unwrap();
        let checker = runtime.create_context().unwrap();
        if let Err(error) = check(&program, &checker) {
            failures.push(format!("{name}: {error}"));
        }
        drop(checker);
        if index % 500 == 0 {
            runtime.run_gc();
        }
    }
    println!(
        "{} files: {scripts} scripts, {modules} modules verified; {skipped} not compiled, \
         {too_large} too large; {} refused",
        files.len(),
        failures.len()
    );
    for failure in failures.iter().take(50) {
        println!("  {failure}");
    }
    assert!(failures.is_empty(), "{} refused", failures.len());
}
