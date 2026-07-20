# Layout architecture — `neutron-star`

`crates/neutron-star` is lynx-vello's box-layout engine: the from-scratch
successor to the Lynx C++ engine's `starlight`
(`lynx/core/renderer/starlight/`). It implements CSS **flexbox**, CSS **Grid**,
and Lynx's Starlight **Relative** and **Linear** layouts as first-class peer
algorithms. It is host/storage-agnostic — the engine owns no tree, no styles,
and no per-node storage — but it **speaks the stylo fork's computed-value
vocabulary**: style accessors return the lynx stylo fork's computed types
directly, so `stylo` (feature `lynx`) is a required dependency and the former
zero-dependency/standalone-publishable pillar is retired (building needs the
`vendor/stylo` submodule and `python3` for stylo's build script; a cold build
takes minutes). Every host boundary is **static dispatch**: `dyn` is
impossible by construction, not by convention. Default builds additionally
enable the optional `text` feature and its Parley-backed measurement core;
`default-features = false` keeps the protocol and box-layout core only.

Status: **Flexbox, Grid, Relative, Linear, and text measurement implemented** —
`neutron-star`'s protocol, generic machinery, cache, leaf and positioned
sizing, rounding, CSS Flexbox Level 1, numeric CSS Grid Level 2, Starlight
Relative Layout Level 1, Starlight Linear algorithms, and the default-on
Parley text measurement core are implemented and conformance-tested against
plain-storage mock hosts. Grid excludes subgrid and named lines/areas, which
are outside the current protocol. The concrete document/stylo host is
implemented in `w3c-dom`'s `layout` module (`StyleEngine::layout_document`):
a two-word `Copy` `LayoutNode` handle over the document slab, style views
lending `ComputedValues` fields (materialized once per pass), display
dispatch, per-node layout state on each `Node`, the fixed/hoisted
positioned pass, and device-pixel rounding, with leaf content measurement
left to an embedder hook. Text truncation, inline boxes, and the
Lynx-widget policy layer (text style/attribute wiring and
text-context/artifact-slot storage included) are not implemented yet. Crate
rustdoc is the API reference; this document is the rationale, performance
architecture, and remaining plan.

Behavior inventory: [`docs/tracking/css-layout.md`](tracking/css-layout.md)
(what Starlight does, which parts are real W3C features vs Lynx extensions,
and the confirmed deviations). The executable standards/source baseline and
scope are recorded in
[`docs/layout-conformance.md`](layout-conformance.md). The standalone Linear algorithm is specified in
[`docs/starlight-linear-layout.md`](starlight-linear-layout.md). Per the
standards policy in
[`AGENTS.md`](../AGENTS.md), flex and Grid are implemented from the
**W3C specs** (Flexbox Level 1, Grid Level 2, Sizing Level 3, Box Alignment
Level 3), not by porting Starlight's C++. Relative and Linear are Lynx-only
extensions. Relative follows the normative
[`Starlight Relative Layout Module Level 1`](starlight-relative-layout.md),
with explicitly documented Rust-surface defaults; Linear follows the
standalone specification linked above and Starlight's behavior.
Text behavior is inventoried in
[`docs/tracking/css-text.md`](tracking/css-text.md).

## Ownership

```text
        lynx-vello host stack                           engine
┌──────────────────────────────────┐     ┌─────────────────────────────────┐
│ lynx-widget + w3c-dom          │     │ neutron-star                    │
│ styles and tree                  │     │ tree/style/text protocols       │
│                                  │     │ flex / grid / relative / linear │
│ future runtime integration:      │────▶│ text feature: Parley measure    │
│ LayoutNode handles + dispatch    │     │ leaf/hidden/cache/position/round│
│ fixed/dirty/staggered integration│     │ stylo vocabulary, no storage    │
└──────────────────────────────────┘     └─────────────────────────────────┘
```

| Layer | Owns | Must not own |
| --- | --- | --- |
| `neutron-star` | Implemented Flex, Grid, Relative, and Linear algorithms; their style-view protocols speaking stylo computed values (including the `relative-*` and `linear-*` longhands); the text style/run protocol; leaf boxing, hidden-subtree cleanup, positioned layout, rounding; shared private arithmetic; geometry and layout IO; cache semantics | Node/style/content storage, display dispatch, DOM/widget types, an engine-side style value vocabulary (it re-exports stylo's), resolved device-unit policy (`rpx`, etc.), stacking/paint order |
| `neutron-star::text` (`text` feature, default-on) | Parley context/font registration, whitespace processing, shaping, line breaking, intrinsic and height-for-width measurement, baselines, and retained `TextLayout` artifact types | Text truncation and ellipsis, inline boxes, paint styling, widget/attribute lowering, resource fetching, or host cache and per-node slot storage |
| `w3c-dom::layout` (implemented) | The `LayoutNode` handle over the document slab (node + pass context, two words); style views lending `ComputedValues` fields (materialized once per pass; logical `relative-*-inline-*` lowering; the W3C fixed/absolute containing-block rule expressed through the protocol's `position()` scheme); display dispatch (flex/grid/linear/relative, `display: none` hiding, leaf fallback); per-node `LayoutData` on `Node` (`AtomicRefCell` — measurement cache, unrounded + rounded layouts, created and dropped with the node); the hoisted positioned pass; device-pixel rounding; the embedder leaf-measurement hook and the manual `Document::invalidate_layout` API | A second layout algorithm, engine-side style copies, Lynx widget vocabulary or device-unit policy (`rpx`), Lynx computed defaults (cascade/UA-sheet policy), text shaping |
| Remaining runtime integration (`lynx-widget`, future) | Automatic style-damage → `Document::invalidate_layout` wiring; Lynx view metrics and `rpx` policy; text style/attribute wiring and text-context/artifact-slot storage behind the leaf hook; `staggered` integration; sticky lowering | A second Flex/Grid/Relative/Linear/text-measurement implementation, engine-side copies of styles |

The engine/host seam keeps the engine storage-free even though its
vocabulary is stylo's: the Lynx-specific values and algorithms for Relative
and Linear live in `neutron-star`, but the crate owns no host storage — its
style accessors return the same computed values the stylo cascade produces,
so a stylo-backed host serves style views with no translation layer. Both
are first-class peers rather than translations into Flex or Grid. The
concrete adapter proved as mechanical as designed (`w3c-dom::layout`):
style views as direct `ComputedValues` field reads, per-node layout slots
on the host's nodes, and one display-mode dispatch — the same `Copy`-handle
shape the tree already implements for stylo's `TNode`/`TElement`.

## The protocol in one page

One tree trait (`neutron_star::tree`) plus style-view traits
(`neutron_star::style`). The host hands the engine **`Copy` node handles**
borrowed from its tree for the duration of one layout flush — a plain
`&'dom Node`, or a `(&'dom Tree, index)` pair. The trait carries no lifetime
parameter and no GATs; the concrete handle type carries the tree lifetime,
exactly like stylo's `TNode`/`TElement` (which w3c-dom already implements
directly on `&'a Node<T>`):

| Item | Provides | Consumed by |
| --- | --- | --- |
| `LayoutNode: Copy + Debug` | child iteration (`children`/`child_count`), the borrowed `Style` view, **`compute_child_layout` (the host display/algorithm dispatch point)**, unrounded/final layout and static-position writes, and per-node cache slots (all three cache methods required — a caching host cannot accidentally omit `cache_clear`; uncached hosts no-op all three) | everything |
| `CoreStyle` | the box-universal style view every algorithm reads | all algorithms |
| `FlexContainerStyle`/`FlexItemStyle` | flex views (bounds on `N::Style`) | the L1 flexbox algorithm |
| `GridContainerStyle`/`GridItemStyle` | grid views, including borrowed `&GridTemplateComponent`/`&ImplicitGridTracks` track-list accessors | the L2 grid algorithm |
| `RelativeContainerStyle`/`RelativeItemStyle` | relative views | the Starlight Relative L1 algorithm |
| `LinearContainerStyle`/`LinearItemStyle` | Starlight Linear views | the Linear algorithm |
| `TextContainerStyle: CoreStyle` | paragraph-level alignment, whitespace, word-break, and indent values | the Parley `TextMeasurer` |
| `TextRunStyle` | run-level font, spacing, line-height, family, feature, and variation views | the Parley `TextMeasurer` |

One `Style` associated type serves every algorithm: hosts implement the
container/item style traits once, on one view type, and each entry point
narrows `N::Style` with the bounds it actually needs. Everything the engine
reads through a handle is immutable for the flush; everything it writes goes
through the handle into host-owned **interior-mutable per-node slots**.

Entry points (`neutron_star::compute`) are free generic functions — there is
no engine object, so unused entry points never monomorphize into the host.
Implemented machinery: `compute_root_layout`, `compute_leaf_layout`
(generic `LeafMeasurer`), explicit hidden-subtree cleanup via
`hide_subtree`, `compute_cached_layout`
(keyed on the **complete `LayoutInput`** — see the caching section),
`compute_absolute_layout` (the positioned pass for out-of-flow nodes whose
containing block is not their formatting parent), and
`round_layout(root, scale)` (device-pixel snapping), plus
`compute_flexbox_layout`, `compute_grid_layout`, `compute_relative_layout`,
and `compute_linear_layout`. All four algorithms share private allocation-free
length, edge, box-sizing, aspect-ratio, clamp, and relative-offset machinery.
Their public entry points use the same fixed shape:

```rust
pub fn compute_flexbox_layout<N>(node: N, input: LayoutInput) -> LayoutOutput
where N: LayoutNode, N::Style: FlexContainerStyle + FlexItemStyle;

pub fn compute_grid_layout<N>(node: N, input: LayoutInput) -> LayoutOutput
where N: LayoutNode, N::Style: GridContainerStyle + GridItemStyle;

pub fn compute_relative_layout<N>(node: N, input: LayoutInput) -> LayoutOutput
where N: LayoutNode, N::Style: RelativeContainerStyle + RelativeItemStyle;

pub fn compute_linear_layout<N>(node: N, input: LayoutInput) -> LayoutOutput
where N: LayoutNode, N::Style: LinearContainerStyle + LinearItemStyle;
```

All four signatures are public; hosts select them in their display dispatch.

Layout IO is three `Copy` PODs: `LayoutInput` (layout goal, sizing mode,
known dimensions, whether those dimensions establish definite percentage
bases, parent size, and available space) → `LayoutOutput` (size, content size,
baselines) per call, and `Layout` (order, location, size, content size,
border/padding/margin) as the durable per-node result. The
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
styles through node handles, then calls
`child.compute_child_layout(input)`. The host's implementation
first handles a non-generated box (`display: none`, i.e. stylo
`Display::is_none`) by calling `hide_subtree` and
returning `LayoutOutput::HIDDEN`; this explicit cleanup precedes and bypasses
the cache. For a generated box, the host routes to a neutron-star algorithm,
leaf measurement, or a future additional host algorithm, wrapping that
routing in `compute_cached_layout`. This decision buys three properties at
once:

1. **Open dispatch with four first-class algorithms.** Flex, Grid, and Lynx's
   non-CSS `display: relative` (id-anchored sibling constraint solving) and
   `display: linear` (Android `LinearLayout` semantics:
   `linear-direction`/`linear-weight`/…) are implemented peers in
   `neutron-star`, against the same node-handle protocol. The `<list>`
   component's staggered-grid remains a future host peer. The engine owns
   **no display enum of its own** — `CoreStyle::display` returns stylo's
   `Display`, the engine consumes it only through `is_none`, and which
   *algorithm* a generated box uses stays the host's dispatch decision.
2. **Uniform caching.** Every generated-box path through dispatch shares one
   cache policy, so mixed-algorithm trees memoize correctly. Future
   host-provided modes can use the same wrapper. Hidden cleanup deliberately
   stays outside that cache boundary.
3. **Partial relayout.** Any node can be a layout root; the engine never
   assumes global tree access.

## Design decisions and their rationale

**No `dyn`, enforced structurally.** `LayoutNode` is dyn-incompatible (a
`Copy` supertrait plus associated types without defaults), and `LeafMeasurer`
carries a GAT: `dyn LayoutNode` and `dyn LeafMeasurer` are compile errors —
there is a `compile_fail` doctest pinning this. What the constraint buys:
every host⇄engine call site monomorphizes, inlines, and const-folds (style
accessors returning constants collapse into the algorithm); no vtable
indirection in the hottest recursion of the frame. The accepted costs:
compile time and per-host codegen (one copy of the algorithms per concrete
handle type — in practice one per binary), and no heterogeneous "list of
engines" (not a goal).

**The host owns all storage; handles are the stylo shape.** A node handle
reaches two kinds of per-node data. Immutable epoch data — topology, child
order, computed styles, leaf content — is read through
plain shared borrows, so a borrowed style view stays valid across recursive
child layout by construction (the protocol has no `&mut` anywhere). Mutable
results — unrounded/final layouts, static positions, cache slots,
measurement contexts, retained text artifacts — live in **host-owned
interior-mutable slots on the nodes** (`Cell`/`RefCell`; or
`AtomicRefCell`/`UnsafeCell` under the host's own discipline, exactly how
the tree already stores stylo's per-element style data). Layout is
single-threaded, and two rules keep runtime borrow tracking trivial: host
dispatch must not hold a per-node slot borrow across the recursive
`compute_child_layout` call, and the engine never re-enters a node's cache
while that node's leaf measurer is live. Per-node derived state therefore
lives *on the node* — no id-keyed side tables, no parallel source/session
arenas kept in lockstep.

A layout run observes one immutable **epoch**. Style, content, child order,
and handle validity cannot change during recursion; such mutations are
staged, invalidate the affected box and measurement caches, and start a new
epoch. Virtualized components therefore realize their visible topology
before layout or explicitly restart after realization.

**Style is read through views, in stylo's computed vocabulary.** Style traits
(`CoreStyle` + container/item traits per box algorithm,
`TextContainerStyle`, and the standalone `TextRunStyle`) hand out **stylo
computed values** per accessor call — the same `Display`,
`LengthPercentage`, `Margin`, `AlignFlags`-based alignment wrappers, grid
track lists, and keyword enums the stylo cascade produces, re-exported from
`neutron_star::style`. There are no engine-owned style value enums and no
materialized engine-side style structs: a stylo-backed host implements the
accessors as direct field reads of its `ComputedValues`, and a cascade-less
host (tests, benches) constructs the same stylo values by hand. Small `Copy`
values (keyword enums, alignment flags, `Au` border widths, numbers) are
returned owned; the `LengthPercentage`-family geometry properties — inset,
size, min/max size, margin, padding, flex-basis, gap, grid-line placements —
are returned **borrowed** as per-field references inside the geometry
wrappers (`Edges<&Margin>`, `Size<&StyleSize>`, `&FlexBasis`), and sequence
values — grid track lists (`&GridTemplateComponent`, `&ImplicitGridTracks`)
— are returned borrowed whole, so no read ever clones a `calc()` tree or
bumps a refcount. Per-field reference wrappers are lendable from any host
storage (a `ComputedValues` host keeps the four margin edges as separate
fields); a host that synthesizes style values per call must materialize them
in per-node storage once per style change and lend from there. Text-run
accessors (font families, features, variations) stay owned — they run once
per (re)shape, amortized by the measurement cache. Defaulted trait methods return the **fork's initial values**:
the CSS initial value except where Lynx documents otherwise
(`relative-layout-once: true` — the Lynx computed default *is* the fork
initial, so the trait default needs no adapter override). Alignment `normal`
likewise needs no host substitution anymore: the algorithms normalize
`normal`/`auto` `AlignFlags` at style-read time (flex `align-items: normal`
behaves as `stretch`). What remains host-side is *computed-value* policy —
Lynx defaults the style system resolves before layout runs:

| Property | Fork initial (trait default) | Lynx computed default (host supplies) |
| --- | --- | --- |
| `box-sizing` | `content-box` | `border-box` (Lynx computes `auto` → border-box) |
| `overflow` | `visible` | `hidden` |
| `position` | `static` | `relative` (≙ static — Lynx has no `static` keyword) |

**`calc()` rides the stylo values.** Percent-bearing `calc()` can only be
resolved during layout, and stylo's computed `LengthPercentage` carries it
and **resolves it itself** against the basis the algorithm supplies — so the
protocol needs no calc plumbing at all. The former opaque `CalcHandle(u64)`
and `LayoutNode::resolve_calc(handle, basis)` callback are deleted.
Length-only `calc()` folds to a length at computed-value time and resolves
without a basis (a documented behavior delta of the vocabulary swap).

**Leaf measurement is generic behavior with a borrowed result view.**
`LeafMeasurer` is a GAT-based, statically-dispatched interface whose
engine-specific `Measurement<'a>` implements the accessor-only
`LeafMeasurement` trait. `compute_leaf_layout` immediately normalizes that
view into the concrete `LeafMetrics` POD used by box math. The default-on
`text` module follows this shape: `TextLayout` retains an owned
`parley::Layout`, and its borrowed measurement view exposes size and first
baseline without cloning or reshaping. The host's leaf dispatch constructs a
node-scoped `TextMeasurer` by borrowing immutable text/style content through
the handle and mutable `TextContext`/artifact slots from its interior-mutable
storage (borrows that end before the cache wrapper stores the result).
Different leaf dispatch arms may instantiate `compute_leaf_layout` with
different concrete measurer types
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
What varies is where the containing block is, read per node from stylo's
computed `PositionProperty` — the engine bakes the Lynx containing-block
policy (`static`/`relative`/`sticky` lay out in flow; `relative` gets the
definite-inset visual nudge):

- `PositionProperty::Absolute` — CB **is** the layout parent. The parent's
  algorithm sizes/places the node fully (insets/percentages against its
  padding box; auto insets fall back to the static position it just
  computed). This is the only case Lynx `position: absolute` produces:
  every Lynx element is positioned, so the nearest positioned ancestor is
  always the parent.
- `PositionProperty::Fixed` — the hoisted case: CB is **not** the parent
  (a host whose `absolute` nodes can escape non-positioned ancestors —
  impossible in Lynx — lowers them to the same hoisted handling). The
  parent's algorithm computes the node's flex/grid-aware static position
  and records it via `LayoutNode::set_static_position`, but does not size
  or place it. After in-flow layout the host runs the **positioned pass**:
  it resolves the CB node (for Lynx `fixed`: the viewport root, or the
  nearest transformed/filtered/`will-change` ancestor per the W3C rule the
  tracking doc mandates), converts the recorded static position into CB
  padding-box space (all unrounded layouts exist by then), and calls
  `compute_absolute_layout(node, cb_padding_box_size,
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
`round_layout(root, scale)` derives snapped finals through the handles'
`unrounded_layout`/`set_final_layout` (whose impl may target a different
store, e.g. the paint-facing side of a widget tree). `scale` is the
device-pixel ratio (physical px per CSS px):
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
  (`children`, style accessors, `compute_child_layout`) are all
  monomorphized; hosts should `#[inline]` their impls of the first two.
- **POD boundary.** Geometry and IO types are small `Copy` structs
  (`#[repr(C)]`), passed by value in registers; `f32` throughout (GPU/SIMD
  native, halves cache traffic vs `f64`; Starlight/Yoga/Taffy all agree).
  No `NaN` sentinel games — unknowns are `Option<f32>`/enum variants, and
  boundary values must be finite (debug-asserted).
- **Shared setup, flat hot scratch.** Flex, Grid, Relative, and Linear reuse
  the same
  inline, statically-dispatched ordering and box-resolution helpers. Their
  temporary
  `ResolvedItemBox`/`ResolvedContainerBox` PODs eliminate duplicate sizing
  rules at the algorithm boundary, then each algorithm destructures item
  values into its own flat scratch record. Inner loops therefore retain
  direct field access without a shared trait, `dyn`, or nested algorithm
  state. Relative additionally resolves every id reference once into a compact
  ordered-item index; its positioning passes never hash or binary-search ids.
  The item resolver is force-inlined because release-IR inspection
  showed that ordinary inlining materialized a 216-byte return temporary;
  forced inlining lets scalar replacement remove that copy chain.
- **The measurement cache is the asymptotic mechanism.** Flex, Grid, two-pass
  Relative, and Linear sizing probe children under multiple constraints;
  uncached,
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
- **Parallelism: designed, deferred, additive.** Immutable epoch data is
  naturally shareable, but layout recursion is inherently sequential and the
  per-node slots assume single-threaded interior mutability — correct for v0
  (layout is rarely the bottleneck vs paint/style, and Yoga/Taffy/Starlight
  are all sequential). The planned extension is a **batched child-layout
  hook**: a defaulted `LayoutNode` method like
  `compute_child_layouts(self, requests)`
  that algorithms call at fan-out points (independent flex-item measure
  probes, grid item contributions); the default body is today's sequential
  loop, and a parallel host overrides it to shard sub-trees across its own
  pool with thread-safe slots (host storage, host threading policy — the
  engine stays thread-unaware). Adding a defaulted method is semver-minor,
  so this ships when profiles earn it, without a protocol break.
- **Flex, Grid, Relative, and Linear benchmarks are landed; broader
  performance hardening remains.** The `divan` (CodSpeed-compatible) suite
  measures engine-native workloads through neutron-star's public host
  protocol. The Flex suite covers deep and wide trees, wrapping, weighted
  distribution, measurement, nested containers, and positioned children.
  The Grid suite covers scaled
  sparse/dense auto-placement, fixed/`fr` tracks, unique intrinsic span
  buckets, flex freeze thresholds, cold/warm nested grids, a root cache hit,
  and dirty-leaf ancestor invalidation. The Relative suite covers independent
  items, reverse dependency chains, duplicate ids, adversarial disjoint
  cycles, one-pass versus two-pass solving, nested cold layout, warm
  descendants, root cache hits, and auto-width refinement. The refinement
  path preserves measurements for unchanged, definite fixed items while
  percentage-dependent or newly double-anchored items remeasure. The Linear
  suite covers fixed stacks,
  weighted distribution and freeze paths, ordering, gravity matrices,
  measurement/stretch, and mixed hidden/absolute children. Equivalent-tree
  Taffy/Yoga and other cross-engine differential baselines remain future
  additions — not to copy those engines' designs, but to keep
  "high-performance" falsifiable.

## Algorithms (Flex, Grid, Relative, and Linear implemented)

This pass structure documents the implemented L1 Flex and L2 Grid algorithms
plus the implemented Relative L1 and first-class Linear algorithms.
Starlight's C++ mirrors the same spec steps
(`flex_layout_algorithm.h` literally cites "Algorithm-3"…"Algorithm-15";
`grid_layout_algorithm.h` uses the spec's track-sizing terms verbatim), so
following the spec text directly also keeps us structurally comparable to
the engine we're succeeding.

**Flexbox (L1)** — CSS Flexbox Level 1 §9, as passes:

1. **Setup** — collect in-flow children (skip non-generated
   `display: none` boxes), stable-sort by style `order`, resolve container
   axes from `flex_direction` × `direction` (rtl flips row axes).
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
   §9.8 (sole-item alignment). `PositionProperty::Absolute` children are
   then sized/placed against the container's padding box from their insets
   (auto insets anchor to the static position); hoisted
   `PositionProperty::Fixed` children only get the static position recorded
   via `set_static_position` — the host's positioned pass finishes them.
9. **Finalize** — per-child `set_unrounded_layout` (only for
   `LayoutGoal::Commit`), container border-box size, `content_size`
   accumulation, container baseline.

The automatic minimum size (§4.5, `min-size: auto`) resolves inside steps
2/4, honoring stylo's `Overflow::is_scrollable`.

**Linear** — Starlight linear layout, as a first-class single-axis pipeline in
`crates/neutron-star`:

1. **Setup** — resolve the container box and axes, classify hidden,
   out-of-flow, and in-flow children, and stably apply non-zero `order`.
2. **Item sizing** — resolve each item's box model, preferred/min/max sizes,
   aspect ratio, intrinsic constraints, effective cross gravity, and auto
   margins through neutron-star's shared private box arithmetic.
3. **Weight distribution** — when the incoming main-axis constraint has a
   decided size, distribute remaining space among positive `linear-weight`
   items using the explicit positive `linear-weight-sum` denominator when
   present, with iterative min/max freezing and final child relayout. This is
   Starlight constraint-mode definiteness, intentionally distinct from
   `LayoutInput::definite_dimensions`: a Flex target may activate Linear
   weights/stretch while remaining indefinite as a descendant percentage
   basis under Flexbox §9.8.
4. **Container sizing** — use the definite content size or the final sum/max
   of item outer sizes, then apply padding, border, aspect ratio, and min/max
   clamps. Once an intrinsic inline size is known, a targeted pass re-resolves
   percentage-dependent box used values against the provisional content size
   before the container's own min/max clamp, but does not remeasure children
   or feed the new values back into the container/main total; this preserves
   Starlight's measured-once `UpdateContainerSize` behavior and call order.
   Dependency flags are Linear-owned: the shared box resolver returns only
   resolved values, and compact item-local bits selectively refresh margin or
   padding/border fields that depend on the newly available inline basis.
   Preferred and min/max sizes remain measured-once; after item sizing they
   have no downstream consumer, so re-resolving them would be a dead update.
5. **Alignment** — derive main-axis gravity from `justify-content` (the
   legacy `linear-gravity` channel) and cross-axis gravity from
   `align-self`/`align-items` (the legacy layout/cross-gravity channels;
   `fill-*` semantics ride `stretch`), including RTL/reverse axes and
   cross-axis auto margins.
6. **Commit and baseline** — lay out in-flow children with their final known
   dimensions, apply relative insets, store parent-relative layouts, and
   export horizontal/vertical-container baselines.
7. **Out-of-flow children** — derive linear-aware static positions, lay out
   parent-contained absolute children against the padding box, and record
   hoisted static positions for the later host fixed-position pass.
8. **Measure-only path** — return sizes and baselines without durable child
   writes, retaining the same `LayoutInput`/cache semantics as flexbox.

Like Flex and Grid, this is a generic neutron-star algorithm over
`LayoutNode` handles with Linear style-view bounds speaking stylo computed
values; it is not yet wired to `WidgetTree`.

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
   horizontal/vertical orders, selectively remeasure tightened one- or double-sided
   constraints, resolve wrap width and cyclic percentages, then resolve height
   and recompute final positions against both final content extents.
5. **Finalize** — commit border-box locations from the content origin plus
   margin-edge positions, apply CSS relative inset offsets visually, and use
   the common absolute/hoisted passes. Relative containers export no baseline.

## Testing strategy

- **L0/L1/L2/L2R (landed):** `tests/protocol.rs` — a complete mock host
  implementing the `LayoutNode` handle protocol over one tree with
  interior-mutable per-node slots; proves the protocol is implementable
  without `dyn` (plus a `compile_fail` doctest pinning the barrier), stores
  and serves stylo computed values directly (no calc callback — stylo
  `LengthPercentage` self-resolves), and exercises the borrowed track-list
  accessors and all shared machinery entry points.
  `tests/support` is the shared real-protocol host for
  Flex, Linear, Relative, and cross-algorithm Grid coverage; Grid additionally
  keeps a local host whose borrowed track lists are real stylo
  `GridTemplateComponent` values, including `repeat()` groups.
  `tests/flexbox.rs` covers grow/shrink/freeze, basis and percentages, wrapping
  and gaps, axes and alignment, auto margins, measurement,
  baselines, and absolute/hoisted positioning. Leaf
  unit tests additionally cover non-`Clone` borrowed GAT results, separate
  probe/commit artifacts, committed box-cache hits, and coordinated
  invalidation. `tests/grid.rs` covers numeric placement, sparse/dense and
  row/column auto-flow, implicit/automatic tracks, intrinsic spanning
  contributions, fixed/intrinsic/`fr`/fit-content/minmax tracks, spans,
  alignment, RTL, baselines, measurement, nested layout, visibility, and
  absolute/hoisted behavior.
  Private unit tests pin placement bit ranges, clamping, repeat expansion, and
  track cycling. `tests/relative.rs` covers every physical reference family,
  duplicate/reserved ids, both solver modes, cycles, intrinsic and percentage
  sizing, parent min/max feedback, selective wrap-width remeasurement,
  measurement, visibility, nested layout, and absolute/hoisted behavior.
  `tests/linear.rs` covers orientation and gravity, weight/sum/freeze, order
  and visibility, intrinsic/minmax sizing, measurement, baselines, auto
  margins, absolute/hoisted behavior, and Flex/Grid composition.
  CI enforces at least 95% line coverage for `neutron-star`
  production source while excluding test and benchmark source from the
  metric.
- **Behavior/performance hardening:** each algorithm has an engine-native
  behavior suite and a CodSpeed-compatible benchmark target. Tests use exact
  geometry, used-edge, baseline, measurement-trace, static-position, layout
  order, or cache-result oracles; repository-text inventories and
  source-migration cardinality guards are intentionally excluded. Browser
  geometry goldens and differential fuzzing against Taffy on the shared
  feature subset remain planned.
- **Positioning boundary:** engine tests cover hoisted
  `PositionProperty::Fixed`
  static-position export and the common `compute_absolute_layout` completion
  pass. CSS Fixed root lowering, Sticky/list/component metadata, and anonymous
  text-item generation remain host/integration responsibilities and are not
  neutron-star behavior contracts.
- **Remaining Lynx integration:** Widget/stylo adapter wiring for Relative
  and Linear, component-specific staggered layout, and mixed-runtime parity remain
  future work; the integration layer's final module or crate placement has not
  been established.

## Milestones

- **L0 — contracts + skeleton** *(complete)*: traits, value types, IO,
  cache semantics, machinery entry-point contracts, and conformance mock.
- **L1 — flexbox** *(complete)*: `compute_flexbox_layout` per the plan above,
  plus the shared machinery it exercises: leaf boxing, hidden-subtree
  cleanup, cache matching policy, root entry, the positioned pass
  (`compute_absolute_layout`), and device-pixel rounding. Engine-native Flex
  fixtures and benchmarks are landed; browser goldens and differential
  fuzzing remain parity/performance hardening.
- **L2 — grid** *(complete)*: `compute_grid_layout`, `auto-fill`/`auto-fit`,
  dense packing, first-baseline alignment, and direct-child
  grid-area-relative absolute positioning.
- **L2R — Starlight relative** *(complete)*: the relative style protocol,
  the one-pass combined and two-pass per-axis dependency solvers,
  intrinsic/percentage remeasurement, deterministic cycles, out-of-flow
  handling, engine-native conformance fixtures, and CodSpeed benchmarks.
- **L3 — Starlight modes + runtime integration** *(partial)*: the Lynx-linear
  value and style-view protocol, generic `compute_linear_layout` algorithm, and
  feature-gated Parley text measurement core are complete in `neutron-star`.
  Remaining L3 work is the concrete
  `lynx-widget`/stylo adapter — a
  `LayoutNode` impl with interior-mutable per-node layout/cache slots and
  display dispatch, dirty→cache invalidation wiring, the root
  fixed-position pass and sticky lowering, style-view wiring over
  `ComputedValues` (plus host lowering of legacy spellings:
  `linear-orientation` into `linear-direction`, logical `relative-inline-*`
  to physical sides), text computed-style/attribute wiring, and text-context
  and artifact-slot storage wiring. The integration layer's final module or crate
  placement remains undecided; no separate text crate is planned.
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
- Crate name availability on crates.io (`neutron-star`) — moot while the
  crate depends on the vendored stylo fork (not publishable as-is); recheck
  only if a publishable stylo dependency ever materializes. The protocol
  doesn't depend on the name.
