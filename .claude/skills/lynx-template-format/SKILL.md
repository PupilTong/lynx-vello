---
name: lynx-template-format
description: Reference for the LynxJS / ReactLynx native binary template format (template-bundle magic 0x00241922) — container envelope, sections, the new element-template (fiber) encoding, CSS/style-object encoding, feature switches, and how it maps to the reactlynx-decoder crate. Use when implementing, debugging, or extending decoding/encoding of Lynx template bundles, or when reasoning about which feature path (legacy vs latest) applies.
---

# LynxJS native binary template format

The canonical output ReactLynx ships to **native** Lynx engines is a binary
template bundle (magic `0x00241922`, LepusNG/QuickJS). This skill indexes the
reverse-engineered format reference and states the decoder's scope.

> There are **two unrelated** Lynx template formats. This skill is about the
> **native** binary (`0x00241922`, produced by the C++ `TemplateBinaryWriter`
> via `@lynx-js/tasm`). It is **not** the web-platform format (`webEncoder.ts`,
> `SDRAWROF` container) handled by `web-core`. A native renderer targets the
> native binary; `web-core`'s Rust crate is only an idiom reference.

## Reference docs (read these for byte-level detail)

All under `docs/lynx/`:

- `01-container-format.md` — header, magic/versions, `header_ext_info`
  (`0x494e464f`), section route table, `lepus::Value`, primitive encoding.
- `02-element-template.md` — `NEW_ELEMENT_TEMPLATE` (the fiber element tree),
  node/section tags, attribute bindings, the router layout.
- `03-css-styles.md` — CSS fragments, parse tokens, `CSSValue`, keyframes/
  font-face, `STYLE_OBJECT` and `PARSED_STYLES`.
- `04-feature-switches.md` — compile options and version gates that select
  legacy vs latest; the table of what's in/out of the latest subset.
- `05-pipeline-and-rust-idioms.md` — ReactLynx encode pipeline; native-vs-web
  clarification; Rust patterns to mirror.
- `06-decoder-implementation-plan.md` — the authoritative implementation plan
  (scope, module layout, public API, task breakdown, test strategy).

## Load-bearing facts

- **`Compact*` integers are fixed-width little-endian, NOT LEB128.**
  `CompactU32`/`CompactS32` = 4 bytes LE, `CompactU64`/`CompactD64` = 8 bytes
  LE/IEEE-754. The `// leb128` comment in the C++ is vestigial. The decoder
  isolates this behind one `Reader` method (single swap point) — verified
  against `binary_input_stream_unittest.cc` (`"test"` → `0x74736574`).
- **No string table.** Strings are inline, length-prefixed: `CompactU32 len`
  then raw UTF-8. The `STRING` section slot is a no-op.
- **Latest = fiber.** Decode `NEW_ELEMENT_TEMPLATE` (BinarySection 17),
  `STYLE_OBJECT` (18), `PARSED_STYLES` (13). Skip legacy `ELEMENT_TEMPLATE`
  (12), radon/virtual-node tree, TTML, Air mode.
- **Section enums** live in `lynx/core/template_bundle/template_codec/template_binary.h`.
  `BinarySection` and `BinaryOffsetType` are **not** aligned (e.g.
  `NEW_ELEMENT_TEMPLATE` is `BinarySection 17` but `BinaryOffsetType 21`).

## Source of truth

- C++ codec: `/Users/akiwah/repos/lynx/core/template_bundle/template_codec`
  (`binary_decoder/` readers, `binary_encoder/` writers).
- ReactLynx SWC transforms: `/Users/akiwah/repos/lynx-stack/packages/react/transform/crates`
  (`swc_plugin_snapshot`, `swc_plugin_element_template`).
- Rust idiom reference: `/Users/akiwah/repos/lynx-stack/packages/web-platform/web-core/src`.
