//! Compiles the built-in TypeScript ESMs into this Cargo build's output.
//!
//! Install the workspace's locked Node dependencies before invoking Cargo.
//! Each target/profile owns its emitted JavaScript under `OUT_DIR`; no
//! generated runtime files are read from or written to the source tree.

use std::env;
use std::path::PathBuf;
use std::process::Command;

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
}
