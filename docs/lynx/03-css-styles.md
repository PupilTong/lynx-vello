# Lynx Template Binary — CSS / Style Encoding Reference

Authoritative byte-layout spec for the CSS-related sections of the LynxJS
template binary, for a Rust decoder. Citations are `path:line` against the two
reference repos:

- **engine** = `/Users/akiwah/repos/lynx` (C++ codec; ground truth for the
  on-disk template binary).
- **stack** = `/Users/akiwah/repos/lynx-stack` (ReactLynx transforms + web-core
  Rust).

> **Critical orientation.** There are **two completely different CSS
> serializations** in these repos. Do not conflate them:
>
> 1. **The template binary CSS section** (this doc's main subject) — produced by
>    the C++ `TemplateBinaryWriter` and read by `LynxBinaryBaseCSSReader`. This
>    is what lives inside a `.lynx`/template bundle. Sections `CSS`,
>    `PARSED_STYLES`, `STYLE_OBJECT`.
> 2. **`web-core` `style_info`** (stack) — an *rkyv*-serialized `RawStyleInfo`
>    used by the web platform's WASM CSS pipeline. It is **NOT** the template
>    binary format; it is a parallel representation with its own struct layout
>    and its own (different) enum integer values. Covered in the last section as
>    "idioms to reuse", not as the wire format to decode.

---

## 1. Primitive encoding (the building blocks)

All template-binary primitives are defined by the engine's `InputStream` /
`BinaryReader`. **In this source snapshot the "Compact" integers are
fixed-width little-endian, not LEB128.**

`InputStream::ReadCompactU32/S32/U64` call `ReadUx<T>`, which is a raw
`memcpy(out, cursor, sizeof(T))` — engine `binary_input_stream.h:49-59`,
`binary_input_stream.cc:31-53`. The comment "Returns the length of the leb128"
(`binary_input_stream.h:92`) is vestigial; no LEB128 path exists in this tree.
The writer mirrors this: `OutputStream::WriteCompactU32` is a 4-byte raw write
(`output_stream.cc:80-94`).

| Primitive | Wire layout | Read fn | Write fn |
|---|---|---|---|
| `U8` | 1 byte | `DECODE_U8` / `ReadUx<uint8_t>` | `WriteU8` |
| `U32` (fixed) | 4 bytes LE | `DECODE_U32` | `WriteU32` |
| `CompactU32` | **4 bytes LE** | `ReadCompactU32` | `WriteCompactU32` |
| `CompactS32` | **4 bytes LE** | `ReadCompactS32` | `WriteCompactS32` |
| `CompactU64` | **8 bytes LE** | `ReadCompactU64` | `WriteCompactU64` |
| `CompactD64` (double) | **8 bytes IEEE-754 LE** | `DECODE_DOUBLE`/`ReadCompactD64` | `WriteCompactD64` |
| `Bool` | 1 byte | `DECODE_BOOL` | `WriteByte` |

> **Decoder note.** If you target a *production* bundle that uses true LEB128
> (some shipped Lynx builds do), this table is the place to swap in a varint
> reader. Everything above this layer is byte-for-byte identical regardless of
> the integer encoding, because all sizes/ids go through `CompactU32`.

### 1.1 Strings — "string directly" (length-prefixed UTF-8)

For the CSS section, strings are **inline length-prefixed UTF-8**, NOT a string
table. `BinaryReader::ReadStringDirectly` reads `CompactU32 length` then `length`
raw bytes (`binary_reader.cc:16-30`). Writer: `WriteStringDirectly` writes
`CompactU32 length` + bytes (`binary_writer.cc:42-52`).

The CSS reader's `DecodeUtf8Str` / `EncodeUtf8Str` resolve to these "directly"
functions in CSS context (`lynx_binary_base_css_reader.cc:762-787`;
`context_binary_writer.cc:225-231` → `WriteStringDirectly`). The global
template-level string table does NOT apply inside CSS fragments.

```
String := CompactU32 byte_len, u8[byte_len] utf8
```

### 1.2 Lepus values (`DecodeValue` / `DecodeRawLynxValue`)

A lepus value is `U8 type_tag` followed by a type-specific body. Tag enum
`lepus::ValueType` — engine `base/include/value/base_value.h:65-90`:

| Tag | Value | Body |
|---|---|---|
| `Value_Nil` | 0 | (none) |
| `Value_Double` | 1 | `CompactD64` (8-byte double) |
| `Value_Bool` | 2 | `Bool` (1 byte) |
| `Value_String` | 3 | `String` (§1.1) |
| `Value_Table` | 4 | `CompactU32 size`, then size × (`String key`, `Value`) |
| `Value_Array` | 5 | `CompactU32 size`, then size × `Value` |
| `Value_Closure` | 6 | (closure body; not used in CSS) |
| `Value_CFunction` | 7 | — |
| `Value_CPointer` | 8 | — |
| `Value_Int32` | 9 | `CompactS32` |
| `Value_Int64` | 10 | `CompactU64` |
| `Value_UInt32` | 11 | `CompactU32` |
| `Value_NaN` | 13 | `Bool` |
| `Value_CDate` | 14 | date struct |
| `Value_RegExp` | 15 | pattern `Value`, flags `Value` |
| `Value_Undefined` | 17 | (none) |
| `Value_ByteArray` | 18 | `CompactU64 len`, raw bytes |
| `Value_RefCounted` | 19 | — |

Read paths: `BaseBinaryReader::DecodeValue` (`base_binary_reader.cc:240-326`)
and `DecodeRawLynxValue` (`base_binary_reader.cc:328+`). Note the writer
`EncodeValue` collapses any number to `Value_Double` unless
`feature_control_variables_` is set (`context_binary_writer.cc:294-298`), so in
practice CSS numeric values usually arrive as tag `1` (Double).

---

## 2. Section identifiers

`enum BinarySection` (engine `template_binary.h:49-69`) — the CSS-relevant ones:

| Section | Value |
|---|---|
| `STRING` | 0 |
| `CSS` | 1 |
| `PARSED_STYLES` | 13 |
| `STYLE_OBJECT` | 18 |

(`PARSED_STYLES`=13 and `STYLE_OBJECT`=18 follow from the full enum order:
STRING, CSS, COMPONENT, PAGE, APP, JS, CONFIG, DYNAMIC_COMPONENT, THEMED,
USING_DYNAMIC_COMPONENT_INFO, SECTION_ROUTE, ROOT_LEPUS, ELEMENT_TEMPLATE,
PARSED_STYLES, JS_BYTECODE, LEPUS_CHUNK, CUSTOM_SECTIONS, NEW_ELEMENT_TEMPLATE,
STYLE_OBJECT.)

The parallel `BinaryOffsetType` enum (`template_binary.h:18-47`) has
`TYPE_PARSED_STYLES` and `TYPE_STYLE_OBJECT` used by the section recorder.

---

## 3. The CSS section (`BinarySection::CSS`)

Encoder entry: `TemplateBinaryWriter::EncodeCSSDescriptor`
(`template_binary_writer_impl.cc:203-231`). Decoder: `DecodeCSSRoute` +
`DecodeCSSFragment` (`lynx_binary_base_css_reader.cc:85-227`).

### 3.1 Layout

The section is `[CSSRoute][Fragment 0][Fragment 1]…`. The route is written last
then `Move`d to the front (`template_binary_writer_impl.cc:223-230`).

```
CSSRoute :=
  CompactU32 fragment_count
  fragment_count × {
    CompactS32 fragment_id
    CompactU32 range_start   // byte offset relative to start of fragment area
    CompactU32 range_end
  }
```

Decoder `DecodeCSSRoute` (`...css_reader.cc:85-104`): after reading the route it
sets `css_section_range_.start = stream.offset()` and `.end = start + max(end)`.
Ranges let a fragment be lazily decoded by seeking to
`css_section_range_.start + range_start`.

### 3.2 CSS fragment (`shared_css_fragment`)

`DecodeCSSFragment` (`...css_reader.cc:106-227`):

```
Fragment :=
  CompactU32 id
  CompactU32 dependent_count
  dependent_count × CompactS32 dependent_css_id

  // ── THREE MUTUALLY EXCLUSIVE BODY FORMS, selected by compile flags ──
  if compile_options.enable_css_rule_:
      CSSRules            (§4, NEWEST rule-based form)
  else:
      if compile_options.enable_css_selector_:
          CompactU32 selector_size
          selector_size × LynxCSSSelectorTuple   (§3.3)
      CompactU32 packed_size      // css_size = packed & 0xFFFF; keyframes = packed >> 16
      css_size × { String key, CSSParseToken }            (§3.4)
      keyframes_size × { String name, CSSKeyframesToken } (§6)
      // trailing typed blocks until descriptor_end:
      while CheckSize(5, descriptor_end):
          U8 type
          CompactU32 typed_size
          if type == CSS_BINARY_FONT_FACE_TYPE (0x01):
              font-face blocks   (§7)
```

The packed size field cleverly stores **two** counts in one `CompactU32`:
low 16 bits = number of CSS parse tokens, high 16 bits = number of keyframes
(`...css_reader.cc:163-165`; encoder
`template_binary_writer_impl.cc:269-272`: `size |= keyframes_count << 16`).

When `enable_css_selector_` is on, the legacy `css_size` map is empty (selectors
carry the tokens via tuples instead) — see comment `...css_reader.cc:160`.

### 3.3 `LynxCSSSelectorTuple` (selector-list form)

`EncodeLynxCSSSelectorTuple` (`template_binary_writer_impl.cc:338-349`) /
inline decode in `DecodeCSSFragment` (`...css_reader.cc:136-157`):

```
LynxCSSSelectorTuple :=
  CompactU32 flattened_size
  if flattened_size == 0: (skip — unsupported selector, decoder `continue`s)
  flattened_size × CSSSelector     // each = one lepus Value (§3.3.1)
  CSSParseToken                    (§3.4)
```

#### 3.3.1 `CSSSelector`

Each flattened selector node is serialized as a single lepus `Value`:
`DecodeCSSSelector` does `DECODE_VALUE(data); LynxCSSSelector::FromLepus(...)`
(`...css_reader.cc:78-83`). Encoder walks the selector chain writing one
`EncodeValue` per node until `IsLastInTagHistory() && IsLastInSelectorList()`
(`template_binary_writer_impl.cc:351-364`). The decoder reads exactly
`flattened_size` of them. The lepus encoding of a single selector node is opaque
here (delegated to `LynxCSSSelector::ToLepus/FromLepus`); treat it as a generic
lepus `Value` blob.

### 3.4 `CSSParseToken` (selector + declarations)

`DecodeCSSParseToken` (`...css_reader.cc:505-537`) /
`EncodeCSSParseToken` (`template_binary_writer_impl.cc:307-336`):

```
CSSParseToken :=
  CSSAttributes attributes                         (§5)   // normal declarations
  if target_sdk >= FEATURE_CSS_IMPORTANT (v3.9):
      CSSAttributes important_attributes           (§5)
  if enable_css_variable_:
      CSSStyleVariables style_variables                   (§3.5)
  if NOT enable_css_selector_:
      CompactU32 sheet_count
      sheet_count × CSSSheet                              (§3.6)
```

### 3.5 `CSSStyleVariables`

`DecodeCSSStyleVariables` (`...css_reader.cc:637-649`) — note keys/values use
`ReadStringDirectly` explicitly:

```
CSSStyleVariables :=
  CompactU32 size
  size × { String key, String value }
```

### 3.6 `CSSSheet` (legacy non-selector form)

`DecodeCSSSheet` (`...css_reader.cc:567-578`):

```
CSSSheet :=
  CompactU32 type     // ignored on read; recomputed by ConfirmType()
  String name
  String selector
```

---

## 4. Rule-based CSS body (`enable_css_rule_`, NEWEST)

When `compile_options.enable_css_rule_` is set, the fragment body is a flat,
length-prefixed, forward-compatible **rule list** instead of §3.2's
selector/token maps. Decoder `DecodeCSSRules`
(`...css_reader.cc:229-272`); encoder `EncodeCSSRules`
(`template_binary_writer_impl.cc:366-411`).

```
CSSRules :=
  CompactU32 rules_count
  rules_count × {
    U8  rule_type            // CSSRuleType (§4.1)
    U32 payload_size         // FIXED 4-byte length prefix (WriteU32, not compact)
    payload[payload_size]    // body per rule type; decoder Seeks to next rule
  }
```

The `U32 payload_size` is a **fixed 4-byte** length written via `WriteU32`
(`template_binary_writer_impl.cc:376`) and read via `DECODE_U32`
(`...css_reader.cc:241`). It lets a decoder skip unknown `rule_type`s by seeking
`Offset() + payload_size` (`...css_reader.cc:242,266-269`). **Always honor this
skip — it is the forward-compat mechanism.**

### 4.1 `CSSRuleType` enum (`uint8_t`)

`enum class CSSRuleType : uint8_t` — engine `template_binary.h:89-113`:

| Name | Value | Name | Value |
|---|---|---|---|
| `kUnknown` | 0 | `kLayerStatement` | 10 |
| `kCharset` | 1 | `kNestedDeclarations` | 11 |
| `kStyle` | 2 | `kFunctionDeclarations` | 12 |
| `kImport` | 3 | `kNamespace` | 13 |
| `kMedia` | 4 | `kContainer` | 14 |
| `kFontFace` | 5 | `kScope` | 15 |
| `kFontFeature` | 6 | `kSupports` | 16 |
| `kProperty` | 7 | `kFunction` | 17 |
| `kKeyframes` | 8 | `kMixin` | 18 |
| `kLayerBlock` | 9 | `kApplyMixin` | 19 |
| | | `kContents` | 20 |
| | | `kPositionTry` | 21 |
| | | `kCustomMedia` | 22 |

Decoder only handles: `kStyle`, `kMedia`, `kSupports`, `kKeyframes`,
`kFontFace`, `kLayerBlock`, `kLayerStatement` (`...css_reader.cc:244-265`);
all others are skipped via the payload length.

### 4.2 Rule payloads

**kStyle** (`DecodeStyleRuleData` `...css_reader.cc:274-294`):
```
CompactU32 position          // document-order index
CompactU32 flattened_size
flattened_size × CSSSelector (§3.3.1)
CSSParseToken                (§3.4)
```

**kMedia / kSupports** (`DecodeConditionRuleData` `...css_reader.cc:316-381`):
```
Value condition              // lepus Value: media query set OR supports condition
CompactU32 child_count
child_count × { U8 child_type, U32 payload_size, payload }   // nested rules
```

**kKeyframes** (`DecodeKeyframesRuleData` `...css_reader.cc:392-401`):
```
String name
CSSKeyframesToken            (§6)
```

**kFontFace** (`DecodeFontFaceRuleData` `...css_reader.cc:415-432`):
```
Value font_face_rule         // single lepus Value → css::FontFaceRule::FromLepus
```
(NB: in the rule-based form a font face is one lepus `Value`, unlike the legacy
typed-block form §7 which is string key/value pairs.)

**kLayerBlock / kLayerStatement** (`DecodeCSSLayerRule`
`...css_reader.cc:434-503`):
```
CompactU32 name_segment_count
name_segment_count × String segment      // e.g. "framework","theme"
CompactU32 layer_position                // parser doc-order index, NOT cascade priority
if kLayerBlock:
    CompactU32 child_count
    child_count × { U8 child_type, U32 payload_size, payload }
```

---

## 5. `CSSAttributes` — property id + value map (the core of style)

This is how `propertyID → CSSValue` is serialized. Used by parse tokens,
keyframe frames, and style objects. Decoder
`DecodeCSSAttributes(StyleMap&, RawStyleMap&, configs)`
(`...css_reader.cc:588-635`); encoder `EncodeCSSAttributes`
(`template_binary_writer_impl.cc:678-689`).

```
CSSAttributes :=
  CompactU32 size
  size × { CompactU32 property_id, CSSValue value }   (§5.2)
```

`property_id` is cast to `CSSPropertyID` (`...css_reader.cc:596`). The
**CSSPropertyID space** is the engine's master property enum; the well-known id↔
name table is mirrored in the stack Rust `STYLE_PROPERTY_MAP` (§9.2) — those
integer ids match the on-wire ids (e.g. `width`=27, `color`=22, `display`=24).

### 5.1 Decode mode branches (read-side behavior, same wire bytes)

`DecodeCSSAttributes` has three branches keyed by reader flags, but **they read
the identical bytes** — only post-processing differs (`...css_reader.cc:591-633`):
- `enable_css_parser_` → store parsed values directly into `StyleMap`.
- `enable_pre_process_attributes_` → pre-decode then `UnitHandler::ProcessCSSValue`.
- else → store into `RawStyleMap` (unprocessed).

### 5.2 `CSSValue`

Decoder `DecodeCSSValue` (`...css_reader.cc:714-760`); encoder
`EncodeCSSValue` (`context_binary_writer.cc:398-418`):

```
CSSValue :=
  if enable_css_parser_:
      CompactU32 pattern        // CSSValuePattern (§5.3); else implicit STRING(1)
  Value  raw_value              // lepus Value (§1.2) — the actual data
  if enable_css_variable_:
      CompactU32 value_type     // CSSValueType: DEFAULT=0, VARIABLE=1
      String     default_value
      if target_sdk >= LYNX_VERSION_2_14:
          Value  default_value_map   // lepus Value (Nil if absent)
```

When `enable_css_parser_` is OFF, no pattern byte is present and the pattern is
forced to `STRING` (`...css_reader.cc:724-729`). The `raw_value` is read via
`DecodeRawLynxValue` into `result->val_uint64` + type.

### 5.3 `CSSValuePattern` enum (`uint8_t`)

Engine `core/renderer/css/css_value.h:28-50`:

| Name | Val | Name | Val | Name | Val |
|---|---|---|---|---|---|
| `EMPTY` | 0 | `RPX` | 6 | `CALC` | 12 |
| `STRING` | 1 | `EM` | 7 | `ENV` | 13 |
| `NUMBER` | 2 | `REM` | 8 | `ARRAY` | 14 |
| `BOOLEAN` | 3 | `VH` | 9 | `MAP` | 15 |
| `ENUM` | 4 | `VW` | 10 | `PPX` | 16 |
| `PX` | 5 | `PERCENT` | 11 | `INTRINSIC` | 17 |
| | | | | `SP` | 18 |
| | | | | `FR` | 19 |
| | | | | `COUNT` | 20 |

`CSSValueType`: `DEFAULT=0`, `VARIABLE=1` (`css_value.h:52-55`).

---

## 6. `CSSKeyframesToken`

Decoder `DecodeCSSKeyframesToken` (`...css_reader.cc:549-565`) →
`DecodeCSSKeyframesMap` (`...css_reader.cc:663-690`). Encoder
`EncodeCSSKeyframesToken` (`template_binary_writer_impl.cc:620-633`) →
`EncodeCSSKeyframesMap` (`...impl.cc:702-723`).

```
CSSKeyframesToken :=
  CSSKeyframesMap frames
  if target_sdk >= FEATURE_CUSTOM_PROPERTY_DECLARATION_KEYFRAME (v3.9)
     AND enable_keyframe_custom_property_declaration_:
      CSSKeyframesCustomPropertyContent custom    (§6.1)

CSSKeyframesMap :=
  CompactU32 size
  size × {
    key:                                   // the "0%"/"from" keyframe selector
      if enable_css_parser_ (sdk>=v2.0):  CompactD64 key   // parsed to float
      else:                                String key_text  // e.g. "0%","from"
    CSSAttributes frame_styles             (§5)
  }
```

Key parsing: `CSSKeyframesToken::ParseKeyStr` maps `"from"`→0, `"to"`→100, `"N%"`
→N (encoder `...impl.cc:714`).

### 6.1 `CSSKeyframesCustomPropertyContent`

`DecodeCSSKeyframesCustomPropertyContent` (`...css_reader.cc:692-712`):
```
CompactU32 size
size × {
  key (CompactD64 if css_parser else String),
  CSSKeyframesCustomProperty:
      CompactU32 n
      n × { String key, CSSValue value }    // (§5.2)
}
```

---

## 7. Font-face (legacy typed-block form, in §3.2 body)

Trailing typed blocks of the legacy fragment body. `type` byte
`CSS_BINARY_FONT_FACE_TYPE = 0x01` (engine
`core/renderer/css/css_font_face_token.h:14`). Decode in `DecodeCSSFragment`
(`...css_reader.cc:194-223`); encode `template_binary_writer_impl.cc:286-304`.

```
FontFaceBlock :=
  U8 type (== 0x01)
  CompactU32 typed_size               // number of font-face entries
  typed_size × FontFaceEntry

FontFaceEntry :=
  if enable_css_font_face_extension_ (target_sdk >= FEATURE_CSS_FONT_FACE_EXTENSION = v2.7):
      CompactU32 token_count
      token_count × CSSFontFaceToken
  else:
      CSSFontFaceToken                // exactly one

CSSFontFaceToken :=                   // DecodeCSSFontFaceToken (...css_reader.cc:539-547)
  CompactU32 attr_count
  attr_count × { String key, String value }   // e.g. "font-family"→"Foo", "src"→"url(...)"
```

The first token's `font-family` attr becomes the map key
(`...css_reader.cc:214-217`).

---

## 8. `PARSED_STYLES` vs `STYLE_OBJECT` — which is latest

### 8.1 `PARSED_STYLES` (older inline-style optimization)

`BinarySection::PARSED_STYLES` (=13). This section stores **pre-parsed inline
style attribute maps** keyed for reuse. The per-entry payload is a `CSSAttributes`
map (§5) decoded with `enable_css_parser=true`. (Encoder path uses
`EncodeCSSValue(p.second, true, true)` at `template_binary_writer.cc:690`.)

### 8.2 `STYLE_OBJECT` (NEWER — "SimpleStyling")

`BinarySection::STYLE_OBJECT` (=18). Encoder
`EncodeSimpleStyleObjects` (`template_binary_writer_impl.cc:525-607`); decoder
`DecodeStyleObjectRoute` + `DecodeStyleObject`
(`...css_reader.cc:797-841`). This is the `style::StyleObjectDecoder` interface.

The section has a fixed set of sub-sections. `enum class StyleObjectSectionType`
(`template_binary.h:82-87`):

| Name | Value |
|---|---|
| `STYLE_OBJECT` | 0 |
| `STYLE_OBJECT_KEYFRAMES` | 1 |
| `STYLE_OBJECT_FONTFACES` | 2 |
| `SECTION_COUNT` | 3 |

```
StyleObjectSection :=
  CompactU32 section_count            // == SECTION_COUNT (3)
  // section 0: STYLE_OBJECT
  StyleObjectRoute route_0
  route_0.count × StyleObjectEntry    // each = CSSAttributes (§5)
  // section 1: STYLE_OBJECT_KEYFRAMES
  StyleObjectRoute route_1
  route_1.count × { String name, CSSKeyframesToken }   (§6)
  // section 2: STYLE_OBJECT_FONTFACES
  StyleObjectRoute route_2
  route_2.count × { String family, FontFaceTokenList }  (§7 list form)

StyleObjectRoute :=                    // DecodeStyleObjectRoute (...css_reader.cc:797-812)
  CompactU32 count
  count × { CompactU32 range_start, CompactU32 range_end }
```

A single style object's bytes (`DecodeStyleObject`, `...css_reader.cc:827-841`):
```
StyleObjectEntry :=
  CompactU32 size
  size × { CompactU32 property_id, CSSValue value }    // CSSValue read as (true,false,false):
                                                        //   pattern byte present, no css-variable trailer
```

Each style-object section is preceded by its own route written-then-`Move`d to
the front, exactly like the CSS section (`...impl.cc:539-556`).

### 8.3 Recommendation

**`STYLE_OBJECT` is the latest** (added 2025; `style_object_parser.cc` and the
reader's `#pragma region SimpleStyling` carry 2025 copyright;
`StyleObjectDecoder` is the new "simple styling" runtime path). **`PARSED_STYLES`
is the older mechanism.** For a new decoder, implement `STYLE_OBJECT` as the
primary path and treat `PARSED_STYLES` as legacy/compat. Likewise within the CSS
section, the **rule-based body (`enable_css_rule_`, §4) is the newest** form;
the selector-tuple form (§3.3, `enable_css_selector_`) supersedes the original
`CSSSheet` form (§3.6). A robust decoder must branch on these compile flags
(read from the template's CompileOptions/config section), since the same fragment
slot can be any of the three.

---

## 9. web-core Rust `style_info` — idioms to reuse (NOT the wire format)

stack `packages/web-platform/web-core/src/template/template_sections/style_info/`.
This module is an **rkyv** serialization of `RawStyleInfo`, used by the web WASM
CSS pipeline. It is independent of the template binary above and has its own enum
values. Reuse its *structure and string-generation logic*, not its bytes.

### 9.1 Structures (rkyv, `raw_style_info.rs`)

- `RawStyleInfo { css_id_to_style_sheet: FnvHashMap<i32, StyleSheet>, style_content_str_size_hint: usize }`.
- `StyleSheet { imports: Vec<i32>, rules: Vec<Rule> }`.
- `Rule { rule_type: RuleType, prelude: RulePrelude, declaration_block, nested_rules: Vec<Rule> }`.
- `RuleType` (`#[repr(i32)]`, `raw_style_info.rs:50-54`): **`Declaration=1,
  FontFace=2, KeyFrames=3`** — note these integer values differ from the engine's
  `CSSRuleType`.
- `OneSimpleSelectorType` (`#[repr(i32)]`, `raw_style_info.rs:90-100`):
  `ClassSelector=1, IdSelector=2, AttributeSelector=3, TypeSelector=4,
  Combinator=5, PseudoClassSelector=6, PseudoElementSelector=7,
  UniversalSelector=8, UnknownText=9`.
- `ParsedDeclaration { property_id: CSSProperty, value_token_list: Vec<ValueToken>,
  is_important: bool }` (`css_property.rs:579-585`).
- `ValueToken { token_type: u8, value: String }` (`css_property.rs:572-577`).

### 9.2 `CSSPropertyEnum` (`css_property.rs:244-464`)

`#[repr(u32)]`, `Unknown=0`, then `Top=1 … OffsetDistance=215`. `STYLE_PROPERTY_MAP`
(`css_property.rs:15-232`) is the parallel index→name table; index equals the
enum value. **These ids align with the template binary's `CSSPropertyID` integers**
for the well-known range, so this array is a ready-made `id → css-name` lookup for
the Rust decoder. Unknown ids carry an explicit `unknown_name: Option<String>`.

### 9.3 Decoder idioms worth porting (`style_info_decoder.rs`)

- Topological flatten of `@import` graph via Kahn's algorithm
  (`flattened_style_info.rs:21-97`); cycles drop out (empty output).
- `imported_by` set drives the web scoping rewrite (`:where([l-css-id="N"])`).
- Selector rewrites: `:root`→`[part="page"]`, `::placeholder`→
  `::part(input)::placeholder`, type-selector→tag map, entry-name attribute
  injection (`style_info_decoder.rs:86-221`).
- `Selector::generate_to_string_buf` (`raw_style_info.rs:272-314`) is the
  canonical simple-selector→CSS-text emitter (`.` for class, `#` for id, `[` `]`
  for attr, `::` for pseudo-element, etc.).
- rkyv round-trip entrypoints: `decode_style_info` / `get_style_content` /
  `get_font_face_content` (`decoded_style_data.rs:44-112`), reading via
  `rkyv::from_bytes_unchecked::<RawStyleInfo>`.

---

## 10. Version gates summary

From engine `core/renderer/tasm/config.h`:

| Macro | Version | Gates |
|---|---|---|
| `FEATURE_CSS_VALUE_VERSION` | v2.0 | `enable_css_parser_`: pattern byte in CSSValue; CompactD64 keyframe keys |
| `FEATURE_CSS_STYLE_VARIABLES` | v2.0 | `enable_css_variable_`: style-variables block; value_type+default_value trailer |
| `FEATURE_CSS_FONT_FACE_EXTENSION` | v2.7 | font-face list form (token_count prefix) vs single token |
| `LYNX_VERSION_2_9` | v2.9 | new `@import` rule handling (`GetEnableNewImportRule`) |
| `LYNX_VERSION_2_14` | v2.14 | CSSValue `default_value_map` trailer; keyframe multi-default |
| `FEATURE_CSS_IMPORTANT` | v3.9 | second `CSSAttributes` (important) in CSSParseToken |
| `FEATURE_CUSTOM_PROPERTY_DECLARATION_KEYFRAME` | v3.9 | keyframe custom-property content block |

`Config::IsHigherOrEqual(target_sdk_version_, FEATURE_*)` gates each. The reader
sets the flags in `lynx_binary_base_template_reader_impl.cc:142-147`
(`EnableCssVariable`/`EnableCssParser`/extension), and they also require the
matching `compile_options.enable_css_*` boolean to be true
(`...css_reader.cc:57-76`).
