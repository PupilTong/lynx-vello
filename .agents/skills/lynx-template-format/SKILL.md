---
name: lynx-template-format
description: Byte-level knowledge of LynxJS template encodings — the native ".lynx.bundle" (lynx target) and the "SDRA WROF" web binary template (web target). Use when decoding/encoding Lynx bundles, working on crates/lynx-template-decoder, or answering questions about Lynx template internals.
---

# Lynx template formats

Two targets, two completely different binary formats:

## 1. Web target (`.web.bundle`) — implemented in `crates/lynx-template-decoder`

- Container: `u32le` magic pair `0x41524453 0x464F5257` ("SDRA WROF"), `u32 version == 1`,
  then repeated `{u32 label, u32 len, bytes}` sections to EOF.
- Labels: 1 Manifest (binary string map), 2 StyleInfo (rkyv), 3 LepusCode (binary
  string map), 4 CustomSections (UTF-16LE JSON), 5 ElementTemplates (reserved),
  6 Configurations (UTF-16LE JSON).
- Binary string map: `u32 count`, then `{u32 klen, key, u32 vlen, val}` × count (UTF-8).
- StyleInfo is **rkyv 0.7, size_32, root-at-end** serialization of `RawStyleInfo`
  (CSS pre-parsed to rules/selectors/declarations). The Rust mirror types in
  `crates/lynx-template-decoder/src/style_info.rs` ARE the wire format — never
  reorder fields or enum variants there.
- Reference impl: lynx-stack `packages/web-platform/web-core` (`ts/server/decode.ts`,
  `ts/encode/webEncoder.ts`, `src/template/template_sections/style_info/*.rs`).
- Web target main-thread code (`lepusCode`) is plain JS text, never bytecode.

## 2. Lynx native target (`.lynx.bundle`) — documented only

- `u32 total_size` (== file size), `u32 magic` (`0x00241922` LepusNG/QuickJS,
  `0xdd737199` legacy Lepus), four `u32len+utf8` version strings, "INFO"
  (`0x494E464F`) header-ext-info block of `{u8 type, u8 key, u16 size, payload}`
  fields, optional Lepus-value `template_info`, then `app_type` string, `u8 snapshot`,
  then sections (usually led by a SECTION_ROUTE `{u8 type, u32 start, u32 end}` table).
- Main-thread code is QuickJS bytecode (ROOT_LEPUS section); background JS in the
  JS section as `path → source` entries; CONFIG section is a JSON string.
- Encoded by `@lynx-js/tasm` (NAPI/wasm build of lynx repo's C++ encoder), decoded
  by `core/template_bundle/template_codec/binary_decoder/` in the lynx repo.

## Full specs

Read [docs/web-binary-template.md](../../../docs/web-binary-template.md) and
[docs/lynx-binary-template.md](../../../docs/lynx-binary-template.md) for the
complete byte layouts, enums (BinarySection, ValueType, CSSPropertyEnum…),
version gates, and source-file map into the local `~/repos/lynx` and
`~/repos/lynx-stack` checkouts.

## Fixtures

Real bundles for tests: `crates/lynx-template-decoder/tests/fixtures/` (vendored),
more in lynx-stack: `packages/web-platform/web-core-e2e/dist/*.web.bundle` (binary web),
`examples/*/dist/main.lynx.bundle` (native), `examples/react-externals/dist/main.web.bundle`
(legacy JSON web bundle).
