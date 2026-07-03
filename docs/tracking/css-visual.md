# CSS visual & paint properties

> Research: multi-agent sweep over `lynx/` and `lynx-stack/` (see [AGENTS.md](../../AGENTS.md) for the reference-repo shorthand and the W3C-first standards policy). Supersedes the earlier stub.

### Paint & Visual CSS Properties

Lynx's paint/visual CSS surface is implemented in `lynx/core/renderer/css/parser/*_handler.cc` (value parsing, driven by `CSSStringParser` in `core/renderer/css/parser/css_string_parser.{h,cc}`), backed by POD structs in `lynx/core/style/*_data.h`, and enumerated end-to-end in the web-bundle wire format at `lynx-stack/packages/web-platform/web-core/src/template/template_sections/style_info/css_property.rs` (this is the authoritative "what actually reaches lynx-vello" list — cross-checked against the C++ side). Overall the surface is a reasonably faithful CSS3 paint subset with a few real gaps (no `backdrop-filter`, no blend modes, no `border-image`, single-function `filter`, no repeating-gradients, no `clip-path: url()`/`polygon()`) and a handful of Lynx-only extensions (`color: <gradient>` as text-gradient sugar, `background-clip: border-area`, `super-ellipse()` basic-shape, permissive unitless/legacy numeric compatibility modes). Gradient angle conventions and side/corner keywords match the CSS spec exactly (0deg = "to top", 180deg = "to bottom") — this is **not** a divergence, contrary to what one might assume from other engines' quirks.

#### Color & general paint

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `color` (keyword/hex/rgb/rgba/hsl/hsla) | Text color, standard grammar | Core | Yes | | lynx/core/renderer/css/css_color.h, lynx/core/renderer/css/parser/color_handler.cc, lynx/core/renderer/css/parser/css_string_parser.cc:2044 |
| `color: <linear-gradient>/<radial-gradient>` | Gradient text fill via `color` itself | Extended | No | Lynx-only sugar equivalent to `background-clip:text`+transparent color+`background-image`; web-core implements it as a value-sniffing rewrite (`starts_with("linear-gradient")` → injects `background-clip:text`). lynx-vello should detect gradient-valued `color` and internally lower to a clip-to-text-glyph-path + gradient fill, not treat `color` as accepting `<image>` per spec | lynx/core/renderer/css/parser/color_handler.cc:48, lynx/core/renderer/css/parser/css_string_parser.cc:450 (ParseTextColorTo), lynx-stack/packages/web-platform/web-core/src/style_transformer/rules.rs:258-290,374-397 |
| `rgb()`/`rgba()` | rgba treated as alias of rgb (CSS Color 4 behavior) | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.h:376-382 |
| `hsl()`/`hsla()` | | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.h:383-385 |
| Hex color (`#rgb`,`#rgba`,`#rrggbb`,`#rrggbbaa`) | | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc (HexColor) |
| Named colors | | Core | Yes (assumed CSS named-color table; not individually enumerated) | | lynx/core/renderer/css/css_color.h:20 (ParseNamedColor) |
| `opacity` | Element opacity, animatable, listed as canonical computed value | Core | Yes | | lynx/core/renderer/css/computed_css_style.cc:5202,5258, lynx/core/renderer/css/css_style_utils.cc:508 |
| `visibility` | `visible`/`hidden`/`collapse` | Core | Yes (assumed; not independently checked) | | lynx-stack/.../css_property.rs:120,352, lynx/core/renderer/css/computed_css_style.cc:5230 |
| `mix-blend-mode` | Not found anywhere in parser or wire enum | — | No (absent) | Not supported at all — flag as a real gap vs. web dev expectations if compositing/blend modes are needed | grep across lynx/core/renderer/css/ (no hits), lynx-stack/.../css_property.rs (no hits) |
| `background-blend-mode` | Not found anywhere | — | No (absent) | Same as above | grep across lynx/core/renderer/css/ (no hits), lynx-stack/.../css_property.rs (no hits) |

#### Background (color / image / gradient / position / size / repeat / origin / clip)

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `background-color` | | Core | Yes | | lynx-stack/.../css_property.rs:255 |
| `background` shorthand | Expands into image/position/size/repeat/origin/clip(+composite for mask) arrays, multi-layer via comma list | Core | Partial | FIXME in code: if a background layer has no image, Lynx silently skips updating that layer's other sub-properties ("different from the web") — a real, acknowledged multi-layer background bug/divergence. lynx-vello should implement full per-layer independence per spec instead | lynx/core/renderer/css/parser/css_string_parser.cc:144-195 (see FIXME comment at :163-165), lynx-stack/.../css_property.rs:307 |
| `background-image` (multi-layer, comma-separated) | `url()` \| `linear-gradient()` \| `radial-gradient()` \| `conic-gradient()` \| `none` | Core | Yes | | lynx/core/renderer/css/parser/background_image_handler.cc, lynx/core/renderer/css/parser/css_string_parser.cc:197 |
| `linear-gradient()` angle syntax | `<angle>` in deg/grad/rad/turn, or `to <side-or-corner>`; 0deg="to top" | Core | Yes | Matches CSS spec exactly — no angle-convention quirk (verify before assuming otherwise) | lynx/core/renderer/css/parser/css_string_parser.cc:1387-1514 |
| `radial-gradient()` | `circle`/`ellipse` shape, `closest-side/corner`, `farthest-side/corner`, explicit length(s), `at <position>` | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:1515-1628 |
| `conic-gradient()` | `from <angle>`, `at <position>` | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:1629-1687 |
| `repeating-linear-gradient()` / `repeating-radial-gradient()` / `repeating-conic-gradient()` | Not found — only `TokenType::LINEAR_GRADIENT/RADIAL_GRADIENT/CONIC_GRADIENT` recognized, no REPEATING_* variants | — | No (absent) | Not supported. Implement via vello's gradient extend modes if bundles ever need it; otherwise document as unsupported author-facing feature | grep across lynx/core/renderer/css/parser/css_string_parser.cc (no REPEATING hits) |
| Gradient color-stop list w/ positions | `<color> [<percentage>|<number>]?`, comma-separated | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:1719-1770 (ColorStopList) |
| `background-position` | Full 1-4 value keyword+length/percentage grammar incl. `<edge> <length>` offsets | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.h:304-316, lynx/core/renderer/css/parser/background_position_handler.cc |
| `background-size` | `<length-percentage>{1,2}` \| `cover` \| `contain` \| `auto` | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.h:317-318, lynx/core/renderer/css/parser/background_size_handler.cc |
| `background-repeat` | `repeat-x`/`repeat-y`/`[repeat\|no-repeat]{1,2}` | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.h:319-320, lynx/core/renderer/css/parser/background_repeat_handler.cc |
| `background-origin` | `border-box`/`padding-box`/`content-box` | Core | Yes | | lynx/core/renderer/starlight/style/css_type.h:275-279, lynx/core/renderer/css/parser/background_box_handler.cc |
| `background-clip` | `border-box`/`padding-box`/`content-box`/`text` | Core | Yes for standard values | | lynx/core/renderer/starlight/style/css_type.h:304-309 (kPaddingBox/kBorderBox/kContentBox/kText), lynx/core/renderer/css/parser/background_clip_handler.cc |
| `background-clip: border-area` | Lynx-only clip region (v3.6), distinct from border-box | Rare | No | Non-standard value. lynx-vello must special-case `BackgroundClipType::kBorderArea` (likely clips to the outer edge of border ± outline, needs behavioral spec-mining) rather than mapping to any W3C box | lynx/core/renderer/starlight/style/css_type.h:309 |
| `background-attachment` | Not found in parser or wire enum | — | No (absent) | Not supported — Lynx has no viewport-fixed background concept (scroll-view-based layout). Expected gap for web devs; document explicitly | grep across lynx/core/renderer/css/ and lynx-stack/.../css_property.rs (no hits) |
| Multiple/comma-separated background image layers | Supported via loop+comma in shorthand and in `background-image` alone | Core | Yes | See background shorthand FIXME above for the one real bug | lynx/core/renderer/css/parser/css_string_parser.cc:155-173, 200-206 |

#### Border (color / style / width / radius, per-corner) & Outline

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `border-color`/`border-{side}-color` | Per-side longhands + shorthand | Core | Yes | | lynx-stack/.../css_property.rs:257-260,308 |
| `border-width`/`border-{side}-width` incl. `thin`/`medium`/`thick` keywords (1/3/5px) | | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:2500-2511 |
| `border-style`/`border-{side}-style` | `solid`,`dashed`,`dotted`,`double`,`groove`,`ridge`,`inset`,`outset`,`hidden`,`none` | Core | Yes | | lynx/core/renderer/starlight/style/css_type.h:53-64, lynx/core/renderer/css/parser/border_style_handler.cc |
| `border-radius` shorthand + all 4 physical corners + 4 logical corners (`border-start-start-radius` etc.), full elliptical x/y per corner | | Core | Yes | | lynx/core/renderer/css/parser/border_radius_handler.cc:24-74 |
| `border-image*` | No handler, no property ID, no wire-format entry found anywhere | — | No (absent) | Not supported at all. Real gap vs. web-dev expectations (border-image is common for 9-slice UI). Flag for lynx-vello scope decision | grep across lynx/core/renderer/css/ (no hits), lynx-stack/.../css_property.rs (no hits) |
| `outline` shorthand + `outline-width`/`outline-color`/`outline-style` | Same BorderStyleType enum reused | Core | Yes for these 3 | | lynx/core/style/outline_data.h, lynx/core/renderer/css/parser/border_handler.cc:72-96 |
| `outline-offset` | No field in `OutLineData`, no property ID found | — | No (absent) | Real gap. Outlines always sit flush to the border edge; no offset control. Document explicitly | lynx/core/style/outline_data.h (no offset field), grep across core/renderer/css (no hits) |

#### Shadows, Filters, Clipping/Masking, Transforms

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `box-shadow` | Comma-separated list; offset-x, offset-y, blur-radius, spread-radius, color, `inset` keyword — full standard grammar | Core | Yes | | lynx/core/renderer/css/parser/shadow_handler.cc, lynx/core/renderer/css/parser/css_string_parser.cc:3327-3420, lynx/core/style/shadow_data.h |
| `text-shadow` | Same parser, `inset`/spread explicitly rejected (correctly, per spec text-shadow has no inset/spread) | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:3357-3361 |
| `filter` | Single function only: `grayscale()`, `blur()`, `brightness()`, `contrast()`, `saturate()`. `none` supported | Extended | No (partial) | Standard CSS `filter` accepts a **space-separated chain** of any number of filter functions (e.g. `filter: blur(2px) grayscale(50%)`); Lynx's parser hard-fails after the first function (`Check(TOKEN_EOF)` required immediately after one function value). Also missing `sepia()`, `invert()`, `opacity()`, `hue-rotate()` (enum value `FilterType::kHueRotate` exists but parser never emits/consumes it — dead enum member), `drop-shadow()`. lynx-vello: implement single-function W3C-legal subset now, chain-of-filters is a design decision (vello supports compositing multiple filter passes) | lynx/core/renderer/css/parser/filter_handler.cc, lynx/core/renderer/css/parser/css_string_parser.cc:3176-3253, lynx/core/renderer/starlight/style/css_type.h:369-377 (kHueRotate defined but unreachable) |
| `backdrop-filter` | Zero references anywhere (only an unrelated `::backdrop` pseudo-element selector exists) | — | No (absent) | Not supported. This is a meaningful visual gap (frosted-glass UI effects); vello can do this via a snapshot-then-filter compositing pass — worth flagging as a lynx-vello value-add beyond Lynx parity if desired | grep across lynx/core/renderer/css/ and lynx/core/style/ (only lynx/core/renderer/css/ng/selector/lynx_css_selector.h:29,109 for `::backdrop` pseudo-element, unrelated), lynx-stack/.../css_property.rs (no hits) |
| `clip-path` | `basic-shape` only: `circle()`, `ellipse()`, `inset()` (+ optional `round`), `path()` (SVG path-data string), plus Lynx-only `super-ellipse()`. No dispatch case for `polygon()` | Extended | Partial | Missing standard `polygon()` and reference clipping (`clip-path: url(#svg-clip-id)`); geometry-box keywords (`border-box` etc. as sole value) not observed as a `BasicShape()` case either. lynx-vello should add `polygon()` at minimum since it's common; SVG `url()` reference needs an SVG subsystem decision | lynx/core/renderer/css/parser/clip_path_handler.cc, lynx/core/renderer/css/parser/css_string_parser.cc:744-759 (BasicShape dispatch: CIRCLE/ELLIPSE/PATH/SUPER_ELLIPSE/INSET only) |
| `super-ellipse()` clip/inset-round shape | `super-ellipse(rx ry)` — Lynx-only extension for squircle-style corners | Rare | No | Non-standard (closest analog: unshipped CSS `corner-shape`/Houdini superellipse proposals). lynx-vello must implement this bespoke shape math explicitly if targeting full compatibility | lynx/core/renderer/starlight/style/css_type.h:384 (kSuperEllipse), lynx/core/renderer/css/parser/css_string_parser.cc:3036-3057, lynx/core/renderer/css/parser/clip_path_handler_unittest.cc:134 |
| `offset-path` | Reuses the same `BasicShape()`/`ParseShapePath()` grammar as clip-path | Extended | Partial | Same polygon/url gaps as clip-path | lynx/core/renderer/css/parser/clip_path_handler.cc:30, lynx/core/renderer/css/parser/css_string_parser.cc:681-688 |
| `mask` shorthand, `mask-image`, `mask-position`, `mask-size`, `mask-repeat`, `mask-origin`, `mask-clip`, `mask-composite` | Mirrors the `background` shorthand machinery (`ParseBackgroundOrMask(mask=true)`); `mask-image` literally shares the background-image handler | Core | Yes | | lynx/core/renderer/css/parser/mask_shorthand_handler.cc, lynx/core/renderer/css/parser/background_image_handler.cc:32, lynx-stack/.../css_property.rs:418,447-452 |
| `mask-composite` values | `add`/`subtract`/`intersect`/`exclude` | Core | Yes | Matches W3C `<compositing-operator>` exactly | lynx/core/renderer/starlight/style/css_type.h:312-317 |
| `transform` functions | `translate`/`translateX/Y/Z`/`translate3d`, `rotate`/`rotateX/Y/Z`, `scale`/`scaleX/Y`, `skew`/`skewX/Y`, `matrix`, `matrix3d` | Core | Partial | Missing `rotate3d()` and `scale3d()` (both real, commonly-used CSS Transforms functions) — no dispatch cases in `ParseTransformParams`. Also no `perspective()` **function** form (only the standalone `perspective` property exists) | lynx/core/renderer/starlight/style/css_type.h:173-192, lynx/core/style/transform_raw_data.h, lynx/core/renderer/css/parser/css_string_parser.cc:3752-3845 |
| `transform` angle/number legacy compatibility | Bare unitless numbers accepted as angle/length in legacy mode (`enable_transform_legacy_`); `translate(x,y,z-as-3rd-arg)` treated as translate3d-like tolerance | Extended | No (permissive superset) | Non-standard leniency vs strict CSSOM grammar; lynx-vello parser should accept these compatibility forms to match real-world `.web.bundle` content but should not require them | lynx/core/renderer/css/parser/css_string_parser.cc:1816-1829 (AngleValue), :3803-3813 |
| `transform-origin` | Standard keyword/length/percentage 1-3 value form | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:3442 (ParseTransformOrigin), lynx/core/style/transform_origin_data.h |
| `perspective` (property) | Standalone property (X-macro `V(Perspective)` generated; not directly grep-able as `kPropertyIDPerspective` in this checkout — likely codegen'd from a schema not present in source tree) | Core | Could not fully confirm parser wiring | Data structure (`PerspectiveData`) and computed-style accessor exist; wire-format ID `Perspective = 190` present. Could not locate the string-value parser/handler file in this checkout — treat as needing direct verification against a build output or newer lynx checkout before implementing | lynx/core/renderer/css/computed_css_style.h:462,605,779, lynx/core/style/perspective_data.h, lynx-stack/.../css_property.rs:206,438 |
| `perspective-origin` | Not found in wire-format enum or parser | — | Could not confirm | No positive evidence found either way in the files opened; flag as unconfirmed rather than asserting absence | grep across lynx-stack/.../css_property.rs (no "PerspectiveOrigin" hit) |

#### Text-paint adjacent (bordering typography, included since paint-relevant)

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `text-stroke` shorthand → `text-stroke-width` + `text-stroke-color` | No separate "style" (matches browsers' own `-webkit-text-stroke`, which also has no style axis) | Extended | Partial (non-standard property name; behavior matches WebKit prefix convention) | `text-stroke` (unprefixed) isn't itself a W3C property (`-webkit-text-stroke` is a de facto standard, not in a W3C spec); treat as intentional convergence with real-world web behavior, not a Lynx quirk to "fix" | lynx/core/renderer/css/parser/text_stroke_handler.cc:30-37, lynx-stack/.../css_property.rs:442-444 |
| `text-decoration-line`/`style` incl. `wavy` | | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:2345-2366, 322-325 |

#### Absent properties confirmed by direct search (no source hits in either repo)

| Item | Tier | Notes |
|---|---|---|
| `backdrop-filter` | — | No handler, no property ID, no wire enum entry |
| `mix-blend-mode` | — | No handler, no property ID, no wire enum entry |
| `background-blend-mode` | — | No handler, no property ID, no wire enum entry |
| `border-image` (+ `-source/-slice/-width/-outset/-repeat`) | — | No handler, no property ID, no wire enum entry |
| `background-attachment` | — | No handler, no property ID, no wire enum entry |
| `outline-offset` | — | No struct field, no property ID |
| `repeating-linear/radial/conic-gradient()` | — | Token types not defined; only plain gradient variants exist |
| `clip-path: polygon()` / `url()` | — | No dispatch case in `BasicShape()`; SVG reference clipping absent |
| `filter: hue-rotate()/sepia()/invert()/opacity()/drop-shadow()`, and filter chaining | — | `FilterType::kHueRotate` enum value is dead code; parser hard-stops after one function |
| `transform: rotate3d()/scale3d()/perspective()` (function form) | — | No dispatch cases in `ParseTransformParams` |

---

## Also see

Scope note: this is the spec for what the `stylo`-backed style engine must resolve and what the `vello`-backed renderer must paint — see `.claude/agents/lynx-css-engine.md` and `.claude/agents/lynx-render-engine.md`.

Implementation-pattern reference (not a behavior spec): `Paws/engine/src/style.rs` and `Paws/engine/src/style/css_style_sheet.rs` show a working `stylo` cascade integration over a custom Rust DOM.
