# Layout architecture — `neutron-star` + `lynx-layout`

`crates/neutron-star` is lynx-vello's box-layout engine: the from-scratch
successor to the Lynx C++ engine's `starlight`
(`lynx/core/renderer/starlight/`). It implements CSS **flexbox** and **Grid**
plus the standalone Starlight **relative-layout** extension. It is
host/storage-agnostic and standalone-publishable — zero required dependencies,
no assumption about DOM, style engine, or storage — and every host boundary
is **static dispatch**: `dyn` is impossible by construction, not by
convention.

Status: **L2R (Relative) + standalone linear milestone** — `neutron-star`'s
protocol, generic machinery, cache, leaf and positioned sizing, rounding,
CSS Flexbox Level 1, numeric CSS Grid Level 2, and Starlight Relative Layout
Level 1 algorithms are implemented and conformance-tested against
plain-storage mock hosts. Grid excludes subgrid and named lines/areas, which
are outside the current protocol.
`crates/lynx-layout` implements the Lynx-only linear computed-style/source
protocol and generic `compute_linear_layout` peer algorithm over that
machinery; the concrete Widget/stylo adapter is not implemented yet. Crate
rustdoc is the API reference; this document is the rationale, performance
architecture, and remaining plan.

Behavior spec: [`docs/tracking/css-layout.md`](tracking/css-layout.md)
(what Starlight does, which parts are real W3C features vs Lynx extensions,
and the confirmed deviations). Per the standards policy in
[`AGENTS.md`](../AGENTS.md), flex and Grid are implemented from the
**W3C specs** (Flexbox Level 1, Grid Level 2, Sizing Level 3, Box Alignment
Level 3), not by porting Starlight's C++. Relative is a Lynx-only extension
and follows the normative
[`Starlight Relative Layout Module Level 1`](starlight-relative-layout.md),
with explicitly documented Rust-surface defaults.

## Ownership

```text
              lynx-vello host stack                        standalone
┌──────────────┐   ┌──────────────────────────┐   ┌──────────────────────────┐
│ lynx-widget  │   │ lynx-layout              │──▶│ neutron-star             │
│ + stylo-dom  │   │ implemented:             │   │ tree/style protocol      │
│ styles, tree │   │ · linear style/source    │   │ flexbox algorithm        │
└──────┬───────┘   │ · linear algorithm       │   │ grid + relative algos    │
       │ future    │ future adapter:          │   │ leaf/hidden/cache/round  │
       └──────────▶│ · source/session/dispatch│   │ support machinery        │
┌──────────────┐   │ · fixed/dirty/staggered  │   │ (one Lynx extension;     │
│ lynx-text    │──▶│ · generic leaf measurers │   │  no storage, no dyn)     │
│ (planned,    │   │ · text artifact storage  │   └──────────────────────────┘
│  parley)     │   └──────────────────────────┘
└──────────────┘
```

| Layer | Owns | Must not own |
| --- | --- | --- |
| `neutron-star` | Flex, Grid, and standalone Starlight Relative algorithms; public support arithmetic; leaf boxing, hidden-subtree cleanup, rounding; protocol vocabulary (geometry, style values, layout IO); cache semantics | Node storage, style storage, display dispatch, other Lynx vocabulary (`linear-*`, `rpx`, …), text shaping, stacking/paint order |
| `lynx-layout` | Implemented Lynx-linear value/style/source protocol and generic linear algorithm; future immutable views over the widget/style tree, mutable layout/cache session, display dispatch, `staggered` algorithm, relative computed-style translation, dirty tracking + cache invalidation, and fixed/sticky lowering | A second flex/grid/relative implementation, engine-side copies of styles |
| `lynx-text` *(planned)* | A generic `LeafMeasurer` adapter over Parley contexts and a host-owned retained text-layout cache | Box layout |

The engine/host seam is exactly the seam that makes the crate publishable:
the relative-layout protocol is the deliberate built-in Starlight extension,
while other Lynx-specific vocabulary lives in `lynx-layout`. Its linear
algorithm is a first-class peer rather than a flex translation. The
still-future concrete adapter is otherwise mechanical: computed-style
accessor translation, separate source/session storage, and one display-mode
dispatch.

## The protocol in one page

Traits (`neutron_star::tree`), layered by capability so each entry point
demands only what it uses:

| Trait | Adds | Consumed by |
| --- | --- | --- |
| `TraverseTree` | immutable child iteration (GAT iterator) | everything |
| `LayoutSource: TraverseTree` | immutable `CoreStyle` views and `resolve_calc` | all algorithms |
| `FlexSource: LayoutSource` | immutable flex container/item style views | the L1 flexbox algorithm |
| `GridSource: LayoutSource` | immutable grid container/item views, including GAT track-list iterators | the L2 grid algorithm |
| `RelativeSource: LayoutSource` | immutable relative container/item style views | the Starlight Relative L1 algorithm |
| `lynx_layout::LinearSource: LayoutSource` | immutable Lynx-linear container/item style views | the host-side linear algorithm |
| `LayoutState` | mutable unrounded-layout and static-position storage | committing algorithms and hidden cleanup |
| `CacheState` | mutable per-node measurement-cache slots | `compute_cached_layout` and hidden cleanup |
| `LayoutSession<Source>: LayoutState + CacheState` | **`compute_child_layout(source, …)`**, the host display/algorithm dispatch point | recursive algorithms |
| `RoundState` | mutable unrounded → final layout storage | `round_layout` |

Entry points (`neutron_star::compute`) are free generic functions — there is
no engine object, so unused entry points never monomorphize into the host.
Implemented machinery: `compute_root_layout`, `compute_leaf_layout`
(generic `LeafMeasurer`), explicit hidden-subtree cleanup via
`hide_subtree`, `compute_cached_layout`
(keyed on the **complete `LayoutInput`** — see the caching section),
`compute_absolute_layout` (the positioned pass for out-of-flow nodes whose
containing block is not their formatting parent), and
`round_layout(source, state, root, scale)` (device-pixel snapping), plus
`compute_flexbox_layout`, `compute_grid_layout`, and
`compute_relative_layout`. In addition,
`neutron_star::compute::support` publicly exposes
the same allocation-free length, edge, box-sizing, aspect-ratio, clamp, and
relative-offset arithmetic to host-side peer algorithms. The live flex and
Grid entry points and the host-side linear entry point all use the same fixed
shape:

```rust
pub fn compute_flexbox_layout<Source, Session>(
    source: &Source, session: &mut Session, node: NodeId, input: LayoutInput)
    -> LayoutOutput
where Source: FlexSource, Session: LayoutSession<Source>;

pub fn compute_grid_layout<Source, Session>(
    source: &Source, session: &mut Session, node: NodeId, input: LayoutInput)
    -> LayoutOutput
where Source: GridSource, Session: LayoutSession<Source>;

pub fn compute_relative_layout<Source, Session>(
    source: &Source, session: &mut Session, node: NodeId, input: LayoutInput)
    -> LayoutOutput
where Source: RelativeSource, Session: LayoutSession<Source>;

pub fn compute_linear_layout<Source, Session>( // implemented in lynx-layout
    source: &Source, session: &mut Session, node: NodeId, input: LayoutInput)
    -> LayoutOutput
where Source: LinearSource, Session: LayoutSession<Source>;
```

All four signatures are public; hosts select them in their display dispatch.

Layout IO is three `Copy` PODs: `LayoutInput` (layout goal, sizing mode,
known dimensions, whether those dimensions establish definite percentage
bases, parent size, and available space) → `LayoutOutput` (size, content size,
baselines) per call, and `Layout` (order, location, size, content size,
scrollbar size, border/padding/margin) as the durable per-node result. The
separate `definite_dimensions` field is necessary because Flexbox can decide
an item's used geometry while §9.8 still classifies that size as indefinite
for percentages in descendants; Grid has the same distinction. A geometric
`Some(size)` therefore cannot double as a definiteness flag.
`LayoutGoal::Measure(RequestedAxis)` makes measurement and its requested axes
one value; `LayoutGoal::Commit` requests durable child geometry.
Hidden-subtree cleanup is deliberately outside this sizing API.
`LayoutInput`/`LayoutOutput`/`Layout` are
`#[non_exhaustive]` so the protocol can grow additively (block-layout margin
collapsing is the known future widener).

**Recursion round-trips through the host.** An algorithm reads topology and
styles from `source`, then calls
`session.compute_child_layout(source, child, input)`. The host
first handles `BoxGenerationMode::None` by calling `hide_subtree` and
returning `LayoutOutput::HIDDEN`; this explicit cleanup precedes and bypasses
the cache. For a generated box, the host routes to a neutron-star algorithm,
leaf measurement, or a host-private algorithm, wrapping that routing in
`compute_cached_layout`. This decision buys three properties at once:

1. **Open algorithm set.** Starlight `display: relative` is a built-in peer
   selected through `RelativeSource`; Lynx's non-CSS `display: linear`
   (Android `LinearLayout` semantics: `linear-weight`/`linear-gravity`/…) is an
   implemented peer algorithm in `lynx-layout`, against the same source and
   session protocol with the engine none the wiser. The `<list>` component's
   staggered-grid remains a future host peer. The engine has **no `Display`
   enum** — dispatch identity belongs to the future concrete host adapter.
2. **Uniform caching.** Every generated-box path through dispatch shares one
   cache policy, so mixed trees of engine and host-private layout modes
   memoize correctly. Hidden cleanup deliberately stays outside that cache
   boundary.
3. **Partial relayout.** Any node can be a layout root; the engine never
   assumes global tree access.

## Design decisions and their rationale

**No `dyn`, enforced structurally.** Source and measurement traits carry GATs
(borrowed child iterators, borrowed style views, borrowed grid track
iterators), which makes them non-object-safe: `dyn LayoutSource` and
`dyn LeafMeasurer` are compile errors — there is a `compile_fail` doctest
pinning this. What the constraint buys: every
host⇄engine call site monomorphizes, inlines, and const-folds (style
accessors returning constants collapse into the algorithm); no vtable
indirection in the hottest recursion of the frame. Mutable layout/cache/round
capability traits additionally require `Sized`, so they cannot be passed as
trait objects around the GAT boundary. The accepted costs:
compile time and per-host codegen (one copy of the algorithms per concrete
source/session combination — in practice one pair per binary), and no
heterogeneous "list of engines" (not a goal).

**The host owns—and separates—all storage.** Immutable semantic data
(topology, child order, computed styles, calc expressions, and leaf content)
lives behind `LayoutSource`; mutable results, caches, measurement contexts,
and retained text artifacts live behind `LayoutSession`. Both sides use the
same opaque `NodeId(u64)`. This is a real storage/lifetime boundary, not two
traits implemented on one `&mut Tree`: source views must remain valid while
recursion mutates the session. A host normally uses parallel document/layout
arenas or constructs an ephemeral session borrowing its mutable stores.
`RefCell`/`UnsafeCell` is not an acceptable substitute for this split.

A layout run observes one immutable **source epoch**. Style, content, child
order, and `NodeId` validity cannot change during recursion; such mutations
are staged, invalidate the affected box and measurement caches, and start a
new epoch. Virtualized components therefore realize their visible topology
before layout or explicitly restart after realization.

**Style is read through views, in engine vocabulary.** Style traits
(`CoreStyle` + container/item traits per algorithm) hand out small `Copy`
values (`Dimension`, `LengthPercentage`, alignment enums, grid track types)
per accessor call — lazy translation from stylo's `ComputedValues`, no
materialized engine-side style structs. Grid track *lists* stay borrowed GAT
iterators with `repeat()` as a nested `GridTemplateRepetition` value, so
even the sequence-valued styles cross the boundary without allocation.
CSS trait-method defaults are **CSS initial values**; Relative methods use
the standalone Relative Level 1 initial values. Lynx's divergent defaults
are computed-value policy and stay in the host's style system:

| Property | Standalone engine default | Lynx computed default (host supplies) |
| --- | --- | --- |
| `box-sizing` | `content-box` | `border-box` (`auto` → border-box) |
| `overflow` | `visible` | `hidden` |
| `position` | `relative` ≙ static | `relative` (same thing — Lynx has no `static`) |
| `align-items`/`align-content` | `normal` (`None`) | `stretch` (host passes it explicitly) |
| `relative-layout-once` | `false` (Relative L1) | `true` |

**`calc()` without a CSS dependency.** Percent-bearing `calc()` can only be
resolved during layout, and its AST lives in stylo. The protocol carries an
opaque `CalcHandle(u64)`; algorithms resolve through
`LayoutSource::resolve_calc(handle, basis)`. Zero parser dependency, full
`calc()` support.

**Leaf measurement is generic behavior with a borrowed result view.**
`LeafMeasurer` is a GAT-based, statically-dispatched interface whose
engine-specific `Measurement<'a>` implements the accessor-only
`LeafMeasurement` trait. `compute_leaf_layout` immediately normalizes that
view into the concrete `LeafMetrics` POD used by box math. This lets a Parley
adapter retain an owned `parley::Layout` in a host cache and return a borrowed
wrapper exposing its size and first baseline—no cloning, reshaping, or Parley
type in neutron-star. The host's leaf dispatch constructs this node-scoped
adapter by borrowing immutable text/style content from the source and mutable
Parley contexts/artifact slots from the session. Different leaf dispatch arms
may instantiate `compute_leaf_layout` with different concrete measurer types
(text, image, custom content), so no common trait object or engine enum is
required. The text artifact cache is separate from the box cache: measurement
probes must not evict the committed paint artifact, and the artifact must
outlive any committed box-cache entry that can skip shaping.
`LeafMeasureInput::goal` carries that probe/commit distinction; no separate
run-mode flag is needed.

**Out-of-flow: the layout tree is the formatting structure; the containing
block is data, not topology.** Out-of-flow nodes are **never reparented** —
they stay children of their formatting parent, because CSS derives their
*static position* from that parent's formatting context (Flexbox §4.1: as
if the sole flex item under the container's alignment; Grid §10.2: the
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
  and records it via `LayoutState::set_static_position`, but does not size
  or place it. After in-flow layout the host runs the **positioned pass**:
  it resolves the CB node (for Lynx `fixed`: the viewport root, or the
  nearest transformed/filtered/`will-change` ancestor per the W3C rule the
  tracking doc mandates), converts the recorded static position into CB
  padding-box space (all unrounded layouts exist by then), and calls
  `compute_absolute_layout(source, session, node, cb_padding_box_size,
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
`round_layout(source, state, root, scale)` derives snapped finals through
`TraverseTree` + `RoundState`. `scale` is the device-pixel ratio (physical px per CSS px):
coordinates are CSS pixels but crisp edges are physical, so snapping is
`snap(v) = css_round(v × scale) / scale` — on a DPR-2 screen `0.5` CSS px is
already an exact physical edge and must survive. The cumulative-error-free
contract still holds (snap accumulated positions, derive sizes as
`snap(pos+size) − snap(pos)` so adjacent edges share a physical pixel).
`css_round` follows [CSS Values' nearest-integer tie rule](https://drafts.csswg.org/css-values-4/#integers)
toward positive infinity (`1.5 → 2`, `-1.5 → -1`), rather than Rust's
away-from-zero rule for negative halves. Flex sizing itself remains
fractional; this optional final pass does not introduce Lynx integer
layout-unit semantics.
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
- **Shared setup, flat hot scratch.** Flex, Grid, and Relative reuse the same
  inline, statically-dispatched ordering and box-resolution helpers. Their temporary
  `ResolvedItemBox`/`ResolvedContainerBox` PODs eliminate duplicate sizing
  rules at the algorithm boundary, then each algorithm destructures item
  values into its own flat scratch record. Inner loops therefore retain
  direct field access without a shared trait, `dyn`, or nested algorithm
  state. Relative additionally resolves every id reference once into a compact
  ordered-item index; its positioning passes never hash or binary-search ids.
  The item resolver is force-inlined because release-IR inspection
  showed that ordinary inlining materialized a 216-byte return temporary;
  forced inlining lets scalar replacement remove that copy chain.
- **The measurement cache is the asymptotic mechanism.** Flex, Grid, and
  two-pass Relative sizing probe children under multiple constraints; uncached,
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
  size/min/max/aspect-ratio, `definite_dimensions` preserves percentage
  definiteness independently from decided geometry, and `parent_size` is the
  percentage basis. All change results, so dropping any of them from the key
  would alias distinct layouts; matching may coalesce entries only under
  provable equivalences (documented in the `cache` module).
- **Incremental relayout is a host workflow the protocol supports, not a
  hidden engine mode.** On style/content/children change the host clears
  that node's cache and its ancestors' (dirty-path invalidation), then
  re-runs `compute_root_layout`: clean subtrees answer from their cache slot
  at the recursion boundary without being walked. Hosts can additionally
  choose a nearer relayout root when the dirty node's size can't escape
  (fixed-size subtree) — the engine is agnostic because any node can be a
  root.
- **Allocation strategy (current).** Algorithms use bounded transient `Vec`
  scratch. Relative deduplicates each item's at-most-eight dependencies in a
  fixed inline `u32` array, bypasses graph construction entirely when no
  sibling reference exists, builds reverse dependencies once as CSR offsets
  plus one flat edge vector, and reuses its count storage as the Kahn queue.
  A `u8` indegree sentinel and monotonic lowest-index cursor provide
  allocation-free linear-time cycle fallback after CSR construction. Grid
  expands borrowed tracks once and keeps contribution state in
  compact per-item/track vectors. Placement uses a packed `u64` occupancy
  matrix with collision jumps for ordinary grids, then switches to sorted
  row intervals above eight million cells so the §5.4 worst case does not
  eagerly allocate a roughly 50 MB rectangle. Nothing engine-side persists
  between calls.
  Benchmark-gated upgrades
  include stack-first storage or a bump arena threaded through *internal*
  recursion — deliberately **not** through the public protocol, where scratch
  lifetimes would infect every host trait signature. Either can be adopted
  without a protocol break.
- **Data-oriented inner loops.** Grid placement scans packed words rather
  than prior items; sparse locked-axis cursors use range-max updates, and
  track/contribution state stays in contiguous vectors. Profiles may justify
  further structure-of-arrays storage or explicit SIMD later; these remain
  engine-internal changes, invisible to the protocol.
- **Parallelism: designed, deferred, additive.** The immutable source is
  naturally shareable, but `LayoutSession::compute_child_layout`'s mutable
  session recursion is inherently sequential — correct for v0 (layout is
  rarely the bottleneck vs paint/style, and Yoga/Taffy/Starlight are all
  sequential). The planned extension is a **batched child-layout hook**: a
  defaulted `LayoutSession` method like
  `compute_child_layouts(&mut self, source, requests)`
  that algorithms call at fan-out points (independent flex-item measure
  probes, grid item contributions); the default body is today's sequential
  loop, and a parallel host overrides it to shard sub-trees across its own
  pool (host storage, host threading policy — the engine stays
  thread-unaware). Adding a defaulted method is semver-minor, so this ships
  when profiles earn it, without a protocol break.
- **Flex, Grid, and Relative benchmarks are landed; broader performance
  hardening remains.** The `divan` (CodSpeed-compatible) suite ports the 18 Flex-tagged
  workloads from `PupilTong/lynx#25` and measures neutron-star's Rust path
  only. Direct Flex workloads retain their complete trees; mixed-display
  workloads explicitly benchmark the Flex-owned slice rather than invoking
  foreign algorithms or the Lynx C++ runner. The Grid suite covers scaled
  sparse/dense auto-placement, fixed/`fr` tracks, unique intrinsic span
  buckets, flex freeze thresholds, cold/warm nested grids, a root cache hit,
  and dirty-leaf ancestor invalidation. The Relative suite covers independent
  items, reverse dependency chains, duplicate ids, adversarial disjoint
  cycles, one-pass versus two-pass solving, nested cold layout, warm
  descendants, root cache hits, and auto-width refinement. The refinement
  path preserves measurements for unchanged, definite fixed items while
  percentage-dependent or newly double-anchored items remeasure. The separate
  PR #25 Relative target ports all nine source workloads that contain a real
  `display: relative` branch; mixed-display workloads keep only their Relative
  slice, and every timed invocation uses a fresh Rust-only tree.
  Equivalent-tree Taffy/Yoga and other
  cross-engine differential baselines remain future additions — not to copy
  those engines' designs, but to keep "high-performance" falsifiable.

## Algorithms (Flex, Grid, Relative, and standalone Linear implemented)

This pass structure documents the implemented L1 Flex and L2 Grid algorithms
plus the implemented Relative L1 and host-side Linear algorithms.
Starlight's C++ mirrors the same spec steps
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

**Linear (standalone host-side milestone implemented)** — Starlight linear
layout, as a single-axis pipeline in `crates/lynx-layout`:

1. **Setup** — resolve the container box and axes, classify hidden,
   out-of-flow, and in-flow children, and stably apply non-zero `order`.
2. **Item sizing** — resolve each item's box model, preferred/min/max sizes,
   aspect ratio, intrinsic constraints, effective cross gravity, and auto
   margins through neutron-star's shared support arithmetic.
3. **Weight distribution** — when the main content size is definite,
   distribute remaining space among positive `linear-weight` items using the
   explicit positive `linear-weight-sum` denominator when present, with
   iterative min/max freezing and final child relayout.
4. **Container sizing** — use the definite content size or the final sum/max
   of item outer sizes, then apply padding, border, aspect ratio, and min/max
   clamps; cyclic inline percentages receive a targeted second pass.
5. **Alignment** — map `linear-gravity`/`justify-content` on the main axis and
   item/container linear gravity plus standard alignment on the cross axis,
   including RTL/reverse axes and cross-axis auto margins.
6. **Commit and baseline** — lay out in-flow children with their final known
   dimensions, apply relative insets, store parent-relative layouts, and
   export horizontal/vertical-container baselines.
7. **Out-of-flow children** — derive linear-aware static positions, lay out
   parent-contained absolute children against the padding box, and record
   hoisted static positions for the later host fixed-position pass.
8. **Measure-only path** — return sizes and baselines without durable child
   writes, retaining the same `LayoutInput`/cache semantics as flexbox.

This is a generic algorithm over `LinearSource` and
`LayoutSession<Source>`; it is not yet wired to `WidgetTree` or stylo
computed values.

**Grid (L2)** — CSS Grid Level 2 (minus subgrid), as a pipeline:

1. **Explicit grid resolution** (§7.2–7.5) — expand `GridTemplateComponent`
   lists into concrete track vectors in algorithm scratch; solve
   `repeat(auto-fill/auto-fit)` against preferred/max/min content-box
   constraints, including percentage gutters and `auto-fit` collapse.
2. **Placement** (§8) — resolve `Line<GridPlacement>` per item
   (start/end/span conflict rules §8.3.1), auto-placement in
   `grid_auto_flow` order with the sparse/dense cursor (§8.5), implicit
   tracks from `grid-auto-rows`/`-columns` (cycled, §7.6), `auto-fit`
   empty-track collapse.
3. **Track sizing** (§12.3, run per axis — columns then rows) — the
   intrinsic track-sizing algorithm: initialize base/growth-limit, apply
   baseline shims, distribute item contributions in span order (including
   infinitely-growable and non-affected-track phases), maximize tracks,
   expand `fr` (§12.7), and stretch `auto` tracks. A bounded cross-axis
   feedback pass detects contribution changes after row sizing. While
   columns are initially sized, only rows with definite max track sizing
   functions provide finite block-axis space; other rows are infinite per
   §12.1. This per-run columns→rows correction is independent of the outer
   Grid layout phases: cyclic percentages are `auto` while finding an
   intrinsic container size, then resolve against that resulting size in a
   final Grid sizing run.
4. **Alignment** (CSS Align) — `align/justify-content` position tracks with
   `gap`; `align/justify-self` place items in their areas; `Rtl` flips the
   inline axis.
5. **Item layout & finalize** — final child layout at known area sizes,
   first-baseline sharing, direct abs-pos children against a resolved grid
   area, `set_unrounded_layout`, container size, and `content_size`.

Last-baseline alignment, subgrid, named lines/areas, fragmentation, and
masonry/`staggered-grid` stay out of scope. The last is a Lynx
`<list>`-component concern, not a Grid mode.

**Starlight Relative (L1)** — the non-CSS id-constrained formatting context:

1. **Setup** — resolve the container box, exclude hidden and out-of-flow
   children, stable-sort relative items by `order`, build a last-wins id map,
   and resolve each physical alignment/adjacency reference once.
2. **Dependency ordering** — deduplicate at most four dependencies per axis
   (eight combined), build CSR reverse edges, then run deterministic Kahn
   traversal; a monotonic lowest-index fallback breaks cycles in linear time.
3. **One-pass mode** — walk one combined order, measure each item under the
   currently resolved horizontal/vertical sides, position both margin-edge
   pairs, and grow wrap-content bounds.
4. **Two-pass mode** — measure initial parent constraints, position separate
   horizontal/vertical orders, selectively remeasure tightened double-sided
   constraints, resolve wrap width and cyclic percentages, then resolve height
   and recompute final positions against both final content extents.
5. **Finalize** — commit border-box locations from the content origin plus
   margin-edge positions, apply CSS relative inset offsets visually, and use
   the common absolute/hoisted passes. Relative containers export no baseline.

## Testing strategy

- **L0/L1/L2/L2R (landed):** `tests/protocol.rs` — a complete mock host implementing
  every trait over physically separate immutable-source and mutable-session
  `Vec` storage; proves the protocol is implementable
  without `dyn` (plus `compile_fail` doctests pinning both the GAT and explicit
  `Sized` barriers), exercises the GAT track-list machinery and all shared
  machinery entry points. `tests/flexbox.rs` supplies a styling-engine-free
  host and spec-derived fixtures for axes, flexing, wrapping, intrinsic
  contributions, alignment, baselines, box sizing, percentages, auto
  minimums, out-of-flow static positions, and measurement-only runs. Leaf
  unit tests additionally cover non-`Clone` borrowed GAT results, separate
  probe/commit artifacts, committed box-cache hits, and coordinated
  invalidation. `tests/grid.rs` covers numeric placement, sparse/dense and
  row/column auto-flow, implicit/automatic tracks, intrinsic spanning
  contributions, alignment, RTL, nested dispatch, and out-of-flow behavior.
  Private unit tests pin placement bit ranges, clamping, repeat expansion, and
  track cycling. `tests/relative.rs` covers every physical reference family,
  duplicate/reserved ids, both solver modes, cycles, intrinsic and percentage
  sizing, parent min/max feedback, selective wrap-width remeasurement,
  hidden/out-of-flow behavior, nested dispatch, and measurement-only runs.
  CI enforces at least 95% line coverage for `neutron-star`
  production source while excluding test and benchmark source from the
  metric.
- **Parity/performance hardening:** the Rust-only Flex benchmark suite and
  migrated `PupilTong/lynx#25` Flex fixtures are landed alongside the Grid
  benchmark suite above, the graph/cache-focused Relative CodSpeed suite, and
  the complete Rust-only PR #25 Relative migration. The latter retains 72
  direct tests, 72 native head-to-head Rust cases, 15 generated matrices (429
  cases), 401 standalone cases, two dedicated standalone-public snapshots plus
  the mixed display matrix's Relative branch, two sticky-boundary cases, four
  algorithm inventory guards, and nine benchmark workloads; its detailed
  source map is recorded in
  [`pr25-relative-migration.md`](pr25-relative-migration.md).
  Browser geometry goldens and differential fuzzing
  against Taffy on the shared feature subset remain planned.
- **Standalone Lynx linear (landed):** `crates/lynx-layout` carries the
  styling-engine-free mock-host seam for linear conformance and performance
  scenarios. Its source of behavior is Starlight plus
  `docs/tracking/css-layout.md`; neutron-star remains unaware of Lynx values.
- **Remaining Lynx integration:** Widget/stylo translation, relative
  computed-style translation, component-specific staggered layout, and
  mixed-runtime parity belong in `lynx-layout`.

## Milestones

- **L0 — contracts + skeleton** *(complete)*: traits, value types, IO,
  cache semantics, machinery entry-point contracts, and conformance mock.
- **L1 — flexbox** *(complete)*: `compute_flexbox_layout` per the plan above,
  plus the shared machinery it exercises: leaf boxing, hidden-subtree
  cleanup, cache matching policy, root entry, the positioned pass
  (`compute_absolute_layout`), and device-pixel rounding. The Rust-only Flex
  fixture and benchmark migration is landed; browser goldens and differential
  fuzzing remain parity/performance hardening.
- **L2 — grid** *(complete)*: `compute_grid_layout`, `auto-fill`/`auto-fit`,
  dense packing, first-baseline alignment, and direct-child
  grid-area-relative absolute positioning.
- **L2R — Starlight relative** *(complete)*: `RelativeSource`, the one-pass
  combined and two-pass per-axis dependency solvers, intrinsic/percentage
  remeasurement, deterministic cycles, out-of-flow handling, conformance
  fixtures, the full Rust-only PR #25 test/benchmark migration, and CodSpeed
  benchmarks.
- **L3 — lynx-layout host layer** *(partial)*: the crate, Lynx-linear
  value/style/source protocol, and standalone generic
  `compute_linear_layout` algorithm are complete. Remaining L3 work is the
  concrete `lynx-widget`/stylo adapter (including `CalcHandle` translation),
  mutable session and display dispatch, dirty→cache invalidation wiring, the
  relative computed-style translation, root fixed-position pass and sticky
  lowering, and a generic Parley `LeafMeasurer` with retained text-layout
  storage.
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
