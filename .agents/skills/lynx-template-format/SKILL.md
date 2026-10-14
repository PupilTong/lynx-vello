---
name: lynx-template-format
description: Byte-level knowledge of LynxJS template encodings — the native ".lynx.bundle" (lynx target) and the "SDRA WROF" web binary template (web target). Use when decoding/encoding Lynx bundles, working in crates/bobcat-source, or answering questions about Lynx template internals.
---

# Lynx template formats

Two targets, two completely different binary formats. Both are decoded by
`crates/bobcat-source`, the single owner of Lynx source parsing and adaptation.
See `docs/source-architecture.md` for the crate's boundaries and parser
resource bounds.

## 1. Web target (`.web.bundle`) — `crates/bobcat-source/src/web/`

- Container: `u32le` magic pair `0x41524453 0x464F5257` ("SDRA WROF"), `u32 version`
  (1 is the maximum accepted), then repeated `{u32 label, u32 len, bytes}` sections to EOF.
- Labels (`SectionLabel` in `src/web/mod.rs`): 1 Manifest (binary string map),
  2 StyleInfo (rkyv), 3 LepusCode (binary string map), 4 CustomSections (UTF-16LE JSON),
  5 ElementTemplates (retained as raw bytes), 6 Configurations (UTF-16LE JSON object),
  7 CustomSectionsStyleInfo (rkyv `StyleInfoMap { name_to_style_info: HashMap<String,
  StyleInfo> }`, web-core's `RawStyleInfoMap`, lynx-stack#4138: one `StyleInfo` per
  `encoding: 'CSS'` custom section, its rules under css id 0; written only when such a
  section exists, and those sections are then absent from the label-4 JSON).
  An unknown label is an error, not a skip.
- Named stylesheets (`src/custom_style.rs`): label 7, else a legacy label-4 JSON
  `{encoding:"CSS", content:{ruleList}}` descriptor (pre-#4138 encoders), else a
  `.lynx.bundle`'s CSS custom section. Each registers at
  `<container path>/custom_section/<encoded key>.css` with the `?#` suffix kept
  (`named_style_url` in `src/container.rs`, `styleSheetURL` in
  `packages/bobcat-element/src/section-url.ts`).
- Binary string map: `u32 count`, then `{u32 klen, key, u32 vlen, val}` × count (UTF-8),
  rejecting trailing bytes.
- StyleInfo is **rkyv 0.7, size_32, root-at-end** serialization of `RawStyleInfo`
  (CSS pre-parsed to rules/selectors/declarations). The Rust mirror types in
  `crates/bobcat-source/src/web/style_info.rs` ARE the wire format — never
  reorder fields or enum variants there. `rkyv` stays pinned at `0.7` for this
  reason (see AGENTS.md's Dependency policy).
- The `css_id_to_style_sheet` keys and each fragment's `imports` are live
  semantics, not just layout: `lower_style.rs` repeats a non-zero fragment's
  selectors per (transitively) importing fragment with
  `:where([l-css-id="N"])`, as web-core's decoder does.
- Validation bounds before a caller gets an owned tree: 1 MiB section length,
  validation subtree depth 72 (rkyv 0.7 `ArchiveValidator::with_max_depth`),
  returned rule depth 64. Decoding is synchronous and thread-free on every
  platform, Wasm included.
- Reference impl: lynx-stack `packages/web-platform/web-core` (`ts/server/decode.ts`,
  `ts/encode/webEncoder.ts`, `src/template/template_sections/style_info/*.rs`).
- Web target main-thread code (`lepusCode`) is plain JS text, never bytecode.

## 2. Lynx native target (`.lynx.bundle`) — `crates/bobcat-source/src/native/`

`bobcat-source::native` decodes **source-based flexible external bundles**
directly into the same `WebTemplate` model the web decoder produces
(`native::decode`), and `native::convert` re-encodes one as a web bundle.
Any executable root Lepus, Lepus chunk, `JS_BYTECODE` or `JsBytecode` custom
section returns `ConvertError::CodeCacheBundle`: real QuickJS/Lepus bytecode is
**rejected**, never executed or decompiled. Named external modules keep their
names and acquire no invented page root; `PageSource::from_native_bundle`
(over `ContainerSources::from_native_bundle`) requires an explicit entry
name, which it registers under its own name and, as the entry, at
`<container>/root.js`.

The layout it reads: `u32 total_size` (== file size), `u32 magic`
(`0x00241922` LepusNG/QuickJS, `0xdd737199` legacy Lepus), four `u32len+utf8`
version strings, an "INFO" (`0x494E464F`) header-ext-info block of
`{u8 type, u8 key, u16 size, payload}` fields, optional Lepus-value
`template_info`, then the `app_type` string, `u8 snapshot`, then sections
(usually led by a `SECTION_ROUTE` (10) `{u8 type, u32 start, u32 end}` table).
Main-thread code is QuickJS bytecode (ROOT_LEPUS section); background JS lives
in the JS section as `path → source` entries; the CONFIG section is a JSON
string. Decoding bounds Lepus/CSS recursion, rejects overlapping section
payloads, and caps CSS fallback expansion work.

Encoded by `@lynx-js/tasm` (NAPI/wasm build of the lynx repo's C++ encoder),
decoded upstream by `core/template_bundle/template_codec/binary_decoder/`.

## 3. The XML source front end — `crates/bobcat-source/src/xml.rs`

`.lynx.xml` is a **source** format, not a third bundle encoding: a
zero-dependency, zero-copy restricted envelope parser (`engine-version`,
`thread="main"` / `thread="background"`) that retains UTF-16 and UTF-8 error
offsets. See `docs/lynx-xml-template.md` for the exact grammar, section
extraction, errors and offsets, and the intentional CSS difference between the
merged XML-to-`.web.bundle` encoder and the raw web loader.

`ZipSource` (`crates/bobcat-source/src/archive.rs`) is the always-available
bounded ZIP decoder; entry selection goes through `PageSource`
(`src/page.rs`), which also registers resources. The crate has no Cargo
feature flags — every embedder, Wasm included, gets all three parsers.

## 4. One registration path for every container — `src/container.rs`

A `.web.bundle` or `.lynx.bundle` becomes a `ContainerSources`, whoever asked:
a page (`PageSource` holds one), a `<frame>`'s main bundle, or a fetch
(`BundleInstaller`, the one `ContainerInstaller`). With `U` the container
URL, `P` its path (one trailing `/` dropped), `S` its `?#` suffix and names
`form_urlencoded` (space `%20`) after one leading `/` is stripped:

| What | URL | Body |
| --- | --- | --- |
| Lepus `root` / explicit native entry | `P/root.js S` | `mts_entry_source` |
| Lepus chunk or string section `main-thread`/`*__main-thread` | `P/<name>.js S` | `MTS_CHUNK_PREAMBLE` + `export default ` + body |
| other Lepus chunk | `P/<name>.js S` | verbatim |
| other string custom section | `P/<name>.js S` | `.json` verbatim, else `bts_module_source` |
| manifest path `p` | `P/<p> S` (no `.js`; `?`/`#` in `p` escaped) | same choice by name |
| named stylesheet `k` | `P/custom_section/<k>.css S` | `PreparsedStyleSheet` |
| non-empty StyleInfo | `P/style-info.css S` | lowered `PreparsedStyleSheet` |
| BTS boot (entry + non-empty manifest) | `U#background-thread` | `__BobcatRegisterBundle(U)` + `requireModule('/app-service.js')` |

`packages/bobcat-element/src/section-url.ts` builds the same strings. Same URL
with the same text deduplicates; different text, or a sheet URL a script also
reaches, is `SourceError::ConflictingSection`; an opaque URL is
`SourceError::OpaqueContainerUrl`. The native decoder files a custom source
section `n` as both `custom_sections[n]` and manifest `/n`, so it lands at
both `P/n.js` and `P/n`. A page's view names `root.js` as its entry, `U` as
its `card_url` (`__Card__`) and `style-info.css` as its only listed sheet.
XML (`bobcat-memory://lynx-xml/*`) and ZIP (`bobcat-memory://archive/`) keep
their own URLs.

Registering a card's bodies includes their import wrappers, because the engine
loads every entry as the fetcher answered it: an MTS body (a container's root
Lepus script, an XML main-thread script) is prefixed with
`MTS_CHUNK_PREAMBLE` (`mts_entry_source`), a BTS body with
`BTS_CHUNK_PREAMBLE` (`bts_module_source`; an XML background-thread script
takes the `.web.bundle` `CommonJS` shape), each on the body's own first line,
so a line of the body keeps its number and a column on its first line is
offset by the prefix's length. A container's `main-thread`/`*__main-thread`
section is the MTS preamble, `export default ` and the body. Any other named
Lepus chunk and a `.json` body stay verbatim.

## Full specs

Read [docs/web-binary-template.md](../../../docs/web-binary-template.md) and
[docs/lynx-binary-template.md](../../../docs/lynx-binary-template.md) for the
complete byte layouts, enums (BinarySection, ValueType, CSSPropertyEnum…),
version gates, and the source-file map into the local reference checkouts.

## Fixtures

No compiled bundle is versioned in this repo. Test inputs are built from the
`packages/reactlynx-test-fixtures` pnpm workspace — run
`pnpm --filter reactlynx-test-fixtures build` before the Rust tests, which read
the generated `dist/index.rs` registry. The `basic-class-selector`,
`basic-bindtap` and `basic-performance-large-css` cards carry the CSS,
empty-StyleInfo and large-StyleInfo decoder cases. Decoder tests live in
`crates/bobcat-source/tests/` (`decode_web_bundle.rs`, `conversion.rs`,
`parser.rs`, `robustness.rs`, `zip_source.rs`). Real upstream bundles can be
found in the `lynx-stack/` checkout; see AGENTS.md "Reference repos" for its
path.
