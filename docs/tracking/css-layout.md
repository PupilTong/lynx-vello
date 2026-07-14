# CSS box model, positioning & layout

> Research: multi-agent sweep over `lynx/` and `lynx-stack/` (see [AGENTS.md](../../AGENTS.md) for the reference-repo shorthand and the W3C-first standards policy). Supersedes the earlier stub.

### Layout Engine: Box Model, Positioning, Flex/Grid, and Lynx-Specific Layout Primitives

Source of truth: Lynx's native layout engine is **Starlight**
(`lynx/core/renderer/starlight/{layout,style,types}`), a from-scratch C++ layout
engine (not a fork of Yoga/Taffy). Its property surface is generated from
`lynx/tools/css_generator/property_index.json` plus one
`css_defines/<id>-<name>.json` file per property (236 properties total as of
`count: 236`); layout-affecting ones are tagged
`"consumption_status": "layout-only"` and land in `LayoutComputedStyle`.
Algorithms live one-per-mode: `flex_layout_algorithm.cc`,
`linear_layout_algorithm.cc`, `relative_layout_algorithm.cc`,
`grid_layout_algorithm.cc`, `staggered_grid_layout_algorithm.cc`, and
`position_layout_utils.cc`. `display` in Lynx is *only* an
internal/child-layout-mode switch (flex/linear/grid/relative/none) — it does
**not** carry CSS's external inline/block dichotomy (confirmed in
`lynx/tools/css_generator/css_defines/24-display.json`: "does not determine the
external display type"). Flexbox and Grid closely mirror the W3C algorithms
step-by-step: `flex_layout_algorithm.h` cites "Algorithm-3" through
"Algorithm-15" matching the CSS Flexbox resolution steps, and
`grid_layout_algorithm.h` uses the Grid track-sizing terms "intrinsic track
sizes", "infinitely growable", and "flexible tracks" verbatim.

The two non-standard layout modes unique to Lynx are **linear**
(Android-`LinearLayout`-derived: `display:linear` plus `linear-*` properties)
and **relative** (Android-`RelativeLayout`-derived: `display:relative` plus
`relative-*` properties and id-based sibling anchoring — unrelated to CSS
`position:relative`). `lynx-stack/packages/web-platform/web-core` does not
reimplement Starlight; it polyfills Linear on top of browser CSS Flexbox via
custom elements and CSS custom properties, while Relative has no web-core
implementation. For example,
`lynx-stack/packages/web-platform/web-core/src/style_transformer/rules.rs`
rewrites `display:linear` to `--lynx-display:linear; display:flex` plus
`--lynx-linear-orientation` custom properties consumed by
`lynx-stack/packages/web-platform/web-elements/**/LinearContainer*`.
lynx-vello implements both Linear and Relative as first-class algorithms in
`neutron-star`, alongside Flex and Grid. Linear is not a Flex polyfill, and
Relative follows its standalone Starlight contract rather than nonexistent
web-core prior art.

**Z-index / stacking context — confirmed W3C deviation.** Lynx does **not** implement the CSS stacking-context algorithm (a per-stacking-context, recursively-scoped paint order with 7 layers: negative z-index → block-level in-flow → floats → inline in-flow → positioned/z-index:0 descendants recursively → positive z-index, each subtree re-entering the same algorithm). Instead (`lynx/core/renderer/dom/element_container.cc`, `ElementContainer::ZIndexChanged`/`UpdateZIndexList`, and `Element::IsStackingContextNode` in `lynx/core/renderer/dom/element.cc:1903`):
- A node is a "stacking context node" only if it is the root, or `has_z_props()` (non-zero `z-index` while positioned), or `is_fixed_`, or has a transform, or has opacity — a much smaller trigger set than CSS (CSS also creates stacking contexts for `will-change`, CSS filters, `isolation:isolate`, `mix-blend-mode`, flex/grid items with z-index≠auto, masks, clip-path, contain:layout/paint, etc. — several of which Lynx's list omits, e.g. filter/mask/clip-path are CSS properties Lynx supports but does **not** list as stacking-context triggers in `IsStackingContextNode()`).
- Any child element whose resolved `z-index != 0` (not just any non-`auto` value — Lynx has no `auto` keyword state distinct from `0`) is **pulled out of the normal container tree and re-parented once** to the *nearest enclosing stacking-context node's* `ElementContainer` (`EnclosingStackingContextNode()` + `MoveZChildrenRecursively`), flattening every intervening non-stacking-context ancestor. This is a single global reparenting per stacking-context "island", not a recursive per-ancestor composition.
- Within that flattened list, siblings are `std::stable_sort`-ed purely by integer `ZIndex()` value (`UpdateZIndexList`), with negative-z children spliced to the front of the parent's child list and non-negative to the back — there is no distinction between "z-index:0 acts like a new stacking context for descendants" vs "z-index:auto participates in the parent's context" (CSS distinguishes these; Lynx's `z-index` default/unset is `0` per `lynx/tools/css_generator/css_defines/147-z-index.json`, and `0` is *also* what any Lynx element with a set-but-equal-to-zero z-index gets, so both "no stacking context" and "explicit z-index:0" collapse to the same code path — see the `child->ZIndex() != 0` checks throughout `element_container.cc`).
- Practical effect: z-index in Lynx only lifts an element within the *single nearest* ancestor stacking-context "bucket"; it does not compose correctly across nested positioned ancestors the way CSS does, and non-stacking-context intermediate ancestors' own paint order (e.g. their own overlapping siblings) is not respected for the reparented subtree.

**What lynx-vello should do instead (W3C-correct):** Implement the real CSS positioned-layout & stacking-context algorithm (CSS2.1 Appendix E / CSS Position / CSS3 "Stacking Context" spec): each box that establishes a stacking context (root; positioned with `z-index != auto`; `opacity < 1`; `transform`/`filter`/`backdrop-filter` != none; `will-change` naming a stacking-context-inducing property; `isolation: isolate`; `mix-blend-mode != normal`; `contain: layout|paint|strict|content`; positioned with `will-change`) paints its own descendants recursively in the standard 7-step order, and z-index values are only compared *among siblings within the same stacking context*, never globally flattened. Since `stylo` is being used for CSS in lynx-vello, stacking-context determination and paint-order sorting should reuse stylo's/Servo's existing stacking-context logic rather than port Lynx's reparenting model. This is the single most important documented behavioral divergence to flag for ReactLynx compatibility work: apps that rely on Lynx's actual (buggy-by-spec) z-index behavior may look different once lynx-vello does it correctly; that is intentional per project goals.

**`position: fixed` containing block — confirmed second W3C deviation.** In every mode Lynx supports (legacy, and both newer `enable-fixed-new`/`enable-unify-fixed-behavior` paths), a fixed element's containing block is unconditionally the single page-root element (`ElementManager::root()`, sized to the viewport):
- **Legacy** (both flags off, the shipped default at the raw config level): Starlight measures `fixed` through the same code path as `absolute` (`LayoutObject::IsFixedOrAbsolute()`), but the render tree physically reparents the element under `element_manager_->root()` the moment it becomes fixed (`FiberElement::InsertFixedElement`, `fiber_element.cc:5037-5096` — its own FIXME comment calls this "a temporary compatibility state"), so its containing block ends up being root anyway.
- **FixedNew/Unify** (`enable_fixed_new_`/`enable_unify_fixed_behavior_`, both default `false` in raw config but `FragmentLayerRenderModeOn` forces `SetEnableFixedNew(true)`, `template_assembler.h:543-548`): every `LayoutNode` gets a direct `GetRoot()` pointer at creation (`layout_context.cc:462-466`), fixed nodes are collected into a manager-level `fixed_node_set_` and measured *only* by the root's own `LayoutAlgorithm` (`InitializeFixedNode`, `layout_algorithm.cc:102-130`, comment: "Only called by root's LayoutAlgorithm"), and the native view is attached as a direct child of the root's container (`ElementContainer::InsertElementContainerAccordingToElement`/`AttachChildToTargetContainerRecursive`, `element_container.cc:258-271,321-327`).
- **Scroll exclusion is structural, not per-ancestor math**: because the fixed element's native view is never mounted inside any scrollable ancestor's view hierarchy, scroll offset from *every* intervening scroll-view/list is excluded uniformly, regardless of nesting depth — there's no separate "walk up N scrollable ancestors" calculation to get right or wrong.
- **No component-boundary containing block**: `element_manager()->root()` is set once per page (`PageElement::PageElement`/`AttachToElementManager`), never per-`<component>`, so fixed is always page-root-relative no matter how deep inside nested components it is.
- **No transform/filter/perspective/will-change/contain escape hatch anywhere**: confirmed absent — no `transform` reference exists anywhere in `core/renderer/starlight/layout/`, and Lynx has no CSS `contain` property implemented at all (the only `kContain` symbol found is `background-size: contain`, unrelated). CSS's real algorithm lets such an ancestor become the containing block instead of the viewport; Lynx always escapes all the way to page root regardless.
- Fixed elements *do* correctly become CSS stacking contexts in Lynx (`Element::IsStackingContextNode()` includes `is_fixed_` — `element.cc:1903-1908`), which matches spec — that part isn't a deviation.

**What lynx-vello should do instead (W3C-correct):** implement the real CSS containing-block algorithm for `position: fixed` — viewport-equivalent containing block by default, re-anchored to the nearest ancestor with a qualifying `transform`/`filter`/`perspective`/`will-change`/`contain` when one exists — rather than Lynx's unconditional escape-to-page-root.

Units confirmed as CSS-standard vs Lynx-only (from `lynx/core/renderer/css/css_keywords.cc` token table and `lynx/core/renderer/css/parser/length_handler_unittest.cc`): standard `px`, `%`, `em`, `rem`, `vw`, `vh`, `calc()`, `deg` all parse; Lynx-only extensions are `rpx` (root/responsive px, screen-width-relative) and `ppx` (physical/device px) — both are Lynx units with no CSS equivalent (also `sp` for font-scaling, mirroring Android). `env(safe-area-inset-{top,bottom,left,right})` is supported (`lynx/core/renderer/css/css_style_utils.cc:92-101`), matching the CSS Environment Variables spec subset used on web/iOS. No `vmin`/`vmax` token was found in `css_keywords.cc`.

Default-value quirks worth flagging: `overflow` defaults to `hidden` in Lynx (`css_defines/25-overflow.json`, `"default_value": "hidden"`) vs CSS's standard default `visible` — this is itself a W3C deviation lynx-vello must decide on (the project brief says to prefer W3C behavior for *stacking*; for `overflow` this is a values-default divergence, flagged here for the same reason — matching ReactLynx behavior requires defaulting to `hidden`, which is the opposite of standard CSS, so lynx-vello likely should follow the Lynx default here to match visual/rendering behavior, unlike z-index where the algorithm itself is followed instead). `box-sizing` defaults to `auto`, which Lynx treats as `border-box` (`css_defines/6-box-sizing.json`: `"lynx:border-box w3c:content-box"`) — another explicit, file-documented Lynx-vs-W3C default divergence. `position` defaults to `relative` in Lynx (`css_defines/5-position.json`) vs CSS's `static` default — Lynx has no `static` value at all in its enum (`absolute | relative | fixed | sticky`), so "static/normal in-flow" positioning in Lynx *is* what `position: relative` means (Lynx's `relative` ≈ CSS `static`, since Lynx elements are not CSS-inline/block boxes to begin with).

The project's own `default_layout_style.h` already encodes a "Lynx default vs W3C default" switch for a couple of properties via a `DEFAULT_CSS_FUNC`/`DEFAULT_CSS_VALUE` macro pair driven by a boolean "w3c-aligned" flag (only observed applied to `border-style`/`border-width` defaults in the current source — `SL_DEFAULT_BORDER_STYLE = kSolid` vs `W3C_DEFAULT_BORDER_STYLE = kNone`), confirming Lynx engineers are aware of and already partially plumbing exactly this kind of divergence.

#### Box model, sizing, and general layout-affecting CSS properties

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `display` | Selects child layout algorithm: `none/flex/grid/linear/relative/block/auto` (`auto`≈flex in Lynx, block in W3C-aligned mode) | Core | Partial | Does not carry CSS's external inline/block box type, only internal layout mode; `block` alias only active under a W3C-alignment flag. lynx-vello should treat `display` as choosing layout algorithm only, and separately track box "outer" type per CSS if targeting full compat | `lynx/tools/css_generator/css_defines/24-display.json`, `lynx/core/renderer/starlight/style/default_layout_style.h` |
| `position` | `absolute\|relative\|fixed\|sticky`, default `relative` | Core | Partial | No `static` keyword; Lynx's `relative` is closer to CSS `static` (plain in-flow) since there's no inline/block box notion; `sticky` behaves like CSS position:sticky (scroll-container relative); `fixed` always resolves against the page root with no transform/filter/perspective/contain escape hatch — see the dedicated `position: fixed` paragraph above and `deviations.md` | `lynx/tools/css_generator/css_defines/5-position.json`, `lynx/core/renderer/starlight/layout/layout_object.h:78-92`, `lynx/core/renderer/dom/fiber/fiber_element.cc:5037-5096` |
| `top`/`right`/`bottom`/`left` | Offsets for absolute/fixed/relative(=static-ish)/sticky | Core | Yes | — | `lynx/tools/css_generator/css_defines/1-top.json` (+2,3,4), `lynx/core/renderer/starlight/layout/position_layout_utils.h` |
| `inset-inline-start`/`inset-inline-end` | Logical offsets | Extended | Yes | — | `css_defines/168-inset-inline-start.json`, `169-inset-inline-end.json` |
| `box-sizing` | `border-box\|content-box\|auto`(→border-box) | Core | Partial | `auto` default resolves to `border-box`; CSS default is `content-box`. Match Lynx default for compat | `css_defines/6-box-sizing.json` |
| `width`/`height` | Content or border box size per box-sizing; default `auto` | Core | Yes | — | `css_defines/26-height.json`, `27-width.json`, `nlength.h` |
| `min-width`/`max-width`/`min-height`/`max-height` | Size clamps | Core | Yes | — | `css_defines/28..31` |
| `aspect-ratio` | `<num>/<num>`, used in auto-size resolution | Core | Partial | `auto` keyword not supported (only numeric ratio); CSS `auto` (use intrinsic ratio) unsupported per compat data | `css_defines/95-aspect-ratio.json` |
| `padding`/`padding-{left,right,top,bottom}` | Inner box inset, default 0 | Core | Yes | — | `css_defines/32-35`, `surround_data.h` |
| `padding-inline-start`/`-end` | Logical padding | Extended | Yes | — | `css_defines/152,153` |
| `margin`/`margin-{left,right,top,bottom}` | Outer box offset, default 0, supports auto-centering | Core | Yes | — | `css_defines/37-41` |
| `margin-inline-start`/`-end` | Logical margin | Extended | Yes | — | `css_defines/150,151` |
| `border-width`/`border-{side}-width`, `border-style`, `border-color` (+side/logical variants) | Border box contribution to size when box-sizing=border-box | Core | Yes (values differ by default) | Default style `solid` in Lynx vs `none` in W3C-aligned mode (explicit dual-default in code) | `default_layout_style.h` (`SL_DEFAULT_BORDER_STYLE` vs `W3C_DEFAULT_BORDER_STYLE`), `css_defines/17-21,74,115-118,154-159` |
| `border-radius` (+corner, logical corner variants) | Corner rounding; paint-only, not layout-affecting for box size | Extended | Yes | — | `css_defines/12-16,160-163` |
| `overflow`/`overflow-x`/`overflow-y` | Clipping of children beyond box; default **`hidden`** | Core | Partial | CSS default is `visible`; Lynx defaults to `hidden`. Match Lynx default for behavior compat | `css_defines/25-overflow.json` ("default_value": "hidden"), `120,121` |
| `visibility` | `visible\|hidden\|none\|collapse`; marked `consumption_status: skip` (handled outside layout-only path) but affects paint, and `none` here behaves like display:none (Lynx-specific overload) | Core | Partial | Standard `hidden` keeps its box; standard `collapse` on a flex item removes main-axis participation but preserves a cross-size strut (implemented by neutron-star's two-round Flex pass). Lynx's non-standard `none` value still belongs in the host lowering and behaves like removing layout entirely, colliding with `display:none`'s job — confirm its exact element-layer behavior before wiring the adapter | `css_defines/104-visibility.json` |
| `z-index` | Integer stacking order for stacking-context participants; default `0` | Core | **No** | See dedicated stacking-context section above; also marked `"consumption_status": "skip"` in property metadata (not part of Starlight's generic layout-only styles — handled specially in the element/paint tree) | `css_defines/147-z-index.json`, `lynx/core/renderer/dom/element_container.cc`, `lynx/core/renderer/dom/element.cc:1903-1908` |
| Stacking context triggers | root / non-zero z-index while positioned / `fixed` / has transform / has opacity | Core | **No** | CSS also triggers stacking contexts for filter, mask, clip-path, `isolation:isolate`, `mix-blend-mode≠normal`, `contain: layout\|paint`, `will-change` naming any of the above, flex/grid items with z-index≠auto. Implement full W3C list + recursive per-context painting instead of Lynx's flatten-and-sort | `lynx/core/renderer/dom/element.cc:1903` (`IsStackingContextNode`) |
| `flex`, `flex-grow`, `flex-shrink`, `flex-basis`, `flex-flow` (shorthand) | Standard flex item sizing | Core | Yes | — | `css_defines/49-52,146` |
| `flex-direction`, `flex-wrap` | Flex container main-axis dir + wrapping | Core | Yes | — | `css_defines/53,54` |
| `justify-content`, `align-items`, `align-self`, `align-content` | Standard flex alignment; default `align-items`/`align-content` = `stretch` (matches CSS default for these) | Core | Yes | — | `css_defines/55-58`, `default_layout_style.h` |
| `order` | Flex/grid item paint/layout order override | Core | Yes | — | `css_defines/75-order.json` |
| `gap`/`row-gap`/`column-gap` (+legacy `grid-row-gap`/`grid-column-gap`) | Standard gutter | Core | Yes | — | `css_defines/205-207,181,182` |
| `grid-template-columns`/`-rows`, `grid-auto-columns`/`-rows`, `grid-auto-flow` | Standard CSS Grid track definition + auto-placement axis/dense mode | Core | Yes | — | `css_defines/171-174,185`, `lynx/core/renderer/starlight/layout/grid_layout_algorithm.h` (mirrors spec: "intrinsic track sizes", dense/sparse placement cursor) |
| `grid-column-start/end`, `grid-row-start/end`, `grid-column`/`grid-row` (shorthands), `grid-column-span`/`grid-row-span` (legacy Lynx spanning props) | Item placement in grid | Core (start/end) / Rare (span aliases) | Partial | `grid-column-span`/`grid-row-span` are pre-CSS-Grid Lynx-only span properties, superseded by standard `span N` syntax in `grid-column-end` etc.; keep both for back-compat but treat span-props as Lynx legacy | `css_defines/175,176,177-180,227,228` |
| `justify-items`, `justify-self` | Grid alignment; default `justify-items:stretch`, `justify-self:auto` | Core | Yes | — | `css_defines/183,184`, `default_layout_style.h` |
| `staggered-grid` behaviors (via `staggered_grid_layout_algorithm.cc`) | Pinterest-style masonry layout for `<list>` component | Rare | No (non-CSS) | Not a standard CSS layout mode at all (no `display: masonry` equivalent shipped cross-browser); implement as a component-level custom layout, not general CSS | `lynx/core/renderer/starlight/layout/staggered_grid_layout_algorithm.h` |
| `list-main-axis-gap`, `list-cross-axis-gap` | Gap sizing specific to the `<list>` component (not general flow), `consumption_status: skip` | Rare | No (non-CSS) | Component-specific, not part of generic box layout | `css_defines/187,188` |

#### Lynx-specific non-CSS layout primitives: `linear-*` (display:linear)

`display:linear` ports Android's `LinearLayout` model. Main axis is chosen by `linear-orientation`/`linear-direction` (two overlapping properties — `linear-orientation` is the legacy/deprecated one per its own compat data `"deprecated": true`; `linear-direction` id 189 is the current one, both accepting `vertical|horizontal|vertical-reverse|horizontal-reverse` and the newer CSS-flex-like aliases `row|row-reverse|column|column-reverse`). Children are laid out sequentially along that axis; cross-axis placement per-child uses `linear-layout-gravity` (child-side, ~`align-self`); container-side distribution along main axis uses `linear-gravity` (~`justify-content` + more absolute anchor keywords `top/bottom/left/right`); container-side cross-axis alignment for all children uses `linear-cross-gravity` (~`align-items`, but with only `start/end/center/stretch/none`, no space-distribution values). Extra space distribution along the main axis, beyond gravity, uses an Android-style weight system: `linear-weight` (per-child, like `flex-grow` but simpler — no shrink/basis split) divided over `linear-weight-sum` (explicit container-declared total, unlike flex's implicit sum-of-flex-grow).

**Implementation status:** `crates/neutron-star` implements this formatting
context as the generic `compute_linear_layout` peer algorithm plus
`LinearSource`, `LinearContainerStyle`, and `LinearItemStyle` protocols,
alongside its Flex, Grid, and Relative protocols and algorithms. Linear uses
the same layout IO, cache/session recursion, private box-model machinery, leaf
dispatch, absolute-position helper, and hidden-subtree cleanup; it does not
translate linear into Flex. The concrete Widget/stylo adapter, dirty/cache
invalidation wiring, root fixed-position pass, Relative and Linear
computed-style translation, and text-style translation/session wiring remain
future L3 work. The feature-gated Parley measurement core itself now lives in
`neutron-star`; no separate integration crate has been established.

Two Starlight-specific sizing rules are deliberately pinned rather than
inherited from Flexbox/web-core. First, Linear weights and default cross-axis
stretch use the incoming constraint's decided geometry even when a Flex parent
marks that target indefinite for descendant percentages under Flexbox §9.8;
`definite_dimensions` remains percentage-basis metadata only. Second, after an
intrinsic Linear container obtains its inline size, percentage margin,
padding, and border used edges are re-resolved without remeasuring the child
or recomputing the already-established main total. Starlight also rewrites its
internal min/max BoxInfo at this point, but item sizing is already complete and
no later phase reads those values, so neutron-star eliminates that dead update.
The percentage basis is the provisional intrinsic content size before the
container's own min/max clamp. This follows
`LinearLayoutAlgorithm::DetermineContainerSize`/`UpdateContainerSize` and
`BoxInfo::UpdateBoxData`; it intentionally differs from web-core's browser
Flex polyfill, which can reflow a percentage-sized child.

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `display: linear` | Enables linear (Android LinearLayout-style) child layout | Core | No (non-CSS) | Implemented in `crates/neutron-star` as a first-class sequential-layout algorithm alongside Flex and Grid, not as a Flex translation | `css_defines/24-display.json`, `lynx/core/renderer/starlight/layout/linear_layout_algorithm.{h,cc}` |
| `linear-orientation` (legacy, deprecated) | Main axis + direction: `horizontal\|vertical\|horizontal-reverse\|vertical-reverse\|row\|column\|row-reverse\|column-reverse`; default `vertical` | Core | No (non-CSS) | No CSS equivalent property name; conceptually ≈ `flex-direction` restricted to a linear container. Deprecated in favor of `linear-direction` | `css_defines/78-linear-orientation.json` |
| `linear-direction` | Same value space as above, current/non-deprecated property; default `column` | Core | No (non-CSS) | Same as above | `css_defines/189-linear-direction.json` |
| `linear-gravity` | Container: main-axis space distribution: `none\|start\|end\|center\|space-between\|top\|bottom\|left\|right\|center-vertical\|center-horizontal`; default `none` | Core | No (non-CSS) | ≈ `justify-content` but with extra directional/absolute keywords (`top/bottom/left/right` independent of orientation) with no direct CSS analog | `css_defines/81-linear-gravity.json` |
| `linear-layout-gravity` | Child: cross-axis self-alignment: `none\|start\|end\|center\|stretch\|top\|bottom\|left\|right\|center-vertical\|center-horizontal\|fill-vertical\|fill-horizontal`; default `none` | Core | No (non-CSS) | ≈ `align-self`, superset of values | `css_defines/82-linear-layout-gravity.json` |
| `linear-cross-gravity` | Container: cross-axis alignment for all children: `none\|start\|end\|center\|stretch`; default `none` | Core | No (non-CSS) | ≈ `align-items` restricted vocabulary | `css_defines/149-linear-cross-gravity.json` |
| `linear-weight` | Per-child proportional extra-space share along main axis; default `0` (no growth) | Core | No (non-CSS) | ≈ `flex-grow` but simpler (no shrink/basis interplay documented at this property level) | `css_defines/80-linear-weight.json` |
| `linear-weight-sum` | Container-declared explicit denominator for weight distribution; default `0` (auto-sum); web_lynx notes it's *not* supported on inline style | Core | No (non-CSS) | No CSS analog — flexbox infers total weight from children's `flex-grow` sum; Lynx allows explicit override enabling "reserved empty space" patterns | `css_defines/79-linear-weight-sum.json` |
| web-core's polyfill strategy for linear | `display:linear` → CSS `display:flex` + custom-property-driven orientation/gravity mapping via custom elements | — | — | Historical browser strategy only; lynx-vello's landed implementation follows Starlight with a dedicated algorithm, avoiding impedance mismatches such as explicit `linear-weight-sum` | `lynx-stack/packages/web-platform/web-core/src/style_transformer/rules.rs` |

#### Lynx-specific non-CSS layout primitives: `relative-*` (display:relative)

Normative algorithm: [Starlight Relative Layout Module Level 1](../starlight-relative-layout.md).

`display:relative` ports Android's `RelativeLayout`. Each child is optionally tagged with a small integer `relative-id` (scope: unique among siblings) so other siblings can anchor to it by id; `relative-{top,right,bottom,left}-of` / `relative-inline-{start,end}-of` reference another sibling's `relative-id` (or the special parent id) to position this child's respective edge adjacent to that sibling; `relative-align-{top,right,bottom,left}` / `relative-align-inline-{start,end}` align this child's edge flush with another element's *same-side* edge (rather than adjacent-placement); `relative-center` centers the child within the parent on one or both axes; `relative-layout-once` is a perf/correctness toggle (when true, it uses one combined dependency order and measures each item as encountered; false uses separate horizontal/vertical orders plus selective remeasurement). Native Lynx computes the property default as `true`; neutron-star's standalone trait surface deliberately defaults it to `false`, and the future Lynx adapter must materialize native's `true`. This is fundamentally a same-generation dependency-graph solver, not a CSS box-flow model — nothing in standard CSS does sibling-referential anchoring by id within a single containing block (closest analogs are grid named lines/areas, which are still index/name-based grid slots not per-element anchors), and CSS Anchor Positioning (`anchor()`/`position-anchor`) is document-wide/absolute-positioning-only, not a same-parent relative-layout mode.

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `display: relative` | Enables relative (Android RelativeLayout-style) sibling-anchored child layout | Extended | No (non-CSS) | Implemented as `neutron_star::compute::compute_relative_layout`; **not** the same as CSS `position:relative` (name collision only) | `css_defines/24-display.json`, `lynx/core/renderer/starlight/layout/relative_layout_algorithm.{h,cc}` |
| `relative-id` | Assigns an integer id to a child so siblings can reference it; default `-1` (none) | Extended | No (non-CSS) | No CSS equivalent (closest: grid-line names, but those are container-scoped slots not per-element anchors) | `css_defines/131-relative-id.json`, `default_layout_style.h` (`SL_DEFAULT_RELATIVE_ID=-1`) |
| `relative-top-of`/`relative-right-of`/`relative-bottom-of`/`relative-left-of` | Place this element's given edge adjacent-outside the referenced sibling's opposite edge (id-based) | Extended | No (non-CSS) | No CSS equivalent | `css_defines/136-139` |
| `relative-inline-start-of`/`relative-inline-end-of` | Logical-direction variants of the above | Extended | No (non-CSS) | Host adapter lowers to physical left/right references using computed writing direction before layout | `css_defines/166,167` |
| `relative-align-top`/`-right`/`-bottom`/`-left` | Align this element's edge flush with referenced sibling's same-side edge (id-based) | Extended | No (non-CSS) | No CSS equivalent | `css_defines/132-135` |
| `relative-align-inline-start`/`-end` | Logical variants | Extended | No (non-CSS) | Host adapter lowers to physical left/right references using computed writing direction before layout | `css_defines/164,165` |
| `relative-center` | Center child within parent: `none\|vertical\|horizontal\|both`; default `none` | Extended | No (non-CSS) | ≈ combination of `align-self:center`+`justify-self:center`, but scoped to `relative` mode only | `css_defines/141-relative-center.json` |
| `relative-layout-once` | Selects one combined dependency/measurement pass (`true`) or separate-axis two-pass solving (`false`); native computed default `true` | Rare | No (non-CSS) | Implemented in neutron-star; its reusable trait default is intentionally `false`, so the Lynx adapter must pass native's computed `true` explicitly | `css_defines/140-relative-layout-once.json`, `relative_layout_algorithm.h` (`InlineDependencies`, `Sort()`) |
| Web-lynx (web-core) support | All `relative-*` properties show `"web_lynx": {"version_added": false}` in compat data | — | — | Confirms `web-core` (the prior reference implementation) never implemented `display:relative` at all — lynx-vello has no working prior-art reference for this mode and must implement solely from Starlight's C++ source | `css_defines/131,140,141-relative-*.json` (each `"web_lynx":{"version_added": false}`) |

The standalone Relative Level 1 contract also makes three explicit repairs
relative to current native C++: id `0` never identifies an item (native can
accidentally add a graph dependency before later treating it as parent),
contradictory double anchors clamp the end to the start, and two-pass solving
performs the specified selective remeasurement/final-size feedback rounds.
These are user-confirmed module semantics, not accidental attempts to extend
the raw value grammar.

Executable coverage lives in neutron-star's engine-native Relative behavior
suite and benchmark target. Tests assert geometry, dependency-order fallback,
measurement traces, visibility, static positions, and cache behavior through
the public host protocol; source inventories and external-runner terminology
are not part of the contract.

#### Units and value types (layout-relevant)

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `px` | Absolute CSS pixel | Core | Yes | — | `lynx/core/renderer/css/css_keywords.cc` (TokenType::PX) |
| `%` | Percentage of containing-block dimension | Core | Yes | — | `nlength.h` (`kNLengthPercentage`), `length_handler_unittest.cc` |
| `em`/`rem` | Font-relative units | Core | Yes | — | `css_keywords.cc` (EM/REM tokens) |
| `vw`/`vh` | Viewport-relative units | Core | Yes | No `vmin`/`vmax` tokens found in `css_keywords.cc` — likely unimplemented; verify before relying on them | `css_keywords.cc` (VW/VH tokens), `length_handler_unittest.cc` |
| `calc()` | Standard CSS calc expressions (mixed units) | Core | Yes | — | `length_handler_unittest.cc` (`"calc(2px + 3rpx)"` test), `nlength.h` (`kNLengthCalc`) |
| `rpx` | "Responsive/root px" — Lynx-defined screen-width-scaled unit | Extended | No (non-CSS) | Lynx-only; needs explicit conversion using the bundle's declared screen-width basis (same one used at bundle-encode time — already covered by the binary-template docs) | `css_keywords.cc` (RPX token), `css_type.h` (`kRpx`), `css_decoder.cc` |
| `ppx` | Physical/device px (density-scaled) | Extended | No (non-CSS) | Lynx-only; needs device-pixel-ratio conversion | `css_keywords.cc` (PPX token), `css_type.h` (`kPx`/`kPpx`-adjacent enum), `css_decoder.cc` |
| `sp` | Font-scale-relative unit (Android-style "scale-independent pixel") | Rare | No (non-CSS) | Lynx-only, font-size-accessibility-scale unit | `css_keywords.cc` (SP token) |
| `env(safe-area-inset-{top,right,bottom,left})` | Safe-area insets | Extended | Yes | Matches CSS Environment Variables spec's safe-area-inset-* subset | `lynx/core/renderer/css/css_style_utils.cc:92-101` |
| `fr` (grid track sizing) | Standard CSS Grid flexible track unit | Core | Yes | — | `nlength.h` (`kNLengthFr`) |
| `fit-content`/intrinsic (`max-content`) sizing keywords | Standard CSS intrinsic sizing keywords | Extended | Yes | — | `nlength.h` (`kNLengthFitContent`, `kNLengthMaxContent`) |

#### Notes on what was and wasn't directly confirmed by source

- Confirmed by reading actual generated-property JSON + Starlight headers/.cc for: all properties tabulated above, their default values, and their `consumption_status`.
- Confirmed the z-index/stacking-context deviation mechanism precisely by reading `element_container.cc` (`ZIndexChanged`, `UpdateZIndexList`, `MoveZChildrenRecursively`) and `element.cc`'s `IsStackingContextNode`.
- Did **not** find a `vmin`/`vmax` token in `css_keywords.cc`; flagging as "likely unsupported" rather than asserting unsupported, since the grep was not exhaustive over every parser file.
- Did not deeply trace `staggered_grid_layout_algorithm.cc` internals (masonry/list layout) — flagged only at a high level as a Rare/component-specific primitive; a dedicated `<list>`-component section of the tracking doc should cover it in depth.
- `writing-mode`/RTL (`direction` property, `logic_direction_utils.cc`) exists in the codebase but is out of scope for this section (covered by a text/i18n or box-model-direction section instead) — flagging its existence since it interacts with logical properties (`margin-inline-start` etc.) tabulated above.
- Did not open `lynx-stack/packages/react` (ReactLynx framework) in this pass — this section is Starlight/CSS-layer only, per the assigned scope.

---

## Also see

Scope note: this is the behavior spec for the *layout algorithm*, which the
from-scratch layout engine (successor to the C++ engine's `starlight`)
implements — see `.claude/agents/lynx-layout-engine.md`. The engine crate is
[`crates/neutron-star`](../../crates/neutron-star): its protocol, shared
machinery, and Flexbox, Grid, Starlight Relative, and Linear algorithms are
implemented alongside its feature-gated Parley measurement core. Its concrete
L3 Widget/stylo runtime adapter, including text-style translation and
text-session wiring, remains pending; no separate integration crate has been
established. The design, ownership boundaries, and milestones are in
[`docs/layout-architecture.md`](../layout-architecture.md).

Implementation-pattern reference (not a behavior spec): `Paws/engine/src/layout/stacking.rs` for a real, WPT-conformance-tested CSS stacking-context implementation over `stylo` computed style — the concrete reference for the z-index deviation.
