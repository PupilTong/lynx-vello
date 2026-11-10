//! Native bundles carrying PrimJS bytecode, for tests of the PrimJS-to-QuickJS
//! bytecode converter. Run `pnpm --filter reactlynx-test-fixtures build`
//! before compiling tests; `scripts/bytecode-bundles.ts` writes the registry.
#![allow(dead_code)] // Each consumer uses a subset of the generated bundles.

/// The encoder options a corpus bundle was built with. Every variant is a
/// fiber-architecture LepusNG page; each carries the program twice, as the
/// main-thread script (`ROOT_LEPUS`) and as the background script
/// `/app-service.js` (`JS_BYTECODE`, packed header).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    /// `targetSdkVersion: "2.13"`, `debugInfoOutside: true`. `ROOT_LEPUS` has
    /// version byte 1 and per-function debug blocks naming the atom
    /// `__lepusNG_function_id__` with a function id and no pc2line;
    /// `JS_BYTECODE` has version byte 1 and no debug blocks.
    Sdk2_13,
    /// `targetSdkVersion: "2.14"`, `debugInfoOutside: true`. Both sections
    /// have version byte 9 with `NEW_DEBUGINFO_FLAG`.
    Sdk2_14,
    /// `targetSdkVersion: "4.1"`, `debugInfoOutside: true`. `ROOT_LEPUS` adds
    /// `STRIP_VARINFO_FLAG` (no vardefs); `JS_BYTECODE` is as for 2.14.
    Sdk4_1,
    /// `targetSdkVersion: "4.1"`, `debugInfoOutside: false`. `ROOT_LEPUS` has
    /// version byte 1 and inline debug info (filename, line, pc2line);
    /// `JS_BYTECODE` is as for 2.14.
    InlineDebug,
}

impl Variant {
    pub const ALL: [Variant; 4] = [
        Variant::Sdk2_13,
        Variant::Sdk2_14,
        Variant::Sdk4_1,
        Variant::InlineDebug,
    ];

    /// The bundle's file stem under `dist/bytecode/corpus/<program>/`.
    pub const fn name(self) -> &'static str {
        match self {
            Variant::Sdk2_13 => "sdk-2.13",
            Variant::Sdk2_14 => "sdk-2.14",
            Variant::Sdk4_1 => "sdk-4.1",
            Variant::InlineDebug => "inline-debug",
        }
    }
}

/// One program of `bytecode-corpus/`. Run as a script, it assigns a string to
/// `globalThis.__result`, synchronously or from a promise job.
pub struct Program {
    pub name: &'static str,
    pub source: &'static str,
    /// One bundle per [`Variant`], in [`Variant::ALL`] order.
    pub bundles: &'static [(Variant, &'static [u8])],
}

impl Program {
    pub fn bundle(&self, variant: Variant) -> &'static [u8] {
        self.bundles
            .iter()
            .find(|(v, _)| *v == variant)
            .map(|(_, bytes)| *bytes)
            .expect("every variant is built")
    }
}

/// A native ReactLynx fixture (a `lynx-*` environment) encoded as Rsbuild
/// emits it. `name` is the key `fixtures::fixture` takes; lazy chunks are the
/// ones that fixture lists.
pub struct BytecodeFixture {
    pub name: &'static str,
    /// Main-thread script as bytecode, background script as source (`JS`).
    pub page: &'static [u8],
    /// Both as bytecode: `experimental_encodeQuickjsBytecode: true` adds a
    /// `JS_BYTECODE` section in place of `JS`.
    pub js_bytecode_page: &'static [u8],
}

include!("dist/bytecode/index.rs");
