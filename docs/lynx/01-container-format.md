# Lynx Template Binary Container Format (Outer Envelope)

Authoritative byte-level reference for the LynxJS **template/encode** binary container
as implemented in the C++ engine codec at
`/Users/akiwah/repos/lynx/core/template_bundle/template_codec`.
All path:line citations are into that `lynx` repo unless noted.

> Scope: this documents the **container envelope** — magic, header, version gates,
> section route table, the lepus primitive read/write helpers, the `lepus::Value`
> dynamic-value serialization, and the `header_ext_info` block. Section *payloads*
> (CSS, element template, JS bundle internals) are out of scope except where their
> framing touches the envelope.

---

## 0. Two unrelated "magic" formats — do not confuse

| Format | Magic | Where | Notes |
|---|---|---|---|
| **C++ engine template container** (THIS doc) | `0xdd737199` (lepus VM) or `0x00241922` (quick/LepusNG) | `lynx` repo, `magic_number.cc:11-13` | The real template bundle envelope. |
| web-platform `.lynx.bundle` wrapper | `0x41524453` `'SDRA'` + `0x464F5257` `'WROF'` | `lynx-stack/packages/web-platform/web-core/ts/constants.ts:51-52` | A *separate* outer container used only by the web runtime; its `version` gate is `<= 1` (`decode.ts:45-49`). Not the engine format. |

The engine container is what `LynxBinaryReader` / `TemplateBinaryWriter` produce and
consume. Everything below is the engine container.

---

## 1. Primitive encoding helpers (CRITICAL)

### 1.1 Byte order: little-endian, host-native `memcpy`

Fixed-width integers are written/read by raw `memcpy` of the host integer
(`OutputStream::WriteImpl` `output_stream.cc:19-30`; `InputStream::ReadUx`
`binary_input_stream.h:49-59`). On all supported targets this is **little-endian**.
The unit test pins it: reading 4 bytes `"test"` as a U32 yields
`'t' + ('e'<<8) + ('s'<<16) + ('t'<<24)` (`binary_input_stream_unittest.cc:142-154`),
i.e. byte 0 is the least-significant byte.

### 1.2 The `Compact*` helpers are NOT LEB128 in this codebase

This is the single most important decoder caveat. Despite the name, in this
open-source tree the "compact" varint helpers are **fixed-width little-endian**, not
variable-length:

| Helper | Bytes written | Source |
|---|---|---|
| `WriteCompactU32` / `ReadCompactU32` | **4** (raw `uint32_t` LE) | `output_stream.cc:80-82`, `binary_input_stream.cc:31-37` |
| `WriteCompactS32` / `ReadCompactS32` | **4** (raw `int32_t` LE) | `output_stream.cc:84-86`, `binary_input_stream.cc:39-45` |
| `WriteCompactU64` / `ReadCompactU64` | **8** (raw `uint64_t` LE) | `output_stream.cc:88-90`, `binary_input_stream.cc:47-53` |
| `WriteCompactD64` / `ReadCompactD64` | **8** (`double` bit-cast to U64, LE) | `output_stream.cc:92-94`, `binary_reader.h:73-80` |
| `WriteU8` / `ReadU8` | 1 | `binary_writer.cc:16-18`, `binary_reader.h:57-59` |
| `WriteU32` / `ReadU32` | 4 (raw `uint32_t` LE) | `binary_writer.cc:22-24`, `binary_reader.h:53-55` |

The unit tests confirm: `ReadCompactU64` on `"test str"` returns the full 8-byte LE
value (`binary_input_stream_unittest.cc:169-185`).

> Decoder recommendation: read `CompactU32`/`U32` as a 4-byte LE `u32`, `CompactU64`
> as an 8-byte LE `u64`, `CompactS32` as a 4-byte LE `i32`, `CompactD64` as an 8-byte
> LE value bit-cast to `f64`. There is **no continuation-bit varint** anywhere in this
> envelope. (A true LEB128 build would supply a different `InputStream` subclass; this
> source ships the fixed-width one. Match the bytes you actually observe, but the
> shipped encoder/decoder here are fixed-width.)

### 1.3 Length-prefixed strings: `WriteStringDirectly` / `ReadStringDirectly`

```
string := CompactU32 length     # 4 bytes LE
          u8[length] utf8_bytes  # raw, no NUL terminator
```
Encoder: `binary_writer.cc:46-52` (writes `length` then `length` bytes; zero-length
writes only the 4-byte length). Decoder: `binary_reader.cc:16-30`.

`EncodeUtf8Str` is just an alias for `WriteStringDirectly`
(`context_binary_writer.cc:225-231`), and `DecodeUtf8Str` is an alias for
`ReadStringDirectly` (`base_binary_reader.cc:189-197`). **There is no string-table /
string-id indirection in this open-source variant** — every string is inlined
length-prefixed at its use site. The `STRING` section and `string_list_` machinery
exist (enum value, route slot, `DeserializeStringSection`) but
`DeserializeStringSection` is a no-op stub returning `true`
(`base_binary_reader.cc:187`), and the route's STRING entry carries no payload to
resolve. Treat all `DECODE_STR`/`DECODE_STDSTR` as inline strings.

---

## 2. Top-level decode flow

`LynxBinaryBaseTemplateReader::Decode()` (`lynx_binary_base_template_reader_impl.cc:37-56`):

```
1. DecodeHeader()                       # §3
2. DidDecodeHeader()                    # build config decoder, page config
3. app_type   := ReadStringDirectly()   # §4  e.g. "card" / "DynamicComponent"
4. DidDecodeAppType()                   # validate against expected type
5. snapshot   := U8 (bool)              # 1 byte, unused/ignored
6. DecodeTemplateBody()                 # §5 — flexible vs non-flexible
7. DidDecodeTemplate()
```

---

## 3. Header layout (`DecodeHeader`, `lynx_binary_base_template_reader_impl.cc:58-149`)

Fields in exact wire order:

| # | Field | Encoding | Notes / source |
|---|---|---|---|
| 1 | `total_size` | `U32` (4B LE) | Must equal full buffer size or decode fails (`:62-70`). Encoder writes the bundle size as the first 4 bytes via `ByteArrayOutputStream::WriteToFile` framing as well; the reader requires `total_size == stream->size()`. |
| 2 | `magic_word` | `U32` (4B LE) | Dispatched by `DecodeMagicWord` (§3.1). |
| 3 | `lepus_version` | `string` | **Deprecated**, kept for compat. Drives the `lepus_version > MIN_SUPPORTED_VERSION` gate. `SupportedLepusVersion()` validates it (`:151-227`). |
| 4 | `cli_version` | `string` | **Deprecated**. Only present when `lepus_version > MIN_SUPPORTED_VERSION` (`:84-91`). |
| 5 | `ios_version` | `string` | **Deprecated name**; this is the real *engine / target_sdk* version. Same gate. |
| 6 | `android_version` | `string` | Currently `== ios_version` (the engine version). `CheckLynxVersion` validates unless value is `"unknown"` (`:96-118`). `target_sdk_version := ios_version`. |
| 7 | `header_ext_info` block | see §6 | Present **iff** `target_sdk_version >= V_1_6` (`FEATURE_HEADER_EXT_INFO_VERSION`, `config.h:46`). Parsed by `DecodeHeaderInfo` → fills `CompileOptions`. Otherwise `compile_options_.target_sdk_version_ = target_sdk_version` directly (`:122-127`). |
| 8 | `template_info` | `lepus::Value` (header mode) | Present **iff** `target_sdk_version >= V_2_7` (`FEATURE_TEMPLATE_INFO`, `config.h:52`). `DecodeValue(&template_info_, /*is_header=*/true)` (`:130-133`). |
| 9 | `trial_options` | `lepus::Value` (header mode) | Present **iff** `compile_options_.enable_trial_options_` (header-ext field id 20). Decoded then discarded (`:136-141`). |

Encoder counterpart: `EncodeHeader` (`template_binary_writer.cc:327-360`) writes magic,
the four version strings, then (gated) header-ext-info, template_info, trial_options.
Note the encoder writes `total_size` separately (it is patched/known after encode);
the reader treats byte 0 as `total_size`.

`MIN_SUPPORTED_VERSION = "0.1.0.0"` (deprecated, `config.h:22`). Version strings parse
to up to 4 numeric components (`LEPUS_VERSION_COUNT = 4`, `template_binary.h:21`) via
`VersionStrToNumber` (`:245-267`), splitting on `.` and `-`.

### 3.1 Magic word dispatch (`DecodeMagicWord`, `lynx_binary_base_template_reader.cc:22-42`)

| Magic (U32 LE) | Constant | Context | `is_lepusng_binary_` |
|---|---|---|---|
| `0x00241922` | `kQuickBinaryMagic` | `LepusNGContextType` (QuickJS) | `true` |
| `0xdd737199` | `kLepusBinaryMagic` | `VMContextType` (lepus VM) | `false` |

Any other value → decode fails. `kLepusBinaryVersion = 1` exists
(`magic_number.cc:14`) but is not written into this container header. Other magics in
`magic_number.h` (`kTasmSsrSuffixMagic = 0xa8432251`, `kRTSBinaryMagic`,
`kRTSNativeBinaryMagic`) belong to SSR/RTS payloads, not this envelope.

---

## 4. App type (`Decode` steps 3–5)

- `app_type` is an inline `string`. Known values: `"card"` (`APP_TYPE_CARD`) and
  `"DynamicComponent"` (`APP_TYPE_DYNAMIC_COMPONENT`) — `ttml_constant.h:43,45`.
- `DidDecodeAppType` maps anything `!= "DynamicComponent"` to `kCard`
  (`:314-337`).
- Then a single `U8` boolean `snapshot` follows (ignored).

---

## 5. Template body: section count + section route table

`DecodeTemplateBody` (`:339-349`) branches on `compile_options_.enable_flexible_template_`
(header-ext field id 27):

### 5.1 Non-flexible body (`DeserializeSection`, `:515-526`)

```
section_count := U8                      # 1 byte
repeat section_count times:
    type := U8                           # BinarySection enum value (§5.3)
    DecodeSpecificSection(type)          # payload decoded inline, in stream order
```
Encoder writes `section_count` via `EncodeSectionCount`
(`template_binary_writer.cc:362-371`): **`count = 7`**, and `count -= 2` (→ 5) when
`app_type == "DynamicComponent"`. Sections are then written back-to-back in the order
emitted by `Encode()` (CSS, JS, … see `template_binary_writer.cc:104-156`); each
section payload begins with its own `U8` type tag written by `TemplateSectionRecorder`
(`:82`).

### 5.2 Flexible body (`DecodeFlexibleTemplateBody`, `:351-393`)

Flexible templates (target >= V_2_8, `FEATURE_FLEXIBLE_TEMPLATE`) use a **route table**
so sections can be seeked individually:

```
# --- Section route (written FIRST in the buffer; moved to front by encoder) ---
section_route_type := U8                 # a route-format tag, currently ignored
section_count      := CompactU32 (4B)
repeat section_count times:
    section := U8                        # BinarySection enum value
    start   := CompactU32 (4B)           # offset relative to end-of-route
    end     := CompactU32 (4B)           # offset relative to end-of-route
# after the loop, offset = base; every (start,end) is rebased: += base
```
Source: `DecodeSectionRoute` (`:395-417`). The reader records `offset() == base` after
the route, then adds `base` to every start/end (`:411-415`).

Then sections are visited **in a fixed canonical order** (not route order), seeking to
each `start_offset_`, reading a `U8` type, and dispatching:

- Fiber arch (`enable_fiber_arch_`, field id 25): order from
  `GetFlexibleTemplateSectionOrder()` (`lynx_binary_base_template_reader.cc:44-60`):
  `STRING, PARSED_STYLES, ELEMENT_TEMPLATE, CSS, JS, JS_BYTECODE, CONFIG, ROOT_LEPUS,
  LEPUS_CHUNK, CUSTOM_SECTIONS, NEW_ELEMENT_TEMPLATE`.
- Non-fiber flexible: `kSectionOrder` (`:355-369`):
  `STRING, PARSED_STYLES, CSS, JS, JS_BYTECODE, COMPONENT, APP, PAGE, CONFIG,
  DYNAMIC_COMPONENT, USING_DYNAMIC_COMPONENT_INFO, THEMED, CUSTOM_SECTIONS`.

Encoder: `EncodeSectionRoute` (`template_binary_writer_impl.cc:93-105`) writes
`CompactU32 count`, then per section `U8 type, CompactU32 (start-base), CompactU32
(end-base)`, where `base = section_ary_[0].start_offset_`. `MoveLastSectionToFirst`
(`:107-124`) relocates the just-written route to the very front of the body.

### 5.3 `BinarySection` enum — integer values (`template_binary.h:49-69`)

Values are the C++ enum ordinals (sequential from 0), and are the `U8` type tags on the
wire:

| Val | Name | Val | Name |
|----:|------|----:|------|
| 0 | `STRING` | 10 | `SECTION_ROUTE` |
| 1 | `CSS` | 11 | `ROOT_LEPUS` |
| 2 | `COMPONENT` | 12 | `ELEMENT_TEMPLATE` (legacy; decode now errors) |
| 3 | `PAGE` | 13 | `PARSED_STYLES` |
| 4 | `APP` | 14 | `JS_BYTECODE` |
| 5 | `JS` | 15 | `LEPUS_CHUNK` |
| 6 | `CONFIG` | 16 | `CUSTOM_SECTIONS` |
| 7 | `DYNAMIC_COMPONENT` | 17 | `NEW_ELEMENT_TEMPLATE` |
| 8 | `THEMED` | 18 | `STYLE_OBJECT` |
| 9 | `USING_DYNAMIC_COMPONENT_INFO` | | |

Dispatch table: `DecodeSpecificSection` (`:419-513`). Notable gates:
`ELEMENT_TEMPLATE` (12) is rejected as legacy (`:481-487`); `PARSED_STYLES` (13)
requires `arch_option_ == FIBER_ARCH` (`:488-498`); `STYLE_OBJECT` (18) is only
reached via the simple-styling path.

`BinaryOffsetType` (`template_binary.h:23-47`) is a parallel, **differently-numbered**
enum used only for the encoder's internal `offset_map_` (e.g. `TYPE_PAGE_ROUTE`,
`TYPE_PAGE_DATA` shift the values). Do not use `BinaryOffsetType` ordinals to decode
the wire; the wire uses `BinarySection`.

### 5.4 Selected section framings (envelope-relevant)

- **CONFIG (6):** single inline `string` (the page-config JSON)
  (`lynx_binary_base_template_reader.cc:62-72`).
- **JS source (5):** `U32 count`, then per entry `string path` + `string content`
  (`:528-539`).
- **JS bytecode (14):** `U32 engine` (must == `JSRuntimeType::quickjs`), `U32 count`,
  then per entry `string path`, `CompactU64 data_len` (8B), `u8[data_len]`
  (`:541-560`).
- **LEPUS_CHUNK route (15):** `CompactU32 size`, then per entry `string path`,
  `CompactU32 start`, `CompactU32 end` (`lynx_binary_reader.cc:227-246`).
- **CUSTOM_SECTIONS (16) route:** `U32 size`, then per entry `string key`,
  `lepus::Value header` (non-header mode), `U32 start`, `U32 end`
  (`lynx_binary_reader.cc:281-302`). Per-section content encoding is chosen by the
  header table's `"encoding"` number: `0=STRING`, `1=JS_BYTECODE`, `2=CSS`
  (`CustomSectionEncodingType`, `template_binary.h:80`). `JS_BYTECODE` payload =
  `CompactU64 code_len` + raw bytes (`:330-338`).

---

## 6. `header_ext_info` block (magic `0x494e464f` = "INFO")

Defined in `header_ext_info.h`. Read by `DecodeHeaderInfo`
(`lynx_binary_base_template_reader_impl.cc:269-297`) / written by `EncodeHeaderInfo`
(`template_binary_writer_impl.cc:126-163`). The block is read by **raw struct
`memcpy`**, so it is host-endian (LE) and `#pragma pack(4)` aligned.

### 6.1 Block header (12 bytes, `struct HeaderExtInfo`)

```
header_ext_info_size          : U32 (4B LE)   # total block size incl. this header + all fields
header_ext_info_magic_        : U32 (4B LE)   # == 0x494e464f  ("INFO" / 'O''F''N''I' in LE bytes)
header_ext_info_field_numbers_: U32 (4B LE)   # number of HeaderExtInfoField entries
```
`HEADER_EXT_INFO_MAGIC 0x494e464f` (`header_ext_info.h:11`). The decoder reads
`sizeof(HeaderExtInfo)` = 12 bytes, asserts the magic, then loops `field_numbers_`
times.

### 6.2 Each field (`HeaderExtInfoField`, `#pragma pack(4)`)

On wire, only the first 3 fields are serialized (the in-memory `void* payload_` is
**not** written; encoder writes `sizeof(field) - sizeof(void*)` = 4 bytes, then the
payload separately — `template_binary_writer_impl.cc:165-172`, reader mirror
`:299-312`):

```
type_         : U8           # value-type tag, see §6.3
key_id_       : U8           # field id (maps to a CompileOptions field, see §6.4)
payload_size_ : U16 (2B LE)  # payload byte length
payload       : u8[payload_size_]   # raw little-endian value or UTF-8 (no length prefix)
```
So each field header is **4 bytes** (1+1+2) followed by `payload_size_` payload bytes.

After all fields, the reader does `Seek(curr_offset + header_ext_info_size)` to skip any
forward-compatible padding (`:294`).

### 6.3 Field value types (`header_ext_info.h:29-50`)

| `type_` | Meaning | payload bytes |
|---:|---|---|
| 0 | `TYPE_STRING` | UTF-8, `payload_size_` bytes (no NUL) |
| 1 | `TYPE_UINT8` | 1 |
| 2 | `TYPE_UINT16` | 2 |
| 3 | `TYPE_UINT32` | 4 |
| 4 | `TYPE_UINT64` | 8 |
| 5 | `TYPE_INT8` | 1 |
| 6 | `TYPE_INT16` | 2 |
| 7 | `TYPE_INT32` | 4 |
| 8 | `TYPE_INT64` | 8 |
| 9 | `TYPE_FLOAT` | 4 |
| 10 | `TYPE_DOUBLE` | 8 |

Reinterpretation is by `memcpy` when `payload_size_ == sizeof(T)`
(`ReinterpretHeaderInfoValue`, `:23-33`), so fixed-width fields are LE.

### 6.4 Field `key_id_` → `CompileOptions` mapping

Fixed-length fields (`compile_options.h:117-149`, `FOREACH_FIXED_LENGTH_FIELD`):

| id | type | field | id | type | field |
|--:|--|--|--:|--|--|
| 1 | U8 | `enable_css_parser_` | 18 | U8 | `enable_event_refactor_` |
| 2 | U8 | `enable_css_external_class_` | 19 | U8 | `force_calc_new_style_` |
| 3 | U8 | `enable_css_strict_mode_` | 20 | U8 | `enable_trial_options_` |
| 4 | U8 | `enable_lepus_ng_` | 21 | U8 | `enable_async_css_decode_` |
| 5 | U8 | `default_overflow_visible_` | 22 | U8 | `enable_css_engine` |
| 6 | U8 | `enable_css_variable_` | 23 | U8 | `enable_component_config_` |
| 7 | U8 | `default_implicit_animation_` | 24 | U8 | `lynx_air_mode_` |
| 8 | I32 | `radon_mode_` | 25 | U8 | `enable_fiber_arch_` |
| 9 | I32 | `front_end_dsl_` | 26 | U8 | `lepusng_debuginfo_outside_` |
| 10 | U8 | `enable_keep_page_data` | 27 | U8 | `enable_flexible_template_` |
| 11 | U8 | `enable_remove_css_scope_` | 28 | U8 | `arch_option_` |
| 13 | U8 | `enable_css_class_merge_` | 29 | U8 | `enable_css_selector_` |
| 14 | U8 | `default_display_linear_` | 30 | U8 | `enable_reuse_context` |
| 15 | U8 | `remove_css_parser_log_` | 31 | U8 | `enable_css_invalidation_` |
| 16 | U8 | `enable_lynx_air_` | 32 | U8 | `enable_async_lepus_chunk_decode_` |
| 17 | U8 | `enable_lazy_css_decode_` | 33 | U8 | `enable_simple_styling_` |

(Note: id `12` is a string field; ids skip `12` in the fixed list.)

String fields (`FOREACH_STRING_FIELD`, `compile_options.h:151-153`):

| id | field |
|--:|--|
| 0 | `target_sdk_version_` |
| 12 | `template_debug_url_` |

---

## 7. `lepus::Value` dynamic value serialization

`DecodeValue(Value*, bool is_header)` (`base_binary_reader.cc:240-326`); encoder
`EncodeValue` mirrors it. Wire form:

```
value := U8 type_tag            # lepus::ValueType ordinal (§7.1)
         <payload by tag>
```

### 7.1 `lepus::ValueType` enum — integer values (`base_value.h:65-91`)

The serializer keys on these ordinals. The enum is **declared in this order**, so the
ordinals are:

| Val | Name | Serialized? | Payload |
|----:|------|:--:|---|
| 0 | `Value_Nil` | yes | none → `SetNil()` |
| 1 | `Value_Double` | yes | `CompactD64` (8B, double bit-cast) |
| 2 | `Value_Bool` | yes | `U8` (0/1) |
| 3 | `Value_String` | yes | header mode: `ReadStringDirectly`; else `DecodeUtf8Str` (both inline length-prefixed) |
| 4 | `Value_Table` | yes | table (§7.2) |
| 5 | `Value_Array` | yes | array (§7.3) |
| 6 | `Value_Closure` | lepus-VM only | closure blob (`#if !ENABLE_JUST_LEPUSNG`) |
| 7 | `Value_CFunction` | no | no-op |
| 8 | `Value_CPointer` | no | no-op |
| 9 | `Value_Int32` | yes | `CompactS32` (4B LE, signed) |
| 10 | `Value_Int64` | yes | `CompactU64` (8B LE; reinterpreted as int64) |
| 11 | `Value_UInt32` | yes | `CompactU32` (4B LE) |
| 12 | `Value_UInt64` | (not in switch) | — |
| 13 | `Value_NaN` | lepus-VM only | `U8` bool flag → `SetNan` |
| 14 | `Value_CDate` | lepus-VM only | 12× `CompactS32` (see §7.4) |
| 15 | `Value_RegExp` | lepus-VM only | 2× `string` (pattern, flags) |
| 16 | `Value_JSObject` | no | — |
| 17 | `Value_Undefined` | yes | none → `SetUndefined()` |
| 18 | `Value_ByteArray` | yes | `CompactU64 len` (8B) + raw `u8[len]` |
| 19 | `Value_RefCounted` | no | no-op |
| 20 | `Value_PrimJsValue` | — | (JS-tag region) |
| 21 | `Value_FunctionTable` | — | |
| 22 | `Value_TypeCount` | sentinel | not a value |

Decoder switch evidence: `base_binary_reader.cc:243-321`. Types 6/13/14/15 are guarded
by `!ENABLE_JUST_LEPUSNG`; in a LepusNG-only build they will not appear.

### 7.2 Table (`Value_Table`) — `DecodeTable` (`base_binary_reader.cc:209-229`)

```
size := CompactU32 (4B)
repeat size times:
    key   := string            # header mode: ReadStringDirectly; else DecodeUtf8Str
    value := lepus::Value       # recursive, same is_header flag
```
Encoder emits entries **sorted by key** (`context_binary_writer.cc:238-254`).

### 7.3 Array (`Value_Array`) — `DecodeArray` (`base_binary_reader.cc:231-238`)

```
size := CompactU32 (4B)
repeat size times: lepus::Value   # recursive, non-header
```

### 7.4 Date (`Value_CDate`) — `EncodeDate` order (`context_binary_writer.cc:266-284`)

12 consecutive `CompactS32` (4B LE each): `language, ms, tm_year, tm_mon, tm_mday,
tm_hour, tm_min, tm_sec, tm_wday, tm_yday, tm_isdst, tm_gmtoff`.

### 7.5 `is_header` flag

When `is_header == true`, strings/table-keys are read with `ReadStringDirectly` because
no string list is populated during header parsing
(`base_binary_reader.cc:217-221,266-269`). In this open-source variant the non-header
path also resolves to inline strings, so the practical wire form is identical
(length-prefixed inline). The flag matters only if a true string-table build is used.

---

## 8. Old-vs-new switches and "latest" recommendation

| Switch | Gate | Recommendation (latest) |
|---|---|---|
| Magic / VM | `kQuickBinaryMagic` (LepusNG/QuickJS) vs `kLepusBinaryMagic` (lepus VM) | **LepusNG (`0x00241922`)** is the modern path; lepus-VM is legacy and may be compiled out (`ENABLE_JUST_LEPUSNG`). Support both for decode. |
| Body framing | non-flexible (`DeserializeSection`, U8 count) vs flexible (`SECTION_ROUTE` route table) | **Flexible** (target >= V_2_8) is latest; needed for seekable/fiber sections. |
| Architecture | non-fiber `kSectionOrder` vs **fiber** `GetFlexibleTemplateSectionOrder()` | **Fiber arch** (`enable_fiber_arch_`, id 25; target >= V_2_8) is the current arch; enables `PARSED_STYLES`, `ELEMENT_TEMPLATE`/`NEW_ELEMENT_TEMPLATE`, `LEPUS_CHUNK`, `CUSTOM_SECTIONS`. |
| Element template | legacy `ELEMENT_TEMPLATE` (12, **decode rejected**) vs `NEW_ELEMENT_TEMPLATE` (17) | **`NEW_ELEMENT_TEMPLATE`** only; legacy hard-errors (`:481-487`). |
| Header ext info | absent (target < V_1_6) vs present `0x494e464f` block | **Present**; all modern bundles (>= V_1_6) carry `CompileOptions` here. |
| `template_info` | absent (< V_2_7) vs present | **Present** (>= V_2_7). |
| Version ceiling | `LYNX_VERSION = V_4_1`, `LYNX_TASM_MAX_SUPPORTED_VERSION = V_3_9`, `MIN_SUPPORTED_LYNX_VERSION = V_1_0` (`config.h:27-30`) | A decoder should accept engine versions in `[V_1_0, current_sdk]`. |

---

## 9. Quick decoder checklist

1. Read `U32 total_size`; verify `== buffer.len()`.
2. Read `U32 magic`; pick LepusNG vs lepus VM (else fail).
3. Read 4 inline strings: lepus_version, cli_version, ios_version, android_version
   (last 3 only if `lepus_version > "0.1.0.0"`). `target_sdk = ios_version`.
4. If `target_sdk >= 1.6`: parse `header_ext_info` (§6) → CompileOptions; else
   `target_sdk` stands alone.
5. If `target_sdk >= 2.7`: `template_info := DecodeValue(header=true)`.
6. If `enable_trial_options_`: `DecodeValue(header=true)` and discard.
7. `app_type := string`; validate; read `U8 snapshot` (ignore).
8. If `enable_flexible_template_`: read SECTION_ROUTE (`U8 fmt`, `CompactU32 count`,
   per-entry `U8 type + CompactU32 start + CompactU32 end`, rebased to post-route
   offset), then visit sections in the canonical fiber/non-fiber order, seeking to each
   `start` and reading a leading `U8 type`. Else: read `U8 count`, then `count`×
   (`U8 type` + inline payload).
9. All multi-byte ints are **fixed-width little-endian**; all strings are
   `CompactU32`-length-prefixed UTF-8; `lepus::Value` is `U8 tag` + tagged payload (§7).
