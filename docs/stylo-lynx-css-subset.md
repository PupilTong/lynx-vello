# Stylo `lynx` CSS subset

> Decision record, 2026-07-16. This is the exact compile-time CSS surface of
> `vendor/stylo` when `feature = "lynx"` is enabled. The official Lynx
> [property index](https://lynxjs.org/next/api/css/properties/) and its
> [Markdown source](https://lynxjs.org/next/api/css/properties.md) are the
> author-surface seed. This document records every property/value grammar that
> intentionally differs from upstream Stylo; properties not listed in the
> value table keep their upstream parser.

The revision-pinned, author-spelling-complete comparison against upstream
Servo—including all 23 Lynx-only names, all 229 upstream-only names, and the
160/51 changed/unchanged partition of common names—is tracked in
[`tracking/stylo-lynx-vs-upstream-properties.md`](tracking/stylo-lynx-vs-upstream-properties.md).

## Generation rules

| Input/design rule | Generated behavior |
|---|---|
| A seed longhand is supported | Every shorthand containing it is enabled with the complete upstream shorthand grammar; all other component longhands of those shorthands are enabled too. |
| A seed shorthand is supported | Every component longhand is enabled. The closure is repeated to a fixed point across overlapping shorthands. |
| Unsupported property | Its spelling is absent from the generated author property-name table. This is compile-time code generation, not a runtime allowlist or a post-parse filter. |
| `feature = "lynx"` is off | Upstream Servo property names, values, initial values, and declaration sizes are retained; parity tests pin representative behavior. |
| Initial values | Stylo keeps upstream W3C initial values. Lynx defaults belong to the UA-origin stylesheet owned by `stylo-dom::Document`; no `lynx_initial` mechanism exists and `lynx-widget` does not calculate them. |

The generated Lynx author surface currently contains 234 non-custom property
spellings: 179 seed names plus these 55 names brought in by
shorthand/longhand closure:

| Closure-added property spellings |
|---|
| `alignment-baseline`, `baseline-shift`, `baseline-source` |
| `animation-range-end`, `animation-range-start`, `animation-timeline` |
| `background-attachment`, `background-position-x`, `background-position-y` |
| `border-image`, `border-image-outset`, `border-image-repeat`, `border-image-slice`, `border-image-source`, `border-image-width` |
| `border-inline`, `border-inline-color`, `border-inline-end`, `border-inline-start`, `border-inline-style`, `border-inline-width` |
| `font`, `font-kerning`, `font-stretch`, `font-variant`, `font-variant-caps`, `font-variant-east-asian`, `font-variant-ligatures`, `font-variant-numeric`, `font-variant-position` |
| `grid`, `grid-area`, `grid-template`, `grid-template-areas` |
| `inset`, `inset-inline`, `margin-inline`, `padding-inline` |
| `mask-clip`, `mask-mode`, `mask-origin`, `mask-position`, `mask-position-x`, `mask-position-y`, `mask-repeat`, `mask-size` |
| `place-content`, `place-items`, `place-self` |
| `text-decoration-color`, `text-decoration-line`, `text-decoration-style`, `text-wrap-mode`, `white-space-collapse` |
| `transition-behavior` |

The following documented spellings are deliberately omitted from that
source: every `-x-*` property; `linear-cross-gravity`, `linear-gravity`, and
`linear-layout-gravity`. `ppx` and `sp` are likewise absent. The canonical
Stylo `-webkit-text-stroke*` implementation names stay private; only Lynx's
unprefixed `text-stroke*` aliases are authorable. Custom properties (`--*`)
remain supported through Stylo's separate custom-property path.

## Property/value behavior

In the table, “reject” means the entire declaration is invalid and therefore
does not enter the cascade, like an unsupported browser CSS value.

| Property | Accepted with `lynx` | Rejected/changed behavior |
|---|---|---|
| Every non-custom property | Its property-specific grammar, `var()` substitutions, and standard CSS-wide values `inherit`, `initial`, `unset`, `revert`, and `revert-layer` | No Lynx-specific cascade-keyword path. |
| Any length-bearing supported property | `0`; `px`, `rpx`, `em`, `rem`, `vw`, `vh`; percentages and typed `calc()` where that property's grammar permits them | No `ppx`, `sp`, absolute units, `ex`/`ch` families, `lh` families, `vmin`/`vmax`, dynamic/small/large viewport units, logical viewport units, or container-query units. `rpx` is unavailable in context-free descriptors such as `@font-face`. |
| Any angle-bearing supported property | `deg`, `rad`, `turn`, and typed `calc()` where permitted | `grad` is absent. |
| `display` | `none`, `flex`, `grid`, `linear`, `relative` | No `block`, `inline`, compound display syntax, table/list values, `contents`, or `flow-root`. In particular, `display:block` is ignored as invalid. |
| `direction` | `ltr`, `rtl` | `normal` and `lynx-rtl` do not exist. This uses Stylo's W3C direction type and semantics. |
| `overflow`, `overflow-x`, `overflow-y` | `visible`, `hidden` | No `scroll`, `auto`, `clip`, or `overlay`; scrolling is widget policy rather than a CSS overflow value. |
| `position` | `relative`, `absolute`, `fixed`, `sticky` | No `static`. |
| `visibility` | `visible`, `hidden` | No `collapse`. |
| `aspect-ratio` | `<non-negative-ratio>`, e.g. `1` or `16 / 9` | The `auto` arm is internal-only. |
| `width`, `height` | Stylo's upstream W3C sizing grammar, including `auto`, intrinsic sizing keywords, and enabled sizing functions | No Lynx-specific value parser; the global Lynx unit subset still applies. |
| `flex-basis` | Stylo's upstream `content | <'width'>` grammar, including intrinsic sizing keywords | No Lynx-specific value parser or additional restriction. |
| `min-width`, `min-height` | Stylo's upstream W3C sizing grammar, including `auto` and intrinsic sizing keywords | No Lynx-specific value parser; negative sizes remain invalid per CSS. |
| `max-width`, `max-height` | Non-negative `<length-percentage>` | No author `none`, intrinsic sizing keyword, or negative value; unbounded `none` remains the upstream internal initial value. |
| `gap`, `row-gap`, `column-gap` and grid aliases | Stylo's upstream W3C grammar: `normal` or a non-negative `<length-percentage>` | The UA sheet may still supply Lynx's zero-gap default. |
| `z-index` | Stylo's upstream W3C grammar: `auto` or `<integer>` | No Lynx-specific value parser. |
| `perspective` | Stylo's upstream W3C grammar: `none` or a non-negative `<length>` | No Lynx-only `auto` spelling. |
| `will-change` | Stylo's upstream W3C grammar, including `auto`, `scroll-position`, `contents`, and `<custom-ident>` property names | Stylo parses, cascades, and computes the hint. Consumption by the future style-to-layout/paint adapter is still pending. |
| `font-size` | Stylo's upstream W3C grammar, including length/percentage and absolute/relative size keywords | No Lynx-specific value parser. The 14px Lynx default comes from the UA sheet. |
| `font-style` property | Stylo's upstream W3C grammar, including `oblique <angle>` | No Lynx-specific value parser. |
| `font-weight` property | Stylo's upstream W3C grammar: `normal`, `bold`, `bolder`, `lighter`, or a number in `[1, 1000]` | Fractional values in range remain valid; no Lynx-specific min/max or integer-only constants are introduced. |
| `line-height` | Stylo's upstream W3C grammar: `normal`, non-negative `<number>`, or non-negative `<length-percentage>` | No Lynx-specific value parser. |
| `letter-spacing` | `<length>` | No `normal` or percentage. |
| `align-content` | `stretch`, `start`, `end`, `flex-start`, `flex-end`, `center`, `space-between`, `space-around` | No `normal`, baseline forms, safe/unsafe, left/right, or `space-evenly`. |
| `justify-content` | The `align-content` set plus `space-evenly` | No `normal`, baseline forms, safe/unsafe, left/right. |
| `align-self` | `auto`, `stretch`, `center`, `start`, `end`, `flex-start`, `flex-end`, `baseline` | No `normal`, self-start/end, last/first baseline, or safe/unsafe. |
| `justify-self` | `auto`, `stretch`, `center`, `start`, `end` | No flex/baseline, left/right, self-start/end, or safe/unsafe values. |
| `align-items` | `stretch`, `center`, `start`, `end`, `flex-start`, `flex-end`, `baseline` | No non-standard `auto`; no `normal`, self-start/end, last/first baseline, or safe/unsafe. |
| `justify-items` | `stretch`, `center`, `start`, `end` | No `normal`, `auto`, flex/baseline, legacy, left/right, or safe/unsafe values. The Lynx default `stretch` is a UA declaration. |
| `text-align` | `start`, `end`, `left`, `right`, `center` | No `justify`, `match-parent`, or browser-prefixed aliases. |
| `text-overflow` | One `clip` or `ellipsis` value | No two-value form or custom string. |
| `text-decoration-line` and `text-decoration` component | Exactly one of `none`, `underline`, `line-through` | No `overline`, `blink`, or combined line flags. The full shorthand still handles all of its enabled components. |
| `text-stroke-width` / `text-stroke` width component | Stylo's upstream `<line-width>` grammar: `thin`, `medium`, `thick`, or a non-negative `<length>` | There is no `parse_lynx_text_stroke_width`; only unprefixed Lynx property names are public. |
| `background-position` and its longhands | Stylo's complete upstream position grammar | There is no `parse_lynx_background_position`; shorthand parsing and expansion remain upstream. |
| `color` | Named/hex colors; `rgb()`/`rgba()`; `hsl()`/`hsla()`; non-repeating `linear-gradient()`, `radial-gradient()`, `conic-gradient()` | `color` may compute to a solid color or retained text gradient. No authored `currentcolor`, system color, relative color, `hwb`/Lab/LCH/color-space functions, `color-mix`, `light-dark`, contrast color, prefixed or repeating gradient. |
| Image-valued properties (`background-image`, `mask-image`, `border-image-source`, etc.) | `none`, `url()`, or the three non-repeating modern gradients | No `image-set`, `cross-fade`, `image()`, `light-dark`, paint worklet, browser-prefixed image, or repeating/prefixed gradient. |
| Gradient stops | At least two color stops; each stop has no position or one literal number/percentage fraction; linear/radial/conic use their corresponding position type | No interpolation hint, two-position stop, length stop, advanced interpolation color space, or single-stop gradient. |
| `background-clip` | `border-box`, `padding-box`, `content-box`, `border-area` | `border-area` is compile-time available rather than pref-gated. |
| `filter` | `none`; chains of `blur()`, `brightness()`, `contrast()`, `grayscale()`, `saturate()` | No `sepia`, `invert`, `opacity`, `hue-rotate`, `drop-shadow`, or URL filter. |
| `box-shadow` | Stylo's upstream box-shadow structure, including `inset`, spread radius, and comma-separated layers | No Lynx-specific box-shadow parser; the global Lynx length and color subsets still apply to its components. |
| `text-shadow` | Exactly one `offset-x offset-y blur color` shadow, in that order | No color-first form, omitted blur, or comma-separated list. |
| `clip-path` | `none` or one of `inset()`, `circle()`, `ellipse()`, `path()` | No `polygon`, URL, geometry-box-only, `rect`, `xywh`, or `shape`. |
| `offset-path` | One of `inset()`, `circle()`, `ellipse()`, `path()` | No author `none`, `ray`, URL, polygon, coordinate box, or path position. |
| `offset-distance` | Literal number `0..=1` or percentage `0%..=100%` | No length, out-of-range value, or `calc()`. Numbers compute as the corresponding path percentage. |
| `offset-rotate` | `auto` or one angle resolving in `0deg..=360deg` | No `reverse`, combined `auto <angle>`, negative/out-of-range angle, or unresolved `calc()`. |
| `transform` | `none`; `matrix`/`matrix3d`; translate 2D/3D; scale/scaleX/scaleY with number factors; rotate/rotateX/Y/Z; skew/skewX/Y | No percentage scale factors, `scaleZ`, `scale3d`, `rotate3d`, or `perspective()` function. |
| `transform-origin` | One- or two-dimensional origin | The third depth component rejects. |
| `cursor` | A keyword cursor such as `auto`, `pointer`, `grab`, `zoom-in` | No URL cursor list or Mozilla-prefixed aliases. |
| `animation-timing-function`, `transition-timing-function` | Standard keywords, `steps()`, `cubic-bezier()`, plus Lynx `square-bezier(x, y)` | No CSS `linear()` function. UA defaults both properties to keyword `linear`. |
| `animation-duration` | Non-negative `<time>` | No `auto` or unitless time. |
| Grid line properties | Stylo's upstream `<grid-line>` grammar, including named lines and named spans | No Lynx-specific `GridLine` parser; standard invalid forms such as line `0` and a negative span still reject. |
| Grid templates and track lists | Stylo's upstream Grid grammar, including `none`, intrinsic tracks, named lines, fixed/auto repeats, `minmax()`, and `fit-content()` | No Lynx-specific grid value types or parser restrictions. |
| `linear-direction` | Exactly `row`, `row-reverse`, `column`, `column-reverse` | No `normal`, `horizontal`, `vertical`, or legacy reverse spellings. |
| `linear-weight`, `linear-weight-sum` | Non-negative `<number>` | Negative values reject. The three gravity properties remain absent. |
| `relative-id` | Positive integer | No `none`, zero, or negative integer. |
| `relative-align-{top,right,bottom,left,inline-start,inline-end}` | `none`, `parent`, or positive integer | Zero and negative integer reject. `none` computes to internal sentinel `-1`; `parent` computes to `0`. |
| `relative-{top,right,bottom,left,inline-start,inline-end}-of` | `none` or positive integer | `parent`, zero, and negative integer reject; `none` computes to `-1`. |
| `relative-center` | `none`, `vertical`, `horizontal`, `both` | Other values reject. |
| `relative-layout-once` | `true`, `false` | Other values reject. |

## Complete shorthand rule

Every enabled shorthand uses its normal upstream parser and expands all of its
upstream component longhands. This applies to `background-position`,
`vertical-align`, `white-space`, `background`, `border*`, `border-image`,
`font`, `grid*`, `mask`, `place-*`, `text-decoration`, `transition`, and
`animation`. For example, edge-offset `background-position`, `white-space:
pre-wrap`, a complete `border-image`, and the two-value `place-items` syntax
all parse. A shorthand's structural grammar is complete, while any component
whose value grammar is explicitly narrowed in the table above keeps that
narrowed component grammar.

## UA defaults

These are not Stylo type/parser changes and are not implemented by this
Stylo-subset patch. They belong to the UA-origin stylesheet owned and computed
by `stylo-dom::Document`, without `!important`, so author declarations override
them normally. A PAPI caller may provide configuration data, but it does not
generate or calculate these styles:

| Target | UA declarations / policy |
|---|---|
| Built-in elements | Configurable `display: linear` or `display: flex`; `box-sizing: border-box`; zero-width solid borders; `position: relative`; `min-width: 0`; `min-height: 0`; `row-gap: 0`; `column-gap: 0`; stretch content/item/justification defaults; linear animation/transition timing; configurable `overflow: hidden` or `visible`. |
| `page` root | `font-size: 14px; font-optical-sizing: none` so the inherited default is established once and ordinary author inheritance still works. |
| `text`, `image`, `raw-text`, `scroll-view` | Component-specific display and scroll-axis declarations layered over the common base; scrolling itself remains widget policy. |
