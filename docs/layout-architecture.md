# Layout architecture — `neutron-star`

`crates/neutron-star` is lynx-vello's box-layout engine: the from-scratch
successor to the Lynx C++ engine's `starlight`
(`lynx/core/renderer/starlight/`). It implements CSS **flexbox**, CSS **Grid**,
and Lynx's Starlight **Relative** and **Linear** layouts as first-class peer
algorithms. It is host/storage-agnostic and standalone-publishable — zero
dependencies for the protocol and box-layout core when built with
`default-features = false`, no assumption about DOM, style engine, or storage
— and every host boundary is **static dispatch**: `dyn` is impossible by
construction, not by convention. Default builds enable the optional `text`
feature and its Parley-backed measurement core.

Status: **Flexbox, Grid, Relative, Linear, text measurement, and DOM layout
integration implemented** —
`neutron-star`'s protocol, generic machinery, cache, leaf and positioned
sizing, rounding, CSS Flexbox Level 1, numeric CSS Grid Level 2, Starlight
Relative Layout Level 1, Starlight Linear algorithms, and the default-on
Parley text measurement core are implemented and conformance-tested against
plain-storage mock hosts. `stylo_dom::layout::DomLayoutSource` borrows the
styled DOM `Arena` directly and supplies the immutable formatting projection
and computed-style views; `stylo_dom::layout::DomLayoutSession` owns the
separate mutable box caches, layouts, Parley context/artifacts, font
registration, display dispatch, and result queries. Real DOM Text nodes form
measured anonymous items rather than fake Elements. Flexbox and Grid use the W3C
anonymous-item rules. Admitting the same generated text leaf to Linear and
Relative is an explicit lynx-vello extension requested for this integration,
not a W3C rule or a native-Lynx parity claim. Grid excludes subgrid and named
lines/areas, which are outside the current protocol. Text truncation, a general
inline formatting context, standards-compliant CSS Block Flow, fine-grained
dirty-subtree invalidation, Lynx PAPI projection, and the root fixed/sticky
pass are not implemented yet. The DOM adapter also does not yet discover and
hoist `position:absolute` boxes across static ancestors to their W3C
containing block. `DomLayoutSession` currently routes
`DomLayoutDisplay::Flow` to a legacy Linear fallback; this is not CSS Flow
conformance. Crate
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
        lynx-vello host stack                           standalone
┌──────────────────────────────────┐     ┌─────────────────────────────────┐
│ stylo-dom: Node/Arena + styles   │     │ neutron-star                    │
│ DomLayoutSource formatting view │     │ tree/style/text protocols       │
│ DomLayoutSession + output query  │────▶│ flex / grid / relative / linear │
│ future PAPI/fixed/dirty work     │     │ text feature: Parley measure    │
│                                  │     │ no host storage, DOM, or stylo  │
└──────────────────────────────────┘     └─────────────────────────────────┘
```

| Layer | Owns | Must not own |
| --- | --- | --- |
| `neutron-star` | Implemented Flex, Grid, Relative, and Linear algorithms; their generic value/style/source protocols (including `relative-*` and `linear-*`); the parley-free text style/run protocol; leaf boxing, hidden-subtree cleanup, positioned layout, rounding; shared private arithmetic; geometry and layout IO; cache semantics | Node/style/content storage, display dispatch, DOM/widget/stylo types, resolved device-unit policy (`rpx`, etc.), stacking/paint order |
| `neutron-star::text` (`text` feature, default-on) | Parley context/font registration, whitespace processing, shaping, line breaking, intrinsic and height-for-width measurement, baselines, and retained `TextLayout` artifact types | Text truncation and ellipsis, inline boxes, paint styling, stylo/widget translation, resource fetching, or host cache/session storage |
| `stylo-dom` | Real `Node<T> = Element | Text` storage; the immutable `DomLayoutSource` that borrows an `Arena`, owns dense formatting metadata and computed-style Arcs, generates anonymous text items, maps DOM ids to source-local layout ids, and translates Stylo values (including `calc()`) into Core/Flex/Grid/Relative/Linear/text protocol views; the mutable `DomLayoutSession` with box caches, rounded layouts, Parley artifacts/font registration, display dispatch, epoch reconciliation, and Element/Text output queries | Lynx tag/PAPI/device policy, algorithm implementations |
| `lynx-widget` | Lynx PAPI/UA/device policy | The currently deferred PAPI-to-layout projection, a second layout implementation, or ownership of DOM layout state |
| Remaining runtime integration | Lynx PAPI projection, fine-grained dirty→ancestor cache invalidation, root fixed-position and sticky passes, replaced-content measurement, component-specific `staggered` integration | A second Flex/Grid/Relative/Linear/text-measurement implementation, engine-side copies of styles |

The engine/host seam is exactly the seam that makes the crate publishable.
The Lynx-specific values and algorithms for Relative and Linear live in
`neutron-star`, but the crate still owns no host storage or style-engine
representation. Both are first-class peers rather than translations into
Flex or Grid. The concrete source and runtime preserve the same source/session
split:
`DomLayoutSource` directly borrows immutable DOM topology and Text data for one
epoch while owning only lightweight formatting metadata and strong
computed-style references; `DomLayoutSession` owns every mutable cache, layout,
and retained text artifact. Both halves live in `stylo-dom`, while
`neutron-star` remains generic and storage-agnostic.

### DOM nodes and formatting nodes

`stylo-dom` is the layout object's source of truth. `Arena<T>` stores a real
`Node<T>` enum: Element nodes own the embedder payload, tag, attributes, and
Stylo `ElementData`; Text nodes own only character data and tree linkage.
`NodeRef` and Stylo's `TNode` therefore report standard Element/Text semantics,
and Text has no tag, attributes, external payload, or independently computed
style.

`DomLayoutSource<'arena, T>` borrows that arena after style flush. It builds
source-local dense formatting metadata for actual Element boxes and generated
anonymous text items, retaining the
computed-style Arcs needed by lazy neutron-star protocol views. DOM ids and
dense layout `NodeId`s are mapped explicitly because anonymous nodes share the
layout id space but have no backing Element.

For Flexbox and Grid, consecutive Text children (including Text promoted
through `display: contents` or a neutral transparent host role) become one
anonymous item; a wholly collapsible whitespace sequence generates no item.
The anonymous item receives CSS-initial box/item values, so parent flex/grid
item properties do not leak into it, while its paragraph and shaping runs use
the applicable inherited styles from surrounding Elements. This is the W3C
anonymous-item model. The same Parley-measured leaf is accepted by Linear and
Relative containers only because the user explicitly requested Text
participation in all four algorithms. Linear and Relative have no W3C rule for
this, and native Lynx ignores virtual raw-text on those non-text paths, so this
choice is documented as a project extension rather than compatibility parity.

The generic `LayoutNodePolicy` extension point can classify embedder Elements
as transparent `Contents`/`TextCarrier` nodes or `Replaced` leaves without
teaching `stylo-dom` any host vocabulary. The current work exercises the HTML
DOM subset itself; `lynx-widget` does not yet map PAPI tags into those roles.

The projection classifies ordinary block/inline values as
`DomLayoutDisplay::Flow` without pretending to implement their formatting
algorithm. Until a CSS Block/Inline engine lands, `DomLayoutSession` routes
that category through a legacy Linear fallback. This preserves a usable host
fallback but is not W3C Flow conformance.

Each Arena receives a process-unique monotonic layout identity, and each source
also records its conservative `layout_revision`. `DomLayoutSession` clears
retained node state when the arena identity, root, or revision changes; an
unchanged epoch can reuse caches and artifacts without aliasing a later Arena
that happens to reuse the same allocation address. After a commit,
`final_layout` returns rounded box output for a real Element or Text node, and
`committed_text_layout` returns the retained paragraph for a Text node.
Consecutive contributors resolve to their shared anonymous item. This keeps
the immutable borrowed source physically separate from all mutable layout and
measurement state.

The public session surface is deliberately small:

| API | Result |
| --- | --- |
| `DomLayoutSession::<T>::new()` / `without_system_fonts()` | Create retained state with the system font collection or an empty deterministic collection. |
| `register_fonts(bytes)` | Register every readable face and, on success, invalidate cached measurements, retained artifacts, and result queries until the next commit. |
| `commit(source, available_space, device_pixel_ratio)` | Reconcile the source epoch, run root layout, device-pixel-round it, and return the root `Layout`. |
| `final_layout(source, node_id)` | Return the final rounded `Layout` for a real Element or contributing Text node in the last committed epoch. |
| `committed_text_layout(source, node_id)` | Return the retained Parley `TextLayout` for a contributing Text node; non-text and omitted-whitespace nodes return `None`. |
| `formatting_layout(source, layout_node_id)` | Return rounded output for an actual or generated formatting node, including anonymous text items. |
| `formatting_text_layout(source, layout_node_id)` | Return the retained paragraph artifact for an anonymous text formatting node. |

## The protocol in one page

Traits (`neutron_star::tree` and `neutron_star::style`), layered by capability
so each entry point demands only what it uses:

| Trait | Adds | Consumed by |
| --- | --- | --- |
| `TraverseTree` | immutable child iteration (GAT iterator) | everything |
| `LayoutSource: TraverseTree` | immutable `CoreStyle` views and `resolve_calc` | all algorithms |
| `FlexSource: LayoutSource` | immutable flex container/item style views | the L1 flexbox algorithm |
| `GridSource: LayoutSource` | immutable grid container/item views, including GAT track-list iterators | the L2 grid algorithm |
| `RelativeSource: LayoutSource` | immutable relative container/item style views | the Starlight Relative L1 algorithm |
| `LinearSource: LayoutSource` | immutable Starlight Linear container/item style views | the Linear algorithm |
| `TextContainerStyle: CoreStyle` | paragraph-level alignment, inherited fallback whitespace/word-break, and indent values | the Parley `TextMeasurer` |
| `TextRunStyle` | run-level font, spacing, line-height, family, feature, variation, and optional computed whitespace/word-break overrides | the Parley `TextMeasurer` |
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
`compute_flexbox_layout`, `compute_grid_layout`, `compute_relative_layout`,
and `compute_linear_layout`. All four algorithms share private allocation-free
length, edge, box-sizing, aspect-ratio, clamp, and relative-offset machinery.
Their public entry points use the same fixed shape:

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

pub fn compute_linear_layout<Source, Session>(
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
leaf measurement, or a future additional host algorithm, wrapping that
routing in `compute_cached_layout`. This decision buys three properties at
once:

1. **Open dispatch with four first-class algorithms.** Flex, Grid, and Lynx's
   non-CSS `display: relative` (id-anchored sibling constraint solving) and
   `display: linear` (Android `LinearLayout` semantics:
   `linear-weight`/`linear-gravity`/…) are implemented peers in
   `neutron-star`, against the same source and session protocol. The `<list>`
   component's staggered-grid remains a future host peer. The engine has **no
   `Display`
   enum** — dispatch identity belongs to the concrete host adapter
   (`DomLayoutSession` for the current DOM integration).
2. **Uniform caching.** Every generated-box path through dispatch shares one
   cache policy, so mixed-algorithm trees memoize correctly. Future
   host-provided modes can use the same wrapper. Hidden cleanup deliberately
   stays outside that cache boundary.
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
(`CoreStyle` + container/item traits per box algorithm,
`TextContainerStyle`, and the standalone `TextRunStyle`) hand out small
`Copy` values (`Dimension`, `LengthPercentage`, alignment enums, grid track
types, and text values) per accessor call — lazy translation from stylo's
`ComputedValues`, no materialized engine-side style structs. Grid track lists,
font families, font features, and font variations stay borrowed GAT iterators,
so sequence-valued styles also cross the boundary without allocation.
Core/Flex/Grid/text trait methods default to **CSS initial values**; Linear
trait methods use Starlight Linear's documented initial values, and Relative
methods use the standalone Relative Level 1 initial values. Other Lynx
compatibility defaults are computed-value policy and stay in the host's style
system:

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
view into the concrete `LeafMetrics` POD used by box math. The default-on
`text` module follows this shape: `TextLayout` retains an owned
`parley::Layout`, and its borrowed measurement view exposes size and first
baseline without cloning or reshaping. The host's leaf dispatch constructs a
node-scoped `TextMeasurer` by borrowing immutable text/style content from the
source and mutable `TextContext`/artifact slots from the session. Different
leaf dispatch arms may instantiate `compute_leaf_layout` with different
concrete measurer types
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

Like Flex and Grid, this is a generic neutron-star algorithm over
`LinearSource` and `LayoutSession<Source>`. The `stylo-dom` computed-style
views and `DomLayoutSession` dispatch wire it to the DOM without giving
neutron-star a DOM dependency.

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

- **L0/L1/L2/L2R (landed):** `tests/protocol.rs` — a complete mock host implementing
  every trait over physically separate immutable-source and mutable-session
  `Vec` storage; proves the protocol is implementable
  without `dyn` (plus `compile_fail` doctests pinning both the GAT and explicit
  `Sized` barriers), exercises the GAT track-list machinery and all shared
  machinery entry points. `tests/support` is the shared real-protocol host for
  Flex, Linear, Relative, and cross-algorithm Grid coverage; Grid additionally
  keeps a local borrowed-repetition host for its track-list GAT cases.
  `tests/flexbox.rs` covers grow/shrink/freeze, basis and percentages, wrapping
  and gaps, axes and alignment, auto margins, collapse struts, measurement,
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
- **Positioning boundary:** engine tests cover `Position::AbsoluteHoisted`
  static-position export and the common `compute_absolute_layout` completion
  pass. CSS Fixed root lowering, Sticky/list/component metadata, and anonymous
  text-item generation remain host/integration responsibilities and are not
  neutron-star behavior contracts.
- **DOM integration:** `stylo-dom` tests cover real Text nodes, anonymous-item
  grouping and initial item values, inherited per-run text styles, all four
  dispatch paths, retained text output, and stale-epoch rejection. Lynx PAPI
  projection, component-specific staggered layout, and mixed-runtime parity
  remain future work.

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
- **L2R — Starlight relative** *(complete)*: `RelativeSource`, the one-pass
  combined and two-pass per-axis dependency solvers, intrinsic/percentage
  remeasurement, deterministic cycles, out-of-flow handling, engine-native
  conformance fixtures, and CodSpeed benchmarks.
- **L3 — Starlight modes + runtime integration** *(partial)*: the Lynx-linear
  value/style/source protocol, generic `compute_linear_layout` algorithm, and
  feature-gated Parley text measurement core are complete in `neutron-star`.
  The concrete `stylo-dom` formatting source and computed-style adapter
  (including `CalcHandle` translation), true DOM Text nodes, anonymous text
  items, mutable `DomLayoutSession`/display dispatch, conservative
  whole-revision cache invalidation, Element/Text output queries, and
  text-context/artifact-slot wiring are also complete. Remaining L3 work is
  Lynx PAPI projection, fine-grained dirty→ancestor cache invalidation, the root
  fixed-position pass and sticky lowering, standards-compliant CSS
  Block/Inline Flow, W3C absolute-position containing-block discovery/hoisting,
  replaced/inline content, and component-specific staggered layout; no
  separate text crate is planned.
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
- Vendored Stylo currently has no computed longhands for `linear-gravity`,
  `linear-cross-gravity`, or `linear-layout-gravity`; the adapter correctly
  falls back to standard alignment properties until that parser/computed-style
  surface is added.
- The Lynx-enabled Stylo grammar intentionally rejects author-facing
  `display: contents`. `DomLayoutSource` nevertheless implements contents
  flattening at the computed-value boundary, while the generic `Contents` role
  can request the same formatting projection without teaching `stylo-dom` a
  host tag name. Mapping Lynx `<wrapper>` or other PAPI nodes to that role is
  deferred. Anonymous text-item construction is implemented; exposing the
  `display: contents` keyword to authors remains a parser-surface decision.
- Crate name availability on crates.io (`neutron-star`) — check before the
  first publish; the protocol doesn't depend on the name.
