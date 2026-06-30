# LynxJS Encode Pipeline & Rust Decoder Idioms

This document maps (a) the end-to-end ReactLynx **encode** pipeline, (b) the
existing `web-core` **Rust** decoder idioms worth mirroring, and (c) the
distinction between the **two** template binary formats — the *native* engine
bundle vs. the *web* template — so we know exactly which one a "ReactLynx
compatible decoder for a native renderer" must target.

> TL;DR for an implementer: a decoder for a **native** renderer (Vello/Skia-class)
> must target the **native binary template**, magic `0x00241922` (LepusNG / quick)
> or `0xdd737199` (classic Lepus), produced by the C++ `TemplateBinaryWriter`.
> The **web** format (`SDRAWROF`, two u32 magics) is a *separate, simpler*
> container produced by `webEncoder.ts` and is **not** what a native engine reads.

---

## 1. The two formats at a glance

| | **Native binary template** | **Web template** |
|---|---|---|
| Producer | C++ `TemplateBinaryWriter::Encode()` (`binary_encoder/template_binary_writer.cc`), exposed to JS via `@lynx-js/tasm` NAPI | TS `encode()` in `web-core/ts/encode/webEncoder.ts` |
| Toolchain plugin | `LynxEncodePlugin` (`template-webpack-plugin`) — `import … from '@lynx-js/tasm'` | `WebEncodePlugin` (`template-webpack-plugin`) — `import { TasmJSONInfo } from '@lynx-js/web-core/encode'` |
| Magic | `0x00241922` (LepusNG/Quick) **or** `0xdd737199` (Lepus classic), single `u32` LE | **two** u32 LE: `0x41524453` ('SDRA') then `0x464F5257` ('WROF') |
| Versioning | engine `target_sdk_version` string + `base::Version` gates (`V_1_0`…`V_4_1`) | single `u32` version, currently `1` |
| Section framing | `u8` section-count, then per-section `u8` type tag + body (offset map built in parallel) | `u32 label` + `u32 length` + body, repeated to EOF |
| Primitive ints | fixed-width LE (see §4) via `lepus::BinaryWriter` "Compact" methods | fixed-width `u32` LE (`DataView.setUint32(..., true)`) |
| Strings | `CompactU32 length` + raw UTF-8 bytes (`WriteStringDirectly`) | length-prefixed UTF-8 (binary maps) **or** UTF-16LE JSON blobs (config/customSections) |
| Style payload | CSS encoded inline by `css_encoder` (engine CSS tokens) | `rkyv`-archived `RawStyleInfo` blob, decoded by the `web-core` Rust/WASM `decode_style_info` |
| Consumer | C++ engine `LynxBinaryReader` / `template_binary_reader.cc` | browser worker / SSR `decode.ts` + WASM |

**A native renderer must decode the native format.** The web format throws away
the Lepus bytecode interpretation model and re-expresses styles as an `rkyv`
struct; it exists only so the web runtime can render without the C++ engine.

---

## 2. ReactLynx source → encoded template

ReactLynx (`.tsx`) is compiled by `@lynx-js/react-transform`
(`packages/react/transform`), a NAPI module wrapping a chain of **SWC plugins**
(Rust crates under `crates/`). Entry points: `transformReactLynxSync` /
`transformReactLynx` (`index.d.ts:684`).

SWC plugin chain (`crates/swc_plugin_*`, toggled by `TransformNodiffOptions`,
`index.d.ts:633`):

- `swc_plugin_directive_dce` — strips `"background-only"` / `"main-thread"` dead code (`directiveDCE`).
- `swc_plugin_define_dce` — `define`-based dead-code elimination, runs **before** directive transforms (`index.d.ts:384`).
- `swc_plugin_worklet` — extracts `main-thread`/worklet functions, hoisting external idents (`worklet`, `WorkletVisitorConfig`).
- `swc_plugin_snapshot` — the JSX transformer (`snapshot` / `JsxTransformerConfig`): turns JSX into **snapshot** descriptors (static element trees + dynamic slots). `target: 'LEPUS' | 'JS' | 'MIXED'`.
- `swc_plugin_element_template` — emits **element templates** (`elementTemplate` / `ElementTemplateConfig`); output surfaces as `TransformNodiffOutput.elementTemplates: ElementTemplateAsset[]` (`index.d.ts:666,676`), each `{ templateId, compiledTemplate, sourceFile }`.
- `swc_plugin_css_scope` — scopes CSS (`cssScope`); pairs with the `l-css-id` attribute scheme.
- `swc_plugin_shake` — tree-shakes (`shake` / `ShakeVisitorConfig`).
- `swc_plugin_compat`, `swc_plugin_list`, `swc_plugin_text`, `swc_plugin_inject`, `swc_plugin_dynamic_import` — compat shims, `<list>`/`<text>` specializations, runtime injection, lazy-bundle imports.

The transform produces **JS/Lepus code + element-template assets + CSS**, *not*
the final binary. The webpack/rspack `LynxTemplatePlugin` collects these into an
`encodeData` ("tasm.json" shape: `manifest`, `lepusCode {root, chunks}`,
`css {chunks}`, `customSections`, `elementTemplates`, `config`, `appType`,
`compilerOptions`) and then one of two encode plugins runs:

1. **`LynxEncodePlugin`** → calls `@lynx-js/tasm` (NAPI → C++ `TemplateBinaryWriter`) in a Tinypool worker → emits the **native** `0x00241922` bundle. This is the real device artifact.
2. **`WebEncodePlugin`** → calls `web-core`'s `encode()` → emits the **web** `SDRAWROF` template for browser rendering.

So: *ReactLynx never hand-rolls the native binary;* it produces tasm.json-style
intermediate data that the C++ `TemplateBinaryWriter` serializes.

---

## 3. Native binary template layout (`0x00241922`)

Source: `binary_encoder/template_binary_writer.cc`,
`template_binary.h`, `magic_number.cc`.

### 3.1 Header — `EncodeHeader()` (`template_binary_writer.cc:327`)
In write order:
1. `WriteU32(magic)` — `kQuickBinaryMagic = 0x00241922` for LepusNG context, else `kLepusBinaryMagic = 0xdd737199` (`magic_number.cc:11-12`). Both `u32` **LE**.
2. `WriteStringDirectly(lepus_version)` — *deprecated* string.
3. `WriteStringDirectly(cli_version)` — *deprecated* string.
4. `WriteStringDirectly(ios_version)` then `(android_version)` — both = `target_sdk_version` (the live engine version).
5. If `target_sdk_version >= FEATURE_HEADER_EXT_INFO_VERSION`: `EncodeHeaderInfo()` (compile-time flags; see `header_ext_info.h`).
6. If `>= FEATURE_TEMPLATE_INFO`: `EncodeValue(template_info)` (a serialized Lepus value).
7. If `enable_trial_options_`: `EncodeValue(trial_options)`.

### 3.2 Body — `Encode()` (`template_binary_writer.cc:104`)
After the header:
1. `WriteStringDirectly(app_type)` (e.g. `"card"`, `"DynamicComponent"`).
2. `WriteU8(false)` — snapshot flag (always `0` here).
3. `EncodeSectionCount(app_type)` (`:362`): `WriteU8(count)` where `count = 7`, minus `2` if `app_type == "DynamicComponent"`. (Non-flexible path.)
4. Section bodies, each wrapped by a `TemplateSectionRecorder` whose ctor writes **`WriteU8(section_type)`** (a `BinarySection` enum value) and records `[start,end)` offsets (`:82`). Body content follows. Order (fiber arch): CSS descriptor → simple styling objects (opt) → JS/Lepus bytecode → CONFIG → ROOT_LEPUS → LEPUS_CHUNK → ELEMENT_TEMPLATE → PARSED_STYLES → CUSTOM_SECTIONS.

There are two body strategies, gated by `compile_options_.enable_flexible_template_`
(default `false`, `compile_options.h:96`): `EncodeNonFlexibleTemplateBody`
(linear, section-count + tagged sections, `:158`) vs.
`EncodeFlexibleTemplateBody` (adds a relocatable `EncodeSectionRoute` offset
table so sections can be reordered/lazy-loaded). **For a first decoder, target
the non-flexible layout** and detect flexible mode via the compile flag in the
header ext-info.

### 3.3 Section enums — `template_binary.h:49`
`enum BinarySection` (the `u8` tag written per section). Values are the
**enumerator ordinals** (0-based, in declaration order):

```
0 STRING        1 CSS           2 COMPONENT      3 PAGE
4 APP           5 JS            6 CONFIG         7 DYNAMIC_COMPONENT
8 THEMED        9 USING_DYNAMIC_COMPONENT_INFO  10 SECTION_ROUTE
11 ROOT_LEPUS  12 ELEMENT_TEMPLATE  13 PARSED_STYLES  14 JS_BYTECODE
15 LEPUS_CHUNK 16 CUSTOM_SECTIONS  17 NEW_ELEMENT_TEMPLATE  18 STYLE_OBJECT
```

`enum BinaryOffsetType` (`:23`) is a *parallel* offset-map key set and has a
**different ordering** (note `TYPE_PAGE_ROUTE`, `TYPE_PAGE_DATA` etc. inserted
early) — do **not** assume `BinarySection == BinaryOffsetType`. Use
`BinarySection` for the wire tag.

Other on-wire enums:
- `enum class CustomSectionEncodingType { STRING=0, JS_BYTECODE=1, CSS=2 }` (`:80`).
- `enum class StyleObjectSectionType { STYLE_OBJECT=0, STYLE_OBJECT_KEYFRAMES=1, STYLE_OBJECT_FONTFACES=2, SECTION_COUNT=3 }` (`:82`).
- `enum class CSSRuleType : uint8_t` (`:89`) — `kUnknown=0, kCharset=1, kStyle=2, kImport=3, kMedia=4, kFontFace=5, …` (one byte each).
- `enum PageSection { MOULD=0, CONTEXT=1, VIRTUAL_NODE_TREE=2, RADON_NODE_TREE=3 }`.
- `enum DynamicComponentSection { DYNAMIC_MOULD=0, DYNAMIC_CONTEXT=1, DYNAMIC_CONFIG=2 }`.

### 3.4 Version gates
`version.h` defines `base::Version` constants `V_1_0`…`V_4_1`. The reader
branches on `target_sdk_version` (the header string) via
`Config::IsHigherOrEqual(...)`. `kLepusBinaryVersion = 1` (`magic_number.cc:14`)
is the *Lepus bytecode* container version, distinct from the SDK version.
`kTasmSsrSuffixMagic = 0xa8432251` marks an SSR suffix blob.

---

## 4. Primitive encoding (CRITICAL — not LEB128 in this tree)

Despite the method names `WriteCompactU32` / `ReadCompactU32` and a stale
"Returns the length of the leb128" comment (`binary_input_stream.h:92`), in this
open-source engine the "Compact" primitives are **fixed-width little-endian**,
*not* variable-length:

- `OutputStream::WriteCompactU32` → `WriteData(&value, sizeof(u32))` = **4 bytes LE** (`output_stream.cc:80`).
- `WriteCompactS32` → **4 bytes LE** (`:84`).
- `WriteCompactU64` → **8 bytes LE** (`:88`).
- `WriteCompactD64` → **8-byte IEEE-754 double LE** (`:92`).
- Reader side mirrors this: `InputStream::ReadCompactU32` does `ReadUx<uint32_t>` (memcpy 4 bytes) and returns length `1` as a stub count (`binary_input_stream.cc:31`). `ReadUx<T>` is a raw `memcpy(out, cursor, sizeof(T))` (`binary_input_stream.h:50`).

> Implementation note: upstream/closed builds may override these with true
> ULEB128 in a `ByteArrayOutputStream` subclass, but **the source in this repo
> serializes fixed-width LE**. A decoder built against *these* bytes must read
> 4/4/8/8 fixed-width LE. Keep the varint logic behind a trait so it can be
> swapped if a real device bundle turns out to be ULEB128 — verify against a
> real `.lynx`/template artifact before committing.

### Strings — `WriteStringDirectly` (`binary_writer.cc:46`)
`WriteCompactU32(length)` (4 bytes LE here) then `length` raw UTF-8 bytes; empty
strings write just the length. `EncodeUtf8Str` is a thin alias
(`context_binary_writer.cc:229`). Tables/maps: `WriteCompactU32(count)` then
`count` × `(key string, value …)` (`EncodeTable`, `:233`).

### Scalars
`WriteU8` = 1 byte; `WriteU32` = 4 bytes **LE** (`binary_writer.cc:22`). Booleans
are written via `WriteU8(0|1)`.

---

## 5. Web template layout (`SDRAWROF`) — reference only

Source: `web-core/ts/encode/webEncoder.ts`, `ts/server/decode.ts`,
`ts/constants.ts`, `ts/common/decodeUtils.ts`. All multi-byte ints are
`u32` little-endian via `DataView(..., true)`.

```
[0..4)   MagicHeader0 = 0x41524453  ('SDRA')   constants.ts:51
[4..8)   MagicHeader1 = 0x464F5257  ('WROF')   constants.ts:52
[8..12)  version (u32) = 1          decode rejects version > 1  (decode.ts:48)
repeat until EOF:
  u32 label   (TemplateSectionLabel)
  u32 length
  length bytes of content
```

`TemplateSectionLabel` (`constants.ts:54`): `Manifest=1, StyleInfo=2,
LepusCode=3, CustomSections=4, ElementTemplates=5, Configurations=6`. (Note
these integers are **unrelated** to native `BinarySection` ordinals.)

Section content encodings:
- **Configurations (6)** & **CustomSections (4)**: UTF-16LE JSON. Encoder writes `JSON.stringify` as a `Uint16Array` (`encodeAsJSON`, webEncoder.ts:15); decoder uses `TextDecoder('utf-16le')` (`decode.ts:81,103`). Config injects `cardType`, `isLazy` (`appType !== 'card'`), and stringified `pageConfig`.
- **LepusCode (3)** & **Manifest (1)**: a "binary string map": `u32 count`, then per entry `u32 keyLen, key(UTF-8), u32 valLen, val(UTF-8/bytes)` (`encodeStringMap` webEncoder.ts:24; `decodeBinaryMap` decodeUtils.ts:7).
- **StyleInfo (2)**: an `rkyv`-archived `RawStyleInfo` (see §6); decoded by the Rust/WASM `decode_style_info(content, entryName, enableCSSSelector, transformVW, transformVH, transformREM)` (`decode.ts:86`).
- **ElementTemplates (5)**: present only if non-empty; web `decode.ts` currently ignores it (`:108`). Element-template shape is the TS `ElementTemplateData` (`types/ElementTemplateData.ts`): `{ type, idSelector?, class?[], attributes?, builtinAttributes?, children?[], events?[{type,name,value}], dataset? }`.

Encoder emits sections in the order Configurations, LepusCode, CustomSections,
StyleInfo, Manifest (webEncoder.ts:124-159); decoder is order-independent and
throws on unknown labels.

---

## 6. Rust decoder idioms in `web-core` to mirror

`web-core/src` is the closest existing Rust precedent. Two distinct binary
strategies live here:

### 6.1 `rkyv` zero-copy archive (style info)
`RawStyleInfo` and friends (`template_sections/style_info/raw_style_info.rs`)
derive `rkyv::{Archive, Serialize, Deserialize}` and are read back via
`rkyv::from_bytes_unchecked::<RawStyleInfo>(&buf)` (`style_info_decoder.rs:954`).
This is *not* a hand-rolled cursor — `rkyv` gives validated/zero-copy struct
decode. Pattern to mirror **only for self-described struct blobs we control**;
the native engine bundle is **not** rkyv, so a native decoder needs a manual
reader (below). Cargo: `rkyv = "0.7"`, features `default/encode/client/server`
(`Cargo.toml`).

### 6.2 Enum encoding idioms
- `#[repr(i32)]` enums with explicit discriminants for wire stability, e.g. `RuleType { Declaration=1, FontFace=2, KeyFrames=3 }` (`raw_style_info.rs:50`) and `OneSimpleSelectorType { ClassSelector=1 … UnknownText=9 }` (`:90`). **Mirror this:** give every on-wire enum an explicit `#[repr]` + discriminant so the native `BinarySection`/`CSSRuleType` integers map 1:1.
- `#[repr(u32)] enum CSSPropertyEnum` plus `impl From<&str>/From<String>/From<usize>/From<CSSPropertyEnum> for CSSProperty` and `From<CSSProperty> for usize` (`css_property.rs:244,478-540`) — bidirectional, fallible-by-convention conversions between the wire integer and a typed enum. **Mirror this** for section/type tags via `TryFrom<u8>` returning a typed error rather than panicking.

### 6.3 Error handling
`web-core` returns `Result<_, wasm_bindgen::JsError>` (`style_info_decoder.rs:45`)
because it is WASM-facing. For a standalone native decoder, replace this with a
crate-local `enum DecodeError { UnexpectedEof, BadMagic(u32), UnknownSection(u8),
UnsupportedVersion(...), Utf8(...) }` and a `type Result<T> = core::result::Result<T, DecodeError>`.
Mirror the *shape* (every fallible step returns `Result`, no panics on
malformed input) — see how `decode.ts` guards every read with an explicit
length check before slicing (`decode.ts:23,42,58,64,70`); the Rust equivalent is
a `Reader` that bounds-checks before each advance.

### 6.4 Cursor / zero-copy slice pattern (recommended new code)
The native bundle needs a manual little-endian reader. Idiomatic Rust to write,
mirroring the C++ `InputStream` (offset + `begin`/`end`, `CheckSize` before each
read, `binary_input_stream.h:99`):

```rust
struct Reader<'a> { buf: &'a [u8], pos: usize }
impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8> { … }                 // 1 byte
    fn u32_le(&mut self) -> Result<u32> { … }            // 4 bytes LE
    fn compact_u32(&mut self) -> Result<u32> { self.u32_le() } // fixed-width here; swap if ULEB128
    fn compact_u64(&mut self) -> Result<u64> { … }       // 8 bytes LE
    fn compact_d64(&mut self) -> Result<f64> { f64::from_le_bytes(…) }
    fn lstr(&mut self) -> Result<&'a str> {              // CompactU32 len + UTF-8
        let n = self.compact_u32()? as usize;
        let bytes = self.take(n)?;                       // returns &'a [u8] (zero-copy)
        core::str::from_utf8(bytes).map_err(DecodeError::Utf8)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> { … } // bounds-checked sub-slice
}
```

Key mirror points: bound-check *before* advancing (like `CheckSize`); return
borrowed `&'a [u8]`/`&'a str` sub-slices for zero-copy (the `web-core` style
decoder borrows from the rkyv archive similarly); keep the varint method behind
a single function so the fixed-LE-vs-ULEB128 decision is one place.

---

## 7. Recommendation: which format is "latest" / which to target

- **For a native renderer (this project): target the NATIVE binary template**, magic `0x00241922` (LepusNG/Quick) — that is what the device engine consumes and what `LynxEncodePlugin` (`@lynx-js/tasm`) emits. Treat `0xdd737199` (classic Lepus) as a legacy variant to detect but you may not need to fully support it; LepusNG/Quick is the current default.
- The **web `SDRAWROF` format is a parallel, web-only container** — useful as a *clean reference* for section framing and for the element-template/style data model, but it is **not** the renderer input. Do not build the native decoder against it.
- Within the native format, prefer the **non-flexible** body first (`enable_flexible_template_ == false`, the default), and gate flexible-section-route support behind the compile flag for later.
- Implement "Compact" ints as **fixed-width LE** to match this source tree, but isolate that behind one `Reader::compact_*` method and validate against a real device artifact before locking it in (some builds may use true ULEB128).

### Authoritative source cites
- Magic numbers: `lynx/core/template_bundle/template_codec/magic_number.cc:11-14`.
- Native header/body: `…/binary_encoder/template_binary_writer.cc:104,158,327,362`; section recorder `:42,82`.
- Section enums: `…/template_codec/template_binary.h:23,49,71,78,80,82,89`.
- Versions: `…/template_codec/version.h:17-53`; `compile_options.h:95-96,141-143`.
- Varint = fixed LE: `lynx/core/runtime/lepus/output_stream.cc:80-94`; `binary_input_stream.cc:31-53`; `binary_input_stream.h:50,92`; strings `binary_writer.cc:46`.
- Web format: `web-core/ts/constants.ts:51-61`; `ts/encode/webEncoder.ts:76-162`; `ts/server/decode.ts:32-116`; `ts/common/decodeUtils.ts:7`; `ts/types/ElementTemplateData.ts`.
- Rust idioms: `web-core/src/template/template_sections/style_info/raw_style_info.rs:50,90`; `css_property.rs:244,478-540`; `style_info_decoder.rs:45,954`; `Cargo.toml`.
- Pipeline: `react/transform/index.d.ts:633,666,676,684`; SWC crates `react/transform/crates/swc_plugin_*`; `template-webpack-plugin/src/{LynxEncodePlugin,WebEncodePlugin}.ts`.
