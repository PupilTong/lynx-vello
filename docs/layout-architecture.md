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
impossible by construction, not by convention. Parley is unconditional:
decoded natural size and concrete Parley text are the only leaf-content paths.

Status: **Flexbox, Grid, Relative, Linear, and text measurement implemented** —
`neutron-star`'s protocol, generic machinery, cache, leaf and positioned
sizing, rounding, CSS Flexbox Level 1, numeric CSS Grid Level 2, Starlight
Relative Layout Level 1, Starlight Linear algorithms, and the
Parley text measurement core are implemented and conformance-tested against
plain tree/state mock hosts. **CSS containment (css-contain-2)** is landed on
the layout side: size/layout containment, `content-visibility` skipped
contents, the relayout-boundary predicate, and containment-bounded cache
invalidation (`invalidate_for_relayout`) — its `w3c-dom` damage producer,
containment folding, and the damage→layout seam all ship alongside: every
style flush consumes harvested relayout-class `StyleDamage` into
`Document::invalidate_layout` automatically, boundary-stopped, entirely inside
the engine layer (no runtime adapter participates). Grid excludes subgrid
and named lines/areas, which are outside the current protocol. The concrete
document/stylo host is
implemented in `w3c-dom`'s `layout` module (`Document::layout`):
`LayoutTree` on immutable `TreeArenas`, with plain `NodeId`s and a separately
borrowed mutable `DocumentLayoutState`. Style views are fetched on engine
request and lend the post-flush `ComputedValues` pointer published under
`Document`'s exclusive flush/layout phase boundary (including the
effective-containment fold that makes a `contain: strict` box a relayout
boundary). They do not enter Stylo's `ElementData` borrow checker or bump the
style `Arc`; layout/text state likewise needs no runtime borrow checking.
Host display dispatch includes `content-visibility: hidden`
skipped-contents routing and the content-visibility-implied fixed/absolute
containing block. Each NodeId-indexed state entry owns one `LayoutSlot` with
the measurement cache, persistent static position, and durable rounded/
unrounded results; `Document` exposes their query APIs. The host also owns the
fixed/hoisted positioned pass (pruned at skipped
subtrees), device-pixel rounding, and automatic
style-damage→`invalidate_layout` consumption with in-place boundary re-layout
that refreshes the boundary's scrollable `content_size`, with
replaced leaves reading their node-owned `NaturalSize`, plus W3C text nodes
using a dedicated text-only inherited-style view, a lazily-created
boxed `TextContext` in `DocumentLayoutState`, and per-node lazily boxed
retained artifacts in that same state. Keeping
the text-only view separate leaves the box-algorithm style view at two words.
Literal text and natural-size metadata reuse the node's existing nullable
content pointer, while retained Parley artifacts stay phase-local in layout
state. Updating replaced metadata automatically invalidates the affected cache
path; it is not exposed through the public DOM or any Element PAPI.
Text truncation, inline boxes, element-backed raw text, and
Lynx-specific text attribute policy are not implemented yet. Crate
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
│ w3c-dom                         │     │ neutron-star                    │
│ immutable TreeArenas + styles    │     │ one LayoutTree protocol         │
│ mutable DocumentLayoutState      │────▶│ flex / grid / relative / linear │
│ boxed text context/artifacts     │     │ concrete Parley measurement     │
│ NodeId dispatch + fixed/dirty    │     │ LayoutSlot shape, no storage    │
│ future staggered integration     │     │ leaf/hidden/cache/position/round│
└──────────────────────────────────┘     └─────────────────────────────────┘
```

| Layer | Owns | Must not own |
| --- | --- | --- |
| `neutron-star` | Implemented Flex, Grid, Relative, and Linear algorithms; one unified source-backed `CoreStyle` protocol speaking stylo computed values (including the `relative-*` and `linear-*` longhands); the text style/run protocol; closed natural-size and Parley leaf paths, hidden-subtree cleanup, positioned layout, rounding; shared private arithmetic; geometry and layout IO; cache semantics | Node/style/content storage, display dispatch, arbitrary host content/measurers, DOM/runtime types, an engine-side style value vocabulary (it re-exports stylo's), resolved device-unit policy (`rpx`, etc.), stacking/paint order |
| `neutron-star::text` (unconditional) | Parley context/font registration, whitespace processing, shaping, line breaking, intrinsic and height-for-width measurement, baselines, and retained `TextLayout` artifact types | Text truncation and ellipsis, inline boxes, paint styling, runtime/attribute lowering, resource fetching, or host cache and per-node slot storage |
| `w3c-dom::layout` (implemented) | `LayoutTree` on immutable `TreeArenas<T>`, plain `NodeId`s, and separately borrowed mutable `DocumentLayoutState` (one protocol; no view/session/store wrapper layers); post-flush style views lending the `ComputedValues` pointer published from Stylo's still-owning primary `Arc` under the exclusive `Document` phase boundary (no `ElementData` borrow check, `Arc` bump, copy, or translation; public computed-style queries remain guarded); logical `relative-*-inline-*` lowering; the W3C fixed/absolute containing-block rule expressed through `position()`; anonymous box geometry plus inherited parent font/text values for text nodes; display dispatch (flex/grid/linear/relative, `display: none` hiding, `content-visibility: hidden` skipped-contents routing before the cache, natural-size leaf, concrete Parley text); lazily boxed shared `TextContext` and per-text-node `TextLayoutStore` in layout state; a NodeId-aligned `LayoutSlot` containing cache, static position, unrounded layout, and rounded layout; `Document` query methods for stored results; automatic dirty-path invalidation when content changes; one fused preorder positioned-and-rounding traversal whose pre-node hook keeps hoisted placement cache-proof, prunes positioning at skipped-contents subtrees so a hoisted descendant cannot be revived, and applies the engine's effective-`order`-0 paint rule for out-of-flow children; device-pixel rounding without a whole-`Layout` clone; the effective-containment fold on the style view (feeding both the relayout-boundary predicate and the content-visibility-aware fixed/absolute containing-block predicate); **automatic style-damage consumption** (every harvest boundary-stops `Document::invalidate_layout` per relayout-damaged node before returning/streaming damage; it also evicts direct text children's measurement caches and retained artifacts because those children read inherited style from the damaged element but have no Stylo damage record of their own; `Document::layout` re-runs each parked `contain: strict`/skipped boundary in place before the root pass, merging the re-run's scrollable `content_size` back into the boundary's stored layout); and the `Document::invalidate_layout` API embedders still call for mutations the style system cannot see | A second layout algorithm, generic content-measurement callbacks, engine-side style copies, layout/text runtime borrow wrappers, Lynx runtime-element vocabulary or device-unit policy (`rpx`), Lynx computed defaults (cascade/UA-sheet policy), text shaping algorithms |
| Future runtime integration | Lynx view metrics and `rpx` policy; Lynx-specific text attributes, element-backed raw text and truncation; `staggered` integration; sticky lowering | A second Flex/Grid/Relative/Linear/text-measurement implementation, arbitrary host content, engine-side copies of styles, the style-damage→layout wiring (now engine-internal in `w3c-dom`) |

The engine/host seam keeps the engine storage-free even though its
vocabulary is stylo's: the Lynx-specific values and algorithms for Relative
and Linear live in `neutron-star`, but the crate owns no host storage — its
style accessors return the same computed values the stylo cascade produces,
so a stylo-backed host serves style views with no translation layer. Both
are first-class peers rather than translations into Flex or Grid. The
concrete adapter proved as mechanical as designed (`w3c-dom::layout`):
style views as direct post-flush `ComputedValues` field reads, per-node
`LayoutSlot`s resolved through the mutable state arena, and one display-mode
dispatch. The existing `&Node` shape remains where Stylo needs it; box layout
crosses the host boundary as `NodeId` so it cannot accidentally regain access
to mutable layout state through a tree backpointer.

## The protocol in one page

One tree trait (`neutron_star::tree::LayoutTree`) plus style-view traits
(`neutron_star::style`). The host gives each entry point three values:
`&tree`, `&mut state`, and a `Copy + Debug` node ID. `LayoutTree` uses GATs
for a borrowed `Style<'tree>` and `ChildIter<'tree>`, so topology and
style data stay tied to the immutable tree borrow while recursion mutates only
the independent state:

| Item | Provides | Consumed by |
| --- | --- | --- |
| `LayoutTree` | associated `NodeId`, mutable `State`, borrowed `Style<'tree>`, and `ChildIter<'tree>`; topology/style reads; **`compute_layout(&self, &mut State, NodeId, input)`** as the host display/algorithm dispatch point; immutable/mutable access to each state-owned `LayoutSlot`; required cache clearing | everything |
| `LayoutSlot` | one node's measurement cache, committed input, static position, unrounded layout, and rounded layout | shared cache/position/rounding machinery and host queries |
| `CoreStyle` | one `computed_values()` source plus defaulted box, Flex, Grid, Relative, and Linear accessors; sequence and geometry values remain borrowed | all box algorithms |
| `TextContainerStyle: CoreStyle` | paragraph-level alignment, whitespace, word-break, and indent values | the Parley `TextMeasurer` |
| `TextRunStyle` | run-level font, spacing, line-height, family, feature, and variation views; a Stylo host can expose one borrowed `computed_text_values()` source | the Parley `TextMeasurer` |

One `Style: CoreStyle` associated type serves every box algorithm. A
Stylo-backed host supplies its post-flush `ComputedValues` through
`computed_values()` and overrides only genuinely host-dependent lowering
(currently `position()` in `w3c-dom`); `CoreStyle`'s defaults lend all other
box/Flex/Grid/Relative/Linear fields directly from that source. Cascade-less
hosts can instead override individual accessors. Everything the engine reads
through `LayoutTree` is immutable for the flush; everything it writes goes
through `&mut State` into host-owned `LayoutSlot`s. The seam deliberately
stops there: there is no `LayoutTreeView`, `LayoutSession`, or `LayoutStore`
abstraction.

Entry points (`neutron_star::compute`) are free generic functions — there is
no engine object, so unused entry points never monomorphize into the host.
Implemented machinery: `compute_root_layout`, `compute_leaf_layout`
(closed `NaturalSize` replaced-content path), explicit hidden-subtree cleanup via
`hide_subtree`, `compute_cached_layout`
(keyed on the **complete `LayoutInput`** — see the caching section),
`compute_absolute_layout` (the positioned pass for out-of-flow nodes whose
containing block is not their formatting parent), and
`round_layout(tree, state, root, scale)` (device-pixel snapping), plus
`compute_flexbox_layout`, `compute_grid_layout`, `compute_relative_layout`,
and `compute_linear_layout`. All four algorithms share private allocation-free
length, edge, box-sizing, aspect-ratio, clamp, and relative-offset machinery.
Their public entry points use the same fixed shape:

```rust
pub fn compute_flexbox_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput;

// Grid, Relative, and Linear use the same four-argument shape.
```

All four signatures are public; hosts select them in their display dispatch.

Layout's transient call boundary uses two small `Copy` PODs: `LayoutInput`
(layout goal, sizing mode, known dimensions, whether those dimensions
establish definite percentage bases, parent size, and available space) →
`LayoutOutput` (size, content size, baselines). `Layout` (order, location,
size, content size, border/padding/margin) is the larger durable per-node
result and is deliberately neither `Copy` nor `Clone`: algorithms move it
into `LayoutSlot`, and readers borrow it through the slot or `Document`
query methods. The rounding pass snapshots only the record's small `Copy`
fields, constructs the rounded record directly, and retains the original
unrounded record in place. Whole-record duplication is not part of the
protocol. The
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
styles through `&tree`, then calls
`tree.compute_layout(state, child, input)`. The host's implementation
first handles a non-generated box (`display: none`, i.e. stylo
`Display::is_none`) by calling `hide_subtree` and
returning `LayoutOutput::HIDDEN`; this explicit cleanup precedes and bypasses
the cache. For a generated box, the host routes to a neutron-star container
algorithm, the natural-size leaf path, the concrete Parley text path, or a
future additional container algorithm, wrapping that
routing in `compute_cached_layout`. This decision buys three properties at
once:

1. **Open dispatch with four first-class algorithms.** Flex, Grid, and Lynx's
   non-CSS `display: relative` (id-anchored sibling constraint solving) and
   `display: linear` (Android `LinearLayout` semantics:
   `linear-direction`/`linear-weight`/…) are implemented peers in
   `neutron-star`, against the same tree/state protocol. The `<list>`
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

**No `dyn`, enforced structurally.** `LayoutTree` is dyn-incompatible because
its borrowed style/child-iterator GATs cannot form a trait object. Every
entry point is generic over the concrete tree, and `NodeId` is
`Copy + Debug`. Leaf content has no host trait to erase: its public choices
are the `NaturalSize` value path and the concrete Parley implementation. What the constraint buys:
every host⇄engine call site monomorphizes, inlines, and const-folds (style
accessors returning constants collapse into the algorithm); no vtable
indirection in the hottest recursion of the frame. The accepted costs:
compile time and per-host codegen (one copy of the algorithms per concrete
tree type — in practice one per binary), and no heterogeneous "list of
engines" (not a goal).

**The host owns all storage; the borrow split is the protocol.** Immutable
epoch data — topology, child order, computed styles, leaf content — is read
through `&LayoutTree`. Mutable results — unrounded/final layouts, static
positions, cache slots, measurement context, retained text artifacts — are
reached only through the disjoint `&mut LayoutTree::State`. This is an
ordinary static Rust borrow split, not `Cell`, `RefCell`, `AtomicRefCell`, an
unsafe borrow bypass, or a copied mirror tree. The concrete `w3c-dom` host
uses `NodeId` to index a `DocumentLayoutState` slab whose entries contain a
`LayoutSlot` plus an optional boxed text store; the shared text context is
also optionally boxed in that state. `TreeArenas` contains only nodes,
payloads, and Stylo traversal state. Its primary node slab selects the ID,
every side slab asserts the same free-list key, and removal drops all four
entries before reuse.

The phase boundary is equally explicit. Style traversal and layout both
require exclusive access to `Document`. Each Stylo preorder callback marks
its element's layout pointer stale before recalculation and publishes the
current primary `Arc<ComputedValues>` pointee only after recalculation
succeeds. The traversal driver must finish before the document publishes the
new generation as layout-ready. Layout style views lend
that post-flush pointee without borrowing `ElementData`, incrementing the
`Arc`, or copying computed style. The pointer remains valid until the next
exclusive style traversal, which cannot overlap the current layout call.
Safe mutable `ElementData` access outside that traversal leaves a per-element
static sentinel, so traversal unwind, a detached element, or an unrelated
retry cannot expose a pointer whose owning Arc was replaced.
The public `Node::computed_style` path still enters Stylo's guarded
`ElementData` API and returns a cloned `Arc`; the raw post-flush pointer is
layout-internal only.

A layout run observes one immutable **epoch**. Style, content, child order,
and NodeId validity cannot change during recursion; such mutations are
staged, invalidate the affected box and measurement caches, and start a new
epoch. Virtualized components therefore realize their visible topology
before layout or explicitly restart after realization.

**Style is read through views, in stylo's computed vocabulary.** The unified
box trait (`CoreStyle`), `TextContainerStyle`, and the standalone
`TextRunStyle` hand out **stylo computed values** per accessor call — the same
`Display`,
`LengthPercentage`, `Margin`, `AlignFlags`-based alignment wrappers, grid
track lists, and keyword enums the stylo cascade produces, re-exported from
`neutron_star::style`. There are no engine-owned style value enums and no
materialized engine-side style structs: a stylo-backed host lends one
`ComputedValues` through `CoreStyle::computed_values()`, whose default methods
perform direct field reads; a cascade-less host (tests, benches) can override
individual methods with hand-built stylo values. Small `Copy` values (keyword
enums, alignment flags, `Au` border widths, numbers) are returned owned; the
`LengthPercentage`-family geometry properties — inset, size, min/max size,
margin, padding, flex-basis, gap, grid-line placements — are returned
**borrowed** as per-field references inside the geometry wrappers
(`Edges<&Margin>`, `Size<&StyleSize>`, `&FlexBasis`), and sequence values —
grid track lists (`&GridTemplateComponent`, `&ImplicitGridTracks`) — are
returned borrowed whole, so no read ever clones a `calc()` tree or bumps a
refcount. Per-field reference wrappers are lendable from any host storage (a
`ComputedValues` host keeps the four margin edges as separate fields); a host
that synthesizes style values per call must materialize them in per-node
storage once per style change and lend from there. A source-backed
`TextRunStyle` similarly lends its computed font family through
`font_family_ref`; owned accessors remain available to hand-built run styles.
Defaulted trait methods return the **fork's initial values**:
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
and node-handle `resolve_calc(handle, basis)` callback are deleted.
Length-only `calc()` folds to a length at computed-value time and resolves
without a basis (a documented behavior delta of the vocabulary swap).

**Leaf content is a closed model, not a host extension point.** Replaced
content currently means images and enters `compute_leaf_layout` as a Copy
`NaturalSize`: independently optional natural dimensions plus a natural
width/height ratio. Before image metadata is decoded the value is
`NaturalSize::NONE`; the future media/runtime integration below the
Element-PAPI boundary owns installing decoded metadata through a
crate-private `w3c-dom` path and invalidating the node-to-root box-cache path.
That placement is deliberate: decoded intrinsic metadata is W3C replaced-
content state, while fetch/decode transport remains outside the generic DOM
API. The public DOM does not expose a natural-size mutation API. This internal state does **not**
mutate `contain-*` or `contain-intrinsic-size`: natural replaced size is
content metadata, whereas CSS size containment changes which intrinsic
contributions layout is allowed to inspect.

Text is the other fixed path. `TextMeasurer::compute_layout` enters the same
crate-private leaf box routine using Parley; external code cannot substitute a
different callback. `TextLayout` retains an owned `parley::Layout`, and its
borrowed view exposes size and first baseline without cloning or reshaping.
The `w3c-dom` host constructs a node-scoped `TextMeasurer` by borrowing
immutable text/style content and the separately mutable layout state. That
state lazily boxes one document `TextContext` and one `TextLayoutStore` for
each text node that is actually measured; ordinary nodes pay only the
nullable pointer in `NodeLayoutState`. The borrows end before the cache
wrapper stores the result. The text artifact cache
is separate from the box cache: probes must not evict the committed paint
artifact, and the artifact must outlive any committed box-cache entry that can
skip shaping. `LeafMeasureInput::goal` carries that probe/commit distinction;
no separate run-mode flag is needed. There is deliberately no
`LeafMeasurer`, `LeafMeasurement`, `FnLeafMeasurer`, or payload `MeasureLeaf`
API and no support for arbitrary host-rendered leaf content.

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
  and records it via
  `tree.layout_mut(state, node).set_static_position(...)`, but does not size
  or place it. After in-flow layout the host runs the **positioned pass**:
  it resolves the CB node (for Lynx `fixed`: the viewport root, or the
  nearest transformed/filtered/`will-change` ancestor per the W3C rule the
  tracking doc mandates), converts the recorded static position into CB
  padding-box space (all unrounded layouts exist by then), and calls
  `compute_absolute_layout(tree, state, node, cb_padding_box_size,
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

**Two durable layouts, no whole-record clone — on the device-pixel grid.**
Algorithms move **unrounded** `f32` layouts into `LayoutSlot`.
`round_layout(tree, state, root, scale)` reads the unrounded record's small
`Copy` fields, constructs the snapped `Layout`, and moves that result into the
same slot's rounded field. `Layout` deliberately implements neither `Copy`
nor `Clone`; retaining both forms does not make whole-record duplication an
API operation. `scale` is the
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

**CSS containment ([css-contain-2](https://drafts.csswg.org/css-contain-2/)).**
`contain` / `content-visibility` are a deliberate user-directed extension
beyond Lynx parity (Lynx has no such property — see
`docs/style-assumptions.md`). The engine reads containment through
`CoreStyle::{containment, contain_intrinsic_width, contain_intrinsic_height,
skips_contents}`, which speak stylo's own computed types directly — the
`Contain` bit set (`SIZE`/`LAYOUT`/`PAINT`/`STYLE` effect bits) and
`ContainIntrinsicSize` (both re-exported from `crate::style`). The host
derives these from computed style, folding `content-visibility` into
`containment()` exactly as stylo's gecko-mode effective-containment mapping
does — `crate::style::effective_containment` mirrors `w3c-dom`'s own
`effective_containment` copy (each crate keeps its own; no dependency between
them). Only `SIZE` and `LAYOUT` have v1 box-layout effects:

- **Size containment** (`containment().contains(SIZE)`) — every content-derived automatic size
  (`auto`, `min-/max-/fit-content`, and the Flexbox §4.5 automatic minimum) resolves **as if the box
  were empty**, substituting `contain-intrinsic-{width,height}` (both physical axes; single-axis
  `inline-size` is ignored). Children are **still laid out** for Commit and still contribute
  scrollable overflow — only the box's *own* content-based sizing ignores them, so a parent probing
  a size-contained child already sees the substituted answer (enforced at the child's own sizing
  layer, not in parents). A size-contained leaf skips its measurer entirely. The skipped-contents
  path shares this sizing.
- **Layout containment** (`containment().contains(LAYOUT)`) — the box exports **no** baseline
  (`LayoutOutput::first_baselines = NONE` at each algorithm's output construction; Relative already
  exports none), and it **changes scrollable overflow**: with `overflow: visible`, a layout-contained
  box's descendant overflow is *ink* overflow ([css-contain-2 §3.3](https://drafts.csswg.org/css-contain-2/#containment-layout),
  item 3), so its `content_size` collapses to its own border box; a scroll container instead keeps
  its interior union as its scroll range. This is applied at each algorithm's output construction by
  the `own_scrollable_overflow` helper. Each display mode is already its own formatting context and
  there is no margin collapsing yet, so IFC establishment is structural (a `debug_assert`/comment
  marks where collapsing would land). Together with `PAINT`, effective layout containment makes the
  box a **containing block for abs/fixed descendants** — a *host* contract, not engine topology: the
  host treats it like transform/filter/`will-change` in its positioned pass (see
  `PositionProperty::Fixed`). `PAINT` (clip + stacking context) and `STYLE` (counter/quote
  scoping) are carried for fidelity but have no v1 box-layout consumer — paint is a render-layer
  concern, and this engine has no counters/quotes. Effect bits are queried individually via
  `Contain::contains` (never the `CONTENT`/`STRICT` marker composites, which carry serialization
  marker bits).

**Scrollable-overflow trapping (css-overflow-3 §3.3).** Orthogonally to containment, every **scroll
container** (any `overflow` axis other than `visible` — under the lynx stylo grammar that means
`hidden`) traps its interior scrollable overflow: it stores its own full `content_size` as its scroll
range, but contributes only its **border box** to an ancestor's `content_size`
([the child's scrollable-overflow rectangle is "clipped to their overflow clip edge if overflow is not
visible"](https://drafts.csswg.org/css-overflow-3/#scrollable)). Each algorithm's per-child
accumulation applies this through the `accumulate_scrollable_overflow` helper. This makes warm
incremental relayout (which refreshes only a boundary's *own* `content_size` in place) consistent with
a cold full relayout by construction: an ancestor never re-derives a value that includes a scroll
container's trapped interior.

**Skipped contents** (`content-visibility: hidden`, or `auto` once a host
reports the box non-relevant): `compute_skipped_contents_layout` sizes the box
purely from styles + `contain-intrinsic` substitution, lays out **no**
children, and on Commit calls `hide_subtree` on each child to clean stale
geometry/caches. It dispatches **before** `compute_cached_layout`, right after
the `display: none` (`Display::is_none`) check. The child-hiding deliberately **precedes
and bypasses the cache boundary** (mirroring `hide_subtree`): caching a skipped
result and later serving it on a hit would leave a re-populated child subtree
un-hidden; sizing a contentless box is cheap and re-hiding per pass is far
cheaper than laying the subtree out.

**The relayout-boundary theorem.** A box is a **relayout boundary** iff its
effective containment includes **both** `LAYOUT` **and** `SIZE` (i.e.
`contain: strict`, or a skipped `content-visibility` box) —
`invalidate::is_relayout_boundary`. Under those two together an internal
descendant mutation (a) cannot escape the box's formatting context and (b)
cannot change the box's own outer size, so no ancestor or sibling needs
re-laying-out and the box is a valid re-layout root — via
`compute_boundary_relayout` with its previous committed `LayoutInput` (see the
host relayout workflow below), because the boundary's *used* size may be
parent-imposed (stretch, flex, percentages) and must be re-derived from the
same input, not re-synthesized from `available_space`. **Critical
caveat — layout alone is not a boundary:** `contain: layout` (or `content`)
*without* size still lets the container's intrinsic size depend on its
contents, so an internal change can resize the container and reflow ancestors.
Only `+size` closes the upward path. (A definite outer size can also close it,
but that is a per-`LayoutInput` property, not a style property, so the
predicate keys off style containment only.)

**The host relayout workflow.**
`invalidate::invalidate_for_relayout(tree, state, node, ancestors)` clears
the NodeId's cache, walks the host-supplied NodeId ancestor path (nearest
first — the engine has no parent links) clearing each cache, and **stops at
and returns** the first ancestor ID that is a relayout boundary (or the last
yielded ID = root). When the
returned root is a containment boundary (not the true tree root), the host
captures the boundary's previous committed `LayoutInput`
(`Cache::committed_input`) *before* the walk clears it and re-runs
`compute_boundary_relayout(tree, state, boundary, input)` — identical input + unchanged
style ⇒ identical outer size,
so only the interior re-arranges and the parent-owned frame stays valid
(`compute_root_layout` remains the entry for the true tree root). This only
ever `clear_layout_cache`s;
it never reads or weakens cache keys (the key stays the complete
`LayoutInput`). The style-damage → host-action translation table (REPAINT /
stacking / overflow-only → no cache work; RELAYOUT → invalidate + re-run;
reconstruct/`display`/structural mutation → same but start from the mutated
node's parent) is rustdoc'd on `invalidate` (`crate::invalidate`); it names the
damage classes conceptually so the engine stays stylo-free. The upstream
producer of those classes is `w3c-dom`'s `StyleDamage`/`FlushSummary`.

## Performance architecture

Target: modern multi-core CPUs with wide SIMD, deep caches, and GPUs doing
the painting — layout's job is to never be the frame's bottleneck.

- **Static dispatch end-to-end** (above). The protocol's hot calls
  (`children`, style accessors, `compute_layout`) are all
  monomorphized; hosts should `#[inline]` their impls of the first two.
- **Cheap transient boundary; explicit durable records.** Geometry,
  `LayoutInput`, and `LayoutOutput` are small `Copy` structs (`#[repr(C)]`),
  passed by value; `Layout` is a larger non-`Copy`, non-`Clone` record moved
  into host storage and read by reference, so a whole-layout clone is not
  available. Values use `f32` throughout (GPU/SIMD native, halves
  cache traffic vs `f64`; Starlight/Yoga/Taffy all agree).
  No `NaN` sentinel games — unknowns are `Option<f32>`/enum variants, and
  boundary values must be finite (debug-asserted).
- **Document data is split by ownership and phase.** `TreeArenas` keeps the
  primary nodes, payloads, and Stylo traversal state together; the disjoint
  `DocumentLayoutState` slab keeps `LayoutSlot` plus the optional text-artifact
  pointer. This gives the engine an ordinary `&TreeArenas` /
  `&mut DocumentLayoutState` split, removes layout/text runtime borrow
  bookkeeping, and keeps recursive layout from touching mutation-heavy DOM
  payloads. The primary slab selects IDs and removal clears all four aligned
  slab entries before reuse. A live-node slot fast path avoids redundant
  occupancy checks after the tree has already proved the ID live.
- **Layout style views do not borrow Stylo state or bump refcounts.** During
  exclusive traversal, each element's preorder callback brackets style
  recalculation with a stale marker and a successful publication into the
  primary `Arc<ComputedValues>` that remains owned by its `ElementData`.
  The driver completion publishes the generation as layout-ready; traversal
  unwind and out-of-band mutation remain fail-closed per element.
  `StyleView` is two words (node + computed-style pointer);
  `TextStyleView` is one pointer to its parent's inherited values. Their
  `CoreStyle` implementations lend fields from that immutable style
  generation with no `ElementData` borrow check, `Arc` increment, or
  engine-side copy. The next exclusive traversal may replace the owning Arc,
  but cannot begin until layout has released its `&mut Document`. The
  `AtomicPtr` occupies the element variant's existing word; the 64-bit
  structural size guard keeps `Node<()>` at no more than 208 bytes (208 in
  debug builds, 200 in release builds because the fork's borrow-checking
  wrapper is debug-only).
  Grid intrinsic sizing may recursively probe an item, then re-fetches that
  item's style from this same published generation instead of cloning the
  style payload into its scratch record. A host that changes styles during a
  layout pass violates the immutable-style phase contract; if the refreshed
  value disagrees with the captured intrinsic variant, the Grid pass
  deliberately fails fast.
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
  generated-box dispatch, per-node slots (`cache::Cache`, embeddable, with
  four common measurement slots inline and an uncommon spill capped at
  `MEASURE_CACHE_SLOTS = 8`, plus one layout slot). Shape-aware replacement is
  implemented; probe-trace validation and tuning remain L4 work. The key is
  the **complete
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
  at the recursion boundary without being walked. The engine now ships the
  containment-aware machinery for this: `invalidate::invalidate_for_relayout`
  walks the host-supplied ancestor path and **stops at the nearest relayout
  boundary** (`contain: strict` / skipped `content-visibility`), returning
  the recommended re-layout root, so a dirty leaf inside a contained subtree
  never invalidates past the boundary. `w3c-dom::layout` now drives this
  end-to-end: `Document::invalidate_layout` inlines the boundary-stopped
  ancestor walk (with real parent links, so no re-root return value is needed).
  Every `w3c-dom` style harvest classifies `StyleDamage` into those calls before
  returning or streaming it, and `Document::layout` re-runs each parked boundary
  via `compute_boundary_relayout` before the root pass. Because it re-runs from
  the document root, a boundary's interior
  is unreachable while the boundary's ancestors stay warm, so the in-place
  boundary re-run is what actually refreshes it (`compute_root_layout` from the
  warm root would answer from cache and never descend). Parked boundaries are
  deduplicated through an `O(1)` `FxHashSet` companion to the parked list, so a
  batch that parks `B` independent boundaries (a dirty leaf per contained row of
  a virtualized list) stays `O(B)`, not `O(B²)`.
- **The fused positioned + rounding tail is scoped to what changed, not the
  whole tree.** The parked-boundary re-runs and the root pass are
  cache-incremental, while hoisted out-of-flow anchoring and device-pixel
  rounding share one `round_layout_subtree_with` traversal. Its `pre_position`
  hook runs before each node is rounded, so positioning observes unrounded
  geometry without requiring a separate walk. `Document::layout` scopes that
  traversal: when nothing has been invalidated since the last pass and the
  viewport/scale are unchanged it **skips the whole pass** (an idle frame is
  `O(1)`, not an `O(N)` re-walk); when every pending change is confined to
  parked containment boundaries it runs the fused traversal **only over each
  outermost parked boundary's subtree**, re-snapping from the parent's
  accumulated unrounded origin byte-identically to a full re-round and leaving
  clean subtrees untouched. Only a change that reached the document root or a
  viewport/scale move falls back to a whole-tree traversal. This keeps a single
  contained mutation `O(boundary subtree)` end-to-end.
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
  single `&mut State` assumes a sequential layout pass — correct for v0
  (layout is rarely the bottleneck vs paint/style, and Yoga/Taffy/Starlight
  are all sequential). The planned extension is a **batched child-layout
  hook**: a defaulted `LayoutTree` method like
  `compute_layouts(&self, &mut State, requests)`
  that algorithms call at fan-out points (independent flex-item measure
  probes, grid item contributions); the default body is today's sequential
  loop, and a parallel host overrides it to shard sub-trees across its own
  pool with thread-safe slots (host storage, host threading policy — the
  engine stays thread-unaware). Adding a defaulted method is semver-minor,
  so this ships when profiles earn it, without a protocol break.
- **Flex, Grid, Relative, and Linear benchmarks are landed; broader
  performance hardening remains.** The `divan` (CodSpeed-compatible) suite
  measures CSS-built documents through w3c-dom's production host: styles are
  flushed outside the timed region, while measured calls enter through
  `Document::layout` and include the real `TreeArenas + DocumentLayoutState`
  protocol,
  per-node caches, positioned pass, and rounding. The Flex suite covers deep
  and wide trees, wrapping, weighted
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
  measurement/stretch, and mixed hidden/absolute children. Each algorithm
  suite also has five text-bearing clones of its complex workloads (20 total)
  that enter through the same production document host and invoke the Parley
  measurer during box layout. The clones cover Flex owner/direction/alignment/
  distribution/wrapping matrices; Grid dense placement, intrinsic spans, flex
  track freezing, and nesting; Linear weighted freezing, positioned/hidden
  children, percentage intrinsic sizing, and cross gravity; and Relative
  wrap-width refinement, dependency chains/cycles, duplicate ids, and nesting.
  Equivalent-tree
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
   main size per item; child measurement via `compute_layout` probes
   with `SizingMode::IgnoreSizeStyles`.
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
   via `LayoutSlot::set_static_position` — the host's positioned pass finishes them.
9. **Finalize** — per-child `LayoutSlot::set_unrounded` (only for
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
`LayoutTree`, a separately borrowed mutable state, and NodeIds, with Linear
style-view bounds speaking stylo computed values; the concrete `w3c-dom` host
dispatches to it.

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
   area, `LayoutSlot::set_unrounded`, container size, and `content_size`.

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
  implementing `LayoutTree` over an immutable tree/style value and a
  separately borrowed plain mutable state; proves the protocol needs neither
  `dyn` nor runtime borrow wrappers, stores and serves stylo computed values
  directly (no calc callback — stylo
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
  behavior suite and a CodSpeed-compatible production-host benchmark target.
  Tests use exact
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
- **Remaining Lynx integration:** runtime-level view/device policy,
  component-specific staggered layout, sticky lowering, and mixed-runtime
  parity remain future work; the integration layer's final module or crate
  placement has not been established.

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
  handling, engine-native conformance fixtures, and w3c-dom-hosted CodSpeed
  benchmarks.
- **L3 — Starlight modes + runtime integration** *(partial)*: the Lynx-linear
  value and style-view protocol, generic `compute_linear_layout` algorithm,
  unconditional Parley text measurement core, and CSS-containment machinery
  (size/layout containment, `content-visibility` skipped contents, the
  relayout-boundary predicate, and `invalidate_for_relayout`) are complete in
  `neutron-star`; `w3c-dom` produces per-node `StyleDamage`/`FlushSummary` and
  the `effective_containment` fold as its style-to-layout seam (kept internal —
  no runtime adapter forwards or re-exports it), and its `layout` module
  now **closes** the damage→layout loop: every style harvest consumes
  relayout-class `StyleDamage` through boundary-stopped
  `Document::invalidate_layout`, and `Document::layout` re-runs each parked
  `contain: strict` boundary via `compute_boundary_relayout` — entirely
  engine-internal, with no runtime adapter involved. The concrete host also
  includes `LayoutTree` on immutable `TreeArenas`, separate
  `DocumentLayoutState`, NodeId display dispatch, fixed positioning,
  post-flush computed-style views, W3C text style lowering, and lazily boxed
  document/per-node text state. Remaining L3 work is sticky lowering, legacy Lynx spelling/attribute lowering,
  element-backed raw text and truncation, view metrics/`rpx`, and component
  modes such as `staggered`. No separate text crate is planned.
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
- **Deeper hot-path allocation/atomic trims (benchmark-gated).** The
  containment-scoped positioned + rounding tail and the idle-frame skip cover
  the asymptotic end-to-end cost; the remaining items are constant-factor and
  should land only behind a profile that shows them:
  - The incremental positioned pass still walks a whole boundary *subtree* to
    find its hoisted nodes; a per-boundary hoisted-node registry would visit
    only the out-of-flow nodes. Similarly, an already-hidden subtree is still
    re-zeroed inside a boundary re-run rather than only on the visible↔hidden
    transition.
  - The rounding, hide-subtree, and positioned walks are recursive, so a
    pathologically deep single subtree still carries stack-depth risk; an
    explicit worklist would remove it (systemic and pre-existing, not
    containment-specific).
  - The flush's zero-alloc damage sink still seeds its spine walk from a
    one-element `Vec`; a reusable scratch stack would make a clean flush truly
    allocation-free.
