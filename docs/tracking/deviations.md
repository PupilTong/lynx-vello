# Known Lynx ↔ W3C behavior deviations

> Research: multi-agent sweep over `lynx/` and `lynx-stack/` (see [AGENTS.md](../../AGENTS.md)
> for the reference-repo shorthand and the W3C-first standards policy).

The per-domain research pass found **283 individual rows** marked `No`/`Partial`
in the `W3C-compliant?` column across `css-layout.md`, `css-visual.md`,
`css-text.md`, `css-animation.md`, `dom-events.md`, `js-runtime.md`,
`components.md`, `reactlynx.md`, `css-selectors-cascade.md`, `css-at-rules.md`,
`media-resources.md`, and `accessibility.md` — that's the honest total, and
this file does **not** reproduce all of it (that would just be a copy of
those tables). It curates the ~35 rows with the broadest architectural
impact — the ones that shape a whole subsystem's design rather than a single
property's value grammar. **For the complete list, read the `W3C-compliant?`
column in each file directly** before assuming something isn't covered here.

Most rows are plain "Lynx has a non-standard extra API/property with no CSS
equivalent" (port as-is, no policy decision needed) — those are *not*
curated here. What's below is specifically: (a) cases where Lynx's behavior
actively conflicts with a W3C algorithm/API Lynx *also* claims to implement,
and (b) cases where matching ReactLynx behavior requires an explicit,
consequential choice about whether to follow the spec or the quirk.

## Layout & stacking (see [css-layout.md](css-layout.md))

- **`z-index`/stacking context** — Lynx reparents any element with
  `z-index != 0` once to the nearest stacking-context ancestor and sorts
  siblings by raw integer value; it does not implement the real recursive,
  per-stacking-context CSS algorithm. **Decision: implement the real CSS
  algorithm** (reuse stylo/Servo's existing stacking-context logic) —
  apps relying on Lynx's actual buggy z-index behavior may render
  differently, and that's intentional.
- **`overflow`/`overflow-x`/`overflow-y` default** — Lynx defaults to
  `hidden`; CSS defaults to `visible`. **Decision: match Lynx's default**,
  not CSS's — this is a values/defaults divergence, not an algorithm one,
  and ReactLynx apps assume clipping-by-default. The *keyword set* went the
  other way (2026-07-29): Lynx's native grammar is `visible | hidden`, but
  the fork now ships `scroll` and `clip` too, because the web bundle this
  stack consumes uses both — `web-elements`' `scroll-view.css` authors
  `overflow-y: scroll` and `overflow-x: clip` — so the trimmed set made a
  scrollable box inexpressible. Only `scroll` responds to gestures; `hidden`
  stays a clip that scrolls only programmatically, which is what keeps the
  `hidden`-on-every-element UA default from making the whole page
  drag-scrollable, and `clip` is not a scroll container at all.
- **`position: sticky` gap resolved (2026-09-22)** — sticky boxes keep
  their normal-flow layout and receive scrollport-relative offsets during
  composition and hit testing. Insets use the nearest scrollport on each
  axis, `hidden` counts and `clip` does not, and the containing block limits
  movement. Grid items use their grid area. The immutable frame carries the
  constraints, so live scrolling needs no layout or commit while the frame's
  encode window covers the new offset. List `sticky-top`/`sticky-bottom`
  attribute rules remain a separate component integration task.
- **`overflow: auto` omitted** — CSS has it; this engine does not.
  **Decision (user, 2026-07-29): leave it out.** Nothing here paints
  scrollbars, so `auto` ("scrollbars only when needed") would behave exactly
  like `scroll` everywhere except one place: css-overflow-3 §3 pairs a
  `visible` axis against a scrollable one by computing the `visible` one to
  `auto`. Without `auto`, stylo's `to_scrollable()` pairs it into `hidden`
  instead. **Consequence to know:** given `overflow-x: visible; overflow-y:
  scroll`, a horizontal overflow that a browser would let the user drag is
  clipped here instead. The vertical scrolling authors actually wanted is
  unaffected, and no axis becomes spuriously draggable.
- **`box-sizing` default** — Lynx defaults to `border-box`; CSS defaults to
  `content-box`. **Decision: match Lynx's default**, same reasoning as
  `overflow` above.
- **`display`** — carries only Lynx's internal child-layout-mode switch
  (flex/linear/grid/relative/none), not CSS's external inline/block
  dichotomy. Model our own `display` computation to expose both: the
  Lynx-compatible internal mode apps' CSS actually selects, plus (opt-in)
  real external display type if we ever need it.
- **`display: linear` / `display: relative` and their `linear-*`/`relative-*`
  properties** — genuine Lynx-only layout algorithms (Android
  `LinearLayout`/`RelativeLayout`-derived) with no CSS equivalent at all;
  not a spec violation, just extensions to implement faithfully as their
  own algorithms (not flex polyfills, unlike how `web-core` does it).
- **`display: -lynx-text`** — a fork-only display value naming one flattened
  Lynx paragraph, added because Lynx's own inline-ness is **structural, not
  display-driven**: `TextElement::OnNodeAdded` converts every added child via
  `ConvertToInlineElement()` (`lynx/core/renderer/dom/fiber/text_element.cc`),
  a node is inline iff its parent is a text or a non-view inline element
  (`element.cc:562-571`), and there is no `display: inline` anywhere in the
  native model. This engine cascades, so the structural rule needs a computed
  value to ride on; `-lynx-text` is it. Block-level, and deliberately **not**
  an item container (`is_item_container() == false`, like `LynxRelative` and
  unlike `LynxLinear`): a text block's subtree is inline content — runs,
  nested text scopes, atomic inline boxes — not flex or grid items. The
  keyword is vendor-prefixed because it is neither a W3C value nor a value
  Lynx itself spells: no bundle author writes it, and it is unreachable from
  a plain `servo` build (pinned by `lynx_feature_off_parity`).
- **Relative logical/physical precedence (temporary implementation
  deviation)** — Starlight remaps logical relative-property IDs to a physical
  slot during style application, so a logical and physical declaration that
  target the same side are last-applied-wins. The stylo fork currently models
  the four logical variants as independent longhands, which discards that
  cross-property source order before `hughie` lowers them. The current
  bridge consequently uses physical-wins-unless-sentinel. Direction comes
  from the item itself and the LTR/RTL mapping is otherwise exact. Exact
  bucket-2 parity requires changing the fork declarations into a true
  `logical = true` group; until then this tie-break is an explicitly recorded
  limitation, not a desired semantic.
- **`position: fixed` containing block** — in every mode Lynx supports, a
  fixed element's containing block is unconditionally the single page-root
  element (reached via render-tree reparenting in the legacy path, or a
  dedicated root pointer + root-only measurement pass in the newer
  `enable-fixed-new`/`enable-unify-fixed-behavior` paths), with **no
  exception for ancestors with `transform`/`filter`/`perspective`/
  `will-change`/`contain`** (confirmed absent — no `transform` reference
  exists anywhere in `core/renderer/starlight/layout/`, and Lynx has no
  `contain` property at all) and no component-boundary-scoped containing
  block either. **Decision: implement the real CSS algorithm** — viewport
  by default, re-anchored to the nearest qualifying ancestor when one
  exists — not Lynx's unconditional escape-to-root. An element with a current
  `transform` animation qualifies whatever its committed value, since
  web-animations-1 treats it as `will-change: transform`; web-core leaves
  that to the browser, which applies it. (Scroll-offset
  exclusion, on the other hand, is achieved structurally — the fixed
  element's native view is simply never mounted inside any scrollable
  ancestor — and that part already matches the *observable* CSS behavior of
  staying put while scrolling, so no decision is needed there.)

## CSS visual/paint & animation (see [css-visual.md](css-visual.md), [css-animation.md](css-animation.md))

- **`background` shorthand, multi-layer** — Lynx has an acknowledged code
  comment ("different from the web"): if a background layer has no image,
  Lynx silently skips updating that layer's other sub-properties instead of
  treating each layer independently per spec. **Decision: implement full
  per-layer independence** per spec rather than the acknowledged bug.
- **`filter`** — standard CSS accepts a space-separated chain of any number
  of filter functions (`blur(2px) grayscale(50%)`); Lynx's parser hard-fails
  after the first function. Implement the real chained grammar.
- **`filter: blur()`** — every Lynx backend diverges from filter-effects-1
  here, in a different way, and **we follow W3C on all of them** (user
  decision, 2026-09-19). What the length means: filter-effects-1 says the
  length **is** σ, while Clay treats it as a *radius* and converts with
  Skia's `SkBlurMask` formula, σ = 0.57735·r + 0.5
  (`clay/gfx/graphics_context.cc:481-483`, reached for `filter` at
  `clay/ui/painter/painting_context.cc:90-97`), so a Clay `blur(4px)` is
  σ ≈ 2.81 where ours is σ = 4. Edge mode: the spec's is transparent black,
  which Clay matches (`TileMode::kDecal`, same lines) but Android ≥ 31 does
  not — `RenderEffect.createBlurEffect(radius, radius, Shader.TileMode.CLAMP)`
  in `platform/android/lynx_android/src/main/java/com/lynx/tasm/utils/BlurUtils.java:46-70`
  extends the edge pixels instead. Filter region: Android < 31 has no
  `RenderEffect` at all and falls back to a `ViewTreeObserver.OnPreDrawListener`
  that redraws the view into a bitmap of the view's own size
  (`.../behavior/ui/view/AndroidView.java:61-76,436-456`), so its blur is
  clipped to the view box rather than overflowing it by 3σ. Grouping: iOS sets
  the `CIFilter` on the view's own layer *and separately* on the background,
  border and outline layers (`platform/darwin/ios/lynx/ui/LynxUI.m:3670-3676`
  with `.../base/background/LynxBackgroundManager.m:1622-1626`), so those blur
  as independent layers instead of as one composed group. Animation: native
  interpolates the *amount* of one filter function and falls back to the start
  value whenever the function type or unit differs
  (`core/animation/keyframed_animation_curve.cc:915-971`,
  `gfx::DiscreteFallback::kUseStart`); we animate `filter` as a real CSS
  filter list through stylo's own engine. `filter` is never exported as a
  composite curve, so an animated blur recommits and re-bakes every tick —
  accepted, and recorded in `crates/dom/src/paint/painter.rs`'s v1 limits
  together with the approximations the offscreen implementation carries (one
  isotropic σ under a non-uniform scale or skew, an area budget past which a
  group renders *unblurred*, and several `blur()` functions folded into one).
- **`backdrop-filter`** — Lynx has the property nowhere: no handler, no
  property ID, no wire enum entry, and `web-core` therefore never emits one.
  We expose it anyway with filter-effects-2 W3C semantics (user decision,
  2026-09-19), so this is a superset of the compat target rather than a
  behavioral conflict. It reuses the `filter` value grammar unchanged and
  triggers the same three structural effects filter-effects-2 §2.1 names: a
  stacking context, a group render layer (the filtered backdrop is painted
  inside the element's own effect layer, so the element's `opacity`,
  `filter`, `clip-path` and `mask-image` apply to backdrop and element
  together), and a containing block for absolutely and fixed positioned
  descendants unless the element is a document root element. Unlike `filter`
  it does not enlarge the element's ink overflow.

  It is **painted** as of 2026-09-19, as a prefix bake on the flat render
  path (`crates/dom/src/paint/compose.rs`'s `PushBackdrop`,
  `crates/dom/src/render/blur.rs`), with these recorded narrowings:

  - **`will-change` Backdrop Roots are not honored**, and this one *is* observable. `will-change`
    is in the fork's author grammar, and filter-effects-2 makes an element naming a rooting
    property (`opacity`, `filter`, `backdrop-filter`, `mask`, `clip-path`) a Backdrop Root. This
    engine opens a group render layer only for the property actually applied, not for a
    `will-change` naming it, so a `backdrop-filter` element inside a `will-change: opacity`
    wrapper reads through that wrapper to the content behind it, where a browser would stop at
    it. That is the ruled trade — no layer per `will-change` element — and
    `crates/dom/src/paint/walker.rs`'s `a_backdrops_range_begins_at_its_backdrop_root` pins it.
    The one `will-change` that is honored is the one Web Animations implies for a current
    `opacity` animation, exported or not: that element already paints a group, and it roots at
    every reading, 1 included (`a_fade_roots_a_backdrop_while_it_reads_1`).
  - **`isolation: isolate` is not a Backdrop Root**, which is the spec's own list rather than a
    deviation: filter-effects-2 does not name `isolation`. It is called out here only because
    `visual::stacking::needs_group_rendering` *does* open a layer for it, which is why the walker
    carries a separate `is_backdrop_root` predicate instead of reusing that one. Unobservable
    either way: `isolation` is absent from the fork's author grammar.
  - **Mirror at the axis-aligned device bounding box.** The spec crops the Backdrop Root Image to
    the element's *transformed* border box before filtering. The bake rect is that box's
    axis-aligned device bbox, so for a rotated or skewed element the kernel mirrors at the bbox
    rather than at the rotated rectangle. The drawn result is still clipped to the rotated rounded
    border box, so the difference is confined to what the kernel reads within a few σ of a rotated
    edge — which is what Chromium does as well.
  - **One isotropic σ**, scaled by the arithmetic mean of the two singular values of the element's
    local-to-viewport map, exactly as `filter: blur()` is; a non-uniform scale or a skew gets one
    number where the spec's filter region would be anisotropic.
  - **Colour passes fold around one blur.** The list splits at its first `blur()`; every other
    `blur()` folds into it by variance addition, the passes before it are drawn inside the bake and
    the passes after it over the composed backdrop. A colour pass sitting *between* two blurs is
    therefore applied inside the one bake rather than between them, the same approximation `filter`
    carries.
  - **Items culled at commit are absent from a straddling crop.** The walker discards items that
    can put no ink in the viewport, so an element whose border box extends past the viewport bakes
    a crop in which the off-screen part holds only what survived culling. Those pixels are outside
    the viewport, so nothing visible reads them directly — but a σ large enough to reach back in
    can, and that part of the crop will be emptier than the spec's Backdrop Root Image.
  - **No composite curve**, like `filter`: an animated `backdrop-filter` recommits and re-bakes
    every tick.
  - **The area budget is shared with `filter: blur()`** (`MAX_FILTER_DIMENSION`,
    `MAX_FILTER_AREA`), consumed in program order; an element past it draws no backdrop at all, so
    what shows is the **unfiltered** backdrop underneath. The same fallback covers a consumer with
    no GPU.
- **`background-clip: border-area`** — a genuine Lynx-only value with no CSS
  equivalent (distinct from `border-box`); needs its own behavioral
  spec-mining rather than mapping to any standard box.
- **`ease` timing-function keyword** — Lynx maps `ease` to the *same* curve
  as `ease-in-out` (`cubic-bezier(0.42,0,0.58,1.0)`); confirmed deliberate
  (asserted in a unit test), not an oversight, and the spec-correct curve
  (`cubic-bezier(0.25,0.1,0.25,1.0)`) exists in the codebase but is
  unreachable. **Decision: implement the real, distinct `ease` curve.**
- **`transition-property`** — Lynx resolves each token against a closed
  internal property enum (plus non-standard `scaleX`/`scaleY`/`scaleXY`
  pseudo-properties) instead of accepting arbitrary CSS property
  idents/`all` per spec. Implement standard open-ended property-name
  matching.

## Text layout (see [css-text.md](css-text.md))

- **The `text-overflow` *attribute* on `<text>` is honoured, where web-core
  ignores it** — native Lynx reads `text-overflow` off the element as well as
  out of CSS: `TextElement::ProcessAttributeForNormalLayoutMode` caches the
  attribute's value onto `kPropertyIDTextOverflow`
  (`lynx/core/renderer/dom/fiber/text_element.cc:176-182`), and the Android
  shadow node declares it as a prop with the default `clip`
  (`BaseTextShadowNode.java:158`). The web target observes no such attribute:
  `x-text` watches `text-maxline`, `text-maxlength`, `tail-color-convert` and
  `text` only, so a `text-overflow` attribute lands in the DOM verbatim and
  nothing reads it — `text-overflow` is CSS-only there, inherited into the
  shadow part by `x-text::part(inner-box) { text-overflow: inherit; }`
  (`lynx-stack/packages/web-platform/web-elements/src/elements/XText/x-text.css:28-31`).
  A user ruling of 2026-09-16 follows native rather than web-core here. The
  mechanism is two UA-sheet attribute selectors keyed on the two literals
  native's enum parser accepts, `text[text-overflow="ellipsis"]` and
  `text[text-overflow="clip"]` (`crates/bobcat-core/src/main/tree/text.rs`),
  rather than a presentational hint: the attribute needs no parsing of its own
  and maps onto an existing CSS property. They are plain declarations, so
  author CSS outranks the attribute the way it outranks the rest of the UA
  sheet, and the attribute's own value is never rewritten.
- **`text-align: justify` does not parse, and the declaration is dropped
  without a trace** — the vendored fork excludes `Justify` from
  `TextAlignKeyword` under its `lynx` feature
  (`vendor/stylo/style/values/specified/text.rs`, `#[cfg(not(feature =
  "lynx"))]`), so the value is invalid at parse time and CSS drops the whole
  declaration. Both worlds it is measured against accept it: native Lynx's own
  parser maps the keyword to `TextAlignType::kJustify`
  (`lynx/core/renderer/css/parser/enum_handler.cc`), and the web target hands
  `text-align` straight to the browser, which really does justify. The failure
  mode is quiet and it is *not* a fallback to `start`: an invalid declaration
  leaves whatever `text-align` already cascaded or inherited in place, so a
  `text` inside a `text-align: center` subtree stays centered instead of
  becoming justified — no console message, no computed-value difference to
  notice. Recorded, not fixed. Nothing below the parser is missing: Parley
  already implements `Alignment::Justify` (inter-cluster expansion on every
  line but the last), and `hughie`'s `TextAlign` → `Alignment` map is a `match`
  with one arm to add. The whole cost is a fork patch un-gating the keyword,
  which is a change to the vendored submodule and its own review, so it is
  filed here rather than smuggled into an engine-side change.
- **Parley 0.11 paragraph base direction** — the public Parley builders and
  `Layout` expose no base-direction override. Internally, text analysis always
  invokes bidi resolution with an automatic/first-strong base level, even
  though the lower-level `parlance` crate defines a `BaseDirection` value.
  Consequently, the `hughie` text adapter can resolve physical
  `text-align: start`/`end` from `CoreStyle::direction()`, but cannot yet force
  UAX #9 paragraph ordering for neutral or opposite-strong text inside an
  explicitly LTR/RTL container. Injecting bidi controls would corrupt source
  byte ranges, so the current implementation retains Parley's automatic bidi
  shaping and records this as an integration limitation. Revisit when Parley
  exposes its base-level input (or if a narrowly-scoped upstream patch is
  adopted).

- **`hughie::text::block` ellipsis algorithm** — the standalone Lynx
  text-block module implements the web reference's *slow path*
  (`XTextTruncation.ts`, source-verified) for **every** ellipsis mode: back
  the maxline cut off three source units — fewer on a short line — and append
  a literal `"..."`; never back off a maxlength cut; take the earlier cut,
  keeping the dot count the maxline branch decided even when maxlength wins
  (the web computes `ellipsisLength` before taking the minimum); with
  inline-truncation content, retreat by freed width with a minimum removal of
  two units (the web's fitting loop starts from an empty measurement and
  decrements before its first width check), and suppress the dots marker
  whenever inline-truncation content exists, shown or not. Line ranges — and
  therefore the back-off basis and `ellipsisCount` — cover visible content
  only: the collapsed whitespace a soft wrap consumed belongs to no line,
  matching the web's rect-derived ranges, so consecutive ranges can have a
  gap. One start-side delta remains: the web steps the next line's start to
  the previous end plus exactly one, so when a wrap collapsed more than one
  whitespace unit the web counts the extras into the next line's range (and
  its short-line dot arithmetic) while this module starts the line at its
  first actually-present unit. Default-mode web bundles actually delegate to the browser's own
  `-webkit-line-clamp` ellipsis, whose cut point and dot styling can differ
  by a character; the slow path is the only inspectable algorithm. Further
  deltas from the web loop, all strictly safer: cuts land on cluster
  boundaries (the web steps raw UTF-16 units and can split a surrogate pair;
  a `text-maxlength` cut inside a pair rounds down), and a dots-or-truncation
  tail that wraps is clamped away by the visible-line rebuild (the web
  overflows it). `text-overflow: clip` is implemented as spec clip — cut at
  the line end, no marker — both references being opaque there. Truncation
  content is shown only on maxline overflow (web-verified:
  `x-show-inline-truncation` is set only in the maxline branch), and
  `justify` can stretch a truncated last line whose tail wrapped and was
  clamped (its break reason is then `Regular`), where the web does not. A
  trailing preserved newline, a hanging trailing space, or the renderless
  line parley commits after a box overflows onto the last line never counts
  as overflow: whether content remains is decided in source units over the
  committed lines, and cuts address boxes in unit space (a box shares its
  byte with the character after it).
- **`tail-color-convert` follows native Lynx, not the web target** — the
  attribute is implemented as the boolean the Android and iOS shadow nodes read
  (`lynx/js_libraries/types/skills/text.md:71-76`, Android
  `TextRenderer.convertTailColor`, iOS
  `LynxTextRenderer.m overrideTruncatedAttrIfNeed`): its default is *false*, in
  which case the truncation marker wears the colour of the inline run the cut
  landed in, and `true` replaces the marker's fill colour with the establishing
  element's and nothing else — the dots keep the cut run's font, shadow, stroke
  and decorations, so the marker's advance never changes between the two
  states. `web-core` inverts the default: the marker is a pseudo-element of the
  outer box carrying the block's whole style, and `="false"` selects a separate
  measured path that splices literal dots into the cut run
  (`XTextTruncation.ts:88-104`, `x-text.css:200-241`). `AGENTS.md` names
  `web-core` as the compatibility target, so this is a knowing divergence from
  it, **ruled by the user on 2026-09-15**. The visible consequence is geometric
  as well as chromatic: a cut landing in a run with a larger font gives a wider
  marker here than under `web-core`. The wiring is one registered custom
  property, `--lynx-tail-color-convert`
  (`crates/bobcat-core/src/main/tree/text.rs:26-39`, `:116`), read back by
  `TextContainerStyle::tail_color_convert`
  (`crates/hughie/src/style/text.rs:55`) and applied by `RunPaint::fill_style`
  (`crates/dom/src/paint/text.rs:116`), so reversing the ruling is a change to
  the painter and the default, not to shaping.
- **`hughie::text::block` vertical alignment of atomic inlines** — parley 0.11
  has no vertical-align: an in-flow box sits bottom-on-baseline and its height
  counts as pure ascent. The module writes each box's line-height
  *contribution* into `InlineBox::height` (above-baseline part for
  baseline-anchored values, full height otherwise) and overrides each box's
  `y` from a placement table, so lines are always tall enough — but the
  baseline's position inside a line grown by a non-baseline-aligned tall box
  follows parley's all-ascent rule rather than the CSS ascent/descent split,
  a baseline-aligned box's below-baseline part does not reserve descent, and
  `super`'s raise is placement-only. `sub`/`super` offsets are named UA
  constants (0.20/0.34 of the reference font size), `Percent`'s basis is the
  line's resolved line height, and the Lynx-only `center` behaves as
  `middle`.
- **`hughie::text::block` line-height scope** — per-run (the web target's CSS
  inheritance behavior), not paragraph-wide as native textra applies it
  (`ApplyParagraphStyle`); the paragraph-wide behavior is the degenerate case
  where every run carries the same value, which inheritance produces on its
  own. This is the D4 question `docs/text-measurement-and-ifc.md` reserves;
  the block module ships the web-shaped answer and records it here.
- **`text-maxline="1"` is a one-line clamp, not a one-line shape** — **ruled by
  the user on 2026-09-15**: on this one the engine follows native Lynx rather
  than the `web-core` default. Native builds the paragraph's layout at the
  available width and keeps one line of it — Android's `StaticLayout` is
  constructed at the measure and told `setMaxLines(1)`
  (`shouldBeSingleLine()`,
  `lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/shadow/text/TextRenderer.java:181-184,231-245`),
  and iOS gives an `NSTextContainer` of the same size a
  `maximumNumberOfLines`
  (`lynx/platform/darwin/ios/lynx/shadow_node/text/LynxTextRenderer.m:1009-1031`)
  — with the tail ellipsize (`TruncateAt.END`,
  `NSLineBreakByTruncatingTail`) reached only under
  `text-overflow: ellipsis`. `web-core` instead makes the attribute a *shape*:
  `white-space: nowrap` on the inner box plus
  `max-width: -webkit-fill-available` on the host
  (`x-text.css:216-241`), so its line never breaks and runs to the parent's
  edge. This engine declares neither, so a one-line clamp stops at the last
  word boundary that fit and the block is as wide as that line. Two replicas
  assert the `web-core` geometry and stay `#[ignore]`d, marked DEVIATION —
  `a_single_line_clamp_caps_the_block_to_its_parent_s_available_width` and
  `a_one_line_clamp_fills_the_available_width_instead_of_breaking_at_a_word`
  in `crates/bobcat-core/src/main/tree/web_text_replication.rs`. Reversing the
  ruling is two UA declarations. Unaffected by it: an overflowing
  `white-space: nowrap` line *is* cut at the clip edge under
  `text-overflow: ellipsis` (`TextBlock::overflow_cut`,
  `crates/hughie/src/text/block/mod.rs:778-876`), which is css-ui
  `text-overflow` in its own right and what both references do for a line that
  overflows its measure.
- **`position: relative` insets on an atomic inline box inside a `<text>` are
  ignored** — **ruled by the user on 2026-09-17**, following native Lynx
  against `web-core`. The atom is still ordinary in-flow content: it advances
  the line, breaks with it and keeps its place among its siblings (only
  `absolute` and `fixed` leave a paragraph's flow). What is dropped is only the
  post-placement shift, so `left`/`top`/`right`/`bottom` move nothing.
  **Native** never applies them to a paragraph's inline content:
  `CalcRelativePosition`
  (`lynx/core/renderer/starlight/layout/position_layout_utils.cc:38-71`) has
  exactly one caller, `LayoutAlgorithm::HandleRelativePosition`
  (`lynx/core/renderer/starlight/layout/layout_algorithm.cc:215-228`), which
  walks the container algorithm's `inflow_items_` — and a `<text>` has a
  `measure_func_`, so it returns before any `LayoutAlgorithm` is built
  (`lynx/core/renderer/starlight/layout/layout_object.cc:684-696`) and the
  inline view's position comes only from `AlignmentByPlatform`. **web-core**
  differs, by inheriting the browser's rule: `x-view` carries
  `position: relative`
  (`lynx-stack/packages/web-platform/web-elements/src/elements/common-css/linear.css:142`)
  and `x-text > x-view` is `display: inline-flex !important`
  (`.../XText/x-text.css:90-93`), so the browser paints it shifted while
  leaving the advance alone. **Consequence:** an authored
  `position: relative; left: …` on a `view` or `image` written inside a `text`
  is a silent no-op — no diagnostic, and the computed style still reports the
  authored inset. A nested `text`/`inline-text`, or a `display: contents`
  wrapper, generates no box at all, so its insets never applied in the first
  place and nothing changed for those. Reversing the ruling is one term in
  `place_and_hide` (`crates/dom/src/layout/text_block.rs`) plus the inset
  resolution to feed it; the guard is
  `a_relative_atom_stays_in_the_line_and_only_absolute_leaves_it`
  (`crates/dom/tests/web_text_replication.rs`).
- **An atomic inline box with no baseline of its own sits with its bottom
  *margin* edge on the line's baseline** — CSS's and `web-core`'s rule for an
  inline-block with no in-flow line boxes, and a **ruled** deviation from
  native (2026-09-17). Native puts the bottom *border* edge there instead and
  lets `margin-bottom` hang below the baseline:
  `LayoutObject::GetOffsetFromTopMarginEdgeToBaseline` returns
  `GetLayoutMarginTop() + offset_height_` whenever the box reports no baseline
  (`lynx/core/renderer/starlight/layout/layout_object.cc:1118-1124`), and
  `offset_height_` is the border-box height
  (`layout_object.h:170`). **Consequence:** `margin-bottom` on such an atom
  grows the line here and does not on native — a 40x60 atom with
  `margin-bottom: 8px` makes a 68px line here against native's 60. The wiring
  is one `max` in phase 2 of `compute_text_block_layout`
  (`crates/dom/src/layout/text_block.rs`), which shifts the reported baseline
  down with the margin box's top edge; the guard is the `.down` block of
  `an_atoms_margins_reach_the_line_and_step_its_border_box_in`
  (`crates/dom/tests/web_text_replication.rs`).

## Event model & gestures (see [dom-events.md](dom-events.md))

- **`bind`/`catch`/`capture-bind`/`capture-catch`/`global-bindEvent`** — phase
  and stop-propagation behavior are baked into which attribute name authored
  a handler, not a runtime `{capture}` flag on a shared listener API as in
  DOM, and there's no way to register more than one handler with independent
  behavior at the same node/phase. **Decision: implement real DOM Level 3
  dispatch** (single capture+bubble walk, explicit `capture: bool` per
  listener) and translate `catch*`/`capture-catch` to "register + implicit
  `stopPropagation()`" — this reproduces Lynx's exact observable behavior
  while still being real `addEventListener` underneath.
- **No `preventDefault()`/`cancelable`** — Lynx's touch/tap/longpress events
  cannot be canceled by app code; built-in behavior suppression (e.g. scroll
  panning) goes through a separate gesture-arbitration API instead.
  **Decision (2026-08-17, superseding an earlier one): do not expose
  `preventDefault()` at all.** It was implemented on 2026-08-16 and removed
  the next day. Nothing on the dispatch path can produce a cancel — the
  Element PAPI cannot, because the worklet runtime has neither the method nor
  a bit for it — so the earlier "expose it for web-platform-code
  compatibility" decision bought a boundary with no producer. Recognizers stay
  keyed off the gesture arbitration API, which is also what will drive the
  one surviving suppression seam, `InputEvent::default_prevented`.
- **`tap` suppressed by `longpress`** — Lynx hardcodes "tap doesn't fire if
  longpress was consumed earlier in the same touch sequence," a global rule
  with no DOM equivalent. **Decision: derive this from gesture-recognizer
  arbitration** (a `waitFor` relationship between built-in `Tap`/`LongPress`
  recognizers) instead of a hardcoded cross-event rule, so apps that don't
  use the gesture API don't inherit an invisible suppression.
- **`tap`/`longpress` synthesis semantics (user ruling 2026-08-21,
  implemented in `crates/bobcat-core/src/paint/gesture.rs`)** — `tap` fires at
  release with the native movement-cancel rule; `longpress` fires at a press
  held past 500ms (the native default on every platform). The recorded
  deviations inside that ruling:
  - **Slop shape is radial, 50px.** Lynx's Android implementation compares
    per-axis and its own source comments name the radial comparison as the
    intended behavior (iOS ships radial at 45pt); we implement the intent,
    radially, at the Android/Harmony default of 50. The page-config knobs
    (`tapSlop`, `longPressDuration`) are not wired yet — every view uses the
    defaults.
  - **The long-press "consumed" gate is name-level, not per-chain.** Lynx
    suppresses `tap` when the `longpress` walk found a handler anywhere in
    the response chain; this engine's presenting side only knows which names
    have listeners somewhere in the document (its own lock-free replica of
    the main thread's index), so a `longpress` listener
    on an unrelated element also suppresses a sequence's `tap`. Per-chain
    precision arrives with a presenting-side per-node index.
  - **The gate is one routing pass stale, and loses rather than replays
    (2026-08-31).** The replica is resynced at the top of each routing or
    gesture pass and then left alone, so a registration the main thread has
    not yet published — or published after this pass drained — is invisible
    until the next pass: an event of a freshly registered name is filtered
    out rather than queued, and a `longpress` registration racing its own
    deadline may land on either side of it. Accepted in exchange for a
    presenting side that takes no lock on the pointer path.
  - **Single-finger only, all pointer kinds.** A second concurrent pointer
    cancels synthesis for the whole overlap (Lynx's single-finger `tap`
    gate, with `enableMultiTouch` unimplemented); mouse and pen sequences
    synthesize like touch, because this engine's embedders feed all three
    through one seam and the web target's `tap` is the browser `click`,
    which mice produce.
  - **The web target's `tap` is the browser `click` renamed** (no slop rule,
    no `longpress` at all, no gesture API — `web-core` limitations, not Lynx
    semantics). This engine implements the native semantics; matching
    web-core's degradations was rejected.
  - **`click` is not synthesized** (Android and the fragment path emit it
    beside `tap` with re-targeting at release); no consumer yet.
  - **`timestamp` is milliseconds on the view's timeline epoch**, which is
    this engine's time origin — web-core's time-origin semantics, and DOM's
    for `Event.timeStamp`. Native Lynx reports epoch milliseconds instead.
    Every dispatched event carries one, taken from the clock reading of the
    pass that decided it: an input's *arrival* (so a due `longpress` flushed
    ahead of that input and the `tap` synthesized after it share the reading)
    or the gesture tick's own `now`. `params` rides beside it on every event
    as a fresh empty object; web-core fills it for `transition*`/`animation*`
    events alone, neither of which this engine dispatches.
  - **Touch events (2026-09-17).** `touchstart`/`touchmove`/`touchend`/
    `touchcancel` are dispatched beside the raw pointer events, and each
    native-versus-web-core conflict was resolved as follows.
    - **Multi-touch is always on** (W3C and web-core). Native's
      `enableMultiTouch` defaults to off — one finger's worth of lists — and
      the flag is unimplemented here; so is the uid-keyed map payload native
      produces in its multi-touch mode.
    - **`touches` excludes the finger that just lifted** (W3C and web-core):
      the list is every finger still down *after* the event. Native's
      single-finger mode keeps the lifted one in it.
    - **Per-touch `x`/`y` are lynx-view-local** (web-core), not native's
      element-local values, and `clientX`/`clientY` equal `pageX`/`pageY`
      equal them: one viewport CSS pixel space, the one `InputEvent::position`
      is already in.
    - **`identifier` is the host's pointer id**, which is what the embedder's
      own feed names a finger by.
    - **The `{x, y}` detail at the last finger's `touchend`/`touchcancel` is
      that finger's point** (native). web-core yields the number `0` there, by
      accident of a `??` over an empty `touches`; that is not reproduced.
      While any finger is still down the detail is `touches[0]`'s point, which
      is web-core's rule.
    - **Recorded gaps**, none of them a decision: no `screenX`/`screenY`, no
      `radiusX`/`radiusY`, no `force`, no `rotationAngle`.
    - **Touch and pen produce touch events, a mouse does not** (both
      references agree), which is deliberately unlike the `tap` ruling above,
      where every pointer kind synthesizes.
    - **A scroll claim sends no `touchcancel`** and `touchmove` goes on
      flowing while the drag recognizer scrolls (native; browsers likewise
      keep sending `touchmove` to a scrolling page). Only
      `PointerPhase::Cancel` produces `touchcancel`.
- **`pointer-events: none` hit-test fall-through** — Lynx falls through to
  the *next sibling* under the point; W3C says the element (and normally its
  subtree) becomes fully transparent to hit-testing, continuing the search
  as if it weren't in the tree at all. These can produce different hit
  targets in overlapping-sibling layouts — needs an explicit implementation
  choice.
- **Nested-scroll coordination** (`scroll-forward-mode`/`scroll-backward-mode`,
  capture-phase scroll interception, fling handoff ordering) has no W3C
  equivalent; the nearest partial analog is `overscroll-behavior`, which only
  controls *whether* scroll chains past a boundary, not consumption order or
  rubber-band restore semantics. Web-core today just relies on uncontrolled
  browser default scroll-chaining — native Lynx's actual coordinated model is
  the real spec to match.

## CSS selectors, cascade & at-rules (see [css-selectors-cascade.md](css-selectors-cascade.md), [css-at-rules.md](css-at-rules.md))

- **Deprecated CSS properties/values are dropped, not implemented** — the
  lynxjs.org API index is the authority (user decision, 2026-07): the style
  engine implements only non-deprecated properties and values. Concretely:
  `linear-orientation`, `linear-gravity`, `linear-layout-gravity`,
  `linear-cross-gravity` (modern API = standard
  `justify-content`/`align-items`/`align-self`), `grid-column-span`/
  `grid-row-span`, and `linear-direction`'s legacy
  `vertical`/`horizontal(-reverse)` value spellings do not parse. Note that
  web-core today still *accepts* these from old bundles — bundles authored
  against deprecated APIs will lose those declarations here. Intentional.
- **CSS inheritance** — native Lynx gates *all* property inheritance behind
  `enableCSSInheritance` (default **off**, allowlist when on;
  [css-text.md](css-text.md) recommends replicating that gate). But the
  compat target `web-core` **ignores that flag entirely** (zero references
  in `lynx-stack/packages/web-platform/`): it runs on the browser's
  always-on W3C inheritance and reproduces the visible Lynx behavior with
  targeted UA-sheet resets (`x-text { color: initial }` +
  `x-text > x-text { color: inherit }` in `web-elements`).
  **Decision (user-confirmed, 2026-07): web-core parity** — lynx-vello uses
  stylo's standard always-on inheritance plus the replicated UA resets. The
  native-Lynx gate is documented as a config-gated fallback design
  (doctored parent `ComputedValues`, allowlist mask) but not implemented.
  Custom properties inherit unconditionally in both worlds.
  **Landed 2026-08-21** in `bobcat_core::tree`'s UA sheet, which had carried
  the decision unimplemented until then — until it landed an ancestor `view`'s
  `color` really did reach the text under it. The port is `color: initial` on
  `text`, `color: inherit` on `text > text` and `text > wrapper > text`, and
  the `text > *` child suppression the same block relies on. Two consequences
  worth knowing: the reset stops a Lynx **text gradient** at the text root
  exactly as it stops a solid color (the fork's `color` holds either, and
  `inherit` carries the gradient into a nested run intact); and a `view` or
  `image` that opts back out of the suppression becomes a plain flex box,
  where web-elements can hand it `inline-flex`/`contents` inside a real inline
  formatting context — the same no-IFC substitution already recorded for a
  nested `text`.
  A tag this engine has no rule for (anything but `wrapper`, `view`, `image`,
  `text`, `raw-text`) generates no box inside a `text`, which is the literal
  `x-text > * { display: none }` behavior and not a narrowing of it.

- **`:is()`, `:where()`, `:has()`, `:nth-child()` family, `:first-child`/`:last-child`/`:only-child`/`:empty`**
  — all are *parsed* by Lynx's selector grammar but have **no matcher case at
  runtime** (always false). This is a bigger gap than a values mismatch: the
  grammar exists, the semantics don't. `:where()` in particular is load-bearing
  — it's the exact mechanism `web-core` uses for CSS-module scoping — so
  lynx-vello needs it working natively regardless of ReactLynx author usage.
  **Decision (user-confirmed, 2026-07-11): full stylo matching** — everything
  that parses, matches per spec; the native matcher's gaps are not
  replicated. See [docs/style-assumptions.md](../style-assumptions.md) §A.3.
- **`::before`/`::after` + `content`** — entirely unimplemented in the native
  engine (no box generation, no `content` property at all); only works on the
  web target via passthrough to the real browser. lynx-vello must explicitly
  decide whether to add real support (W3C-correct) since there's no native
  Lynx behavior to fall back to.
  **Updated decision (user-directed, 2026-09-12): element text content.**
  String and untyped `attr()` content on primary styles replace rendered
  children inside text paragraphs, preserving the DOM. `text[text]` and
  `raw-text` use UA CSS, without custom-element reflection. This explicit
  extension supersedes text-only `::before`; both before/after rendering,
  generated boxes and non-text content remain deferred. See
  [docs/style-assumptions.md](../style-assumptions.md) §A.4.

- **`@media` queries** — the C++ engine has a complete, recently-added,
  spec-modeled evaluator, but it is **wired only into the native `.lynx.bundle`
  pipeline**. The `.web.bundle` wire format lynx-vello actually decodes has
  **no `@media` representation at all** (confirmed: upstream `RuleType` and
  our decoder's `RuleKind` each have exactly three corresponding variants in
  `lynx-stack`'s encoder and our decoder). Any `@media` block in ReactLynx-for-web
  source is dropped today. This is the single biggest at-rule gap: decide
  whether to (a) match today's web-bundle behavior (media queries never
  apply) or (b) extend the format, using the C++ engine's evaluator model as
  the behavioral reference either way.
  **Decision (user-confirmed, 2026-07-11): (b) — wire stylo's `@media`
  evaluation now**, ahead of the wire format, with the C++ NG evaluator as
  the behavioral reference; the format extension itself stays future work.
  See [docs/style-assumptions.md](../style-assumptions.md) §C.10.
- **`@supports`** — parser and evaluator exist in the C++ engine but have
  **zero production call sites** (confirmed by grep); `selector()`,
  `font-tech()`, `font-format()`, and `at-rule()` all unconditionally return
  false. Not a quirk so much as a shipped-but-dead subsystem — lynx-vello
  should decide whether to actually wire it up rather than copy the "always
  false" behavior.
- **`@font-face` nested in `@media`/`@supports`/`@layer`** — the native
  engine parses then silently discards these; the web-bundle build tool
  drops them even earlier, with no trace. Neither is spec-compliant CSS
  Cascading & Fonts behavior; pick the real spec as the target since there's
  no working native behavior worth preserving here.
- **A top-level `@font-face` is parsed, mounted, and then completely inert**
  — this one is ours, not Lynx's. The path runs end to end: the decoder
  surfaces `PreparsedRule::FontFace`, `bobcat_core::style::lower` turns it
  into a rule through `Document::build_font_face_rule`, `dom` parses the
  descriptor block with stylo's own `parse_font_face_block`, and
  `append_rules` mounts the result in the stylist. Nothing then reads it.
  Fonts reach Parley by exactly one route — the embedder listing `FontBlob`s
  in the `ViewSources::fonts` it builds a view from, which `hughie`'s
  `TextContext` passes to Parley with no alias, so the families come from
  inside the font file —
  and family matching never consults a font-face rule. So `src` is never
  fetched, a `font-family` descriptor never names anything, and `font-display`,
  `unicode-range`, and the descriptor-level weight/style/stretch ranges have no
  effect at all. A sheet declaring a webfont costs a parse and a rule slot and
  changes no glyph; text renders in whatever the embedder registered, or the
  system fallback. Recorded, not fixed: making it real needs a font *loader* —
  a fetch of the `src` list with format/tech negotiation, a load state machine
  behind `font-display`, and a Parley registration keyed by the descriptor
  family rather than the file's own — which is a resource-pipeline feature,
  not a style-engine one. Until then, treat `@font-face` in a bundle as
  something that will silently do nothing.

## Components (see [components.md](components.md))

- **`<image>` is never sized by its bitmap**, unlike the `<img>` it resembles.
  Native gives the tag no platform layout node unless it carries `auto-size`
  (`LayoutContext::NoNeedPlatformLayoutNode`'s table is literally
  `{"image": {"auto-size"}}`), which leaves starlight measuring it as a
  childless leaf — `LayoutObject::UpdateMeasureWithLeafNode` writes the
  constraint on a definite axis and **zero** on every other one — and no
  intrinsic ratio softens that, since `SL_DEFAULT_ASPECT_RATIO` is `-1.0f`.
  web-core reaches the identical result through the browser with
  `contain: strict` on `x-image`. So an unsized `<image src>` renders as
  nothing in *both* references, while this engine's replaced-content path
  would size it from its pixels for free.
  **Decision (user-confirmed, 2026-09-06): match Lynx** — the standards
  policy's second bucket forbids "improving" a Lynx-only feature toward the
  W3C feature it resembles, and both references agree. Landed as
  `contain: size` in `bobcat_core`'s `tree::image` UA rules, with
  `hughie::compute_leaf_layout` suppressing the natural aspect ratio under
  size containment so a single authored axis cannot derive the other.
  Two consequences worth knowing: `contain` is a property Lynx has no
  equivalent of at all, so an author's `contain: none` can switch this off in
  a way no Lynx target permits (accepted — it is also exactly how
  `image[auto-size]` is written, which is why the UA declaration is not
  `!important`); and the same rule applies to an `<image>` written inside a
  `<text>`, which follows native (an inline image is sized from its own style)
  rather than web-core, which erases the host with `display: contents
  !important` and promotes the shadow `<img>` in its place.
  Deliberately **not** ported from `x-image.css`: `contain`'s
  layout/paint/style bits, `flex-direction: row !important` and the alignment
  triple (all scaffolding for a shadow `<img>` this engine does not have).
- **`padding` on an `<image>` inside a `<text>` is a silent no-op**, and it is
  a no-op in both references too — just by a different mechanism, which is why
  reproducing it costs a UA-origin `!important`. In web-core the authored host
  element has no box at all: `x-text > x-image`, and the `inline-text >`,
  `inline-truncation >` and `lynx-wrapper` variants of it, are
  `display: contents !important`
  (`packages/web-platform/web-elements/src/elements/XText/x-text.css:69-82`),
  and the box that lands on the line is the shadow `::part(img)`, assembled by
  inheriting `width`, `height`, `border`, `border-radius`,
  `background-color`, `vertical-align`, `object-fit`, `flex`, `align-self` and
  `margin` — a list `padding` is not on (`:120-135`; the only
  `padding: inherit` in web-elements belongs to `x-image[auto-size]::part(img)`,
  an attribute this engine does not implement). Native is split:
  `TextLayoutTextra::HandleInlineImageProps`
  (`core/renderer/ui_wrapper/layout/textra/text_layout_textra.cc:448-537`),
  Android's `InlineImageSpan` and iOS's default shadow-node path size the image
  from the specified width and height plus the four margins and read `padding`
  nowhere, while Harmony and iOS's layout-in-element path measure it as a
  starlight leaf whose border box is floored at padding plus border
  (`core/renderer/starlight/layout/layout_object.cc:515-519`). Here the
  authored `image` *is* the box, so under the Lynx `box-sizing: border-box`
  default a `width: 22px; padding-left: 50px` image would floor its border box
  at 50 and advance the line by 50 where web-core advances by 22.
  `text > image, text > wrapper > image, inline-text > image, … { padding: 0
  !important; }` in `bobcat_core`'s `tree::text` UA rules is the cascade
  spelling of the erasure, and it is `!important` for the same reason
  web-core's is: a normal declaration loses to the author's own `padding`.
  **Consequences to know:** an author's `padding` — or an inline one, or one
  written through `setNativeProps` — on an inline image is dropped without a
  diagnostic, and the computed value reads back `0` where web-core's host
  element still reports the authored value (web-core's `x-image` keeps its own
  computed style; only its *box* is gone). `margin` is untouched, because the
  shadow part inherits it and it does reach the line in both references. A
  `view` inside a `text` is untouched too: web-core leaves that one a real
  `inline-flex` box with its padding intact. This is the fourth
  `!important` in the UA sheet and the second argument
  [docs/style-assumptions.md](../style-assumptions.md) §D.15 admits;
  `the_ua_sheet_is_important_free_apart_from_the_text_block` pins the set.
- **`<image auto-size>` is sized the way both references size it.** The
  attribute lifts the rule above — `image[auto-size]:not([auto-size="false"])
  { contain: none; max-width: 100%; max-height: 100%; }` — and the box is then
  an ordinary replaced flex item. Read alone, native's measure functions look
  as if they disagree with web-core: Android's `AutoSizeImage.measure`
  (`platform/android/.../image/AutoSizeImage.java:105-156`) and iOS's
  `measureNode:` (`LynxUIImage.mm:1992-2043`) take the natural size whenever
  neither axis is exact, and never grow a bitmap on an at-most axis. But
  starlight hands a stretched item an *exact* cross constraint before it
  measures it: in a `nowrap` flex container with a definite cross size, an
  `align-self: stretch` item with an auto cross size and no auto cross margins
  is measured with `OneSideConstraint::Definite`
  (`core/renderer/starlight/layout/flex_layout_algorithm.cc:132-146` for the
  flex base size, `:335-345` for the hypothetical cross size), and linear
  layout does the same (`linear_layout_algorithm.cc:182-185`). Both measure
  functions then take the exact axis as given and derive the other through the
  bitmap's ratio, capped by the at-most constraint on it. That is
  css-flexbox-1 §9.8 followed by the ratio transfer and `max-*: 100%`, which
  is what web-core's shadow `<img>` gets from the browser
  (`x-image.css:55-81`). Walking the six parents of
  `auto_size_sizes_the_box_from_its_bitmap` through native's code gives the
  same six sizes Chrome renders for web-core, so no conflict is recorded here.
  Derived by reading `lynx/`, not by running a native build.
  Two smaller records under the same attribute. web-core makes `mode` and
  `blur-radius` inert under `auto-size` (a side effect of the `::part(img)`
  rules its `display: contents` leaves unmatched, not a decision); native keeps
  them live and so does this engine, since nothing couples them. And a
  `placeholder` can size an `auto-size` box here, because `dom` takes the
  natural size from whichever bitmap is drawn — iOS and web-core do the same,
  Android sizes from `src` alone.
- **`<image mode>` loses to author CSS here and wins in web-core.** The three
  modes that are not `fill` are UA attribute rules
  (`image[mode="aspectFit"] { object-fit: contain; }` and its two siblings),
  so a page's own `object-fit` outranks them — the standing PR #261 gave the
  `text-overflow` attribute. web-core's `x-image[mode=…]` is (0,1,1) in an
  *author*-level sheet, so there it beats a page's class rule. Accepted.
  `scaleToFill`, an unknown value and no attribute at all are the initial
  `fill`, which is Android's (`LynxImageManager.getMode`) and Harmony's
  (`ui_new_image.cc:220-231`) fallback; iOS's converter falls back to
  `aspectFill` instead (`LynxUIImage.mm:2063-2081`), a native-internal
  disagreement this engine resolves toward Android/Harmony and web-core.
  `center` is `object-fit: none` with the initial `object-position: 50% 50%`,
  which is Harmony's `ARKUI_OBJECT_FIT_NONE` exactly and web-core's centred,
  unscaled, host-clipped `<img>` exactly; one source pixel becomes one CSS
  pixel, which is web-core's basis — Android maps a source pixel to a dip and
  iOS to a point.
- **`<image blur-radius>` blurs the whole element, and a unitless value is
  dropped.** The attribute becomes a `filter: blur(…)` presentational hint over
  the raw attribute value, which is web-core's grammar (it writes the value
  into `--blur-radius` and lets `blur(var(--blur-radius))` judge it). So a
  unitless `blur-radius="10"` is invalid CSS and blurs nothing, while Android
  reads it as physical pixels (`UnitUtils.toPxWithDisplayMetrics`'s final
  `Float.parseFloat`) and blurs — **following web-core**. Since PR #273 a
  `filter: blur()` paints as an offscreen bake of the whole element, so this
  hint blurs the element's background and border along with its bitmap, and
  its ink overflows the box by 3σ, where both references blur the bitmap alone
  and keep it inside the box (web-core's filter sits on the shadow `<img>`
  under the host's `overflow: clip`; native post-processes the bitmap).
  **Decision (user, 2026-09-20): keep the host-level hint in this change**;
  bitmap-only blur is a follow-up — an engine-internal declaration the
  reflection writes instead of `filter`, and a paint walk that opens the blur
  bracket around the replaced draw alone, clipped to the box.
- **`<image>`'s `load` and `error` fire for `src` alone.** Both are web-core's
  events — `load` detail `{width, height}` from the intrinsic pixel size,
  `error` detail `{}`, both non-bubbling (`XImage/ImageEvents.ts:44-72` over
  `commonEventInitConfiguration.ts`) — dispatched per the 2026-09-17 ruling.
  What differs is what a *placeholder* does. web-core has one inner `<img>`
  and uses the placeholder as both its initial `src` and its error fallback
  (`XImage/ImageSrc.ts:28-31, 54-60`), so a placeholder that loads fires the
  host's `load`; here the two sources are concurrent requests under the native
  model the same ruling chose, and
  `dom` has no variant naming a placeholder's outcome, so neither its load nor
  its failure is an event. Native agrees with this engine: Android's
  `mPlaceHolderListener` (`platform/android/.../image/LynxImageManager.java:416-437`)
  sets the drawable on success and does nothing at all on failure, while only
  the source's listener reaches `onImageLoadSuccess`/`onImageLoadError`.
- **`error` carries `{}`, where native carries a reason.** Native's detail is
  `{errMsg, error_code, lynx_categorized_code}`
  (`ImageErrorCodeUtils.checkImageExceptionCategory`, buckets 1000s/1100s/
  1200s); web-core's is the empty object a browser's `error` event leaves it,
  and that is what this engine emits. It is not only a compatibility choice:
  nothing below this layer produces a reason at all, since a failure reaches
  the engine as `ImageReports::failed(source)` with no category, message or
  code. Adding the native fields would mean inventing them here.
- **A request is issued even for a 0x0 box, so a `load` fires where native
  fires none.** Binding a `src` is what asks the host for it, whatever the
  element measures — and `<image>`'s own rule is that an unsized box is 0x0
  (the first entry in this section). Android refuses the fetch outright in
  that state: `LynxImageManager.updateImageSource`
  (`platform/android/.../image/LynxImageManager.java:886-913`) leaves
  `needRequest` false when the view has no size, no pre-fetch size and no
  `auto-size`, so an unsized `<image src>` there never loads and never
  reports. web-core's inner `<img>` carries the `src` whatever the host
  measures and fires `load` as this engine does. **Following web-core**, which
  is also the cheaper contract to state: whether an element asks for its
  source does not depend on layout.
- **A `load` that settles while the element is detached is still delivered.**
  web-core buffers it and replays it from `connectedCallback`
  (`XImage/ImageEvents.ts:44-63`), which is a workaround for its host and its
  inner `<img>` being two objects. Here an event path is computed for a
  detached target exactly as the DOM standard specifies — it ends at the
  topmost ancestor — so the handler on the element itself runs whether or not
  it is connected. Accepted: a compiled ReactLynx card writes `src` and
  appends in the same render, so the two differ only for a card that holds an
  element out of the tree across a turn.
- **`<blur-view>`'s `blur-radius` is a CSS length here, where web-core and iOS
  read a number and throw the unit away.** The attribute is the whole of the
  component: it is reflected into a `backdrop-filter: blur(…)` presentational
  hint (`crates/bobcat-core/src/main/tree/blur_view.rs`), installed under both
  the native tag `blur-view` and the tag a compiled `.web.bundle` writes,
  `x-blur-view` — web-core registers only the latter
  (`web-elements/src/elements/XView/XBlurView.ts`) and native only the former
  (`LYNX_LAZY_REGISTER_UI("blur-view")`, `@LynxBehavior(tagName =
  ["blur-view"])`, `registry.cc`'s `map["blur-view"]`). The four references
  disagree about units: web-core's `BlurRadius.ts` writes
  `:host { backdrop-filter: blur(${parseFloat(newVal)}px) }` into a
  per-instance shadow `<style>`, so `20rpx` becomes `20px`; iOS takes
  `blur-radius` as a `LYNX_PROP_SETTER(…, CGFloat)` whose conversion is
  `LynxConverter`'s `toCGFloat`, i.e. `[value doubleValue]`, dropping the
  suffix the same way; Android runs the string through
  `UnitUtils.toPxWithDisplayMetrics` (`rpx`, `ppx`, `px`, `%`, `rem`, `em`,
  `vw`, `vh`); and Harmony parses it with `CSSStringParser::ParseLengthTo`
  before handing it to ArkUI's `NODE_BACKDROP_BLUR`.
  **Decision (user, 2026-09-20): unit conversion is the styling engine's job**,
  so the attribute's text enters the cascade as a CSS length — which is
  Android's and Harmony's behavior, and gives `rpx`, `em`, `vw` and `calc()`
  one resolution path instead of a second, component-local one. A bare number
  is the one rewrite: it is not a CSS length and every reference reads it as
  pixels, so `25` is reflected as `25px`, from the *parsed* number rather than
  by gluing `px` onto the text (`5.` is a `parseFloat` number and not a CSS
  one, and `inf`/`NaN` must never reach a declaration). Consequences to know:
  a page that wrote `blur-radius="20rpx"` expecting web-core's 20 *pixels* gets
  ~10.47 px on a 393 px-wide viewport; a value the grammar rejects (`abc`,
  `-4px`) clears the hint rather than keeping the radius before it, which is
  why the reflection clears before it sets — `set_presentational_hint` is a
  no-op on an invalid value; and author CSS or inline style naming
  `backdrop-filter` outranks the attribute, as web-core's `:host` rule loses to
  author styles.
  Two smaller divergences ride along. **The element's own backgrounds and
  borders paint**, which is web-core (the host element is an ordinary box
  around a shadow `<slot>`) and not iOS, where `LynxUIBlurView` overrides
  `background`, `background-color`, `background-image`, `background-size`,
  `background-position`, `background-repeat`, `background-origin`,
  `background-clip` and `background-capInsets` with empty bodies because the
  view *is* a `UIVisualEffectView`. And **every platform-only property is
  ignored** — `blur-effect` (the iOS `light`/`dark`/`extra-light`/`glass`/
  `glass-container` system tint), `blur-sampling`, `spacing`,
  `android-capture-target`, `enable-auto-blur`,
  `experimental-update-blur-radius`, `ios-user-interface-style`,
  `glass-interactive`, `glass-tint-color`, `glass-style` — which is what
  web-core does too, since `BlurRadius` observes `blur-radius` alone.
  On the cascade side both tags join `tree::ua_sheet`'s container list and its
  `defaultOverflowVisible` rule, following native, where `LynxUIBlurView`
  extends `LynxUIView`. web-core is narrower: `x-blur-view` is in
  `linear.css`'s common block and in the linear *item* rules, but in neither
  the `--lynx-display-toggle` list `defaultDisplayLinear` drives nor the
  `[lynx-default-overflow-visible="true"] x-view` escape, so a browser gives it
  a row flex box that always clips whichever way the two switches are set. That
  is the same shape of divergence `scroll-view` and `list` record above, and
  for the same reason: a per-tag exception would have to be `!important`, which
  [docs/style-assumptions.md](../style-assumptions.md) §D.15 forbids in this
  sheet.
- **Almost every built-in component exposes a bespoke imperative JS method
  surface** (`invoke()`-based RPC: `scrollTo`, `getScrollInfo`,
  `setInputFilter`, `startAnimate`, etc.) instead of standard DOM
  properties/methods — this is a systemic pattern, not a per-component
  quirk; see the full table in `components.md` and `js-runtime.md` before
  assuming any one component's API is a one-off.
- **`loadLazyBundle`** intentionally resolves synchronously on the fast path,
  which **violates the Promise/microtask spec** (`.then` must be async) to
  make first-screen render synchronous. This is the reverse of the usual
  policy: matching ReactLynx behavior here means *replicating* a spec
  violation on purpose. Needs an explicit decision, not a default policy
  application.
- **Form constraint-validation** (`checkValidity()`, `required`, `min`/`max`/`step`,
  `ValidityState`) is **absent entirely**, not just non-standard — no
  ReactLynx app depends on it existing. Document as intentionally
  unimplemented rather than a gap to fill.
- **`x-list` virtualization** is CSS `content-visibility: auto` +
  `contentvisibilityautostatechange` on the web target (not true cell
  recycling); native Lynx has three independently-implemented list backends
  with overlapping method names. No single "the" behavior to copy — treat
  native Lynx's virtualized-position-map model as the reference since that's
  what `list.scrollToPosition`/`getVisibleCells` actually need.
- **`scroll-view` and `list` take their display mode from
  `defaultDisplayLinear` here; web-core gives neither of them that**, for two
  different reasons. `scroll-view` is pinned linear:
  `scroll-view { --lynx-display: linear !important }` in `scroll-view.css`,
  which the page-config sheet's `[lynx-default-display-linear=false] *` cannot
  outrank, so on the current container-query path a page built with the flag
  off still gets a linear scroller in a browser. `x-list` is the opposite — it
  is in `linear.css`'s common block but in neither the list that enables the
  `--lynx-display-toggle` nor the one that swaps
  `flex-direction`/`flex-wrap`/`justify-content` by mode, and `x-list.css`
  writes `display: flex; flex-direction: column` outright, so a list is a flex
  container in a browser whichever way the flag is set. **Decision (user,
  2026-08-21): the UA sheet treats a scroller's display like a view's** — one
  rule, one switch, and no per-tag exception. That is deliberate, and the
  alternative is not a different UA rule: `!important` is exactly what
  [docs/style-assumptions.md](../style-assumptions.md) §D.15 forbids in this
  sheet (web-elements' defaults are author origin, ours are user-agent origin,
  and an important UA declaration would outrank author `!important` instead of
  losing to it). §D.15 gained one exception on 2026-09-03 —
  `display: -lynx-text` on `text`/`inline-text` — and this row is the case
  that exception deliberately does **not** cover: a scroller's display is a
  *default*, gated on page config by web-core itself, where the only question
  is which layout mode the box gets; a text's inline-ness is a *structural
  invariant* Lynx establishes at tree-mutation time and no author CSS can
  undo. So §D.15 still hands forced modes to the layout engine's element
  policy — which is the piece not yet built, and what this row records. It
  shows only with `defaultDisplayLinear` off for `scroll-view` and only with it
  on for `list`, and with the main axis pinned the same way in both modes what
  is left to differ is how a child is sized —
  `linear-weight`/`linear-weight-sum` against
  `flex-grow`/`flex-shrink`/`flex-basis` — and that a linear container never
  wraps.
- **A `scroll-view` is a scroll container and an axis, and nothing else** —
  the UA sheet gives the tag the container block (border box, configured
  display), the scrolling axis with its clipped cross axis, the matching main
  axis in both display modes, and `enable-scroll="false"`. The rest of what
  `scroll-view.css` carries is absent: fading edges, scrollbar visibility
  (`enable-scrollbar`/`scroll-bar-enable`), scroll snapping
  (`item-snap`/`paging-enabled`), and the threshold-observer parts behind
  `scrolltoupper`/`scrolltolower`. Several of those are shadow-part machinery
  that a UA sheet cannot express alone; they belong with the component work,
  not with the tag defaults.
- **A `list` is that plus virtualized, placed cells — and nothing else yet**
  *(2026-09-21)*. `crates/bobcat-core/src/main/tree/list.rs` carries the same
  axis rules written against `scroll-orientation`, and on top of them
  `x-list.css`'s three real mechanisms: `container-type: size` makes the list
  a size query container, `list-item` is
  `content-visibility: auto; contain: layout paint` with
  `contain-intrinsic-size: none auto
  attr(estimated-main-axis-size-px px, 100cqh)`
  (the horizontal variant swapping the axes and the unit),
  `recyclable="false"` opts a cell out, and `list-type` selects `display: grid`
  or `display: grid-lanes` over `repeat(var(--list-item-span-count), …)` with
  `full-span` as a placement rule. `span-count`/`column-count`,
  `sticky-offset` and a cell's `estimated-main-axis-size-px` reach the cascade
  through UA `attr()` declarations (2026-09-23); cell estimates require
  nonnegative numbers and their CSS fallback is the scrollport size. `sticky-top="true"` supplies sticky positioning, the
  nonnegative inherited offset and `z-index: 1`. Still absent from the sheet:
  horizontal sticky insets and `sticky-bottom`, `item-snap` /
  `paging-enabled` scroll snapping, the scrollbar rules, the threshold
  observers, `initial-scroll-index`, every list event and every list UI
  method — and cell recycling, which this engine does not do at all: a
  virtualized cell keeps its element and skips its contents, which is
  web-core's model too (`components.md` row 23).
- **List waterfall over `display: grid-lanes` — path assessment
  (2026-09-18).** The layout mode landed
  ([style-assumptions.md](../style-assumptions.md) §24); this records how far
  it can carry `<list list-type="waterfall">` and where it would not match.
  **The cell-delivery path is no longer a prerequisite**: a compiled ReactLynx
  `<list>` receives its children only through `__SetAttribute(list,
  "update-list-info", {insertAction, removeAction, updateAction})`
  (`lynx-stack/packages/react/runtime/src/snapshot/list/listUpdateInfo.ts:112`),
  which makes the host call the `componentAtIndex` filed by `__CreateList`
  (`.../snapshot/snapshot/list.ts:11-40`) and append the cell
  (`.../snapshot/list/list.ts:202`). web-core materialises every `insertAction`
  as a real DOM child inside a microtask, removing each `removeAction` child
  after handing it to `enqueueComponent`
  (`.../web-core/ts/client/mainthread/elementAPIs/createElementAPI.ts:461-499`);
  it keeps no virtualization of its own. This engine now implements that same
  algorithm over the `childElementIds`, `insertBefore` and `removeElement` host
  members (`packages/bobcat-element/src/element-papi.ts`, `updateListInfo`), so
  a list's cells are real element children under every layout mode and what is
  left here is layout and placement alone.
  **The placement rule native and web-core agree on**: shortest lane, exact
  float comparison, lowest lane index on a tie; a full-span item goes at the
  maximum of every lane and resets them all to that maximum plus its own
  extent. Native:
  `lynx/core/renderer/ui_component/list/staggered_grid_layout_manager.cc`
  (`std::min_element` over `end_lines`, ~:884-898; full span ~:524-549).
  web-core:
  `lynx-stack/packages/web-platform/web-elements/src/elements/XList/XListWaterfall.ts`
  (full span ~:107-136; shortest lane with a strict `<`, so the first minimum
  wins, ~:137-174). Note that
  `lynx/core/renderer/starlight/layout/staggered_grid_layout_algorithm.cc` is
  *not* that algorithm — see [css-layout.md](css-layout.md).
  **Where css-grid-3 §4.4 placement differs from it:**
  - (a) `flow-tolerance` defaults to `normal` = `1em`, so lanes within `1em`
    of the shortest count as equally short. `flow-tolerance: 0` removes this
    difference entirely.
  - (b) The auto-placement cursor. Among tied lanes grid-lanes takes the first
    one at or after the previous auto-placed item's end line; Lynx always takes
    the lowest index. No CSS value disables the cursor. Minimal case: two
    lanes, item heights `[100, 200, 100, X]`, gap 0 — Lynx puts `X` in lane 0,
    grid-lanes in lane 1. The hughie test
    `flow_tolerance_decides_which_lanes_count_as_equally_short` pins the W3C
    outcome for a three-lane example.
  - (c) Main-axis margins: all three differ, and native vs web-core is a
    conflict by itself. Native advances the lane by the margin box plus the
    leading gap (`GetDecoratedMeasurement`,
    `lynx/core/renderer/ui_component/list/list_orientation_helper.cc:59-66`)
    but sets the item's top to the lane position plus the gap only, so the
    leading margin never reaches the origin. web-core advances by
    `getBoundingClientRect()` — the border box, no margins —
    (`XListWaterfall.ts:103-105`) while writing the result into `left`/`top`
    on an absolutely positioned cell (`XListWaterfall.ts:177-188`,
    `x-list.css:270-272`), which positions the *margin* edge. grid-lanes tiles
    margin boxes in both respects.
  - (d) web-core only: a non-`list-item` direct child is `display: none`
    (`x-list.css:16-18`) yet the waterfall pass still walks it
    (`XListWaterfall.ts:94`) and advances a lane by the main-axis gap for it,
    because its rect is zero. And a `list-item` nested in a `lynx-wrapper` is
    not a direct child, so it is never positioned at all while
    `x-list.css:270-272` keeps it absolutely positioned. grid-lanes does
    neither: a `display: none` child takes no lane, and a `wrapper` — which
    this engine's UA sheet already makes `display: contents`
    (`crates/bobcat-core/src/main/tree/ua_sheet.rs`) — flattens into the item
    list through `LayoutTree::flattened_children`.
  - (e) Replaced children do not fill their lane by default. css-grid-3 §6.2
    sends grid-axis alignment through regular Grid, and css-grid-1 §6.2 makes
    `normal` leave a replaced box with a natural size at that natural size —
    so an `<image>` in a waterfall lane is laid out at its own pixels unless
    the lane carries `justify-items: stretch` or the item carries
    `width: 100%`. That is what a browser does with `<img>` in a grid too, and
    it is what web-core's own waterfall inherits, since its cells are real
    CSS boxes. A `<list>` UA rule here would have to supply the stretch the
    same way `x-list.css` does. See "Items with an intrinsic aspect ratio" in
    [css-layout.md](css-layout.md).
  **Mechanisms the path uses, built 2026-09-21**: web-core carries the
  column count as the custom property `--list-item-span-count`, set from
  `span-count`/`column-count`
  (`.../XList/XListAttributes.ts:33-39`), and writes `list-type="flow"` as
  real CSS Grid `repeat(var(--list-item-span-count), 1fr)`
  (`x-list.css:210-240`); here tag-scoped UA `attr()` rules set the registered
  property, with `span-count` taking precedence over `column-count`. The
  registration defaults to one, and the track count is clamped to at least
  one. Numeric attributes require their declared grammar: the former
  `parseFloat` prefix acceptance is deliberately removed (2026-09-23 user
  ruling). No `list` or `list-item` reflection component is needed.
  `full-span` is matched as a value, web-core's
  `[full-span]:not([full-span="false"])` (`x-list.css:294`), never as a
  presence test, because `__SetAttribute` stringifies every value, so
  `full-span={false}` arrives as `"false"`. A horizontal list maps to
  `grid-template-rows` **and resets `grid-template-columns: none`**, which
  `x-list.css:221-229` does not (and `:262-264`, the horizontal waterfall,
  only swaps a `flex-direction`, since web-core's waterfall is never a grid):
  grid-lanes reads the absence of a
  column template as the statement that the block axis carries the tracks
  (`crates/hughie/src/compute/grid/lanes.rs:535-547`), so leaving the column
  template standing would stack a horizontal waterfall downwards.
  `list-main-axis-gap`/`list-cross-axis-gap` do not exist here
  (`list_main_axis_gap_is_absent`, `crates/dom/tests/grammar_layout.rs`) and do
  not exist as properties in web-core either — its style transformer renames
  both to custom properties
  (`.../web-core/src/style_transformer/rules.rs:20-21`); an author writes
  `row-gap`/`column-gap`. The scroll extent
  needs nothing new: `crates/dom/src/scroll/mod.rs` derives it from the layout
  algorithm's `content_size`.
  One further span-count divergence: the UA default here is **1**, where
  `x-list.css:11` writes `0`. `repeat(0, 1fr)` is an invalid track list, so
  web-core's default would drop the whole declaration; web-core survives it
  because its own JavaScript rewrites the property, and its changelog records
  the same symptom from the other end ("list may only render only one column
  in ReactLynx", `web-elements/CHANGELOG.md:376-378`, lynx-stack PR #1280).
  **Decisions:**
  1. *The tie-break* — **decided (user, 2026-09-18): accept the W3C cursor.**
     A list waterfall laid out by `display: grid-lanes` places a tied item
     where css-grid-3 §4.4 places it, not where Lynx does; the difference in
     (b) above is a recorded deviation, and no engine-internal switch is added.
     `display: grid-lanes` stays W3C-correct for authors.
  2. *How the UA sheet selects it* — **decided (user, 2026-09-21):
     per-attribute UA `display` rules.** `list[list-type="waterfall"] { display: grid-lanes }` and
     `list[list-type="flow"] { display: grid }` are written as ordinary
     declarations in `crates/bobcat-core/src/main/tree/list.rs`, so an author
     `display` on the list overrides them — the UA sheet may not use
     `!important` ([style-assumptions.md](../style-assumptions.md) §D.15),
     and the one recorded exception to that was granted for a structural
     invariant rather than for a default. It *is* a per-attribute exception to
     the 2026-08-21 decision in the entry above, and deliberately so: that
     decision is about which of `linear`/`flex` a container gets from
     `defaultDisplayLinear`, and a `list-type` is the author naming a layout
     mode outright, the way `x-list.css:210-264` does. A list with no
     `list-type` still takes the page-config display like every other
     container. Recorded in
     [style-assumptions.md](../style-assumptions.md) §19.

## JS runtime & APIs (see [js-runtime.md](js-runtime.md), [accessibility.md](accessibility.md))

- **`lynx.createSelectorQuery()`/`NodesRef`** — modeled on WeChat Mini
  Program's batched `SelectorQuery`, not DOM `querySelector`: async,
  explicit `.exec()`, callback-based `invoke()`. This shape (async/batched
  query, not live synchronous DOM references) recurs across most of the
  JS-facing element API — treat it as the systemic pattern, not a one-off.
- **`invoke('boundingClientRect')` excludes transforms** — the two references
  disagree with each other. Native's own engine-side conversion
  (`core/renderer/dom/fragment/event/platform_event_target_helper.cc`) sums
  each ancestor's `Left()`/`Top()` plus scroll offsets under an explicit
  `TODO: add transform support`, and Android excludes them unless
  `androidEnableTransformProps` is passed; only iOS, which goes through
  UIKit's `convertRect:toView:`, folds them in, as does web-core, which calls
  DOM `getBoundingClientRect()`. **Decision: follow the engine path** — the
  rect is the untransformed border box the layout pass produced, so a rotated
  or translated element reports where it was laid out, not where it paints.
  `relativeTo`, `androidEnableTransformProps` and `iOSEnableAnimationProps`
  are not accepted at all; the `params` object is ignored.
- **The rect carries `id` and `dataset`** — native's result bundles both
  (`LynxUI.m`, `platform_event_target_helper.cc`); web-core's carries the
  geometry and the id only, because DOM `getBoundingClientRect()` has neither.
  **Decision: follow native** and include the typed `dataset` copy, which is
  what a card that measures a list row then reads its keys off the same answer
  expects. This is one of the places where the web default is not taken.
- **An unknown UI method answers 3, not 1** — the code table is shared
  (`lynx_get_ui_result.h`, web-core's `constants.ts`), but native reports the
  generic `UNKNOWN = 1` for a method its per-class registry has no entry for,
  while web-core reports `METHOD_NOT_FOUND = 3`. **Decision: 3** — web-core is
  the default resolution and the code is the one that actually names the
  failure. Node-resolution codes are unchanged: 2 for no match, 5 for a
  selector the query layer refuses.
- **Measurement and computed-style readback never flush** — neither
  `__InvokeUIMethod` nor `__GetComputedStyleByKey` runs style, layout or
  paint; they report the last completed pass and the styles it was computed
  from. **Not a deviation from either reference so much as a contract worth
  stating**, because it is observable: a BTS query is always current (the
  entry that queued it returned and its epilogue committed), while an MTS
  worklet that mutates and measures in one job reads the pre-mutation geometry
  until it calls `__FlushElementTree` — the order ReactLynx's own
  `Element.invoke` uses, since it flushes *after* the PAPI call. An element no
  pass has reached answers zeros for the rect and nothing at all for style.
  Within style, the CSSOM and Typed OM halves differ as the specs do:
  `__GetComputedStyleByKey` reports *resolved* values, so `width`, `height`,
  `margin-*` and `padding-*` come back as the used px of the last layout when
  the element has a box, while `__BobcatComputedStyleMap` reports computed
  values and is a per-call snapshot rather than the standard's `[SameObject]`
  live map. **Known gap:** shorthands are reported by neither — absent from
  the map, `""` by key — because Typed OM excludes them and the two references
  disagree on what a shorthand's text should be. Non-custom names match
  ASCII-case-insensitively, as stylo's own parse does; `--*` names match
  exactly, so `marginTop` is empty where `margin-top` answers.
- **`requestAnimationFrame`/timers** — signature-compatible with the W3C
  APIs, but callback timing is tied to Lynx's own frame/vsync pump (paused in
  background, no guaranteed cadence). Implement by driving our own frame pump
  rather than assuming browser rAF semantics transfer directly.
- **`lynx.SystemInfo` / `SystemInfo.pixelWidth`/`pixelHeight`** — no direct
  W3C equivalent (closest: `devicePixelRatio`/`screen.width`); Lynx exposes
  it as a static global snapshot rather than a live queryable API, and the
  Android implementation is documented as a "shared, polluted, process-global"
  value not maintained per-view — worth a cleaner per-view accessor in
  lynx-vello rather than copying that specific bug.
- **`lynx.requireModule`'s load of a path no bundle carries** — three
  choices in one API, all resolved toward web-core. There is **no fetch
  timeout**: web-core has none, and native's 5 s default is unreachable from
  `requireModule` anyway, its `loadScript` binding reading a timeout only from a
  *number* third argument where lynx-core hands it the whole `options` object
  (`js_app.cc:197-199`), so `options` is accepted and ignored. A load the host
  cannot answer carries **this engine's own `cannot load '<url>'` text**, not
  lynx-core's `load failed. path:…,entryName:…` (`app.ts` `_$executeInit`),
  because the host member underneath is where the failure is known. And **`Card`/`Component` are always
  in scope for a body the container carried**, where web-core omits the pair
  for a React card (`createChunkLoading.ts`): a body that does not name them is
  unaffected either way.
- **A compiled bundle's bodies evaluate once per realm; native re-evaluates
  per call** — every body a *BTS* container carries beyond its entry script is
  an ES module here (`PageSource`), and `lynx.requireModule`,
  `lynx.loadScript` and `nativeApp.loadScript` each `require` it by URL,
  synchronously, at the call that first asks for it. A URL is one module per
  realm, so a second call for the same path is answered from the evaluation
  the first one ran and only the `init` runs again. Native's
  `App::LoadScript` re-reads and re-runs a script per call (`js_app.cc`).
  **Decision: once per realm**, which is the ESM module map's own rule and the
  rule `lynx.requireModule`'s cache already followed. Observable where a body's
  top-level code has side effects it expects to repeat; the compiler's own
  chunks define and export, and are unaffected. A body that throws while
  evaluating is reported at the `requireModule` that reached it, which is where
  native reports it too. This covers bundle bodies only: a **named Lepus
  chunk** is not a module, and `__LoadLepusChunk` loads and runs it again on
  every call, as native does. **A lazy container's section is a body too**, so
  MTS's `lynx.loadScript(key, {bundleName})` evaluates it once per realm on
  that thread as well.
- **`lynx.fetchBundle` answers native's `{wait, then}` object, not web-core's
  Promise** — native's `ResponsePromise` (`lynx.cc` `FetchBundle`) is a host
  object with exactly `wait(seconds)` and `then(callback)`; `.then` returns
  `undefined`, so there is no chaining and no `catch`, and `options` is
  ignored. web-core's `fetchBundle` is a real Promise with no `wait` at all
  (`createMainThreadGlobalAPIs.ts`). **Decision: native**, because the
  compiled ReactLynx caller is written to it — `lazy-bundle.ts` calls
  `.wait(5)` for a `mode: 'sync'` import, and wraps the asynchronous path in a
  `new Promise` of its own rather than chaining — so a Promise here would
  break the synchronous shape outright.
- **`.then` on an already-settled fetch: inline on MTS, posted on BTS;
  web-core's is a microtask on both** — native's `LynxActor::Act` acts on the
  value being there, which on the main thread means running the callback then
  and there and on the background thread posting a task
  (`bts_runtime_mediator`). "Settled" is this realm holding the outcome,
  however it arrived: a `wait` that returned, or the Promise of the
  `bobcat:future` `Future` the fetch is — so `h.wait(5); h.then(cb)` runs
  `cb` at the `then` rather than through a delivery behind it. A callback
  registered while the fetch is still outstanding runs as a reaction of that
  Promise, which the realm owner's epilogue settles on a task of its own;
  native's BTS posts a task and its MTS posts to the Lepus thread, so that
  half is asynchronous on both. **Decision: native**, for the case this
  engine can still reach — but see the entry below: the case
  `rLynxPrepareLazyBundleMTS` actually depends on is a *repeat* fetch, which
  this engine no longer answers synchronously at all.
- **A repeat `lynx.fetchBundle` of a URL this view already fetched settles in
  the same job, through the fetcher's probe** — native answers one at once out
  of `TemplateAssembler::FindTemplateBundle`, so MTS's `.then` runs **inline**;
  web-core resolves from its own promise cache, so its `.then` is a microtask.
  **Decision: native**, and it is load-bearing rather than a nicety.
  `lynx.fetchBundle` is *a plain fetch* — a user ruling: apart from being
  waitable it behaves like `fetch(image_url)`, and `bobcat-core` contains no
  bundle-specific code at all — so nothing in the engine remembers a URL.
  What does is the **fetcher**: `ResourceFetcher::fetch_probe()` hands the
  realms a `Send + Sync` reader of what it has fetched, `fetchResource`
  consults it before requesting anything, and a hit answers `true` with no
  request, which makes the handle settled from the start.
  What a *microtask* answer would cost was measured before the probe existed,
  and is why it exists: `prepareLazyBundleMTS` is written to the inline answer
  ("`.then` will be a sync function since the bundle has been loaded in BTS"),
  so with an asynchronous one it returns having loaded nothing, the
  `callLepusMethod` reply resolves BTS's lazy import, BTS renders and sends
  `rLynxChange`, and MTS applies a patch naming a snapshot its own
  `main-thread` section has not registered yet —
  `Error: Snapshot not found: __snapshot_…`, which rides the reply back and
  ends the BTS Worker, 5 runs out of 5. With the probe, `react-lazy` and
  `react-lazy-sync` both pass 5 of 5
  (`crates/bobcat-source/tests/lazy_bundle.rs`). Only a *completed* fetch is
  remembered; a failure is not, as native caches none.
- **A failed `fetchBundle` carries the failure's text in `error_msg`; native
  leaves it empty** — native's failure path fills `code` and leaves
  `error_msg` empty (`lynx.cc`), and web-core names the key `errorMsg`
  instead. **Decision: native's key, this engine's value** — `error_msg`, so
  the compiled caller's `JSON.stringify(info)` diagnostic reads the same, but
  carrying the resource failure's own message, because an empty string makes
  every lazy-bundle failure indistinguishable and this engine has the text
  right there. `code` is native's: `0` fetched, `-1` the fetch failed, `-2` a
  `wait` timeout, with native's exact timeout message. `-1` is what the
  rejected `Future` becomes; the fetch itself never rejects past the handle.
- **`fetchBundle(...).wait(t)` takes a number of *seconds*, and refuses
  anything else** — native's JSI binding reads a number and throws otherwise;
  web-core has no `wait` at all. **Decision: native**, including the timeout
  record's exact text
  (`ResponsePromise wait timeout after <t> seconds for url: <url>`) and the
  fact that a timeout **cancels nothing**: the fetch goes on, and a later
  `wait` or `then` still sees its result. The seconds become milliseconds at
  the boundary, because the wait itself is `bobcat:future`'s `Future.wait`;
  `Infinity` seconds is that Future's "no deadline at all", and a number
  `Future.wait` refuses — `NaN`, or anything else that is not a number —
  throws there rather than answering a timeout record.
- **A `wait` *after* a `then` on one `fetchBundle` handle throws a
  `TypeError`; native allows the pair** — native's `ResponsePromise` holds a
  `std::shared_future`, so a handle can be waited on after a callback was
  registered on it. Here both members are one `bobcat:future` `Future`, and a
  `then` converts it into a Promise the realm's owner settles with a *job*;
  a job cannot run inside another job's wait, so a `wait` on a converted
  Future could only ever time out or hang and is refused outright.
  **Decision: this engine's structure**, because the refusal is what makes the
  two ways out exclusive rather than deadlock-prone — and no compiled
  ReactLynx path does both on one handle: `lazy-bundle.ts` picks `wait` for a
  `mode: 'sync'` import and `then` for the asynchronous one.
- **A lazy container's own `StyleInfo` is not applied at fetch** — native
  installs a lazy bundle's CSS only when the card asks for it, through
  `__LoadStyleSheet('CSS', bundleName)` and `__AdoptStyleSheet`, where
  web-core pushes the container's StyleInfo unscoped during
  `loadExternalBundle`. (Here the decision is the *installer's*, in
  `bobcat-source`: `bobcat-core` never sees the container at all.) **Decision: native**, because the compiled card
  already makes that call and web-core's extra push would mount the same rules
  twice — and unscoped, where per-component css-id scoping is not implemented
  at all here. A container's `config` is ignored too: page policy is the
  page's.
- **`NativeModules.<name>` for a module the host does not have** — native's
  `LynxJSIModuleBinding::get` answers `null`
  (`lynx_jsi_module_binding.cc:23`), while web-core builds a plain object out
  of `createNativeModules`, so a missing name is simply `undefined`.
  **Decision: `undefined`** — web-core is the default resolution, and every
  in-repo consumer tolerates either (compiled bundles probe with `?.`,
  explorer-lib guards on `typeof`). A method a module did not declare is
  `undefined` on both references, and so here. MTS `NativeModules` stays
  `undefined` altogether, as Lepus has no module binding.
- **Accessibility**: Lynx has **no implicit ARIA-like semantic
  roles/focusability** (nothing is focusable/announced unless explicitly
  opted in via `accessibility-element`), and `accessibility-traits` is a flat
  non-standard vocabulary mixing ARIA roles and states in one list, not the
  ARIA role/state separation. The legacy Android virtual a11y tree is also
  reconstructed from flattened hit-testing rather than DOM reading order.
  These are the systemic AT-model differences to design around, not
  individual prop quirks.

*(Full per-row detail, including ~250 more narrowly-scoped items — mostly
Lynx-only extension properties/APIs with no W3C equivalent to conflict with —
lives in the `W3C-compliant?` column of each linked file.)*
