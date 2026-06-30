# reactlynx-decoder — Implementation Plan

Authoritative implementation plan for a Rust crate (`reactlynx-decoder`) that
decodes the **native** LynxJS template binary (magic `0x00241922`, LepusNG/QuickJS)
for the **latest** ReactLynx/fiber feature subset only. Legacy paths
(radon / virtual node tree, old `ELEMENT_TEMPLATE`, TTML page/component descriptors,
Air mode, the web `SDRAWROF` container) are explicitly out of scope.

This plan is the synthesis of the five analysis docs in this directory:
- `01-container-format.md` — outer envelope, header, section route, primitives, lepus value, header-ext-info.
- `02-element-template.md` — `NEW_ELEMENT_TEMPLATE` body, element node sections.
- `03-css-styles.md` — `CSS`, `PARSED_STYLES`, `STYLE_OBJECT` payloads.
- `04-feature-switches.md` — compile-options / version gates that select old vs new.
- `05-pipeline-and-rust-idioms.md` — encode pipeline, Rust idioms to mirror, two-format distinction.

All C++ `path:line` cites are into `lynx` (engine) unless noted
`stack:` → `lynx-stack`.

> **One load-bearing caveat repeated from every analyst:** in this source tree the
> `Compact*` integer helpers are **fixed-width little-endian, NOT LEB128**
> (`output_stream.cc:80-94`, `binary_input_stream.cc:31-53`, proven by
> `binary_input_stream_unittest.cc:142-185`). `CompactU32`/`CompactS32` = 4 bytes LE,
> `CompactU64`/`CompactD64` = 8 bytes LE. Isolate this behind one `Reader` method so a
> future ULEB128 device build can be swapped in one place.

---

## 1. SCOPE

### 1.1 IN scope

**Container / envelope** (`01`):
- Magic `kQuickBinaryMagic = 0x00241922` (LepusNG). Detect `kLepusBinaryMagic = 0xdd737199` only to emit a clean `UnsupportedVm` error (do not decode VM bytecode).
- Header: `total_size`, magic, 4 version strings, `header_ext_info` (`0x494e464f`) → `CompileOptions`, header-mode `template_info` lepus value (≥ V_2_7), `trial_options` (decode-and-discard iff `enable_trial_options_`), `app_type`, snapshot bool.
- **Flexible body only**: `SECTION_ROUTE` table + **fiber** canonical section order. (Non-flexible flat body is out of scope — ReactLynx/fiber templates are always flexible, `enable_flexible_template_`, V_2_8.)
- Primitives: `U8`, `U32` LE, `CompactU32/S32` (4B LE), `CompactU64/D64` (8B LE), inline length-prefixed UTF-8 strings, `lepus::Value`.

**Sections** (`BinarySection` enum, `template_binary.h:49-69`):

| Section (value) | Why IN | Reference |
|---|---|---|
| `CONFIG=6` | page-config JSON (required) | `01 §5.4`, `04 §3` |
| `NEW_ELEMENT_TEMPLATE=17` | the fiber element tree — core | `02 §3` |
| `CSS=1` | CSS route + fragments (parsed values, css variables, css selector, rule-based when `enable_css_rule_`) | `03 §3-4` |
| `STYLE_OBJECT=18` | simple-styling objects/keyframes/fontfaces (newest style form) | `03 §8.2` |
| `PARSED_STYLES=13` | pre-parsed inline-style maps (fiber only) | `03 §8.1`, `02 §4` |
| `ROOT_LEPUS=11` | root lepus context bundle (header framing → value) | `01 §5`, `04 §3` |
| `LEPUS_CHUNK=15` | named lepus chunk route | `01 §5.4` |
| `JS=5` | JS source `(path, content)*` | `01 §5.4` |
| `JS_BYTECODE=14` | quickjs bytecode `(path, len, bytes)*` | `01 §5.4` |
| `CUSTOM_SECTIONS=16` | custom-section route (string / js-bytecode / css payloads) | `01 §5.4`, `04 §5` |
| `STRING=0` | route slot only; handler is a no-op stub — framing acknowledged, payload skipped | `01 §1.3` |

**Lepus value** full tag set for the LepusNG build (the `!ENABLE_JUST_LEPUSNG` tags
6/13/14/15 are decoded defensively but should not appear).

### 1.2 OUT of scope (reject or skip)

| Item | Disposition | Reference |
|---|---|---|
| `kLepusBinaryMagic = 0xdd737199` VM bytecode path | `DecodeError::UnsupportedVm` | `04 §6` |
| Non-flexible (flat) body framing | not implemented (assume flexible) | `01 §5.1` |
| `ELEMENT_TEMPLATE=12` (legacy) | `DecodeError::LegacySection` (engine hard-errors) | `01 §5.3`, `02 §2.3` |
| `COMPONENT=2`, `PAGE=3`, `APP=4`, `DYNAMIC_COMPONENT=7`, `THEMED=8`, `USING_DYNAMIC_COMPONENT_INFO=9`, `SECTION_ROUTE=10` (as a visited body section) | skip via route range (no-op stubs in engine) | `04 §3` |
| Radon node tree / virtual node tree (`PageSection`) | not implemented | `04 §6` |
| TTML page/component descriptors, dynamic-component moulds | not implemented | `04 §6` |
| Air mode (`AIR_ARCH`, `lynx_air_mode_`) | not implemented | `04 §6` |
| Web `SDRAWROF` container (`0x41524453`/`0x464F5257`) | not this crate | `05 §1` |
| Non-fiber flexible section order | not implemented (assume `FIBER_ARCH`) | `04 §2` |

Out-of-scope sections that still appear in the route are **skipped by their
`[start,end)` range**, never hard-failed (except the two explicit reject cases
above), so an unknown/legacy section never aborts the decode.

---

## 2. CRATE / MODULE LAYOUT

```
reactlynx-decoder/
├── Cargo.toml                 # edition 2021; deps: thiserror. (no rkyv — native bundle is manual cursor)
├── src/
│   ├── lib.rs                 # pub re-exports, `decode_template(&[u8]) -> Result<TemplateBundle>`
│   ├── error.rs               # DecodeError (thiserror), type Result<T>
│   ├── reader.rs              # Reader<'a>: bounds-checked LE cursor over &'a [u8]
│   ├── version.rs             # Version (a.b.c.d parse + compare), FEATURE_* gate constants
│   ├── value.rs               # lepus::Value model + decode_value()
│   ├── container/
│   │   ├── mod.rs             # Decode orchestration: header → app_type → body
│   │   ├── header.rs          # DecodeHeader: total_size, magic, versions
│   │   ├── header_ext_info.rs # 0x494e464f block → CompileOptions
│   │   ├── compile_options.rs # CompileOptions struct + key_id field map + ArchOption/FeOption
│   │   └── section_route.rs   # SECTION_ROUTE table + fiber canonical order + dispatch
│   ├── sections/
│   │   ├── mod.rs
│   │   ├── config.rs          # CONFIG (page-config JSON string)
│   │   ├── element_template.rs# NEW_ELEMENT_TEMPLATE: router + element tree
│   │   ├── css.rs             # CSS section: route, fragment, rules, parse token, attributes, value
│   │   ├── style_object.rs    # STYLE_OBJECT (+ keyframes/fontfaces)
│   │   ├── parsed_styles.rs   # PARSED_STYLES (fiber)
│   │   ├── lepus.rs           # ROOT_LEPUS + LEPUS_CHUNK route
│   │   ├── js.rs              # JS source + JS_BYTECODE
│   │   └── custom.rs          # CUSTOM_SECTIONS route
│   └── model/
│       ├── mod.rs             # TemplateBundle, Header, top-level decoded model
│       ├── element.rs         # ElementNode, ElementTag, AttributeBinding, builtin attrs, events
│       ├── style.rs           # CssFragment, CssRule, CssParseToken, CssValue, StyleObject, property ids
│       └── enums.rs           # BinarySection, ElementSectionEnum, CSSRuleType, CSSValuePattern, etc.
└── tests/
    ├── primitives.rs          # reader unit tests (pin LE widths)
    ├── value.rs               # lepus value round-trips against hand-built bytes
    ├── fixtures/              # *.lynx artifacts + expected JSON
    └── golden.rs              # end-to-end decode of real fixtures
```

### Module → source reference map

| Module | C++ / Rust reference |
|---|---|
| `reader.rs` | `binary_input_stream.h:49-99`, `binary_input_stream.cc:31-53`, `binary_reader.cc:16-30`; idiom from `05 §6.4` |
| `version.rs` | `version.h:17-53`, `config.h:33-62`, `VersionStrToNumber` (`...reader_impl.cc:245-267`) |
| `value.rs` | `base_binary_reader.cc:209-326`, `base_value.h:65-91` |
| `container/header.rs` | `lynx_binary_base_template_reader_impl.cc:37-149` |
| `container/header_ext_info.rs` | `header_ext_info.h:11-50`, `...reader_impl.cc:269-312` |
| `container/compile_options.rs` | `compile_options.h:29-153` (`FOREACH_*_FIELD`) |
| `container/section_route.rs` | `...reader_impl.cc:339-417`, fiber order `lynx_binary_base_template_reader.cc:44-60` |
| `sections/config.rs` | `lynx_binary_base_template_reader.cc:62-72` |
| `sections/element_template.rs` | `csr_element_binary_writer.cc:97-354`, `element_binary_reader.cc:94-890` |
| `sections/css.rs` | `lynx_binary_base_css_reader.cc:85-760`, `template_binary_writer_impl.cc:203-411` |
| `sections/style_object.rs` | `lynx_binary_base_css_reader.cc:797-841`, `template_binary_writer_impl.cc:525-607` |
| `sections/parsed_styles.rs` | `...reader_impl.cc:488-499`, `template_binary_writer.cc:690` |
| `sections/lepus.rs` | `lynx_binary_reader.cc:227-246` (chunk route), `DecodeContext` |
| `sections/js.rs` | `lynx_binary_base_template_reader.cc:528-560` |
| `sections/custom.rs` | `lynx_binary_reader.cc:281-338`, `CustomSectionEncodingType` `template_binary.h:80` |
| `model/enums.rs` | `template_binary.h:49-113`, `element_property.h:21-79` |
| Rust idioms (enum repr, TryFrom, Result-everywhere) | `05 §6.2-6.4`, stack `css_property.rs:244,478-540`, `raw_style_info.rs:50,90` |

> Note on the **STRING section / string table**: there is *no* string-table
> indirection in this tree — every string is inline length-prefixed
> (`01 §1.3`). The "string table" module mentioned in the brief collapses to a
> no-op acknowledgement of the `STRING=0` route slot; it lives in
> `sections/mod.rs` as a `skip` and needs no dedicated decoder. Do not build a
> string-id resolver.

---

## 3. PUBLIC API SKETCH

Entry point (`lib.rs`):

```rust
/// Decode a native LynxJS template bundle (LepusNG, fiber, flexible).
/// Borrows from `buf`: returned `&str`/`&[u8]` slices point into `buf`.
pub fn decode_template(buf: &[u8]) -> Result<TemplateBundle<'_>>;
```

### error.rs

```rust
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of input: need {need} bytes at offset {at}, have {have}")]
    UnexpectedEof { at: usize, need: usize, have: usize },
    #[error("bad magic: {0:#010x}")]
    BadMagic(u32),
    #[error("lepus VM bundle (0xdd737199) is not supported; only LepusNG (0x00241922)")]
    UnsupportedVm,
    #[error("total_size {declared} != buffer length {actual}")]
    SizeMismatch { declared: u32, actual: usize },
    #[error("legacy section {0} is not supported")]
    LegacySection(u8),
    #[error("unknown lepus value tag {0}")]
    BadValueTag(u8),
    #[error("unknown element section tag {0}")]
    BadElementTag(u8),
    #[error("invalid utf-8 at offset {0}")]
    Utf8(usize),
    #[error("header ext info magic {0:#010x} != 0x494e464f")]
    BadHeaderExtMagic(u32),
    #[error("malformed value: {0}")]
    Malformed(&'static str),
}
pub type Result<T> = core::result::Result<T, DecodeError>;
```

No panics on malformed input — every fallible step returns `Result`; `Reader`
bounds-checks before each advance (mirrors C++ `CheckSize`).

### reader.rs

```rust
pub struct Reader<'a> { buf: &'a [u8], pos: usize }
impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self;
    pub fn pos(&self) -> usize;
    pub fn seek(&mut self, pos: usize) -> Result<()>;
    pub fn u8(&mut self) -> Result<u8>;
    pub fn u32_le(&mut self) -> Result<u32>;
    pub fn i32_le(&mut self) -> Result<i32>;
    pub fn u64_le(&mut self) -> Result<u64>;
    pub fn f64_le(&mut self) -> Result<f64>;
    pub fn compact_u32(&mut self) -> Result<u32> { self.u32_le() }      // fixed-width here
    pub fn compact_s32(&mut self) -> Result<i32> { self.i32_le() }
    pub fn compact_u64(&mut self) -> Result<u64> { self.u64_le() }
    pub fn compact_d64(&mut self) -> Result<f64> { self.f64_le() }
    pub fn lstr(&mut self) -> Result<&'a str>;                          // compact_u32 len + utf-8
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]>;               // bounds-checked sub-slice
    pub fn bool(&mut self) -> Result<bool> { Ok(self.u8()? != 0) }
}
```

A **sub-reader** helper (`fn slice(&self, start: usize, end: usize) -> Result<Reader<'a>>`)
lets each section be decoded from its own `[start,end)` route range independently
(matches the C++ seek-to-`start`, read-leading-`U8 type`, decode pattern).

### value.rs — `lepus::Value`

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    Nil,
    Undefined,
    Bool(bool),
    Double(f64),
    Int32(i32),
    Int64(i64),
    UInt32(u32),
    UInt64(u64),
    String(&'a str),
    Array(Vec<Value<'a>>),
    Table(Vec<(&'a str, Value<'a>)>),  // key order preserved as decoded (encoder sorts by key)
    ByteArray(&'a [u8]),
}
pub fn decode_value<'a>(r: &mut Reader<'a>) -> Result<Value<'a>>;
```

Tag → payload per `01 §7.1` / `03 §1.2`. Tags `Closure=6`, `NaN=13`, `CDate=14`,
`RegExp=15` are `!ENABLE_JUST_LEPUSNG`-only; decode them defensively (or return
`BadValueTag` if a strict-LepusNG mode is desired). `is_header` flag is irrelevant
in this tree (no string table) — strings are always inline; carry no flag.

### model/mod.rs — `TemplateBundle`

```rust
pub struct TemplateBundle<'a> {
    pub header: Header<'a>,
    pub compile_options: CompileOptions<'a>,
    pub template_info: Option<Value<'a>>,     // header-mode lepus value, >= V_2_7
    pub app_type: AppType,                     // Card | DynamicComponent
    pub page_config: Option<PageConfig<'a>>,   // CONFIG section (raw JSON + parsed view)
    pub element_templates: ElementTemplates<'a>,
    pub css: CssDescriptor<'a>,                // CSS fragments
    pub style_objects: Option<StyleObjects<'a>>,
    pub parsed_styles: Option<ParsedStyles<'a>>,
    pub root_lepus: Option<LepusContext<'a>>,
    pub lepus_chunks: Vec<LepusChunk<'a>>,
    pub js_sources: Vec<JsSource<'a>>,
    pub js_bytecode: Vec<JsBytecode<'a>>,
    pub custom_sections: Vec<CustomSection<'a>>,
}

pub struct Header<'a> {
    pub magic: Magic,                  // LepusNg
    pub lepus_version: &'a str,
    pub cli_version: &'a str,
    pub target_sdk: Version,           // = ios_version
    pub android_version: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version([u16; 4]);          // a.b.c.d; impl is_at_least(FEATURE_*)

pub enum AppType { Card, DynamicComponent }

pub struct PageConfig<'a> {
    pub raw_json: &'a str,             // verbatim CONFIG payload
    // optionally a parsed serde_json::Value behind a feature flag; raw kept always
}
```

### model/element.rs — `ElementTemplate` node

```rust
pub struct ElementTemplates<'a> {
    /// template_id (e.g. "_et_f47c3e863a57") -> root elements
    pub templates: Vec<(&'a str, Vec<ElementNode<'a>>)>,
}

pub struct ElementNode<'a> {
    pub tag: ElementTag<'a>,
    pub attributes_array: Vec<AttributeBinding<'a>>,  // ELEMENT_ATTRIBUTE_ARRAY (14)
    pub slot_index: Option<u32>,                      // ELEMENT_SLOT_INDEX (15)
    pub builtin_attributes: Vec<(BuiltinAttr, Value<'a>)>, // base 1000
    pub id_selector: Option<&'a str>,                 // ELEMENT_ID_SELECTOR (4)
    pub styles: Vec<(u32 /*CSSPropertyID*/, Value<'a>)>, // ELEMENT_STYLES (7)
    pub class: Vec<&'a str>,                           // ELEMENT_CLASS (6)
    pub events: Vec<EventBinding<'a>>,                 // ELEMENT_EVENTS (9)
    pub attributes: Vec<(&'a str, Value<'a>)>,         // ELEMENT_ATTRIBUTES (8)
    pub dataset: Vec<(&'a str, Value<'a>)>,            // ELEMENT_DATA_SET (10)
    pub parsed_style_key: Option<&'a str>,             // ELEMENT_PARSED_STYLES_KEY (12)
    pub parsed_styles: Option<ParsedStyleBlock<'a>>,   // ELEMENT_PARSED_STYLES (11)
    pub children: Vec<ElementNode<'a>>,                // ELEMENT_CHILDREN (5), always last
}

pub enum ElementTag<'a> {
    Builtin(ElementBuiltInTag),   // U8 enum, ELEMENT_TAG_ENUM (1)
    Custom(&'a str),              // ELEMENT_TAG_STR (2)
}

pub enum AttributeBinding<'a> {
    Static { key: &'a str, value: Value<'a> },   // STATIC=0
    Dynamic { key: &'a str, attr_slot_index: u32 },// DYNAMIC=1 (slot)
    Spread { attr_slot_index: u32 },              // SPREAD=2
}

pub struct EventBinding<'a> {
    pub kind: EventType,    // U8: bind/catch/capture-bind/capture-catch/global-bind
    pub name: &'a str,
    pub value: &'a str,
}
```

### model/style.rs — Style / CSS

```rust
pub struct CssDescriptor<'a> {
    pub fragments: Vec<CssFragment<'a>>,
}
pub struct CssFragment<'a> {
    pub id: u32,
    pub dependent_ids: Vec<i32>,
    pub body: CssBody<'a>,        // RuleBased | SelectorTokens | LegacySheets
}
pub enum CssBody<'a> {
    Rules(Vec<CssRule<'a>>),                  // enable_css_rule_ (newest)
    SelectorTokens { selectors: Vec<CssSelectorTuple<'a>>, /* + tokens/keyframes/fontfaces */ },
    // legacy CSSSheet form kept minimal / behind the same enum
}
pub enum CssRule<'a> {
    Style { position: u32, selectors: Vec<Value<'a>>, token: CssParseToken<'a> },
    Media { condition: Value<'a>, children: Vec<CssRule<'a>> },
    Supports { condition: Value<'a>, children: Vec<CssRule<'a>> },
    Keyframes { name: &'a str, token: CssKeyframesToken<'a> },
    FontFace(Value<'a>),
    Layer { /* segments, position, children */ },
    Skipped { rule_type: u8 },   // unknown rule, skipped by payload_size
}
pub struct CssParseToken<'a> {
    pub attributes: Vec<(u32 /*CSSPropertyID*/, CssValue<'a>)>,
    pub important: Vec<(u32, CssValue<'a>)>,        // >= V_3_9
    pub variables: Vec<(&'a str, &'a str)>,         // enable_css_variable_
    pub sheets: Vec<CssSheet<'a>>,                  // only when !enable_css_selector_
}
pub struct CssValue<'a> {
    pub pattern: CssValuePattern,    // when enable_css_parser_; else STRING
    pub value: Value<'a>,
    pub value_type: CssValueType,    // DEFAULT/VARIABLE when enable_css_variable_
    pub default_value: Option<&'a str>,
    pub default_value_map: Option<Value<'a>>, // >= V_2_14
}

pub struct StyleObjects<'a> {
    pub objects: Vec<Vec<(u32, CssValue<'a>)>>,     // STYLE_OBJECT entries
    pub keyframes: Vec<(&'a str, CssKeyframesToken<'a>)>,
    pub fontfaces: Vec<(&'a str, Vec<Vec<(&'a str, &'a str)>>)>,
}
```

`model/enums.rs` holds every on-wire enum with explicit `#[repr(u8)]`/`#[repr(u32)]`
discriminants and `TryFrom<u8>`/`TryFrom<u32>` → typed `DecodeError`, mirroring the
stack Rust idiom (`05 §6.2`): `BinarySection`, `ElementSectionEnum`,
`ElementBuiltInTag`, `BuiltinAttr`, `AttributeBindingType`, `EventType`,
`CSSRuleType`, `CSSValuePattern`, `CSSValueType`, `StyleObjectSectionType`,
`CustomSectionEncodingType`, `ArchOption`, `FeOption`.

---

## 4. DEPENDENCY-ORDERED TASK BREAKDOWN

Tasks are ordered so foundational primitives land first; each is independently
delegable to a coding agent. "Reference" gives the authoritative C++/Rust cite;
"Accept" gives the test gate. Tasks T1–T5 need no real fixture (hand-built bytes);
T6+ benefit from a real artifact (see §5) but each has a synthetic fallback.

---

**T1 — Crate skeleton + error + reader primitives**
- Creates: `Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/reader.rs`, `tests/primitives.rs`.
- Reference: `binary_input_stream.h:49-99`, `binary_input_stream.cc:31-53`, `binary_reader.cc:16-30`; idiom `05 §6.4`. Fixed-width LE confirmed by `binary_input_stream_unittest.cc:142-185`.
- Accept: unit tests assert `u32_le`/`compact_u32` consume exactly 4 bytes (LE), `compact_u64` exactly 8, `f64_le` bit-exact, `lstr` reads `[len:4 LE][utf-8]`, and every reader fails with `UnexpectedEof` (no panic) when short. Port the C++ unit-test vectors: `"test"` → `0x74736574` as U32; `"test str"` → its 8-byte LE u64.

**T2 — Version + feature gates**
- Creates: `src/version.rs`.
- Reference: `version.h:17-53`, `config.h:33-62`, `VersionStrToNumber` (`...reader_impl.cc:245-267`).
- Accept: `Version::parse("2.14.0.1")` → `[2,14,0,1]`; split on both `.` and `-`; `is_at_least(FEATURE_FLEXIBLE_TEMPLATE /*V_2_8*/)` etc. correct for the V_1_6 / V_2_0 / V_2_7 / V_2_8 / V_2_14 / V_3_9 boundaries used downstream.

**T3 — Lepus value decoder**
- Creates: `src/value.rs`, `tests/value.rs`.
- Reference: `base_binary_reader.cc:209-326`, `base_value.h:65-91` (`01 §7`, `03 §1.2`).
- Accept: hand-built byte round-trips for Nil/Undefined/Bool/Double/Int32/Int64/UInt32/String/Array/Table/ByteArray. Table key order preserved; nested Array/Table recursion. Unknown tag → `BadValueTag`. Assert Double reads 8-byte LE bit-cast; Int32 reads `compact_s32`.

**T4 — Enums (`model/enums.rs`)**
- Creates: `src/model/mod.rs`, `src/model/enums.rs`.
- Reference: `template_binary.h:49-113`, `element_property.h:21-79`, `compile_options.h:29-43`.
- Accept: every enum has explicit discriminant matching the documented integer (e.g. `BinarySection::NewElementTemplate = 17`, `ElementSectionEnum::Children = 5`, `CSSRuleType::kStyle = 2`, `ElementBuiltInTag::Slot = 18`, `BuiltinAttr::ComponentId = 1000`); `TryFrom` round-trips all valid values and rejects out-of-range with the typed error. Pure table-driven, no I/O.

**T5 — CompileOptions + header-ext-info block**
- Creates: `src/container/compile_options.rs`, `src/container/header_ext_info.rs`.
- Reference: `header_ext_info.h:11-50`, `...reader_impl.cc:269-312`, `compile_options.h:117-153` (`FOREACH_FIXED_LENGTH_FIELD`/`FOREACH_STRING_FIELD`).
- Accept: given a hand-built `0x494e464f` block `{size, magic, field_count}` + fields `{u8 type, u8 key_id, u16 size, payload}`, parser fills `CompileOptions` (key_id 25→`enable_fiber_arch_`, 27→`enable_flexible_template_`, 28→`arch_option_`, 1→`enable_css_parser_`, 6→`enable_css_variable_`, 29→`enable_css_selector_`, 33→`enable_simple_styling_`, 20→`enable_trial_options_`, string id 0→`target_sdk_version_`) and then seeks to `start+size`. Reject wrong magic. Fields with `payload_size != sizeof(T)` handled per `ReinterpretHeaderInfoValue`.

**T6 — Container header + decode orchestration shell**
- Creates: `src/container/header.rs`, `src/container/mod.rs`, `src/lib.rs` `decode_template` wiring (returns `TemplateBundle` with sections empty for now).
- Reference: `...reader_impl.cc:37-149`.
- Accept: parses `total_size` (== buffer len, else `SizeMismatch`), magic (`0x00241922` → ok; `0xdd737199` → `UnsupportedVm`; else `BadMagic`), 4 version strings (gated on `lepus_version > "0.1.0.0"`), header-ext-info (≥ V_1_6), `template_info` (≥ V_2_7), `trial_options` (iff `enable_trial_options_`, discarded), `app_type`, snapshot bool. `target_sdk = ios_version`. Round-trip against a hand-built minimal header + a real fixture header prefix.

**T7 — Section route + fiber dispatch**
- Creates: `src/container/section_route.rs`; wires `container/mod.rs` to walk sections.
- Reference: `...reader_impl.cc:351-417` (`DecodeFlexibleTemplateBody`/`DecodeSectionRoute`), fiber order `lynx_binary_base_template_reader.cc:44-60`.
- Accept: reads `U8 route_type`, `CompactU32 count`, per-entry `U8 section + CompactU32 start + CompactU32 end`; rebases every `[start,end)` by the post-route offset; builds a `section → (start,end)` map; visits in fiber canonical order `STRING, PARSED_STYLES, ELEMENT_TEMPLATE, CSS, JS, JS_BYTECODE, CONFIG, ROOT_LEPUS, LEPUS_CHUNK, CUSTOM_SECTIONS, NEW_ELEMENT_TEMPLATE`; seeks to each `start`, reads leading `U8 type`, dispatches. `ELEMENT_TEMPLATE=12` → `LegacySection`. Out-of-scope sections present in the route are skipped by range. Test with a synthetic 2-section route (CONFIG + NEW_ELEMENT_TEMPLATE) asserting both ranges resolve.

**T8 — CONFIG section**
- Creates: `src/sections/config.rs`, `model` `PageConfig`.
- Reference: `lynx_binary_base_template_reader.cc:62-72`.
- Accept: reads one inline string; exposes verbatim `raw_json`; optional `serde_json` parse behind a cargo feature. Golden: page-config JSON from a real fixture matches the bytes the encoder embedded.

**T9 — NEW_ELEMENT_TEMPLATE section (the core)**
- Creates: `src/sections/element_template.rs`, `src/model/element.rs`.
- Reference: `csr_element_binary_writer.cc:97-354`, `element_binary_reader.cc:94-222,455-890` (`02 §3-4`).
- Accept:
  - `OrderedStringKeyRouter`: `CompactU32 count`, `count × {String key, CompactU32 start}`; `descriptor_offset` = stream pos after router; template body at `descriptor_offset + start`.
  - Template body: `CompactU32 element_count`, then elements.
  - Element node: read `U32 children_section_offset` (raw 4B LE); loop `U8 ElementSectionEnum` tags until `ELEMENT_CHILDREN=5`; on unknown tag, seek to `field_end + children_section_offset` (forward-compat skip); decode children recursively.
  - Per-section payloads: tag (enum/str), `ATTRIBUTE_ARRAY` (STATIC/DYNAMIC/SPREAD), `SLOT_INDEX`, `BUILTIN_ATTRIBUTE` (base-1000 ids), `ID_SELECTOR`, `STYLES`, `CLASS`, `EVENTS` (`EventTypeEnum`), `ATTRIBUTES`, `DATA_SET`, `PARSED_STYLES_KEY`, `PARSED_STYLES`.
  - Golden: a real fixture's element tree round-trips against the expected JSON (tag names, attribute bindings, slot indices, children nesting).

**T10 — CSS section**
- Creates: `src/sections/css.rs`, `src/model/style.rs` (CSS half).
- Reference: `lynx_binary_base_css_reader.cc:85-760`, `template_binary_writer_impl.cc:203-411` (`03 §3-7`).
- Accept:
  - `CSSRoute`: `CompactU32 fragment_count`, `count × {CompactS32 id, CompactU32 start, CompactU32 end}`; `css_section_range_.start` = post-route offset.
  - Fragment: `id`, `dependent_count × CompactS32`, then **one of three bodies** keyed on flags: `enable_css_rule_` → `DecodeCSSRules`; else selector-tuple form (`enable_css_selector_`) + packed `css_size|keyframes<<16` maps + trailing font-face typed blocks.
  - `CSSRules`: `CompactU32 rules_count`, `count × {U8 rule_type, U32 payload_size (fixed 4B), payload}`; **always honor `payload_size` skip** for unknown rule types. Decode kStyle/kMedia/kSupports/kKeyframes/kFontFace/kLayerBlock/kLayerStatement.
  - `CSSParseToken`: attributes; important (≥ V_3_9); style-variables (`enable_css_variable_`); sheets (only when `!enable_css_selector_`).
  - `CSSValue`: optional pattern byte (`enable_css_parser_`), raw lepus value, css-variable trailer (`enable_css_variable_`, multi-default ≥ V_2_14).
  - Golden: CSS fragment property ids + values from a real fixture match.

**T11 — STYLE_OBJECT + PARSED_STYLES sections**
- Creates: `src/sections/style_object.rs`, `src/sections/parsed_styles.rs`, `model/style.rs` (style-object half).
- Reference: `lynx_binary_base_css_reader.cc:797-841`, `template_binary_writer_impl.cc:525-607`; `parsed_styles` `...reader_impl.cc:488-499`, `template_binary_writer.cc:690` (`03 §8`).
- Accept:
  - `STYLE_OBJECT`: leading `CompactU32 section_count` (== 3); three sub-sections each a `StyleObjectRoute {count, (start,end)*}` then entries — objects = `CSSAttributes`, keyframes = `{String name, CSSKeyframesToken}`, fontfaces = `{String family, FontFaceTokenList}`. `CSSValue` read as (pattern present, no css-var trailer).
  - `PARSED_STYLES`: `StringKeyRouter` + per-entry `CSSAttributes` (parsed). Decode only when `arch_option_ == FIBER_ARCH` (else the engine errors — surface `Malformed`).
  - Golden: when a fixture has `enable_simple_styling_`, the style objects decode; otherwise the section is absent and `style_objects == None`.

**T12 — Lepus context + JS + custom sections**
- Creates: `src/sections/lepus.rs`, `src/sections/js.rs`, `src/sections/custom.rs`.
- Reference: ROOT_LEPUS `DecodeContext`; LEPUS_CHUNK route `lynx_binary_reader.cc:227-246`; JS source/bytecode `lynx_binary_base_template_reader.cc:528-560`; custom `lynx_binary_reader.cc:281-338` (`01 §5.4`, `04 §5`).
- Accept:
  - `JS`: `U32 count`, `count × {String path, String content}`.
  - `JS_BYTECODE`: `U32 engine` (== quickjs else `Malformed`), `U32 count`, `count × {String path, CompactU64 len, bytes}`.
  - `LEPUS_CHUNK`: `CompactU32 size`, `size × {String path, CompactU32 start, CompactU32 end}` → named chunk ranges.
  - `CUSTOM_SECTIONS`: `U32 size`, `size × {String key, Value header, U32 start, U32 end}`; per-section encoding from header `"encoding"`: 0=String, 1=JsBytecode (requires LepusNG), 2=Css.
  - Golden: counts and first path strings match the fixture; bytecode `engine` field validated.

**T13 — End-to-end bundle assembly + golden harness**
- Creates: `tests/golden.rs`, fills `decode_template` to populate the full `TemplateBundle`.
- Reference: `...reader_impl.cc:37-56` (`Decode`).
- Accept: `decode_template(real_fixture)` returns `Ok` with non-empty `element_templates`, a `page_config`, and a non-empty `css`; assert specific known values (a template id, a tag name, a CSS property id→value, the `target_sdk`). No panics across the corpus; truncated/corrupted inputs return `Err` not panic (fuzz-lite: truncate the fixture at every 64-byte boundary and assert no panic).

**T14 — Robustness / forward-compat pass (optional but recommended)**
- Creates: hardening in `reader.rs`, `section_route.rs`, `css.rs`, `element_template.rs`.
- Reference: forward-skip rules `element_binary_reader.cc:212-217`, `...css_reader.cc:242,266-269`.
- Accept: unknown element-section tag and unknown CSS rule type both skip correctly without error; oversized declared lengths fail with `UnexpectedEof`, never OOB; a `cargo fuzz` (or `arbitrary`-driven) target runs N iterations with zero panics.

---

## 5. TEST STRATEGY

### 5.1 Producing real fixtures (authoritative)

The only ground-truth encoder is the C++ `TemplateBinaryWriter` reached via
`@lynx-js/tasm` (NAPI), driven by the ReactLynx toolchain (`05 §2`). Two routes:

1. **Build a minimal ReactLynx app in `lynx-stack`** (preferred). Scaffold a tiny
   `rspeedy`/`@lynx-js/rspeedy` project with one page containing a representative
   element tree (nested `<view>`/`<text>`, a `class`, an inline `style`, an event
   handler, a CSS file with a class rule + a CSS variable + a keyframe). Build it;
   the `LynxEncodePlugin` (`@lynx-js/tasm`) emits the native bundle (the `.lynx`
   / `tasm` artifact). Capture that file as `tests/fixtures/basic.lynx` and record
   the `compilerOptions` used (so the expected feature flags are known). Vary
   compile options to get fixtures that exercise: `enable_css_rule_` on/off,
   `enable_css_selector_` on/off, `enable_simple_styling_` on/off — one fixture per
   relevant body form.
   - Concretely: locate a workspace example under `lynx-stack/examples/*` or
     `packages/.../template-webpack-plugin` test apps; run its build; grab the
     emitted native template. Confirm the first 4 bytes (after `total_size`) are
     `0x00241922` LE to ensure it is the native LepusNG bundle, not the web
     `SDRAWROF` one.

2. **Reuse existing encoder snapshots / test artifacts.** The engine repo
   (`lynx`) ships codec unit tests; the SWC element-template
   crate ships `tests/__combined_snapshots__/*.snap` (`02 §5.3`) which give the
   **input JSON** shape (useful to predict the decoded model, not the bytes).
   Search both repos for committed `*.lynx`/`*.tasm`/binary template test
   artifacts to use directly as golden inputs.

> **Do not** use `lynx-stack` web-core `.lynx.bundle` (`SDRAWROF`) fixtures — they
> are the web format, a different container (`05 §1`). Verify the magic before
> adopting any fixture.

### 5.2 Synthetic fixtures (no toolchain dependency)

For T1–T7 and unit coverage, hand-assemble byte vectors in tests:
- A `build_header(...)` helper that writes `total_size`, magic, version strings,
  a `0x494e464f` block with chosen flags, `app_type`, snapshot — so header/route
  tests run without the C++ encoder.
- A `build_value(...)` helper mirroring the lepus value layout for `value.rs` tests.
- A minimal end-to-end synthetic bundle (header + 1-entry route + a tiny
  NEW_ELEMENT_TEMPLATE of one `<view>` with a static `class`) proves the full
  pipeline before a real fixture is available.

These double as a **decoder spec lock**: the byte layouts encoded in the helpers
are the documented layouts, so a regression in the reader breaks them immediately.

### 5.3 Golden checks to assert

Per fixture, assert:
- **Envelope**: `total_size == buf.len()`; `magic == LepusNg`; `target_sdk` equals
  the SDK the app was built with; `compile_options` flags match the recorded
  `compilerOptions` (esp. `enable_fiber_arch_`, `enable_flexible_template_`,
  `enable_css_parser_`, `enable_css_selector_`/`enable_css_rule_`,
  `enable_simple_styling_`).
- **Element template**: the set of `template_id`s matches the `elementTemplates`
  the transform emitted; for one chosen template, the root tag, child count, a
  static attribute (`class`/`id`), and a slot index match the source JSX.
- **CSS**: a known selector's declarations decode to the expected
  `CSSPropertyID → value` pairs (use the `STYLE_PROPERTY_MAP` id table from stack
  `css_property.rs` to translate ids to names: `width`=27, `color`=22, `display`=24,
  etc., `03 §9.2`); a CSS variable and a keyframe present when the source had them.
- **Config**: `page_config.raw_json` parses and contains expected keys
  (`enableCSSSelector`, etc.).
- **Round-trip safety**: every section's `[start,end)` route range is fully
  consumed (cursor ends at `end`) — a strong structural check that catches
  off-by-one width bugs (the #1 risk given the fixed-LE-vs-LEB128 ambiguity).
- **No panic**: truncation/corruption fuzz over the real fixture yields `Err`.

### 5.4 The fixed-width-vs-LEB128 verification gate

Because every analyst flagged the `Compact*` ambiguity, make the **first** golden
check after obtaining a real fixture a width probe: decode the header with
fixed-width LE; if `target_sdk` / `app_type` come out as garbage or the section
route ranges don't tile `[route_end, total_size)`, switch the single
`Reader::compact_*` implementation to ULEB128 and re-run. Lock the decision with a
comment + a test asserting the observed width on the real artifact. This keeps the
varint choice in exactly one place (T1's `Reader`), as recommended in `05 §6.4`.

---

## 6. Sequencing summary

```
T1 reader ─┬─ T3 value ─┐
           ├─ T2 version ┤
           └─ T4 enums ──┼─ T5 compile_options/ext_info ─ T6 header ─ T7 section_route ─┬─ T8 config
                         │                                                              ├─ T9 element_template (core)
                         │                                                              ├─ T10 css
                         │                                                              ├─ T11 style_object/parsed_styles
                         │                                                              └─ T12 lepus/js/custom
                         └──────────────────────────────────────────────────────────────── T13 golden ─ T14 fuzz
```

T1–T5 are pure and parallelizable. T6–T7 are the spine. T8–T12 are independent
section decoders that can be delegated in parallel once T7 lands. T13/T14 close out.
