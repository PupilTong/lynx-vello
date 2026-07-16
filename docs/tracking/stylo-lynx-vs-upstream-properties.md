# Stylo `lynx` vs upstream Servo property inventory

> Snapshot: 2026-07-16. This file compares `vendor/stylo` with
> `feature = "lynx"` at `ac7e1c68e88c1b932b4eded5a4048d157a99eacf`
> against `upstream/main` at
> `219f722e8be7feceacd7a870e93b39f6c6d22b7c`.

This is the exhaustive author-property comparison for the pinned revisions.
It tracks three independent kinds of difference:

1. a property spelling exists on only one side;
2. the same spelling is disabled by Servo's default static prefs but forced on
   by `lynx`;
3. the same spelling exists on both sides but its accepted value grammar,
   computed representation, animation behavior, or property-reference
   semantics differ.

## Scope and extraction method

The spelling sets come from the generated `PropertyId` static lookup table,
specifically entries mapping to `StaticId::NonCustom`. This is the table used
by author declarations and CSSOM `setProperty`. The generated
`css-properties.json` is not sufficient for this audit: it omits an exposed
alias when its canonical implementation name is hidden, including Lynx's
`text-stroke*` aliases.

The value comparison is a source audit of every `#[cfg(feature = "lynx")]`,
every `lynx_*` property-generator override, and every Lynx-only value type in
`style/`. Shorthand effects are propagated to their authorable shorthands.

The counts exclude custom properties (`--*`), which remain accepted on both
sides through Stylo's separate custom-property path. `var()` substitution and
the CSS-wide values `inherit`, `initial`, `unset`, `revert`, and
`revert-layer` also remain supported for every generated property. The `all`
*property* is still a normal property spelling for this inventory and is
currently upstream-only.

| Set | Count |
|---|---:|
| Lynx author spellings | 234 |
| Upstream Servo author spellings | 440 |
| Spellings common to both | 212 |
| Lynx-only spellings | 22 |
| Upstream-only spellings | 228 |
| Common spellings with a known behavior difference | 160 |
| Common spellings audited as behaviorally unchanged | 52 |

The accounting is closed: `22 + 228 + 212 = 462` spellings in the union, and
the 212 common spellings split into `160 + 52`.

## Lynx-only spellings (22)

These names are absent from the pinned upstream Servo author table.

| Property spelling(s) | Current `lynx` grammar / behavior |
|---|---|
| `linear-direction` | `column | row | column-reverse | row-reverse`; initial `column`. |
| `linear-weight`, `linear-weight-sum` | Non-negative `<number>`; initial `0`. |
| `relative-id` | Positive integer; internal initial sentinel `-1` is not authorable. |
| `relative-align-top`, `relative-align-right`, `relative-align-bottom`, `relative-align-left`, `relative-align-inline-start`, `relative-align-inline-end` | `none | parent | <positive-integer>`; computed values are `-1`, `0`, or the id. |
| `relative-top-of`, `relative-right-of`, `relative-bottom-of`, `relative-left-of`, `relative-inline-start-of`, `relative-inline-end-of` | `none | <positive-integer>`; `none` computes to `-1`. |
| `relative-center` | `none | vertical | horizontal | both`; initial `none`. |
| `relative-layout-once` | `true | false`; current generated initial is `true`. |
| `offset-rotate` | `auto` or one angle that can resolve at parse time into `[0deg, 360deg]`; no `reverse`, combined form, context-dependent `calc()`, or `grad`. |
| `text-stroke-color` | Lynx's restricted `<color>` grammar. The canonical `-webkit-text-stroke-color` implementation name is hidden. |
| `text-stroke-width` | `thin | medium | thick | <non-negative-length>` using the Lynx length-unit set; initial `0px`. The canonical prefixed name is hidden. |
| `text-stroke` | Unprefixed alias for the width/color shorthand; either component may appear in either order and omitted components use their initial values. The canonical prefixed name is hidden. |

Of these, the 18 `linear-*`/`relative-*` names are Lynx-only extensions.
`offset-rotate` is a standard motion-path property that upstream Servo does
not expose, while `text-stroke*` uses Lynx's unprefixed spelling over Stylo's
Gecko-only compatibility implementation. `offset-distance` follows upstream
Servo and is therefore absent from both author tables.

## Upstream-only spellings (228)

Every spelling below exists in the pinned upstream Servo generated author
table and is absent from the `lynx` generated table. Under `lynx`, declaration
lookup fails before value parsing. This list intentionally includes aliases,
prefixed spellings, and upstream internal-looking spellings because all of
them are entries in the upstream author lookup table.

```text
-moz-animation
-moz-animation-delay
-moz-animation-direction
-moz-animation-duration
-moz-animation-fill-mode
-moz-animation-iteration-count
-moz-animation-name
-moz-animation-play-state
-moz-animation-timing-function
-moz-appearance
-moz-backface-visibility
-moz-border-image
-moz-box-sizing
-moz-default-appearance
-moz-font-feature-settings
-moz-font-language-override
-moz-min-font-size-ratio
-moz-perspective
-moz-perspective-origin
-moz-transform
-moz-transform-origin
-moz-transform-style
-moz-transition
-moz-transition-delay
-moz-transition-duration
-moz-transition-property
-moz-transition-timing-function
-moz-user-select
-servo-top-layer
-webkit-align-content
-webkit-align-items
-webkit-align-self
-webkit-animation
-webkit-animation-delay
-webkit-animation-direction
-webkit-animation-duration
-webkit-animation-fill-mode
-webkit-animation-iteration-count
-webkit-animation-name
-webkit-animation-play-state
-webkit-animation-timing-function
-webkit-appearance
-webkit-backface-visibility
-webkit-background-clip
-webkit-background-origin
-webkit-background-size
-webkit-border-bottom-left-radius
-webkit-border-bottom-right-radius
-webkit-border-image
-webkit-border-radius
-webkit-border-top-left-radius
-webkit-border-top-right-radius
-webkit-box-shadow
-webkit-box-sizing
-webkit-clip-path
-webkit-filter
-webkit-flex
-webkit-flex-basis
-webkit-flex-direction
-webkit-flex-flow
-webkit-flex-grow
-webkit-flex-shrink
-webkit-flex-wrap
-webkit-font-feature-settings
-webkit-justify-content
-webkit-mask
-webkit-mask-clip
-webkit-mask-composite
-webkit-mask-image
-webkit-mask-origin
-webkit-mask-position
-webkit-mask-position-x
-webkit-mask-position-y
-webkit-mask-repeat
-webkit-mask-size
-webkit-order
-webkit-perspective
-webkit-perspective-origin
-webkit-text-security
-webkit-transform
-webkit-transform-origin
-webkit-transform-style
-webkit-transition
-webkit-transition-delay
-webkit-transition-duration
-webkit-transition-property
-webkit-transition-timing-function
-webkit-user-select
-x-lang
-x-text-scale
all
animation-composition
appearance
backdrop-filter
backface-visibility
background-blend-mode
block-size
border-block
border-block-color
border-block-end
border-block-end-color
border-block-end-style
border-block-end-width
border-block-start
border-block-start-color
border-block-start-style
border-block-start-width
border-block-style
border-block-width
border-collapse
border-spacing
box-decoration-break
caption-side
caret-color
clear
clip
color-scheme
column-count
column-span
column-width
columns
container-name
container-type
content
corner-block-end-shape
corner-block-start-shape
corner-bottom-left-shape
corner-bottom-right-shape
corner-bottom-shape
corner-end-end-shape
corner-end-start-shape
corner-inline-end-shape
corner-inline-start-shape
corner-left-shape
corner-right-shape
corner-shape
corner-start-end-shape
corner-start-start-shape
corner-top-left-shape
corner-top-right-shape
corner-top-shape
counter-increment
counter-reset
empty-cells
fill
fill-opacity
fill-rule
float
font-language-override
font-size-adjust
font-synthesis-weight
forced-color-adjust
grid-gap
inline-size
inset-block
inset-block-end
inset-block-start
isolation
line-break
list-style
list-style-image
list-style-position
list-style-type
margin-block
margin-block-end
margin-block-start
mask-type
math-depth
math-style
max-block-size
max-inline-size
min-block-size
min-inline-size
mix-blend-mode
object-fit
object-position
outline
outline-color
outline-offset
outline-style
outline-width
overflow-block
overflow-clip-margin
overflow-inline
overflow-wrap
overscroll-behavior
overscroll-behavior-block
overscroll-behavior-inline
overscroll-behavior-x
overscroll-behavior-y
padding-block
padding-block-end
padding-block-start
perspective-origin
position-area
position-try-fallbacks
quotes
rotate
scale
scroll-behavior
scrollbar-color
scrollbar-width
stroke
stroke-dasharray
stroke-dashoffset
stroke-linecap
stroke-linejoin
stroke-miterlimit
stroke-opacity
stroke-width
tab-size
table-layout
text-align-last
text-justify
text-orientation
text-rendering
text-transform
touch-action
transform-style
translate
unicode-bidi
user-select
view-transition-class
view-transition-name
word-spacing
word-wrap
writing-mode
zoom
```

## Common spellings with different behavior (160)

This is the canonical, de-duplicated set. The detailed mechanism tables below
overlap because one property can contain lengths, colors, and gradients while
also having a property-specific parser change.

```text
align-content
align-items
align-self
animation
animation-duration
animation-range-end
animation-range-start
animation-timeline
animation-timing-function
background
background-clip
background-color
background-image
background-position
background-position-x
background-position-y
background-size
baseline-shift
border
border-bottom
border-bottom-color
border-bottom-left-radius
border-bottom-right-radius
border-bottom-width
border-color
border-end-end-radius
border-end-start-radius
border-image
border-image-outset
border-image-source
border-image-width
border-inline
border-inline-color
border-inline-end
border-inline-end-color
border-inline-end-width
border-inline-start
border-inline-start-color
border-inline-start-width
border-inline-width
border-left
border-left-color
border-left-width
border-radius
border-right
border-right-color
border-right-width
border-start-end-radius
border-start-start-radius
border-top
border-top-color
border-top-left-radius
border-top-right-radius
border-top-width
border-width
bottom
box-shadow
clip-path
color
column-gap
contain
cursor
display
filter
flex
flex-basis
font
font-optical-sizing
font-size
font-style
font-variation-settings
gap
grid
grid-area
grid-auto-columns
grid-auto-flow
grid-auto-rows
grid-column
grid-column-end
grid-column-gap
grid-column-start
grid-row
grid-row-end
grid-row-gap
grid-row-start
grid-template
grid-template-areas
grid-template-columns
grid-template-rows
height
image-rendering
inset
inset-inline
inset-inline-end
inset-inline-start
justify-content
justify-items
justify-self
left
letter-spacing
line-height
margin
margin-bottom
margin-inline
margin-inline-end
margin-inline-start
margin-left
margin-right
margin-top
mask
mask-clip
mask-composite
mask-image
mask-mode
mask-origin
mask-position
mask-position-x
mask-position-y
mask-repeat
mask-size
max-height
max-width
min-height
min-width
offset-path
overflow
overflow-x
overflow-y
padding
padding-bottom
padding-inline
padding-inline-end
padding-inline-start
padding-left
padding-right
padding-top
perspective
place-content
place-items
place-self
position
right
row-gap
text-align
text-decoration
text-decoration-color
text-decoration-line
text-indent
text-overflow
text-shadow
top
transform
transform-origin
transition
transition-property
transition-timing-function
vertical-align
visibility
width
will-change
```

### Default availability differences (34)

All three upstream Servo static prefs below default to `false`. Upstream keeps
the names in its generated table but `parse_enabled_for_all_content` rejects
them until the pref is enabled. The `lynx` build force-enables the selected
properties at compile time.

| Upstream pref | Common property spellings forced on by `lynx` |
|---|---|
| `layout.grid.enabled` | `grid`, `grid-area`, `grid-auto-columns`, `grid-auto-flow`, `grid-auto-rows`, `grid-column`, `grid-column-end`, `grid-column-start`, `grid-row`, `grid-row-end`, `grid-row-start`, `grid-template`, `grid-template-areas`, `grid-template-columns`, `grid-template-rows` |
| `layout.unimplemented` | `animation-range-end`, `animation-range-start`, `animation-timeline`, `contain`, `mask`, `mask-clip`, `mask-composite`, `mask-image`, `mask-mode`, `mask-origin`, `mask-position`, `mask-position-x`, `mask-position-y`, `mask-repeat`, `mask-size`, `offset-path`, `text-overflow` |
| `layout.variable_fonts.enabled` | `font-optical-sizing`, `font-variation-settings` |

`display: grid` is also accepted unconditionally by `lynx`; upstream Servo
gates that value on `layout.grid.enabled`.

### Shared primitive grammar changes

These changes occur below individual property parsers and therefore propagate
through every listed longhand and shorthand.

#### Length-bearing properties (99)

Lynx accepts unitless zero and the units `px`, `rpx`, `em`, `rem`, `vw`, and
`vh`; percentages and typed `calc()` remain available wherever the enclosing
property permits them. Servo does not have `rpx`. Lynx removes Servo's
`in`/`cm`/`mm`/`q`/`pt`/`pc`, extended font-relative units, `lh` families,
`vmin`/`vmax`, small/large/dynamic and logical viewport units, and
container-query units. `rpx` computes as viewport width divided by 750.

Affected spellings:

`animation`, `animation-range-end`, `animation-range-start`, `background`,
`background-position`, `background-position-x`, `background-position-y`,
`background-size`, `baseline-shift`, `border`, `border-bottom`,
`border-bottom-left-radius`, `border-bottom-right-radius`,
`border-bottom-width`, `border-end-end-radius`, `border-end-start-radius`,
`border-image`, `border-image-outset`, `border-image-width`, `border-inline`,
`border-inline-end`, `border-inline-end-width`, `border-inline-start`,
`border-inline-start-width`, `border-inline-width`, `border-left`,
`border-left-width`, `border-radius`, `border-right`, `border-right-width`,
`border-start-end-radius`, `border-start-start-radius`, `border-top`,
`border-top-left-radius`, `border-top-right-radius`, `border-top-width`,
`border-width`, `bottom`, `box-shadow`, `clip-path`, `column-gap`, `filter`,
`flex`, `flex-basis`, `font`, `font-size`, `gap`, `grid`,
`grid-auto-columns`, `grid-auto-rows`, `grid-column-gap`, `grid-row-gap`,
`grid-template`, `grid-template-columns`, `grid-template-rows`, `height`,
`inset`, `inset-inline`, `inset-inline-end`, `inset-inline-start`, `left`,
`letter-spacing`, `line-height`, `margin`, `margin-bottom`, `margin-inline`,
`margin-inline-end`, `margin-inline-start`, `margin-left`, `margin-right`,
`margin-top`, `mask`, `mask-position`, `mask-position-x`, `mask-position-y`,
`mask-size`, `max-height`, `max-width`, `min-height`, `min-width`,
`offset-path`, `padding`, `padding-bottom`, `padding-inline`,
`padding-inline-end`, `padding-inline-start`, `padding-left`, `padding-right`,
`padding-top`, `perspective`, `right`, `row-gap`, `text-indent`,
`text-shadow`, `top`, `transform`, `transform-origin`, `vertical-align`,
`width`.

Gradient stop positions are deliberately not accounted through this generic
length list: Lynx gives gradients a separate literal number/percentage stop
grammar described below.

#### Angle-bearing properties (10)

Lynx accepts `deg`, `rad`, and `turn`, including typed `calc()` where the
enclosing grammar allows it, but removes Servo's `grad` unit.

Affected spellings: `background`, `background-image`, `border-image`,
`border-image-source`, `color`, `font`, `font-style`, `mask`, `mask-image`,
`transform`.

#### Color-bearing properties (28)

Lynx keeps named and hex colors, `transparent`, `rgb()`/`rgba()`, and
`hsl()`/`hsla()`. Compared with upstream Servo it removes authored
`currentcolor`, `hwb()`, Lab/LCH/OKLab/OKLCH, `color()`, `color-mix()`,
`light-dark()`, relative-color `from` syntax, and pref-gated `alpha()` and
`contrast-color()` forms. Upstream Servo does not expose Gecko system colors,
so system colors are not counted as a Lynx-vs-Servo difference here.

Affected spellings: `background`, `background-color`, `background-image`,
`border`, `border-bottom`, `border-bottom-color`, `border-color`,
`border-image`, `border-image-source`, `border-inline`, `border-inline-color`,
`border-inline-end`, `border-inline-end-color`, `border-inline-start`,
`border-inline-start-color`, `border-left`, `border-left-color`,
`border-right`, `border-right-color`, `border-top`, `border-top-color`,
`box-shadow`, `color`, `mask`, `mask-image`, `text-decoration`,
`text-decoration-color`, `text-shadow`.

Omitting a color from a border or shadow shorthand can still produce the
internal initial/current-color value; the difference is that authors cannot
spell `currentcolor` as a component.

#### Image and gradient-bearing properties (7)

Lynx image values retain `none`, URLs, and non-repeating modern
`linear-gradient()`, `radial-gradient()`, and `conic-gradient()`. It removes
upstream Servo's `image-set()`, `image()`, paint worklet, pref-gated
`cross-fade()`/`light-dark()`, repeating gradients, WebKit-prefixed gradients,
and legacy `-webkit-gradient()`.

Lynx gradients require at least two color stops. A stop position is absent or
one literal number/percentage fraction; lengths and `calc()` are rejected.
Interpolation hints, two-position stops, and selectable interpolation color
spaces are rejected. Gradient colors use the restricted Lynx color grammar.

Affected spellings: `background`, `background-image`, `border-image`,
`border-image-source`, `color`, `mask`, `mask-image`. The `color` property uses
only the gradient branch of this image grammar; it does not accept `none` or a
URL.

### Property-specific grammar and semantic changes

The global primitive changes above still apply to every property in this
table.

| Property spelling(s) | Difference from upstream Servo |
|---|---|
| `align-content` | Lynx accepts `stretch`, `start`, `end`, `flex-start`, `flex-end`, `center`, `space-between`, `space-around`; it rejects upstream `normal`, baseline forms, safe/unsafe positions, left/right, and `space-evenly`. |
| `justify-content` | Same Lynx set as `align-content`, plus `space-evenly`; it rejects upstream `normal`, baseline forms, safe/unsafe positions, and left/right. |
| `align-self` | Lynx accepts `auto`, `stretch`, `center`, `start`, `end`, `flex-start`, `flex-end`, `baseline`; it rejects `normal`, self-start/end, first/last-baseline forms, and safe/unsafe positions. |
| `justify-self` | Lynx accepts only `auto`, `stretch`, `center`, `start`, `end`; it rejects flex/baseline, left/right, self-start/end, normal, and safe/unsafe forms. |
| `align-items` | Lynx accepts `stretch`, `center`, `start`, `end`, `flex-start`, `flex-end`, `baseline`; it rejects normal, self-start/end, first/last-baseline, and safe/unsafe forms. |
| `justify-items` | Lynx accepts only `stretch`, `center`, `start`, `end`; it rejects normal, legacy, left/right, flex/baseline, and safe/unsafe forms. |
| `place-content`, `place-self`, `place-items` | Component grammars above apply. For a one-value shorthand, Lynx explicitly rejects a value that cannot also be copied to the inline-axis component; two-value forms remain supported. |
| `animation-duration`, `animation` | Lynx accepts a non-negative `<time>` only. Upstream can additionally accept `auto` when its scroll-driven-animation pref is enabled. |
| `animation-timing-function`, `transition-timing-function`, `animation`, `transition` | Lynx adds `square-bezier(x, y)` and removes CSS `linear()`. Standard keywords, `steps()`, and `cubic-bezier()` remain. `square-bezier` is converted to an equivalent cubic Bézier for evaluation. |
| `background-clip` | `border-area` is unconditionally accepted under Lynx; upstream Servo gates that value with `layout.css.background-clip.border-area.enabled`. Other common box values are unchanged. |
| `clip-path` | Lynx accepts `none`, `inset()`, `circle()`, `ellipse()`, or `path()`. It rejects upstream URL, polygon/shape/rect/xywh, and geometry-box forms. |
| `color` | Unlike upstream Servo's solid-only property, Lynx also accepts and preserves one of its restricted gradients for text painting. A gradient uses transparent as the solid fallback for internal current-color consumers. `color` animation is discrete instead of interpolated, and the document-colors-disabled cascade special case is not applied to the Lynx enum. |
| `cursor` | Lynx accepts only a cursor keyword. It rejects upstream URL/hotspot image lists and the `-moz-grab`, `-moz-grabbing`, `-moz-zoom-in`, and `-moz-zoom-out` value aliases. |
| `contain` | Lynx force-enables the property while upstream Servo's default `layout.unimplemented` pref rejects it. Its W3C parser, computed `Contain` bit layout, and structural meaning are otherwise unchanged. |
| `display` | Lynx accepts exactly `none`, `contents`, `flex`, `grid`, `linear`, `relative`. It rejects `block`, `inline`, flow/table/list values, inline-flex/grid, and compound syntax. `contents` retains Stylo's original `is_contents` behavior, `DISPLAY_CONTENTS_IN_ITEM_CONTAINER` propagation, and root/top-layer blockification. The upstream initial inline-flow encoding remains internal; therefore authored `display: block` is still invalid. |
| `filter` | Lynx accepts `none` or chains of `blur()`, `brightness()`, `contrast()`, `grayscale()`, and `saturate()`. It rejects upstream Servo's `hue-rotate()`, `invert()`, `opacity()`, `sepia()`, and `drop-shadow()`. URL filters are Gecko-only and therefore are not a Lynx-vs-Servo difference. |
| `image-rendering` | The standard Servo values remain, but Lynx removes the `-moz-crisp-edges` value alias. |
| `letter-spacing` | Lynx accepts `<length>` only. Upstream also accepts `normal` and percentages through its shared spacing type. |
| `max-width`, `max-height` | Lynx accepts only a non-negative `<length-percentage>` and disables quirks parsing. It rejects upstream `none`, intrinsic sizing keywords/functions, and other `MaxSize` arms; upstream `none` remains the internal initial value. |
| `offset-path` | Lynx accepts exactly one `inset()`, `circle()`, `ellipse()`, or `path()` and assigns the border box internally. It rejects authored `none`, `ray()`, URL, polygon/other shapes, a coordinate box, and path-position combinations. |
| `overflow`, `overflow-x`, `overflow-y` | Lynx accepts only `visible | hidden`. It removes `scroll`, `auto`/`overlay`, and `clip`. In the mixed-axis computed-value fixup, Lynx maps the non-scrollable side to `hidden` because no `auto` representation exists. |
| `position` | Lynx accepts `relative | absolute | fixed | sticky`; authored `static` is rejected although the upstream internal initial value remains representable. |
| `text-align` | Lynx accepts `start | end | left | right | center`; it rejects `justify`, `match-parent`, and the WebKit/Mozilla compatibility aliases. |
| `text-decoration-line`, `text-decoration` | The line component must be exactly one of `none`, `underline`, or `line-through`. Lynx rejects `overline`, `blink`, and combined line flags. Other enabled shorthand components retain their own grammars. |
| `text-overflow` | Lynx accepts one `clip` or `ellipsis`; upstream's two-value form and custom string arm are removed. |
| `text-shadow` | Lynx accepts exactly one `offset-x offset-y blur color` in that order, with all four components required. It rejects color-first/color-last flexibility, omitted blur, and comma-separated layers. The property is non-animatable instead of upstream's shadow interpolation. |
| `transform` | Lynx requires number scale factors, removes `scaleZ()`, `scale3d()`, `rotate3d()`, and `perspective()`, while retaining matrix/matrix3d, translate 2D/3D, scale X/Y, rotate X/Y/Z, and skew forms. The Lynx length and angle subsets apply to retained functions. |
| `transform-origin` | Lynx accepts only a one- or two-dimensional origin; the third depth length is rejected. |
| `visibility` | Lynx accepts `visible | hidden`; `collapse` is compiled out. |
| `transition-property`, `transition` | Property names missing from the Lynx lookup table compute as `Unsupported` identifiers instead of upstream `NonCustom` ids. This includes explicit `transition-property: all`: because the `all` property spelling is hidden, the authored token does not become Stylo's special all-properties id, even though omitted transition-property still uses the internal initial `all`. Lynx-only property names have the inverse difference and resolve to real ids. |
| `will-change` | The authored W3C grammar is retained. `contain` is now a real Lynx property id and `will-change: contain` sets Stylo's original `CONTAIN` structural bit. Property-name-derived bits still require a name in the Lynx lookup table and a compiled match arm, so omitted hints such as `backdrop-filter`, `view-transition-name`, and individual `translate`/`rotate`/`scale` retain their identifier text without setting those upstream bits. Supported names such as `transform`, `filter`, `opacity`, `perspective`, `position`, `z-index`, `mask-image`, and `clip-path` retain their original bits. |

## Common spellings audited as unchanged (52)

These names are present on both sides and have no known Lynx-specific parser,
primitive-value, computed-value, animation, default-availability, or
property-reference difference at the pinned revisions. This means their
property grammar is unchanged; UA stylesheet defaults can still produce a
different page result without changing the Stylo property implementation.

```text
alignment-baseline
animation-delay
animation-direction
animation-fill-mode
animation-iteration-count
animation-name
animation-play-state
aspect-ratio
background-attachment
background-origin
background-repeat
baseline-source
border-bottom-style
border-image-repeat
border-image-slice
border-inline-end-style
border-inline-start-style
border-inline-style
border-left-style
border-right-style
border-style
border-top-style
box-sizing
direction
flex-direction
flex-flow
flex-grow
flex-shrink
flex-wrap
font-family
font-feature-settings
font-kerning
font-stretch
font-variant
font-variant-caps
font-variant-east-asian
font-variant-ligatures
font-variant-numeric
font-variant-position
font-weight
opacity
order
pointer-events
text-decoration-style
text-wrap-mode
transition-behavior
transition-delay
transition-duration
white-space
white-space-collapse
word-break
z-index
```

Notably, this parity set includes the properties explicitly restored to
upstream structure during the subset work: `aspect-ratio`, `z-index`, border
styles,
`border-image-slice`/`border-image-repeat`, flex direction/flow/grow/shrink,
and the white-space components. `column-gap`, `row-gap`, sizing properties,
`perspective`, and `box-shadow` are not in the parity set because, although
their property-specific parsers were restored, their nested Lynx length and/or
color grammar still differs.

## Maintenance rules

- Re-run both property generators whenever the Stylo fork or `upstream/main`
  revision changes. Compare the generated `StaticId::NonCustom` lookup entries,
  not only `css-properties.json`, so hidden-canonical aliases remain visible.
- Re-audit every added/removed `cfg(feature = "lynx")`, `lynx_*` generator
  override, and Lynx-only value type. Update both the canonical 160-name set
  and the mechanism tables; the 160/52 partition must still cover all common
  spellings.
- Keep custom properties out of the non-custom counts, but retain an explicit
  test that `--*` lookup and `var()` substitution work with the feature on.
- A property restored to upstream W3C structure can remain in the different
  set when it contains a globally restricted primitive. Remove it only when
  its complete authored and computed behavior matches upstream Servo.
