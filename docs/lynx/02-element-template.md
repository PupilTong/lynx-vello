# Lynx Element Template Binary Format

Authoritative reference for decoding the **element template** (compiled snapshot) sections of a
LynxJS template bundle. Distilled from the C++ engine codec
(`lynx/core/template_bundle/template_codec`) and the ReactLynx SWC transforms
(`lynx-stack/packages/react/transform/crates`).

Citations are `path:line` relative to the two repo roots above.

---

## 1. TL;DR for a decoder implementer

- **There are two element-template top-level sections**: legacy `ELEMENT_TEMPLATE` (section id `12`,
  offset-type `16`) and the new `NEW_ELEMENT_TEMPLATE` (section id `17`, offset-type `21`).
- **NEW_ELEMENT_TEMPLATE is the latest and the only one the current encoder emits.** The legacy
  `ELEMENT_TEMPLATE` enum value still exists for backward compatibility but
  `TemplateBinaryWriter::EncodeElementTemplateSection()` writes `BinarySection::NEW_ELEMENT_TEMPLATE`
  unconditionally (`binary_encoder/template_binary_writer.cc:470`). Target NEW.
- **"Compact" integers in this codebase are NOT LEB128.** `WriteCompactU32`/`ReadCompactU32` write/read
  a **fixed 4-byte little-endian `uint32`**. `U64` = 8 bytes LE, `D64` = 8 bytes IEEE-754, `U8` = 1 byte.
  The `// Returns the length of the leb128` comment is vestigial; the implementation is a fixed-width read.
- **Strings are inline, length-prefixed**: `CompactU32 length` (4 bytes LE) followed by raw UTF-8 bytes.
  No string-table indirection is used inside an element template.
- An element node is a **sequence of tagged sections**, each introduced by a 1-byte
  `ElementSectionEnum` tag, terminated by the `ELEMENT_CHILDREN` section. The node is prefixed by a
  raw 4-byte "children section offset" used for forward-skip of unknown sections.

---

## 2. Where the section sits in the bundle

### 2.1 File header (context only)

`TemplateBinaryWriter::EncodeHeader()` — `binary_encoder/template_binary_writer.cc:327`:

| Field | Encoding | Value |
|---|---|---|
| magic | `WriteU32` (4 bytes LE) | `0xDD737199` (`kLepusBinaryMagic`) for Lepus, or `0x00241922` (`kQuickBinaryMagic`) for LepusNG/QuickJS |
| lepus_version | string (deprecated) | |
| cli_version | string (deprecated) | |
| ios_version / android_version | string ×2 (deprecated; use `target_sdk_version_`) | |

Magic constants: `template_codec/magic_number.cc:11-14`
(`kQuickBinaryMagic = 0x00241922`, `kLepusBinaryMagic = 0xDD737199`,
`kTasmSsrSuffixMagic = 0xA8432251`, `kLepusBinaryVersion = 1`).

After the header comes a section count (`uint8`) and the section route table, then section bodies.
The element template section is only emitted when `compile_options_.enable_fiber_arch_` is true
(`template_binary_writer.cc:131-146`).

### 2.2 Section enums

`template_binary.h:23-69`. **These are 0-based and order-defined; integer value = position.**

`enum BinaryOffsetType` (used in the offset/route table):

```
0  TYPE_STRING                         12 TYPE_PAGE
1  TYPE_CSS                            13 TYPE_DYNAMIC_COMPONENT
2  TYPE_COMPONENT                      14 TYPE_SECTION_ROUTE
3  TYPE_PAGE_ROUTE                     15 TYPE_ROOT_LEPUS
4  TYPE_PAGE_DATA                      16 TYPE_ELEMENT_TEMPLATE        (legacy)
5  TYPE_APP                            17 TYPE_PARSED_STYLES
6  TYPE_JS                             18 TYPE_JS_BYTECODE
7  TYPE_CONFIG                         19 TYPE_LEPUS_CHUNK
8  TYPE_DYNAMIC_COMPONENT_ROUTE        20 TYPE_CUSTOM_SECTIONS
9  TYPE_DYNAMIC_COMPONENT_DATA         21 TYPE_NEW_ELEMENT_TEMPLATE    (latest)
10 TYPE_THEMED                         22 TYPE_STYLE_OBJECT
11 TYPE_USING_DYNAMIC_COMPONENT_INFO
```

`enum BinarySection`:

```
0  STRING        5  JS                  10 SECTION_ROUTE     15 LEPUS_CHUNK
1  CSS           6  CONFIG              11 ROOT_LEPUS         16 CUSTOM_SECTIONS
2  COMPONENT     7  DYNAMIC_COMPONENT   12 ELEMENT_TEMPLATE   17 NEW_ELEMENT_TEMPLATE  (latest)
3  PAGE          8  THEMED              13 PARSED_STYLES      18 STYLE_OBJECT
4  APP           9  USING_DYNAMIC_COMPONENT_INFO  14 JS_BYTECODE
```

> Note the two enums are **not** aligned: `BinarySection::NEW_ELEMENT_TEMPLATE == 17` but
> `BinaryOffsetType::TYPE_NEW_ELEMENT_TEMPLATE == 21`. The route table keys by `BinaryOffsetType`.

### 2.3 Legacy vs new — the switch and which is latest

The encoder always emits NEW (`binary_encoder/template_binary_writer.cc:462-476`):

```cpp
void TemplateBinaryWriter::EncodeElementTemplateSection() {
  if (element_template_ == nullptr || !element_template_->IsObject() ||
      element_template_->GetObject().MemberCount() == 0) return;
  TemplateSectionRecorder recorder(BinarySection::NEW_ELEMENT_TEMPLATE,
                                   BinaryOffsetType::TYPE_NEW_ELEMENT_TEMPLATE, ...);
  EncodeTemplatesToBinary(element_template_);
}
```

- **Gate to even produce the section:** `compile_options_.enable_fiber_arch_` (fiber architecture).
  Compile-option bit `enable_fiber_arch_` is index 25; `enable_flexible_template_` is index 27
  (`compile_options.h:141,143`).
- **No runtime version chooses legacy vs new at encode time** — the `ELEMENT_TEMPLATE`/`TYPE_ELEMENT_TEMPLATE`
  enum slots are retained only so old readers can recognize the id space. A modern decoder should
  implement `NEW_ELEMENT_TEMPLATE` and may treat a bare `ELEMENT_TEMPLATE` section as legacy/unsupported.

**Recommendation: decode `NEW_ELEMENT_TEMPLATE` (BinarySection 17 / BinaryOffsetType 21). It is "latest".**

---

## 3. NEW_ELEMENT_TEMPLATE section body

Encoder: `CSRElementBinaryWriter` (`binary_encoder/csr_element_binary_writer.cc`).
Decoder: `ElementBinaryReader` (`binary_decoder/element_binary_reader.cc`).

### 3.1 Section layout (multiple templates)

`EncodeTemplatesBody` (`csr_element_binary_writer.cc:97-129`) /
`DecodeTemplates` + routers (`element_binary_reader.cc:519-568`):

```
NEW_ELEMENT_TEMPLATE :=
  <template bodies, concatenated>      // one per template key
  OrderedStringKeyRouter               // moved to the FRONT of the section at encode time
```

`OrderedStringKeyRouter` (the element-templates router) —
`EncodeOrderedStringKeyRouter` (`csr_element_binary_writer.cc:264-279`) /
`DecodeOrderedStringKeyRouter` (`element_binary_reader.cc:876-890`):

```
router :=
  CompactU32 count
  count × {
    String  key                       // template_id, e.g. "_et_f47c3e863a57"
    CompactU32 start_offset            // offset of this template's body, RELATIVE to descriptor_offset_
  }
```

`descriptor_offset_` is the absolute stream offset where the router begins (set after the router is read).
A template body lives at `descriptor_offset_ + start_offset`.

Each **template body** (`EncodeTemplatesBody` loop, `DecodeTemplates`):

```
template_body :=
  CompactU32 element_count            // number of root elements for this key
  element_count × Element
```

(The Rust side keys these as `template_id` → root element; see §5.)

### 3.2 Element node layout

Encoder `EncodeElementRecursively` (`csr_element_binary_writer.cc:281-354`),
decoder `DecodeElementRecursively` (`element_binary_reader.cc:94-222` for fiber,
`:570-682` for `ElementInfo`).

```
Element :=
  U32 children_section_offset         // RAW 4-byte LE. Bytes from end-of-this-field to the
                                       // ELEMENT_CHILDREN tag. Used to skip unknown sections.
  [ Section ELEMENT_CONSTRUCTION_INFO ]   // optional, MUST be first if present
  Section  (ELEMENT_TAG_ENUM | ELEMENT_TAG_STR)   // REQUIRED, MUST be the tag section
  Section*  (any order, see emit order below)
  Section  ELEMENT_CHILDREN           // REQUIRED, ALWAYS LAST; terminates the node
```

Each section = `U8 ElementSectionEnum tag` + section-specific payload.

**Decoder skip rule (forward compat):** when the reader hits an unrecognized section tag, it
`Seek(children_section_offset)` and resumes at the children section
(`element_binary_reader.cc:212-217, 673-678`). The encoder therefore *requires* that any new section
be placed immediately before `ELEMENT_CHILDREN` (`csr_element_binary_writer.cc:327-332`).

**Encoder emit order** (`EncodeElementRecursively`, `csr_element_binary_writer.cc:281-354`):
1. `U32 0` placeholder for children offset (overwritten later)
2. tag section (`ELEMENT_TAG_ENUM` or `ELEMENT_TAG_STR`)
3. `ELEMENT_ATTRIBUTE_ARRAY`
4. `ELEMENT_SLOT_INDEX`
5. `ELEMENT_BUILTIN_ATTRIBUTE`
6. `ELEMENT_ID_SELECTOR`
7. `ELEMENT_STYLES`
8. `ELEMENT_CLASS`
9. `ELEMENT_EVENTS`
10. `ELEMENT_ATTRIBUTES`
11. `ELEMENT_DATA_SET`
12. `ELEMENT_PARSED_STYLES_KEY`
13. `ELEMENT_PARSED_STYLES`
14. `ELEMENT_CHILDREN` (always last)

Sections 2–13 are only emitted when the corresponding source field is present and non-empty.

### 3.3 `ElementSectionEnum` values

`core/renderer/dom/element_property.h:62-79`. **1-byte tag, values are 0-based:**

```
0  ELEMENT_CONSTRUCTION_INFO     8  ELEMENT_ATTRIBUTES
1  ELEMENT_TAG_ENUM              9  ELEMENT_EVENTS
2  ELEMENT_TAG_STR              10  ELEMENT_DATA_SET
3  ELEMENT_BUILTIN_ATTRIBUTE    11  ELEMENT_PARSED_STYLES
4  ELEMENT_ID_SELECTOR          12  ELEMENT_PARSED_STYLES_KEY
5  ELEMENT_CHILDREN             13  ELEMENT_PIPER_EVENTS
6  ELEMENT_CLASS                14  ELEMENT_ATTRIBUTE_ARRAY
7  ELEMENT_STYLES               15  ELEMENT_SLOT_INDEX
```

---

## 4. Per-section payloads

All `Section` payloads below come *after* the 1-byte tag. `Value` = a Lepus value via
`EncodeValue`/`DecodeValue` (typed: see `base_binary_reader` value codec — type byte then payload;
strings inside values follow the same length-prefixed inline scheme).

### Tag section — `ELEMENT_TAG_ENUM` (1) / `ELEMENT_TAG_STR` (2)
`EncodeElementTagSection` (`csr_element_binary_writer.cc:356-394`),
`ConstructElement`/`DecodeEnumTagSection`/`DecodeStrTagSection`
(`element_binary_reader.cc:469-507, 830-839`).

- `ELEMENT_TAG_ENUM` payload: `U8 ElementBuiltInTagEnum`.
- `ELEMENT_TAG_STR` payload: `String tag` (custom/user tag).

`enum ElementBuiltInTagEnum` (`element_property.h:27-48`) — note the **gap**, `ELEMENT_EMPTY = 13`:

```
0  ELEMENT_VIEW         5  ELEMENT_LIST         10 ELEMENT_OTHER
1  ELEMENT_TEXT         6  ELEMENT_COMPONENT    11 ELEMENT_X_TEXT
2  ELEMENT_RAW_TEXT     7  ELEMENT_PAGE         12 ELEMENT_X_SCROLL_VIEW
3  ELEMENT_IMAGE        8  ELEMENT_NONE         13 ELEMENT_EMPTY  (BUILTIN_TAG_EMPTY_ID; sentinel)
4  ELEMENT_SCROLL_VIEW  9  ELEMENT_WRAPPER      14 ELEMENT_INLINE_TEXT
                                                15 ELEMENT_X_INLINE_TEXT
                                                16 ELEMENT_X_NESTED_SCROLL_VIEW
                                                17 ELEMENT_INLINE_IMAGE
                                                18 ELEMENT_SLOT
```

Tag-string → enum map (encoder): `view`→0, `text`→1, `raw-text`→2, `image`→3, `scroll-view`→4,
`list`→5, `component`→6, `page`→7, `none`→8, `wrapper`→9, `x-text`→11, `x-scroll-view`→12, `slot`→18.
Unknown tags fall back to `ELEMENT_TAG_STR`.

### `ELEMENT_ATTRIBUTE_ARRAY` (14)
`EncodeElementAttributeArray` (`csr_element_binary_writer.cc:396-464`),
`DecodeAttributesArraySection` (`element_binary_reader.cc:695-722`).

This is the **primary attribute carrier for the NEW element template** (the pre-normalized template
attribute descriptor list). Payload:

```
CompactU32 valid_count
valid_count × {
  CompactU32  AttributeBindingType   // 0=STATIC, 1=DYNAMIC(slot), 2=SPREAD
  String      key                    // for SPREAD the literal "spread" is written
  if STATIC:  Value value            // literal value
  else:       CompactU32 attrSlotIndex   // points into the template-invocation attr-slot payload
}
```

`enum AttributeBindingType` (`element_property.h:21-25`): `STATIC=0`, `DYNAMIC=1`, `SPREAD=2`.
Source descriptor `kind` strings map: `"static"`→STATIC, `"slot"`→DYNAMIC, `"spread"`→SPREAD.

### `ELEMENT_SLOT_INDEX` (15)
`EncodeSlotElementIndex` (`csr_element_binary_writer.cc:466-478`),
`DecodeSlotElementIndexSection` (`element_binary_reader.cc:724-730`).
Payload: `CompactU32 elementSlotIndex`. Marks a slot-placeholder element with its mount-point index.

### `ELEMENT_BUILTIN_ATTRIBUTE` (3)
`EncodeElementBuiltinAttrSection` (`csr_element_binary_writer.cc:480-529`),
`DecodeBuiltinAttributesSection` (`element_binary_reader.cc:224-236, 684-693`).

```
CompactU32 count
count × { CompactU32 ElementBuiltInAttributeEnum ; Value value }
```

`enum class ElementBuiltInAttributeEnum` (`element_property.h:50-60`) starts at
`BUILTIN_ATTRIBUTE_MIN_ID = 1000`:

```
1000 COMPONENT_ID   1002 COMPONENT_PATH   1004 NODE_INDEX   1006 CONFIG
1001 COMPONENT_NAME 1003 CSS_ID           1005 DIRTY_ID     1007 IS_TEMPLATE_PART
```

> CSS property ids occupy `0..999`; builtin-attribute ids start at `1000` so the two key spaces never
> collide in a shared map (`element_property.h:13-18`). `config` JSON object is encoded under `CONFIG (1006)`.

### `ELEMENT_ID_SELECTOR` (4)
`csr_element_binary_writer.cc:531-540`, `element_binary_reader.cc:238-245, 732-735`.
Payload: `String idSelector`. (Source JSON key `idSelector`.)

### `ELEMENT_STYLES` (7) — inline styles
`csr_element_binary_writer.cc:542-572`, `element_binary_reader.cc:247-258, 737-749`.

```
CompactU32 count
count × { CompactU32 CSSPropertyID ; Value value }
```

Key is the numeric `CSSPropertyID` (the source JSON `styles` object keys are stringified property ids;
unknown ids are skipped). Value is typically a string.

### `ELEMENT_CLASS` (6)
`csr_element_binary_writer.cc:574-598`, `element_binary_reader.cc:260-270, 751-758`.

```
CompactU32 count
count × String className
```

### `ELEMENT_EVENTS` (9)
`EncodeElementJSEventSection` (`csr_element_binary_writer.cc:600-656`),
`DecodeEventsSection` (`element_binary_reader.cc:272-356, 760-777`).

```
CompactU32 count
count × {
  U8     EventTypeEnum
  String name           // event name; empty string written if absent
  String value          // handler/callback name; empty string written if absent
}
```

`enum class EventTypeEnum` (`core/renderer/events/events.h:68-75`):
`kBindEvent=0`, `kCatchEvent=1`, `kCaptureBind=2`, `kCaptureCatch=3`, `kGlobalBind=4`, `kMax=5`.
Source `type` strings (`bindEvent`/`catchEvent`/`capture-bind`/`capture-catch`/`global-bindEvent`)
map to these. A `kMax`/empty type is a decode error.

### `ELEMENT_PIPER_EVENTS` (13) — lepus/static events
`DecodePiperEventsSection` (`element_binary_reader.cc:358-417`). Same framing as events but the third
field is a `Value` (table of `{ piperFunctionName, piperFuncArgs }`) instead of a string callback.
(Emitted for the LEPUS target; not produced by `EncodeElementJSEventSection`.)

### `ELEMENT_ATTRIBUTES` (8) — generic attributes
`csr_element_binary_writer.cc:658-675`, `element_binary_reader.cc:419-429, 779-787`.

```
CompactU32 count
count × { String key ; Value value }
```

### `ELEMENT_DATA_SET` (10) — dataset
`csr_element_binary_writer.cc:677-694`, `element_binary_reader.cc:431-442, 789-800`.

```
CompactU32 count
count × { String key ; Value value }
```

### `ELEMENT_PARSED_STYLES_KEY` (12)
`csr_element_binary_writer.cc:696-706`, `element_binary_reader.cc:823-828`.
Payload: `String parsedStyleKey`. References a shared parsed-style entry in the `PARSED_STYLES`
section's `StringKeyRouter` (used in multi-template mode). In single-template mode the engine never
reaches this (styles are inlined via `ELEMENT_PARSED_STYLES`).

### `ELEMENT_PARSED_STYLES` (11)
`EncodeParsedStyle` (`csr_element_binary_writer.cc:176-245`),
`DecodeParsedStylesSectionInternal` (`element_binary_reader.cc:842-859`).

```
CompactU32 style_count
style_count × { CompactU32 CSSPropertyID ; CSSValue value }   // CSSValue via EncodeCSSValue/DECODE_CSS_VALUE
CompactU32 css_var_count
css_var_count × { String varName ; String varValue }
```

### `ELEMENT_CHILDREN` (5) — always last
`csr_element_binary_writer.cc:334-353`, `element_binary_reader.cc:455-467, 813-821`.

```
CompactU32 child_count
child_count × Element        // recursive; 0 if no children
```

---

## 5. ReactLynx snapshot → element template → binary

The Rust crates do **not** emit the binary; they emit the JSON tree that the C++
`CSRElementBinaryWriter` consumes (the `element_template_` rapidjson document).

### 5.1 swc_plugin_snapshot
`crates/swc_plugin_snapshot/lib.rs` transforms JSX into **snapshot** runtime calls and assigns each
static subtree a stable snapshot id (`_et_<hash>`, e.g. `_et_f47c3e863a57`). Dynamic holes (expression
children, dynamic attribute values, spreads, refs, events) are hoisted out as runtime "slots". The
snapshot is the React-runtime view; the static skeleton it references is the element template.

### 5.2 swc_plugin_element_template
`crates/swc_plugin_element_template/` lowers each static skeleton into the ET JSON consumed by the
encoder.

- **Extractor** (`extractor.rs`) walks JSX, separates static structure from `DynamicAttributePart` /
  `DynamicElementPart`, and assigns `attrSlotIndex` / `elementSlotIndex`.
- **`template_definition.rs`** serializes the static tree. The node shape (`element_template_to_json`
  + `element_template_element_node`, `template_definition.rs:133-145`) is:

  ```json
  { "kind": "element", "type": "<tag>", "attributesArray": [ ... ], "children": [ ... ] }
  ```

  Attribute descriptors (`template_definition.rs:103-131`):
  ```json
  { "kind": "static",  "key": "...", "value": <literal> }
  { "kind": "slot",    "key": "...", "attrSlotIndex": <i32> }
  { "kind": "spread",  "attrSlotIndex": <i32> }
  { "kind": "elementSlot", "type": "slot", "elementSlotIndex": <i32> }
  ```

- **Key mapping** (`template_attribute.rs:9-15`): `className` → `class`; namespaced attrs become
  `"ns:name"`. `__lynx_part_id` attributes are dropped (`template_definition.rs:251-253`).
- **Text optimization** (`template_definition.rs:315-355`): a `<text>`/`raw-text`/`inline-text`/
  `x-text`/`x-inline-text` element with a single static text child collapses the child into a
  `text` static attribute (no `raw-text` child node). Plain JSX text children otherwise become a
  `raw-text` element node carrying a `text` static attribute.
- **css-id**: when a CSS scope id is active it is appended as a `static` `css-id` attribute
  (`template_definition.rs:77-86, 368-370`).
- **Refs / events on the LEPUS target** are lowered to placeholder slot values `"1"`
  (`lowering.rs:49-61`) — the actual handler/ref lives in the runtime snapshot, the ET only records
  the slot binding.

### 5.3 JSON → binary key mapping

The encoder reads these JSON keys (`core/renderer/utils/base/tasm_constants.h`) and emits the
corresponding binary sections:

| ET JSON key | tasm_constants | Binary section |
|---|---|---|
| `type` | `kElementType` | tag (`ELEMENT_TAG_ENUM`/`ELEMENT_TAG_STR`) |
| `attributesArray` | `kElementAttributesArray` | `ELEMENT_ATTRIBUTE_ARRAY` (14) |
| `elementSlotIndex` | `kElementSlotIndex` | `ELEMENT_SLOT_INDEX` (15) |
| `builtinAttributes` + `config` | `kElementBuiltinAttributes`,`kElementConfig` | `ELEMENT_BUILTIN_ATTRIBUTE` (3) |
| `idSelector` | `kElementIdSelector` | `ELEMENT_ID_SELECTOR` (4) |
| `styles` | `kElementStyles` | `ELEMENT_STYLES` (7) |
| `class` | `kElementClass` | `ELEMENT_CLASS` (6) |
| `events` | `kElementEvents` | `ELEMENT_EVENTS` (9) |
| `attributes` | `kElementAttributes` | `ELEMENT_ATTRIBUTES` (8) |
| `dataset` | `kElementDataset` | `ELEMENT_DATA_SET` (10) |
| `parsedStyleKey` | `kElementParsedStyleKey` | `ELEMENT_PARSED_STYLES_KEY` (12) |
| `parsedStyle` | `kElementParsedStyle` | `ELEMENT_PARSED_STYLES` (11) |
| `children` | `kElementChildren` | `ELEMENT_CHILDREN` (5) |

> The Rust ET path centers on `attributesArray` (descriptor list). `class`, `id`, `style` arrive as
> `static` entries inside `attributesArray` (e.g. `{"kind":"static","key":"class","value":"container"}`,
> `crates/.../tests/__combined_snapshots__/should_output_template_with_static_attributes.snap`). The
> dedicated `ELEMENT_CLASS`/`ELEMENT_ID_SELECTOR`/`ELEMENT_STYLES` sections exist for direct
> styles/class/id JSON objects and are independent of the descriptor array.

---

## 6. Primitive encoding quick reference

| Primitive | Bytes | Notes |
|---|---|---|
| `U8` | 1 | raw byte (`ReadU8`/`WriteU8`) |
| `U32` (raw) | 4 LE | `WriteU32`/`ReadU32`; used for the children-section offset |
| `CompactU32` | **4 LE (fixed, not LEB128)** | `WriteCompactU32` → `WriteData(&value, 4)` (`runtime/lepus/output_stream.cc:80-82`); `ReadCompactU32` → `ReadUx<uint32_t>` (`runtime/lepus/binary_input_stream.cc:31-37`) |
| `CompactS32` | 4 LE | same fixed width |
| `CompactU64` | 8 LE | |
| `CompactD64` | 8 | IEEE-754 double |
| `String` | `CompactU32 len` (4 LE) + `len` UTF-8 bytes | `WriteStringDirectly` (`runtime/lepus/binary_writer.cc:42-52`) / `ReadStringDirectly` (`runtime/lepus/binary_reader.cc:16-30`). Inline; no string table. |
| `Value` | type byte + payload | Lepus value codec (`DecodeValue`/`EncodeValue`); nested strings use the same inline String scheme |

**Router note:** both `StringKeyRouter` and `OrderedStringKeyRouter` are written `{ count, (key,
offset)* }` and then **moved to the front** of their section by the encoder (`Move`), so the router is
the first thing a decoder reads in `NEW_ELEMENT_TEMPLATE` / `PARSED_STYLES`. Offsets stored in the
router are relative to `descriptor_offset_` (the router's own start). See
`csr_element_binary_writer.cc:247-279`, `element_binary_reader.cc:861-890`.

---

## 7. Decoder checklist

1. Read file header (magic 4 bytes LE → `0xDD737199` Lepus / `0x00241922` LepusNG), skip deprecated
   version strings, read section count + route table.
2. Find the `TYPE_NEW_ELEMENT_TEMPLATE` (21) range. (Ignore/upgrade-path any `TYPE_ELEMENT_TEMPLATE`=16.)
3. At the section start, read the `OrderedStringKeyRouter` (`CompactU32 count`, then
   `(String key, CompactU32 offset)*`). `descriptor_offset_` = stream pos after the router.
4. To decode template `key`: seek to `descriptor_offset_ + offset`, read `CompactU32 element_count`,
   then `element_count` Elements.
5. For each Element: read `U32 children_section_offset`; loop reading `U8` section tags until
   `ELEMENT_CHILDREN` (5). On an unknown tag, seek to `field_end + children_section_offset` and read
   the children section there. Children recurse.
6. Remember: every "compact" int is a fixed 4-byte LE read; every string is `4-byte LE length` + bytes.
