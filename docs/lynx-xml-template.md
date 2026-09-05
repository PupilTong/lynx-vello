# Lynx XML markup template

This document specifies the single-file Lynx XML source envelope accepted by
lynx-vello. The current grammar uses `engine-version` on the root and a
quoted `thread` selector on scripts:

| Current syntax | Replaced legacy syntax |
| --- | --- |
| `<lynx engine-version="4.2">` | `<lynx version="5.4.2">` |
| `<script thread="main">` | `<script main-thread>` |
| `<script thread="background">` | `<script background>` |

This is an intentional breaking grammar revision. The replaced spellings are
rejected rather than treated as aliases.

The inherited envelope and extraction behavior was derived from the upstream
implementation at the **2026-08-16** evidence snapshot:

- the parser and build-time encoder merged through
  [`lynx-stack#3402`](https://github.com/lynx-family/lynx-stack/pull/3402)
  and were released in `@lynx-js/web-core` 0.24.0;
- direct loading of raw XML is still proposed by
  [`lynx-stack#3390`](https://github.com/lynx-family/lynx-stack/pull/3390), at
  head `cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb`;
- the public native change cited by those implementations,
  [`lynx#7796`](https://github.com/lynx-family/lynx/pull/7796), exposes a
  [`TemplateBundleBuilder`](https://github.com/lynx-family/lynx/blob/dbaff6aa072fcf09d627abd571c00fff7857de46/core/template_bundle/lynx_template_bundle.cc#L56-L103),
  but does not contain a public XML parser or a style input.

Those references predate the attribute revision and therefore show the legacy
spellings in their examples. They remain evidence for the restricted
container, section extraction, and ingestion behavior, not for the current
attribute names.

The parser and its 45 tests no longer appear in #3390's current diff because
the same files are already in that PR's base through #3402; their absence from
the diff does not mean the raw loader stopped using them.

## Format identity and non-goals

Lynx XML is a source container for a hand-written card. It carries at most one
stylesheet and two JavaScript programs:

1. the required main-thread program;
2. an optional background-thread program;
3. an optional stylesheet.

The implementation also calls it “TemplateBundle XML” and a “buildless markup
card”; these names refer to the same XML-shaped source envelope.

Despite its name, it is **not a general XML document format**. It has no DOM
element vocabulary below `<lynx>`, no namespaces, entity decoding, mixed
content model, or schema extension mechanism. A view such as `<view>` is not
markup in this format: the main-thread program creates the element tree through
the Lynx Element PAPI.

It is also not a third `.web.bundle` encoding or an encoding of the native
`.lynx.bundle` binary format. `encodeLynxXML()` lowers it to the ordinary
modern `SDRA WROF` web binary format described in
[web-binary-template.md](web-binary-template.md). The bundle's reserved
`ElementTemplates` section and ReactLynx's similarly named element-template
compiler backend are unrelated concepts.

## Canonical document

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE lynx>
<lynx engine-version="4.2">
  <style><![CDATA[
    :root { background: white; }
    .title { color: #222; }
  ]]></style>

  <script thread="main"><![CDATA[
    // Create and update the page through the Lynx Element PAPI.
  ]]></script>

  <script thread="background"><![CDATA[
    // Optional background-thread program.
  ]]></script>
</lynx>
```

The XML declaration, doctype, style section, and background script are all
optional. The `<lynx>` root and one main-thread script section are mandatory.
Sections may occur in any order.

## Lexical rules

### ASCII whitespace

Where this document writes `S`, it means exactly one of:

| Character | Code point |
| --- | --- |
| space | U+0020 |
| horizontal tab | U+0009 |
| line feed | U+000A |
| carriage return | U+000D |
| form feed | U+000C |

This set deliberately differs from both JavaScript `trim()` and XML 1.0.
Notably, vertical tab and non-breaking space are not whitespace, while form
feed is accepted even though XML 1.0 does not allow it.

### Case and delimiters

All element names, attribute names and values, closing tags, `CDATA`, and the
`<?xml` prefix are case-sensitive. They must use the spellings shown here.
Only the `DOCTYPE` keyword and its `lynx` name are ASCII-case-insensitive.

Opening tags may contain the specified ASCII whitespace; closing tags are
exact strings with no internal whitespace: `</style>`, `</script>`, and
`</lynx>`. Self-closing tags are unsupported.

### Comments

`<!--` starts a comment and the first following `-->` ends it. The parser does
not validate an XML comment's contents.

Comments are ignorable wherever a grammar `Ignorable` appears: before the XML
declaration, between prelude items, between sections, and after the root.
A comment written inside a section body is payload text, not an ignorable
token.

### Byte order mark and source encoding

The string parser accepts one U+FEFF only as the first code point. A BOM after
whitespace or a second BOM is not ignorable.

The proposed raw web loader decodes response bytes as UTF-8. Text in an XML
declaration, including `encoding="..."`, is not interpreted and cannot select
another decoder. Build tools likewise pass a JavaScript string to the parser;
their byte-to-string decoding is outside this format.

## Grammar

The following EBNF is a compact guide. The procedural rules following it are
authoritative where scanning to a delimiter cannot be expressed cleanly.

```ebnf
Document          ::= BOM? Ignorable XMLDeclaration? Ignorable
                      Doctype? Ignorable LynxRoot Ignorable EOF

Ignorable         ::= (S | Comment)*
Comment           ::= "<!--" CommentBody "-->"

XMLDeclaration    ::= "<?xml" DeclarationBody "?>"
Doctype           ::= "<!" S* "DOCTYPE" S+ "lynx" S* ">"

LynxRoot          ::= RootOpen Ignorable
                      (Section Ignorable)*
                      "</lynx>"
RootOpen          ::= "<lynx" S+ EngineVersionAttribute S* ">"
EngineVersionAttribute
                  ::= "engine-version" S* "=" S* QuotedNonEmpty

Section           ::= Style | MainScript | BackgroundScript
Style             ::= "<style" S* ">" Body "</style>"
MainScript        ::= "<script" S+ MainThreadAttribute S* ">"
                      Body "</script>"
BackgroundScript  ::= "<script" S+ BackgroundThreadAttribute S* ">"
                      Body "</script>"

MainThreadAttribute
                  ::= "thread" S* "=" S* ("\"main\"" | "'main'")
BackgroundThreadAttribute
                  ::= "thread" S* "=" S*
                      ("\"background\"" | "'background'")

QuotedNonEmpty    ::= "\"" DoubleQuotedCodeUnit+ "\""
                    | "'" SingleQuotedCodeUnit+ "'"
DoubleQuotedCodeUnit
                   ::= any UTF-16 code unit except '"' and '>'
SingleQuotedCodeUnit
                   ::= any UTF-16 code unit except "'" and '>'
```

`DOCTYPE` and `lynx` in `Doctype` alone are matched without regard to ASCII
case. `CommentBody` ends at the first `-->`. `DeclarationBody` ends at the
first `?>`; its contents are otherwise unvalidated, so even the declaration's
usual `version` and `encoding` attributes are not required. The `<?xml` check
is only a prefix check with no name boundary, so `<?xml-stylesheet?>` is also
consumed as the one declaration slot.

This EBNF describes token arrangement. The cardinality and duplicate-slot rules
and the procedural definitions of `Body`, `CommentBody`, and `DeclarationBody`
are specified by the surrounding sections.

The parser permits comments before an XML declaration, unlike an XML processor.
At most one declaration and one doctype fit the document grammar. A doctype may
have extra ASCII whitespace, but public identifiers, system identifiers, and
internal subsets are rejected.

## Root and section constraints

The root opening tag accepts exactly one `engine-version` attribute. Its value
must be quoted and non-empty, but is otherwise opaque: the parser returns it as
`engine_version` without interpreting or negotiating it. An extra or
duplicated attribute rejects the document.

Non-empty means at least one UTF-16 code unit, so `engine-version=" "` is
valid. There is no backslash or entity escape mechanism. The first quote
matching the opener must also be the attribute's last character, and the
opening-tag scanner stops at the first `>` even inside quotes; a value cannot
contain its delimiting quote or a literal `>`. Characters such as `<` and
`&gt;` are not XML-validated or decoded.

The XML declaration's `version`, the root `engine-version`, and a
`.web.bundle` container version are distinct. The declaration body is ignored;
the root value is returned as card metadata; and the binary container version
belongs to the encoder.

The three section slots have these cardinalities:

| Section | Cardinality | Successful result field | Empty body |
| --- | --- | --- | --- |
| `<style>` | zero or one | `style: Option<&str>` | `Some("")` |
| `<script thread="main">` | exactly one | `main_thread_script: &str` | accepted as `""` |
| `<script thread="background">` | zero or one | `background_thread_script: Option<&str>` | `Some("")` |

A missing optional section produces `None`; a present but empty section
produces `Some("")`. The distinction is preserved through embedder mapping.
An empty main section satisfies the web parser's structural requirement,
although the native `TemplateBundleBuilder` in #7796 rejects an empty main
program. Portable documents should therefore provide a non-empty one.

The only accepted script attributes are:

```xml
<script thread="main">...</script>
<script thread='main'>...</script>
<script thread="background">...</script>
<script thread='background'>...</script>
```

Whitespace around `=` is allowed. `thread` must be the only attribute.
Missing, empty, unquoted, or differently cased values and arbitrary additional
attributes are rejected. `<style>` accepts no attributes.

Sections may be ordered arbitrarily, but a second section assigned to the same
slot is an error. Unknown root-level tags, processing instructions, namespaces,
and literal non-whitespace text between sections are errors.

## Section body extraction

A section body is extracted in one of two mutually exclusive modes.

### Bare body

Unless the body after leading ASCII whitespace starts with `<![CDATA[`, every
code unit before the first exact closing-tag string is returned verbatim. This
includes leading and trailing whitespace, comments, entity-looking text such as
`&lt;`, and nested-tag-looking text. No XML entity is decoded and no internal
markup is parsed, and no XML line-ending normalization is performed.

The consequence is important for JavaScript: a literal `</script>` anywhere in
a bare main or background body closes the section, even if it occurs inside a
JavaScript string or comment. CDATA permits those closing-tag literals provided
the payload does not itself contain `]]>`. Source containing both delimiters
cannot be represented verbatim in one section and must first be rewritten.

### CDATA body

If the ASCII-trimmed body starts with `<![CDATA[`, the whole trimmed body must
be exactly one CDATA wrapper ending in `]]>`. ASCII whitespace outside the
wrapper is discarded; content inside it is returned exactly. Empty CDATA is
valid.

CDATA may contain literal `</script>` or `</style>`. Multiple concatenated
CDATA wrappers, non-whitespace text after the wrapper, or an additional `]]>`
inside the extracted content are rejected. If non-whitespace text precedes the
CDATA marker, CDATA mode is not selected at all and the entire body is returned
as bare text. Entity references inside CDATA also remain literal text.

## Parser API and failures

`bobcat_source::xml::parse(source)` returns borrowed metadata and section bodies, or the
first structural error:

```rust
pub struct LynxXml<'source> {
    pub engine_version: &'source str,
    pub style: Option<&'source str>,
    pub main_thread_script: &'source str,
    pub background_thread_script: Option<&'source str>,
}
```

`ParseError::offset()` counts UTF-16 code units and
`ParseError::byte_offset()` reports the corresponding UTF-8 byte boundary.
Its display form is always:

```text
invalid TemplateBundle XML at offset <offset>: <message>
```

The two offsets agree for an ASCII prefix and may differ once non-ASCII text
precedes the failure.

The parser emits the following messages. Dynamic spellings are shown in angle
brackets in the table; the actual message includes the parsed tag text.

| Condition | `message` | Offset points at |
| --- | --- | --- |
| XML declaration has no `?>` | `unterminated XML declaration` | declaration `<` |
| comment has no `-->` | `unterminated comment` | comment `<` |
| `<!...` has no `>` | `unterminated doctype declaration` | declaration `<` |
| doctype is not the restricted Lynx form | `expected '<!doctype lynx>'` | declaration `<` |
| root is absent or misspelled | `expected '<lynx engine-version="...">' root element` | expected root position |
| a root prefix whose name boundary was recognized has no `>` | `unterminated '<lynx>' opening tag` | root `<` |
| root attributes violate the contract | `'<lynx>' requires exactly one non-empty 'engine-version' attribute` | root `<` |
| bare non-ignorable text occurs between sections | `unexpected content outside a section` | first bad code unit |
| a section opening tag has no `>` | `unterminated opening tag` | section `<` |
| an empty or slash-prefixed opening tag appears where a section must begin | `unexpected closing tag` | tag `<` |
| style has attributes | `'<style>' does not accept attributes` | style `<` |
| script attributes do not select exactly one slot | `'<script>' requires exactly one 'thread' attribute with value 'main' or 'background'` | script `<` |
| another root-level tag is used | `unsupported top-level tag '<TAG>'` | tag `<` |
| a slot is assigned twice | `duplicate '<OPENING_TAG>' section` | second section `<` |
| a section closing tag is absent | `missing closing tag '</style>'` or `missing closing tag '</script>'` | index immediately after opening `>` |
| a leading CDATA has no `]]>` | `unterminated CDATA section` | CDATA `<` |
| trimmed CDATA-mode body has trailing text and does not end in `]]>` | `unterminated CDATA section` | index immediately after opening `>` |
| text between the initial CDATA opener and final `]]>` contains an earlier `]]>` | `unexpected content after the CDATA section` | index immediately after opening `>` |
| root closing tag is absent | `missing closing tag '</lynx>'` | EOF |
| non-ignorable content follows the root | `unexpected content after '</lynx>'` | first trailing code unit |
| no main section was seen | `missing '<script thread="main">' section` | EOF after trailing ignorable content |

For a duplicate section, `<OPENING_TAG>` is the second opening tag's
ASCII-trimmed text, for example `script thread="main"`.

The implementation also contains a defensive `unknown error` fallback, but no
normal parser branch reaches it.

A bare `<lynx` at EOF does not pass the root-name boundary check and therefore
uses the earlier `expected '<lynx engine-version="...">' root element` error. The
unterminated-root message applies after the name was followed by ASCII
whitespace but no `>` was found.

## Logical template mapping

Both web ingestion paths start with the same logical lowering:

| XML source | Web template field |
| --- | --- |
| `<lynx engine-version="...">` | returned as source metadata; not currently applied to Bobcat runtime policy |
| `<style>` | stylesheet under CSS id `"0"` |
| main-thread script | `lepusCode.root` |
| background script | `manifest["/app-service.js"]` |
| no corresponding syntax | empty/absent `customSections` and `elementTemplates` |

The root main chunk is the chunk the web runtime automatically executes. The
background source is stored verbatim and is later evaluated through the
runtime's normal background chunk wrapper. The XML itself does not configure a
page or carry an initial-data processor.

The two translators use the same values but not quite the same configuration
layer:

| Key | Build-time `TasmJSON` and bundle | Proposed raw-worker Config |
| --- | --- | --- |
| `appType` | top-level `card`; used to derive `isLazy`, not serialized into Configurations | `card` |
| `cardType` | top-level `react`, then serialized as `react` | `react` |
| `isLazy` | derived and serialized as `false` | `false` |
| `enableCSSSelector` | page config serialized as `true` | `true` |
| `enableRemoveCSSScope` | page config serialized as `true` | `true` |
| `defaultDisplayLinear` | page config serialized as `false` | `false` |
| `defaultOverflowVisible` | page config serialized as `false` | `false` |
| `enableJSDataProcessor` | page config serialized as `false` | `false` |

The raw loader still participates in web-core's normal runtime configuration
override mechanism; that loader concern does not add attributes to the XML
format.

## Two web ingestion paths

The grammar is shared, but the produced artifact and CSS fallback policy are
not.

| Property | Build-time `encodeLynxXML` | Proposed raw XML loader |
| --- | --- | --- |
| Status at evidence snapshot | merged in #3402; released in `@lynx-js/web-core` 0.24.0 | open PR #3390 |
| Input boundary | caller supplies the complete source string | decode worker fetches and sniffs response bytes |
| Result | ordinary binary `.web.bundle` | in-memory JSON-artifact-shaped object, emitted as labeled worker messages |
| Downstream path | unchanged `SDRA WROF` decoder | load-time bypass joins the existing JSON assembly path |
| Produced protocol data | encoder writes Configurations, LepusCode, CustomSections, StyleInfo, and Manifest bundle sections, including empty maps | worker emits Config and Lepus messages always, a StyleInfo message only if style was present, and a Manifest message only if background was present |
| CSS not representable in `StyleInfo` | dropped with structured build diagnostics | kept as ordered verbatim browser CSS |
| Portability | same bundle capabilities as a normal built card | can acquire browser-only behavior from verbatim CSS |

The raw loader requires an HTTP status of exactly `200` and a response body. It
does not select XML by extension or response `Content-Type`. It requests
`application/octet-stream`, `application/json`, `application/xml`, and
`text/xml`, then inspects content: if the first character other than BOM/ASCII
whitespace is `<`, it tries the XML parser; byte zero equal to `{` selects the
JSON path; other input reaches the binary magic check. Its initial sniff window
is eight bytes, but a window composed entirely of ASCII whitespace and/or
U+FEFF causes it to read farther before classifying. Leading whitespace before
JSON is therefore not accepted as JSON.

The XML bytes are decoded by the default `TextDecoder`, which means UTF-8 with
replacement for malformed sequences rather than a fatal XML encoding error.
The sniff is intentionally broader than the grammar: it skips any number of
U+FEFF characters anywhere in the leading ignorable run, while the string
parser accepts one only at offset zero. Whitespace followed by a BOM and a
`<lynx>` root is therefore classified as XML and then rejected by the parser.
Classification as XML does not imply a successful parse.

On an XML parse failure, the proposed worker sends the formatted error and does
not emit partial template messages or a completion event. On success, present
messages are ordered Config, StyleInfo, LepusCode, Manifest; either optional
message is skipped when its source section is absent.

## CSS contract

Representable style rules are lowered to Lynx's `StyleInfo` model under CSS id
zero. Ordinary rules, `@font-face`, and `@keyframes` have model equivalents.
Tokenized rules pass through web-core's ordinary transformations, including
Lynx-only declaration transforms such as `display: linear` and selector
adaptation such as `:root` to the card page. They are also eligible for
`vw`/`vh`/`rem` rewriting when the host `<lynx-view>` enables its corresponding
`transform-vw`, `transform-vh`, or `transform-rem` option. Those options are
host settings, not XML attributes.

The binary model has no rule kind for conditional group rules and a one-sheet
XML card cannot resolve a URL import into another CSS id. This creates the most
important behavioral split between the two ingestion paths.

### Build-time path

`xmlToTasmJSON()` and `encodeLynxXML()` discard constructs the bundle cannot
carry and report each discarded kind:

| Reason | Constructs |
| --- | --- |
| `unrepresentable` | `@media`, `@supports`, `@layer` |
| `unsupported` | at-rules the Lynx CSS serializer does not recognize, including `@container`, `@property`, `@scope`, `@starting-style`, `@page`, `@charset`, and `@namespace` |
| `unresolvable` | URL-based `@import` |

A dropped group also drops every nested rule. The successful conversion result
returns a `discarded` list, and `encodeLynxXML()` additionally writes warnings.
A numeric CSS-id import can pass the encoder's syntax check, but the XML format
provides only sheet zero and no syntax for defining another sheet; authors must
not rely on it.

### Proposed raw-load path

The current #3390 head tokenizes representable top-level rules at load time.
It preserves `@media`, `@supports`, `@layer`, URL `@import`, unknown at-rules,
and unclassified raw fragments as browser CSS. Tokenized and verbatim entries
are kept in source order so equal-specificity cascade order is not changed. If
the entire stylesheet cannot be parsed, it is passed through verbatim.
Tokenized declarations retain custom properties and `!important`.

Here, `verbatim` names the raw CSS channel, not byte-for-byte preservation:
recognized at-rule nodes are regenerated by `css-tree` and may have formatting
or comments normalized. Only the whole-stylesheet parse fallback carries the
original source string as-is. A CSS parse problem is not an XML parse problem.

Rules inside a preserved block are not tokenized. Lynx selector, unit, and
declaration rewrites therefore do not occur inside that block; the browser
interprets it directly. This is a deliberate web-only capability and is not
equivalent to the build-time result.

The #3390 PR description's older statement that the whole stylesheet remains
verbatim, including an unreplaced `:root`, is stale. The source at the recorded
head implements the mixed tokenized/verbatim behavior above.

For consistent build, raw-web, and prospective native behavior, use ordinary
rules plus `@font-face` and `@keyframes`, and avoid conditional, unknown, and
import at-rules. See [tracking/css-at-rules.md](tracking/css-at-rules.md) for the
bundle representation inventory.

## Execution and trust boundary

The main and background bodies are JavaScript source, not declarative data.
Loading a document can execute both programs with the authority supplied by its
Lynx host. Consumers must apply the same trust and resource-origin policy they
apply to a built bundle.

The main program is responsible for constructing the page through the Element
PAPI and for normal lifecycle interaction. The format itself defines neither a
DOM serialization nor lifecycle callbacks. The optional background program is
placed at the conventional `/app-service.js` chunk key; omitting the section
means that no such chunk is present.

## Portable authoring profile

Until the raw loader and a public native parser converge, a document intended
for more than the merged web build path should:

- be encoded as UTF-8 and use at most one leading BOM;
- use the canonical lowercase spellings and the simple `<!DOCTYPE lynx>`;
- keep a non-empty main-thread program;
- prefer exactly one CDATA wrapper, and rewrite a payload that itself contains
  `]]>`;
- restrict CSS to tokenizable ordinary rules, `@font-face`, and `@keyframes`;
- treat the `engine-version` value as required metadata, not feature
  negotiation;
- not depend on diagnostic offsets matching after non-ASCII text.

## Known specification gaps

These are observed gaps rather than rules to guess around:

1. **Native parser provenance.** The web parser says it mirrors a native
   reference, and its test suite carries a native rejection corpus, but the
   public native #7796 patch contains only builder APIs. The claimed native
   grammar and `<style>` behavior cannot be independently derived from that PR.
2. **Empty main program.** The web parser accepts a present empty section;
   #7796's builder rejects an empty `mainThreadScript`.
3. **Empty background program.** Both web paths preserve a present empty
   background section as an empty `/app-service.js` entry. The native builder
   omits its background entry when the supplied string is empty.
4. **Native configuration provenance.** Comments in the web translators claim
   the native builder disables its CSS parser, while #7796's implementation
   sets both its compile option and page configuration CSS-parser flags to
   true. The builder has no stylesheet parameter with which to resolve the
   discrepancy.
5. **Error coordinates.** Web errors count UTF-16 code units while the cited
   native implementation counts UTF-8 bytes.
6. **Conditional CSS.** The merged encoder drops conditional groups; the open
   raw web loader preserves them for the browser. There is no single result to
   promise across both paths.
7. **Opaque engine version.** An engine version is syntactically mandatory and
   returned by the parser, but Bobcat does not yet validate it against the host
   engine or change runtime behavior based on it.

## Implications for lynx-vello

lynx-vello accepts the ordinary `.web.bundle` and the Lynx XML source envelope
at its reference-embedder boundaries. A Lynx XML card compiled with
`encodeLynxXML()` still fits the binary boundary and needs no new decoder
format; raw XML instead takes the explicit source-front-end path below.

`crates/bobcat-source/src/xml.rs` implements the separate source-parsing front end: it validates
the restricted grammar and returns the borrowed engine version, style,
main-thread script, and background-thread script. Its primary error offset
counts UTF-16 code units like the reference web parser, while `byte_offset()`
exposes the same position as a UTF-8 byte boundary for Rust callers. The parser
deliberately does not sniff inputs, perform I/O, apply the fixed template
mapping, parse CSS, encode a bundle, negotiate the engine version, or launch a
runtime. Rust `&str` cannot represent the lone UTF-16 surrogates that a
JavaScript string can contain; this does not affect the raw UTF-8/TextDecoder
ingestion path.

`bobcat-source` implements the shared mapping and source registration for
`bobcat-cli` and `bobcat-wasm`. They register the main-thread body as a script,
mount a present `<style>` body through `StyleSheetPayload::Text` before starting
that script, and construct the XML page with the fixed `false`/`false`/`true`
display/overflow/selector defaults unless the browser host deliberately
overrides them. Native registration retains and warns about a present background body;
browser registration only names and warns about it without copying its bytes. Neither executes it: Bobcat's background-thread realm and cross-realm protocol are
still pending.

Raw source CSS is evaluated by Stylo, so standard at-rules fall under this
repo's W3C-correctness policy. Lynx-only selector, unit, and declaration
rewrites are not synthesized for raw fragments. This integration remains an
explicit embedder/source-front-end path and is not hidden inside
`bobcat-source::web` as another binary encoding.

## Historical upstream evidence

These sources establish the inherited restricted-envelope behavior. They
predate the current attribute spellings described at the top of this document.

- [`parseLynxXML.ts` at #3390 head](https://github.com/lynx-family/lynx-stack/blob/cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb/packages/web-platform/web-core/ts/common/xml/parseLynxXML.ts)
  and its
  [45 parser tests](https://github.com/lynx-family/lynx-stack/blob/cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb/packages/web-platform/web-core/tests/parseLynxXML.spec.ts)
- [raw XML translation](https://github.com/lynx-family/lynx-stack/blob/cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb/packages/web-platform/web-core/ts/client/decodeWorker/xmlTemplate.ts),
  [CSS conversion](https://github.com/lynx-family/lynx-stack/blob/cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb/packages/web-platform/web-core/ts/common/xml/cssToStyleInfo.ts),
  and the
  [decode-worker integration](https://github.com/lynx-family/lynx-stack/blob/cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb/packages/web-platform/web-core/ts/client/decodeWorker/decode.worker.ts)
- the raw loader's
  [decode tests](https://github.com/lynx-family/lynx-stack/blob/cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb/packages/web-platform/web-core/tests/decode-xml.spec.ts)
  and
  [CSS tokenization tests](https://github.com/lynx-family/lynx-stack/blob/cfaed8c5d5e320082aa8288fafbca4fe1d3b4ecb/packages/web-platform/web-core/tests/markup-css-tokenization.spec.ts)
- [`xmlToTasmJSON.ts` from merged #3402](https://github.com/lynx-family/lynx-stack/blob/cc5c71453f12a3feb3f78b6067a049ef52b4fcd5/packages/web-platform/web-core/ts/encode/xmlToTasmJSON.ts)
  and its
  [bundle tests](https://github.com/lynx-family/lynx-stack/blob/cc5c71453f12a3feb3f78b6067a049ef52b4fcd5/packages/web-platform/web-core/tests/xml-to-web-bundle.spec.ts)
- [native `LynxTemplateBundle::Build()` at #7796 head](https://github.com/lynx-family/lynx/blob/dbaff6aa072fcf09d627abd571c00fff7857de46/core/template_bundle/lynx_template_bundle.cc#L56-L103)
- [`@lynx-js/web-core` 0.24.0 on npm](https://www.npmjs.com/package/@lynx-js/web-core/v/0.24.0)
- [`lynx-stack#3390`](https://github.com/lynx-family/lynx-stack/pull/3390),
  [`lynx-stack#3402`](https://github.com/lynx-family/lynx-stack/pull/3402), and
  [`lynx#7796`](https://github.com/lynx-family/lynx/pull/7796)
