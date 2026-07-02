# Lynx web binary template (`.web.bundle`)

Knowledge distilled from `/Users/akiwah/repos/lynx-stack` (`packages/web-platform/*`,
`packages/webpack/template-webpack-plugin`), verified against real bundles in
`packages/web-platform/web-core-e2e/dist/*.web.bundle`.

This is the **"web" target** template format. It is *not* the native
`.lynx.bundle` format (see [lynx-binary-template.md](lynx-binary-template.md)) —
the web platform never ships Lepus/QuickJS bytecode; its main-thread code is
plain JavaScript, and its CSS is pre-parsed into rkyv-serialized Rust structs.

`crates/lynx-template-decoder` in this repo implements a native Rust decoder
for exactly this format.

## Two encodings, one target

`@lynx-js/template-webpack-plugin` `WebEncodePlugin` picks the encoding via the
`EXPERIMENTAL_USE_WEB_BINARY_TEMPLATE` env var:

- **JSON** (legacy/debug, env var set to `'false'`/`'0'`): the `TasmJSONInfo`
  object serialized as JSON — `{styleInfo, manifest, cardType, appType,
  pageConfig, lepusCode, customSections, elementTemplates}`.
- **Binary** (default): `encode(tasmJSONInfo)` from `@lynx-js/web-core/encode`
  (`ts/encode/webEncoder.ts`) → the format below.

## Binary container layout

All integers **little-endian u32** unless noted. Constants from
`packages/web-platform/web-core/ts/constants.ts`.

```
u32  magic0    0x41524453   bytes "SDRA"
u32  magic1    0x464F5257   bytes "WROF"
u32  version   1            (decoder rejects version > 1)
     repeated until EOF:
       u32  section_label
       u32  section_length  in bytes
       [section_length bytes of section data]
```

Section labels (`TemplateSectionLabel`):

| Label | Name | Payload encoding |
| --- | --- | --- |
| 1 | Manifest | binary string map |
| 2 | StyleInfo | rkyv-serialized `RawStyleInfo` (see below) |
| 3 | LepusCode | binary string map |
| 4 | CustomSections | UTF-16LE JSON |
| 5 | ElementTemplates | (reserved; current encoder never emits it, runtime ignores it) |
| 6 | Configurations | UTF-16LE JSON |

The current encoder writes sections in the order: Configurations, LepusCode,
CustomSections, StyleInfo, Manifest. Decoders must not rely on order (the
reference decoder loops on labels), except that `Configurations` is needed to
interpret StyleInfo flags at runtime.

### Payload encodings

**Binary string map** (`ts/common/decodeUtils.ts` / `webEncoder.ts`):

```
u32 count
count × { u32 key_len, key_len bytes UTF-8 key,
          u32 val_len, val_len bytes UTF-8 value }
```

Used for `manifest` (chunk path → JS source) and `lepusCode`
(`root` + chunk names → main-thread JS source).

**UTF-16LE JSON**: `JSON.stringify` output written as one `u16` code unit per
JS char (i.e. UTF-16LE without BOM). Used for `Configurations` and
`CustomSections`.

- Configurations content: flat string map — `cardType`, `isLazy`
  (`"true"` when appType != `"card"`), plus every `pageConfig` entry stringified
  (`enableCSSSelector`, `enableRemoveCSSScope`, `defaultDisplayLinear`,
  `defaultOverflowVisible`, `enableJSDataProcessor`, …).
- CustomSections content: `Record<string, { type?: 'lazy', content: string | object }>`.

### StyleInfo: rkyv-serialized CSS

Encoded by Rust (wasm) in `packages/web-platform/web-core/src/template/template_sections/style_info/`
with **rkyv 0.7**, default features (`size_32` ⇒ archived `usize`/lengths are u32),
via `rkyv::to_bytes::<_, 1024>` and read back with `rkyv::from_bytes_unchecked`.
rkyv is a *root-at-the-end* format: the archived root struct sits at the end of
the buffer.

Data model (field order matters — it defines the archived layout):

```rust
RawStyleInfo   { css_id_to_style_sheet: HashMap<i32, StyleSheet>,  // Fnv hasher in source; hasher irrelevant to wire format
                 style_content_str_size_hint: usize }
StyleSheet     { imports: Vec<i32>, rules: Vec<Rule> }
Rule           { rule_type: RuleType, prelude: RulePrelude,
                 declaration_block: DeclarationBlock, nested_rules: Vec<Rule> }
RuleType       { Declaration = 1, FontFace = 2, KeyFrames = 3 }        // #[repr(i32)] in source
RulePrelude    { selector_list: Vec<Selector> }   // for KeyFrames: single selector holding prelude text; FontFace: empty
Selector       { simple_selectors: Vec<OneSimpleSelector> }
OneSimpleSelector      { selector_type: OneSimpleSelectorType, value: String }
OneSimpleSelectorType  { Class=1, Id=2, Attribute=3, Type=4, Combinator=5,
                         PseudoClass=6, PseudoElement=7, Universal=8, UnknownText=9 }
DeclarationBlock       { declarations: Vec<ParsedDeclaration> }
ParsedDeclaration      { property_id: CSSProperty, value_token_list: Vec<ValueToken>,
                         is_important: bool }
CSSProperty    { id: CSSPropertyEnum, unknown_name: Option<String> }   // unknown_name set iff id == Unknown
CSSPropertyEnum  // #[repr(u32)], 216 variants: Unknown=0, Top=1 … OffsetDistance=215
                 // canonical name list = STYLE_PROPERTY_MAP in css_property.rs
ValueToken     { token_type: u8, value: String }   // token_type from web-core's css_tokenizer
```

`css_id_to_style_sheet` keys are the per-entry CSS fragment ids (`cssId`);
`imports` model `@import` between fragments (flattening uses Kahn topological
sort in `flattened_style_info.rs`).

At runtime the wasm `decode_style_info(buffer, entry_name, enable_css_selector,
transform_vw, transform_vh, transform_rem)` deserializes `RawStyleInfo`, runs
`StyleInfoDecoder` (selector rewriting to HTML — e.g. type selectors become
`[lynx-tag="…"]`-style rules — plus vw/vh/rem transforms) and re-serializes a
`DecodedStyleData { style_content: Option<String>, font_face_content:
Option<String>, css_og_… map }` back over the wasm boundary.

## Runtime decode pipeline (reference implementation)

`packages/web-platform/web-core/ts/server/decode.ts` `decodeTemplate()`:

1. Check magic0/magic1, read version (must be ≤ 1).
2. Loop `label + length + content` to EOF.
3. Configurations → UTF-16LE → `JSON.parse` → `config`.
4. LepusCode → binary string map → `lepusCode: Record<string, Uint8Array>`.
5. CustomSections → UTF-16LE → `JSON.parse`.
6. StyleInfo → wasm `decode_style_info(...)` (flags read from `config`).
7. Manifest / ElementTemplates → ignored server-side; unknown labels → error.

## Where things live (lynx-stack repo)

| Path | What |
| --- | --- |
| `packages/web-platform/web-core/ts/constants.ts` | Magic + section labels |
| `packages/web-platform/web-core/ts/encode/webEncoder.ts` | Binary encoder + `TasmJSONInfo` type |
| `packages/web-platform/web-core/ts/encode/encodeCSS.ts` | styleInfo → Rust `RawStyleInfo` builder |
| `packages/web-platform/web-core/ts/server/decode.ts` | Reference decoder |
| `packages/web-platform/web-core/ts/common/decodeUtils.ts` | Binary string map |
| `packages/web-platform/web-core/src/template/template_sections/style_info/*.rs` | rkyv CSS data model |
| `packages/web-platform/web-core/binary/{encode,client,server}` | Prebuilt wasm |
| `packages/webpack/template-webpack-plugin/src/WebEncodePlugin.ts` | Web-target emit plugin |
| `packages/web-platform/web-core-e2e/dist/*.web.bundle` | Real binary fixtures |

Rust crates already in lynx-stack: `web-core` (wasm: encode/client/server,
includes the CSS tokenizer + style transformer), `web_elements`, and the SWC
ReactLynx transform plugins under `packages/react/transform`.
