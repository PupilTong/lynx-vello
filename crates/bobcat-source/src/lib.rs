//! Parse Lynx XML, web/native source bundles and ZIP resource archives.
//!
//! XML always borrows its input. Web decoding uses the rkyv 0.7 wire model;
//! native decoding and native-to-web conversion use that same model. All
//! parsers and Bobcat source registration are always available to every
//! embedder. This crate neither owns IO nor launches a view.
#![forbid(unsafe_code)]

mod archive;
mod container;
mod custom_style;
mod lower_style;
pub mod native;
mod page;
pub mod web;
pub mod xml;
// The compiled cards, for the unit tests that hold a page's registration up
// against a fetch's. Run `pnpm --filter reactlynx-test-fixtures build` first.
#[cfg(test)]
#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;
pub use archive::*;
pub use container::{BundleInstaller, ContainerSources};
pub use page::*;

/// Every compiled web bundle the fixture packages built, from disk.
#[cfg(all(test, not(target_arch = "wasm32")))]
fn compiled_web_bundles() -> Vec<(String, Vec<u8>)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages");
    let mut bundles = Vec::new();
    let mut pending = vec![
        root.join("reactlynx-test-fixtures/dist"),
        root.join("web-core-e2e-fixtures/dist"),
    ];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(bytes) = std::fs::read(&path)
                && page::looks_like_web_bundle(&bytes)
            {
                bundles.push((path.display().to_string(), bytes));
            }
        }
    }
    bundles.sort();
    bundles
}
