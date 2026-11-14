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

/// One lazy bundle: its path relative to the page, as `fixtures::Fixture`'s
/// `chunks` names it, and its bytes.
pub type Chunk = (&'static str, &'static [u8]);

/// A native ReactLynx fixture (a `lynx-*` environment) encoded as Rsbuild
/// emits it. `name` is the key `fixtures::fixture` takes; the source lazy
/// chunks are the ones that fixture lists.
pub struct BytecodeFixture {
    pub name: &'static str,
    /// Main-thread script as bytecode, background script as source (`JS`).
    pub page: &'static [u8],
    /// Both as bytecode: `experimental_encodeQuickjsBytecode: true` adds a
    /// `JS_BYTECODE` section in place of `JS`.
    pub js_bytecode_page: &'static [u8],
    /// The fixture's lazy bundles at the paths `fixtures::Fixture::chunks`
    /// gives them, re-encoded from their retained inputs with the
    /// `main-thread` custom section as `JsBytecode`, as a production build
    /// writes it. The `background` (and `CSS`) custom sections are unchanged:
    /// `background` stays a string custom section. Empty for a fixture with
    /// no lazy bundle.
    pub chunks: &'static [Chunk],
    /// The same with `experimental_encodeQuickjsBytecode: true` as well. A
    /// lazy bundle's manifest is empty, so this adds an empty `JS_BYTECODE`
    /// section (no files) in place of `JS`; the background code is still the
    /// string custom section `background`.
    pub js_bytecode_chunks: &'static [Chunk],
}

/// A hand-written card of `bytecode-cards/<name>/`, encoded by
/// `scripts/bytecode-bundles.ts` with target SDK 4.1.0 (fiber architecture,
/// LepusNG). Its root is `ROOT_LEPUS`, so a page is booted with
/// `PageSource::from_bytes` and entry `root`.
pub struct Card {
    pub name: &'static str,
    /// Main-thread code as bytecode (`ROOT_LEPUS`, `LEPUS_CHUNK`, `JsBytecode`
    /// custom sections); the manifest, if any, as source (`JS`).
    pub page: &'static [u8],
    /// The same with `experimental_encodeQuickjsBytecode: true`: the manifest
    /// as `JS_BYTECODE` (an empty one for a card without a manifest).
    pub js_bytecode_page: &'static [u8],
    /// The lazy bundles the card fetches, at the path relative to the page
    /// it fetches them by, with their `main-thread` section as `JsBytecode`.
    pub chunks: &'static [Chunk],
    /// Console lines the bytecode forms log once the engine runs them, each
    /// exactly; lines from the two threads come in no fixed order.
    pub expected: &'static [&'static str],
    /// The same card with every script as a string custom section.
    pub source: SourceCard,
}

/// The source twin of a [`Card`], repacked the way
/// `scripts/source-bundles.ts` repacks a fixture's page: every body is a
/// string custom section, and the root is the section [`SourceCard::entry`],
/// which `PageSource::from_native_bundle` takes as the explicit entry.
pub struct SourceCard {
    pub page: &'static [u8],
    /// `<card>__main-thread`.
    pub entry: &'static str,
    /// `(section, chunk)` for each Lepus chunk of the card, carried as the
    /// string section `<chunk>__main-thread`. A `.lynx.bundle` cannot carry a
    /// source Lepus chunk under its own name (its `*__main-thread` sections
    /// load as `lynx.loadScript` modules), so a test that runs the root's
    /// `__LoadLepusChunk(chunk)` converts the page to a `.web.bundle` and
    /// renames `section` to `chunk` and `entry` to `root` in its `LepusCode`,
    /// as `named_lepus_chunks_load_and_run_on_every_call` in
    /// `crates/bobcat-source/tests/conversion.rs` does. Booted without that,
    /// `__LoadLepusChunk` answers `false`.
    pub lepus_chunks: &'static [(&'static str, &'static str)],
    /// The lazy bundles with their `main-thread` section as a string.
    pub chunks: &'static [Chunk],
    /// Console lines the source twin logs, each exactly.
    pub expected: &'static [&'static str],
}

/// The fixture of that name in [`FIXTURES`].
pub fn fixture(name: &str) -> &'static BytecodeFixture {
    FIXTURES
        .iter()
        .find(|fixture| fixture.name == name)
        .unwrap_or_else(|| panic!("unknown bytecode fixture: {name}"))
}

/// The card of that name in [`CARDS`].
pub fn card(name: &str) -> &'static Card {
    CARDS
        .iter()
        .find(|card| card.name == name)
        .unwrap_or_else(|| panic!("unknown bytecode card: {name}"))
}

include!("dist/bytecode/index.rs");
