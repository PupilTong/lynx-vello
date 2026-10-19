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
  encode window covers the new offset and the offset main adopts is not yet
  `recenter_due`. List `sticky-top`/`sticky-bottom`
  attribute rules remain a separate component integration task.
- **Loose text: anonymous items, and `raw-text` outside Lynx tags (user
  ruling, 2026-10-16)** — css-flexbox-1 §4 and css-grid-1 §6 wrap each
  contiguous sequence of a container's child text in an anonymous block
  container that is one item, and render nothing for a white-space-only
  sequence. `dom` builds those boxes (`crates/dom/src/tree/anonymous.rs`)
  for `flex`, `grid` and `grid-lanes` and, by ruling, for Lynx's `linear`
  and `relative`, which have no such rule of their own; each box is a
  `(block, flow)` block container laid out as a CSS inline formatting context
  (`crates/dom/src/layout/inline_flow.rs`), never a Lynx text
  paragraph. On the Lynx side a `raw-text` renders as such text inside a tag
  that is not a Lynx tag (`div`, `span`, any other name), directly or
  through one `wrapper` (`crates/bobcat-core/src/main/tree/raw_text.rs`).
  **Native:** a raw-text under a `view` is a virtual node, never laid out or
  painted (`lynx/core/renderer/dom/fiber/fiber_element.cc:4099-4131`); an
  unknown tag such as `div` has no platform UI, and creating it throws
  (Android `BehaviorRegistry.java:71-84`, iOS `LynxUIOwner.m:556-562`).
  **web-core:** `div` is a real `HTMLDivElement`, but
  `raw-text { display: none }` hides every carrier outside `x-text`
  (`lynx-stack/packages/web-platform/web-elements/src/elements/XText/x-text.css:261-265`),
  so `<div>hello</div>` renders nothing. **This engine renders it** — a
  deviation from both references. What follows from it:
  - a DOM text node renders wherever its container lays children out as
    items, a Lynx tag included — the W3C behavior; Lynx script never
    creates one (a `raw-text` carries its text as generated content), and a
    text node no longer lays out as an empty item that took a `gap` and a
    grid cell;
  - a `div` keeps the Lynx initial `display: flex` row, not HTML's `block`;
  - a sequence ends at every element that generates a box, out-of-flow ones
    included, and `display: none` ends none;
  - a hit on the text targets the container, as a hit on a paragraph
    targets its element;
  - `linear` measures items at max-content on its main axis
    (`crates/hughie/src/compute/linear.rs`), so text there does not wrap.
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
- **Item margin areas in scrollable overflow stop at the scroll container**
  — css-overflow-3 §3.3 defines a box's scrollable overflow to include "the
  margin areas of grid item and flex item boxes for which the box
  establishes a containing block", and reads as applying to every box, so a
  `visible` flex container would carry its items' margins up to the scroll
  container above it. Chrome, Firefox and Safari count them only in a
  container that itself scrolls
  ([csswg-drafts#9194](https://github.com/w3c/csswg-drafts/issues/9194),
  open); this engine does the same (`item_end_margin`,
  `crates/hughie/src/compute/util.rs`), for Linear and Relative items too.
  Details in [layout-architecture.md](../layout-architecture.md).
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
- **A Lynx text element is a Lynx paragraph by its kind, not by a `display`
  value** — because Lynx's own inline-ness is **structural, not
  display-driven**: `TextElement::OnNodeAdded` converts every added child via
  `ConvertToInlineElement()` (`lynx/core/renderer/dom/fiber/text_element.cc`),
  a node is inline iff its parent is a text or a non-view inline element
  (`element.cc:562-571`), and there is no `display: inline` anywhere in the
  native model. This engine says it structurally too: `text`, `inline-text`
  and `inline-truncation` are Lynx text elements by their definition
  (`CustomElement::is_lynx_text`, read once by `Document::define` and kept on
  each element from its creation), and `dom::layout::display_mode` lays one
  out as one flattened Lynx paragraph, or as a nested text scope of the
  paragraph around it, under every computed `display` but `none` and
  `contents`. Its subtree is inline content — runs, nested text scopes,
  atomic inline boxes — not flex or grid items. From 2026-09-03 the stylo
  fork carried a fork-only display value for this instead,
  `display: -lynx-text` (block-level, not an item container), which the UA
  sheet wrote on the text tags. The value was removed on 2026-10-17 (user ruling),
  together with its `DisplayInside` variant and the fork's style adjuster
  that restored it on the text tags; no sheet can write it any more.
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
- **css-anchor-position-1 (user-directed extension, 2026-09-29/30)** —
  neither native Lynx nor web-core has the module (web-core leaves it to the
  browser, so a web bundle gets Chrome's behaviour); this engine implements
  the Editor's Draft in `hughie` and `dom`
  ([style-assumptions.md](../style-assumptions.md) §28). What a page can
  observe differently from Chrome or from the ED:
  - **`position-visibility` is a paint-time flag.** A hidden box draws
    nothing and hits nothing, but its computed `visibility` stays what the
    cascade says (Blink's model; the ED's `visibility: force-hidden` is not
    a computed value here), and `boundingClientRect` still reports it.
  - **No transforms in anchor geometry.** The anchor box is the layout box
    (relative offsets included), not the bounding box of its transformed
    border box.
  - **No implicit anchor elements; no top-layer clauses.** `position-anchor:
    auto` finds nothing; `normal` behaves as `none` (as `auto`, so again
    nothing, under a `position-area`). The top layer exists
    ([style-assumptions.md](../style-assumptions.md) §29), but the
    top-layer clauses of §2.3 are not implemented: every box is treated as
    in one layer, and popover, dialog and `::backdrop` anchoring do not
    exist.
  - **Flat tree order, and one initial containing block.** "Tree order" in
    the target lookup is the flat tree's, and the initial containing block
    is the viewport's containing block, so a `fixed` box may anchor to
    anything in flow (as Blink allows).
  - **`@position-try` on the Lynx surface.** `inset-block-*`,
    `margin-block-*`, `block-size`, `inline-size` and their min/max are not
    Lynx properties, so they are dropped inside `@position-try` as in any
    style rule; the physical and inline-axis ones work.
  - **Legacy spellings are rejected.** By the user's ruling
    `position-visibility` takes only `anchor-valid` / `anchor-visible`:
    the legacy `anchors-valid` and `anchors-visible`, which the Editor's
    Draft lets a user agent keep as aliases, are parse errors. WPT's
    `parsing/position-visibility-*.html` cases still use the old spelling,
    so their ports in `vendor/stylo/style/tests/lynx_anchor_positioning.rs`
    are translated to the current one. Every other withdrawn spelling
    (`inset-area`, `position-try-options`, `@position-fallback`,
    `anchor(implicit)`) is rejected too.
  - **Lynx `linear` and `relative` containers ignore authored
    `justify-self`/`align-self` on their own absolutely positioned
    children**, as starlight does (`position_layout_utils.cc`,
    `CalcStartOffset`); flexbox and grid containers follow css-position-3 §4
    and align them, and so does every container for a box that escapes into
    it through a non-positioned wrapper (Lynx has no such boxes).
    `anchor-center` and `position-area`'s default alignment still apply in
    every container.
  - **No anti-flicker rule when a scroll forces a new fallback.** Blink
    (`out_of_flow_layout_part.cc`) skips, when it re-determines at the
    current scroll offset, every option that also overflowed at the
    remembered one, so two options with the same constraints do not
    alternate as the user scrolls; this engine tries every option but the
    current one, as §6.5 reads, and a box that overflowed in every option is
    re-determined only after it fits again and leaves again. The fit test
    after a scroll leaves a length inset's side unconstrained (as Blink
    does). Transforms on the scrollers between anchor and containing block
    are ignored in the default scroll shift (Chrome maps through them), and
    a box anchored to an anchored box does not follow that box's own shift
    (Chrome adds the chained translation; the spec adjusts for scroll
    containers only).
  - **Overflow limit of a grid item.** For an absolutely positioned grid
    item whose `position-area` region lies at an empty column, css-align-3
    §4.4.1.2's overflow limit is the bounding box of the inset-modified
    containing block and the grid container's padding box, not the grid
    area, so the box is shifted
    back to end at the padding edge; WPT `grid-position-area-basic.html`'s
    reference leaves it overflowing (unverified in browsers).
- **`first baseline` / `last baseline` self-alignment (user rulings,
  2026-10-16 and 2026-10-17)** — `hughie` aligns flex and grid items by
  their last baselines (css-align-3 §9.3; css-flexbox-1 §8.3, §9.4 step 8
  and §8.5; css-grid-1 §10.6 and §11.5 step 1), and every box reports
  `LayoutOutput::last_baselines` beside its first baselines. The stylo
  fork's `lynx` grammar parses css-align-3's `<baseline-position>` —
  `baseline`, `first baseline`, `last baseline` — for `align-items`,
  `align-self`, `justify-items` and `justify-self` (fork PR
  PupilTong/stylo#50; `ItemPlacement` and `SelfAlignment` in
  `vendor/stylo/style/values/specified/align.rs`), as web-core's browser
  does. **Native** has neither: `FlexAlignType`
  (`lynx/core/renderer/starlight/style/css_type.h`) ends at `kBaseline`, and
  `JustifyType` (`css_type.h:28`) has no baseline value at all, so
  `justify-items`/`justify-self: baseline` — accepted now because
  `first baseline` serializes as `baseline` (css-align-3 §4.2) and has to
  parse back — is a native-vs-web difference resolved toward web-core.
  `last_baseline_lines_anonymous_text_up_on_its_last_line` in
  `crates/dom/src/tree/anonymous.rs` drives both through anonymous text
  items. What is decided:
  - **Where a group sits in its flex line (user ruling, 2026-10-17: as
    browsers do).** The first-baseline group sits against the line's
    cross-start edge and the last-baseline group against its cross-end edge
    (css-flexbox-1 §8.3), so `wrap-reverse`, which makes cross-start the
    bottom edge, moves each group to the other edge. WPT
    `css/css-flexbox/multiline-reverse-wrap-baseline.html` checks this, and
    Chromium does it (`FlexLayoutAlgorithm::BaselineAscent`, baseline groups
    by line side). css-align-3 §9.3 step 3 places a group by its fallback
    alignment "resolved to physical directions", which would keep the first
    group at the top under `wrap-reverse`; that reading, which this engine
    followed for the first-baseline group until this ruling, is not used.
    Grid rows run top to bottom with no reversal, so grid has nothing to
    change.
  - **A group taller than its line or row overflows.** A first-baseline
    group overflows the end edge and a last-baseline group the start edge;
    nothing moves it back. css-align-3 gives `last baseline` the fallback
    alignment `safe self-end`, which would put an overflowing last-baseline
    group at the start; Chromium applies `safe` only when the author writes
    it, in flex and grid alike, and this engine follows Chromium.
  - **An item that shares no baseline takes the `safe` fallback.** This is
    a different rule from the one above: it is about one item that joins no
    group — a column container's item, whose inline axis is the cross axis,
    or an absolutely positioned child's static position — not about a group
    taller than its line. Such an item takes css-align-3 §4.2's fallback
    alignment, `safe self-start` for `baseline` and `safe self-end` for
    `last baseline`: its own start or end edge, by its own `direction`, and
    the container's start edge once it overflows the line. Flex and grid
    both do this (`item_alignment_offset` in
    `crates/hughie/src/compute/flexbox.rs` and
    `crates/hughie/src/compute/grid/alignment.rs`). In flex this replaced
    plain `end` for `last baseline` and the line's cross-start edge for
    `baseline`, so under `wrap-reverse` an unshared `baseline` item now sits
    at the start edge rather than at the cross-start edge, as Chromium's
    `CrossAxisStaticPositionEdge` also places a static position.
  - **The container's first and last baselines under `wrap-reverse`.**
    css-flexbox-1 §8.5 takes them from the startmost and endmost line, and
    its "startmost" is css-writing-modes-4 §6.2's "start": the line nearest
    the block-start (top) edge in a row container and the inline-start edge
    in a column container, whatever the wrap direction — not the line
    nearest the cross-start edge, which `wrap-reverse` moves to the other
    side (csswg-drafts#995; WPT
    `css/css-flexbox/flex-order-wrap-reverse-baseline.html`). This engine
    does the same. Within that line §8.5 prefers the first-baseline group for
    the first baseline and the last-baseline group for the last, and this
    engine follows that text. Chromium's `BaselineAccumulator` instead
    prefers the group flush with the line's top edge for the first baseline
    and the one flush with its bottom edge for the last, which differs from
    §8.5 only when one `wrap-reverse` line holds both groups: §8.5 then gives
    a first baseline below the last. No WPT covers that case.
  - **Container baselines, §8.5 and §10.6 steps 3 and 4.** Taken together,
    as the first baseline always was: the first (last) item gives its
    baseline, or one synthesized from its border box, where the steps would
    take the first (last) item that has a baseline.
  - **Lynx `linear` and `relative` containers report no last baseline
    (user ruling, 2026-10-17: as native).** Starlight has no last baseline,
    so a `last baseline` parent synthesizes one from their border box's
    bottom edge. web-core differs: there a `linear` view is a CSS flex box
    (`lynx-stack/packages/web-platform/web-elements/src/elements/common-css/linear.css`)
    with a last baseline
    of its own, so a `<view>` that keeps the default `display: linear`
    aligns by its last line under web-core and by its bottom edge here.
  - **Grid shares baselines along rows only.** Baselines are horizontal in
    this engine, so `justify-self: baseline`/`last baseline` take their
    fallbacks, `safe self-start`/`safe self-end`. css-grid-1 §10.3 and §10.4
    exclude an item whose size depends on an intrinsically sized track from
    baseline alignment; that exclusion is not implemented for either set.
  - **grid-lanes** aligns by no baseline (`style-assumptions.md` §24):
    `last baseline` takes `end` in the grid axis, as `baseline` takes
    `start`, and the container's last baseline is the lowest among the items
    placed last in each track (css-grid-3 §6.5).

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
  `crates/dom/src/render/blur.rs`). The Backdrop Root Image of the document's
  root element holds the canvas: an element whose Backdrop Root is the
  document's bakes over the colour the painter clears its target to (white),
  and an element inside a group Backdrop Root over transparent black, since
  that group's own pixels start empty. The filtered image is composited
  source-over in both cases (filter-effects-2's last step), so inside a group
  the group's unfiltered pixels show through wherever its filtered copy is
  translucent, as they do in Chromium. These are the recorded narrowings:

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
    a crop in which the off-screen part holds only what survived culling, over the canvas colour
    at the document's root. Those pixels are outside the viewport, so nothing visible reads them
    directly — but a σ large enough to reach back in can, and that part of the crop will be
    emptier than the spec's Backdrop Root Image.
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
- **`animation-duration: auto`** — native Lynx rejects the keyword; web-core
  (and css-animations-2) accept it, and it is the initial value. **User ruling:
  accepted under `lynx`**, serialized as `auto`, because
  scroll-driven animations (a W3C extension native Lynx does not have; see
  [css-animation.md](css-animation.md#scroll-driven-animations)) fill their
  range with it. On the document timeline it still means `0s`.

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
- **A `<text>` keeps its text verbatim: no CSS white-space processing,
  where web-core collapses it** — every native backend hands a `<text>`'s
  string to its platform engine as written: the element stores the text
  unchanged (`lynx/core/renderer/dom/fiber/raw_text_element.cc:16-46`), and
  Android (`BaseTextShadowNode.java:494-498`), iOS
  (`LynxBaseTextShadowNode.m:240-249`) and textra
  (`text_layout_textra.cc:569-575`) append it to the paragraph without
  touching spaces, tabs or newlines. The web target hands `x-text` to the
  browser, which applies css-text-3's `white-space-collapse: collapse` to
  every run except `raw-text`, which declares `preserve-breaks`
  (`lynx-stack/packages/web-platform/web-elements/src/elements/XText/x-text.css:261-265`).
  A user ruling of 2026-10-16 follows native. Every Lynx paragraph (a Lynx
  text element's) is built with `WhiteSpaceProcessing::Verbatim`
  (`crates/hughie/src/text/paragraph/content.rs`, user ruling 2026-10-17:
  no switch), so:
  space runs and leading spaces stay; every `\n`, in a `raw-text` carrier or
  a `text` attribute, is a forced break and the spaces after it stay; no
  run's own `white-space-collapse` is read; a soft wrap ends a space run on
  the line before it, and every space at a line end hangs — it adds no width
  and is not line content, so `getTextBoundingRect` and the per-line layout
  data exclude it, as Android's `getLineMax` does. Parley 0.12 hangs a
  whole sequence of overflowing spaces (CSS Text 4 §4.3.2) and takes a
  newline's break before any wrap opportunity, and a `<text>` hands every
  run parley's `collapse` hanging, which hangs line-end spaces
  unconditionally — before a newline and under `nowrap` too
  (`white_space_hanging` in `crates/hughie/src/text/paragraph/mod.rs`). A tab advances to the
  first multiple of 20px strictly past its position, Android's
  `Layout.TAB_INCREMENT` (AOSP `Layout.java:3714`), by Android's own
  `nextTab(h) = floor((h + 20) / 20) * 20` with no rounding tolerance. The
  position is measured from the start of the line's text, after the leading
  indent: Lynx applies `text-indent` as a `LeadingMarginSpan.Standard(indent,
  0)` (`lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/shadow/text/TextShadowNode.java:295-298`),
  a margin outside the text Android's `TextLine` measures tabs in, so a tab at
  the start of a 15px-indented line runs to 35px, where css-text-3 measures
  from the content edge and stops at 20 (`TabStops::Native` in
  `crates/hughie/src/text/paragraph/tab.rs`). iOS and Harmony leave tabs to
  their platform engines and are not confirmed to match. Which lines carry the
  indent follows native too (user ruling 2026-10-17): Android gives a
  `LeadingMarginSpan`'s first-line margin to the first line of every
  paragraph, so a `<text>` indents its first line and every line that starts
  after a `\n` — css-text-3's `each-line` (`BlockStyle::indent_keywords` in
  `crates/hughie/src/text/block/style.rs`) — and no soft-wrapped line; each
  indented line measures its tab stops from its own text start, after the
  indent. A `<markdown>` paragraph, a CSS inline formatting context, follows
  the author's `text-indent` keywords (its first line only by default).
  Under
  `white-space: nowrap` nothing after the first `\n` shows, as Android's
  `TextRenderer.handleWhiteSpaceWrap` deletes it
  (`TextRenderer.java:850-867`): the cut keeps the `\n` itself, so a `\n` at
  index 0 shows one empty line (Android deletes nothing there and its one-line
  limit leaves the same empty line); the dropped content shapes nothing, an
  atomic inline box after the cut is hidden, the cut draws no truncation
  marker, the dropped source units are still counted in source ranges, and
  the first line keeps its own width cut and `text-overflow` ellipsis. iOS
  sets `maximumNumberOfLines = 1` instead (`LynxTextRenderer.m:1024-1025`);
  whether TextKit's tail truncation then marks the hidden lines is not
  confirmed here. web-core keeps breaking at a `raw-text` newline under
  `nowrap`, because `preserve-breaks` is the carrier's own declared value.
  Only an authored `nowrap` stops the wrapping and cuts (user ruling
  2026-10-16): native's enum parser knows `normal` and `nowrap` alone and
  drops every other keyword (`enum_handler.cc:76-89`), so `pre`, `pre-wrap`,
  `pre-line` and `break-spaces` on a `<text>` wrap at the box width with
  every line shown, as `normal` does, where web-core shows `pre` unwrapped.
  `pre` is the one other keyword that computes `text-wrap-mode: nowrap`; a
  verbatim block tells it apart by its `white-space-collapse`, which is not
  `collapse`, and wraps (`wrap_mode` in
  `crates/hughie/src/text/block/style.rs`, the one place the block's wrap
  mode is decided). A newline that ends the text adds an empty last line,
  as Android's `StaticLayout` does and Lynx measures through
  `Layout.getHeight` (`TextRenderer.java:720`), by the same ruling:
  `"aaa\n"` is two lines tall. The empty line is reported in the per-line
  layout data with `start == end ==` the source length and the height of
  the line before it; it shows no content, so `getTextBoundingRect` gives it
  no box (an empty range at its start answers the natives' zero box with its
  height), and the source map is unchanged. iOS and Harmony are not
  confirmed; the engine follows Android. Under the `nowrap` cut the text
  already ends at its first newline, which shows as one line, so a newline
  that ends the text adds nothing there; a `<markdown>` paragraph, a CSS
  inline formatting context, keeps css-text-3's result, where a final
  preserved newline opens no line. `text-maxlength` composes in Android's order —
  `getUsedSpanClippedWithMaxLength` clips first and appends its ellipsis
  (`TextRenderer.java:126-135`), the newline deletion runs after it — so a
  length past the `\n` cuts nothing and leaves no marker (Android's appended
  ellipsis is deleted with the rest), while a length before it cuts as usual,
  its marker still gated on `text-overflow` (A0 in
  `web-text-test-replication.md`). `<markdown>`'s shadow paragraphs are
  CSS inline formatting contexts and keep css-text-3 collapsing, as
  web-core's `x-markdown` does.
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
- **Paragraph base direction: the inline formatting context follows
  `direction`; a `<text>` stays first-strong** — parley 0.12 takes a base
  direction (`StyleRunBuilder::set_base_direction`, `parley::BaseDirection`).
  The CSS block container's inline formatting context (`hughie::text::inline`)
  hands it the block's `direction`, as css-writing-modes-3 takes the bidi
  paragraph level from the block (`unicode-bidi: plaintext`, the one
  content-detected case, is not compiled under `lynx`): `text-align: start`,
  the order of a line mixing directions and the end edge whitespace hangs on
  all follow `direction`. A `<text>` keeps `BaseDirection::Auto`, first-strong
  detection, as it had before the upgrade; its `text-align: start`/`end` still
  resolve from `direction`. One limit remains in both: parley does not apply
  UAX #9 rule L1 to trailing whitespace, so a space that ends a line inside a
  run of the opposite direction stays at that run's level, inside the line,
  where L1 would move it to the paragraph level and hang it at the end edge
  (`an_rtl_container_sets_the_paragraph_level` in
  `crates/hughie/tests/inline_flow.rs`).
- **Inline formatting context: one line-break override per paragraph** —
  `word-break` is a per-run value there, and parley honours it per style, but
  the Chromium line-break override (a wrap opportunity after every space
  sequence, an ASCII pair table) is one decision per parley builder. A
  paragraph with any `break-all` run is therefore analysed without the
  override, because the pair table would suppress the opportunities between
  ASCII letters `break-all` adds; the other runs of that paragraph fall back
  to the UAX #14 opportunities alone (`crates/hughie/src/text/paragraph/shape.rs`,
  `shape`). A paragraph without `break-all` keeps the override, as `<text>`
  always has.
- **Line boxes: `line-height: normal` and fallback fonts** — CSS 2.2
  §10.8.1 leaves `normal` to the user agent. Both paragraph kinds take parley
  0.12's line boxes, which follow Chrome: a span's first available font
  (fontique's `first_available_font`) gives ascent + descent + line gap, and
  a run shaped in a fallback font adds that font's box as well. With a
  numeric or length `line-height` only the first available font sets the
  span's box, and glyphs from a fallback font overflow the line box. Metrics
  are not rounded to whole pixels as Chrome rounds them (the builder's
  `quantize` is off, `crates/hughie/src/text/paragraph/shape.rs`).
- **Inline formatting context: `text-overflow` not implemented** — it
  applies only to a block whose `overflow` is not `visible` (css-overflow-3).
  The producers of this context — the anonymous block box around loose text,
  and the block containers of HTML content the engine builds — compute
  `overflow: visible` or are unreachable by author CSS, so the property is
  not implemented, by the user's ruling of 2026-10-16; a block container
  that clipped would apply it per overflowing line box after the break
  (`crates/hughie/src/text/inline/mod.rs`).
- **Inline formatting context: a block-level box among inline content is an
  atomic inline (an engine decision in the plan the user adopted on
  2026-10-17)** — CSS 2.2 §9.2.1.1 breaks an
  inline box around an in-flow block-level box inside it: the inline box and
  its inline ancestors on that line are split into a piece before and a
  piece after, the line boxes on either side are wrapped in anonymous block
  boxes, and the block-level box becomes their sibling, a full-width block
  of its own between them. Chrome and Firefox do exactly that, and paint the
  split inline box's start edge on the piece before and its end edge on the
  piece after. This engine does not split: a `block`, `flex`, `grid`,
  `grid-lanes`, `linear` or `relative` box, or a Lynx text element, inside a
  block container's content is one atomic inline box, laid out by its own
  display, placed on the line by its margin box and `vertical-align` as an
  `inline-block` would be, with the inline content continuing beside it on
  the same line (`crates/dom/src/layout/inline_flow.rs`).
- **Inline formatting context: its atoms follow CSS, a `<text>`'s follow
  native Lynx (2026-10-17)** — an atomic inline box of a block container's
  inline formatting context with an auto width shrinks to fit (CSS 2.2
  §10.3.9: `min(max(min-content, available), max-content)` against the
  block's content width, `min-width`, `max-width` and percentages included),
  so a wide one wraps its own content, and a `position: relative` one is
  painted, hit-tested and measured at its offset from its place on the line
  (§9.4.3) (`crates/dom/src/layout/inline_flow.rs`, `size_atoms`,
  `place_and_hide`). A Lynx `<text>`'s atoms still differ, as native Lynx
  does: measured once at max-content, so a wide one overflows its line, and
  placed with their relative insets ignored (the user's 2026-09-17 ruling,
  below).
- **Inline formatting context: what an inline box and an atom do not do** —
  the full inline box model of CSS 2.2 §10.3.1 and §10.6.1 with
  `box-decoration-break: slice` is implemented (`hughie::text::inline`,
  `crates/hughie/src/text/inline/edge.rs`, `paint::walker`'s
  `paint_inline_boxes`); these parts of it are not, each recorded rather than
  silently approximated:
  - `vertical-align` on a non-atomic inline box is not applied (its text sits
    on the line's baseline), and an inline box adds its own line-height box to
    a line only through its own text runs there, so a box whose content on a
    line is only nested boxes does not grow it (CSS 2.2 §10.8.1 counts the
    box's strut-like box on every line it is on); an empty box with nonzero
    edges keeps the line from being a zero-height one (§9.4.2) but adds no
    line height of its own beyond the root's strut;
  - `position: relative` offsets, `opacity`, `transform`, `filter`,
    `box-shadow` and `outline` on a non-atomic inline box are not applied, and
    `background-clip: text` on one clips to its border box;
  - an out-of-flow box's static position is where the content after it
    begins, by that content's direction (`InlineFlow::static_position`), not
    where the bidi algorithm would put a neutral placeholder, so on a line
    that mixes directions the two can differ at a direction boundary; in a
    right-to-left block, a box hoisted past the block takes that position as
    its left margin edge, where CSS 2.2 §10.3.7 sets `right` to it (a box the
    block or one of its inline boxes lays out does take it as its right
    edge); a block-level out-of-flow box takes its static position on the
    line, as the atom it would be here (block-in-inline, above), where CSS
    puts it below the line; and an inline box split across lines is the
    containing block of its absolutely positioned descendants as the bounding
    box of its first and last fragments' padding boxes, a case CSS 2.2
    §10.1.4 leaves undefined (`crates/dom/src/layout/inline_flow.rs`,
    `out_of_flow_boxes`);
  - a box's edges sit beside the logical first and last characters of its
    content, so a box whose first or last character runs against the box's
    own direction carries its edge beside that character rather than at the
    box's visual extreme (CSS 2.2 §9.10 puts an `ltr` box's left edge on its
    leftmost piece); a box that bidi reordering splits on one line reports one
    fragment spanning both pieces;
  - a box's end edge sits before a collapsible space its content ends with,
    so that space still hangs at a soft wrap after the box; mid-line the
    space is drawn after the edge rather than inside it;
  - before a box whose first character is in a cursively joining script
    (Joining_Type D, R, L or C: Arabic, Syriac, Mongolian, N'Ko, Old Uyghur,
    Chorasmian and the like) or one UAX #14 segments by dictionary
    (Line_Break SA: Thai, Lao, Khmer, Myanmar, Tai Le, Tai Tham and the
    like), the only soft wrap opportunity kept at the box's start is the one
    after a space: a dictionary break at a box boundary inside unspaced text
    is lost (`start_cluster` in `edge.rs`, its properties from
    `icu_properties`);
  - a UAX #14 rule that reads past a box's first character (LB15b, LB19a,
    LB25, LB28a) reads the start edge's copy of that character as the
    character's successor, so its answer at the box's start can differ for
    non-ASCII text (Chromium's pair table settles every pair of printable
    ASCII characters other than the space first): a Brahmic orthographic
    syllable that a box boundary splits before its final virama gains an
    opportunity there (LB28a);
  - a box that starts between the two regional indicators of one flag keeps
    no opportunity between the flags after it inside the box: the shaper
    pairs the indicators of the box's run from its own first one, and the
    opportunities fall inside those clusters (nothing breaks inside a flag);
  - percentage margins and padding resolve against the block container's
    content width as the layout input implies it (its own width when the
    input fixes one, else its parent's offer), and a box with a percentage
    edge re-shapes the paragraph when that moves;
  - paint order inside the paragraph is every inline box's background and
    border in tree order, then every glyph, then each atom whole — CSS2
    Appendix E's order wherever content does not overlap, but not its
    line-by-line interleaving; and `Document::bounding_client_rect` of an
    inline box is its fragments' union unsnapped to device pixels.

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
  `LynxTextStyle::tail_color_convert`
  (`crates/hughie/src/style/text.rs:67`) and applied by `RunPaint::fill_style`
  (`crates/dom/src/paint/text.rs:116`), so reversing the ruling is a change to
  the painter and the default, not to shaping.
- **`hughie::text::block` vertical alignment of atomic inlines** — parley
  0.12 aligns and places an atomic box itself, by CSS 2.2 §10.8.1 with
  css-inline-3's `alignment-baseline` plus `baseline-shift`
  (`crates/hughie/src/text/paragraph/position.rs`): a box's part below its
  baseline reserves descent, and a non-baseline value moves the line's
  baseline as CSS does. What remains approximate: every box aligns against
  the root inline box (a `StyleRunBuilder` box's parent span is always style
  0), not against the nested text it sits in; `sub` and `super` are parley's
  (one fifth and one third of the root's font size, WebKit's and Blink's
  constants); a percentage is of the root's `line-height` — its font's
  ascent plus descent for `normal` — where CSS takes the box's own; and the
  Lynx-only `center` behaves as `middle`.
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
- **A text tag is one Lynx paragraph whatever its `display` says, except
  `none`** — **ruled by the user on 2026-10-15**, following native Lynx
  against `web-core`, and since 2026-10-17 carried by the element's kind
  rather than by a `display` value. `text`, `inline-text` and
  `inline-truncation` are Lynx text elements
  (`CustomElement::is_lynx_text`, `crates/bobcat-core/src/main/tree/text.rs`),
  which `dom::layout::display_mode` reads after `none` and `contents` and
  before every other computed `display`. So an author `display: none` hides
  one in all three engines, and `<text style="display: flex">` is still one
  paragraph. **Native** ignores the value: a text's inline-ness is structural
  (`TextElement::ConvertToInlineElement`,
  `lynx/core/renderer/dom/fiber/text_element.cc:196-205`). **web-core** lets
  the author's value take over the `x-text` host (`x-text { display: flex }`
  is author-origin, `x-text.css:7-13`), so the host is a flex box there while
  its shadow tree keeps the text. The UA sheet declares no `display` for the
  three tags, so the computed style reports the author's value (initially
  `flex`). `display: contents` on a text tag is **undefined behaviour (user
  ruling, 2026-10-17)**: it acts as computed, as on every element, before the
  element's kind is read, and that is handled only so that it does not crash
  (`a_text_with_display_contents_is_undefined_and_only_does_not_crash` in
  `crates/bobcat-core/src/main/tree/text.rs`). Until 2026-10-17 the stylo
  fork's style adjuster computed every value on the three tags but `none`
  back to `-lynx-text`; it was removed with the value.
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
- **Each realm's Lynx walk runs at the path's top step (2026-10-18, user
  ruling: scheme D; every realm at the top, user ruling 2026-10-19).** The
  kernel walks the path once (`MainThreadRuntime::dispatch` over `dom`'s
  `EventWalk`) and calls each realm that listens — the page realm, and each
  `frame`'s card in its `ShadowRealm` — once, at the path's first capture
  step, the page realm first, where that realm runs its whole Lynx walk
  (capture, bubble, `global-bindEvent`) over the steps of the path it holds
  handles for. This is web-core's model — its listener sits on the view's
  shadow root, or on `document` for keys (`WASMJSBinding.ts:262-282`), and
  `#commonEventHandler` runs the whole Lynx walk from there — and not native
  Lynx's, where one propagation crosses the page and every card. A
  `global-bindEvent` is scoped as web-core's listener is: reached by an
  event whose path passes through the registering node's tree root (the
  document node, or a frame's shadow root for a card), and, for web-core's
  document-level `keydown`/`keyup`, by every key. The recorded differences:
  - **A realm's Lynx handlers run before every component handler on the
    path and cannot stop them.** Their call is at the path's top step, so
    the engine's own components on the steps after it (a swiper hearing a
    `tap` on its content, a markdown on its own shadow content) hear the
    event after every Lynx handler of every realm has run, bubble-pass ones
    included, and whatever those handlers did.
  - **A frame card's Lynx handlers run when the page realm's do, at the top
    of the path.** Every realm is called at the same step, the page realm
    first, then the cards in construction order, so the page realm's whole
    walk — its bubble pass included — runs before the card's capture pass,
    and a page-level component between the frame and the card (a swiper
    the frame sits in, say) hears the event after the card's handlers. In
    web-core the card's listener sits on the card's own shadow root, deeper
    in the path, so that component would hear it first.
  - **Lynx `catch` and `stopPropagation` never reach the kernel's stop
    flag.** They end the realm's own walk: another realm on the path — a
    card inside a frame the page `catch`es on — and the components still
    hear the event. web-core never calls the DOM `stopPropagation` either.
    A component's `stop_propagation` is the one flag; every realm being
    called at the first step, it ends the components' walk alone.
  - **Which realms take part is decided before the walk.** The invitees are
    computed from the listener table before the first step, so a realm that
    first registers for the name during a dispatch — a listener of another
    realm, called before it, filing one in it — takes part from the next
    dispatch, and a realm that lost every registration for the name before
    its call — a realm called before it freed its nodes or closed it — is
    not called. Within a realm that is called, a registration made during
    the dispatch follows the standard (the path is fixed, a node's
    listeners are read when its step runs), because that walk is the
    realm's own.
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
  - **`beforeinput` is not cancelable either** (user ruling 2026-10-19,
    editable text E2). UI Events and Input Events Level 2 make `beforeinput`
    (all but `insertCompositionText`), `keydown` and `pointerdown`
    cancelable, and canceling them drops the edit or the editing default
    action. Here every editing event — `beforeinput`, `input`, the
    composition and focus events — is dispatched not cancelable, an edit
    always applies, a `keydown` at a focused host always runs its editing
    command and a press always places the caret, whatever a listener does.
    Why: a Lynx event object has no `preventDefault`; web-core registers its
    root listeners `passive: true` (`WASMJSBinding.ts:270,278`), so no Lynx
    handler cancels anything there either; a bit with no producer is not
    worth a boundary, so the 2026-08-17 ruling stands. E2 had briefly carried
    a `cancelable` argument into `__BobcatDispatchEvent` and a
    `defaultPrevented` answer back; both are gone. The only veto left on a
    default action is the embedder's `InputEvent::default_prevented`.
    Agreed future shape: a Rust component that needs to veto a default
    action sets a `canceled` flag on `dom`'s `EventWalk` beside its stop
    flag, never a script channel.
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
  - **`click` and `dblclick` are dispatched beside `tap` (2026-10-13,
    architect ruling)**, a superset of both references: Lynx has no event of
    either name in script (Android and the fragment path emit a native
    `click` beside `tap`, re-targeted at release), and web-core renames the
    browser's `click` to `tap` (`W3cEventNameToLynx`). Here `click` is UI
    Events': every pointer kind, the primary button of the primary pointer,
    no movement slop, at the nearest common inclusive ancestor of the down
    and release targets, suppressed by a consumed scroll or a cancel, and not
    by a delivered `longpress`; `dblclick` follows a second `click` at the
    same target within 500 ms. The order after a release is `pointerup`,
    `touchend`, `mouseup`, `click`, `dblclick`, `tap`. The event object of
    every pointer-shaped event (`pointer*`, `mouse*`, `click`, `dblclick`,
    `tap`, `longpress`) carries `x`, `y`, `clientX`, `clientY`, `pageX`,
    `pageY`, `button` and `buttons` (2026-10-13 user ruling: by payload
    shape, not by name; web-core gives a `click` only the four client and
    page coordinates and a `pointer*` event none), a `pointer*` event
    `pointerId` and `pointerType` too, and the `detail` is `{x, y}`.
  - **Compatibility mouse events for a mouse only (2026-10-13, architect
    ruling).** `mousedown`/`mousemove`/`mouseup` follow a mouse pointer's
    `pointerdown`/`pointermove`/`pointerup` at the same target (Pointer
    Events Level 3 §11); touch and pen produce none, where browsers emit them
    for touch after the touch events. Their event object carries web-core's
    `button`, `buttons`, `x`, `y`, `clientX`, `clientY`, `pageX` and `pageY`
    (all in the one viewport CSS px space); their `detail` is the `{x, y}`
    every pointer event has, where web-core leaves the DOM `UIEvent.detail`
    number. Both shipped embedders report a mouse honestly as
    `PointerKind::Mouse`, so they produce these events. **A mouse drag
    scrolls** a scroll container here (user ruling 2026-10-13, an engine
    policy in `PointerKind::drags_to_scroll`), unlike every browser; a drag
    that scrolls suppresses `click` and `tap` as a touch drag does, and
    `mousemove` keeps flowing while it scrolls. Deferred:
    `pointerover`/`out`/`enter`/`leave`, `mouseover`/`out`/`enter`/`leave`,
    `auxclick`, `contextmenu`.
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
- **W3C `scroll`/`scrollend` under Lynx's `scroll` name (2026-10-13,
  architect ruling).** CSSOM-View's two events fire at every scroll
  container, non-bubbling. Lynx's own `scroll` (`bindscroll` on
  `scroll-view` and `list`; web-core's `lynxscroll`, renamed to `scroll` by
  `W3cEventNameToLynx`) fires only on those components; here every scroll
  container fires it. Its Lynx `detail` is built from the extras channel
  (user ruling 2026-10-13, "kernel events are W3C"): `scrollTop`,
  `scrollLeft`, `scrollHeight`, `scrollWidth` read at delivery, `deltaX`/
  `deltaY` the change the post that queued it made (0 for `scrollend`), and
  `isDragging`, **always `false`** in this step, as web-core reports it
  (native reports whether a finger holds the container).
  Recorded choices inside the step:
  - **A `scrollend` ends a scroll that happened.** The main thread records
    every container that scrolled (an adopted offset moved it, or a
    script-facing instant scroll did) whether or not anything listens, and a
    post naming it at rest ends that record and fires `scrollend` if the name
    is heard at that moment. An instant scroll fires its `scroll` in the task
    that scrolled and its `scrollend` from the painter's rest post after it
    carried the request out — or after the drag or fling that holds the
    container ends. A view with no painter attached never posts that rest.
  - **A wheel gesture ends after 100 ms with no further tick**
    (`WHEEL_REST_SECONDS`): each tick that moves a container is a `scroll`,
    and the gesture one `scrollend`. A drag or fling that moves the
    container takes it over, and its own end decides the rest. Browsers
    debounce the same way, with their own intervals.
  - **The render's own positioning fires nothing**: `scroll-initial-target`
    and the re-snap after a layout change move a container without an event.
  - **An attribute-driven instant scroll at connect fires `scroll` then
    `scrollend` at mount**: a `scroll-top`/`scroll-left` on `scroll-view`, or
    any other component attribute that scrolls through
    `Document::scroll_to_with(.., Instant)`, is a script-facing instant
    scroll. That matches web-core, whose `scrollTop` assignment fires a
    browser `scroll`, and not native Lynx, which fires nothing for it.
- **`wheel`'s `detail` is `{}` (2026-10-13, user ruling).** It was
  `{x, y, deltaX, deltaY}`. `deltaX`, `deltaY`, `x`, `y`, `clientX`,
  `clientY`, `pageX` and `pageY` are on the event object, where W3C
  `WheelEvent` and native Lynx put them; web-core forwards the DOM
  `UIEvent.detail` number instead.
- **Keyboard: `keydown`/`keyup` only, targeted at the document element
  (2026-10-13, architect ruling).** W3C UI Events' two key events reach the
  page with `key`, `code`, `repeat`, `altKey`, `ctrlKey`, `metaKey` and
  `shiftKey` on the event object and a `{}` detail, which is web-core's shape
  (`createCrossThreadEvent.ts`), bubbling and composed. Recorded deviations:
  - **The target is the focused element** (2026-10-14, editable text E2):
    the focused editing host when one is focused, and the document element
    (the page) otherwise, as in a browser with nothing focused; the painter
    does not hit-test a key, and the main thread retargets it. Only editing
    hosts are focusable (`docs/editable-text-design.md`).
  - **No `keypress`.** `beforeinput`/`input` and the composition events are
    the editing host's (`docs/tracking/dom-events.md`); an input method's
    text arrives through `LynxView::edit`, not as keys.
  - **No `keyCode`.** web-core forwards the deprecated numeric `keyCode` for
    parity with iOS/Android; this engine reports the W3C `key`/`code` only.
  - **No `keydown` for a key the macOS input method consumed** (2026-10-14,
    editable text E3). winit 0.30 sends no key event for a key that went to a
    composition, where a browser sends `keydown` with `key = "Process"`; the
    composition's events still fire (`docs/editable-text-design.md`).
  - **The macOS CLI maps winit's keys** through a table
    (`crates/bobcat-cli/src/cli/keys.rs`): winit's variant names are the UI
    Events values, except the Command key (`Meta`, `MetaLeft`/`MetaRight`)
    and the space bar's `key` (`" "`). The browser embedder passes the
    browser's own strings through, and its canvas hears keys only while it has
    focus, which takes a `tabindex` the embedding page chooses.
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
  [css-text.md](css-text.md) describes that gate). The compat target
  `web-core` runs on the browser's always-on W3C inheritance and reproduces
  the visible default with targeted UA-sheet resets
  (`x-text { color: initial }` + `x-text > x-text { color: inherit }` in
  `web-elements`). When the decision below was taken it also **ignored the
  flag entirely** (zero references in `lynx-stack/packages/web-platform/`);
  since lynx-stack #3907 it reads it — see *The switch* at the end of this
  entry.
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
  **The switch (2026-10-13): `enableCSSInheritance: true` is honored the way
  web-core honors it.** lynx-stack #3907 (`8d0b358bf`, "support
  enableCSSInheritance for text in views") removed the premise that web-core
  never reads the flag: `config['enableCSSInheritance'] === 'true'` now sets
  `lynx-enable-css-inheritance="true"` on the page element
  (`web-core/ts/client/mainthread/elementAPIs/createElementAPI.ts`), and
  `web-core/css/in_shadow.css` adds
  `[lynx-enable-css-inheritance="true"] :where(x-view > x-text, x-view >
  lynx-wrapper, x-view > lynx-wrapper > x-text)` over fourteen `inherit`
  declarations. The decision is unchanged and its consequence is new:
  web-core parity now includes the switch. `PageConfig::enable_css_inheritance`
  is read from the page config string as `defaultDisplayLinear` is, and adds
  one UA rule after the text block,
  `view > text, view > wrapper, view > wrapper > text { color: inherit;
  text-decoration: inherit; }`
  (`bobcat_core::tree::text::CSS_INHERITANCE_RULES`). Off or absent — an
  absent key is off, as under web-core — the sheet is unchanged. Three things
  differ from web-core's text of the rule, none of them in what a page
  renders except where noted:
  - *Two declarations, not fourteen.* `--lynx-text-bg-color`,
    `background-clip` and `-webkit-background-clip` belong to web-core's
    lowering of a gradient `color` for a browser (transparent glyphs over a
    background image clipped to them, which `x-text.css` resets at the text
    root); the fork's `color` holds the gradient as its value, so
    `color: inherit` inherits it. `direction`, `font-family`, `font-size`,
    `font-style`, `font-weight`, `letter-spacing`, `line-height`,
    `text-align` and `text-shadow` are inherited properties that neither
    sheet resets at a text root: they reach the text with the switch off in
    both engines, so a declaration for them changes no value. The visible
    remainder: under web-core with the switch on, a `background-clip` an
    author writes on a view is also inherited by the text inside it; here it
    is not.
  - *Tag selectors at user-agent origin, not `:where()`.* web-core's selector
    has the specificity of one attribute selector, and web-core extends every
    authored selector with `:not([l-e-name])`, so no authored rule loses to
    it. Here every authored declaration and inline style outranks a UA rule
    whatever its selector, and the rule needs its tag specificity to outrank
    `text { color: initial }` inside the UA origin.
  - *No page attribute.* The switch selects UA-sheet text
    (`docs/style-assumptions.md` §D.14), so there is no
    `lynx-enable-css-inheritance` attribute for an author selector to match.
  The scope is web-core's selector list and no wider: a `text` under `page`,
  `scroll-view`, `list-item`, a blur view, or two wrappers deep keeps the
  reset.
  **Native's switch does more, and that part is not built (unruled).** With it
  on, native inherits a list of properties — `font-size`, `font-family`,
  `text-align`, `line-spacing`, `letter-spacing`, `line-height`, `font-style`,
  `font-weight`, `color`, `text-decoration`, `text-shadow`, `direction`,
  `cursor` (`lynx/core/renderer/css/dynamic_css_styles_manager.cc:30-38`),
  or the page's `customCSSInheritanceList` in its place
  (`lynx/core/renderer/dom/element.cc:2815-2826`) — through *every* element,
  so a `text` under a `scroll-view`, a `list-item` or the page inherits
  `color` there and keeps the reset here and under web-core, which reads no
  `customCSSInheritanceList`. With it off, native
  inherits nothing at all, where both web targets still inherit every W3C
  inherited property (`font-size` from a view to its text, for one; `cursor`
  as well, which is on native's list). `line-spacing` is in neither
  web-core's rule nor this engine's grammar. The same native switch also
  turns on its direction handling in fiber mode
  (`Element::IsDirectionChangedEnabled`, `element.cc:2828-2834`; without it
  `FiberElement::TryResolveLogicStyleAndSaveDirectionRelatedStyle`,
  `fiber/fiber_element.cc:4949-4953`, resolves no logical style against
  `direction`), which stylo does whatever the page config says. web-core is
  this project's default resolution of such conflicts (AGENTS.md); none of
  these has been put to a ruling.

- **`:is()`, `:where()`, `:has()`, `:nth-child()` family, `:first-child`/`:last-child`/`:only-child`/`:empty`**
  — all are *parsed* by Lynx's selector grammar but have **no matcher case at
  runtime** (always false). This is a bigger gap than a values mismatch: the
  grammar exists, the semantics don't. `:where()` in particular is load-bearing
  — it's the exact mechanism `web-core` uses for CSS-module scoping — so
  lynx-vello needs it working natively regardless of ReactLynx author usage.
  **Decision (user-confirmed, 2026-07-11): full stylo matching** — everything
  that parses, matches per spec; the native matcher's gaps are not
  replicated. See [docs/style-assumptions.md](../style-assumptions.md) §A.3.
  Since 2026-10-12 css-id scoping relies on it: `lower_style` synthesizes
  web-core's `:where([l-css-id="N"])` guards (§D.16 there).
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
- **System colours parse for authors (2026-10-07)** — `Canvas`,
  `CanvasText` and the rest of css-color-4's system colours (Stylo's
  `SystemColor` set) parse in every origin, as in a browser, and compute
  through Stylo's Servo device under the light scheme (`Canvas` is
  `rgb(255, 255, 255)`, `CanvasText` `rgb(0, 0, 0)`). Native Lynx's colour
  parser has none: `CSSColor::ParseNamedColor`
  (`lynx/core/renderer/css/css_color.cc:243-251`) accepts only the named
  colours in `css_keywords.tmpl`, so a native page drops such a declaration.
  web-core runs on the browser's parser and accepts them; web-core is
  followed. The fork change that enabled them (lynx fork `76f6a809b`) did so
  for the UA `<dialog>` rules.

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
  `inline-flex` box with its padding intact. This is the text block's one
  `!important` in the UA sheet, an argument
  [docs/style-assumptions.md](../style-assumptions.md) §D.15 admits;
  `the_ua_sheet_is_important_free_apart_from_the_recorded_exceptions` pins the set.
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
- **SVG (2026-10-09): one vector image, rendered wherever web-core renders
  it; `<svg>` is the Lynx component.** Native draws SVG only in `<svg>`,
  through ServalSVG at the element's frame size, and its `<image>` refuses SVG
  (`js_libraries/types/skills/image.md:268-269`); web-core's `x-svg` and
  `x-image` are shadow `<img>`s, so the browser renders SVG in
  `<svg src|content>`, `<image src>`, `background-image` and `mask-image`.
  Here the engine parses every SVG with its own converter
  (`crates/dom/src/render/svg/`) and the painter rasterises each draw at its
  device size (`crates/dom/src/render/vector_textures.rs`); design in
  `docs/svg-lynx-component-design.md`, pipeline in
  [media-resources.md](media-resources.md), status in
  [components.md](components.md)'s `x-svg` row. Where the references
  disagree, and which side was followed (user rulings 2026-10-08 and
  2026-10-09):
  - *`<image src="x.svg">`, `background-image` and `mask-image` render*
    (web-core).
  - *Intrinsic size is CSS Images 3 §4.1 default sizing* (web-core): an
    unsized `<svg>` lays out at it. Native sizes an `<svg>` from CSS alone
    (`skills/svg.md:117-118`).
  - *`<svg>`'s `load` detail is the element's border-box layout size* (native,
    ruled 2026-10-08), 0×0 without a box, read when the event is delivered.
    web-core re-dispatches its shadow `<img>`'s `load` with
    `naturalWidth`/`naturalHeight`, the document's intrinsic size. `<svg>`
    fires no `error` (web-core fires none).
  - *An empty or removed `content` keeps the current source* (web-core's
    `_handleContent` revokes its `Blob` URL and leaves the `<img>`'s `src`).
  - *Children generate no box and are never read* (web-core's `x-svg`
    renders no light-DOM child): the markup is the `content` attribute, not
    the subtree.
  - *`preserveAspectRatio` is honoured.* A draw maps the document's viewport
    onto its box by the root's `preserveAspectRatio` (`none` stretches,
    `meet`/`slice` scale uniformly and align), as the browser does; the
    revision-3 stretch to the box is gone.
  - *Fonts come from the SVG's own attributes* (architect's choice under the
    owner's permission): `font-family`, `font-weight`, `font-style`,
    `font-stretch` and `font-size`, inherited down the SVG tree; the host
    element's computed style is not read, which is also what an SVG inside
    web-core's `<img>` sees. A missing family resolves through the document's
    default family; glyphs come from the document's own `TextContext`, so
    `@font-face` faces apply, but text shaped before a face arrives keeps its
    fallback glyphs.
  - *CSS inside an SVG document is ignored* (native, ruled 2026-10-09): the
    converter reads presentation attributes only, so a `<style>` element's
    rules and a `style` attribute change nothing, `!important` included.
    web-core's shadow `<img>` hands the document to the browser, which
    applies both; native's ServalSVG has no `style` element in its tag
    subset. The project keeps one styling engine, stylo, and adds no second
    one for SVG. A document coloured only by a `<style>` sheet (an
    Illustrator export's `.st0 { fill: … }` classes) draws with the initial
    black fill here.
  - *Masks, filters, patterns, markers and a nested `image` draw nothing*,
    where the browser draws them all and native's subset lists `image`: a
    masked group is skipped with its subtree (drawing it unmasked would show
    what the author hid), a filter is ignored, a pattern paint is `none`.
  - *Text is a subset*: `<text>`/`<tspan>` with absolute `x`/`y` starting a
    chunk and `dx`/`dy` moving it (first list value only), `text-anchor` and
    `letter-spacing`; no `textPath`, `dominant-baseline` or bidi reordering
    within a chunk, and the `font` shorthand is not read.
  - *`current-color` is not implemented* (web-core lacks it; native resolves
    `currentColor` from it).
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
  `display: -lynx-text` on `text`/`inline-text`, plain from 2026-10-15 so
  that `display: none` still hides a text, and gone since 2026-10-17, when a
  text became a paragraph by its element kind and the value was removed —
  and this row is the case that exception deliberately
  does **not** cover: a scroller's display is a
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
  not with the tag defaults. The tag's component answers three UI methods
  and nothing else (the `scroll-view` UI methods entry below).
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

- **`<viewpager>` (2026-09-28): the authored pager is the scroll container,
  and its row is pinned in the cascade.** `crates/bobcat-core/src/main/tree/viewpager.rs`
  translates `x-viewpager-ng.css` onto all four tags (native's `viewpager` /
  `viewpager-item`, web-core's `x-viewpager-ng` / `x-viewpager-item-ng`). Where
  it leaves a reference:
  - *The row is UA `!important`.* web-core keeps its pages in a row inside a
    shadow `#content` box (`htmlTemplates.ts:379-405`) that no author rule on the
    host reaches; native places the pages itself. Here the authored pager lays
    them out, so `flex-direction: row`, `linear-direction: row` and
    `flex-wrap: nowrap` on the pager and `position: relative` on a page (the one
    web-core also pins, `x-viewpager-ng.css:63`) are important UA declarations —
    new §D.15 exceptions ([style-assumptions.md](../style-assumptions.md)),
    argued as structural invariants. An author `display` other than `flex` or
    `linear` is not pinned; web-core's `#content` inherits the host's `display`
    too.
  - *`bounces` stretches both edges* (`overscroll-behavior-x: contain-bounce`).
    web-core shows a page-wide blank box ahead of the pages only
    (`x-viewpager-ng.css:43-45`), so only the leading edge can be pulled; iOS
    hands the attribute to `UIScrollView.bounces` (`LynxUIViewPager.m:712`) and
    Harmony to `ARKUI_EDGE_EFFECT_SPRING` (`ui_viewpager.cc:55-59`), both edges.
    **Native over web-core; the architect's decision, to be confirmed by the
    user.** The default stays off, as in web-core and Harmony.
  - *`selectTab` with no numeric `index` is code 4 and moves nothing* (Android
    `LynxUIViewPager.kt:172-174`, Harmony `ui_viewpager.cc:224-227`). web-core
    multiplies whatever it gets (`XViewpagerNg.ts:26-35`): a missing or `NaN`
    index scrolls to 0 and succeeds, and a numeric string or a boolean is
    coerced. iOS answers its generic code 1. **Native over web-core; the
    architect's decision, to be confirmed by the user.**
  - *`selectTab` scrolls the page into view* (2026-10-11): it is CSSOM-View's
    "scroll an element into view" (`Document::scroll_into_view`,
    `crates/dom/src/scroll/into_view.rs:188`) on the pager's element child
    number `index`, with `inline: start`, `block: nearest` and
    `container: nearest`. So an out-of-range or negative index is code 4 and
    moves nothing (Android `LynxUIViewPager.kt:168-170`, Harmony
    `ui_viewpager.cc:229-235`; iOS code 1), where web-core clamps
    (`XViewpagerNg.ts:26-35`); a fraction is truncated (iOS `intValue`,
    `LynxUIViewPager.m:771`; Harmony `static_cast<int32_t>`,
    `ui_viewpager.cc:222`), where web-core multiplies it; and the page's
    layout position, the pager's `scroll-padding`, the page's `scroll-margin`
    and `direction: rtl` apply, where web-core and the natives assume uniform
    full-width pages. A pager an author made no scroll container is left
    alone, never an ancestor scrolled in its place. **Native over web-core
    for the index rules; the standard algorithm for the position; the
    architect's decision, to be confirmed by the user.**
  - *`selectTab` answers at once*, when the scroll is requested, as web-core
    does. iOS answers a smooth turn when it lands and fails the earlier
    callback when a second `selectTab` interrupts it
    (`LynxUIViewPager.m:757-829`).
  - *`smooth` is read with JavaScript truthiness* and defaults to `true`
    (web-core).
  - *`select-index` / `initial-select-index` are CSS, not a read at connect*
    (user ruling, 2026-09-28: standard CSS in the UA sheet, no component
    code). The pager sets `--viewpager-initial-index` from the typed `attr()`
    of `select-index`, else `initial-select-index` (web-core's order,
    `XViewpagerNg.ts:38-39`), and each page is its container's
    `scroll-initial-target` when that equals its `sibling-index() - 1`.
    web-core reads the attributes once, at connect, and scrolls once its
    width is known (`:37-57`). Accepted consequences, each pinned by a test in
    `crates/bobcat-core/src/main/tree/viewpager.rs`:
    - changing the attribute in force after the first layout turns the pager
      to the new page, instantly, over a user's swipe
      (`changing_the_index_in_force_turns_the_pager`; painter side
      `a_selected_page_is_the_first_frame_and_a_new_index_overrides_a_swipe`);
      web-core does not;
    - inserting or removing a page ahead of the target makes another page the
      target, and the pager turns to it
      (`inserting_or_removing_a_page_ahead_of_the_target_moves_the_target`);
      web-core does not;
    - a commit that leaves the target alone does not move a pager the user
      swiped away (`an_unrelated_commit_leaves_a_swiped_pager_alone`);
    - pages wrapped one by one in `wrapper` all count as index 0, and pages
      sharing one `wrapper` count correctly
      (`wrapped_pages_count_among_their_own_parents_children`);
    - a present `select-index` that is not an integer (`"abc"`, `"1.5"`,
      `""`) does not fall back to `initial-select-index` — the fork's typed
      `attr()` fallback gap ([style-assumptions.md](../style-assumptions.md)
      §27) — and shows the first page
      (`a_select_index_that_is_no_integer_shows_the_first_page`); web-core
      shows the first page for `"abc"` (`NaN`), lands between pages for
      `"1.5"`, and falls back for `""`;
    - a negative or out-of-range index names no page: the first page
      (`an_index_naming_no_page_shows_the_first_page`); web-core scrolls to
      the clamped position;
    - an inner pager does not inherit an outer pager's index
      (`a_nested_pager_does_not_inherit_the_outer_index`);
    - a `display: none` pager starts on the page at the first commit that
      shows it (`a_pager_shown_later_starts_on_the_selected_page_when_shown`);
      pages that arrive after the first layout bring the target with them
      (`pages_that_arrive_later_bring_the_target_with_them`);
    - **a pager zero pixels wide at its first commit stays on the first page**
      once it gets a width: its target is honoured at offset 0 and not again
      (`a_pager_that_is_zero_wide_at_its_first_commit_stays_on_the_first_page`).
      web-core retries every animation frame until the pager has a width
      (`:45-51`).
  - *A `viewpager-item` outside a pager is an ordinary container*, where
    web-core hides it (`x-viewpager-ng.css:6-14`) — the `list-item` decision
    above. *No `contain: strict` from the fifth page on* (`:66-68`): a browser
    performance shortcut, not a behavior.

- **`scroll-view` UI methods (2026-10-10): `scrollTo`, `scrollBy` and
  `getScrollInfo` on the scrolling axis, `autoScroll` since 2026-10-13;
  `takeContentScreenshot` not built.** `crates/bobcat-core/src/main/tree/scroll_container.rs`
  answers them in the `scroll-view` component's `CustomElement::invoke`.
  References: iOS `LynxUIScroller.m:1078-1118,1219-1360`, Android
  `UIScrollView.java:746-749,922-931,960-1050`, Harmony
  `ui_scroll.cc:64-155,750-799`, web-core `ScrollView.ts:32-112` (all under
  the paths the module documentation gives). Where it leaves a reference:
  - *The other axis keeps its offset.* Every method moves the axis the UA
    rules scroll (x under `scroll-x` / `scroll-orientation="horizontal"`,
    else y, iOS's `_enableScrollY`, Harmony's `IsHorizontal()`). Native
    writes 0 to the cross axis, which never moves there; web-core writes the
    one value to both axes and lets the browser clamp (`ScrollView.ts:48-53`).
  - *No right-to-left mirroring* of an indexed x target, which iOS
    (`LynxUIScroller.m:1282-1284`) and Android (`UIScrollView.java:1028-1032`)
    apply; web-core has none.
  - *`scrollTo`'s `offset` units.* A number is CSS px; a string is a number
    with `px`, `rpx` (viewport width / 750), `ppx` (divided by the device
    pixel ratio) or no unit; anything else (`"10vw"`, `"abc"`, `""`, `null`,
    a boolean) is code 4. iOS also reads `rem`, `em`, `vw` and `vh` and reads
    what it cannot parse as 0 (`lynx/platform/darwin/common/lynx/utils/LynxUnitUtils.m:10-17,110-144`); Android and
    Harmony read a number only (`UIScrollView.java:1001`, `ui_scroll.cc:92-93`);
    web-core `parseFloat`s a string and drops its unit (`ScrollView.ts:48-50`).
  - *`index`.* A non-number is code 4, where Harmony ignores it and scrolls
    to `offset` (`ui_scroll.cc:89-91`). A fraction truncates toward zero, as
    every native truncates it. An index outside the element children is
    code 4 and moves nothing, as all three natives answer
    (`LynxUIScroller.m:1265-1272`, `UIScrollView.java:1005-1011`,
    `ui_scroll.cc:81-88`); web-core ignores it (`ScrollView.ts:55-73`). The
    child's position is its layout location less the scroller's border, 0
    for a child with no box, which is web-core's `offsetTop` of a
    `display: none` child; web-core's index 0 scrolls to `offset` alone
    (`:57-59`).
  - *An empty scroller takes an offset-only `scrollTo`* (web-core). The
    natives answer code 4 for any `scrollTo` on a scroller with no children
    (`LynxUIScroller.m:1258-1261`, `UIScrollView.java:995-999`,
    `ui_scroll.cc:752-756`); here that holds only when an `index` is given.
  - *A clamped target succeeds* (iOS `LynxUIScroller.m:1096-1103`, web-core).
    Android scrolls to the clamped target and answers code 4
    (`UIScrollView.java:1019-1023,1048-1050`); Harmony answers code 4 and does
    not scroll, with the content size as its upper bound rather than the
    range (`ui_scroll.cc:759,773-777`).
  - *`smooth` is read with JavaScript truthiness* and defaults to `false`
    (web-core's `smooth ? 'smooth' : 'auto'`; Android's
    `getBoolean("smooth", false)` and Harmony's `smooth{false}` read a boolean
    only). *A smooth `scrollTo` answers at once* (web-core); native answers
    when the animation ends and reports an interrupted one to the earlier
    callback (`LynxUIScroller.m:1251-1256`, `UIScrollView.java:987-993`).
  - *`scrollBy` refuses a missing or non-numeric `offset`* with code 4
    (Android `UIScrollView.java:965-968`, iOS `LynxUIScroller.m:1224-1228`);
    Harmony reads it as 0 (`ui_scroll.cc:139-142`), and web-core's
    `HTMLElement.scrollBy` reads no `offset` and moves nothing. The answer's
    numbers are the natives' (the cross axis consumes nothing and leaves the
    whole `offset` unconsumed) as `f32` CSS px, where Android and iOS
    truncate them to integers (`UIScrollView.java:976-979`,
    `LynxUIScroller.m:1236-1239`). web-core answers no data.
  - *`getScrollInfo`'s `scrollRange` is the maximum offset*, Android's
    (`UIScrollView.java:929,1294-1297`) and Harmony's
    (`ui_scroll.cc:798`) reading, written beside iOS's `maxScrollOffset`
    under that name too (`LynxUIScroller.m:1323`). iOS's own `scrollRange`,
    the content size, marked there as a legacy field
    (`LynxUIScroller.m:1324-1325`), is not followed. Values are `f32`, where
    Android answers integers (`putInt`). web-core has no `getScrollInfo`
    (code 3).
  - *`takeContentScreenshot` is code 3.* It needs a pixel readback from the
    painter, which does not exist.
  - *`autoScroll({rate, start = false, autoStop = true})` (2026-10-13) is a
    motion of the painter, shared whole with `list`.* The document records
    a change of the container's auto-scroll (`dom::Document::auto_scroll`,
    `crates/dom/src/scroll/request.rs`), the committed frame carries it
    beside its scroll slots, and the painter moves the scrolling axis on its
    own clock (`crates/bobcat-core/src/paint/inertia.rs`). References: iOS
    `LynxUIScroller.m:1337-1375,1975-2040`, Android
    `AndroidScrollView.java:142-235`, Harmony `ui/utils/auto_scroller.cc`,
    web-core `ScrollView.ts:84-112` and `XList.ts:118-205`. Where it leaves
    a reference:
    - *It yields to a finger and to every other motion, and goes on
      afterwards* (user ruling 2026-10-13). While a drag holds the
      container, or a fling, a bounce back or a glide moves it, the
      auto-scroll adds nothing; it continues from where they leave it.
      Every reference writes the offset each frame whatever the finger
      does, except iOS's list, whose `touchBehavior: "pause"` is this and
      whose `"stop"`, `"forbid"` and default are not built
      (`LynxUIListContainer.mm:1062-1070`, iOS-only and undocumented).
    - *`scrollTo` and `scrollBy` do not end it*, as in every reference:
      an instant one moves the container and the auto-scroll goes on from
      there, a smooth one glides first. iOS's list answers a pending
      `scrollToPosition` callback with an error when an auto-scroll starts
      (`:1038-1042`); here the two do not interact.
    - *`autoStop` is read on both tags, default `true`* (user ruling
      2026-10-13), as Harmony reads it: the auto-scroll ends for good at the
      edge it moves toward, which is all iOS's and Android's `scroll-view`
      do. With the JSON `false` it stays at the edge and goes on when the
      content grows. web-core's `ScrollView` reads no `autoStop` and never
      ends (`ScrollView.ts:96-108`).
    - *`start` defaults to `false`*, as web-core, iOS, Harmony and
      Android's list since 4.1 read it; Android's `scroll-view` defaults it
      to `true` (`AndroidScrollView.java:144`).
    - *The rate is per second of the painter's clock*, Harmony's and
      web-core `XList`'s rule. iOS and Android move `rate / 60` per tick of
      a timer or of the display, so their speed follows the tick rate.
      web-core's `ScrollView` issues a smooth `scrollBy` of both axes every
      100 ms.
    - *A `rate` is a number or a `px`/`rpx`/`ppx` string on both tags*; the
      natives read a number on `scroll-view` and a unit string on `list`,
      and web-core `parseFloat`s a string and ignores its unit. A `start`
      with a zero, missing or unreadable rate is code 4 and leaves an
      auto-scroll in progress running (Harmony's answer; iOS and Android
      return without one; web-core's `ScrollView` stops the one in
      progress).
    - *A negative rate stops at the start edge.* Android's and Harmony's do;
      iOS's `scroll-view` and web-core's `XList` test the end edge only.
    - *A write is at most about sixty a second and at least one physical
      pixel* (`AUTO_SCROLL_MIN_INTERVAL_SECONDS`, 1/80 s: every frame of a
      60 Hz display, every second one at 120 Hz, 45 a second at 90 Hz and
      72 at 144 Hz). iOS's list steps every 16 ms and lowers that until a
      step is a device pixel (`:1047-1058`); Android's `scroll-view` every
      16 ms; the others every display frame. An auto-scroll that cannot
      move — its container held, or at its edge with `autoStop: false` —
      owes no frame at all, where every reference keeps its timer or frame
      callback running.
    - *It moves its own container only and never stretches*: no scroll
      chaining, no `contain-bounce` overshoot. A snapping container is not
      snapped while it has an auto-scroll and settles when that ends. No
      right-to-left mirroring, as for the other methods.
    - *The call answers at once.* A painter replaced while an auto-scroll
      runs does not continue it, as it does not continue a glide.

- **`scrollIntoView` UI method (2026-10-10): the CSSOM-View algorithm with
  `container: "nearest"`, on every element.** User ruling 2026-10-10: Lynx's
  method is the standard `scrollIntoView` of the CSSOM-View editor's draft
  (§"scroll an element into view", §"determine the scroll-into-view
  position") called with `container: "nearest"`, not an algorithm of its
  own. `dom::Document::scroll_into_view`
  (`crates/dom/src/scroll/into_view.rs:188`) is the algorithm, and the base
  method (`crates/bobcat-core/src/main/tree/base_methods.rs:252`) reads the
  params into its options. References: iOS `LynxUI.m:1549-1615` and
  `scroll_view/LynxUIScroller.m:1027-1065`, Android `LynxBaseUI.java:1055-1119`
  and `scroll/UIScrollView.java:762-810`, Harmony `ui_scroll.cc:157-200`,
  web-core `XView.ts:22-37` and `ScrollView/ScrollIntoView.ts` (all under the
  paths the base method's documentation gives). Where it leaves a reference:
  - *`nearest` scrolls the least distance that shows the element*, as the
    draft does, on both axes. The natives do nothing at all for `nearest` on
    their scroller's scrolling axis (iOS `LynxUIScroller.m:1033-1035,1049-1051`,
    Android `UIScrollView.java:771-773,791-793`, Harmony
    `ui_scroll.cc:166-168,182-184`); web-core's handler has no `nearest`
    case and so aligns the start edge (`ScrollIntoView.ts:46-70`).
  - *Both axes of the nearest scroll container are placed.* The natives and
    web-core position the scroller's scrolling axis alone and write 0 to the
    other (iOS `LynxUIScroller.m:1047,1063`, Android `UIScrollView.java:789,808`,
    web-core's `scrollTo({left, top})` with the unscrolled axis 0); here a
    `scroll-view`'s cross axis, a scroll container only script moves
    (`overflow: hidden`), is placed by `inline` (or `block`) like any
    scrolling box, and under the default `inline: "nearest"` it moves only
    when the element sticks out across it.
  - *The element's `scroll-margin` and the scroller's `scroll-padding`
    apply*, and positions align with the snapport; the references align the
    element's frame with the scroller's frame. Android's `end` also adds a
    `bottomInset` (`UIScrollView.java:778`), which has no counterpart.
  - *No scroll container above the element is code 8*, `OPERATION_ERROR`, as
    iOS (`LynxUI.m:1603-1611`) and Android (`LynxBaseUI.java:1110-1115`)
    answer. web-core's `__scrollIntoView` event reaches no handler and its
    call answers 0. An element with no box (`display: none`, or under one) is
    code 8 too: the draft scrolls nothing for it.
  - *A `scrollIntoViewOptions` value that is not an object is code 4*, like
    a missing one; iOS sends it `allKeys` regardless and Android's `HashMap`
    cast throws.
  - *A smooth scroll answers at once*, as the `scroll-view` methods do; the
    natives' `setContentOffset:animated:` / `setScrollTo` return before the
    animation too, and answer success on return.
  - *`block`/`inline` values other than the four keywords read as
    `"start"`*, as the natives' string compares and web-core's `switch`
    default leave them; a non-string value too.

- **Base-set `scrollTo`/`scrollBy` UI methods (2026-10-11): CSSOM-View's
  `Element.scrollTo(options)` and `Element.scrollBy(options)`, on every
  element whose kind has no method of the name.** User ruling 2026-10-09
  puts them in the base set; web-core reaches `HTMLElement.prototype`'s on
  every element (`createInvokeUIMethod.ts:29-31`). The base method
  (`crates/bobcat-core/src/main/tree/base_methods.rs:143`) reads `params`
  as a `ScrollToOptions` dictionary and runs the standard steps over
  `dom::Document::scroll_to_with`. Where it leaves the standard or web-core, and the rules a reader might not expect:
  - *`behavior: "auto"` is instant.* CSSOM-View's `auto` takes the
    element's computed `scroll-behavior`, and this engine has no
    `scroll-behavior` property, so `auto` is its initial value's instant
    scroll. Only `"smooth"` is smooth.
  - *A box that is no scroll container, or has no box, answers code 0 and
    does nothing*, as web-core's `HTMLElement.scrollTo` on a plain box does;
    CSSOM-View terminates the steps there and has no failure to report.
  - *Params that do not convert are code 4*, web-core's answer: a
    `behavior` that is present and not one of `"auto"`, `"instant"`,
    `"smooth"`, and params that are a number, a string or a boolean, make
    Web IDL throw a `TypeError`, which web-core's `__InvokeUIMethod`
    answers as 4 (`createInvokeUIMethod.ts:35-41`). The params convert
    before the algorithm runs, so this holds on an element with no box too.
    `null` and an array read as `{}`, as Web IDL converts them.
  - *A numeric value that is not a JSON number* converts as ECMAScript's
    `ToNumber` for a boolean or a decimal string, and reads as 0 for a
    hexadecimal, octal or binary string, an array or an object, where
    `ToNumber` reads `"0x10"` as 16 and `[5]` as 5. A non-finite result is 0
    in both, CSSOM-View's normalisation.
  - *A target equal to the current offset records no scroll request* unless
    one is pending, as `Document::scroll_into_view` does; a pending one is
    replaced, which is CSSOM-View's abort of the ongoing smooth scroll.
  - *A kind's own method shadows it*: a `scroll-view`'s `scrollTo`/`scrollBy`
    and an `x-swiper`'s `scrollTo` read their Lynx params (`index`,
    `offset`, `smooth`) and not `left`/`top`/`behavior`, as web-core's
    `ScrollView.scrollTo` override and `XSwiper.scrollTo` shadow
    `HTMLElement.prototype.scrollTo` there.

- **`list` UI methods (2026-10-11): `scrollToPosition`, `scrollBy`,
  `getScrollContainerInfo` and `getVisibleCells`, on a `list` component's
  `CustomElement::invoke` (`crates/bobcat-core/src/main/tree/list.rs`).**
  The architect's decision, to be confirmed by the user. Every cell is a
  real element child here (no recycling), so an index is an element child
  and a position is the cell's last layout. Where it leaves a reference:
  - *The index rules.* `scrollToPosition` names the cell by a string
    `itemKey` equal to an element child's `item-key` when one matches, else
    by `index` (read first), else by `position`, truncated toward zero. A
    missing, non-numeric, negative or out-of-range number is code 4 and
    moves nothing: this engine's index rule (`scroll-view`'s `scrollTo`, the
    pager's `selectTab`), which the iOS older list answers too
    (`LynxUICollection+Delegate.m:323-480`). The iOS list container
    (`LynxUIListContainer.mm:1107-1215`) and Android
    (`UIListContainer.java:815-1042`) answer code 8, `OPERATION_ERROR`;
    web-core moves nothing and answers 0 (`XList.ts:89-107`); Android reads a
    missing number as 0. iOS reads `index` after `position`, Android before
    it; here `index` is read first.
  - *`alignTo` and `itemKey` are honoured*, as both natives honour them;
    web-core ignores both. `"middle"` and `"bottom"` place the cell's centre
    or end at the scrollport's; anything else is `"top"`.
  - *`offset` moves the scrolling axis alone*, read as `scroll-view`'s
    `scrollTo` reads it (a number, or a `px`/`rpx`/`ppx` string); web-core
    `parseFloat`s a string, ignores its unit and writes the value to both
    axes. A bad `offset` is code 4.
  - *A list or cell with no box answers code 0 and moves nothing.* web-core
    reads such a cell's `offsetTop` as 0 and scrolls to `offset`.
  - *`getVisibleCells` reports rects relative to the list's scrollport*
    (the padding-box origin less the scroll offset), the docs' "relative to
    the list" and what iOS (`visibleCellsInfo`, `:1704`) and Android
    (`:966-1002`) report; web-core reports viewport coordinates
    (`getBoundingClientRect`, `XList.ts:220-250`). A cell is visible when
    its border box overlaps the scrollport by a positive area; web-core's
    fallback test counts a touching edge, and its usual path reports every
    cell `content-visibility: auto` has found relevant, a wider set. A cell
    held in a `wrapper` is not reported. Android's extra `position` field
    is not written.
  - *`scrollBy` is `scroll-view`'s*: the scrolling axis consumes what it
    moved, and the cross axis consumes nothing and leaves the whole `offset`
    unconsumed, as the natives answer; web-core has no `scrollBy` of its
    own and `HTMLElement.scrollBy` reads no `offset`.
  - *`autoScroll` is `scroll-view`'s* (2026-10-13), on the list's axis,
    with every consequence listed there. *`getScrollInfo` is code 3*
    because only the natives answer it on a list and the docs do not list
    it (the `setDragGesture` ruling, 2026-10-11).
    The list has no `scrollTo` of its own, as web-core's `XList` has none,
    so the base set's CSSOM-View `scrollTo` answers.

- **`text` UI methods (2026-10-11): `getTextBoundingRect({start, end})` on a
  `text` component's `CustomElement::invoke`
  (`crates/bobcat-core/src/main/tree/text.rs`), over
  `Document::text_range_boxes` and `hughie`'s `TextBlock::range_boxes`
  (`crates/hughie/src/text/block/range.rs`).** The architect's decision, to
  be confirmed by the user. web-core's `XText` has none of the three text
  methods, so the references are the natives (iOS `LynxUIText.m:790-807`,
  `LynxTextView.m:205-235`; Android `UIText.java:343-364`,
  `AndroidText.java:438-475`). Where it leaves them:
  - *Per-line glyph extents, iOS's reading.* Each line's box spans from the
    first selected cluster's left edge to the last one's right edge, as
    iOS's `enumerateEnclosingRectsForGlyphRange` gives it. Android clips
    only the first and the last line to the range's horizontal ends, so its
    middle lines span the whole layout width. Both take the line's top and
    height. The collapsible whitespace a soft wrap leaves hanging at a line's
    end adds nothing to the box (it belongs to no line, as in the `layout`
    event's ranges).
  - *Nothing found is code 8*, `OPERATION_ERROR`: a range past the text's
    end, one that shows no content (text past a truncation cut, the
    truncation marker, a hanging space), a nested `text`, an element with no
    box. The natives answer 1, `UNKNOWN`, "Can not find text bounding
    rect"; the engine has no code 1.
  - *Params.* `start` and `end` must be JSON numbers, truncated toward zero
    as iOS's `integerValue` and Android's `getInt` truncate, and then
    non-negative, else code 4. `start > end` is code 4, Android's answer;
    iOS answers 1. A missing one is 4, iOS's answer; Android's `getInt`
    throws and its catch answers 1. A numeric string is 4, where iOS's
    `intValue` reads its digits.
  - *`relativeTo` is not read*: the boxes are in viewport CSS px, the space
    `boundingClientRect` answers in, as `boundingClientRect` reads none of
    its params here. Both natives offset the boxes by the element's
    position relative to `relativeTo` when one is named.
  - *`start == end` keeps the natives' zero box*: one box of zero width at
    the content box's origin, as tall as the line holding `start`, which
    says nothing about where `start` is; kept for parity rather than
    answering a caret position.
  - *Selection methods absent.* `getSelectedText` and `setTextSelection`
    are code 3: there is no text selection model, nothing selects text, and
    so there is nothing to read or set.

- **`x-swiper` (2026-10-06): a component with web-core's shadow `#content`
  scroll container and a dot strip, and its layouts are UA rules.** `crates/bobcat-core/src/main/tree/swiper.rs` translates
  `x-swiper.css` and its template (`htmlTemplates.ts:225-275`) onto web-core's
  two tags, `x-swiper` and `x-swiper-item` (web-core's tag map has no `swiper`
  entry; native's tag is `swiper`, Android `XSwiperUI.java:52`). No events and
  no UI methods. Where it leaves a reference:
  - *The shadow tree is `#content` (with one default slot) and `#indicator`
    only*: no `#bounce-padding`, no `circular-start`/`circular-end` slots, no
    per-dot elements.
  - *Each item's main-axis size is UA `!important`* (§D.15,
    [style-assumptions.md](../style-assumptions.md)), web-core's own
    `!important` kept as a structural invariant. The main axis itself is a
    plain declaration on `#content`, which no author rule reaches, as in
    web-core. An item's `position` is not pinned.
  - *`coverflow` turns its items in a parallel projection, not a
    perspective one* (2026-10-12, on the user's request). web-core's
    keyframes rotate each item (`rotateY`/`rotateX`, up to 50°, with
    `scale(0.8)`, `x-swiper.css:244-267`) under `perspective: 200px` and
    `transform-style: preserve-3d`. A 3D keyframe refuses the painter's
    curve export, so here the turn is three 2D functions per keyframe:
    `scale(0.514, 0.8)` (web-core's scale, and `0.8 × cos 50°` along the
    main axis, the width a turned item shows), `skewY(14deg)` (`skewX` under
    `vertical`; the depth cue, the row seen from above, so the outer edge
    drops) and `translateX(24.3%)` toward the current item (half of what the
    scale removed, so the edge facing the current item stays where layout
    put it and no item overlaps its neighbour). It exports, and is sampled
    from the live scroll offset with no main-thread work per frame. What
    differs on screen: a turned item is a parallelogram, where web-core's is
    a trapezoid whose nearer edge is longer; its inner edge stays at its
    layout position, where web-core's rotation about the item's centre moves
    it; the stops are `0%`/`100%` turned and `45%, 55%` flat, where
    web-core's are `25%` (−50°) and `100%` (40°) with nothing at `0%`, so an
    item entering there starts flat; at rest the items beside the current
    one show the width of a 41° turn at scale 0.86, web-core's −40° and 38°
    at 0.84 and 0.81. `coverflow` and `carry` also drop web-core's
    `z-index: 1` keyframe, and run linear rather than web-core's default
    `ease` per segment (linear is also what keeps the inner edge still).
    **The numbers are the architect's, to be confirmed by the user.**
  - *Under `vertical`, the 20% margins resolve against the swiper's width*,
    as CSS resolves every percentage margin and as web-core does; native
    offsets the first page by 20% of the main axis (`XSwiperUI.java:582-604`).
    `carousel`'s trailing margin is `margin-bottom` under `vertical`, where
    web-core writes `margin-right` in both orientations
    (`x-swiper.css:193-195`).
  - *`current` is CSS* — the `<viewpager>` recipe above, with
    `--swiper-current` — so every consequence listed there for
    `select-index` applies, and a later change turns the swiper instantly
    where web-core turns it smoothly (`XSwiperAutoScroll.ts:35-41`,
    `XSwiper.ts:95-103`). The initial position is the item's snap position,
    as in web-core: its start, or its centre in `flat-coverflow`,
    `coverflow` and `carry`.
  - *`bounces` stretches both ends of the main axis*
    (`overscroll-behavior-x`/`-y: contain-bounce`), the `<viewpager>`
    decision; web-core shows a blank page-wide box ahead of the items only
    (`x-swiper.css:64-66`, `htmlTemplates.ts:226-232`).
  - *`page-margin`, `previous-margin`, `next-margin`, `duration`: no
    effect, following web-core*, where the three margins become custom
    properties on the indicator container (`XSwiperIndicator.ts:54-74`) that
    no item reads, and nothing observes `duration`. Native differs:
    `page-margin` is the gap between pages (`XSwiperUI.java:569`),
    `previous-margin`/`next-margin` size and offset the page in `coverflow`,
    `flat-coverflow` and `carry` (`:617-638`), and `duration` is the turn's
    animation length, 500 ms by default (`:91,855-863`).
  - *`indicator-dots`: shown by default and by `"true"`, hidden by any other
    present value.* The default is web-core's (native's is hidden,
    `@LynxProp(name = "indicator-dots", defaultBoolean = false)`,
    `XSwiperUI.java:887-890`); `"true"` showing is native's (web-core's rule
    `x-swiper[indicator-dots]::part(indicator-container) { display: none }`,
    `x-swiper.css:133-135`, hides the dots for any present value, `"true"`
    included); every other value hides them on both references, including the
    literal string `"{{false}}"` that web-core's own golden
    `basic-element-x-swiper-indicator-dots` writes
    (`__SetAttribute(s, "indicator-dots", "{{false}}")` in its `.web.bundle`)
    and shows hidden. The architect's decision.
  - *The strip's geometry is web-core's, not native's*: a `0.6rem` dot
    (`1rem` is 16px here, so 9.6px) at a `0.6rem × 7 / 5` pitch, `0.5rem` in
    from the bottom edge (the right edge under `vertical`)
    (`x-swiper.css:137-178`). Native draws a 7dp dot with 3.5dp on each side
    (a 14dp pitch), 10dp from the edge (Android `SwiperView.java:50,85,92,
    112-116`), and its default inactive colour is white at 35%
    (`XSwiperUI.java:57`) where web-core's is `#ffffff4d` (30%). The strip's
    box spans only the dots; web-core's container spans the whole edge, so a
    press beside the dots reaches `#content` here and the container there.
    An `indicator-color` that is no colour keeps the default, as native does
    (`XSwiperUI.java:892-901`); web-core writes it into `--indicator-color`
    unchecked.
  - *The active dot switches at the scroll range's page boundaries.* One
    animation on `#content`'s scroll timeline, `steps(count, jump-none)`,
    moves the active layer, so the dot changes when the offset passes the
    middle of a step of the whole range. web-core runs one animation per dot
    on its item's view timeline, active between 30% and 70% of it
    (`htmlTemplates.ts:250-266`), and native switches on page selection.
    Before a centred mode's at-rest snap settles an initial `current` under
    `vertical`, the first frame can show the dot after it, for the frame
    that shows the item's start.
  - *`circular` wraps the scroll axis itself* (user ruling 2026-10-07): a UA
    rule sets `overscroll-behavior: circular` on `#content`'s main axis
    (winning over `bounces` on that axis, which then has no edge to bounce),
    so the painter keeps the offset on a circle one scrolling area long and
    draws the seam ([style-assumptions.md](../style-assumptions.md) §25).
    web-core re-slots the first and last items into its shadow tree around
    the current one (`XSwiperCircular.ts`) and turns snapping off
    (`x-swiper.css:122-131`); native loops its ViewPager
    (`XSwiperUI.java:865-868`, `setLoop`). Here snapping stays, so a drag or
    flick pages across the seam like any other page, and script-facing
    offsets stand in the period `[0, P)`: while the seam is in view the
    document reports the offset past `max_offset` the screen shows, and an
    end item's position past `max_offset` or before `0` — the last
    `carousel` item's start, the first centred item's centre — is admitted
    there, so `current` and `scrollTo({index})` land on the item. A scroll
    timeline through the seam reads the nearer end of the range, so the dot
    strip shows the first dot while the first item is centred across the
    seam.
  - *`scrollTo` (2026-10-11)* is CSSOM-View's "scroll an element into view"
    (`Document::scroll_into_view`) on the swiper's element child number
    `index`, in the shadow `#content`, at the item's own `scroll-snap-align`
    on the main axis (`crates/bobcat-core/src/main/tree/swiper.rs`). Where it
    leaves each reference:
    - A missing, non-numeric, negative or out-of-range `index` is code 4 and
      moves nothing (Android `XSwiperUI.java:651-680`), where web-core does
      nothing and answers 0 (`XSwiper.ts:104-129,142-170`: a non-numeric
      `index` falls through to `HTMLElement.scrollTo`, and
      `children.item(index)` names nothing for the others).
    - `smooth` defaults to `true` (web-core), where Android defaults to the
      `smooth-scroll` prop (`XSwiperUI.java:89`).
    - Android's `direction` (`"begin"`/`"end"`, `XSwiperUI.java:767-772`) is
      not read; web-core has none.
    - The target is the item's snap position in every mode, where web-core
      aims at the item's start in `coverflow` and `carry` and lets mandatory
      snapping move it on to the centre; both end on the same offset, the
      engine without the second movement. An author's own
      `scroll-snap-align` on an item decides where it lands.
    **Native over web-core for the index rules; the standard algorithm for
    the position; the architect's decision, to be confirmed by the user.**
  - *`autoplay`, `interval` and `smooth-scroll` (2026-10-13) are a timer of
    the component's, web-core's `XSwiperAutoScroll.ts` without the script.*
    A connected swiper with `autoplay` keeps one timer in the document's
    `Autoplays` (`crates/bobcat-core/src/main/tree/swiper.rs`); the page
    waits for it with its realm's own timers and runs it in its epilogue,
    and a page with no such swiper arms and reads nothing. Each turn goes to
    the next item's snap position, or from the last to the first under
    `circular`, and does nothing on the last item otherwise. web-core and
    Android are the only references: neither iOS nor Harmony has a swiper,
    and the tag is in no public type declaration. Where it leaves one:
    - *Removing `autoplay` does not stop the timer* (user ruling
      2026-10-13, web-core to the letter: its handler has no branch for the
      attribute going away, `XSwiperAutoScroll.ts:62-70`). The timer starts
      when the swiper is connected with `autoplay` and starts over when
      `autoplay` or `interval` changes on a connected swiper that has it;
      only disconnection ends it. Android cancels on `autoplay=false`
      (`XSwiperUI.java:684-691`) and applies a new `interval` from the next
      turn rather than at once (`:851-853`).
    - *A turn that comes due while a finger holds the swiper is not carried
      out, and the timer goes on unchanged* (user ruling 2026-10-13): the
      turn is `Document::scroll_to_yielding`, a request the painter declines
      while a pointer has the container, the fling that pointer let go
      included. web-core scrolls under the finger. Android stops its timer
      for the drag and waits a whole interval after it (`:208-220`).
    - *`smooth-scroll` present with any value makes the turn instant*,
      `"false"` and `"true"` included, which is web-core as written
      (`XSwiper.ts:28-31,101`); Android reads a boolean that defaults to
      `true` (`:877-885`). `duration` is not read, as in web-core; Android
      turns over it, 500 ms by default.
    - *`interval` is `parseFloat`'s reading in milliseconds, 5000 when
      missing or no number*, web-core's rule, and no shorter than 4 ms: a
      browser's `setInterval` of 0, which web-core's e2e suite uses, settles
      at the nested-timer clamp. `Infinity` reads as no number.
    - *The current item is the one whose snap position is nearest the
      document's offset*, which is web-core's geometric current restated;
      Android keeps a stored page index. A turn counts from the offset the
      painter last posted, so one that comes before a smooth turn has
      landed asks for the same item again.
    - *A `circular` swiper's turn from the last item runs along the shorter
      arc*, the painter's rule for any smooth request. web-core travels
      forward by re-slotting its first item, and backward with two items
      (`XSwiperCircular.ts:44-118`); Android always forward (`:754-757`).
    - *A turn does not replace a `scrollTo` the painter has not handled
      yet*, and the document's own offset waits for the painter's post even
      for an instant turn.
    - *Not built:* Android's `finish-reset`, and autoplay's part in the
      swiper's events, none of which exist here.
  - *Not implemented:* every event; every UI
    method but `scrollTo`; `contain: strict`/
    `content-visibility` from the twentieth item (`x-swiper.css:77-80`), a
    browser shortcut.
  - *A child that is neither an `x-swiper-item` nor a `wrapper` generates no
    box* (`x-swiper.css:98-100`), as in web-core. *An item inside a `wrapper`
    gets none of the item rules*; web-core sizes only the swiper's own
    children too, but still gives a wrapped item `scroll-snap-align: start`
    (`:68-75`).

- **`x-refresh-view` (2026-10-06): a component with web-core's shadow
  `#container` scroll container, and its snap alignment is a view-timeline
  animation.** `crates/bobcat-core/src/main/tree/refresh_view.rs` translates
  `x-refresh-view.css` and its template (`htmlTemplates.ts:174-219`) onto
  web-core's three tags, `x-refresh-view`, `x-refresh-header` and
  `x-refresh-footer`. The shadow tree is web-core's: `#container` (`overflow-y:
  scroll`, `scroll-snap-type: y mandatory`) holding a 30% placeholder, the
  header slot, `#content` with the default slot (the one box that snaps at
  rest, centred), the footer slot and a second placeholder. The component sets
  `slot` on the header and the footer when they reach its default slot, where
  web-core's header and footer set it themselves when they connect. No
  events; the three UI methods since 2026-10-11 (the last item). Where it
  leaves a reference:
  - *The header holds at 90% visible* (web-core: the `IntersectionObserver`
    threshold of `XRefreshSubElementIntersectionObserver.ts`, the footer
    alike). Native triggers when the pull reaches the header's full height
    (iOS `headerReleased`, `LynxUIRefresh.m:268-271`).
  - *The pull range is the 30% placeholder* (web-core), where native's bounce
    has no such bound.
  - *A footer, load-more, `enable-loadmore` and `enable-auto-loadmore` exist
    in web-core only.* Native registers `refresh` and `refresh-header`
    (Android `LynxUIRefresh.kt:42`, `LynxUIRefreshHeader.kt:14`) and no
    footer; Android turns load-more off (`setEnableLoadMore(false)`,
    `canLoadMore() = false`, `LynxUIRefresh.kt:184,223`) and iOS marks
    `startLoadMore` and `footerReleased` deprecated (`LynxUIRefresh.m:264-266,
    296-298`). This engine follows web-core: the footer shows, holds and
    hides as the header does. `enable-auto-loadmore` and
    `enable-footer-rebound` only change events, so they have no visible
    effect.
  - *The header's and the footer's snap alignment follows their visibility*:
    `scroll-snap-align` is animated on each one's own `view(block)` timeline
    (`start` for the header from `cover 0%` to `exit 0%`, `none` from `exit
    20%`; the footer's mirror on `entry`), so the discrete change sits at 90%
    visible. web-core sets `x-magnet-enable` at that threshold and keeps it
    until `finishRefresh`/`finishLoadMore` removes it. Here a header that
    snapped in stays until the user scrolls it out or `finishRefresh`
    scrolls the content back: a drag on the header moves the shadow
    scroller, a drag on the content scrolls the content first. The alignment re-cascades on the
    main thread when it adopts a scroll of `#container`; a release that
    reaches the painter before the main thread has published the new snap
    position settles on the content.
  - *The content child's y-axis bounce is suppressed inside a refresh view*:
    every content child is `overscroll-behavior-y: auto !important` (§D.15,
    [style-assumptions.md](../style-assumptions.md)), so a pull chains from it
    into `#container`. Native attaches the header to the inner scroller's own
    bounce.
  - *The shadow parts are fixed flex columns.* web-core gives `#container`,
    `#content` and the slot the host's display mode (`--lynx-display:
    inherit`, `x-refresh-view.css:22-26`). The header and the footer follow
    the engine's default display policy (`display: linear` under
    `defaultDisplayLinear`), which is web-core's linear column under its
    default toggle (`common-css/linear.css`, the `--lynx-display-toggle`
    list).
  - *web-core's typed aliases are not registered*: its tag map turns
    `<refresh>` and `<refresh-header>` into `x-refresh-view` and
    `x-refresh-header` (`web-core/ts/constants.ts:112-113`); only the
    `x-refresh-*` spellings are tags here.
  - *The three UI methods (2026-10-11) are the standard scroll-into-view
    rather than the attribute latch* (the architect's decision, to be
    confirmed by the user). web-core's `finishRefresh`/`finishLoadMore`
    remove `x-magnet-enable` from the first header (footer) and let the
    browser re-snap to `#content`, and `autoStartRefresh` sets it on the
    header and smooth-scrolls `#container` to `top: 0`
    (`XRefreshView.ts:26-44`). Here there is no latch to remove: the first
    two smoothly scroll `#content` into view at `block: center`, its snap
    position, and `autoStartRefresh` scrolls the header into view at
    `block: start`, where web-core's scroll plus the magnet leaves it.
    `autoStartRefresh` is instant where web-core's is smooth (the
    coordinator's ruling, 2026-10-11): the painter glides a smooth request
    to where the frame's at-rest snap rule would settle its target, and the
    header is a snap position only once it is at least 90% visible, so from
    rest a smooth request to it is settled back to `#content` and nothing
    moves. The instant request moves the document at once, and the commit
    carrying it re-samples the header's alignment, so the painter rests on
    the header. A smooth version needs a request the painter does not
    settle at request time, left as a follow-up for the user.
    web-core's `autoStartRefresh` also marks the next refresh as not manual
    (`_nextRefreshIsManual = false`), which only its `startrefresh` event
    reads; with no events here, no such flag is kept. `autoStartRefresh`
    on a view with no header, or with `enable-refresh="false"`, moves
    nothing, where web-core still scrolls to the top placeholder. iOS has
    no footer method (`LynxUIRefresh.m:373-388`).

- **`<scroll-coordinator>` (2026-09-29): the authored coordinator is the
  scroll container, its slot is anchor-sized, and the fold is
  `scroll-capture`.** `crates/bobcat-core/src/main/tree/scroll_coordinator.rs`
  translates `x-foldview-ng.css` onto all ten tags (native's
  `scroll-coordinator`, `-header`, `-toolbar`, `-slot`, `-slot-drag`;
  web-core's `x-foldview-ng`, `x-foldview-header-ng`, `x-foldview-toolbar-ng`,
  `x-foldview-slot-ng`, `x-foldview-slot-drag-ng`) and replaces web-core's
  component code with CSS: the header and the slot are absolutely positioned,
  the slot sits at `anchor-size()` of the header and is the coordinator's
  height less `anchor-size()` of the toolbar
  ([style-assumptions.md](../style-assumptions.md) §28), the toolbar is
  sticky, and every box inside the slot is `scroll-capture-y: nearest
  forward` (§25). web-core writes the slot's `top` from a `ResizeObserver`
  (`XFoldviewNg.ts:27-38`), caps `scrollTop` (`:42-61`) and routes drags in
  its own slot handler (`XFoldviewSlotNgTouchEventsHandler.ts`); native uses
  Material's CoordinatorLayout + AppBarLayout (Android) and `UIScrollView`
  code (iOS). The geometry agrees with all three (slot at the header's
  height, slot height = coordinator height less toolbar height, range =
  header height less toolbar height). Where it leaves them:
  - *The structure is UA `!important`*: the coordinator's `overflow-y:
    scroll` (web-core's own `!important`, `x-foldview-ng.css:8`) and its
    `flex-direction`/`linear-direction: column`, the header's and the slot's
    `position: absolute`, the toolbar's `position: sticky`, and the
    `overflow-y: hidden` of `enable-scroll="false"` that has to beat that
    scroll — new §D.15 exceptions
    ([style-assumptions.md](../style-assumptions.md)), argued as structural
    invariants. Plain author declarations can still take the geometry apart
    and are not pinned, the same way an author `scroll-snap-align` on a pager
    item is not: a `top` on the slot (`top: auto` included) replaces its
    `anchor-size()` offset, and an author `anchor-name` on the header or the
    toolbar renames the anchor the slot reads, which then takes its `0px`
    fallback.
  - *A coordinator with `height: auto` collapses to its toolbar's height*:
    absolutely positioned children never size their parent. web-core and
    native size it from toolbar + slot. The docs require the coordinator to
    be sized ("coordinator height == slot height + toolbar height"), and the
    UA gives it `height: 100%`.
  - *`flex-grow` on the toolbar fills the coordinator*: the toolbar is its
    only in-flow child. web-core's `size-toolbar-slot-share-flex-parent`
    fixture shares the space between toolbar and slot.
  - *A slot written before its header sits at the top*: an absolutely
    positioned anchor is acceptable only before the query box in tree order
    (css-anchor-position-1 §2.3), so the slot takes the `0px` fallback. The
    documented structure (toolbar, header, slot) is unaffected.
  - *A slot taller than the coordinator less the toolbar extends the range*
    past header less toolbar; web-core caps `scrollTop` and native caps the
    fold. Cards that size the slot to exactly that are unaffected;
    web-core's `size-toolbar-and-slot-size-lager` (slot 500 in a 600
    coordinator with a 200 toolbar) folds 300 here, 200 there; an author `flex: 1` on the slot (the docs' recipe) is
    ignored, but the UA height gives the same length whenever the
    coordinator's height is definite.
  - *Stray children of the coordinator are shown*; web-core `display: none`s
    every child that is not one of the three roles (`x-foldview-ng.css:36-50`).
    That needs `!important` on `display`, which is a default here — the
    `list-item` decision above.
  - *A fling continues from the fold into the content*, as native's nested
    fling does: every fling frame is a chain step ordered by its own
    direction. web-core instead replaces the release with
    `scrollBy(deltaY * 4, smooth)` on the last box it scrolled, and skips it
    once the fold is complete.
  - *The fold order nests*: a list inside a scroll-view inside the slot goes
    forward coordinator, scroll-view, list (`nearest` nests outward-first);
    web-core's touch handler pairs the coordinator with the innermost
    scroller that can still move in the drag's direction
    (`XFoldviewSlotNgTouchEventsHandler.ts:75-93`). Horizontal nesting inside
    the slot is untouched, because only the `scroll-capture-y` longhand is
    set: a horizontal list in a horizontal pager inside the slot still
    scrolls inner first.
  - *web-core's `x-foldview-slot-ng scroll-view { overscroll-behavior-y:
    none }` (`x-foldview-ng.css:75-78`, a Safari bounce fix) is not carried*:
    here it would fence the chain and nothing inside the slot could unfold
    the header. For the same reason an author `overscroll-behavior-y:
    contain` on a scroller inside the slot keeps that scroller from moving
    the fold (Lynx has no `overscroll-behavior`, so ported cards do not write
    it).
  - *`enable-scroll="false"` / `scroll-enable="false"`: a three-way
    conflict.* Here the coordinator is not user-scrollable (`overflow-y:
    hidden !important`): the fold stays where it is, scrollers inside the slot
    scroll themselves, script can still scroll the coordinator. web-core's
    rule is dead — `x-foldview-ng[scroll-enable="false"] { overflow-y: hidden
    }` (`:32-34`) loses to its own `overflow-y: scroll !important`, and its
    slot handler writes `scrollTop` regardless — so nothing is disabled.
    Android swallows every vertical drag in the coordinator, inner lists
    included; iOS disables its pan and keeps the inner content pinned while
    expanded; both keep programmatic scrolling. **The architect's decision,
    to be confirmed by the user.** Both spellings are accepted (the
    documented/Android `enable-scroll`, and iOS's alias `scroll-enable`,
    which web-core reads).
  - *`header-over-slot` raises the header* (`z-index: 1`, present and not
    `"false"`), the documented and native meaning; the toolbar's `z-index: 2`
    keeps it above both. web-core raises the *slot* instead
    (`x-foldview-ng.css:66-69`), which contradicts its own docs and both
    platforms and changes nothing visible, the slot already painting after
    the header. **Docs and native over web-core; the architect's decision, to
    be confirmed by the user.** web-core's toolbar is `z-index: 1`
    (`:55`); here it is `2` so a raised header stays under it.
  - *`bounces` is opt-in* (`overscroll-behavior-y: contain-bounce` when
    present and not `"false"`), the `<viewpager>` precedent. Default off, as
    web-core has it (`overscroll-behavior: contain`); iOS bounces by default.
  - *`enable-scroll-bar` / `scroll-bar-enable` are no-ops*: the engine draws
    no scrollbars.
  - *Slot-drag `enable-drag` is not implemented* (Android
    `LynxUIScrollCoordinatorSlotDrag.kt:114`; web-core implements nothing for
    the tag). With the default, a drag on non-scrolling content in the slot
    already latches the coordinator and folds it.
  - *Not implemented*: the `offset` event, `granularity`, `refresh-mode`,
    `tab-movable-enable`, `toolbar-interaction-enable`,
    `header-scrollview-enable`, `compat-container-popup` and every
    `ios-*`/`android-*` prop.
  - *`setFoldExpanded` (2026-10-11)* is a component on the two coordinator
    tags (`ScrollCoordinator`, its `CustomElement::invoke`). **The
    architect's decision, to be confirmed by the user.** Where it leaves a
    reference:
    - `offset` is read as `scroll-view`'s `scrollTo` reads it: a number in
      CSS px, or a string that is a number followed by `px`, `rpx`, `ppx` or
      nothing. web-core `parseFloat`s it (the param is typed as a string,
      `XFoldviewNg.ts:63-72`), which takes the number and ignores any unit;
      iOS reads a number or a unit string through `toPtFromUnitValue`
      (`LynxUIScrollCoordinator.m:771-830`), which knows more units.
    - A missing or unreadable `offset` is code 4 and nothing moves, the
      engine's rule for a required numeric param. web-core scrolls to 0
      (`parseFloat(undefined)` is `NaN`, and `scrollTo({top: NaN})` reads it
      as 0) and answers 0; iOS reads 0.
    - The clamp is the coordinator's scroll range, which the UA rules make
      the header height less the toolbar height, web-core's
      `scrollableLength` and iOS's `expandHeight`. An author slot taller
      than the UA height extends the range (the deviation above), and the
      clamp with it, where web-core and iOS still stop at the fold.
    - The call answers at once, smooth or not, as web-core's does. The
      coordinator declares no `scroll-snap-type`, so the painter, which
      settles a smooth request's target onto the frame's snap positions,
      does not move this one; an author `scroll-snap-type` on the
      coordinator would.
    - iOS's `getScrollInfo` (`{scrollX: 0, scrollY, scrollRange}`) and
      `scrollBy({offset})` (what it consumed) are not built: web-core has
      neither and the docs do not list them, the `setDragGesture` ruling
      (2026-10-11). `getScrollInfo` is code 3. `scrollBy` is not: the
      coordinator's kind has no method of the name, so the base set's
      CSSOM-View `scrollBy` answers, which reads `left`/`top`, ignores
      `offset` and answers 0, the same as `HTMLElement.scrollBy` in
      web-core. If the ruling is overturned, `scrollBy` is `scroll-view`'s
      `scroll_by` on y and `getScrollInfo` is `{scrollX: 0, scrollY,
      scrollRange}` with `scrollRange` the range's maximum.
  - *A coordinator nested in another's header or slot reads its own header
    and toolbar*: `anchor-scope` is not implemented, but anchor targets are
    only the containing block's own children
    ([style-assumptions.md](../style-assumptions.md) §28), so the outer
    header is never a candidate.

- **`<dialog>` and the top layer (2026-10-07)** — W3C (bucket 1): web-core
  hands `<dialog>` to the browser, and native Lynx has neither a dialog nor a
  top layer. Implemented in `dom` (the layer, `::backdrop`, inertness) and
  `bobcat-core`'s `tree::dialog`
  ([style-assumptions.md](../style-assumptions.md) §29). What a page can
  observe differently from a browser:
  - *Leaving the top layer is immediate.* There is no `overlay` property and
    so no pending removal a transition could observe.
  - *No close requests.* Escape and the back gesture do not reach a dialog,
    `closedby` is not parsed, and there is no light dismiss.
  - *`cancel` is not cancelable.* There is no `preventDefault` in this
    engine's event model, so `requestClose()` always fires `cancel` and then
    `close`, and always closes.
  - *A top-layer element inside skipped contents stays hidden.* Under a
    `content-visibility: hidden` (or non-relevant `auto`) ancestor it is not
    rendered, where css-position-4 renders it.
  - *`dialog:modal` scrolls with `overflow: scroll`*, not HTML's
    `overflow: auto`, which this engine deliberately lacks, and is placed by
    `top: 0; bottom: 0` for HTML's `inset-block: 0`, which the fork disables.
  - *`Canvas`/`CanvasText` are always the light-scheme values*
    (`rgb(255, 255, 255)`/`rgb(0, 0, 0)`): the engine's device has no dark
    scheme and `color-scheme` is not authorable under `lynx`.
  - *A `display: contents` modal dialog computes to `flex`, not `block`.*
    `dialog:modal` declares `-servo-top-layer: auto`, so css-position-4
    §3.1's fixups run (a `position` other than `absolute`/`fixed` computes to
    `absolute`, as in a browser), and the blockification of `contents` yields
    `flex`, the `lynx` grammar's initial display, where a browser computes
    `block`. The dialog renders as a `flex` box centred in the viewport
    (`a_display_contents_modal_dialog_is_blockified_and_renders` in
    `tree::dialog`). An element `dom` puts in the top
    layer without such a UA rule is not adjusted: membership alone places it
    against the viewport, and a `display: contents` one renders nothing.
  - *`returnValue` is dropped.* `close(returnValue)` and
    `requestClose(returnValue)` accept and ignore it; Lynx JS has no reader.
  - *`InvalidStateError` is `invoke` code 7, not web-core's 4.* `show()` on
    a modal dialog and `showModal()` on an open non-modal or a disconnected
    one answer native's `INVALID_STATE_ERROR` (7,
    `lynx_get_ui_result.h:53-61`). web-core reports every method that throws
    as `PARAM_INVALID` (4, `createInvokeUIMethod.ts:12-44`), and native has no
    dialog to raise its 7. A ruled deviation from web-core (user ruling
    2026-10-09, `docs/element-methods-design.md` §9): a component method's
    `MethodError::InvalidState` is 7 and `MethodError::Operation` is 8 for
    every element kind, so a script can tell a refused state from refused
    params.
  - *No toggle events, no focus.* `beforetoggle`/`toggle`, the focusing
    steps and `autofocus` are absent, and `::backdrop` itself does not
    animate.

- **`overlay` / `x-overlay-ng` (2026-10-08)** — Lynx-only (bucket 2): native
  registers `overlay` with `x-overlay-ng` as its legacy alias, web-core maps
  `overlay` to `x-overlay-ng` and builds it from a shadow `<dialog>` opened
  with `showModal()`. That structure is followed: a `display: contents`
  host and a shadow `<dialog>` that enters the top layer and carries the
  content (`bobcat-core`'s `tree::overlay`,
  [style-assumptions.md](../style-assumptions.md) §30). Where native and
  web-core disagree, and which side was followed:
  - *`level` is ignored* (web-core). Native has four tiers, level 1
    frontmost, last in first out within one; web-core's `z-index` ladder has
    no effect under its `display: contents` host, so its overlays stack in
    `showModal()` order. Overlays here stack in the order they were shown.
  - *`events-pass-through` passes every gesture* (native). A pass-through
    overlay's shadow dialog is shown as a Manual popover instead of with
    `showModal()` — the one top-layer element HTML does not make the
    document inert for — and it and its `::backdrop` are `pointer-events:
    none`, so a touch outside the content reaches what is below, scrolls
    included, as native does (`LynxOverlayContainer.m:169-214`,
    `LynxOverlayView.kt:726-752`). web-core shows it modally and forwards
    only `click`, by closing the dialog, `elementFromPoint`, a synthetic
    click and `showModal()` on the next frame (`XOverlayAttributes.ts:69-88`):
    a browser workaround, not copied. The popover is HTML's
    ([style-assumptions.md](../style-assumptions.md) §31), so the
    difference is which standard API the overlay calls, not any behavior of
    its own.
  - *No `requestclose`, no Escape, no back gesture* (nothing to send it).
    Native Android and Harmony send `requestclose` and iOS `onRequestClose`;
    web-core's Escape closes the browser dialog silently, with `visible`
    still set.
  - *`showoverlay` carries no detail* (web-core). Android sends `{errorCode,
    errorMsg}`.
  - *Removing a shown overlay from the document fires no `dismissoverlay`*
    (web-core). Android fires one.
  - *No `dismissoverlay` at mount with `visible` false* (web-core and
    Android). iOS likely fires one.
  - *The host's own styles paint nothing* (web-core). The host is `display:
    contents`, and an author `display` gives it only a zero-sized box, so an
    author `background-color` on it never covers the screen, and an author
    `display` never shows a hidden overlay: what fills the viewport is the
    shadow dialog, transparent, and hidden is that dialog's closed state.
    On Android the host's own background paints over the window.
  - *The first child's `display` is not forced to `flex`*: web-core writes
    `display: flex` on it; here it keeps the display its own tag gets.
  - *Events fire on transitions only.* web-core re-runs `showModal()` and
    fires `showoverlay` for every write of a showing value (`"true"` to
    `""`); here a rewrite that keeps the overlay shown fires nothing.
- **`<frame>` renders its card in the host's document**, under a nested page
  in the frame's shadow root that belongs to that card — letting go of the
  card frees its page, and the next card gets a fresh one (user ruling
  2026-10-19) — driven by a
  `ShadowRealm` child of the MTS realm (`bobcat-core`'s `main/tree/frame.rs`
  and `bobcat-element`'s `frame.ts`/`frame-boot.ts`, user rulings: one PAPI,
  no second engine; a custom-element ability). Native stands up a second
  `LynxTemplateRender` per frame (`LynxFrameView.java`) and web-core a nested
  `lynx-view` with an iframe realm; what follows from sharing one document:
  - *A frame inside a nested card is disconnected for nobody but the page
    realm.* Connections and disconnections reach `bobcat:frame` in the page
    realm alone, which holds no handle for a frame a nested card created; so
    a nested card that removes one of its own frames and keeps its handle
    keeps that frame's card running. Letting go of the handle disposes of it
    (the child's `FinalizationRegistry`), and so does the outer card going:
    its `dispose` closes every card its frames loaded. web-core's nested
    `lynx-view` hears its own `disconnectedCallback` at any depth.
  - *The nested card's `PageConfig` is ignored.* The nested page is styled by
    the host document's UA sheet, built from the host's
    `defaultDisplayLinear`, `defaultOverflowVisible` and the rest; native and
    web-core each read the nested bundle's own page config.
  - *`rpx`, `vw` and `vh` inside a frame are the viewport's.* web-core
    rebases them on the frame (`--rpx-unit: calc(1cqw / 7.5)`,
    `--vw-unit: 1cqw`, `--vh-unit: 1cqh` on `lynx-view`,
    `web-core/css/index.css`); here the document's one device resolves every
    length, the frame's included.
  - *`@font-face` in a frame's sheets is not loaded.* A sheet a frame realm
    adopts is mounted on the frame's shadow root, and `dom` loads no
    `@font-face` rule of a shadow-tree sheet
    (`Document::append_shadow_rules`).
  - *No `load` event* yet, and a frame with no author height and no
    `auto-height` is 0 tall (strict containment).
  - *A `src` change keeps the frame's last `data` and `global-props`*
    (user ruling 2026-10-15): every boot — the first, a new `src`'s, a
    reconnected frame's — reads the last values written, as web-core's
    `LynxView` renders every card with its current `initData`/`globalProps`
    and Harmony re-sends both. Native Android and iOS consume the pending
    values at the first load, so a new `src` boots with none unless `data`
    is written again before its bundle arrives (`UIFrame.java:117-129,
    334-349`, `LynxUIFrame.mm:96-104, 173-186`).
  - *`data`/`global-props` take plain objects only* (my decision): a
    non-null, non-array object, or JSON text of one; anything else is
    ignored, where Android clears the pending value and web-core passes a
    string through `JSON.parse`. A removal leaves the running card as it is
    and makes the next boot's value `{}` (native's reset; web-core's string
    path). Every write is an update, with no deduplication, as on both.
  - *`auto-width` with no author height takes its content's height too*
    (user ruling 2026-10-15, native's `auto-width` over web-core's none):
    `frame[auto-width]` is `contain: layout paint style`, since CSS has no
    containment of the height alone; native leaves such a frame 0 tall.
    `auto-height` is web-core's `contain: content` over its
    `container-type: inline-size`, i.e. `inline-size layout paint style`.
  - *`preset-width`/`preset-height` and `enable-multi-async-thread` do
    nothing* (my decision, web-core's answer): native's presets are the
    nested page's viewport before the frame has a content rect, which never
    happens when the nested page lays out in the frame's own pass, and its
    `enable-multi-async-thread` picks threads.
  - *No frame methods* (user ruling 2026-10-15, native's surface): a frame
    answers the shared base set and code 3 for anything else. web-core's
    `invoke` reaches its `lynx-view`'s `updateData`, `updateGlobalProps`
    and `reload` by reflection, and a card can write `init-data`,
    `browser-config` and `transform-*` on it; none is adopted.
  - *No group inheritance of global props*: Android applies the host
    `LynxViewGroup`'s group global props before the frame's own
    (`LynxTemplateRender.java:1686-1692`); web-core and this engine start a
    frame's global props from what the frame was given.
  - *The nested card's BTS is a `Worker` its `ShadowRealm` child owns*
    (user ruling 2026-10-15): `frame-boot.ts` connects
    `new Worker("bobcat:bts")` at `U#background-thread` as the page boot
    does, the worker runs on the group's worker runtime beside the page's
    BTS, and its messages are delivered to the child. Native and web-core
    both start a full nested runtime, so a compiled card hydrates, runs its
    effects and re-renders on a later `data`/`global-props` write as there
    (web-core-e2e `api-frame-data-update`, ported in
    `crates/bobcat-source/tests/frame.rs`). Where the two references
    disagree:
    - *`sharedData` is the group's* (user ruling 2026-10-15, native): the
      frame's BTS reads and writes the same `lynx.setSharedData` store as
      the page's BTS and every other view of the group, and the observers of
      all of them run on a set. Native's nested render joins its parent's
      `LynxGroup`, sharing its JS context when that group is shared, which a
      group here always is (`LynxTemplateRender.java:4280-4305`,
      `runtime_manager.cc:167-200`); web-core gives a frame a dedicated
      Worker, never the parent's `lynx-group-id`, so its own `sharedData`
      (`Background.ts:66-69, 155-171`).
    - *`NativeModules` are the view's* (user ruling 2026-10-15, native):
      `bobcat:frame` hands the child the view's module table and the
      frame's BTS calls the view's modules, answered in that BTS. Native's
      nested render inherits its parent's module wrappers
      (`LynxBackgroundRuntimeOptions.java:251-266`); web-core gives a frame
      `{}` and three stub modules that never answer
      (`LynxViewInstance.ts:117-118`).
    - *`SystemInfo` is the parent's* (my decision, native's): web-core gives
      a frame the default, with no `browserConfig`.
    - *A frame's BTS reports as any BTS* (my decision): `WorkerThrew` and
      `WorkerEnded` name `worker <id> bobcat:bts`, non-fatal, so an embedder
      cannot tell it from the page's.
    - *`lynx.stopExposure`/`resumeExposure` from a frame's BTS switch the
      whole document's exposure* (my decision): both references scope it to
      the nested view; here exposure is the document's.
    - *Disposal does not hold the frame* (my decision): a removal or a new
      `src` runs the old card's BTS `dispose` (ReactLynx's `destroyCard`,
      its unmount effects) and frees the card's nested page at once, without
      waiting for the acknowledgement; the old child is kept until its BTS
      acknowledges and ignores what that BTS says meanwhile. As the view ends
      the page closes its frames first, so each frame card's BTS gets
      `dispose` ahead of the page BTS's, but nothing waits for their
      acknowledgements. web-core awaits its nested instance's dispose RPC
      before terminating the Worker; native destroys the nested render with
      the frame.
  - *Events reach the card in its own realm, called by the kernel at the
    path's top* (user rulings 2026-10-16 and 2026-10-18, scheme D, and
    2026-10-19; "Each realm's Lynx walk runs at the path's top step", Event
    model & gestures): the page realm is called first, then the card's, at
    the same step and with the same path, each running its own Lynx walk
    over the steps it holds handles for, with no forwarding between them.
    Where the references disagree:
    - *A catch stops one card's walk alone* (web-core): a `catchtap` in the
      card does not stop the page's `bindtap` on the frame's ancestors, and a
      page `capture-catch` does not stop the card, as each web-core
      `lynx-view` walks its own shadow root (`event_apis.rs:406-432`).
      Native runs one propagation across both pages
      (`LynxEventEmitter.java:118-158`, `touch_event_handler.cc:1537-1931`),
      where either catch ends it; not adopted.
    - *Every card hears every key* (web-core): a card with a
      `global-bindkeydown`/`keyup` is called for each key and, holding no
      step of the key's path, runs that global pass alone, as web-core's
      nested binding listens on `document`; native Android/iOS have no
      `keydown`. Every other global registration of a card is reached only by
      a path through its frame's shadow root, as web-core's card listens on
      its own shadow root.
    - *The card's `detail.x/y` is frame-local* (my decision; both references
      agree): the card's pointer, wheel and touch `x/y` and its position
      `detail` are relative to the frame's border box, read once per
      dispatch from the layout walk (`boundingClientRect`, so transforms are
      ignored); `clientX/Y`/`pageX/Y` stay the viewport's, as web-core's
      `clientX/Y` are. A frame with no box — one a listener earlier in the
      same dispatch removed — answers the viewport's origin.
    - *A frame an earlier listener removed still has its card called* in the
      same dispatch: the realms a dispatch calls are fixed before its walk,
      and the card keeps its registrations until the frame lets go of it,
      which the disconnection starts in a later entry. A new `src` written
      by an earlier listener lets go of the old card at once: `resetFrame`
      frees its page, its registrations leave with its nodes, and its call
      is skipped, since it holds nothing for the name any more.
    - *The card's exposure records go to the page's BTS* (deferred): a
      card's elements' `exposure`/`disexposure` lists still go to the page
      realm's BTS, where both references keep exposure per card.
    - *A frame's connection calls reach the page realm alone* (deferred):
      `__BobcatFrameConnected`/`__BobcatFrameDisconnected` are not routed to
      the realm whose scope holds the frame's handle, so a frame inside a
      nested card that the card disconnects without letting go of its handle
      keeps its card.
  - *A relative `src` resolves against the view's base*, not the host card's
    URL: `bobcat:frame` resolves `src` with `resolveUrl`, which is the
    resolution the fetcher makes against `ViewSources::base_url`, and the
    child's `__Card__` is that absolute URL, so a relative `src` inside a
    nested card also resolves against the view's base rather than against
    the nested card's URL.
    web-core fetches its `url` against the embedding document's base
    (`templateManager.fetchBundle` in `LynxView.ts`), which nests no base per
    card either; native hands `src` as written to its resource loader
    (`LazyBundleLoader::LoadFrameBundle`), whose resolution is the
    platform's.
  - *Only a `.web.bundle` (or another container `ContainerSources`
    registers a `root` for) boots in a frame*: the boot loads
    `<U>/root.js`. A source-built `.lynx.bundle` names its main-thread
    entry `<name>__main-thread` and has no `root`, so its load fails with one
    diagnostic and the frame stays empty.
  - *The nested card's first flush is queued a microtask after its
    `renderPage`*, as the page boot queues its own; until the page's first
    flush has settled the listed sheets and the painter binding, a child's
    flush commits nothing and the epilogue's commit catches up.

- **`markdown` / `x-markdown` (2026-10-14)** — Lynx-only (bucket 2): native
  registers `markdown`, web-core `x-markdown`; both are defined, on one
  component (`bobcat-core`'s `tree::markdown`). web-core's structure is
  followed — a UA shadow tree whose `#markdown-root` holds the elements
  markdown-it renders — and where it cannot be, or where web-core, native and
  the typings disagree:
  - *The shadow tree is HTML-named elements over standard inline formatting
    contexts, but no block flow.* The elements are markdown-it's (`p`,
    `h1`-`h6`, `ul`/`ol`/`li`, `blockquote`, `pre > code`, `table`, `a`,
    `strong`, …) so `markdown-style`'s selectors match as in web-core. Every
    text holder is `display: block`, one CSS inline formatting context, and
    every phrasing element `display: inline`, an inline box with the full
    inline box model (the engine's own shadow sheet can write those values;
    `markdown-style`, an author sheet, cannot). Block-in-block flow is not
    built, so every block container is a column flex box: margins between
    blocks add up where a browser collapses them (a heading after a
    paragraph is 0.75em + 0.8em apart, not 0.8em), and an image, which
    web-core makes `display: block`, is an atomic inline in its line.
    Two element kinds are the engine's own: a line holder for text
    markdown-it writes straight into a container (a tight list item's text),
    and a list marker.
  - *List markers and tables are approximations.* A marker is an element
    holding `•`/`◦`/`▪` (by list depth, the browser's `disc`/`circle`/
    `square`) or `N.`, absolutely placed at the item's start edge in the
    list's 40px padding, where a browser draws an outside `::marker`. A table
    is a grid of `auto` columns with `thead`/`tbody`/`tr` as `display:
    contents`, the browser's 2px `border-spacing` as gap and padding; it does
    not size rows and columns by the table algorithm.
  - *`pre` scrolls with `overflow-x: scroll; overflow-y: hidden`* for
    web-core's `overflow: auto` (no `auto` here), and its code paragraph is
    HTML's `pre`: `white-space-collapse: preserve` (spaces, tabs to 8-column
    stops, line breaks) with `text-wrap-mode: nowrap`. The `preserve` mode
    landed with this component ([css-text.md](css-text.md)); `tab-size` is
    not a property here, so the stop is 8.
  - *An image is an atomic box in its line* (the engine's `image` with
    `auto-size`), where web-core makes its `<img>` `display: block`: text
    beside an image in one paragraph shares its line.
  - *Rendering reparses and keeps the equal prefix.* Every `content` or
    `content-range` change renders synchronously, reparses the whole content
    and keeps the elements of the leading top-level blocks equal to the
    previous parse's. web-core appends a chunk up to the last line break and
    flushes the rest on a timer (`#appendIncrementally`,
    `XMarkdownAttributes.ts:759-782`); here the block an append changes is
    rebuilt, and markup an append completes restyles what came before it, as
    a full render does. `parseEnd` is queued after every render of
    non-empty content; web-core skips it for an append ending in a line
    break.
  - *`getContent`'s `end` is inclusive* (web-core, `XMarkdown.ts:240-246`:
    `end` defaults to the last unit, the slice is `[start, end + 1)`, and
    `end < start` is the empty string). Native's `end` is exclusive and
    `start >= end` is code 4 (`LynxUIMarkdown.m:105-134`).
  - *`getImages` answers `{images}` and `getParseResult` `{id, result}`* —
    the typings' and native's shapes; web-core answers the bare array and
    the bare map. `id` is the `content-id`, `""` without one (native).
  - *Offsets are web-core's rendered-text offsets.* `getParseResult`,
    `getTextBoundingRect` and `getCharIndexByPoint` count in the shadow
    root's `textContent`, including the `"\n"` text nodes markdown-it writes
    between block tags (counted, not built: they render nothing). Native's
    `getParseResult` answers source ranges instead.
  - *`getTextBoundingRect` answers per-line `boxes` too*, the `text`
    component's record, where web-core answers `{boundingRect}` of a DOM
    range. With neither `start` nor `end` web-core measures the selection;
    there is no selection model, so that is code 8.
  - *`getCharIndexByPoint` is native's* (web-core has none): `x`/`y`
    relative to the element's border box, the rendered-text index (or, with
    `indexType: "source"`, the source offset web-core's greedy prefix walk
    matches it at); nothing under the point is code 8, native's 1.
  - *`link`, `imageTap` and `parseEnd` do not bubble and are composed*
    (user ruling 2026-10-14). Not bubbling is native's: a component's custom
    event is dispatched with `bubbles_ = false, composed_ = false` on the
    legacy path (`lynx/core/renderer/events/touch_event_handler.cc:399-403`)
    and as an `event::CustomEvent` whose defaults are `Bubbles::kNo`,
    `ComposedMode::kScoped` on the refactored one (`:388-394`,
    `lynx/core/event/custom_event.h:17-23`), so only a `bindlink` on the
    element itself hears it, never an ancestor's nor `global-bindEvent`.
    Composed departs from native's flag so that script's `target` is the
    markdown element itself. web-core fires all three
    `bubbles: true, composed: true` (`XMarkdownAttributes.ts:627-643,
    994-995`), which no ReactLynx handler can observe there, because its
    events are misnamed (next bullet). `link` and `imageTap` come from a
    `tap`: a paragraph's runs have no box, so the anchor under a tap is
    found from the character it hits.
  - *The events are named `link`, `imageTap` and `parseEnd`*, native's names
    (`LynxMarkdownEventDispatcher.m`), so a compiled card's `bindlink`,
    `bindimageTap` and `bindparseEnd` handlers run. web-core's `XMarkdown`
    dispatches DOM events named `bindlink`, `bindimageTap` and
    `bindparseEnd` (`XMarkdownAttributes.ts:627-643, 994-995`) while its
    element API listens for the lowercased name without the prefix
    (`event_apis.rs:33`, `WASMJSBinding.ts:262-280`), so under web-core a
    ReactLynx handler for them never fires and only a direct
    `addEventListener('bindlink')` hears them (its own fixture,
    `tests/fixtures/x-markdown/events.html`). Read from the code, not run;
    a web-core defect this engine does not copy.
  - *Links follow markdown-it 15 as web-core configures it*:
    `www.example.com` and `example.com` are not autolinked (linkify-it's
    `fuzzyLink` is off there; verified against markdown-it), `https://`,
    `ftp://` and e-mail addresses are; `javascript:`, `vbscript:`, `file:`
    and non-image `data:` destinations are refused and stay text. Not
    replicated: scheme-relative `//host` autolinks (markdown-it links them),
    and punycode for non-ASCII hosts (an `href` keeps the host
    percent-encoded).
  - *Deferred* (no reference behavior is approximated): the typewriter
    animation and its attributes, events and the `typewriterCursor` style
    (over the document's component timers, `Autoplays`, PR #408, as a
    follow-up); `text-maxline`, `overflow` and the `truncation` marker;
    `markdown-effect`; selection (`text-selection`, `getSelectedText`,
    `setTextSelection`, `selectionchange`); `childrenexpose`/
    `exposure-tags`; the native-only `allow-break-around-punctuation`,
    `image-downsampling`, `text-mark-attachments` and `markdown-max-height`.
    `pauseAnimation`, `resumeAnimation`, `getSelectedText` and
    `setTextSelection` answer code 3.
  - *The host is a border box that clips*, the box block every container
    tag gets, with `x-markdown.css`'s column display; web-core's host is in
    no such block.

- **`x-input` / `x-textarea` (2026-10-14, editable text E5)** — Lynx-only
  (bucket 2): web-core wraps a real `<input>`/`<textarea>` in a shadow
  tree; here the shadow tree holds one plain-text editing host over
  `dom::editing` (`bobcat-core`'s `tree::text_field`,
  `docs/editable-text-design.md` E5). Where native Lynx and web-core (or
  the Lynx typings) disagree, web-core was followed:
  - *`send-composing-input` is off by default* (web-core). Native sends
    `input` during a composition by default; web-core suppresses it until
    `compositionend` unless the attribute is present.
  - *`maxlength` defaults to 140* (web-core and the typings). Native has no
    limit by default.
  - *`maxlength` counts UTF-16 units* (web-core, the browser's rule) and
    takes back the excess from the end of the text just inserted, never
    half a surrogate pair. Native counts UTF-16 units too but never splits a
    composed character sequence, so a grapheme cluster of several code
    points can be cut here where native drops it whole.
  - *`input-filter` removes what the pattern matches* (web-core's
    `value.replace(new RegExp(filter, 'g'), '')`). Native keeps only the
    characters that match. The pattern is compiled by `regex-lite`, not an
    ECMAScript engine: `\uXXXX` escapes are translated, and lookaround and
    backreferences, which it lacks, make the pattern filter nothing.
  - *`setInputFilter({pattern})` filters.* web-core's writes the inner
    `<input>`'s `pattern` attribute, which filters nothing; here it
    replaces `input-filter`'s pattern, the last write winning.
  - *An `x-textarea` confirms only with `confirm-enter`* (web-core). Native
    fires `confirm` on Enter whenever `confirm-type` is not the default.
    Here `confirm` follows that Enter's `input` (web-core fires it on the
    `keyup`), so an Enter the input method consumed does not confirm.
  - *`getValue` answers a field that is not focused* with a caret at the
    end of the value (web-core's `<input>` keeps that selection); native
    answers `-1` for both offsets. The data carries the typings'
    `selectionStart` and web-core's `selectionBegin`, both.
  - *`selection` fires for a selected range only* (web-core's `select`):
    a change of selection that changed no value and leaves text selected.
    Native fires it on every selection change, caret moves included.
  - *`readonly` cannot be focused.* A browser's (and native's) read-only
    field takes focus and a caret but no edit; here `readonly`, like
    `disabled`, turns the editor's `contenteditable` off, so a tap focuses
    nothing and no `focus` fires.
  - *A `password` field is not masked.* web-core's `<input type=password>`
    shows bullets; here the text shows as typed, because masking needs a
    paragraph built from other text than the value, over which `dom`
    refuses edits. `TextInputState::input_type` is `Password`.
  - *No intrinsic width.* An unsized field in a row is as wide as its text;
    a browser's `<input>` is about 20 characters wide.
  - *The single-line field does not follow its caret.* Text wider than the
    field is clipped; a browser scrolls its inner editor to keep the caret
    in view.
  - *`placeholder-font-style` and the `-x-placeholder-*` aliases* are
    native's and not web-core's, and not built.

## JS runtime & APIs (see [js-runtime.md](js-runtime.md), [accessibility.md](accessibility.md))

- **`ShadowRealm` children have no animation frames, and report under their
  parent's source** (2026-10-13; `crates/bobcat-core/src/realm/child.rs`).
  `requestAnimationFrame` is a member of a window's or a dedicated worker's
  global, not of a ShadowRealm's; here a child has the realm core, whose
  `requestScriptFrame` member is installed as a **no-op**, so a child that
  imports `bobcat:animation-frame` links and its callbacks never run. The
  display-frame demand is one flag per realm key — `None` for the MTS realm,
  the worker's key for a worker — and a child has no key of its own: one
  reporting under its parent's would cancel the parent's demand with its own
  `false`. A child's `ConsoleMessage` and `ScriptReported`, and the failures
  its owner reports for it, carry the **parent's** `ScriptSource` (`main`, or
  `worker <id> <url>`), so an embedder cannot tell a child's output from its
  parent's; a source of its own is a follow-up. A child is also freed only
  by its owner's epilogue, once nothing of another realm refers to it, it has
  no timer armed and no load or settle in flight — a child held only by
  cyclic garbage waits for a collection.
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
  are not accepted at all; `boundingClientRect` reads nothing from its
  `params`.
- **The Intersection Observer primitive includes transforms while
  `invoke('boundingClientRect')` does not** (user-directed 2026-10-08). The
  observer is bucket 1 — web-core delegates to the browser's
  `IntersectionObserver`, whose rects are `getBoundingClientRect()`'s and so
  transformed — and native's platform observers ignore transforms unless
  `enableTransformForPositionCalculation` is on. **Decision: follow the
  browser** for the observer (a 100px target under `translateX(500px)` in a
  300px viewport is not intersecting) and keep the earlier engine-path
  decision for `boundingClientRect`, so the two readbacks of one element can
  differ under a transform. `isIntersecting` is Chromium's clipped,
  edge-inclusive answer and `rootMargin` percentages resolve by
  height/width as every engine does; both recorded in
  `docs/style-assumptions.md` §32. Every observer also carries a target
  margin the spec does not have, which dilates each target's border box
  before its transform and every clip, with the entry's rects and ratio
  over the dilated rect. The reason is native exposure, which grows the
  element's own rect and so reaches an element a clipping ancestor cut,
  where a root margin cannot (§32). No script surface exists yet.
- **Lynx exposure is the primitive's geometry, native's switches and
  web-core's payload** (user-directed 2026-10-11;
  `crates/bobcat-core/src/main/exposure.rs`). The references disagree on
  nearly every point, so the decisions are listed. Rulings: an element is
  exposed when the W3C observer says it intersects the viewport through
  every clipping ancestor with a ratio of at least `exposure-area`
  (web-core's browser observer uses the nearest scroll container *instead
  of* the viewport; native tests the screen, the root and each scroll
  ancestor's whole rect separately); `exposure-ui-margin-*` always apply, as
  web-core's do: neither the per-element `enable-exposure-ui-margin`
  attribute nor the `enableExposureUIMargin` page config is read
  (user-directed 2026-10-12; native applies the margins only under the
  attribute or, without it, the page config); `lynx.stopExposure` stops detection and `resumeExposure` restarts
  it (web-core's flag is read nowhere else); delivery is one batch per
  update with no timer (web-core coalesces the global lists on a 50 ms
  timer, native polls at 50 ms), and `setObserverFrameRate` is accepted and
  ignored. Defaults taken from web-core: the `uiappear` detail and the
  global record shapes (`unique-id` is the element's uid; no `sign`,
  `extra-data`, `dataSet` or `internal-signature`), `exposure` before
  `disexposure` within a batch (native sends `disexposure` first), identity
  is the element (two elements with one id+scene are two records), a
  teardown or an `exposure-id` change emits a `disexposure` record with the
  old id and no `uidisappear`, `exposure-area` is `parseFloat(value) / 100`,
  zero-area and edge-adjacent targets count as intersecting, and
  `visibility: hidden`/`opacity: 0` do not hide (native checks alpha and
  hidden). Defaults taken from native: global records need a non-empty
  `exposure-id` (web-core emits records with a null id for listener-only
  elements), a freed exposed element emits one `disexposure` (web-core
  emits nothing for a removed node), `rpx` margins resolve (web-core:
  0), and every registration form arms an element — an
  `__AddEventListener` closure for `uiappear` included, since native syncs
  every form into the element's event set (web-core marks from
  `__AddEvent` alone). One more from web-core: `lynx.stopExposure({})`,
  an object naming no `sendEvent`, sends the records (`sendEvent ?? true`;
  native sends nothing for it). Margins (2026-10-12) fold into the
  observer's root and target margins: a ui margin on the target's same
  side, a positive screen margin on the target's opposite side and a
  negative one on the root's same side, native's logic without the
  switch; a `%` is of the rect it lands on, so a ui margin's is of the
  element's own size as native's is (a positive screen margin's of the
  element's, a negative one's of the viewport's; native has no screen `%`),
  and a `%` and a length that land on one side keep the length. Two
  deviations from native follow: with the ui switch on native puts every
  screen margin on the screen rect, and here a positive one stays on the
  element, which gives the same intersect-or-not answer at
  `exposure-area` 0 and a ratio over the grown element above it, because a
  root margin cannot reach past the page's clip or a scroller's clip,
  which native's independent per-rect tests do reach, and the 2026-10-11
  ruling keeps the clip-through geometry; and an element a negative ui
  margin shrinks past its size keeps the primitive's zero-area rule
  (intersecting, ratio 1) where native says it is not in the window.
  web-core folds every margin into one `rootMargin` of the nearest scroll
  container. Not modelled: `enable-exposure-ui-clip` (every clipping ancestor
  clips), native's iOS flap filter
  and `enableCheckExposureOptimize`, the other exposure page configs
  (`enableDisexposureWhenLynxHidden`, `enableExposureWhenLayout`,
  `enableExposureWhenReload`, `enableDisexposureWhenBackground`,
  `enableiOSAnimationLayerForExposure`), overlay special-casing (the top
  layer escapes to the viewport in the primitive already) and the native
  component registration API (`uniqueID`/`extraData`/`useOptions`).
- **The rect carries `id` and `dataset`** — native's result bundles both
  (`LynxUI.m`, `platform_event_target_helper.cc`); web-core's carries the
  geometry and the id only, because DOM `getBoundingClientRect()` has neither.
  **Decision: follow native** and include the typed `dataset` copy, which is
  what a card that measures a list row then reads its keys off the same answer
  expects. This is one of the places where the web default is not taken.
- **A dataset entry is matched by `[data-<key>]`, and the three reference
  rules for it disagree** — native has no `data-*` attribute at all: its
  `SelectorQuery` matcher reads the typed dataset for a `data-` name
  (`core/renderer/dom/selector/matching/attribute_selector_matching.cc:24-28`),
  so `[data-k]` matches whenever the key exists, a null value included, the
  key is compared case-sensitively, and a value is compared as text only when
  it is a string (`Value::ToString`, `base/src/value/base_value.cc:1181-1193`,
  is `""` for every other type); its stylesheet matcher has no attribute
  selectors (`core/renderer/css/ng/matcher/selector_matcher.cc`). web-core
  reflects the dataset as DOM attributes, by two rules. `__AddDataset`
  (`web-core/ts/client/mainthread/elementAPIs/createElementAPI.ts:434-446`)
  removes the attribute for any falsy value and otherwise writes an object's
  JSON text or the value's `toString()`. `__SetDataset`
  (`web-core/src/main_thread/client/element_apis/dataset_apis.rs:12-55`)
  replaces the dataset, removes the attribute of every key the new object
  lacks and of a null or undefined value, writes a string as it is and writes
  `""` for every other type, and skips a key whose value is unchanged. Both
  spell the name `data-${key}` through `setAttribute`, which lowercases it.
  **Implemented, not ruled:** the typed store keeps native's merge
  (`AttributeHolder::SetDataSet`, `core/renderer/dom/attribute_holder.cc:229-234`),
  the attributes follow the typed
  store — so `__SetDataset` removes no attribute for a key it does not name —
  and one text rule serves both members, web-core's `__AddDataset`, because
  that is the member a compiled `data-*` JSX attribute reaches (`__SetDataset`
  is the spread path) and the only one of the three that lets `[data-n="7"]`
  match a number. Visible differences: from native, `[data-k]` does not match
  a key whose value is falsy, `[data-k="7"]` matches the number 7, and
  `[data-testid]` matches the key `testId`; from web-core's `__SetDataset`, a
  key dropped from a spread keeps its attribute, a non-string value is its
  string instead of `""`, and `false`, `0` and `""` have no attribute. A value
  JSON cannot carry (a cycle) is `""` here, where web-core's `__AddDataset`
  throws.
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
  an ES module here (`ContainerSources`, registered under the container's
  URL), and `lynx.requireModule`,
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
  web-core pushes the container's StyleInfo during `loadExternalBundle` with
  the container's URL as its entry name, so each rule also carries
  `[l-e-name="<url>"]` (`web-core/ts/client/mainthread/LynxViewInstance.ts:181-192`,
  rule 5 of `style_info_decoder.rs`). (Here the decision is `bobcat-source`'s:
  `bobcat-core` never sees the container at all. `ContainerSources` — the one
  registration path a page and a fetched container share,
  `crates/bobcat-source/src/container.rs` — *registers* the StyleInfo at
  `<bundle>/style-info.css` either way, and only a page's view lists it, so a
  lazy container's is held and never adopted; a `<frame src>` fetches a full
  card through the same path and adopts its StyleInfo into the frame's shadow
  root, which `rLynxPrepareLazyBundleMTS` never asks for.) **Decision: native**, because the compiled card
  already makes that call and web-core's extra push would mount the same rules
  twice — and without that entry-name guard: css-id guards are synthesized
  here (`lower_style`), the `l-e-name` guard is not, and `__SetCSSId`
  ignores its `entryName`. A container's `config` is ignored too: page policy
  is the page's.
- **CSS custom sections are read from the `CustomSectionsStyleInfo` section
  *and* from the legacy JSON form** — since lynx-stack#4138 (75c770fe)
  web-core's encoder writes every `encoding: 'CSS'` custom section into
  `.web.bundle` label 7 (an rkyv `RawStyleInfoMap`) and no longer into the
  JSON `CustomSections` section, while web-core's own decoders accept label 7
  but do not consume it yet (`ts/server/decode.ts`,
  `ts/client/decodeWorker/decode.worker.ts`), so on web-core
  `getCustomSectionSync(name)` answers `undefined` for such a section. This
  engine lowers label 7 into the named stylesheets `__LoadStyleSheet` loads,
  and **keeps lowering the legacy JSON `{encoding:"CSS", content:{ruleList}}`
  descriptor** for bundles an encoder before #4138 wrote
  (`crates/bobcat-source/src/custom_style.rs`); a name label 7 carries wins
  over a JSON descriptor of the same name. No bundle in this workspace — the
  fixtures or the pinned `lynx-examples` tarballs of the catalog — carries a
  JSON CSS section today, so the legacy path can be removed once no supported
  bundle predates #4138. **Decision: read both**, so that neither encoder
  generation loses its named CSS.
- **`__LoadStyleSheet` for a section a container does not carry answers
  `null`; native answers `undefined`** — native's `LoadStyleSheet` returns
  `undefined` for a missing section
  (`lynx/core/runtime/lepus/bindings/renderer_functions.cc:963-968`). Here the
  realm asks the fetcher's `ResourceFetcher::style_sheet_probe` through the
  `hasStyleSheet` host member — synchronous, registry-only in
  `bobcat-resources`, never a request — and answers `null` when nothing is
  registered at `<bundle>/custom_section/<key>.css`. **Resolved: `null`**,
  because ReactLynx adopts only a handle that is `!== null`
  (`rLynxPrepareLazyBundleMTS`, the sync lazy path): `null` makes it adopt
  nothing, as native's `undefined` does there, where `undefined` would reach
  `__AdoptStyleSheet` and throw. `react-lazy-nested`'s inner container, which
  has no `CSS` section, runs with no report and no stylesheet request
  (`crates/bobcat-source/tests/lazy_bundle.rs`).
- **`NativeModules.<name>` for a module the host does not have** — native's
  `LynxJSIModuleBinding::get` answers `null`
  (`lynx_jsi_module_binding.cc:23`), while web-core builds a plain object out
  of `createNativeModules`, so a missing name is simply `undefined`.
  **Decision: `undefined`** — web-core is the default resolution, and every
  in-repo consumer tolerates either (compiled bundles probe with `?.`,
  explorer-lib guards on `typeof`). A method a module did not declare is
  `undefined` on both references, and so here. MTS `NativeModules` stays
  `undefined` altogether, as it is in both references (see the next entry).
- **No native module API on the main thread** — native's MTS has one,
  `lynx.module(name).invoke(method, ...args)`: it returns synchronously, turns
  each function argument into a callback, and is off by default, turned on by
  the host's `enableMTSModule` (`renderer_ng.cc`, `lynx_lepus_module.cc`).
  Neither reference has a `NativeModules` global on the main thread, and
  ReactLynx sets it to `undefined` there. web-core's MTS has no native module
  path at all. **Decision: web-core's surface for now, with the transport in
  place** (user ruling): no MTS API is built yet, and `NativeModules` stays
  `undefined`. The MTS realm declares `bobcat-internal:native-modules`, whose
  one member is `invokeNativeModule`, `bobcat:native-modules` links there,
  and `LynxView::pump` answers a call it makes through the view's command
  FIFO (`ToMain::ModuleCallback`), so the native API can be added later
  without a second reply path. That answer is the one command sent outside
  the seat's counting `CommandSender`, so a frame post's fence does not count
  it.
- **`lynx.reportError(error, {level: 'fatal'})` is reported and nothing more**
  — lynx-core's `reportError` maps `'fatal'` to `LynxErrorLevel.Fatal`
  (`lynx.ts`), and native's `App::ReportException` then sets the BTS app state
  to `kAppLoadFailed` (`js_app.cc`), so the BTS stops taking calls into its app
  while the view and MTS go on. web-core ignores the level altogether
  (`createNativeApp.ts` `reportException`). **Decision: report it as a level**
  (user ruling): `EngineEvent::ScriptReported` carries `"fatal"` from the realm
  that reported it, beside `"warn"` for `'warning'` and `"error"` for anything
  else, and the engine does nothing more for it — the BTS keeps taking messages
  and calls, and no realm and no view ends.
- **A `Worker` whose script cannot be loaded is reported and stays** — HTML's
  "run a worker" (HTML Standard, 10.2.4 Processing model) fetches the script
  and its module graph before anything runs; when that fetch fails, or the
  script has an error to rethrow, it fires `error` at the `Worker` object, runs
  the environment discarding steps and stops, so the worker never runs and
  nothing is left of it. Here a worker's realm opens as its `Start` is served
  and loads the module at its URL as its root module, and the request that load
  makes is an ordinary module request, sent and completed as an import's is
  (`../runtime-architecture.md`). **Decision: no path of the root's own** (user
  ruling): a script the fetcher could not load, or answered with something
  other than a script, rejects that load in the realm, exactly as a module the
  script imports does. It is one `WorkerPayload::Errored`: the `Worker` object
  gets its one `error` event, as in HTML, and the embedder gets
  `EngineEvent::WorkerThrew` (context `loading a worker module`), not
  `WorkerEnded`. The realm stays allocated until `terminate()`, the collection
  of the `Worker` object or the release of the creating realm, which is what
  `new Worker("bobcat:nope")` leaves as well. Nothing ran in it, so what is
  posted to it is dropped and nothing is reported for the post. Two more
  consequences of the same rule: the request leaves `bobcat-workers` in the
  worker's boot job, not at construction, so after
  `new Worker(url).terminate()` the host sees no request for `url` or one,
  which reads as cancelled once the worker has ended; and the only ordering
  between a `terminate()` and the script is an import's, that the script does
  not run when the worker has read the `Terminate` before the job that
  completes the script runs.
- **`lynx.setSharedData`'s store has no prototype** (2026-10-15;
  `packages/bobcat-element/src/background-thread-runtime.ts`). lynx-core's
  store is a `{}` on the group's shared JS global, so a key no card set but
  `Object.prototype` has — `getSharedData('toString')` — answers that member.
  Here the store is `Object.create(null)` on the runtime's shared object,
  because every view's BTS is a realm of its own and a `{}` would carry the
  first view's intrinsics: such a key reads `undefined`. Everything else is
  lynx-core's: values by reference, and every observer of the group called
  inside the set.
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
