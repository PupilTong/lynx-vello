# Lynx native binary template (`.lynx.bundle`)

Knowledge distilled from `/Users/akiwah/repos/lynx` (the Lynx engine C++ monorepo) and
verified empirically against real bundles built by rspeedy in
`/Users/akiwah/repos/lynx-stack` (e.g. `examples/react-externals/dist/main.lynx.bundle`).

This is the **"lynx" target** format: the binary produced by `@lynx-js/tasm`
(a NAPI/wasm packaging of the C++ encoder in the lynx repo) and consumed on
iOS/Android by the C++ `LynxBinaryReader`. The **"web" target** uses a completely
different, much simpler container — see [web-binary-template.md](web-binary-template.md).

## Source-of-truth files (lynx repo)

| File | Purpose |
| --- | --- |
| `core/template_bundle/template_codec/magic_number.h` | Magic constants |
| `core/template_bundle/template_codec/template_binary.h` | `BinarySection` enum, route structures |
| `core/template_bundle/template_codec/version.h` | Template version constants (V_1_0 … V_4_1) |
| `core/template_bundle/template_codec/compile_options.h` | Header ext-info fields (key ids) |
| `core/template_bundle/template_codec/header_ext_info.h` | Header ext-info field types |
| `core/template_bundle/template_codec/binary_decoder/lynx_binary_base_template_reader_impl.cc` | `DecodeHeader()` + section decode |
| `core/template_bundle/template_codec/binary_encoder/encoder.cc` | `encode()` entry point (called by `@lynx-js/tasm`) |
| `core/runtime/lepus/base_binary_reader.cc` | Lepus `Value` decoding |
| `base/include/value/base_value.h` | Lepus `ValueType` enum |

## Encoding conventions

- **Everything is little-endian.**
- Header strings are `u32 length + UTF-8 bytes` (verified empirically; the C++
  "compact" readers are currently fixed-width reads, not LEB128).
- Lepus values are type-tagged (1 byte) followed by type-specific payload.

## File layout (verified against a 3.2-era bundle)

```
u32   total_size            == file size in bytes
u32   magic                 0x00241922 (LepusNG/QuickJS) | 0xdd737199 (legacy Lepus VM)
str   lepus_version         e.g. "0.2.0.0"  (str = u32 len + utf8)
str   cli_version           deprecated, e.g. "unknown"
str   ios_version           target engine version, e.g. "3.2"
str   android_version       same as ios_version in practice

-- header ext info (present when target sdk >= 1.6) --
u32   header_ext_info_size  total bytes of this block incl. these 12 bytes
u32   ext_magic             0x494E464F ("INFO")
u32   field_count           e.g. 34
      repeated field_count times, no padding:
        u8   type           0=STR 1=U8 2=U16 3=U32 4=U64 5=I8 6=I16 7=I32 8=I64 9=F32 10=F64
        u8   key_id         see key table below
        u16  payload_size
        [payload_size bytes]

-- conditional trailer of the header --
value template_info         Lepus value; present when target sdk >= 2.7 (0x00 = Nil when absent-ish)
value trial_options         Lepus value; only when enable_trial_options flag set

-- body --
str   app_type              "card" | "DynamicComponent"
u8    snapshot              bool
      then sections until EOF (see below)
```

### Header ext-info key ids (from `compile_options.h`)

Numeric fields (type U8 unless noted): 1 enable_css_parser, 2 enable_css_external_class,
3 enable_css_strict_mode, 4 enable_lepus_ng, 5 default_overflow_visible,
6 enable_css_variable, 7 default_implicit_animation, 8 radon_mode (I32),
9 front_end_dsl (I32: TT=0 REACT=1 REACT_NODIFF=2 STANDALONE=3), 10 enable_keep_page_data,
11 enable_remove_css_scope, 13 enable_css_class_merge, 14 default_display_linear,
15 remove_css_parser_log, 16 enable_lynx_air, 17+ additional feature flags.
String fields: 0 target_sdk_version (e.g. "3.2"), 12 template_debug_url.
String fields are written after the numeric ones.

### Sections

`BinarySection` enum (`template_binary.h`):

```
0 STRING            7 DYNAMIC_COMPONENT              14 JS_BYTECODE
1 CSS               8 THEMED                         15 LEPUS_CHUNK
2 COMPONENT         9 USING_DYNAMIC_COMPONENT_INFO   16 CUSTOM_SECTIONS
3 PAGE             10 SECTION_ROUTE                  17 NEW_ELEMENT_TEMPLATE
4 APP              11 ROOT_LEPUS                     18 STYLE_OBJECT
5 JS               12 ELEMENT_TEMPLATE
6 CONFIG           13 PARSED_STYLES
```

Observed body layout in real bundles (fiber arch, sdk 3.2): a single
`SECTION_ROUTE` (0x0a) leads the body:

```
u8    0x0a                 SECTION_ROUTE marker
u32   route_count          e.g. 4
      repeated route_count times:
        u8   section_type
        u32  start_offset   relative to end of the route table
        u32  end_offset
      section data follows; each section begins with its own u8 type byte again
```

Verified: last route entry's `end_offset` + route-table-end == file size.

Section payloads (per the C++ readers):

- **STRING**: `compact_u32 count`, then `count` × (`len + utf8`) — the string table
  other sections index into.
- **CSS**: CSSRoute (`fragment_id -> {start,end}` map) then encoded
  `CSSParseToken` trees per fragment. Rule types enum in `template_binary.h`
  (2=style, 4=media, 5=font-face, 8=keyframes, …).
- **JS**: `u32 file_count`, then per file `str path` + `str source`
  (e.g. `/app-service.js`).
- **JS_BYTECODE**: `u32 engine_type (1=quickjs)`, `u32 file_count`, per file
  `str path` + `compact_u64 len` + raw QuickJS bytecode.
- **CONFIG**: one string containing JSON (page config).
- **ROOT_LEPUS**: main-thread code — QuickJS bytecode when magic is LepusNG.
- **LEPUS_CHUNK**: named map of extra Lepus chunks.
- **CUSTOM_SECTIONS**: its own route: `count` × (`str name`, `value header`,
  `u32 start`, `u32 end`).

### Lepus value encoding (`base_binary_reader.cc`)

Type byte from `ValueType` (`base_value.h`):

```
0 Nil        6 Closure     12 UInt64      18 ByteArray
1 Double     7 CFunction   13 NaN         19 RefCounted
2 Bool       8 CPointer    14 CDate       20 PrimJsValue
3 String     9 Int32       15 RegExp      21 FunctionTable
4 Table     10 Int64       16 JSObject
5 Array     11 UInt32      17 Undefined
```

- `Bool`: u8. `Int32/UInt32`: compact 32-bit. `Int64/UInt64`: compact 64-bit.
- `Double`: 64-bit IEEE 754.
- `String`: direct string in header context; string-table index in body context.
- `Table`: `count`, then `count` × (`key string`, recursive value).
- `Array`: `count`, then `count` × recursive value.
- `ByteArray`: `compact_u64 len` + raw bytes.

### Version gates worth knowing

- Header ext info: >= 1.6. `template_info`: >= 2.7. `trial_options`: >= 2.5.
- Flexible template (offset-map body layout, `GetFlexibleTemplateSectionOrder()`:
  STRING → PARSED_STYLES → ELEMENT_TEMPLATE → CSS → JS → JS_BYTECODE → CONFIG →
  ROOT_LEPUS → LEPUS_CHUNK → CUSTOM_SECTIONS → NEW_ELEMENT_TEMPLATE): >= 2.8.
- Current encoder default version: V_4_1; bundles built by current rspeedy
  report engine "3.2".

## Encode pipeline ("lynx" target)

```
rspeedy build (rsbuild/webpack)
  → @lynx-js/template-webpack-plugin LynxEncodePlugin
  → tinypool worker → @lynx-js/tasm encode()  (NAPI binary or wasm fallback of the C++ encoder)
  → .lynx.bundle
```

The encoder input is JSON (sourceContent/css/customSections/lepusCode/manifest…),
assembled by `LynxTemplatePlugin`; the C++ side parses CSS + TTML, compiles Lepus
to QuickJS bytecode, and serializes all sections.
