//! Shared inputs for native/Wasm tests and benchmarks.
//! Run `pnpm --filter reactlynx-test-fixtures build` before compiling tests.
#![allow(dead_code)] // Each consumer uses a subset of the generated fixtures.

pub struct Fixture {
    pub page: &'static [u8],
    pub chunks: &'static [(&'static str, &'static [u8])],
    pub public_path: Option<&'static str>,
}

// The compiler determines chunk names and development asset URLs. Embed the
// generated index so Wasm tests need neither a filesystem nor a dev server.
include!("dist/index.rs");
