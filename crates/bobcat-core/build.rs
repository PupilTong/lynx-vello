//! Strips the types from the realm runtime's TypeScript.
//!
//! The ESMs every realm preloads are TypeScript in
//! `packages/bobcat-element/src`, and `QuickJS` runs JavaScript. Each module's
//! types are erased by swc's strip-only mode — the stripper Node's own type
//! stripping uses — which overwrites every type with whitespace and moves
//! nothing else, so a line and column `QuickJS` reports is the line and column
//! in the `.ts` file. It accepts only erasable syntax, which the package's
//! `erasableSyntaxOnly` already demands: anything that would need type
//! information to lower fails the build here, naming the file and the span.

use std::path::{Path, PathBuf};
use std::{env, fs};

use swc_common::SourceMap;
use swc_common::errors::{HANDLER, Handler};
use swc_common::sync::Lrc;
use swc_ts_fast_strip::{Mode, Options, operate};

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo always provides CARGO_MANIFEST_DIR"),
    );
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo always provides OUT_DIR"))
        .join("bobcat-element");
    let runtime_dir = manifest_dir.join("../../packages/bobcat-element/src");
    // Cargo rescans a directory whole, so a module added there reruns this.
    println!("cargo:rerun-if-changed={}", runtime_dir.display());

    fs::create_dir_all(&out_dir).expect("OUT_DIR is writable");
    let entries = fs::read_dir(&runtime_dir)
        .unwrap_or_else(|error| panic!("{}: {error}", runtime_dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("{}: {error}", runtime_dir.display()))
            .path();
        if let Some(name) = module_name(&path) {
            fs::write(out_dir.join(format!("{name}.js")), strip_types(&path))
                .expect("OUT_DIR is writable");
        }
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

fn strip_types(path: &Path) -> String {
    let source =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let source_map = Lrc::<SourceMap>::default();
    // Diagnostics go to stderr, which Cargo prints when this script fails.
    let handler =
        Handler::with_emitter_writer(Box::new(std::io::stderr()), Some(source_map.clone()));
    // Strip-only mode reports unsupported syntax through the scoped handler
    // alone; outside one it would pass the syntax through unreported.
    HANDLER
        .set(&handler, || {
            operate(
                &source_map,
                &handler,
                source,
                Options {
                    module: Some(true),
                    filename: Some(path.display().to_string()),
                    mode: Mode::StripOnly,
                    ..Options::default()
                },
            )
        })
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .code
}
