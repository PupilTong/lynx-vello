# Layout architecture — `neutron-star`

`crates/neutron-star` is lynx-vello's box-layout engine: the from-scratch
successor to the Lynx C++ engine's `starlight`
(`lynx/core/renderer/starlight/`). It implements CSS **flexbox** and carries
the host protocol for the planned CSS **Grid** algorithm. It is deliberately
Lynx-agnostic and standalone-publishable — zero required dependencies, no
assumption about DOM, style engine, or storage — and every host boundary is
**static dispatch**: `dyn` is impossible by construction, not by convention.

Status: **L1 (flexbox)** — the protocol, generic machinery, cache, leaf and
positioned sizing, rounding, and CSS Flexbox Level 1 algorithm are
implemented and conformance-tested against a plain-storage mock host. The
Grid contracts exist, but its L2 algorithm is still pending. The crate
rustdoc is the protocol reference; this document is the rationale, the
performance architecture, and the remaining plan.

Behavior spec: [`docs/tracking/css-layout.md`](tracking/css-layout.md)
(what Starlight does, which parts are real W3C features vs Lynx extensions,
and the confirmed deviations). Per the standards policy in
[`AGENTS.md`](../AGENTS.md), flex is implemented—and grid is planned—from the
**W3C specs** (Flexbox Level 1, Grid Level 2, Sizing Level 3, Box Alignment
Level 3), not by porting Starlight's C++.

## Ownership

```text
              lynx-vello host stack                        standalone
┌──────────────┐   ┌──────────────────────────┐   ┌──────────────────────────┐
│ lynx-widget  │──▶│ lynx-layout (planned)    │──▶│ neutron-star             │
│ + stylo-dom  │   │ host adapter:            │   │ tree/style protocol      │
│ styles, tree │   │ · impls the tree traits  │   │ flexbox algorithm        │
└──────────────┘   │ · stylo → style views    │   │ grid protocol + L2 plan  │
     ▲             │ · display dispatch       │   │ leaf/hidden/cache/round  │
┌──────────────┐   │ · linear/relative algos  │   │ machinery                │
│ lynx-text    │◀──│ · text measure closures  │   │ (no Lynx vocabulary,    │
│ (planned,    │   │ · fixed/sticky lowering  │   │  no storage, no dyn)    │
│  parley)     │   └──────────────────────────┘   └──────────────────────────┘
└──────────────┘
```

| Layer | Owns | Must not own |
| --- | --- | --- |
| `neutron-star` | Implemented flex algorithm; planned grid algorithm; leaf boxing, hidden-subtree cleanup, rounding; protocol vocabulary (geometry, style values, layout IO); cache semantics | Node storage, style storage, display dispatch, Lynx vocabulary (`linear-*`, `rpx`, …), text shaping, stacking/paint order |
| `lynx-layout` *(planned host adapter)* | Trait impls over the widget tree, stylo `ComputedValues` → style views, display-mode dispatch, `linear`/`relative`/`staggered` algorithms, dirty tracking + cache invalidation, fixed/sticky lowering | A second flex/grid implementation, engine-side copies of styles |
| `lynx-text` *(planned)* | Text measurement closures handed to `compute_leaf_layout` | Box layout |

The engine/host seam is exactly the seam that makes the crate publishable:
everything Lynx-specific lives in the adapter, and the adapter's job is
mechanical (accessor translation + one `match` on display mode).

## The protocol in one page

Traits (`neutron_star::tree`), layered by capability so each entry point
demands only what it uses:

| Trait | Adds | Consumed by |
| --- | --- | --- |
| `TraverseTree` | child iteration (GAT iterator) | everything |
| `LayoutTree: TraverseTree` | `CoreStyle` views, `resolve_calc`, layout/static-position storage, hidden-cache invalidation hook, **`compute_child_layout` (the host dispatch point)** | all algorithms |
| `FlexTree: LayoutTree` | flex container/item style views | the L1 flexbox algorithm |
| `GridTree: LayoutTree` | grid container/item style views (GAT track-list iterators) | the L2 grid algorithm |
| `CacheTree` | per-node measurement-cache slots | `compute_cached_layout` |
| `RoundTree: TraverseTree` | unrounded → final layout storage | `round_layout` |

Entry points (`neutron_star::compute`) are free generic functions — there is
no engine object, so unused entry points never monomorphize into the host.
Implemented machinery: `compute_root_layout`, `compute_leaf_layout`
(host measure closure), explicit hidden-subtree cleanup via
`hide_subtree`, `compute_cached_layout`
(keyed on the **complete `LayoutInput`** — see the caching section),
`compute_absolute_layout` (the positioned pass for out-of-flow nodes whose
containing block is not their formatting parent), and
`round_layout(tree, root, scale)` (device-pixel snapping), plus the L1
`compute_flexbox_layout`. The live flex entry point uses the fixed shape that
the Grid entry point will retain in L2:

```rust
pub fn compute_flexbox_layout<Tree: FlexTree>(  // implemented
    tree: &mut Tree, node: NodeId, input: LayoutInput) -> LayoutOutput;
pub fn compute_grid_layout<Tree: GridTree>(     // planned L2 API
    tree: &mut Tree, node: NodeId, input: LayoutInput) -> LayoutOutput;
```

The flex signature is public now; the Grid signature documents the planned
L2 API so hosts can reserve the corresponding dispatch arm.

Layout IO is three `Copy` PODs: `LayoutInput` (layout goal, sizing mode,
known dimensions / parent size / available space) → `LayoutOutput` (size,
content size, baselines) per call, and `Layout` (order, location, size,
content size, scrollbar size, border/padding/margin) as the durable
per-node result. `LayoutGoal::Measure(RequestedAxis)` makes measurement and
its requested axes one value; `LayoutGoal::Commit` requests durable child
geometry. Hidden-subtree cleanup is deliberately outside this sizing API.
`LayoutInput`/`LayoutOutput`/`Layout` are
`#[non_exhaustive]` so the protocol can grow additively (block-layout margin
collapsing is the known future widener).

**Recursion round-trips through the host.** An algorithm never walks the
tree itself; it calls `tree.compute_child_layout(child, input)`. The host
first handles `BoxGenerationMode::None` by calling `hide_subtree` and
returning `LayoutOutput::HIDDEN`; this explicit cleanup precedes and bypasses
the cache. For a generated box, the host routes to a neutron-star algorithm,
leaf measurement, or a host-private algorithm, wrapping that routing in
`compute_cached_layout`. This decision buys three properties at once:

1. **Open algorithm set.** Lynx's non-CSS `display: linear` (Android
   `LinearLayout` semantics: `linear-weight`/`linear-gravity`/…) and
   `display: relative` (id-anchored sibling constraint solving) become peer
   algorithms in the host adapter, implemented against the same traits, with
   the engine none the wiser. Same for the `<list>` component's
   staggered-grid. The engine has **no `Display` enum** — dispatch identity
   belongs to the host.
2. **Uniform caching.** Every generated-box path through dispatch shares one
   cache policy, so mixed trees of engine and host-private layout modes
   memoize correctly; Grid will use the same path when its L2 algorithm
   lands. Hidden cleanup deliberately stays outside that cache boundary.
3. **Partial relayout.** Any node can be a layout root; the engine never
   assumes global tree access.

## Design decisions and their rationale

**No `dyn`, enforced structurally.** Every trait carries GATs (borrowed
child iterators, borrowed style views, borrowed grid track iterators), which
makes them non-object-safe: `dyn LayoutTree` is a compile error — there is a
`compile_fail` doctest pinning this. What the constraint buys: every
host⇄engine call site monomorphizes, inlines, and const-folds (style
accessors returning constants collapse into the algorithm); no vtable
indirection in the hottest recursion of the frame. The accepted costs:
compile time and per-host codegen (one copy of the algorithms per host tree
type — in practice one host per binary), and no heterogeneous "list of
engines" (not a goal).

**The host owns all storage.** Node data, styles, per-node caches, and both
layout copies live host-side, addressed by opaque `NodeId(u64)`. The engine
allocates only transient algorithm scratch. This is what "standalone" means
operationally: the engine imposes zero data-model decisions, and lynx-vello
keeps a single source of truth (the widget tree) instead of mirroring into
an engine tree and diffing.

**Style is read through views, in engine vocabulary.** Style traits
(`CoreStyle` + container/item traits per algorithm) hand out small `Copy`
values (`Dimension`, `LengthPercentage`, alignment enums, grid track types)
per accessor call — lazy translation from stylo's `ComputedValues`, no
materialized engine-side style structs. Grid track *lists* stay borrowed GAT
iterators with `repeat()` as a nested `GridTemplateRepetition` value, so
even the sequence-valued styles cross the boundary without allocation.
Trait-method defaults are **CSS initial values**; Lynx's divergent defaults
are computed-value policy and stay in the host's style system:

| Property | CSS initial (engine default) | Lynx computed default (host supplies) |
| --- | --- | --- |
| `box-sizing` | `content-box` | `border-box` (`auto` → border-box) |
| `overflow` | `visible` | `hidden` |
| `position` | `relative` ≙ static | `relative` (same thing — Lynx has no `static`) |
| `align-items`/`align-content` | `normal` (`None`) | `stretch` (host passes it explicitly) |

**`calc()` without a CSS dependency.** Percent-bearing `calc()` can only be
resolved during layout, and its AST lives in stylo. The protocol carries an
opaque `CalcHandle(u64)`; algorithms resolve through
`LayoutTree::resolve_calc(handle, basis)`. Zero parser dependency, full
`calc()` support.

**Out-of-flow: the layout tree is the formatting structure; the containing
block is data, not topology.** Out-of-flow nodes are **never reparented** —
they stay children of their formatting parent, because CSS derives their
*static position* from that parent's formatting context (Flexbox §4.1: as
if the sole flex item under the container's alignment; Grid §10.1: the
content-edge area), and reparenting would destroy exactly that context.
What varies is where the containing block is, encoded per node by the host:

- `Position::Absolute` — CB **is** the layout parent. The parent's
  algorithm sizes/places the node fully (insets/percentages against its
  padding box; auto insets fall back to the static position it just
  computed). This is the only case Lynx `position: absolute` produces:
  every Lynx element is positioned, so the nearest positioned ancestor is
  always the parent.
- `Position::AbsoluteHoisted` — CB is **not** the parent (CSS `fixed`; or
  `absolute` escaping non-positioned ancestors in non-Lynx hosts). The
  parent's algorithm computes the node's flex/grid-aware static position
  and records it via `LayoutTree::set_static_position`, but does not size
  or place it. After in-flow layout the host runs the **positioned pass**:
  it resolves the CB node (for Lynx `fixed`: the viewport root, or the
  nearest transformed/filtered/`will-change` ancestor per the W3C rule the
  tracking doc mandates), converts the recorded static position into CB
  padding-box space (all unrounded layouts exist by then), and calls
  `compute_absolute_layout(tree, node, cb_padding_box_size,
  static_position)`. That entry sizes the node per the CSS abs-pos rules,
  lays out its subtree normally, and *returns* the node's own layout in CB
  space; the host converts it into formatting-parent space and stores it,
  keeping `Layout::location`'s parent-relative contract intact for rounding
  and painting.

`position: sticky` remains a host post-pass (scroll-time offset clamping),
as in production engines.

**Physical axes + `Direction`, no writing modes.** The vendored stylo fork's
`lynx` feature disables `writing-mode` entirely, so the engine is
physical-axis (`x`/`width`, `y`/`height`) with `direction: rtl` (and Lynx's
`lynx-rtl`, lowered by the host) handled inside algorithms by flipping the
main/inline axis — the same simplification Starlight and Yoga make.
Logical properties (`inset-inline-*`, `margin-inline-*`) are resolved to
physical edges by the style system before layout.

**Two layout copies, one rounding pass — on the device-pixel grid.**
Algorithms produce **unrounded** `f32` layouts (`set_unrounded_layout`);
`round_layout(tree, root, scale)` derives snapped finals through
`RoundTree`. `scale` is the device-pixel ratio (physical px per CSS px):
coordinates are CSS pixels but crisp edges are physical, so snapping is
`snap(v) = round(v × scale) / scale` — on a DPR-2 screen `0.5` CSS px is
already an exact physical edge and must survive. The cumulative-error-free
contract still holds (snap accumulated positions, derive sizes as
`snap(pos+size) − snap(pos)` so adjacent edges share a physical pixel).
Relayout always restarts from unrounded values — re-rounding rounded values
is how engines drift.

**`order` is protocol; `z-index` is not.** Flex/grid items expose `order`
(Lynx supports it) and `Layout.order` records the resulting sibling
traversal/paint index. Stacking contexts and `z-index` are per the tracking
doc a **render-layer** concern implemented W3C-correctly over stylo (see
`Paws/engine/src/layout/stacking.rs` as the pattern reference) — box layout
neither knows nor cares.

## Performance architecture

Target: modern multi-core CPUs with wide SIMD, deep caches, and GPUs doing
the painting — layout's job is to never be the frame's bottleneck.

- **Static dispatch end-to-end** (above). The protocol's hot calls
  (`child_ids`, style accessors, `compute_child_layout`) are all
  monomorphized; hosts should `#[inline]` their impls of the first two.
- **POD boundary.** Geometry and IO types are small `Copy` structs
  (`#[repr(C)]`), passed by value in registers; `f32` throughout (GPU/SIMD
  native, halves cache traffic vs `f64`; Starlight/Yoga/Taffy all agree).
  No `NaN` sentinel games — unknowns are `Option<f32>`/enum variants, and
  boundary values must be finite (debug-asserted).
- **The measurement cache is the asymptotic mechanism.** Flex sizing—and
  Grid once L2 lands—probes children under multiple constraints; uncached,
  nested containers go super-linear (the classic exponential blowup). The
  protocol bakes the fix in: `compute_cached_layout` around every
  generated-box dispatch, per-node slots
  (`cache::Cache`, embeddable, fixed-size, allocation-free —
  `MEASURE_CACHE_SLOTS = 8` measurement slots + 1 layout slot). Shape-aware
  replacement is implemented; probe-trace validation and tuning remain L4
  work. The key is the **complete
  `LayoutInput`** — `goal` distinguishes side-effect-free measurements
  (including their requested axes) from geometry commits, `sizing_mode`
  controls whether content-size probes ignore the node's own
  size/min/max/aspect-ratio, and `parent_size` is the percentage basis. All
  change results, so dropping any of them from the key would alias distinct
  layouts; matching may coalesce entries only under provable equivalences
  (documented in the `cache` module).
- **Incremental relayout is a host workflow the protocol supports, not a
  hidden engine mode.** On style/content/children change the host clears
  that node's cache and its ancestors' (dirty-path invalidation), then
  re-runs `compute_root_layout`: clean subtrees answer from their cache slot
  at the recursion boundary without being walked. Hosts can additionally
  choose a nearer relayout root when the dirty node's size can't escape
  (fixed-size subtree) — the engine is agnostic because any node can be a
  root.
- **Allocation strategy (current).** Algorithms use transient `Vec` scratch;
  nothing engine-side persists between calls. Benchmark-gated upgrades
  include stack-first storage or a bump arena threaded through *internal*
  recursion — deliberately **not** through the public protocol, where scratch
  lifetimes would infect every host trait signature. Either can be adopted
  without a protocol break.
- **Data-oriented inner loops (planned).** The current flex implementation
  uses array-of-structs scratch. Profiles may justify structure-of-arrays
  storage and explicit SIMD for its flexible-length loops later; these are
  engine-internal changes, invisible to the protocol.
- **Parallelism: designed, deferred, additive.** `compute_child_layout`'s
  `&mut self` recursion is inherently sequential — correct for v0 (layout is
  rarely the bottleneck vs paint/style, and Yoga/Taffy/Starlight are all
  sequential). The planned extension is a **batched child-layout hook**: a
  defaulted `LayoutTree` method like
  `compute_child_layouts(&mut self, requests: &mut [ChildLayoutRequest])`
  that algorithms call at fan-out points (independent flex-item measure
  probes, grid item contributions); the default body is today's sequential
  loop, and a parallel host overrides it to shard sub-trees across its own
  pool (host storage, host threading policy — the engine stays
  thread-unaware). Adding a defaulted method is semver-minor, so this ships
  when profiles earn it, without a protocol break.
- **Benchmarks planned for performance/parity hardening.** `divan`
  (CodSpeed-compatible, per repo toolchain) micro/macro benches: deep flex
  nesting, wide children, wrap-heavy, grid auto-placement, incremental
  single-node dirty. Baselines: Taffy and Yoga on equivalent trees — not to
  copy their design, but to keep "high-performance" falsifiable.

## Algorithms (L1 implemented, L2 planned)

This pass structure documents the implemented L1 flex algorithm and the
planned L2 grid algorithm. Starlight's C++ mirrors the same spec steps
(`flex_layout_algorithm.h` literally cites "Algorithm-3"…"Algorithm-15";
`grid_layout_algorithm.h` uses the spec's track-sizing terms verbatim), so
following the spec text directly also keeps us structurally comparable to
the engine we're succeeding.

**Flexbox (L1)** — CSS Flexbox Level 1 §9, as passes:

1. **Setup** — collect in-flow children (skip `box_generation_mode() ==
   None`), stable-sort by style `order`, resolve container axes from
   `flex_direction` × `direction` (rtl flips row axes).
2. **Available space & flex base sizes** (§9.2) — flex base + hypothetical
   main size per item; child measurement via `compute_child_layout` probes
   with `SizingMode::ContentSize`.
3. **Line breaking** (§9.3) — single line for `NoWrap`, else greedy
   line-fill against main available size including `gap`.
4. **Resolving flexible lengths** (§9.7) — the freeze/unfreeze grow/shrink
   loop per line → target main sizes.
5. **Cross sizing** (§9.4) — hypothetical cross sizes (probes with known
   main size), line cross sizes, `align-content: stretch`.
6. **Main-axis alignment** (§9.5) — auto margins, then `justify-content`
   with `gap`.
7. **Cross-axis alignment** (§9.6) — auto margins, `align-self` (baseline
   alignment via child `first_baselines`), `align-content`.
8. **Out-of-flow children** (§4.1) — compute each one's static position per
   §9.8 (sole-item alignment). `Position::Absolute` children are then
   sized/placed against the container's padding box from their insets (auto
   insets anchor to the static position); `Position::AbsoluteHoisted`
   children only get the static position recorded via
   `set_static_position` — the host's positioned pass finishes them.
9. **Finalize** — per-child `set_unrounded_layout` (only for
   `LayoutGoal::Commit`), container border-box size, `content_size`
   accumulation, container baseline.

The automatic minimum size (§4.5, `min-size: auto`) resolves inside steps
2/4, honoring `Overflow::is_scroll_container`.

**Grid (L2)** — CSS Grid Level 2 (minus subgrid), as a pipeline:

1. **Explicit grid resolution** (§7.2–7.5) — expand `GridTemplateComponent`
   lists into concrete track vectors in algorithm scratch; solve
   `repeat(auto-fill/auto-fit)` counts against the definite axis size.
2. **Placement** (§8) — resolve `Line<GridPlacement>` per item
   (start/end/span conflict rules §8.3.1), auto-placement in
   `grid_auto_flow` order with the sparse/dense cursor (§8.5), implicit
   tracks from `grid-auto-rows`/`-columns` (cycled, §7.6), `auto-fit`
   empty-track collapse.
3. **Track sizing** (§11.5, run per axis — columns then rows, §12) — the
   intrinsic track-sizing algorithm: initialize base/growth-limit, size to
   item contributions in span order (measurement probes through
   `compute_child_layout`), maximize tracks, expand `fr` (§12.7), stretch
   `auto` tracks under `*-content: stretch`.
4. **Alignment** (CSS Align) — `align/justify-content` position tracks with
   `gap`; `align/justify-self` place items in their areas; `Rtl` flips the
   inline axis.
5. **Item layout & finalize** — final child layout at known area sizes,
   abs-pos children against the padding box, `set_unrounded_layout`,
   container size, `content_size`.

Grid-item baseline alignment and grid-area-relative abs-pos placement are
trailing L2 refinements; masonry/`staggered-grid` stays out of scope (a
Lynx `<list>`-component concern, not a grid mode).

## Testing strategy

- **L0/L1 (landed):** `tests/protocol.rs` — a complete mock host implementing
  every trait over plain `Vec` storage; proves the protocol is implementable
  without `dyn` (plus the `compile_fail` doctest making non-object-safety a
  tested guarantee), exercises the GAT track-list machinery and all shared
  machinery entry points. `tests/flexbox.rs` supplies a styling-engine-free
  host and spec-derived fixtures for axes, flexing, wrapping, intrinsic
  contributions, alignment, baselines, box sizing, percentages, auto
  minimums, out-of-flow static positions, and measurement-only runs.
- **Parity/performance hardening (planned):** golden layout trees compared
  against browser-computed geometry for the same CSS, differential fuzzing
  against Taffy on the shared feature subset, and the flex benchmarks above.
- **Lynx modes (host-side):** `linear`/`relative` conformance fixtures come
  from Starlight behavior per `docs/tracking/css-layout.md`, tested in the
  adapter crate, not here.

## Milestones

- **L0 — contracts + skeleton** *(complete)*: traits, value types, IO,
  cache semantics, machinery entry-point contracts, and conformance mock.
- **L1 — flexbox** *(complete)*: `compute_flexbox_layout` per the plan above,
  plus the shared machinery it exercises: leaf boxing, hidden-subtree
  cleanup, cache matching policy, root entry, the positioned pass
  (`compute_absolute_layout`), and device-pixel rounding. Browser goldens,
  differential fuzzing, and benchmarks remain parity/performance hardening.
- **L2 — grid**: add `compute_grid_layout` per the plan above,
  `auto-fill`/`auto-fit`, dense packing; baseline alignment and
  grid-area-relative abs-pos as trailing refinements.
- **L3 — lynx-layout adapter**: trait impls over `lynx-widget`/`stylo-dom`,
  stylo→view translation (incl. `CalcHandle` into stylo's calc nodes),
  dispatch, dirty→cache invalidation wiring, `linear`/`relative` algorithms,
  fixed/sticky lowering, text measurement via the text engine.
- **L4 — performance**: probe-trace-tuned cache slots, SoA scratch, arena
  exploration, the batched-children parallel hook if profiles justify it.
- **L5 — parity hardening**: WPT-derived flex/grid suites, web-core
  side-by-side fixtures, fuzzing.

## Open follow-ups (tracked, non-blocking)

- Percentage-height resolution quirks: does Starlight resolve `%` heights
  against indefinite parents anywhere CSS wouldn't? Needs a
  `lynx-behavior-researcher` pass before parity hardening closes this
  edge-case surface.
- Expand `aspect-ratio` parity coverage across `min/max`, stretch, and
  intrinsic keywords.
- Whether Lynx's legacy `grid-*-span` properties need adapter-side lowering
  beyond `span N` placement (tracking doc says they're superseded aliases).
- Crate name availability on crates.io (`neutron-star`) — check before the
  first publish; the protocol doesn't depend on the name.
