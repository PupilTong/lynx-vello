# Lynx Template Binary — Feature Switches & Compile Options

Authoritative map of the compile options / feature switches that select **old vs new**
encoding behavior in the LynxJS template binary, so a Rust ReactLynx decoder can target the
**latest feature subset** and skip legacy paths. All citations are `path:line` into the
`lynx` C++ engine repo (`lynx`) unless noted.

> Scope note: `lynx-stack` web-core Rust (`packages/web-platform/web-core/src/template/`)
> uses its **own `rkyv` IR**, not the raw C++ template binary. The C++ codec under
> `core/template_bundle/template_codec` is the **single authoritative source** for the raw
> wire format. (`template/mod.rs:1-17`)

---

## 0. Magic numbers, versions, primitive encoding (decoder must-knows)

### Magic words (`magic_number.cc:11-14`)
| Constant | Value | Meaning |
|---|---|---|
| `kQuickBinaryMagic` | `0x00241922` | **LepusNG / QuickJS** context → `is_lepusng_binary_ = true`, `context_type_ = LepusNG`. **This is the latest/target.** |
| `kLepusBinaryMagic` | `0xdd737199` | Legacy VM ("lepus") context. Rejected outright when built `ENABLE_JUST_LEPUSNG`. |
| `kTasmSsrSuffixMagic` | `0xa8432251` | SSR suffix marker. |
| `kLepusBinaryVersion` | `1` | |
| `HEADER_EXT_INFO_MAGIC` | `0x494e464f` ("INFO") | Header-ext-info block magic (`header_ext_info.h:11`). |

Magic dispatch: `lynx_binary_base_template_reader.cc:22-42` (`DecodeMagicWord`).

### Versions (`version.h:17-53`, gates in `core/renderer/tasm/config.h`)
- Engine line currently `LYNX_VERSION = V_4_1`; tasm max supported `V_3_9`; min supported `V_1_0`.
- A decoder keys nearly every behavior off `target_sdk_version_` (a `"a.b"` string) via
  `Config::IsHigherOrEqual(target_sdk_version_, FEATURE_X)`.
- Relevant `FEATURE_*` gates (`config.h:33-62`):
  `FEATURE_HEADER_EXT_INFO_VERSION = V_1_6`, `FEATURE_CSS_VALUE_VERSION = V_2_0`,
  `FEATURE_CSS_STYLE_VARIABLES = V_2_0`, `FEATURE_NEW_RENDER_PAGE = V_2_1`,
  `FEATURE_CSS_FONT_FACE_EXTENSION = V_2_7`, `FEATURE_TEMPLATE_INFO = V_2_7`,
  `FEATURE_FLEXIBLE_TEMPLATE = V_2_8`, `FEATURE_FIBER_ARCH = V_2_8`,
  `FEATURE_OPT_LEPUS_BYTECODE = V_3_8`, `FEATURE_CSS_IMPORTANT = V_3_9`,
  `FEATURE_CUSTOM_PROPERTY_DECLARATION_KEYFRAME = V_3_9`.

### Integer encoding — **NOT varint / NOT LEB128** (critical)
Despite method names containing "Compact" and a stale `// leb128` comment, every integer is
**fixed-width little-endian**:
- `WriteCompactU32` → fixed **4-byte LE u32** (`output_stream.cc:80-82`)
- `WriteCompactS32` → fixed **4-byte LE i32** (`output_stream.cc:84-86`)
- `WriteCompactU64` → fixed **8-byte LE u64** (`output_stream.cc:88-90`)
- `WriteCompactD64` → fixed **8-byte LE f64** (`output_stream.cc:92-94`)
- `ReadUx<T>` = `memcpy(sizeof(T))` LE (`binary_input_stream.h:49-59`); the unit test
  `binary_input_stream_unittest.cc:142-185` proves `ReadCompactU32` consumes exactly 4 bytes
  and `ReadCompactU64` exactly 8. `ReadU8`/`ReadU32` are 1/4 fixed LE bytes
  (`binary_reader.h:53-58`).

So in Rust: `read_u8`, `read_u32_le`, `read_i32_le`, `read_u64_le`, `read_f64_le`. There is
**no continuation-bit decoding anywhere** in this codec build.

### Strings (`binary_reader.cc:16-30`)
`ReadStringDirectly` = `CompactU32` length (4 LE bytes) + that many raw UTF-8 bytes, **inline**.
There is **no shared string-table indexing** on the wire in this path — the `STRING` section
exists but `DeserializeStringSection` is a no-op here (`base_binary_reader.cc:187`). Decode
strings as inline length-prefixed UTF-8.

### Lepus `Value` wire tags (1-byte `WriteU8(type)`, then payload) — `DecodeValue` `base_binary_reader.cc:240-326`, enum `base/include/value/base_value.h:65-90`
`Value_Nil=0, Value_Double=1, Value_Bool=2, Value_String=3, Value_Table=4, Value_Array=5,`
`Value_Closure=6, Value_CFunction=7, Value_CPointer=8, Value_Int32=9, Value_Int64=10,`
`Value_UInt32=11, Value_UInt64=12, Value_NaN=13, Value_CDate=14, Value_RegExp=15,`
`Value_JSObject=16, Value_Undefined=17, Value_ByteArray=18, Value_RefCounted=19`.
Payloads: Int32→S32(4LE); UInt32→U32(4LE); Int64→U64(8LE); Double→D64(8LE); Bool→u8;
String→inline str; Table→U32 count then (key,value)* ; Array→U32 count then value*;
ByteArray→U64 len + raw bytes. `is_header=true` forces inline-string keys
(`base_binary_reader.cc:209-229`).

---

## 1. Top-level header & body decode flow (latest path)

`Decode()` (`lynx_binary_base_template_reader_impl.cc:37-56`):
1. `DECODE_U32 total_size` (must equal stream size, `:62-70`).
2. `DECODE_U32 magic_word` → `DecodeMagicWord`.
3. inline `lepus_version` string (deprecated), then if `> MIN_SUPPORTED_VERSION`: three inline
   strings `cli_version`, `ios_version`, `android_version` (`:84-119`).
4. **Header ext info** if `target_sdk >= V_1_6`: `DecodeHeaderInfo` (`:122-124`); else just set
   `target_sdk_version_`.
5. **template_info** lepus value if `target_sdk >= V_2_7` (`:130-133`).
6. **trial_options** lepus value iff `compile_options_.enable_trial_options_` (decode-and-discard, `:136-141`).
7. app_type string + `snapshot` bool, then `DecodeTemplateBody`.

**Header ext info block** (`lynx_binary_base_template_reader_impl.cc:269-312`,
`header_ext_info.h`): fixed struct `{u32 size, u32 magic=0x494e464f, u32 field_count}`, then
`field_count` fields `{u8 type, u8 key_id, u16 payload_size, payload[payload_size]}`. Fields are
keyed by `key_id` into `CompileOptions` via `FOREACH_FIXED_LENGTH_FIELD` /
`FOREACH_STRING_FIELD` (`compile_options.h:117-153`). Then seek to `start + size` (forward-compat
padding). This is how every `enable_*` compile flag arrives.

---

## 2. THE master switch: body layout — flat vs section-route ("flexible") vs fiber order

`DecodeTemplateBody` (`lynx_binary_base_template_reader_impl.cc:339-349`).

| Switch | Toggles | OLD behavior | NEW behavior | Introduced | Recommendation |
|---|---|---|---|---|---|
| `enable_flexible_template_` (id 27) | body framing | **Flat**: `DeserializeSection` = `u8 section_count` then `section_count × (u8 type + section payload)` (`:515-526`) | **Section route**: `DecodeSectionRoute` = `u8 route_type`, `U32 count`, `count×(u8 section,U32 start,U32 end)`; offsets are relative to post-route start; sections decoded in a **fixed order** by seeking each route entry (`:351-393`) | `FEATURE_FLEXIBLE_TEMPLATE = V_2_8` | **Use NEW.** ReactLynx/fiber templates are flexible. |
| `enable_fiber_arch_` (id 25) / `arch_option_=FIBER_ARCH` (id 28) | section iteration order in flexible body | non-fiber order `kSectionOrder` (`:355-369`) | **fiber order** `kFiberSectionOrder` (`lynx_binary_base_template_reader.cc:46-59`): STRING, PARSED_STYLES, ELEMENT_TEMPLATE, CSS, JS, JS_BYTECODE, CONFIG, ROOT_LEPUS, LEPUS_CHUNK, CUSTOM_SECTIONS, **NEW_ELEMENT_TEMPLATE** | `FEATURE_FIBER_ARCH = V_2_8` | **Use NEW (fiber).** This is ReactLynx. |

`ArchOption` (`compile_options.h:43`): `RADON_ARCH=0, FIBER_ARCH=1, AIR_ARCH=2`. Latest ReactLynx = **FIBER_ARCH**.

---

## 3. Section enums (the `u8` section type tag)

`BinarySection` (`template_binary.h:49-69`) — values are the enum ordinals:
`STRING=0, CSS=1, COMPONENT=2, PAGE=3, APP=4, JS=5, CONFIG=6, DYNAMIC_COMPONENT=7,`
`THEMED=8, USING_DYNAMIC_COMPONENT_INFO=9, SECTION_ROUTE=10, ROOT_LEPUS=11,`
`ELEMENT_TEMPLATE=12, PARSED_STYLES=13, JS_BYTECODE=14, LEPUS_CHUNK=15,`
`CUSTOM_SECTIONS=16, NEW_ELEMENT_TEMPLATE=17, STYLE_OBJECT=18`.

Dispatch: `DecodeSpecificSection` (`lynx_binary_base_template_reader_impl.cc:419-513`).

| Section (value) | Handler | LATEST? |
|---|---|---|
| `STRING=0` | `DeserializeStringSection` (no-op stub here) | keep (framing only) |
| `CSS=1` | `DecodeCSSDescriptor` (route + greedy fragments) | **LATEST** |
| `JS=5` | `DeserializeJSSourceSection` (`u32 count × (str path, str content)`) | LATEST |
| `CONFIG=6` | `DecodeConfigSection` → page-config JSON string | **LATEST (required)** |
| `THEMED=8` | `DecodeThemedSection` | optional |
| `USING_DYNAMIC_COMPONENT_INFO=9` | dynamic-component decls | optional |
| `ROOT_LEPUS=11` | `DecodeContext` → context bundle (lepus chunk) | **LATEST** |
| `ELEMENT_TEMPLATE=12` | **HARD ERROR** — "legacy element template is no longer supported" (`:481-487`) | **LEGACY — SKIP / reject** |
| `PARSED_STYLES=13` | `DecodeParsedStylesSection`; **errors unless `arch_option_==FIBER_ARCH`** (`:488-499`) | **LATEST (fiber only)** |
| `JS_BYTECODE=14` | `DeserializeJSBytecodeSection` (`u32 engine==quickjs`, then `u32 count × (str path, U64 len, bytes)`) | **LATEST (LepusNG)** |
| `LEPUS_CHUNK=15` | `DecodeLepusChunk` (route of named chunks → context bundles) | **LATEST** |
| `CUSTOM_SECTIONS=16` | `DecodeCustomSectionsSection` | LATEST (optional) |
| `NEW_ELEMENT_TEMPLATE=17` | `DecodeElementTemplateSection` (fiber element tree) | **LATEST (core)** |
| `STYLE_OBJECT=18` | `DecodeStyleObjects` (simple-styling objects/keyframes/fontfaces) | **LATEST** (when `enable_simple_styling_`) |

> `PAGE=3`, `APP=4`, `COMPONENT=2`, `DYNAMIC_COMPONENT=7`, `SECTION_ROUTE=10` handlers in the
> base reader are **stubs that `return true`** (`lynx_binary_base_template_reader.cc:74-120`) —
> the radon/virtual-node trees they used to carry are gone from this path.

---

## 4. Full switch table (compile options → old vs new)

All flags live in `CompileOptions` (`compile_options.h:60-115`); ids are the header-ext-info
`key_id` from `FOREACH_FIXED_LENGTH_FIELD`/`FOREACH_STRING_FIELD` (`compile_options.h:117-153`).

| Flag (id) | Toggles | OLD | NEW | Introduced | Recommendation |
|---|---|---|---|---|---|
| magic `kQuickBinaryMagic` vs `kLepusBinaryMagic` | runtime/bytecode | VM lepus bytecode | **LepusNG/QuickJS** | — | **NEW (LepusNG).** Assume `is_lepusng_binary_`. |
| `enable_flexible_template_` (27) | body framing | flat section list | **section-route ("flexible")** | V_2_8 | **NEW** |
| `enable_fiber_arch_` (25) + `arch_option_` (28) | arch / section order / element model | radon node tree | **fiber/element architecture** | V_2_8 | **NEW (FIBER_ARCH)** |
| `enable_css_parser_` (1) | CSS values | raw/string CSS | **parsed CSS values** (`EnableCssParser` needs `target_sdk>=V_2_0` AND flag) (`lynx_binary_base_css_reader.cc:64-68`) | V_2_0 | **NEW** |
| `enable_css_variable_` (6) | CSS vars | none | **CSS variables** (`EnableCssVariable` needs `>=V_2_0` AND flag) (`:57-61`) | V_2_0 | **NEW** |
| (derived) CSS-var multi-default | css var defaults | single default | **multi default** (`>=V_2_14`) (`:71-76`) | V_2_14 | NEW if `>=2.14` |
| `enable_css_selector_` (29) | selector model | legacy class match | **standard CSS selector** (`SetEnableStandardCSSSelector`) (`lynx_binary_config_decoder.cc:186`) | — | NEW |
| `enable_css_invalidation_` (31) | style invalidation | off | on | — | NEW |
| `enable_css_class_merge_` (13) | fragment class merge | off | on (`lynx_binary_reader.cc:195`) | — | follow flag |
| `enable_simple_styling_` (33) | STYLE_OBJECT section | absent | **emits/decodes STYLE_OBJECT** | — | **NEW (support STYLE_OBJECT)** |
| `enable_css_rule_` (derived from page config) | CSS fragment body | token-based (`DecodeCSSFragment` parses selectors/tokens) | **rule-based** `DecodeCSSRules` (`lynx_binary_base_css_reader.cc:120-123`) | — | support both; prefer rule when set |
| `enable_keyframe_custom_property_declaration_` (—) / `FEATURE_CUSTOM_PROPERTY_DECLARATION_KEYFRAME` | keyframes | plain | custom-prop declarations (`:556-558`) | V_3_9 | NEW if `>=3.9` |
| CSS `!important` | importance flag | absent | present (`FEATURE_CSS_IMPORTANT`, `:508-509`) | V_3_9 | NEW if `>=3.9` |
| `enable_css_font_face_extension_` (derived) | @font-face | legacy | extended (`FEATURE_CSS_FONT_FACE_EXTENSION`) | V_2_7 | NEW if `>=2.7` |
| `enable_lepus_ng_` (4) / `context_type_` (105) | script context | VM | LepusNG | — | NEW |
| `enable_async_lepus_chunk_decode_` (32) | chunk decode timing | sync | async-capable | — | cosmetic (decode same bytes) |
| `encode_quickjs_bytecode_` / `enable_opt_lepus_bytecode_` | JS_BYTECODE payload | source only | quickjs bytecode (`FEATURE_OPT_LEPUS_BYTECODE`) | V_3_8 | support bytecode section |
| `enable_trial_options_` (20) | header | absent | extra lepus value after template_info (decode-and-discard) | — | must skip if set |
| `enable_keep_page_data` (10) | page data persistence | off | on (needs `>=V_2_1`) (`lynx_binary_config_decoder.cc:130-133`) | V_2_1 | config-only |
| `lynx_air_mode_` (24) / `enable_lynx_air_` (16) / AIR_ARCH | Air (lite) runtime | — | Air mode | — | **SKIP — not ReactLynx** |
| `enable_component_config_` (23) | component config blocks | off | on | V_2_6 | optional |
| `target_sdk_version_` (string id 0) | global version gate | — | — | — | read first; drives all `IsHigherOrEqual` |
| `template_debug_url_` (string id 12) | debug | — | — | — | ignore |

`FeOption` tri-state (`compile_options.h:29-33`): `FE_OPTION_UNDEFINED=1, FE_OPTION_ENABLE=2,
FE_OPTION_DISABLE=3` (used by `enable_event_refactor_`, `force_calc_new_style_`,
`enable_lazy_css_decode_`, `enable_async_css_decode_`). For `enable_event_refactor_`, UNDEFINED
**counts as enabled** (`lynx_binary_config_decoder.cc:143-145`).

---

## 5. NEW_ELEMENT_TEMPLATE (fiber element tree) — the core latest section

Decoded by `ElementBinaryReader` (`element_binary_reader.h`). Each element is a sequence of
sub-sections tagged by `ElementSectionEnum` (`element_property.h:62-79`):
`ELEMENT_CONSTRUCTION_INFO=0, ELEMENT_TAG_ENUM=1, ELEMENT_TAG_STR=2,`
`ELEMENT_BUILTIN_ATTRIBUTE=3, ELEMENT_ID_SELECTOR=4, ELEMENT_CHILDREN=5, ELEMENT_CLASS=6,`
`ELEMENT_STYLES=7, ELEMENT_ATTRIBUTES=8, ELEMENT_EVENTS=9, ELEMENT_DATA_SET=10,`
`ELEMENT_PARSED_STYLES=11, ELEMENT_PARSED_STYLES_KEY=12, ELEMENT_PIPER_EVENTS=13,`
`ELEMENT_ATTRIBUTE_ARRAY=14, ELEMENT_SLOT_INDEX=15`.

Supporting enums (`element_property.h:18-60`):
- `AttributeBindingType`: `STATIC=0, DYNAMIC=1, SPREAD=2`.
- `ElementBuiltInTagEnum`: `ELEMENT_VIEW=0, TEXT=1, RAW_TEXT=2, IMAGE=3, SCROLL_VIEW=4, LIST=5,`
  `COMPONENT=6, PAGE=7, NONE=8, WRAPPER=9, OTHER=10, X_TEXT=11, X_SCROLL_VIEW=12, EMPTY=13,`
  `INLINE_TEXT=14, X_INLINE_TEXT=15, X_NESTED_SCROLL_VIEW=16, INLINE_IMAGE=17, SLOT=18`.
- `ElementBuiltInAttributeEnum` (base `1000`): `COMPONENT_ID=1000, COMPONENT_NAME=1001,`
  `COMPONENT_PATH=1002, CSS_ID=1003, NODE_INDEX=1004, DIRTY_ID=1005, CONFIG=1006,`
  `IS_TEMPLATE_PART=1007`. Builtin attribute IDs ≥1000; CSS property IDs <1000 share the map.

Multi-template routing: `NEW_ELEMENT_TEMPLATE` begins with an **ordered** string-key router
(`OrderedStringKeyRouter`: `descriptor_offset`, linked map name→offset); `PARSED_STYLES` uses an
unordered `StringKeyRouter`. (`template_binary.h:167-183`, `element_binary_reader.h:111-123`).

`StyleObjectSectionType` (`template_binary.h:82-87`): `STYLE_OBJECT=0, STYLE_OBJECT_KEYFRAMES=1,`
`STYLE_OBJECT_FONTFACES=2, SECTION_COUNT=3` — `DecodeStyleObjects` reads a leading `U32
section_count` and early-returns per sub-section (`lynx_binary_reader.cc:112-177`).

`CustomSectionEncodingType` (`template_binary.h:80`): `STRING=0, JS_BYTECODE=1, CSS=2`
(JS_BYTECODE requires `is_lepusng_binary_`, `lynx_binary_reader.cc:330-338`).

---

## 6. LATEST feature subset (must support) vs LEGACY (skip)

**Must support (ReactLynx / fiber, target_sdk ≥ ~3.x, LepusNG magic):**
- Magic `0x00241922` (LepusNG); header-ext-info block (`0x494e464f`); fixed-width LE ints; inline length-prefixed strings; lepus `Value` tags above.
- Flexible body framing (`enable_flexible_template_`) with **fiber section order**.
- Sections: `STRING(0)` framing, `CONFIG(6)` page-config JSON, `CSS(1)` (route+fragments, parsed values, css variables, css selector, css_rule when set), `STYLE_OBJECT(18)` (simple styling), `PARSED_STYLES(13)` (fiber), `ROOT_LEPUS(11)` + `LEPUS_CHUNK(15)` context bundles, `JS(5)` + `JS_BYTECODE(14)` (quickjs), `CUSTOM_SECTIONS(16)`, and **`NEW_ELEMENT_TEMPLATE(17)`** (the fiber element tree with `ElementSectionEnum`).

**Legacy — safe to SKIP / reject:**
- `kLepusBinaryMagic` VM bytecode path.
- `ELEMENT_TEMPLATE(12)` — old element template: decoder **hard-errors**; never implement.
- Radon node tree / virtual node tree (`PageSection::VIRTUAL_NODE_TREE`, `RADON_NODE_TREE`,
  `template_binary.h:71-76`) — handlers are no-op stubs in this path; not emitted under fiber.
- TTML/Radon page & component descriptors (`DecodePageDescriptor`, `DecodeComponentDescriptor`,
  dynamic-component moulds) — all stubs returning `true`.
- Air mode (`AIR_ARCH`, `lynx_air_mode_`, `enable_air_raw_css_`) — separate lite runtime, not ReactLynx.
- `enable_trial_options_` payload — decode-and-discard only.

**Bottom line for the Rust decoder:** assume **LepusNG + FIBER_ARCH + flexible template**,
read fixed-width LE integers (no varint), decode sections via the section route in fiber order,
implement `NEW_ELEMENT_TEMPLATE` + `CSS`/`STYLE_OBJECT`/`PARSED_STYLES` + lepus context bundles,
and reject/ignore `ELEMENT_TEMPLATE`, radon/virtual trees, TTML page parsing, and Air mode.
