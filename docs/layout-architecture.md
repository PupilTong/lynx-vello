# Layout architecture — `neutron-star`

`crates/neutron-star` is lynx-vello's box-layout engine: the from-scratch
successor to the Lynx C++ engine's `starlight`
(`lynx/core/renderer/starlight/`), supporting CSS **flexbox** and **grid**.
It is deliberately Lynx-agnostic and standalone-publishable — zero required
dependencies, no assumption about DOM, style engine, or storage — and every
host boundary is **static dispatch**: `dyn` is impossible by construction,
not by convention.

Status: **L0 (contracts + skeleton)** — every interface and value type is
final-shaped, documented, and conformance-tested against a mock host. The
crate contains **no layout algorithm**: the flex/grid *contracts* (style
traits, `FlexTree`/`GridTree`) exist, their algorithm entry points and
implementations land in L1/L2 (plans below), and the generic machinery
entry points are `todo!()` stubs whose rustdoc is their specification. The
crate rustdoc is the protocol reference; this document is the rationale, the
performance architecture, and the plan.

Behavior spec: [`docs/tracking/css-layout.md`](tracking/css-layout.md)
(what Starlight does, which parts are real W3C features vs Lynx extensions,
and the confirmed deviations). Per the standards policy in
[`AGENTS.md`](../AGENTS.md), flex/grid are implemented from the **W3C specs**
(Flexbox Level 1, Grid Level 2, Sizing Level 3, Box Alignment Level 3), not
by porting Starlight's C++.

## Ownership

```text
              lynx-vello host stack                        standalone
┌──────────────┐   ┌──────────────────────────┐   ┌──────────────────────────┐
│ lynx-widget  │──▶│ lynx-layout (planned)    │──▶│ neutron-star             │
│ + stylo-dom  │   │ host adapter:            │   │ tree/style protocol      │
│ styles, tree │   │ · impls the tree traits  │   │ flexbox algorithm        │
└──────────────┘   │ · stylo → style views    │   │ grid algorithm           │
     ▲             │ · display dispatch       │   │ leaf/hidden/cache/round  │
┌──────────────┐   │ · linear/relative algos  │   │ machinery                │
│ lynx-text    │◀──│ · text measure closures  │   │ (no Lynx vocabulary,    │
│ (planned,    │   │ · fixed/sticky lowering  │   │  no storage, no dyn)    │
│  parley)     │   └──────────────────────────┘   └──────────────────────────┘
└──────────────┘
```

| Layer | Owns | Must not own |
| --- | --- | --- |
| `neutron-star` | Layout algorithms (flex, grid, leaf boxing, hidden, rounding), the protocol vocabulary (geometry, style values, layout IO), cache semantics | Node storage, style storage, display dispatch, Lynx vocabulary (`linear-*`, `rpx`, …), text shaping, stacking/paint order |
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
| `LayoutTree: TraverseTree` | `CoreStyle` views, `resolve_calc`, `set_unrounded_layout`, **`compute_child_layout` (the host dispatch point)** | all algorithms |
| `FlexTree: LayoutTree` | flex container/item style views | the L1 flexbox algorithm |
| `GridTree: LayoutTree` | grid container/item style views (GAT track-list iterators) | the L2 grid algorithm |
| `CacheTree` | per-node measurement-cache slots | `compute_cached_layout` |
| `RoundTree: TraverseTree` | unrounded → final layout storage | `round_layout` |

Entry points (`neutron_star::compute`) are free generic functions — there is
no engine object, so unused entry points never monomorphize into the host.
Landed as machinery stubs: `compute_root_layout`, `compute_leaf_layout`
(host measure closure), `compute_hidden_layout`, `compute_cached_layout`,
`round_layout`. The algorithm entry points arrive with their
implementations, as siblings with the fixed shape

```rust
pub fn compute_flexbox_layout<Tree: FlexTree>(  // L1
    tree: &mut Tree, node: NodeId, input: LayoutInput) -> LayoutOutput;
pub fn compute_grid_layout<Tree: GridTree>(     // L2
    tree: &mut Tree, node: NodeId, input: LayoutInput) -> LayoutOutput;
```

so hosts can already shape their dispatch around them.

Layout IO is three `Copy` PODs: `LayoutInput` (run mode, sizing mode,
known dimensions / parent size / available space) → `LayoutOutput` (size,
content size, baselines) per call, and `Layout` (order, location, size,
content size, scrollbar size, border/padding/margin) as the durable
per-node result. `LayoutInput`/`LayoutOutput`/`Layout` are
`#[non_exhaustive]` so the protocol can grow additively (block-layout margin
collapsing is the known future widener).

**Recursion round-trips through the host.** An algorithm never walks the
tree itself; it calls `tree.compute_child_layout(child, input)` and the host
routes the child — to a neutron-star algorithm, to leaf measurement, or to a
host-private algorithm — wrapping the routing in `compute_cached_layout`.
This single decision buys three properties at once:

1. **Open algorithm set.** Lynx's non-CSS `display: linear` (Android
   `LinearLayout` semantics: `linear-weight`/`linear-gravity`/…) and
   `display: relative` (id-anchored sibling constraint solving) become peer
   algorithms in the host adapter, implemented against the same traits, with
   the engine none the wiser. Same for the `<list>` component's
   staggered-grid. The engine has **no `Display` enum** — dispatch identity
   belongs to the host.
2. **Uniform caching.** Every path through dispatch shares one cache policy,
   so mixed trees (flex containing linear containing grid) memoize
   correctly.
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

**Absolute positioning resolves against the layout parent.** The engine
sizes/places `Position::Absolute` children against the container being laid
out (its padding box), per CSS. Finding the *containing block* is the host's
job (arrange the layout tree so the abs-pos node's parent is its CB). For
Lynx this is degenerate-and-correct: every Lynx element is positioned
(default `position: relative`), so the CSS "nearest positioned ancestor" is
always the parent. `position: fixed` is lowered by the host per the real
W3C rule the tracking doc mandates (viewport root by default, re-anchored
under a transformed/filtered/`will-change` ancestor when present) — the
engine only ever sees `Absolute`. `position: sticky` is a host post-pass
(scroll-time offset clamping), as in production engines.

**Physical axes + `Direction`, no writing modes.** The vendored stylo fork's
`lynx` feature disables `writing-mode` entirely, so the engine is
physical-axis (`x`/`width`, `y`/`height`) with `direction: rtl` (and Lynx's
`lynx-rtl`, lowered by the host) handled inside algorithms by flipping the
main/inline axis — the same simplification Starlight and Yoga make.
Logical properties (`inset-inline-*`, `margin-inline-*`) are resolved to
physical edges by the style system before layout.

**Two layout copies, one rounding pass.** Algorithms produce **unrounded**
`f32` layouts (`set_unrounded_layout`); `round_layout` derives pixel-snapped
finals through `RoundTree` with the cumulative-error-free contract (round
accumulated positions, derive sizes as `round(pos+size) − round(pos)` so
adjacent edges share a physical pixel). Relayout always restarts from
unrounded values — re-rounding rounded values is how engines drift.

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
- **The measurement cache is the asymptotic mechanism.** Flex/grid sizing
  probes children under multiple constraints; uncached, nested containers go
  super-linear (the classic exponential blowup). The protocol bakes the
  fix in: `compute_cached_layout` around every dispatch, per-node slots
  keyed by constraint shape (`cache::Cache`, embeddable, fixed-size,
  allocation-free — `MEASURE_CACHE_SLOTS = 8` measurement slots + 1 layout
  slot, policy documented in the module and validated against probe traces
  in L1).
- **Incremental relayout is a host workflow the protocol supports, not a
  hidden engine mode.** On style/content/children change the host clears
  that node's cache and its ancestors' (dirty-path invalidation), then
  re-runs `compute_root_layout`: clean subtrees answer from their cache slot
  at the recursion boundary without being walked. Hosts can additionally
  choose a nearer relayout root when the dirty node's size can't escape
  (fixed-size subtree) — the engine is agnostic because any node can be a
  root.
- **Allocation strategy (L1 rule, benchmark-gated evolution).** Per-node
  transient state (flex line arrays, grid placement/track vectors) uses
  stack-first small-vector storage sized for the common fan-out, spilling to
  heap only on large child counts; nothing engine-side persists between
  calls. If benches justify it, the upgrade path is a bump-arena scratch
  threaded through the *internal* recursion — deliberately **not** in the
  public protocol (the host round-trip would infect every trait signature
  with scratch lifetimes), so it can be adopted without a protocol break.
- **Data-oriented inner loops.** Inside algorithms, per-item working sets
  are laid out as structure-of-arrays scratch so the resolve-flexible-
  lengths loop and grid track sizing iterate contiguous `f32` runs —
  auto-vectorizable now, explicit SIMD later if profiles demand. This is
  engine-internal and invisible to the protocol.
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
- **Benchmarks from L1.** `divan` (CodSpeed-compatible, per repo toolchain)
  micro/macro benches: deep flex nesting, wide children, wrap-heavy,
  grid auto-placement, incremental single-node dirty. Baselines: Taffy and
  Yoga on equivalent trees — not to copy their design, but to keep
  "high-performance" falsifiable.

## Algorithm plans (L1/L2)

Kept here — not in the crate — so the code stays contracts-and-skeleton
until the implementations land. Starlight's C++ mirrors the same spec steps
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
8. **Absolutely-positioned children** (§4.1) — sized/placed against the
   container's padding box from insets; static position per §9.8.
9. **Finalize** — per-child `set_unrounded_layout` (skipped under
   `ComputeSize`), container border-box size, `content_size` accumulation,
   container baseline.

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

- **L0 (landed):** `tests/protocol.rs` — a complete mock host implementing
  every trait over plain `Vec` storage; proves the protocol is implementable
  without `dyn` (plus the `compile_fail` doctest making non-object-safety a
  tested guarantee), exercises the GAT track-list machinery, and pins every
  entry point as callable (`#[should_panic]` on the `todo!` stubs).
- **L1+:** spec-derived unit fixtures per algorithm pass; golden layout
  trees compared against Chrome-computed geometry for the same CSS
  (web-core parity is the project bar — an optional `serde` feature for
  serializing fixtures/results is introduced together with these tests, by
  the user's call it stays out of L0); differential fuzzing against Taffy
  on the shared feature subset (cheap oracle, since the protocols are
  shaped alike); the benches above.
- **Lynx modes (host-side):** `linear`/`relative` conformance fixtures come
  from Starlight behavior per `docs/tracking/css-layout.md`, tested in the
  adapter crate, not here.

## Milestones

- **L0 — contracts + skeleton** *(this change)*: traits, value types, IO,
  cache semantics, machinery entry-point contracts, conformance mock, this
  document. Deliberately **no algorithm code** — not even stubs — for
  flex/grid.
- **L1 — flexbox**: add `compute_flexbox_layout` and implement it per the
  plan above, plus the shared machinery it exercises: leaf boxing, hidden
  layout, cache matching policy, root entry, rounding. Benches + Chrome
  goldens.
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

## Open questions (tracked, non-blocking)

- Percentage-height resolution quirks: does Starlight resolve `%` heights
  against indefinite parents anywhere CSS wouldn't? Needs a
  `lynx-behavior-researcher` pass before L1 finalizes `parent_size`
  edge-case semantics.
- `aspect-ratio` interaction matrix (with `min/max`, stretch, and intrinsic
  keywords) — spec-complete in L1 or staged?
- Whether Lynx's legacy `grid-*-span` properties need adapter-side lowering
  beyond `span N` placement (tracking doc says they're superseded aliases).
- Crate name availability on crates.io (`neutron-star`) — check before the
  first publish; the protocol doesn't depend on the name.
