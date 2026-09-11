//! Refuses to build over a stale realm runtime.
//!
//! The ESMs every realm preloads are TypeScript in
//! `packages/bobcat-element/src`, and what core embeds is the JavaScript
//! TypeScript 7 emitted for them into `packages/bobcat-element/dist`, which is
//! committed so that a cargo build needs no Node. Each emitted file records the
//! FNV-1a hash of the source it came from. A source that no longer hashes to
//! that value was edited after the emit, and embedding the file would build
//! the older code into the engine, so the build fails instead, naming the
//! source and the command that regenerates `dist/`.

use std::path::{Path, PathBuf};
use std::{env, fs};

const REGENERATE: &str = "run `pnpm --filter bobcat-element build`";

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo always provides CARGO_MANIFEST_DIR"),
    );
    let package_dir = manifest_dir.join("../../packages/bobcat-element");
    let (src_dir, dist_dir) = (package_dir.join("src"), package_dir.join("dist"));
    // Cargo rescans a directory whole, so any source or emit change reruns this.
    println!("cargo:rerun-if-changed={}", src_dir.display());
    println!("cargo:rerun-if-changed={}", dist_dir.display());

    let entries =
        fs::read_dir(&src_dir).unwrap_or_else(|error| panic!("{}: {error}", src_dir.display()));
    for entry in entries {
        let source_path = entry
            .unwrap_or_else(|error| panic!("{}: {error}", src_dir.display()))
            .path();
        let Some(name) = module_name(&source_path) else {
            continue;
        };
        let source = fs::read(&source_path)
            .unwrap_or_else(|error| panic!("{}: {error}", source_path.display()));
        let emitted_path = dist_dir.join(format!("{name}.js"));
        let emitted = fs::read_to_string(&emitted_path)
            .unwrap_or_else(|error| panic!("{}: {error}; {REGENERATE}", emitted_path.display()));
        let recorded = emitted
            .lines()
            .take(2)
            .find_map(|line| line.strip_prefix("// source fnv1a64 "));
        let actual = format!("{:016x}", fnv1a64(&source));
        assert!(
            recorded == Some(actual.as_str()),
            "{} is not the emit of {}: the source changed after it was emitted; {REGENERATE}",
            emitted_path.display(),
            source_path.display(),
        );
    }
}

/// The module a `.ts` file is, by base name. A declaration file
/// (`native.d.ts`) is not one: it carries no code.
fn module_name(path: &Path) -> Option<&str> {
    let name = path.file_stem()?.to_str()?;
    let is_module =
        path.extension()? == "ts" && Path::new(name).extension().is_none_or(|ext| ext != "d");
    is_module.then_some(name)
}

/// FNV-1a over the bytes; `packages/bobcat-element/scripts/build.ts` agrees.
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}
