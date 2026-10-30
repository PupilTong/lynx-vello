//! Compiles the built-in TypeScript ESMs into this Cargo build's output, and
//! each emitted module on to `QuickJS` bytecode.
//!
//! Install the workspace's locked Node dependencies before invoking Cargo.
//! Each target/profile owns its emitted JavaScript and bytecode under
//! `OUT_DIR`; no generated runtime files are read from or written to the
//! source tree.
//!
//! The bytecode is what a realm loads: `esm.rs` registers each module's
//! `.qbc` on the runtime, and a realm that imports it reads the module back
//! (`JS_ReadObject`) instead of parsing and compiling the text, which is
//! what makes a realm's boot cheap. It is written by the same
//! `quickjs-rust-bridge` the engine links, compiled for the build machine,
//! so the format is the engine's own; `QuickJS` bytecode names atoms by
//! their text and carries no pointer-sized or endian-specific layout, so
//! bytecode written here reads on every target the engine builds for.
//! Each module is compiled under the specifier a realm imports it by,
//! which is the name its errors and stack frames carry.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use quickjs_rust_bridge::{Runtime, RuntimeOptions};

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo always provides CARGO_MANIFEST_DIR"),
    );
    let workspace_dir = manifest_dir.join("../..");
    let package_dir = workspace_dir.join("packages/bobcat-element");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo always provides OUT_DIR"));

    // Directory tracking includes added and removed modules. Configuration and
    // compiler version changes also invalidate the emit; Cargo owns the cache.
    for input in [
        package_dir.join("src"),
        package_dir.join("scripts/build.ts"),
        package_dir.join("package.json"),
        workspace_dir.join("tsconfig.base.json"),
        workspace_dir.join("pnpm-lock.yaml"),
        workspace_dir.join("pnpm-workspace.yaml"),
    ] {
        println!("cargo:rerun-if-changed={}", input.display());
    }

    let status = Command::new("node")
        .arg(package_dir.join("scripts/build.ts"))
        .arg(out_dir.join("runtime"))
        .status()
        .expect(
            "runtime build requires Node.js; install Node and run `pnpm install --frozen-lockfile`",
        );
    assert!(
        status.success(),
        "runtime TypeScript compilation failed; install dependencies with `pnpm install --frozen-lockfile` and fix the compiler errors above",
    );

    compile_bytecode(
        &package_dir.join("src/tsconfig.json"),
        &out_dir.join("runtime"),
    );
}

/// Compiles every module `tsconfig` maps a specifier to, from the JavaScript
/// `tsc` emitted for it under `runtime`, to `<file>.qbc` beside it.
///
/// The specifier table is the `paths` of the built-ins' own `tsconfig`,
/// which `esm.rs`'s `BUILTIN_MODULES` is tested to equal, one entry per
/// line in the form `"bobcat:name": ["./name.ts"],`.
fn compile_bytecode(tsconfig: &Path, runtime: &Path) {
    let tsconfig_text = fs::read_to_string(tsconfig)
        .unwrap_or_else(|error| panic!("reading {}: {error}", tsconfig.display()));
    let paths: Vec<(&str, &str)> = tsconfig_text
        .lines()
        .map(str::trim)
        .skip_while(|line| *line != r#""paths": {"#)
        .skip(1)
        .take_while(|line| *line != "}")
        .map(|line| {
            let (specifier, file) = line
                .split_once(": ")
                .unwrap_or_else(|| panic!("one path per line in {}: {line}", tsconfig.display()));
            let file = file
                .trim_end_matches(',')
                .strip_prefix(r#"["./"#)
                .and_then(|file| file.strip_suffix(r#".ts"]"#))
                .unwrap_or_else(|| panic!("one file per path in {}: {line}", tsconfig.display()));
            (specifier.trim_matches('"'), file)
        })
        .collect();
    assert!(
        !paths.is_empty(),
        "{} maps no specifier",
        tsconfig.display()
    );

    // The built-ins' source text is not kept: nothing in the engine reads a
    // built-in function's text back, and leaving it out keeps the bytecode,
    // and so every realm's copy of it, at a fraction of the size. Line and
    // column information stays for stack traces.
    let compiler = Runtime::with_options(RuntimeOptions {
        strip_source: true,
        ..RuntimeOptions::default()
    })
    .expect("the bytecode compiler's runtime builds");
    let realm = compiler
        .create_context()
        .expect("the bytecode compiler's realm opens");
    for (specifier, file) in paths {
        let source_path = runtime.join(format!("{file}.js"));
        let source = fs::read_to_string(&source_path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", source_path.display()));
        let bytecode = realm
            .compile_module_bytecode(specifier, &source)
            .unwrap_or_else(|error| {
                panic!("compiling {specifier} ({}): {error}", source_path.display())
            });
        let bytecode_path = runtime.join(format!("{file}.qbc"));
        fs::write(&bytecode_path, bytecode)
            .unwrap_or_else(|error| panic!("writing {}: {error}", bytecode_path.display()));
    }
}
