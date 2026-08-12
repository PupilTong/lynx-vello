# Design: Taffy-backed Flex/Grid with a Taffy-shaped Lynx layout protocol

Status: **design, not yet implemented**. This document records the decision
basis and target architecture for replacing `hughie`'s hand-written Flexbox
and Grid algorithms with the `taffy` crate's low-level custom-tree API, and
for restructuring `hughie` and `dom::layout` so the whole layout protocol
uses taffy's vocabulary. Verified against **taffy 0.13.0** (released
2026-08-08) and Blitz `blitz-dom`/`stylo_taffy` at 2026-08-12 main. The raw
research reports (taffy API inventory, Blitz prior art, hughie/dom protocol
inventories, Linear/Relative requirements, test-impact inventory) are in
[taffy-layout-integration-research.md](taffy-layout-integration-research.md).

Requirements this design satisfies (user directive, 2026-08-12):

1. Only taffy's **low-level custom-tree API** is used — never `TaffyTree`,
   never taffy-owned node storage.
2. The dom-facing layout traits are refactored around taffy's traits.
3. `hughie` is restructured so its remaining algorithms (Linear, Relative,
   leaf/text, positioned pass, containment, rounding) have the same API
   shape as taffy's compute functions and trait extensions.
4. Linear, Relative, CSS containment, and the fixed/hoisted positioning
   capability are all retained at current functionality.
5. `display: contents` flattening moves out of the engine into `dom`, which
   flattens automatically when building layout child lists.
6. Latest taffy (0.13.0).
7. The implementation preserves the current performance architecture
   (static dispatch, zero style materialization, incremental relayout,
   allocation discipline).

## 1. What taffy 0.13 provides, verified

| Capability | Status in taffy 0.13.0 |
| --- | --- |
| Flexbox L1, Grid L2 via `compute_flexbox_layout` / `compute_grid_layout`, generic over host traits | Yes — `LayoutFlexboxContainer` / `LayoutGridContainer: LayoutPartialTree` |
| `direction: rtl` | **Yes** (since 0.12.0; `CoreStyle::direction()`, consumed by flex/grid/block) |
| Named grid lines / areas | Yes (generic `CustomIdent: CheapCloneStr`) |
| calc() | Yes — 8-byte tagged pointer in `CompactLength`; host resolves via `LayoutPartialTree::resolve_calc_value(*const (), basis) -> f32` |
| Static-position fallback for absolute children (auto insets) | Yes, flex and grid (reworked 0.13, #1072/#1071) |
| content_size (scrollable overflow) incl. scroll-container trapping | Yes — non-`visible` children contribute border box only (`compute_content_size_contribution`) |
| Host-owned caching | Yes — `CacheTree` trait; `taffy::Cache` is optional |
| Partial relayout from any node | Yes — every compute function takes `NodeId` + explicit `LayoutInput` |
| CSS `order` | **No** — no such property; `Layout.order` is child-list index |
| `position: fixed` / containing block other than the layout parent | **No** — `Position` is `Relative \| Absolute`; abs-pos resolves against the direct parent |
| CSS containment / `content-visibility` | **No** |
| `min-content`/`max-content`/`fit-content()` as width/height values | **No** — `Dimension` is length/percent/auto/calc only (intrinsic tags exist only for grid track sizing functions) |
| Percentage-definiteness separate from `known_dimensions` (Flexbox §9.8 nuance) | **No** — definiteness is the `Option`-ness of `known_dimensions`/`parent_size` |
| Last-baseline alignment | No (first baseline only — parity with hughie today) |
| DPR-aware rounding | No — `round_layout` rounds to integer CSS px, origin fixed at (0,0) |
| Subgrid / masonry | No (parity with hughie today) |

Every "No" above maps to machinery we already have and keep (order sorting,
positioned pass, containment layer, extended input, our rounding pass), or to
an accepted, recorded behavior delta (§10).

### 1.1 Consolidated: what cannot be done

Three groups. Group A is functionality this migration **loses** — no
host-side workaround exists short of changing taffy itself. Group B is taffy
components we **cannot adopt** and must keep our own replacements for.
Group C is taffy gaps that are fully covered by retained host/companion
machinery (no functional loss, but the code cannot be deleted).

**A. Cannot be preserved (hard losses):**

- **A1. Intrinsic sizing keywords on flex/grid boxes.** `min-content`,
  `max-content`, and `fit-content()` as preferred/min/max width/height of a
  flex or grid item/container collapse to `auto` — taffy's `Dimension` is
  length/percent/auto/calc only (the intrinsic tags exist solely for grid
  *track* sizing functions), and the sizing paths inside
  `compute_flexbox_layout`/`compute_grid_layout` cannot be intercepted per
  property. hughie currently implements these keywords; Blitz ships the same
  collapse. Only remedy: upstream taffy contribution or a vendored patch.
  Linear, Relative, and leaf boxes retain full support (their algorithms are
  ours and keep reading stylo values).
- **A2. Known-but-indefinite percentage bases inside flex/grid subtrees.**
  taffy has no definiteness flag separate from `known_dimensions`; a
  parent-decided size is always a definite percentage basis for descendants.
  The Flexbox §9.8 / Starlight distinction hughie models with
  `definite_dimensions` cannot be threaded through taffy's algorithms. It is
  preserved only across linear/relative/leaf edges via `LayoutInputExt`
  (§5); within flex/grid subtrees taffy's model applies (delta D2).
- **A3. hughie's grid hardening policies.** Track-limit component dropping,
  hostile `repeat()` count bounding, and minimum-precedence clamping of
  auto-repeat counting bases are replaced wholesale by taffy's own policies;
  the exact current behaviors are not reproducible from outside the
  algorithm.
- **A4. Flex/grid probe-shape and cache-traffic guarantees.** The
  single-axis measure fast path, probe-count linearity, and
  measure-goal write discipline pinned by ~10 algorithm-internal tests are
  properties of hughie's implementations; taffy's internal call pattern
  differs and is not observable or controllable. Those tests die and the
  CodSpeed flex/grid baselines reset.
- **A5. Bit-exact flex/grid geometry stability.** Where both
  implementations are spec-conformant but the spec allows latitude
  (tie-breaks, iteration order, float accumulation), some results will
  move; the conformance corpus is re-pinned to taffy's answers in those
  cases rather than treated as regressions.

**B. taffy components that cannot be adopted (keep our own):**

- **B1. `taffy::Cache`** — cannot return the committed `LayoutInput`
  (private, lossily packed keys), which parked-boundary relayout must
  replay, and cannot key on the `LayoutInputExt` definiteness bits (distinct
  inputs would alias). Keep hughie's cache behind the `CacheTree` trait.
- **B2. `taffy::compute_leaf_layout`** — its measure closure returns only
  `Size<f32>`, dropping text first-baselines, which Linear consumes at
  measure time and flex baseline alignment consumes at commit. Keep
  hughie's leaf engine and the Parley `TextMeasurer` probe/commit artifact
  path.
- **B3. `taffy::round_layout`** — integer CSS-px rounding with no
  device-pixel-ratio parameter and a subtree origin hardcoded to (0,0);
  incompatible with DPR-aware snapping and the boundary-scoped incremental
  tail. Keep the fused positioned+rounding pass.
- **B4. `taffy::compute_root_layout`** — no definiteness bits and no
  parked-boundary integration. Keep ours.
- **B5. The `stylo_taffy` crate** — depends on crates.io stylo 0.19/0.20;
  our stylo is the vendored fork (`lynx` branch: no `writing-mode`, Lynx
  `overflow` grammar, `lynx-rtl`, `linear-*`/`relative-*` longhands). The
  conversion module is rewritten in-repo modeled on it
  (`hughie::style::convert`).
- **B6. `TaffyTree`** — excluded by requirement 1 (host-owned storage
  only); also loses the split-borrow architecture and NodeId alignment.

**C. taffy gaps fully covered by retained machinery (no loss, no deletion):**

- **C1. CSS `order`** — no taffy property; dom pre-sorts layout child lists
  (§4).
- **C2. `position: fixed` / CB-escaping absolute** — taffy positions
  absolute children against the direct parent only; the host positioned
  pass and `compute_absolute_layout` remain (§7).
- **C3. CSS containment / `content-visibility`** — absent in taffy; the
  dispatch-wrapper interception plus the boundary/invalidation machinery
  remain (§7).
- **C4. Starlight Linear/Relative** — out of taffy's scope by definition;
  remain hughie algorithms.
- **C5. `display: contents`** — taffy has no contents box-generation mode;
  dom flattens (§4), which is requirement 5 anyway.

Parity items (absent on both sides, no change): last-baseline alignment,
subgrid, masonry.

## 2. Ownership after the migration

```text
        dom (host)                                  hughie (Lynx layout companion to taffy)
┌─────────────────────────────────────┐   ┌──────────────────────────────────────────┐
│ LayoutCtx<'t,T> {&TreeArenas,       │   │ taffy-shaped extension traits:           │
│   &mut DocumentLayoutState}         │──▶│   LayoutLynxTree, LayoutLinearContainer, │
│ implements taffy traits +           │   │   LayoutRelativeContainer                │
│ hughie extension traits             │   │ compute_linear_layout /                  │
│ layout_children: flattened +        │   │ compute_relative_layout (taffy shape)    │
│   order-sorted box child lists      │   │ leaf/text (Parley, baselines),           │
│ dispatch + containment wrapper +    │   │ compute_absolute_layout, skipped-        │
│ positioned pass + DPR rounding      │   │ contents, hide, boundary relayout,       │
│ stylo→taffy style views (convert)   │   │ Cache (ext-keyed), invalidate, rounding  │
└─────────────────────────────────────┘   └──────────────────────────────────────────┘
                     │                                      │
                     └────────────► taffy 0.13 ◄────────────┘
                            compute_flexbox_layout,
                            compute_grid_layout, traits,
                            geometry + style value types
```

- **taffy** owns the Flexbox and Grid algorithms, the trait vocabulary
  (`TraversePartialTree`, `LayoutPartialTree`, `CacheTree`,
  `LayoutFlexboxContainer`, `LayoutGridContainer`), and the value types
  (`LayoutInput`, `LayoutOutput`, `Layout`, `Style` value enums, geometry
  `Point/Size/Line/Rect`). Workspace dependency `taffy = "0.13"`,
  `default-features = false`, features `std, flexbox, grid, calc,
  content_size`. No `block_layout` (Lynx has no block formatting context —
  `DisplayInside::Flow` remains a leaf), no `taffy_tree`. Start on crates.io;
  vendor only if we end up needing patches (repo precedent exists for
  vello/wgpu/stylo, and §10 lists the candidate reasons).
- **hughie** stops being a self-contained engine and becomes the Lynx
  companion crate to taffy: Starlight Linear + Relative as taffy-shaped
  compute functions, the Parley leaf/text path, the shared absolute/hoisted
  resolver, containment machinery, the extended-key cache, invalidation, and
  DPR rounding. Its own `LayoutTree`, `LayoutInput/LayoutOutput/Layout`,
  `AvailableSpace`, and geometry module are **deleted** in favor of taffy's
  (plus one extension struct, §5). `compute/flexbox.rs` (~2.5k lines) and
  `compute/grid/` (~6k lines) are deleted.
- **dom** keeps all host policy: display dispatch, style views, containment
  fold, positioned pass, invalidation funnel, parked boundaries, fused
  positioned+rounding traversal, and — new — `display: contents` flattening
  and `order` sorting (§4).

The stylo→taffy value conversion module lives in `hughie::style::convert`,
modeled line-for-line on Blitz's `stylo_taffy/src/convert.rs` but written
against **our stylo fork** (`stylo_taffy` itself depends on crates.io stylo
0.19/0.20 and cannot be reused; our fork also lowers differently — no
`writing-mode`, Lynx `overflow` grammar, `lynx-rtl`). hughie already owns the
stylo dependency, and its test/bench mock hosts need the conversions too.

## 3. The host object and trait stack

taffy's traits take `&mut self` on one object; the current protocol passes
`(&tree, &mut state)`. The borrow split survives inside a wrapper:

```rust
pub(crate) struct LayoutCtx<'t, T> {
    tree: &'t TreeArenas<T>,
    state: &'t mut DocumentLayoutState,
}
```

`LayoutCtx` implements, statically dispatched (no `dyn` anywhere, as today):

- `taffy::TraversePartialTree` — `child_ids` iterates the node's **layout
  children** (§4); `child_count`/`get_child_id` are O(1) slice reads.
- `taffy::LayoutPartialTree` — `type CustomIdent = stylo::Atom`;
  `get_core_container_style` returns the taffy-vocabulary style view (§6);
  `resolve_calc_value` casts the tagged pointer back to the fork's
  `CalcLengthPercentage` and evaluates it (identical to Blitz);
  `set_unrounded_layout` writes `slot.unrounded`; `compute_child_layout`
  forwards to the extended dispatch below.
- `taffy::CacheTree` — forwards to the ext-keyed cache with input
  normalization (§5).
- `taffy::LayoutFlexboxContainer` + `taffy::LayoutGridContainer` — style
  views per §6.
- `hughie::LayoutLynxTree` (new, defined in hughie):

```rust
pub trait LayoutLynxTree: taffy::LayoutPartialTree + taffy::CacheTree {
    /// Stylo-vocabulary style view. A plain associated type, not a GAT:
    /// the implementing context is itself lifetime-parameterized, so the
    /// view can borrow the *arena* lifetime and stay alive across
    /// `&mut self` child-layout calls (taffy's `CoreContainerStyle<'a>`
    /// GAT is tied to `&'a self` and cannot).
    type LynxStyle: LynxCoreStyle;
    fn lynx_style(&self, node: NodeId) -> Self::LynxStyle;
    fn set_static_position(&mut self, node: NodeId, pos: Point<f32>);
    fn unrounded_layout(&self, node: NodeId) -> &Layout;
    /// Extended dispatch carrying Starlight definiteness (§5).
    fn compute_child_layout_ext(&mut self, node: NodeId, input: LayoutInputExt)
        -> LayoutOutput {
        self.compute_child_layout(node, input.base)
    }
}

pub trait LayoutLinearContainer: LayoutLynxTree {
    type LinearContainerStyle<'a>: LinearContainerStyle where Self: 'a;
    type LinearItemStyle<'a>: LinearItemStyle where Self: 'a;
    fn get_linear_container_style(&self, node: NodeId) -> ...;
    fn get_linear_child_style(&self, child: NodeId) -> ...;
}
// LayoutRelativeContainer mirrors taffy's container-trait shape the same way.
```

`LayoutCtx<'t, T>` sets `type LynxStyle = StyleView<'t, T>` — the existing
two-word post-flush view, unchanged in mechanism: it lends the published
`ComputedValues` pointer under the exclusive flush/layout phase boundary, no
`Arc` bump, no `ElementData` borrow. Because `'t` is the arena borrow rather
than the `&self` borrow, Linear/Relative keep holding style views across
recursive child layout exactly as they do today; this is what makes the
`&mut self` migration borrow-check without copying styles into scratch.

Entry points keep taffy's function shape:

```rust
pub fn compute_linear_layout<T: LayoutLinearContainer>(
    tree: &mut T, node: NodeId, input: LayoutInputExt) -> LayoutOutput;
pub fn compute_relative_layout<T: LayoutRelativeContainer>(...) -> LayoutOutput;
pub fn compute_absolute_layout<T: LayoutLynxTree>(
    tree: &mut T, node: NodeId, containing_block: Size<f32>,
    static_position: Point<f32>) -> Layout;
pub fn compute_skipped_contents_layout<T: LayoutLynxTree>(...) -> LayoutOutput;
pub fn compute_boundary_relayout<T: LayoutLynxTree>(...) -> LayoutOutput;
pub fn compute_root_layout<T: LayoutLynxTree>(...);   // kept ours: definite
                                                      // bits + parked-boundary
                                                      // integration
pub fn hide_subtree<T: LayoutLynxTree>(...);          // caller-driven, keeps
                                                      // the paint-order slot
pub fn round_layout_subtree_with<T: LayoutLynxTree>(...);  // DPR rounding
```

`LayoutSlot` survives with taffy's `Layout` inside:

```rust
pub struct LayoutSlot {
    cache: Cache,                       // hughie cache, ext-keyed (§5)
    pub static_position: Point<f32>,
    pub unrounded: taffy::Layout,
    pub rounded: taffy::Layout,
}
```

taffy's `Layout` is `Copy`; the previous non-`Copy` discipline on the durable
record is dropped as a consequence of adopting taffy's type (the rounding
pass already only reads small fields; no new whole-record clones are
introduced by us, but the type no longer forbids them).

## 4. `display: contents` and `order` move into dom's child lists

taffy requires random access (`get_child_id(parent, index)`) and has no
`order` property, so lazy flattening iterators are out and pre-sorting is
required anyway. dom gains a per-node derived **layout child list**:

- Storage: `Option<Box<[NodeId]>>` on the node's layout-adjacent state.
  `None` means "identical to `flat_children`" — the common case pays one
  null pointer, and `child_ids` serves the flat-tree slice directly.
- The boxed list exists only when the node has at least one
  `display: contents` child (recursively spliced, source order — the exact
  semantics of today's `FlattenedChildren`) or at least one child with
  non-zero effective `order`.
- Sort key: `(effective_order, flattened_index)` where in-flow children use
  style `order` and absolute/fixed children use 0, `display: none` children
  keep their slot (taffy hides them; exclusion would lose paint-order
  slots). This is exactly today's `sibling_effective_paint_order` rule, so
  taffy's `Layout.order` (= index in the sorted list) becomes the correct
  order-modified-document-order paint rank **for in-flow and absolute
  children alike**, and `dom::visual` keeps consuming `Layout.order`
  unchanged.
- Rebuild triggers, driven by the existing damage harvest and mutation
  paths: child-list mutation under a parent with a boxed list (or where the
  inserted child is `display: contents`); a child's `display` crossing the
  contents boundary or `order` changing (both are `RestyleDamage` reconstruct
  /relayout classes we already harvest); mutations under a contents element
  rebuild the nearest box ancestor's list (`box_parent` walk, which already
  exists). Rebuilds happen during flush commit, outside the layout pass, so
  the layout epoch still observes immutable topology.
- Consumers migrated from `flattened_children`: the four algorithms (via
  `child_ids` now), `sibling_paint_order`, and `visual/build.rs`. The
  engine-side `FlattenedChildren` iterator is deleted; `hughie` never sees a
  contents node (its `LynxCoreStyle` keeps the `display()` accessor for
  `is_none()` classification only).
- All other dom-side contents semantics are already host-side and stay
  verbatim: CB predicates return false, containment empty, never skipped,
  slot zeroed by the positioned pass, `box_parent` inheritance for text.

Because taffy has no `Contents` box-generation mode (Blitz flattens at box
construction too), this also removes the `unreachable!` dispatch arm.

## 5. The extended input: Starlight definiteness

Linear and Relative consume **and produce** a per-axis "known but not
definite as a percentage basis" bit on every child call (Starlight §2:
content-derived sizes are not percentage bases; stretch/constraint-imposed
sizes are). taffy's `LayoutInput` cannot express it, and it must participate
in cache keys (two calls differing only in definiteness must not alias).
Verified consumption sites: `linear.rs:1307` (outer definiteness →
`definite_inner_size` → children's percent basis; two-phase box refresh
gate), `relative.rs:1296/831/794` (initial parent size; remeasure freeing;
fixed-measurement memo).

```rust
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct LayoutInputExt {
    pub base: taffy::LayoutInput,
    pub definite_dimensions: Size<bool>,
}
impl From<taffy::LayoutInput> for LayoutInputExt {
    // taffy semantics: a known dimension is definite.
    fn from(base: taffy::LayoutInput) -> Self {
        Self { definite_dimensions: base.known_dimensions.map(|d| d.is_some()), base }
    }
}
```

- The host's real dispatch is `compute_child_layout_ext`;
  `compute_child_layout(input)` (what taffy's flex/grid call) forwards as
  `compute_child_layout_ext(input.into())`. Linear/Relative call the ext
  method directly and thread real definiteness.
- **The cache stays hughie's**, retyped: key = the full `LayoutInputExt`
  (`run_mode`, `axis`, `sizing_mode`, `known_dimensions`,
  `definite_dimensions`, `parent_size`, `available_space`;
  `vertical_margins_are_collapsible` is excluded — always default without
  block layout). Slot structure, commit-satisfies-`Measure(Both)`
  equivalence, shape-aware replacement, and `committed_input()` retrieval
  all survive; `committed_input` is what parked-boundary relayout replays,
  and taffy's own `Cache` cannot return it (private lossy packed keys) nor
  key on the ext bits — which is why we do not adopt it. `CacheTree` for
  taffy's algorithms normalizes through the `From` impl above, so taffy- and
  hughie-initiated probes share one per-node cache coherently.
- At the boundary between vocabularies the bits degrade exactly once, by
  design: a Linear parent that passes known-but-indefinite to a **flex/grid**
  child loses the distinction inside that subtree (taffy's model applies
  there). Linear/relative/leaf subtrees keep Starlight behavior. Recorded as
  delta D2 (§10).

Vocabulary mapping (mechanical, applied throughout hughie):
`LayoutGoal::Commit` → `RunMode::PerformLayout`; `LayoutGoal::Measure(axis)`
→ `RunMode::ComputeSize` + `LayoutInput.axis`; `SizingMode::ApplySizeStyles`
→ `SizingMode::InherentSize`; `IgnoreSizeStyles` → `ContentSize`;
`AvailableSpace`/geometry types → taffy's. hughie `Edges` → `taffy::Rect`.

## 6. Style views: two vocabularies, one mechanism, zero materialization

`StyleView<'dom, T>` (node + post-flush `ComputedValues` pointer, two words)
grows a second set of trait impls. No `taffy::Style` is ever materialized —
unlike Blitz's eager path, which re-converts every node every frame, every
accessor converts on read (`#[inline]`, branch-on-tag, zero allocation):

- `taffy::CoreStyle`: `box_generation_mode` (`None` for `display:none`;
  `Contents` never reaches taffy), `box_sizing`, `direction` (fork
  `direction` + host `lynx-rtl` lowering — Blitz's lazy wrapper omits this;
  we must not), `overflow` (fork grammar → `Visible/Hidden/Scroll/Clip`
  1:1), `scrollbar_width` = 0 (Lynx overlay scrollbars), `position` (§7),
  `inset/size/min_size/max_size/margin/padding/border` via
  `hughie::style::convert` (calc = `CompactLength::calc(ptr)` tagged
  pointer; used border widths keep the none/hidden-style zeroing),
  `aspect_ratio`.
- `taffy::FlexboxContainerStyle/ItemStyle`, `GridContainerStyle/ItemStyle`:
  direct conversions; physical `left/right` alignment keywords are resolved
  against `direction` at conversion time (as `stylo_taffy` does); grid
  template lists are lending iterators over the fork's
  `GridTemplateComponent` (the `GenericRepetition`/`TemplateLineNames` GAT
  wrappers, modeled on `stylo_taffy/src/wrapper.rs`). `CustomIdent =
  stylo::Atom`; if the fork grammar does not parse named lines/areas the
  accessors return `None` at zero cost.
- `hughie::LynxCoreStyle` (the trimmed successor of today's `CoreStyle`,
  same defaulted-accessor macro over `computed_values()`): the box set that
  Linear/Relative/leaf/absolute/containment actually read — raw stylo
  values, because those algorithms need `has_percentage()`, `is_auto()`,
  intrinsic-keyword variants, and `fit-content()` payloads that taffy's
  `Dimension` cannot represent — plus `display`, `position`, `order`,
  `direction`, `overflow`, containment accessors, and the `linear-*` /
  `relative-*` sets with their existing logical→physical lowering. The
  flex/grid accessor block is deleted. `TextContainerStyle`/`TextRunStyle`
  are unchanged.

calc lifetime discipline is unchanged from today's pointer publication rule
and identical to Blitz's: the tagged pointer aims into the post-flush
`ComputedValues` kept alive by the element's primary `Arc`; the next
exclusive style traversal cannot begin until layout releases `&mut Document`;
caches store only `f32` results, never `CompactLength`, so no stale pointer
survives a flush.

## 7. Dispatch, containment, and out-of-flow

`compute_child_layout_ext` — the single dispatch, replacing today's
`compute_layout`:

```text
text node                → cached( Parley TextMeasurer path )            (leaf)
display: none, or
run_mode = PerformHiddenLayout
                         → hide_subtree; LayoutOutput::HIDDEN            (outside cache)
skips_contents           → compute_skipped_contents_layout               (outside cache)
replaced element         → cached( natural-size leaf path )
otherwise, cached(ext):
    SIZE containment?    → ComputeSize: substituted empty-box sizing
                           (contain-intrinsic + clamps), no descent;
                           PerformLayout: fill each unknown axis of
                           known_dimensions with the substituted size,
                           then run the algorithm below
    Flex                 → taffy::compute_flexbox_layout
    Grid                 → taffy::compute_grid_layout
    Linear               → hughie::compute_linear_layout
    Relative             → hughie::compute_relative_layout
    Flow (leaf)          → natural-size leaf path
  then LAYOUT containment output rewrite:
    first_baselines = NONE; own_scrollable_overflow (content_size collapses
    to the border box unless the box is a scroll container)
```

Containment therefore moves from inside the algorithms (where hughie
implemented it) to the dispatch wrapper — the only place it can live once
flex/grid are taffy's. The semantics are unchanged: substitution happens at
the child's own sizing layer, so parents probing a size-contained child see
the substituted answer; children are still laid out under `PerformLayout`
and still contribute scrollable overflow; scroll-container trapping inside
taffy already matches our `accumulate_scrollable_overflow` rule. The
relayout-boundary predicate, `invalidate_for_relayout`, the
`Document::invalidate_layout` funnel, parked-boundary dedup, deepest-first
re-runs via `compute_boundary_relayout(committed_input)`, the idle-frame
skip, and the boundary-scoped fused tail are all **unchanged** (retyping
only) — taffy's explicit-input compute functions support re-entry from any
node, which is precisely what the parked-boundary mechanism needs.

**Absolute (CB = layout parent)**: taffy handles fully inside flex/grid,
including the static-position fallback for auto insets — this replaces
hughie's per-algorithm §9.8/§10.2 static-position code for flex/grid.
Linear/Relative keep their own absolute handling through the retained
`compute_absolute_layout` (which also keeps the RTL `prefer_end` rule and
the measure-goal variant Linear's gravity equation needs).

**Fixed (hoisted, CB ≠ parent)**: `StyleView::position()` for taffy maps
resolved `Fixed` to `taffy::Position::Absolute`, so the parent computes a
static-position-informed layout for it (against its own padding box — the
same basis hughie's `measure_absolute_layout` used for gravity). The
positioned pass keeps everything else: `pre_position` detects resolved
`Fixed`, resolves the true containing block (transform / perspective /
offset-path / will-change / filter / containment ancestors — predicates
unchanged), derives the static position from the taffy-written location
(margin-box origin = `location − margin`, valid exactly on the auto-inset
axes where the static position is consumed), re-runs
`compute_absolute_layout` against the true CB, converts back to
formatting-parent space, and re-stamps `Layout.order` with the effective-0
rank (now a plain index lookup in the sorted layout child list). The
explicit `set_static_position` channel remains only for Linear/Relative
hoisted children (Linear's gravity-aligned static position has no taffy
counterpart). Cost note: a hoisted-fixed child's subtree is now laid out
once by taffy in-parent and once by the positioned pass; sizes agree unless
CB-relative percentages are involved, so the second pass is mostly cache
hits, and hoisted-fixed elements are rare.

**Leaf/text**: taffy's `compute_leaf_layout` is **not** used — its measure
closure returns only `Size<f32>`, which would drop text baselines (Linear
consumes measure-time baselines; flex baseline alignment needs them at
commit). hughie's leaf engine (`compute_leaf_layout_with_measurement`) and
the Parley `TextMeasurer` (probe/commit artifact store, rebreak, shaping
reuse) survive with retyped IO. Same for replaced `NaturalSize` leaves.

**Rounding**: taffy's `round_layout` is not used (integer CSS px, no DPR,
origin pinned at (0,0)). The DPR-aware, CSS round-half-up,
cumulative-error-free `round_layout_subtree_with` with its `pre_position`
hook — including the boundary-scoped incremental tail — is retained,
retyped over `taffy::Layout`. We do not implement `RoundTree`/`PrintTree`;
nothing calls them.

**Root**: hughie's `compute_root_layout` is kept (root margin resolution +
definite bits + display-none root) rather than taffy's, so the driving
sequence in `run_layout` is unchanged.

## 8. What gets deleted, kept, rewritten

| Unit | Fate |
| --- | --- |
| `hughie/src/compute/flexbox.rs`, `compute/grid/*` (~8.5k lines) | **Deleted** — taffy provides |
| `hughie/src/tree/{mod,io}.rs` (`LayoutTree`, `LayoutInput/Output/Layout`, `FlattenedChildren`) | **Deleted** — taffy types + `LayoutLynxTree` + `LayoutInputExt`; `LayoutSlot` moves next to the cache |
| `hughie/src/geometry.rs` | **Deleted** — taffy geometry (`Edges`→`Rect`) |
| `hughie/src/style/mod.rs` `CoreStyle` | **Trimmed** to `LynxCoreStyle` (box core + display/position/order/direction/overflow + containment + linear-*/relative-*); flex/grid accessors deleted; + new `style::convert` (stylo fork → taffy values, modeled on `stylo_taffy`) |
| `hughie/src/cache.rs` | **Kept**, keyed on `LayoutInputExt` |
| `hughie/src/invalidate.rs`, containment helpers, `compute/{mod,leaf,linear,relative,single_axis,util}.rs`, `text/*` | **Kept**, retyped; linear/relative restructured to `&mut tree` (their scratch records already copy what crosses child calls; style views survive on the arena lifetime, §3) — probe patterns, memoization, CSR solver, freeze loops, and allocation structure preserved verbatim |
| `dom/src/layout/host.rs` | **Rewritten**: `LayoutCtx` + taffy trait impls + ext dispatch + containment wrapper; `run_layout`/parked-boundary/positioned-pass logic retained with the §7 fixed-hoisting change |
| `dom/src/layout/style.rs` | **Extended**: taffy trait impls on `StyleView`; `resolve_position`/CB predicates unchanged |
| dom child storage | **New**: layout child lists (flatten + order sort) with flush-time rebuild |
| `dom` public API (`Document::layout`, `rounded_layout`, `set_natural_size`, `register_fonts`, scroll/visual consumers) | **Unchanged signatures**; `dom::layout::Layout` is now `taffy::Layout` (gains `scrollbar_size`, loses nothing consumed today) |

Dependency edges after: `dom → { hughie, taffy, stylo }`,
`hughie → { taffy, stylo, parley }`. Nothing outside dom touches hughie
types except through existing dom re-exports (verified: only `FontBlob`,
`Layout`, `Size`, `NaturalSize`).

## 9. Performance architecture (requirement 7)

- **Static dispatch end-to-end is preserved.** taffy's compute functions are
  generic over the concrete `LayoutCtx`; every host⇄taffy⇄hughie call
  monomorphizes exactly as today. No `dyn`, no vtables.
- **Zero style materialization** — the deliberate divergence from Blitz's
  production path (which builds a 520-byte `taffy::Style` per node per
  frame). Accessor-level conversion is a field read + tag branch; calc is a
  pointer tag. Grid template conversion is lazy lending iterators.
- **Caching**: same 8+1 slot structure, same full-input keying (already
  stricter than taffy 0.12's corrected key, so none of taffy's ~10%
  key-correctness cost is new to us), same commit/measure equivalence, plus
  the ext bits. Warm-path asymptotics (parked boundaries, idle-frame O(1),
  boundary-scoped fused tail) are untouched.
- **Allocation discipline**: Linear/Relative scratch, the CSR solver, freeze
  loops, and measurement memos are preserved unchanged; the new layout child
  lists allocate only on nodes that need them, at flush time, never during
  layout; taffy's flex/grid allocate transient per-pass scratch comparable
  to the deleted implementations.
- **Measured claims stay falsifiable**: the divan/CodSpeed suites enter
  through `Document::layout` and survive structurally (benchmark history
  resets — flex/grid numbers become taffy's). The
  `containment.rs` bench target retypes with the protocol. A/B comparison
  against the pre-migration commit on the flex/grid suites is part of the
  acceptance gate; regressions localized to taffy internals get upstream
  issues rather than local forks, unless severe (then vendor+patch, §2).

## 10. Recorded behavior deltas (to land in `docs/tracking/deviations.md` and a rewritten `docs/layout-conformance.md`)

- **D1 — intrinsic sizing keywords in flex/grid.** `min-content` /
  `max-content` / `fit-content()` as width/height of flex/grid boxes
  collapse to `auto` (taffy `Dimension` cannot express them; Blitz ships the
  same collapse). Linear, Relative, and leaf boxes keep full support. This
  is the largest functional regression in the design; if Lynx content
  depends on it inside flex/grid, the remedy is an upstream taffy
  contribution (or vendor patch), not a local reimplementation.
- **D2 — §9.8 known-but-indefinite inside taffy subtrees.** Flex/grid treat
  parent-decided sizes as definite percentage bases (taffy's model);
  Starlight definiteness survives across linear/relative/leaf edges via
  `LayoutInputExt`. Existing tests pinning the flex-side nuance
  (`indefinite_percentage_flex_basis_falls_back_to_content_not_width`,
  auto-repeat seeding) re-pin to taffy's behavior or are dropped with a
  deviation note.
- **D3 — grid details.** taffy's bounding policies replace hughie's
  (track-limit drop, hostile auto-repeat counting); named lines/areas become
  available if the fork grammar supplies them; grid baseline
  group/shim/synthesis details re-verified against taffy (its abs-pos grid
  handling was reworked in 0.13 and is current-spec-aligned).
- **D4 — paint-order mechanism.** `Layout.order` semantics are preserved
  (order-modified document order, effective 0 for out-of-flow) but are now
  produced by dom's sorted child lists + taffy's index stamping instead of
  `sort_and_assign_layout_order`, which is deleted from Linear/Relative.
- **D5 — cache/probe traces.** Tests and benches asserting hughie flex/grid
  probe counts, measure-input traces, or write discipline die with those
  implementations; geometry oracles are retargeted (see the migration test
  classification: ~120 of the 137 flex/grid tests survive as conformance
  oracles against taffy, ~10 are algorithm-internal and die, protocol tests
  retype, all 113 linear/relative tests survive with retyping).
- **D6 — `content_size` origin.** taffy 0.13 measures `content_size` from
  the padding-box origin (#1051); verify `scroll::resolve` and boundary
  content-size merge-back against it during implementation (hughie's
  convention must be confirmed equal or the reader adjusted).

## 11. Migration order

1. **Types + conversion floor** — workspace `taffy` dep; hughie:
   `LayoutInputExt`, retyped cache/invalidate/geometry aliases,
   `style::convert` + `LynxCoreStyle` trim, trait definitions
   (`LayoutLynxTree`, container traits). Everything still compiles with the
   old algorithms behind a temporary shim, or this step lands together with
   2–4 on the integration branch.
2. **dom child lists** — layout children (flatten + order sort), rebuild
   triggers in flush/mutation paths, retarget `visual`/`sibling_paint_order`
   iteration. Independently testable against today's engine (the
   `FlattenedChildren` outputs must match).
3. **Host cutover** — `LayoutCtx`, taffy trait impls, ext dispatch +
   containment wrapper, leaf/text retyping, taffy flex/grid live,
   linear/relative ported to `&mut` protocol, positioned-pass rework,
   rounding retype; delete hughie flex/grid/tree/geometry.
4. **Test/bench migration** per the classification above; conformance
   re-verification list from `docs/layout-conformance.md` (RTL, baselines,
   automatic minimum, abs-pos fallbacks, auto-repeat, rounding interplay,
   content_size); CodSpeed A/B.
5. **Docs**: `layout-architecture.md` rewrite (ownership table §2 here),
   `layout-conformance.md` re-baseline to taffy's tracked spec drafts,
   tracking/deviations updates, `AGENTS.md` crate description.

Steps 3–5 are one atomic behavior change on this branch (no stacked PRs);
steps 1–2 can merge first as no-behavior-change preparation if desired.

## 12. Open questions

- Does the fork grammar parse named grid lines/areas (and should the adapter
  wire them), or do the accessors return `None` for now?
- Does any Lynx corpus rely on intrinsic sizing keywords on flex/grid boxes
  (D1)? If yes, scope the upstream taffy contribution early.
- `content_size` origin audit (D6).
- Whether to enable `detailed_layout_info` for debugging parity with Blitz
  (off by default in our feature set; zero cost when unused).
