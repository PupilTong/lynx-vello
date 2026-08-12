# Research appendix: Taffy layout integration

Raw research reports behind [taffy-layout-integration.md](taffy-layout-integration.md).
Produced 2026-08-12 by six parallel research agents; the two reports about
external code (taffy, Blitz) were additionally re-verified claim-by-claim
against primary sources (docs.rs, GitHub source at pinned revisions) by
dedicated verification agents, whose corrections are already applied in the
text below (each ends with a "Corrections and confidence" section).

Contents:

- [Taffy 0.13.0 low-level custom-tree API](#taffy-0130-low-level-custom-tree-api)
- [Blitz / stylo_taffy prior art](#blitz-stylo_taffy-prior-art)
- [hughie protocol surface inventory](#hughie-protocol-surface-inventory)
- [dom layout host inventory](#dom-layout-host-inventory)
- [Linear/Relative protocol requirements](#linearrelative-protocol-requirements)
- [Test/bench/docs migration impact inventory](#testbenchdocs-migration-impact-inventory)

---

# Taffy 0.13.0 low-level custom-tree API

> Provenance: Verified against a clone of the v0.13.0 tag, crates.io API, and docs.rs; a second adversarial verification pass re-checked every load-bearing claim and recorded its corrections in-line.

All claims checked. Every load-bearing signature, struct, enum, cache detail, changelog item, and the Blitz snippets verified against the cloned v0.13.0 tag, crates.io API, and raw.githubusercontent.com. Five corrections needed (one broken URL, one wrong count, three precision fixes). Here is the corrected report.

## Taffy 0.13.0 low-level custom-tree API — research report

All code claims verified against a clone of the `v0.13.0` tag (github.com/DioxusLabs/taffy). File references below are paths within that tag; permalink form is `https://github.com/DioxusLabs/taffy/blob/v0.13.0/<path>`. Note: there is **no `RELEASES.md`** in the repo (404); the release log is `CHANGELOG.md`.

### 1. Versions and releases

Latest published: **0.13.0, released 2026-08-08** (crates.io API: https://crates.io/crates/taffy). MSRV 1.71. Recent versions: 0.12.2 (2026-07-15), 0.12.1 / 0.12.0 (2026-07-03), 0.11.0 (2026-06-12), 0.10.1 (2026-04-14), 0.10.0 (2026-03-31), 0.9.2 (2025-11-22), 0.9.1 (2025-08-20), 0.9.0 (2025-08-07).

Headlines after 0.9.x (source: `CHANGELOG.md`, https://github.com/DioxusLabs/taffy/blob/main/CHANGELOG.md):
- **0.10.0**: `direction` property (RTL for block/flex/grid); `float`/`clear` support (new `float_layout` feature; `FloatContext`, `BlockContext` shared across a block formatting context).
- **0.10.1**: grid auto-repeat and minimum-size fixes (#946).
- **0.11.0**: safe alignment keywords — alignment types became structs of `AlignmentKeyword` + `AlignmentSafety`; enum variants (`AlignContent::Start`) replaced by associated consts (`AlignContent::START`). Grid item percentages now resolve against the grid area, not the container (#960).
- **0.12.0**: block `align-content`; **cache-key correctness change** — key now includes axis, parent size, and available space (~10% perf cost in common cases, ~60% in pathological ones, needed for correctness, #911); block aspect-ratio definite-height derivation (#965).
- **0.12.1/0.12.2**: critical block-layout caching fixes (run_mode passthrough, deferred layout commits, margin-collapse outputs from ComputeSize).
- **0.13.0**: `self-start`/`self-end` alignment keywords; `Display::FlowRoot`; numeric style helpers take `Into<f64>`; `grid_template_areas` became `Option<GridTemplateAreas<S>>` with explicit `row_count`/`column_count`; `BlockContext::place_floated_box` gained `adjoins_unresolved_strut: bool`; ~30 flexbox/grid/block/float conformance fixes (29 bullets in the Fixed section; notably abspos static-position and auto-margin handling #1072, grid abspos track-edge resolution #1071, content_size measured from padding-box origin #1051).

### 2. Low-level trait inventory (`src/tree/traits.rs`, `src/tree/node.rs`)

`NodeId` is `pub struct NodeId(u64)` — `Copy + Clone + PartialEq + Eq + Hash + Debug`, `const fn new(u64)`, `From<u64>/From<usize>` both ways (plus slotmap `DefaultKey` conversions). Docs: https://docs.rs/taffy/latest/taffy/tree/struct.NodeId.html

```rust
pub trait TraversePartialTree {
    type ChildIter<'a>: Iterator<Item = NodeId> where Self: 'a;
    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_>;
    fn child_count(&self, parent_node_id: NodeId) -> usize;
    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId;
}

pub trait TraverseTree: TraversePartialTree {}   // marker: recursion guarantee

pub trait LayoutPartialTree: TraversePartialTree {
    type CoreContainerStyle<'a>: CoreStyle<CustomIdent = Self::CustomIdent> where Self: 'a;
    type CustomIdent: CheapCloneStr;             // string type for named grid lines/areas
    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_>;
    fn resolve_calc_value(&self, val: *const (), basis: f32) -> f32 { 0.0 }  // default impl
    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout);
    fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput;
}

pub trait CacheTree {
    fn cache_get(&self, node_id: NodeId, input: &LayoutInput) -> Option<LayoutOutput>;
    fn cache_store(&mut self, node_id: NodeId, input: &LayoutInput, layout_output: LayoutOutput);
    fn cache_clear(&mut self, node_id: NodeId);
}

pub trait RoundTree: TraverseTree {
    fn get_unrounded_layout(&self, node_id: NodeId) -> Layout;
    fn set_final_layout(&mut self, node_id: NodeId, layout: &Layout);
}

pub trait PrintTree: TraverseTree {
    fn get_debug_label(&self, node_id: NodeId) -> &'static str;
    fn get_final_layout(&self, node_id: NodeId) -> Layout;
}

#[cfg(feature = "flexbox")]
pub trait LayoutFlexboxContainer: LayoutPartialTree {
    type FlexboxContainerStyle<'a>: FlexboxContainerStyle where Self: 'a;
    type FlexboxItemStyle<'a>: FlexboxItemStyle where Self: 'a;
    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_>;
    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_>;
}

#[cfg(feature = "grid")]
pub trait LayoutGridContainer: LayoutPartialTree {
    type GridContainerStyle<'a>: GridContainerStyle<CustomIdent = Self::CustomIdent> where Self: 'a;
    type GridItemStyle<'a>: GridItemStyle<CustomIdent = Self::CustomIdent> where Self: 'a;
    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_>;
    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_>;
    #[cfg(feature = "detailed_layout_info")]
    fn set_detailed_grid_info(&mut self, _node_id: NodeId, _info: DetailedGridInfo) { ... }  // optional
}

#[cfg(feature = "block_layout")]
pub trait LayoutBlockContainer: LayoutPartialTree {
    type BlockContainerStyle<'a>: BlockContainerStyle where Self: 'a;
    type BlockItemStyle<'a>: BlockItemStyle where Self: 'a;
    fn get_block_container_style(&self, node_id: NodeId) -> Self::BlockContainerStyle<'_>;
    fn get_block_child_style(&self, child_node_id: NodeId) -> Self::BlockItemStyle<'_>;
    fn compute_block_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>) -> LayoutOutput
    { self.compute_child_layout(node_id, inputs) }   // default forwards, dropping the BFC ctx
}
```

Caveat: the module-level doc comment in `traits.rs` (and some docs.rs prose) still shows an old `LayoutPartialTree` with `get_style`/`get_cache_mut`; the real trait is as above — caching moved to the separate `CacheTree` trait. There is no upward traversal in any trait.

### 3. Style trait inventory (`src/style/mod.rs`, `style/flex.rs`, `style/block.rs`, `style/grid.rs`)

Style-trait methods have default implementations returning `Style::DEFAULT` values, so hosts override only what they support — with the exception of `GridContainerStyle`, where the template/auto-track/areas/line-name getters (marked "required" below) and the associated types have no defaults and must be implemented.

```rust
pub trait CheapCloneStr:                       // with alloc/std; empty trait without
    AsRef<str> + for<'a> From<&'a str> + From<String> + PartialEq + Eq + Clone + Default + Debug + 'static {}

pub trait CoreStyle {
    type CustomIdent: CheapCloneStr;
    fn box_generation_mode(&self) -> BoxGenerationMode;   // Normal | None
    fn is_block(&self) -> bool;                            // true ONLY for display:block, not flow-root
    fn is_compressible_replaced(&self) -> bool;
    fn box_sizing(&self) -> BoxSizing;                     // BorderBox | ContentBox
    fn direction(&self) -> Direction;                      // Ltr | Rtl
    fn overflow(&self) -> Point<Overflow>;                 // Visible | Clip | Hidden | Scroll
    fn scrollbar_width(&self) -> f32;
    fn position(&self) -> Position;                        // Relative | Absolute (only)
    fn inset(&self) -> Rect<LengthPercentageAuto>;
    fn size(&self) -> Size<Dimension>;
    fn min_size(&self) -> Size<Dimension>;
    fn max_size(&self) -> Size<Dimension>;
    fn aspect_ratio(&self) -> Option<f32>;                 // width / height
    fn margin(&self) -> Rect<LengthPercentageAuto>;
    fn padding(&self) -> Rect<LengthPercentage>;
    fn border(&self) -> Rect<LengthPercentage>;
}

pub trait FlexboxContainerStyle: CoreStyle {
    fn flex_direction(&self) -> FlexDirection;
    fn flex_wrap(&self) -> FlexWrap;
    fn gap(&self) -> Size<LengthPercentage>;
    fn align_content(&self) -> Option<AlignContent>;
    fn align_items(&self) -> Option<AlignItems>;
    fn justify_content(&self) -> Option<JustifyContent>;
}
pub trait FlexboxItemStyle: CoreStyle {
    fn flex_basis(&self) -> Dimension;
    fn flex_grow(&self) -> f32;
    fn flex_shrink(&self) -> f32;
    fn align_self(&self) -> Option<AlignSelf>;
}

pub trait BlockContainerStyle: CoreStyle {
    fn text_align(&self) -> TextAlign;                     // Auto | LegacyLeft | LegacyRight | LegacyCenter
    fn align_content(&self) -> Option<AlignContent>;
}
pub trait BlockItemStyle: CoreStyle {
    fn is_table(&self) -> bool;
    #[cfg(feature = "float_layout")] fn float(&self) -> Float;   // Left | Right | None
    #[cfg(feature = "float_layout")] fn clear(&self) -> Clear;   // Left | Right | Both | None
}

pub trait GridContainerStyle: CoreStyle {
    type Repetition<'a>: GenericRepetition<CustomIdent = Self::CustomIdent> where Self: 'a;
    type TemplateTrackList<'a>: Iterator<Item = GenericGridTemplateComponent<Self::CustomIdent, Self::Repetition<'a>>>
        + ExactSizeIterator + Clone where Self: 'a;
    type AutoTrackList<'a>: Iterator<Item = TrackSizingFunction> + ExactSizeIterator + Clone where Self: 'a;
    type TemplateLineNames<'a>: TemplateLineNames<'a, Self::CustomIdent> where Self: 'a;
    type GridTemplateAreas<'a>: IntoIterator<Item = GridTemplateArea<Self::CustomIdent>> where Self: 'a;

    fn grid_template_rows(&self) -> Option<Self::TemplateTrackList<'_>>;      // required
    fn grid_template_columns(&self) -> Option<Self::TemplateTrackList<'_>>;   // required
    fn grid_auto_rows(&self) -> Self::AutoTrackList<'_>;                      // required
    fn grid_auto_columns(&self) -> Self::AutoTrackList<'_>;                   // required
    fn grid_template_areas(&self) -> Option<Self::GridTemplateAreas<'_>>;     // required
    fn grid_template_area_row_count(&self) -> u16;      // default derives from areas
    fn grid_template_area_column_count(&self) -> u16;   // default derives from areas
    fn grid_template_column_names(&self) -> Option<Self::TemplateLineNames<'_>>;  // required
    fn grid_template_row_names(&self) -> Option<Self::TemplateLineNames<'_>>;     // required
    fn grid_auto_flow(&self) -> GridAutoFlow;           // Row | Column | RowDense | ColumnDense
    fn gap(&self) -> Size<LengthPercentage>;
    fn align_content(&self) -> Option<AlignContent>;
    fn justify_content(&self) -> Option<JustifyContent>;
    fn align_items(&self) -> Option<AlignItems>;
    fn justify_items(&self) -> Option<AlignItems>;
    // provided helpers: grid_template_tracks(AbsoluteAxis), grid_align_content(AbstractAxis)
}
pub trait GridItemStyle: CoreStyle {
    fn grid_row(&self) -> Line<GridPlacement<Self::CustomIdent>>;
    fn grid_column(&self) -> Line<GridPlacement<Self::CustomIdent>>;
    fn align_self(&self) -> Option<AlignSelf>;
    fn justify_self(&self) -> Option<AlignSelf>;
    fn grid_placement(&self, axis: AbsoluteAxis) -> Line<GridPlacement<Self::CustomIdent>>;  // provided
}
```

Grid track/name typing: `pub type TrackSizingFunction = MinMax<MinTrackSizingFunction, MaxTrackSizingFunction>` where both halves are newtypes over `CompactLength` (8 bytes each). Template track lists are **lending associated iterator types**, not slices: `GenericGridTemplateComponent<S, Rep>` is `Single(TrackSizingFunction) | Repeat(Rep)`; `GenericRepetition` exposes `count() -> RepetitionCount` (`AutoFill | AutoFit | Count(u16)`), `tracks()`, `lines_names()`. Line names are a nested iterator trait `TemplateLineNames<'a, S>: Iterator<Item = Self::LineNameSet<'a>>` with `LineNameSet<'b>: Iterator<Item = &'b S>`. Named strings use the tree-level `CustomIdent: CheapCloneStr` associated type (recommendation in-source: `Arc<str>` or `string_cache::Atom`). `GridPlacement<S>` = `Auto | Line(GridLine) | NamedLine(S, i16) | Span(u16) | NamedSpan(S, u16)`. `GridTemplateAreas<S>` = `{ areas: GridTrackVec<GridTemplateArea<S>>, row_count: u16, column_count: u16 }`; `GridTemplateArea<S>` = `{ name: S, row_start/row_end/column_start/column_end: u16 }`. Concrete `Style<S: CheapCloneStr = DefaultCheapStr>` implements every trait; measured sizes: `Style<Arc<str>>` = 520 bytes, `Style<String>` = 552 (size test in `style/mod.rs`).

### 4. LayoutInput / LayoutOutput / RunMode / SizingMode / RequestedAxis (`src/tree/layout.rs`)

```rust
pub enum RunMode { PerformLayout, ComputeSize, PerformHiddenLayout }
pub enum SizingMode { ContentSize, InherentSize }
pub enum RequestedAxis { Horizontal, Vertical, Both }

pub struct LayoutInput {                       // Copy + Clone + PartialEq
    pub run_mode: RunMode,
    pub sizing_mode: SizingMode,
    pub axis: RequestedAxis,
    pub known_dimensions: Size<Option<f32>>,
    pub parent_size: Size<Option<f32>>,        // "intended to be used for percentage resolution"
    pub available_space: Size<AvailableSpace>, // Definite(f32) | MinContent | MaxContent
    pub vertical_margins_are_collapsible: Line<bool>,
}
// LayoutInput::HIDDEN const provided.

pub struct LayoutOutput {                      // Copy + Clone
    pub size: Size<f32>,
    #[cfg(feature = "content_size")] pub content_size: Size<f32>,
    pub first_baselines: Point<Option<f32>>,   // first baseline only; Point::NONE if none
    pub top_margin: CollapsibleMarginSet,      // { positive: f32, negative: f32 } (private fields)
    pub bottom_margin: CollapsibleMarginSet,
    pub margins_can_collapse_through: bool,
}
// consts HIDDEN/DEFAULT; ctors from_sizes_and_baselines / from_sizes / from_outer_size.
```

**No separate percentage-definiteness flag.** The percentage basis is exactly `parent_size: Size<Option<f32>>`; `None` = indefinite, so percentages resolve to `None`/`0` per call site. Flexbox §9.8-style definiteness is communicated only through `Option`-ness of `known_dimensions`/`parent_size` — a container makes a child's percentage basis definite by passing `Some` in `parent_size` when re-laying it out. Since 0.11, grid passes the item's **grid area** size as the child's `parent_size` (CHANGELOG #960). Calc resolution also receives this basis (`resolve_calc_value(ptr, basis)`).

### 5. Compute function signatures (`src/compute/*`)

```rust
// compute/flexbox.rs
pub fn compute_flexbox_layout(tree: &mut impl LayoutFlexboxContainer, node: NodeId, inputs: LayoutInput) -> LayoutOutput;

// compute/grid/mod.rs
pub fn compute_grid_layout<Tree: LayoutGridContainer>(tree: &mut Tree, node: NodeId, inputs: LayoutInput) -> LayoutOutput;

// compute/block.rs
pub fn compute_block_layout(tree: &mut impl LayoutBlockContainer, node_id: NodeId, inputs: LayoutInput,
    block_ctx: Option<&mut BlockContext<'_>>) -> LayoutOutput;

// compute/leaf.rs — note: does NOT take a tree; takes style + calc resolver directly
pub fn compute_leaf_layout<MeasureFunction>(
    inputs: LayoutInput,
    style: &impl CoreStyle,
    resolve_calc_value: impl Fn(*const (), f32) -> f32,
    measure_function: MeasureFunction,
) -> LayoutOutput
where MeasureFunction: FnOnce(Size<Option<f32>>, Size<AvailableSpace>) -> Size<f32>;

// compute/mod.rs
pub fn compute_root_layout(tree: &mut impl LayoutPartialTree, root: NodeId, available_space: Size<AvailableSpace>);
pub fn compute_cached_layout<Tree: CacheTree + ?Sized, ComputeFunction>(
    tree: &mut Tree, node: NodeId, inputs: LayoutInput, compute_uncached: ComputeFunction) -> LayoutOutput
where ComputeFunction: FnOnce(&mut Tree, NodeId, LayoutInput) -> LayoutOutput;
pub fn compute_hidden_layout(tree: &mut (impl LayoutPartialTree + CacheTree), node: NodeId) -> LayoutOutput;
pub fn round_layout(tree: &mut impl RoundTree, node_id: NodeId);
```

Also exported: `BlockContext<'bfc>`, `BlockFormattingContext`, and (float_layout) `FloatContext`, `BfcSlot`, `ContentSlot`, `FloatIntrinsicWidthCalculator`. `BlockContext::place_floated_box(&mut self, floated_box: Size<f32>, min_y: f32, direction: FloatDirection, clear: Clear, adjoins_unresolved_strut: bool) -> Point<f32>`. `compute_root_layout` handles block-root width-stretching, RTL root placement, and writes the root's `Layout` itself.

### 6. `taffy::Cache` (`src/tree/cache.rs`)

```rust
pub struct Cache {                                   // Clone (not Copy), Default
    final_layout_entry: Option<CacheEntry<LayoutOutput>>,   // private
    measure_entries: [Option<CacheEntry<Size<f32>>>; 9],    // private; CACHE_SIZE = 9
    is_empty: bool,
}
pub const fn new() -> Self;
pub fn get(&self, input: &LayoutInput) -> Option<LayoutOutput>;
pub fn store(&mut self, input: &LayoutInput, layout_output: LayoutOutput);
pub fn clear(&mut self) -> ClearState;               // ClearState = Cleared | AlreadyEmpty
pub fn is_empty(&self) -> bool;
```

- **Slots**: 1 `PerformLayout` slot (full `LayoutOutput`) + 9 `ComputeSize` slots (size only). Slot selection: slot 0 = both known dimensions set; slots 1–4 = one known dimension × (MinContent vs MaxContent/Definite on the other axis); slots 5–8 = no known dimensions × the 2×2 of MinContent vs MaxContent/Definite per axis.
- **Keying**: private packed `CacheKey { kd_available_space: u64, parent_size: u64 }` — per axis, `known_dimensions` bits if set else the `available_space` encoding (Definite stored negated; MinContent/MaxContent as ±inf bit patterns); `RequestedAxis` packed into the two sign bits of `parent_size`. `ComputeSize` hits compare only `kd_available_space` plus the x-axis half of `parent_size`.
- **Retrieving the committed final layout's `LayoutInput`: NO.** The key is a lossy packed encoding, fields are private, and no accessor exposes entry keys or the final-layout entry. A host that needs "the inputs of the committed layout" must record them itself in `compute_child_layout`/`set_unrounded_layout`.

### 7. calc() support

- Representation: `Dimension`, `LengthPercentage`, `LengthPercentageAuto` are all newtypes over `CompactLength` (`src/style/compact_length.rs`), an 8-byte tagged value: tag in the low bits (`CALC_TAG = 0b000`, `LENGTH_TAG`, `PERCENT_TAG`, `AUTO_TAG`, `FR_TAG`, `MIN_CONTENT_TAG`, `MAX_CONTENT_TAG`, `FIT_CONTENT_PX_TAG`, `FIT_CONTENT_PERCENT_TAG`). A calc value is stored as a **tagged pointer**: `pub fn calc(ptr: *const ()) -> Self` — "treated as an opaque handle to the actual calc representation and may be a pointer, index, etc. The low 3 bits are used as a tag value and will be returned as 0" (so the handle must be 8-aligned or an index shifted left 3; `CompactLength::calc` asserts non-null and 8-aligned). Accessors on `CompactLength`: `tag()`, `value()`, `calc_value() -> *const ()`, `is_calc()`; the escape hatches `unsafe from_raw(CompactLength)` / `into_raw()` live on the wrapper types (`Dimension`, `LengthPercentage`, `LengthPercentageAuto`), not on `CompactLength` itself.
- Resolver: carried by **`LayoutPartialTree`**: `fn resolve_calc_value(&self, val: *const (), basis: f32) -> f32` (default returns `0.0`). `compute_leaf_layout` instead takes the resolver as a direct `impl Fn(*const (), f32) -> f32` parameter. All internal resolution paths thread a `calc_resolver` closure (e.g. `LengthPercentageAuto::resolve_to_option(self, context: f32, calc_resolver: impl Fn(*const (), f32) -> f32)`).
- Stylo-backed host practice (Blitz): `stylo_taffy` converts stylo values by `CompactLength::calc(calc_ptr as *const stylo::CalcLengthPercentage as *const ())` then `unsafe { taffy::LengthPercentage::from_raw(val) }` (https://github.com/DioxusLabs/blitz/blob/main/packages/stylo_taffy/src/convert.rs); `blitz-dom` implements the resolver as, verbatim (https://github.com/DioxusLabs/blitz/blob/main/packages/blitz-dom/src/layout/mod.rs):
  ```rust
  pub(crate) fn resolve_calc_value(calc_ptr: *const (), parent_size: f32) -> f32 {
      let calc = unsafe { &*(calc_ptr as *const CalcLengthPercentage) };
      let result = calc.resolve(CSSPixelLength::new(parent_size));
      result.px()
  }
  ```
  The pointer aims into the stylo `ComputedValues` the host keeps alive for the node; lifetime discipline is entirely the host's responsibility. Serde deserialization of calc values is unsupported (tag validation rejects `CALC_TAG`).

### 8. Cargo features (`Cargo.toml`)

Default: `std, taffy_tree, flexbox, grid, block_layout, float_layout, calc, content_size, detailed_layout_info`.

| Feature | Gates |
|---|---|
| `flexbox` | flexbox algorithm, `compute_flexbox_layout`, flex style traits/fields |
| `grid` | CSS grid; pulls `alloc` + `smallvec` |
| `block_layout` | block algorithm, `BlockContext`, block style traits, `TextAlign`, `Display::Block/FlowRoot` |
| `float_layout` | `float`/`clear`, `FloatContext` (sub-feature of block layout) |
| `calc` | calc() tagged-pointer values + resolver plumbing |
| `content_size` | `Layout::content_size` / `LayoutOutput::content_size` computation |
| `detailed_layout_info` | `set_detailed_grid_info` / `DetailedGridInfo` (grid only) |
| `taffy_tree` | built-in `TaffyTree` (pulls `slotmap`) |
| `std` / `alloc` | std vs alloc; no-std possible without either (CheapCloneStr degrades to an empty trait) |
| `serde` | Serialize/Deserialize on style types |
| `parse` / `parse_faster` | `FromStr` for style types via `cssparser` |
| `strict_provenance` | strict-provenance pointer APIs (Rust ≥1.84) |
| `debug` / `profile` | internal logging/profiling |

### 9. Feature-support gaps (YES/NO)

- **direction: rtl — YES** (0.10.0; `CoreStyle::direction()`, `Direction::{Ltr,Rtl}`; block/flex/grid).
- **`order` property — NO.** No `order` field or method anywhere in `src/style/`. `Layout::order` is document/insertion order for paint stacking, not CSS `order`.
- **position: fixed / non-parent containing block — NO.** `Position` is `Relative | Absolute` only. Abspos children are laid out and positioned by their direct parent container; the trait system has no upward traversal, so "closest positioned ancestor" hop must be built by the host outside taffy.
- **Static-position fallback for abspos children with auto insets — YES in flex, YES in grid.** Flexbox: reworked in 0.13 (#1072) — writing-mode-relative `start`/`end`, fallbacks for distributed keywords and baseline, auto margins absorb space only when inset-constrained (CSS2 §10.3.7/§10.6.4). Grid: auto placement lines resolve to container padding-box/track edges (#1071, #1075) and alignment applies within the resulting area (last-baseline for abspos still a TODO comment in `grid/types/grid_item.rs`).
- **CSS containment / content-visibility — NO.** No occurrence in the source.
- **First/last baseline alignment — first YES, last NO.** `AlignItemsKeyword::Baseline` exists and `LayoutOutput.first_baselines` propagates (horizontal-axis baseline used); there is no `LastBaseline` variant; `grid/types/grid_item.rs:59`: "TODO: Support last baseline and vertical text baselines".
- **aspect-ratio — YES** (`CoreStyle::aspect_ratio() -> Option<f32>`, applied in all algorithms incl. block since 0.12 #965).
- **Percentage gaps — YES.** `gap: Size<LengthPercentage>`; flexbox resolves against `node_inner_size.or(Size::zero())` (`compute/flexbox.rs:475`), grid similarly (against inner container size, `compute/grid/explicit_grid.rs`).
- **Auto margins — YES** (`margin: Rect<LengthPercentageAuto>`; flexbox free-space distribution steps 10/13; block auto-margin centering; abspos rules refined in 0.13).
- **min-content/max-content/fit-content sizing keywords on width/height — NO.** `Dimension` constructors are `length/percent/auto/calc` only; serde validation rejects other tags (`src/style/dimension.rs:350`). The intrinsic tags in `CompactLength` are reachable only for grid `MinTrackSizingFunction`/`MaxTrackSizingFunction` (`min_content`/`max_content` on both; `fit_content_px`/`fit_content_percent` and `fr` on `MaxTrackSizingFunction` only). External callers can still request a node's intrinsic size via `AvailableSpace::MinContent/MaxContent`.
- **overflow/scrollbar_size — YES with taffy's semantics.** `Overflow::{Visible,Clip,Hidden,Scroll}`; `Hidden|Scroll` zero the automatic minimum size; `Scroll` reserves `scrollbar_width` and reports it in `Layout::scrollbar_size`. **Descendant overflow of a scroll container is excluded from the parent's content_size: YES** — `compute_content_size_contribution` (`src/compute/common/content_size.rs`) contributes `max(size, content_size)` only for `Overflow::Visible`; any other value contributes the border-box size only.
- **subgrid — NO** (only a spec-quoting comment in `compute/grid/types/named.rs`).
- **Named grid lines/areas — YES** (since 0.9.0; `GridPlacement::NamedLine/NamedSpan`, `grid_template_{row,column}_names`, `grid_template_areas` with implicit area-derived line names).
- **masonry — NO** (no occurrence).
- **text-align on blocks — PARTIAL/NO.** Only `TextAlign::{Auto, LegacyLeft, LegacyRight, LegacyCenter}` implementing legacy `<center>`/`align=` behavior for block-level children; taffy performs no inline/text layout, so real `text-align` is out of scope.

### 10. Rounding

`round_layout(tree: &mut impl RoundTree, node_id: NodeId)` (`src/compute/mod.rs:219`): pure CSS-px integer rounding via `f32::round` — **no device-pixel-ratio parameter**. Cumulative-error handling: it recurses carrying viewport-cumulative x/y; positions round as `round(location)`, but every extent (size, border, padding, content_size per side) is computed as `round(cum + edge_far) - round(cum + edge_near)` so adjacent boxes never open gaps (technique credited to Yoga commit `aa5b296` in the doc comment). It reads via `get_unrounded_layout` and writes via `set_final_layout` specifically so re-rounding after relayout never re-rounds rounded values. `RoundTree` methods are only those two getters/setters (plus inherited traversal). `TaffyTree` keeps the double-storage and offers `enable_rounding()`/`disable_rounding()`. Hosts wanting device-pixel snapping replace or wrap this pass (scale by DPR, round, unscale) — taffy's units are deliberately abstract ("Users of Taffy may define what they correspond to", `dimension.rs`), and nothing in the crate knows about DPR.

### 11. Layout output storage

`Layout` (`src/tree/layout.rs:226`), `#[derive(Debug, Copy, Clone, PartialEq)]`:

```rust
pub struct Layout {
    pub order: u32,                    // topological/paint order, NOT css order
    pub location: Point<f32>,          // top-left, parent-border-box relative
    pub size: Size<f32>,               // border-box width/height
    #[cfg(feature = "content_size")]
    pub content_size: Size<f32>,       // scrollWidth/scrollHeight-style, from padding-box origin (0.13, #1051)
    pub scrollbar_size: Size<f32>,
    pub border: Rect<f32>,
    pub padding: Rect<f32>,
    pub margin: Rect<f32>,             // margin IS present (resolved used margins)
}
```

Helpers: `content_box_width/height/size()`, `content_box_x/y()`, and (content_size) `scroll_width()/scroll_height()`.

### 12. Incremental relayout

- Caching is host-owned via `CacheTree`; taffy's algorithms call `compute_child_layout`, and the host is expected to wrap its dispatch in `compute_cached_layout(tree, node, inputs, dispatch)` — get/compute/store is entirely in host hands.
- Dirtying = clearing caches up the ancestor chain. Reference implementation `TaffyTree::mark_dirty(node)` clears the node's `Cache` and recurses to the parent, stopping early when an ancestor's cache is already empty (`ClearState::AlreadyEmpty` short-circuit; `src/tree/taffy_tree.rs:877`). `dirty(node)` = `cache.is_empty()`. A host tree replicates exactly this with `Cache::clear() -> ClearState`.
- Partial-subtree relayout: structurally supported — every compute function takes any `NodeId` plus explicit `LayoutInput`, and `LayoutPartialTree` only requires one level of children, so a host can re-run layout from any node whose input constraints are unchanged (or call `compute_root_layout` at an interior node treated as a root). Unchanged clean subtrees short-circuit through cache hits (a full `PerformLayout` entry returns the stored `LayoutOutput` without descending). The 0.12 cache-key change (axis + parent_size + available_space in the key, available_space ignored on an axis with a known dimension) is what makes these hits sound; budget for its measured ~10% cost. `round_layout` can likewise start at any subtree node, but must be passed that node's cumulative offset implicitly as zero — it hardcodes `(0.0, 0.0)` at the entry, so subtree re-rounding away from the viewport origin needs a host-side reimplementation of `round_layout_inner`.

Primary sources: https://crates.io/crates/taffy · https://docs.rs/taffy/latest · https://github.com/DioxusLabs/taffy/blob/main/CHANGELOG.md · `v0.13.0` tag files `src/tree/{traits,layout,cache,node}.rs`, `src/style/{mod,flex,block,grid,dimension,compact_length,available_space,alignment,float}.rs`, `src/compute/{mod,flexbox,leaf,block}.rs`, `src/compute/grid/mod.rs`, `src/compute/common/content_size.rs`, `Cargo.toml` · Blitz: https://github.com/DioxusLabs/blitz/blob/main/packages/stylo_taffy/src/convert.rs, https://github.com/DioxusLabs/blitz/blob/main/packages/blitz-dom/src/layout/mod.rs

### Corrections and confidence

Verification method: fresh shallow clone of the `v0.13.0` tag from github.com/DioxusLabs/taffy; every quoted signature, struct, enum, cache detail, line number, and CHANGELOG entry re-read from that clone; crates.io claims re-checked against `https://crates.io/api/v1/crates/taffy`; Blitz snippets re-fetched from raw.githubusercontent.com (main branch, 2026-08-12).

Corrections made:

1. **NodeId docs.rs URL** (§2). Original cited `https://docs.rs/taffy/latest/taffy/struct.NodeId.html`, which returns 404. The struct is documented under the `tree` module: https://docs.rs/taffy/latest/taffy/tree/struct.NodeId.html (page confirmed live, documenting v0.13.0). All other NodeId claims (u64 newtype, derives, `const fn new`, u64/usize/DefaultKey conversions) confirmed against `src/tree/node.rs`.
2. **0.13.0 fix count** (§1). Original said "~35 flexbox/grid/block/float conformance fixes"; the 0.13.0 "Fixed" section of CHANGELOG.md contains 29 bullets — corrected to "~30 (29 bullets)". https://github.com/DioxusLabs/taffy/blob/v0.13.0/CHANGELOG.md
3. **Style-trait defaults claim** (§3 preamble). Original said "All style-trait methods have default implementations". True for `CoreStyle`, `FlexboxContainerStyle`, `FlexboxItemStyle`, `BlockContainerStyle`, `BlockItemStyle`, and `GridItemStyle`, but `GridContainerStyle` has seven required methods with no default bodies (`grid_template_rows/columns`, `grid_auto_rows/columns`, `grid_template_areas`, `grid_template_column_names`, `grid_template_row_names`) plus required associated types — the report's own per-method "required" annotations were already correct; the preamble was fixed to match. `src/style/grid.rs`.
4. **`from_raw`/`into_raw` location** (§7). Original listed them among `CompactLength` accessors; they are defined on the wrapper newtypes `Dimension`, `LengthPercentage`, `LengthPercentageAuto` (`src/style/dimension.rs:71/76/176/181/308/313`), not on `CompactLength`. The Blitz usage (`taffy::LengthPercentage::from_raw`) was already consistent with this.
5. **fit-content intrinsic tags precision** (§9). Original implied `fit_content_px`/`fit_content_percent` are reachable on both grid track-sizing halves; they are constructible only on `MaxTrackSizingFunction` (`src/style/grid.rs`, ctors at ~880/894 within the `MaxTrackSizingFunction` impl), while `min_content`/`max_content` exist on both `MinTrackSizingFunction` and `MaxTrackSizingFunction`.

Verified exactly as originally stated (spot-listing the load-bearing ones): version numbers/dates and MSRV 1.71 (crates.io API); absence of RELEASES.md; every trait signature in §2 including `CacheTree`, default `resolve_calc_value` returning 0.0, and `compute_block_child_layout`'s default forwarding; the `get_style`/`get_cache_mut` stale doc-comment caveat (traits.rs module doc lines ~78–86); all §3 trait methods, `CheapCloneStr` bounds and no-std degradation, `GridPlacement`/`GridTemplateArea(s)`/`RepetitionCount`/`TemplateLineNames` shapes, `TrackSizingFunction` type alias (grid.rs:1290), Style sizes 520/552 (mod.rs:1378/1384); all §4 structs/enums verbatim including `CollapsibleMarginSet` private fields and `LayoutInput::HIDDEN`; all §5 signatures including `compute_leaf_layout`'s tree-less form and `place_floated_box`'s 5-arg signature (block.rs:148); §6 cache layout, 9 slots + slot-selection rules (cache.rs doc + `compute_cache_slot`), packed `CacheKey` encoding (Definite negated, axis in sign bits, x-axis-only parent_size comparison for ComputeSize), `ClearState`, and no accessor for stored keys; §7 tag constants and `CompactLength::calc` alignment asserts, plus both Blitz snippets verbatim (convert.rs lines ~65–69, layout/mod.rs lines 32–36); §8 feature table against Cargo.toml; every §9 YES/NO (no `order`, `Position` = Relative|Absolute only, no containment/subgrid/masonry occurrences, `AlignItemsKeyword::Baseline` present with no LastBaseline, grid_item.rs:59 TODO, Dimension ctor set, dimension.rs:350 serde validation, flexbox.rs:475 gap resolution, content_size.rs Visible-only contribution, TextAlign legacy variants); §10 rounding mechanics, Yoga commit `aa5b296` credit (mod.rs:215), `(0.0, 0.0)` entry hardcode, enable/disable_rounding (taffy_tree.rs:564/569), "abstract units" quote (dimension.rs:42); §11 `Layout` fields and helpers (layout.rs:226); §12 `mark_dirty` recursion with `AlreadyEmpty` short-circuit (taffy_tree.rs:877) and `dirty()` = `cache.is_empty()` (taffy_tree.rs:904).

UNVERIFIED (soft claims, not contradicted): the assertion that the Blitz calc pointer specifically targets data inside stylo `ComputedValues` kept alive per node — consistent with the fetched code but not traced end-to-end; the characterization of grid gap resolution as "similarly" to flexbox (grid resolves against inner container size via a different code path, `explicit_grid.rs:156`); "first baseline propagates (horizontal-axis baseline used)" — the field and `Point<Option<f32>>` shape are confirmed, but which axis each algorithm consumes was not re-traced through every call site.

---

# Blitz / stylo_taffy prior art

> Provenance: Verified against DioxusLabs/blitz @ 4322e86f0b27 (2026-08-12 main) and the taffy v0.13.0 rev it pins; second verification pass included.

All verification is complete. Every load-bearing claim has been checked against the local clones at the exact cited commits, crates.io API, docs.rs, and the GitHub issues. Here is the corrected report.

## Blitz (DioxusLabs) — Stylo ↔ Taffy integration research report

Sources: shallow clone of `github.com/DioxusLabs/blitz` @ `4322e86f0b27` (2026-08-12, main) and `github.com/DioxusLabs/taffy` @ pinned rev `d4a2b3bf17ed` (v0.13.0, 2026-08-10). All paths below are repo-relative. Crate metadata verified against crates.io API and docs.rs/stylo_taffy.

---

### 1. `stylo_taffy`: versions, API, trait-implementation strategy

**Versions.** crates.io latest: `0.3.0-beta.1` (2026-07-10), deps `stylo ^0.19`, `stylo_atoms ^0.19`, `taffy ^0.12.1` (features `std,flexbox,grid,block_layout,content_size,calc,detailed_layout_info`). Prior stable `0.2.0` (published 2025-10-06): `stylo ^0.8`, `stylo_atoms ^0.8`, `taffy ^0.9`. Git main (`packages/stylo_taffy`, workspace version `0.3.0-beta.1`) is ahead of the publish: `stylo 0.20.0` (aliased `style = { package = "stylo" }`, plus `style_atoms`) and **a git-pinned taffy rev** `DioxusLabs/taffy rev d4a2b3bf17ed` = taffy **0.13.0** — note `DioxusLabs/taffy` is taffy's canonical upstream repository (taffy 0.13.0 was released on crates.io 2026-08-08; the pin is a rev two days after the release), not a fork — features `["std","flexbox","grid","block_layout","content_size","calc","detailed_layout_info"]` (root `Cargo.toml:98-106`). Feature flags on stylo_taffy: `default = ["std","block","flexbox","grid"]`, optional `floats` → `taffy/float_layout` (`packages/stylo_taffy/Cargo.toml`).

**Public API** (`packages/stylo_taffy/src/lib.rs`, 13 lines):
- `pub struct TaffyStyloStyle<T: Deref<Target = ComputedValues>>(pub T)` (`src/wrapper.rs:17`) — wraps anything derefing to `stylo::ComputedValues` (`&`, `Arc`, `Ref`).
- `pub fn to_taffy_style(style: &ComputedValues) -> taffy::Style<Atom>` (`src/convert.rs:661`) — eager whole-struct conversion.
- `pub mod convert` — ~30 free per-property conversion functions (`length_percentage`, `dimension`, `margin`, `inset`, `border`, `display`, `position`, `overflow`, `direction`, `aspect_ratio`, `content_alignment`, `justify_content`, `item_alignment`, `gap`, `flex_basis`, `grid_line`, `track_size`, `min_track`/`max_track`, `grid_auto_flow`, …).
- `pub use style::Atom` — the `CustomIdent` type used for grid line/area names (`taffy::Style<Atom>`; taffy's traits are generic over `CustomIdent`).

**Both strategies are implemented:**
- *Lazy wrapper*: `TaffyStyloStyle` implements `taffy::CoreStyle` (with `type CustomIdent = Atom`), `BlockContainerStyle`, `BlockItemStyle`, `FlexboxContainerStyle`, `FlexboxItemStyle`, `GridContainerStyle`, `GridItemStyle` — every accessor is `#[inline]` per-read conversion, e.g. `fn size()` returns `convert::dimension(&self.0.get_position().width/height)` (`wrapper.rs:34-158`). Grid accessors are zero-alloc via GATs: associated iterator types (`SliceMapIter`, `StyloLineNameIter`, `RepetitionWrapper` implementing `taffy::GenericRepetition`, `taffy::TemplateLineNames`) lazily map stylo `OwnedSlice`s (`wrapper.rs:265-337, 339-522`).
- *Eager*: `to_taffy_style` builds a full `taffy::Style<Atom>` in one pass (`convert.rs:661-805`).

**blitz-dom today uses the eager path, not the wrapper.** The only `stylo_taffy` call sites in blitz-dom are `to_taffy_style` (`blitz-dom/src/layout/damage.rs:562`, `layout/table.rs:74,274`). The wrapper is exported for standalone integrators.

**Per-read cost:** accessors are field reads on `ComputedValues` sub-structs (`get_position()`, `get_box()`, …) plus a `match` — no allocation, no refcounting, except: `Atom` clones (refcount bump) for named grid lines/areas, and `clone_display()`/`clone_text_align()` (Copy types). `to_taffy_style` additionally allocates `Vec`s for the seven grid template/auto-track/line-name/area fields (`Vec::new()` = no alloc when unset). `length_percentage` conversion is branch-on-tag via `LengthPercentage::unpack()` → `Unpacked::{Length, Percentage, Calc}` — no calc-tree evaluation at conversion time (see §2). Note: the wrapper's `CoreStyle` impl does **not** override taffy's `direction()` accessor (defaults to `Ltr`); only the eager path carries `direction` through — a live gap in the lazy path.

### 2. calc() across the boundary

Zero-copy raw pointer. `convert::length_percentage` (`convert.rs:63-74`):

```rust
stylo::UnpackedLengthPercentage::Calc(calc_ptr) => {
    let val = CompactLength::calc(calc_ptr as *const stylo::CalcLengthPercentage as *const ());
    unsafe { taffy::LengthPercentage::from_raw(val) }
}
```

`taffy::CompactLength` is a 64-bit tagged-pointer type; `CompactLength::calc(ptr)` asserts non-null and 8-byte alignment and stores the pointer with `CALC_TAG = 0b000` (taffy `src/style/compact_length.rs:256-259`). Taffy treats it opaquely; whenever a calc value needs resolving against a basis it calls the tree's hook `LayoutPartialTree::resolve_calc_value(&self, val: *const (), basis: f32) -> f32` (taffy `src/tree/traits.rs:190`), also threaded into helper traits (`ResolveOrZero`, `MaybeResolve` take the resolver as an argument).

Blitz's implementation (`blitz-dom/src/layout/mod.rs:32-36`):

```rust
pub(crate) fn resolve_calc_value(calc_ptr: *const (), parent_size: f32) -> f32 {
    let calc = unsafe { &*(calc_ptr as *const CalcLengthPercentage) };
    let result = calc.resolve(CSSPixelLength::new(parent_size));
    result.px()
}
```

i.e. it casts back to stylo's `CalcLengthPercentage` and evaluates stylo's calc tree per resolution. **Lifetime coupling:** the materialized `taffy::Style` stored on each node embeds raw pointers into the node's primary-style `ServoArc<ComputedValues>`; validity depends on the node keeping that Arc alive in `stylo_element_data` and on styles being re-flushed (they are, every `flush_styles_to_layout` pass) before layout runs. Taffy's cache stores only `LayoutOutput`s / `Size<f32>`s (f32 data), never `CompactLength`s, so no stale calc pointers survive in caches.

### 3. blitz-dom's layout driver

All in `packages/blitz-dom/src/layout/mod.rs`. `BaseDocument` (the node slab: `self.nodes[NodeId]`) implements taffy's entire low-level trait stack:

- **NodeId mapping** — trivial: `taffy_node_id(id) = taffy::NodeId::from(id.as_u64())`, `dom_node_id` inverse (`blitz-dom/src/lib.rs:92-100`). Same slab keys, no side table.
- **`TraversePartialTree`** (`mod.rs:330-358`): `type ChildIter<'a> = RefCellChildIter<'a>` iterating `node.layout_children: RefCell<Option<ThinVec<NodeId>>>` — a *separate* child list from DOM `children` (built by box construction, §4/§5). Plus `TraverseTree` marker.
- **`LayoutPartialTree`** (`mod.rs:361-391`): `type CoreContainerStyle<'a> = &'a taffy::Style<Atom>`, `type CustomIdent = Atom`; `get_core_container_style` returns `node.style()` — the **materialized `taffy::Style<Atom>` stored per node** (on `ElementData`/`DocumentData`, forwarded via `universal_accessors!` in `node/node.rs:160-178`). `set_unrounded_layout` writes `node.unrounded_layout`. `compute_child_layout` wraps `compute_child_layout_internal` in `taffy::compute_cached_layout`.
- **`taffy::CacheTree`** (`mod.rs:393-419`): `cache_get/store/clear` delegate to a `taffy::Cache` stored per node (`ElementData.cache`); taffy 0.13's `Cache` = 1 final-layout entry + fixed array of 9 measure entries + `is_empty` flag.
- **`LayoutBlockContainer`** (adds `compute_block_child_layout` threading an `Option<&mut BlockContext>` — taffy's block-formatting-context/floats context, added upstream in taffy 0.12), **`LayoutFlexboxContainer`**, **`LayoutGridContainer`** (also `set_detailed_grid_info` storing `taffy::DetailedGridInfo` boxed on the element), **`RoundTree`** (`get_unrounded_layout`/`set_final_layout` — separate `unrounded_layout` and `final_layout` per node), **`PrintTree`**.

**Dispatch** — `compute_child_layout_internal(node_id, inputs, block_ctx)` (`mod.rs:48-327`) matches on `NodeData`:
- `Text` → should never be measured individually (all text is wrapped in an inline context); returns `LayoutOutput::HIDDEN` + trace error.
- `Element | AnonymousBlock`: special-case leaves first — `<textarea>`/`<input>` via `compute_leaf_layout(inputs, node.style(), resolve_calc_value, measure_closure)` with hardcoded intrinsic sizes from rows/cols/line-height; replaced elements (`img/svg/canvas/video/embed/iframe`, `layout/replaced.rs`) via a hand-written CSS2 replaced-sizing `replaced_measure_function(known_dimensions, parent_size, available_space, &ReplacedContext { inherent_size, attr_size, inherent_ratio }, style, sizing_mode, axis)`; table roots (flag `IS_TABLE_ROOT`) via `compute_grid_layout` on a `TableTreeWrapper { doc, ctx: Arc<TableContext> }` shim tree (tables emulated as CSS Grid, `layout/table.rs`); inline roots (flag `IS_INLINE_ROOT`) via `self.compute_inline_layout` (§5). Otherwise dispatch on the materialized `style.display`: `Block → compute_block_layout(self, id, inputs, block_ctx)`, `FlowRoot → compute_block_layout(.., None)`, `Flex → compute_flexbox_layout`, `Grid → compute_grid_layout`, `None → HIDDEN`.
- Root drive: `BaseDocument::resolve_layout` (`resolve.rs:401-419`) = `taffy::compute_root_layout(self, root_element_id, viewport_definite_available_space)` then `taffy::round_layout(self, root_element_id)`.

The full frame pipeline (`resolve.rs:43-157`): `resolve_stylist` (stylo restyle) → `propagate_damage_flags` (incremental only) → `resolve_layout_children` (box construction) → `resolve_deferred_tasks` (parallel text shaping) → `flush_styles_to_layout` (stylo→taffy conversion + z-order lists) → `resolve_layout` (taffy) → `resolve_transforms` (overflow rects) → clear damage.

### 4. display:contents

Flattening happens at **box-construction time in the DOM layer**, when building the `layout_children` list — taffy never sees a contents node (stylo_taffy's `box_generation_mode` has the `Contents` mapping commented out; `convert.rs:206-212`). Mechanism in `blitz-dom/src/layout/construct.rs`:
- `collect_layout_children` on a container whose own `display.inside() == DisplayInside::Contents` calls `push_hoisted_children_and_pseudos` (`construct.rs:200-226`): pushes the contents node's **children themselves** (not their layout children) into the parent's `LayoutChildren` accumulator, recursing only through nested `display:contents` children; construction damage on the contents node is cleared (`construct.rs:454-461`).
- Inline-vs-block classification recurses transparently through contents nodes (`classify_flow_children`, `construct.rs:273-342`, sets `has_contents`; a contents child "casts no vote itself — its children decide").
- In `collect_complex_layout_children` (anonymous-box path) a `DisplayInside::Contents` child triggers `collect_layout_children(doc, child_id, out)` recursion (`construct.rs:762-764`); in flex/grid containers, presence of a contents child forces the complex path (`construct.rs:526-558`); inside inline contexts contents children are walked through both in `find_inline_layout_embedded_boxes_recursive` and `build_inline_layout_recursive` (`construct.rs:885-895, 1087-1100`).
- The contents node still exists in the DOM tree and carries damage; `should_traverse_layout_children` returns `false` for it so damage propagation walks its DOM children (`traversal.rs:104-114`). The implementation is self-described as incomplete (a `// TODO: fix display:contents` sits at `construct.rs:480`); display:contents does not appear as an item on the roadmap issue.

### 5. Text/inline layout (Parley) alongside Taffy

Architecture: **"inline context" roots** — if a flow container's in-flow children are all inline (`classification.all_inline`), the container gets `NodeFlags::IS_INLINE_ROOT` and its whole inline subtree is laid out by one Parley layout; block/inline mixing creates **anonymous block boxes** instead.

- *Anonymous boxes*: `LayoutChildren` accumulator (`construct.rs:66-182`) wraps runs of inline/text children of block/flex/grid containers into `NodeData::AnonymousBlock` nodes (styled via stylo `style_for_anonymous::<&Node>(…, PseudoElement::ServoAnonymousBox, parent_style)`), tracked in `node.anonymous_blocks` for deallocation on reconstruction; whitespace-only anon blocks are deleted.
- *Construction*: inline roots are queued as `ConstructionTask{ data: InlineLayout(Box<TextLayout>) }` and shaped **in parallel** (rayon, feature `parallel-construct`) in `resolve_deferred_tasks` (`resolve.rs:318-395`) via `build_inline_layout_into` (`construct.rs:933-1224`): walks the inline subtree with a `parley::TreeBuilder`, pushing style spans (`stylo_to_parley::style`), text runs (with `text-transform`, `white-space-collapse`, `<br>` as preserved `\n`), and `parley::InlineBox{ id: node_id.as_u64(), kind }` placeholders for atomic inlines (replaced elements, `inline-block`, inputs), with `InlineBoxKind::{InFlow, OutOfFlow (abspos), CustomOutOfFlow (floats)}`. Result stored as `ElementData.inline_layout_data: Option<Box<TextLayout>>` (`text: String`, `content_widths: Option<ContentWidths>` cache, `layout: parley::Layout<TextBrush>`).
- *Embedded box list*: `find_inline_layout_embedded_boxes` collects the atomic-inline node ids as the inline root's `layout_children`, so taffy caching/traversal still sees them.
- *Measurement/layout*: `BaseDocument::compute_inline_layout` (`layout/inline.rs:21-828`) is a hand-written taffy-compatible algorithm: resolves size styles itself (using taffy helper traits + `resolve_calc_value`), sizes each in-flow inline box by recursing `self.compute_child_layout(taffy::NodeId::from(ibox.id), child_inputs)` (hits taffy's cache), feeds widths/heights back into parley's `InlineBox`es, uses `calculate_content_widths()` for min/max-content, then `break_all_lines(Some(width))` (or the float-aware `break_lines()` driver interleaving with taffy's `BlockContext::find_content_slot`/`place_floated_box` under feature `floats`), applies `text-align`, and finally writes each inline box's `unrounded_layout` (size/location/padding/border) directly from parley line items. Returns a `LayoutOutput` with first baseline, content_size, and margin-collapse info. Inline roots participate in taffy's BFC: an inherited `BlockContext` is reused unless the root is a scroll container.

### 6. position:absolute / position:fixed

- Style mapping (`stylo_taffy/src/convert.rs:222-234`): `Static → taffy::Position::Relative` (TODO), `Absolute → Absolute`, **`Fixed → Absolute` (TODO)**, `Sticky → Relative` (TODO).
- **Taffy does abspos layout** for block/flex/grid containers (e.g. `perform_absolute_layout_on_absolute_children`, taffy `src/compute/block.rs:1482`): each container positions its own `Position::Absolute` children against its padding box, resolving insets/auto-margins with `resolve_calc_value` and `direction`. Because *every* element maps to taffy `Relative`, the effective containing block is always the **direct layout parent** — no nearest-positioned-ancestor walk, no hoisting in blitz's tree construction. (Taffy issue #212 — "Support `position: static` (absolute position relative a non-parent ancestor)", open since August 2022 — tracks this; "position:static and position:fixed" is a Blitz 1.0 roadmap item, issue #119.)
- **Blitz runs its own positioned pass only for abspos children of inline roots**: `layout_abspos_child` (`blitz-dom/src/layout/inline.rs:838-1145`), a free function generic over `impl taffy::LayoutBlockContainer` — full CSS2 §10.3.7/§10.6.4 solving: static position from the parley line position (inline-level hypothetical box) or the container's content-box left edge (block-level, using stylo's `original_display.outside()` — pre-blockification display), two-phase `compute_child_layout` (ComputeSize then PerformLayout), auto-margin distribution, RTL over-constrained resolution, then `set_unrounded_layout` directly.
- Static position for block-container abspos children is handled inside taffy's block algorithm, keyed off the same `Relative`-mapped siblings.
- **Fixed**: no containing-block or paint special-casing found anywhere in blitz-dom/blitz-paint — fixed elements behave exactly like abspos (they scroll with their parent; no viewport anchoring, no transformed-ancestor handling). `position:fixed`/`sticky` do force a stacking context (`node.rs:1088`).

### 7. direction:rtl, `order`, containment

- **RTL**: upstream taffy added `Direction { Ltr, Rtl }` to `Style` and `CoreStyle::direction()` (default Ltr) in **0.12.0** ("Support for `direction`" — RTL layout of boxes in Block, Flexbox, and Grid; 0.13.0 added the direction-relative `self-start`/`self-end` alignment keywords); `to_taffy_style` maps stylo `direction` (`convert.rs:249-255, 679`). Consumed by taffy block layout, by `convert::justify_content` (resolves physical `left`/`right` keywords against flex main axis + direction at conversion time, `convert.rs:312-343`), and by blitz's inline abspos/inset code (`inline.rs:770, 1091-1106`). Text-level bidi is Parley's. `writing-mode` is backlog (roadmap).
- **`order`**: taffy has no `order` property; blitz implements it by **physically sorting `layout_children`** of flex/grid containers by `node.order()` during `flush_styles_to_layout` (`damage.rs:607-613`), where `order()` returns `i32::MIN`/`i32::MAX` for `::before`/`::after` and `clone_order()` otherwise (`node/node.rs:1058-1066`). Paint order uses order-modified document order for in-flow flex/grid items and excludes out-of-flow ones (`node_to_paint_order`, `damage.rs:703-722`); `paint_children` is a second sorted list, plus `HoistedPaintChildren` per stacking context for non-zero z-index.
- **Containment / content-visibility**: nothing — zero hits for `contain`/`content-visibility` in blitz-dom; `contain` appears only as a TODO in `is_stacking_context_root` (`node.rs:1100-1105`) and as a 1.0 roadmap item.

### 8. Incremental layout

`BaseDocument.incremental_layout: bool` (default **true**, `document.rs:460`). Pieces:

- **Damage source**: stylo's `RestyleDamage` bits, with blitz-private bits layered on: `ONLY_RELAYOUT = 0b1000`, `CONSTRUCT_BOX = 0b1_0000`, `CONSTRUCT_FC = 0b10_0000`, `CONSTRUCT_DESCENDENT = 0b100_0000` (`layout/damage.rs:22-33`). `compute_layout_damage(old, new)` (`damage.rs:214-307`) diffs `ComputedValues`: display/float/position/visibility/font changes, BFC-establishing changes, list-marker changes, pseudo `content` changes ⇒ `ALL_DAMAGE` (rebuild boxes); direction/bidi/white-space/text-transform/letter-spacing changes ⇒ `ALL_DAMAGE` (reshape text); everything else ⇒ `RELAYOUT` only. (`RestyleDamage::compute_style_difference` is used for pseudo-element sync.)
- **Propagation**: `propagate_damage_flags` (`damage.rs:36-127`) walks the tree (via layout_children when appropriate — `should_traverse_layout_children`, `traversal.rs:104`), ORs child damage into the parent (`damage_for_parent = damage`, i.e. any relayout damage clears taffy caches along the whole ancestor chain), and for nodes with `ONLY_RELAYOUT | CONSTRUCT_BOX` clears the node's taffy `Cache` and the parley `content_widths` cache. `CONSTRUCT_BOX ⇒ RELAYOUT`.
- **Reconstruction**: `resolve_layout_children` (`resolve.rs:245-316`) re-runs `collect_layout_children` only where `CONSTRUCT_FC | CONSTRUCT_BOX` (or always when non-incremental), deallocating the node's previous anonymous blocks; otherwise just recurses through existing layout_children.
- **Style flush**: `flush_styles_to_layout_impl` currently re-runs `to_taffy_style` for **every node every frame** (the damage gate is commented out, `damage.rs:569`); in non-incremental mode it also unconditionally clears every cache.
- **Relayout**: always a full `compute_root_layout` from the root; partial relayout is achieved through taffy's per-node caches — undamaged subtrees are `compute_cached_layout` hits.
- **Epilogue**: `resolve_transforms` recomputes `scrollable_overflow` only under `RECALCULATE_OVERFLOW` damage; then all node damage and stylo `dirty_descendants` are cleared (`resolve.rs:112-118`). Viewport/scale changes call `invalidate_inline_contexts` (`damage.rs:397-433`) which stamps every inline root `ALL_DAMAGE` (text must reshape at the new scale).

### 9. Scroll containers

- Style: `overflow` maps 1:1 except **`Auto → taffy::Overflow::Scroll`** (TODO in taffy, `convert.rs:237-246`); `scrollbar_width` is hardcoded **0.0** (both wrapper and eager) — no layout-reserved gutters; scrollbars are **overlay**, painted by blitz-paint from `node/scrollbar.rs` (activity-based fade, `scrollbar_activity` map on the document).
- Taffy computes `Layout.content_size` (feature `content_size`) and `Layout.scrollbar_size` per node; blitz's own inline path sets `scrollbar_size` in `layout_abspos_child` (transposed axes) but otherwise relies on taffy.
- Consumption: scroll extents = `final_layout.scroll_width()/scroll_height()` (taffy helpers over `content_size` vs `size`); scrolling state is `node.scroll_offset: Point<f64>` mutated by `scroll_node_by_has_changed` (`document.rs:1990-2120`) with per-axis clamping and **remainder bubbling** to parent/viewport; root-element scrolls forward to the viewport per CSS overflow propagation; scroll events report `scroll_width = size.max(content_size)`. `scroll_offset` is applied at paint time and in hit testing, and subtracted when accumulating hoisted stacking-context child positions (`damage.rs:662-671`). `scrollable_overflow` (kurbo Rect, transform-aware) is a separate paint/overflow-rect computation in `resolve_transforms`.
- Inline layout reserves scrollbar gutters itself only when `Overflow::Scroll` (using the style's `scrollbar_width`, i.e. currently 0) (`inline.rs:175-186`); a scroll container also forces a fresh BFC for inline layout.

### 10. Known limitations / regrets on record

- `position: static` and `position: fixed` unimplemented (mapped to relative/absolute; roadmap 1.0, issue #119); `sticky → relative` (code TODO in `convert.rs`; sticky is not an explicit roadmap item).
- Abspos containing block is always the direct layout parent — nearest-positioned-ancestor resolution missing (taffy issue #212, open since 2022, the Bevy days). No fixed/transformed-ancestor containing-block logic.
- Intrinsic sizing keywords `min-content`/`max-content`/`fit-content()`/`stretch` on width/height all collapse to `Dimension::AUTO` (`convert.rs:77-115`; roadmap 1.0). `flex-basis: content → AUTO`.
- Subgrid and masonry: dropped to `None` in all grid template accessors (`wrapper.rs:384-387` etc.).
- Tables are **emulated as CSS Grid** (`display()` maps `DisplayInside::Table → taffy::Display::Grid`; `TableTreeWrapper` + `RowDense` auto-flow + a content-size clamping "HACK: Cap content size at node size to prevent scrolling", `layout/mod.rs:303-305`); real (non-grid-emulated) table layout is a backlog roadmap item. `border-collapse: collapse` has an approximation inside the grid emulation (`layout/table.rs:116-162`: gap and table border derived from the first cell's max adjacent border widths), rather than true border resolution.
- `overflow: auto` treated as `scroll` (taffy TODO); `scrollbar-width` style not read (scrollbar styling is a 0.3-beta roadmap item).
- `display: contents` implemented but self-described as incomplete (`// TODO: fix display:contents`, `construct.rs:480`); `writing-mode`, multicol/fragmentation, anchor positioning backlog; anchor-positioning stylo values `unreachable!()`d in conversion (`convert.rs:91-92` etc. — panics if stylo ever produces them with the feature on).
- `contain`/`content-visibility`: no support; `mix-blend-mode`/`filter`/`clip-path`/`mask`/`isolation` missing from stacking-context detection (TODOs, `node.rs:1100-1105`).
- Baseline content-alignment falls back to start/end; `last baseline` item alignment falls back to `END` (comments in `convert.rs:289-293, 361-364`).
- Text nodes reaching taffy individually is treated as a bug (returns `HIDDEN` + `tracing::error`, `layout/mod.rs:72-104`).
- Non-incremental fallback (`incremental: false`) clears every cache each frame; even in incremental mode style flush is a full-tree walk (gate commented out, `damage.rs:569`). A separate "TODO: see if this can be made more efficient (/run less often)" (`damage.rs:54`) concerns the per-node pseudo-element style sync inside `propagate_damage_flags`.
- `resolve_layout` carries the regret comments: "TODO: update taffy to use an associated type instead of slab key" and "TODO: update taffy to support traited styles so we don't even need to rely on taffy for storage" (`resolve.rs:399-400`).
- Content-widths caching for inline layouts under min-/max-content constraints is known-tricky and partially disabled (`inline.rs:340-345`, `resolve.rs:368-372`).
- The lazy `TaffyStyloStyle` wrapper misses `direction()` (falls back to Ltr) and cannot express `item_is_replaced` (blitz patches the materialized style manually, `damage.rs:564-567`).

URLs: https://github.com/DioxusLabs/blitz (packages/stylo_taffy/src/{lib,wrapper,convert}.rs; packages/blitz-dom/src/{resolve.rs, layout/{mod,construct,inline,damage,table,replaced,list}.rs, node/node.rs, traversal.rs, document.rs}), https://github.com/DioxusLabs/taffy (canonical taffy repo; src/style/compact_length.rs, src/compute/block.rs, src/tree/{traits,cache}.rs, CHANGELOG.md), https://docs.rs/stylo_taffy/latest/stylo_taffy/, https://github.com/DioxusLabs/taffy/issues/212, https://github.com/DioxusLabs/blitz/issues/119.

### Corrections and confidence

Verification method: fresh clones of `DioxusLabs/blitz` @ `4322e86f0b27` and `DioxusLabs/taffy` @ `d4a2b3bf17ed` (both matching the report's cited commits), every cited file/line inspected directly; crates.io API (`/api/v1/crates/{stylo_taffy,taffy}` and `/dependencies` endpoints), docs.rs, and the two GitHub issues fetched.

**Corrections made:**

1. **"a git-pinned taffy fork DioxusLabs/taffy"** → `DioxusLabs/taffy` is taffy's canonical upstream repository (crates.io `taffy` lists it as the repository; https://crates.io/api/v1/crates/taffy). All derived "fork" phrasing fixed throughout (§1, §3, §5, §6, §7): `BlockContext`/floats and `direction` are upstream taffy features, not fork patches. Additionally, taffy 0.13.0 is a published crates.io release (2026-08-08); the pinned rev is two days after that release, not an unpublished fork version.
2. **§1 workspace taffy features** — the pin's feature list also includes `"detailed_layout_info"` (root `Cargo.toml:98-106`); the report omitted it (it is also present in the published stylo_taffy 0.3.0-beta.1's taffy dependency features, per crates.io).
3. **§1 stylo_taffy 0.2.0 date** — "(2026-08-06 docs.rs rebuild)" → published **2025-10-06** (crates.io API; docs.rs shows the same date). Deps confirmed as `stylo ^0.8`, `taffy ^0.9`, plus `stylo_atoms ^0.8` (added).
4. **§7 RTL provenance** — "the taffy fork (0.13) added `Direction { Ltr, Rtl }`…" → `direction`/RTL support landed in upstream **taffy 0.12.0** (taffy CHANGELOG.md, "Support for `direction`" under 0.12.0); 0.13.0 added `self-start`/`self-end` and `Display::FlowRoot`. `CoreStyle::direction()` default Ltr confirmed (taffy `src/style/mod.rs:114`).
5. **§4/§10 display:contents roadmap claim** — "Roadmap still lists display:contents under backlog" removed: the roadmap issue #119 body does not contain a display:contents item (checked verbatim via the GitHub API). The in-code `// TODO: fix display:contents` (`construct.rs:480`) stands.
6. **§10 sticky "(backlog)"** — the roadmap does not list `position: sticky`; attribution changed to the code TODO in `convert.rs` ("TODO: support position:fixed and sticky").
7. **§10 border-collapse** — "real table layout and `border-collapse: collapse` are roadmap items" corrected: `border-collapse: collapse` is approximated inside the grid emulation (`layout/table.rs:116-162` — `BorderCollapse::Collapse` branch computing gap/table-border from the first cell's max adjacent border widths); the roadmap places border-collapse under the 0.3-beta milestone, while "Table layout (not emulated with CSS Grid)" is the backlog item.
8. **§10 TODO misattribution** — the quoted "TODO: see if this can be made more efficient" (`damage.rs:54`, not 52-55) is attached to `sync_pseudo_element_styles` inside `propagate_damage_flags`, not to the full-tree style-flush walk; sentence restructured.
9. **§1 grid allocation count** — "six" → **seven** `Vec`-carrying fields in `to_taffy_style` (`grid_template_rows`, `grid_template_columns`, `grid_template_row_names`, `grid_template_column_names`, `grid_template_areas`, `grid_auto_rows`, `grid_auto_columns`; `convert.rs:769-805`).
10. **Minor line-number fixes**: root `Cargo.toml:99-105` → 98-106; wrapper grid helper types `wrapper.rs:248-336` → 265-337 and GridContainerStyle `339-520` → 339-522; `convert.rs:82-113` → 77-115 (intrinsic keywords); `convert.rs:249-254` → 249-255; `convert.rs:311-343` → 312-343; `convert.rs:289-292, 361-363` → 289-293, 361-364; `inline.rs:1089-1106` → 1091-1106; `resolve.rs:401-418` → 401-419. `resolve_calc_value` snippet adjusted to the actual two-statement body. Cache measure-entry array size specified (9, `taffy src/tree/cache.rs:11`).

**Verified correct (spot-listing the highest-risk claims):** blitz commit `4322e86f0b27` (2026-08-12) and taffy rev `d4a2b3bf17ed` = v0.13.0 (2026-08-10) both exact; stylo_taffy 0.3.0-beta.1 (2026-07-10) deps `stylo ^0.19`/`stylo_atoms ^0.19`/`taffy ^0.12.1`; stylo 0.20.0 aliasing in workspace; lib.rs is 13 lines with exactly the four exports; `TaffyStyloStyle` at wrapper.rs:17 and the seven trait impls with `type CustomIdent = Atom`; no `direction()` override and no `item_is_replaced` in the wrapper; the calc pointer round-trip code verbatim (`convert.rs:63-74`; `CompactLength::calc` asserts at `compact_length.rs:256-259`, `CALC_TAG = 0b000`; `resolve_calc_value` default hook at `traits.rs:190`); all `Static/Fixed/Sticky` position mappings and TODOs; `Auto → Scroll` overflow TODO; `to_taffy_style` at convert.rs:661-805 including `direction`, `scrollbar_width: 0.0`, `item_is_replaced: false`; the three `to_taffy_style` call sites (damage.rs:562, table.rs:74/274) as the only stylo_taffy uses in blitz-dom; the full trait-stack line ranges in `layout/mod.rs` (330-358, 361-391, 393-419, 421-451, 492-502, 504-514); `perform_absolute_layout_on_absolute_children` at taffy `block.rs:1482` exactly; the Text→HIDDEN error path; textarea/input/replaced/table/inline-root dispatch including the content-size HACK comment at mod.rs:303-305; the resolve() pipeline order at resolve.rs:43-157 and both regret TODOs at 399-400; all §4 display:contents mechanics including the "casts no vote" comment; §5 Parley architecture (TreeBuilder, InlineBox ids, InlineBoxKind semantics, TextLayout fields, rayon `parallel-construct`, `<br>` → `"\n"`, whitespace anon-block deletion, embedded-box layout_children); `layout_abspos_child` at inline.rs:838-1145 with `impl taffy::LayoutBlockContainer`, `original_display.outside()` static-position logic, ComputeSize/PerformLayout two-phase, RTL branches; taffy has no `order` property and blitz's sort at damage.rs:607-613 with `order()` at node.rs:1058-1066; `node_to_paint_order` at damage.rs:703-722; containment absent except node.rs:1105 TODO; damage bit values, `compute_layout_damage` diff sets at 214-307, propagation semantics including cache/content_widths clearing and `damage_for_parent = damage`; commented-out flush gate at damage.rs:569 and item_is_replaced patch at 564-567; incremental default true at document.rs:460; `invalidate_inline_contexts` (damage.rs:397-433) called on scale change (document.rs:1789); scroll remainder bubbling, viewport forwarding, `scroll_width = size.max(content_size)`, scroll-offset subtraction at damage.rs:662-671, scrollbar gutter logic at inline.rs:175-186; subgrid/masonry → `None` at wrapper.rs:384-387; baseline fallbacks; `flex_basis: Content → AUTO`; anchor `unreachable!()`s; taffy issue #212 (title "Support `position: static` (absolute position relative a non-parent ancestor)", open, created 2022-08-02) and blitz issue #119 (open roadmap; "position:static and position:fixed", "min-content/max-content/fit-content()", and "contain" under 1.0; writing-mode/table/subgrid/anchor/fragmentation-multicol under backlog).

**UNVERIFIED (left in place, low risk):** the characterization "full CSS2 §10.3.7/§10.6.4 solving" in §6 (the code implements auto-margin and over-constrained inset resolution consistent with those sections, but I did not line-check every equation); the roadmap's exact checkbox state for the 0.3-beta `border-collapse: collapse` item (one fetch reported it checked — treat the milestone placement, not the completion state, as established); "~30 free per-property conversion functions" is approximate (33 public functions besides `to_taffy_style`).

---

# hughie protocol surface inventory

> Provenance: Read directly from crates/hughie at this branch.

## hughie public protocol surface inventory

Crate root: `/Users/akiwah/repos/lynx-vello/.claude/worktrees/funny-newton-c16b43/crates/hughie/src/`. All paths below relative to that. The flexbox/grid/linear/relative algorithm internals were skipped per instruction; their public entry points are included.

### 1. Public items per module

#### `lib.rs` (crate root)
Public modules: `cache`, `compute`, `geometry`, `invalidate`, `style`, `text`, `tree`.

`pub mod prelude` re-exports:
- `crate::compute::{LeafMeasureInput, LeafMetrics, NaturalSize}`
- `crate::geometry::{Edges, Line, Point, Size}`
- `crate::style::{CoreStyle, TextContainerStyle, TextRunStyle}`
- `crate::tree::{AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSlot, LayoutTree, RequestedAxis, SizingMode}`

#### `geometry.rs`
All `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)] #[repr(C)]`:
- `pub struct Point<T> { pub x: T, pub y: T }` — `const fn new(x, y)`, `fn map<U>(self, impl FnMut(T)->U) -> Point<U>`; `Point<f32>::ZERO`; `Point<Option<f32>>::NONE`.
- `pub struct Size<T> { pub width: T, pub height: T }` — `const fn new`, `map`, `zip_map<U,V>(self, Size<U>, impl FnMut(T,U)->V) -> Size<V>`, `const fn as_ref(&self) -> Size<&T>`; `Size<f32>::ZERO`; `Size<Option<f32>>::NONE`, `unwrap_or(self, Size<f32>) -> Size<f32>`, `or(self, Self) -> Self`.
- `pub struct Edges<T> { pub left, right, top, bottom: T }` — `uniform(T) -> Self where T: Clone`, `map`, `const fn as_ref -> Edges<&T>`; `Edges<f32>::ZERO`, `horizontal_sum(&self) -> f32`, `vertical_sum(&self) -> f32`.
- `pub struct Line<T> { pub start: T, pub end: T }` — `const fn new(start, end)`.

#### `tree` (mod.rs + io.rs)
- `pub struct LayoutSlot { cache: Cache /*private*/, pub static_position: Point<f32>, pub unrounded: Layout, pub rounded: Layout }` (`Debug, Default`). Methods: `cached_layout(&self, LayoutInput) -> Option<LayoutOutput>`, `store_cached_layout(&mut self, LayoutInput, LayoutOutput)`, `clear_layout_cache(&mut self)`, `committed_input(&self) -> Option<LayoutInput>`, `layout_cache_is_empty(&self) -> bool`.
- `pub trait LayoutTree`:
  ```rust
  type NodeId: Copy + core::fmt::Debug;
  type State;
  type Style<'tree>: CoreStyle where Self: 'tree;
  type ChildIter<'tree>: Iterator<Item = Self::NodeId> where Self: 'tree;
  fn children(&self, node: Self::NodeId) -> Self::ChildIter<'_>;               // required
  fn child_count(&self, node: Self::NodeId) -> usize;                          // default: children().count()
  fn flattened_children(&self, node: Self::NodeId) -> FlattenedChildren<'_, Self> where Self: Sized;  // default; flattens display:contents
  fn style(&self, node: Self::NodeId) -> Self::Style<'_>;                      // required
  fn layout<'s>(&self, state: &'s Self::State, node: Self::NodeId) -> &'s LayoutSlot;         // required
  fn layout_mut<'s>(&self, state: &'s mut Self::State, node: Self::NodeId) -> &'s mut LayoutSlot; // required
  fn set_unrounded_layout(&self, state: &mut Self::State, node: Self::NodeId, layout: Layout); // default via layout_mut
  fn set_static_position(&self, state: &mut Self::State, node: Self::NodeId, position: Point<f32>); // default
  fn compute_layout(&self, state: &mut Self::State, node: Self::NodeId, input: LayoutInput) -> LayoutOutput; // required — host dispatch
  fn clear_layout_cache(&self, state: &mut Self::State, node: Self::NodeId);   // default via layout_mut
  ```
- `pub struct FlattenedChildren<'tree, T: LayoutTree>` — `Iterator<Item = (T::NodeId, T::Style<'tree>, Display)>`; skips `display: contents` boxes and descends into them in source order; `capacity_hint(&self) -> usize`; `size_hint` is `(0, None)`; `Debug`.

`tree::io` (re-exported through `tree`), the per-call wire format:
- `pub enum SizingMode { #[default] ApplySizeStyles, IgnoreSizeStyles }` (`Copy, Eq, Hash, Default`)
- `pub enum RequestedAxis { Horizontal, Vertical, #[default] Both }`
- `pub enum LayoutGoal { Measure(RequestedAxis), #[default] Commit }`
- `pub enum AvailableSpace { Definite(f32), MinContent, #[default] MaxContent }` — `const fn is_definite(self) -> bool`, `const fn definite_value(self) -> Option<f32>`; `From<f32>` (Definite), `From<Option<f32>>` (None → MaxContent). Inherent on `Size<AvailableSpace>`: consts `MAX_CONTENT`, `MIN_CONTENT`; `definite_values(self) -> Size<Option<f32>>`.
- `#[non_exhaustive] pub struct LayoutInput { pub goal: LayoutGoal, pub sizing_mode: SizingMode, pub known_dimensions: Size<Option<f32>>, pub definite_dimensions: Size<bool>, pub parent_size: Size<Option<f32>>, pub available_space: Size<AvailableSpace> }` — constructors `LayoutInput::commit(known_dimensions, parent_size, available_space) -> Self` and `LayoutInput::measure(known_dimensions, parent_size, available_space, requested_axis) -> Self` (both set `sizing_mode: ApplySizeStyles`, `definite_dimensions = known_dimensions.map(is_some)`).
- `#[non_exhaustive] pub struct LayoutOutput { pub size: Size<f32>, pub content_size: Size<f32>, pub first_baselines: Point<Option<f32>> }` — `const HIDDEN`, `new(size, content_size)`, `with_first_baselines(self, Point<Option<f32>>) -> Self`.
- `#[non_exhaustive] pub struct Layout { pub order: u32, pub location: Point<f32>, pub size: Size<f32>, pub content_size: Size<f32>, pub border: Edges<f32>, pub padding: Edges<f32>, pub margin: Edges<f32> }` (`Debug, PartialEq, Default`; not `Copy`/`Clone`) — `with_order(u32) -> Self`.

#### `cache.rs`
- `pub const MEASURE_CACHE_SLOTS: usize = 8;`
- `pub struct Cache` (`Debug, PartialEq, Default`) — `const fn new() -> Self`, `is_empty(&self) -> bool`, `committed_input(&self) -> Option<LayoutInput>`, `get(&self, LayoutInput) -> Option<LayoutOutput>`, `store(&mut self, LayoutInput, LayoutOutput)`, `clear(&mut self)`. (`PackedLayoutInput`/`PackedLayoutOutput`/`MeasurementSlot` are private.)

#### `invalidate.rs`
- `pub fn is_relayout_boundary<S: CoreStyle>(style: &S) -> bool`
- `pub fn invalidate_for_relayout<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId, ancestors: impl Iterator<Item = T::NodeId>) -> T::NodeId`

#### `style` (mod.rs + containment.rs + text.rs)
- Traits: `CoreStyle`, `TextContainerStyle`, `TextRunStyle` (details in §2/§3), `TextRun<'a, R>`, `type TextBrush = ()` — see §3.
- `pub mod containment` with `pub fn effective_containment(...)` (§6).
- Consts: `pub const RELATIVE_REFERENCE_NONE: RelativeReference = -1;` `pub const RELATIVE_REFERENCE_PARENT: RelativeReference = 0;`
- stylo re-exports: see §2.

#### `compute` (mod.rs façade)
Public: `compute_flexbox_layout`, `compute_grid_layout`, `compute_linear_layout`, `compute_relative_layout`, `compute_leaf_layout`, `LeafMeasureInput`, `LeafMetrics`, `NaturalSize`, `compute_root_layout`, `compute_boundary_relayout`, `compute_cached_layout`, `hide_subtree`, `compute_skipped_contents_layout`, `compute_absolute_layout`, `round_layout`, `round_layout_subtree`, `round_layout_subtree_with` (`#[doc(hidden)]`), and `#[cfg(feature = "layout-test-utils")] #[doc(hidden)] compute_leaf_layout_with_measurement_for_testing`. Signatures in §6. (`compute_leaf_layout_with_measurement`, `measure_absolute_layout`, `hide_child_at_order`, `compute_absolute_layout_with_static_position`, all of `single_axis.rs` (`FlowAxes`, `BaseReversals`, `flow_start/flow_end/set_*`, `flow_to_physical`, `measure_child`) and all of `util.rs` are `pub(crate)`/`pub(super)` — NOT public API.)

#### `text` (mod.rs)
Public re-exports: `TextContext` (context.rs), `FontBlob` (font.rs), `TextLayout`, `TextLayoutStore`, `TextMeasurement` (layout.rs), `TextMeasurer` (measure.rs). `content.rs` is private. Details in §7.

### 2. CoreStyle

`pub trait CoreStyle: Sized` in `style/mod.rs`, generated by the local `style_protocol!` macro. **Defaulted-method scheme**: every accessor is a defaulted method whose body reads `self.computed_values()` (or `self.inherited_values()`); a host implements the trait by overriding only `computed_values()` (and anything it wants to special-case). The macro also emits a blanket `impl<S: CoreStyle> CoreStyle for &S` that forwards every method, so accessor overrides survive reference views. Base default for `computed_values()` is a process-wide `LazyLock<Arc<ComputedValues>>` of `ComputedValues::initial_values_with_font_override(Font::initial_values())`. `inherited_values()` defaults to `computed_values()`.

Complete accessor list (B = borrowed from stylo `ComputedValues`, O = owned/copied):

| accessor | return type | B/O |
|---|---|---|
| `computed_values` | `&ComputedValues` | B |
| `inherited_values` | `&ComputedValues` | B |
| `display` | `Display` | O (clone) |
| `visibility` | `visibility::T` | O |
| `position` | `PositionProperty` | O |
| `inset` | `Edges<&Inset>` | B |
| `size` | `Size<&StyleSize>` | B |
| `min_size` | `Size<&StyleSize>` | B |
| `max_size` | `Size<&MaxSize>` | B |
| `aspect_ratio` | `AspectRatio` | O |
| `margin` | `Edges<&Margin>` | B |
| `padding` | `Edges<&NonNegativeLengthPercentage>` | B |
| `border` | `Edges<BorderSideWidth>` | O — *used* widths: zeroed when `border-*-style` is none/hidden |
| `overflow` | `Point<Overflow>` | O |
| `box_sizing` | `box_sizing::T` | O |
| `direction` | `direction::T` | O (reads `inherited_values()`) |
| `containment` | `Contain` | O — empty when `contain` empty + `content-visibility: visible`, or `display: contents`; otherwise `effective_containment(contain, content_visibility, skips_contents())` |
| `contain_intrinsic_width` / `contain_intrinsic_height` | `ContainIntrinsicSize` | O |
| `skips_contents` | `bool` | O — `content-visibility: hidden && !display:contents` |
| `flex_direction` | `flex_direction::T` | O |
| `flex_wrap` | `flex_wrap::T` | O |
| `gap` | `Size<&NonNegativeLengthPercentageOrNormal>` | B (column_gap=width, row_gap=height) |
| `align_content` / `justify_content` | `ContentDistribution` | O |
| `align_items` | `ItemPlacement` | O |
| `flex_basis` | `&FlexBasis` | B |
| `flex_grow` / `flex_shrink` | `NonNegativeNumber` | O |
| `align_self` / `justify_self` | `SelfAlignment` | O |
| `order` | `i32` | O |
| `grid_template_rows` / `grid_template_columns` | `&GridTemplateComponent` | B |
| `grid_auto_rows` / `grid_auto_columns` | `&ImplicitGridTracks` | B |
| `grid_auto_flow` | `GridAutoFlow` | O |
| `justify_items` | `JustifyItems` | O |
| `grid_row_start` / `grid_row_end` / `grid_column_start` / `grid_column_end` | `&GridLine` | B |
| `linear_direction` | `linear_direction::T` | O |
| `linear_weight_sum` / `linear_weight` | `NonNegativeNumber` | O |
| `relative_layout_once` | `relative_layout_once::T` | O |
| `relative_id` | `RelativeReference` | O |
| `relative_align` | `Edges<RelativeAlign>` | O — physical values win over logical (`lower_relative_logical`), inline start/end mapped by `direction` |
| `relative_adjacent` | `Edges<RelativeReference>` | O — same physical-over-logical lowering |
| `relative_center` | `relative_center::T` | O |

**stylo types re-exported through `hughie::style`**:
- `pub use stylo::computed_values::{box_sizing, direction, flex_direction, flex_wrap, linear_direction, relative_center, relative_layout_once, text_wrap_mode, visibility, white_space_collapse}` (keyword modules; values are `<module>::T`)
- `pub use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal`
- `pub use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference}` (`RelativeReference` is an `i32` alias; sentinels `RELATIVE_REFERENCE_NONE = -1`, `RELATIVE_REFERENCE_PARENT = 0`)
- `pub use stylo::values::computed::{AspectRatio, Au, BorderSideWidth, Contain, ContainIntrinsicSize, ContentDistribution, ContentVisibility, Display, FlexBasis, FontFamily, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, GridAutoFlow, GridLine, GridTemplateComponent, ImplicitGridTracks, Inset, ItemPlacement, JustifyItems, LengthPercentage, LetterSpacing, LineHeight, Margin, MaxSize, NonNegativeLengthPercentage, NonNegativeNumber, Overflow, PositionProperty, SelfAlignment, Size as StyleSize, TextAlign, TextIndent, WordBreak}`
- `pub use stylo::values::specified::align::AlignFlags`
- `pub use text::{TextBrush, TextContainerStyle, TextRun, TextRunStyle}`

### 3. TextContainerStyle and TextRunStyle (`style/text.rs`)

`pub trait TextContainerStyle: CoreStyle` — same macro/default scheme; all defaults read `inherited_values().get_inherited_text()`:
- `text_align -> TextAlign`
- `text_wrap_mode -> text_wrap_mode::T`
- `white_space_collapse -> white_space_collapse::T`
- `word_break -> WordBreak`
- `text_indent -> TextIndent`

`pub trait TextRunStyle: Sized` (NOT a CoreStyle subtrait) — pivot accessor `computed_text_values -> Option<&ComputedValues>` (default `None`); every other default maps over it with a hardcoded fallback:
- `font_family -> FontFamily` (owned clone; fallback: initial-values font family)
- `font_family_ref -> Option<&FontFamily>` (borrowed; `None` when no computed values)
- `font_size -> f32` (px; fallback 16.0)
- `font_weight -> FontWeight` (fallback `FontWeight::NORMAL`)
- `font_style -> FontStyle` (fallback `FontStyle::NORMAL`)
- `letter_spacing -> LetterSpacing` (fallback `LetterSpacing::normal()`)
- `line_height -> LineHeight` (fallback `LineHeight::normal()`)
- `font_feature_settings -> FontFeatureSettings` (fallback normal)
- `font_variation_settings -> FontVariationSettings` (fallback normal)

Also:
- `#[derive(Debug)] pub struct TextRun<'a, R: TextRunStyle> { pub text: &'a str, pub style: &'a R, pub preserve_newlines: bool }` — manual `Copy`/`Clone` impls (unconditional on `R`).
- `pub type TextBrush = ();`

### 4. cache module

- **Keying** — the full `LayoutInput`: `goal`, `sizing_mode`, `known_dimensions` (value + presence), `definite_dimensions`, `parent_size` (value + presence), `available_space` (tag + definite value). Packed into `PackedLayoutInput { values: [f32; 6], flags: u16 }` (known w/h, parent w/h, available w/h values; presence/tag/goal/sizing/definite bits). Exact-f32 `PartialEq` semantics preserved (NaN never matches; ±0.0 match). Baseline presence bits (`BASELINE_X/Y_PRESENT`) live in the same flags word but are **output metadata**, excluded from key comparison.
- **Slots** — `committed: Option<MeasurementSlot>` (1 slot) + `measurements: SmallVec<[MeasurementSlot; 8]>` (`MEASURE_CACHE_SLOTS = 8`, inline, never spills). Size budget asserted: `Cache` ≤ 488 bytes, slot ≤ 52.
- **`get` semantics** — checks the committed slot first; a stored `Commit` entry satisfies a requested `Commit` **or** `Measure(Both)` (never single-axis measures). If the request is `Commit` and the committed slot misses, returns `None` without consulting measurement slots. Measurement requests then scan the 8 slots linearly.
- **Equivalence rules** — goal: commit matches commit/measure-Both; measure matches only the identical `RequestedAxis`. `sizing_mode`, `known_dimensions`, `definite_dimensions`, `parent_size` must match exactly (flag bits + f32 values). Available-space is per-axis equivalent when tags+values match, **or** when that axis's known dimension is present and either side's `Definite(v)` equals it (`packed_available_space_matches` — known-dimension fallback, axis-local).
- **Replacement policy (`store`)** — `Commit` overwrites the committed slot. `Measure`: first replace an exact-key slot; else replace the first slot with the same *constraint shape* (per-axis shape = `3` if known-dimension present else the available-space tag 0/1/2, plus matching `definite_dimensions` bits); else push if `len < 8`; else overwrite slot `(width_shape*4 + height_shape) % 8` (`packed_constraint_shape_hash`).
- **`committed_input()`** — returns the unpacked `LayoutInput` of the committed slot; hosts use it to replay/compare the last committed constraints (e.g. boundary relayout). Exposed on `LayoutSlot` as well.

### 5. invalidate module

Exact and complete public API (file is 29 lines, module doc: "Containment-bounded, damage-driven cache invalidation"):

```rust
pub fn is_relayout_boundary<S: CoreStyle>(style: &S) -> bool
// containment().contains(Contain::SIZE) && containment().contains(Contain::LAYOUT)

pub fn invalidate_for_relayout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    ancestors: impl Iterator<Item = T::NodeId>,
) -> T::NodeId
```

`invalidate_for_relayout` clears `node`'s cache, then walks `ancestors` clearing each cache; returns the first ancestor whose style `is_relayout_boundary` (relayout root), else the last ancestor visited, else `node` itself.

**Note for the orchestrator**: this version of `invalidate.rs` contains **no damage-class translation table in its rustdoc** — only the two functions above with the one-line module doc. If a mapping table (damage class → invalidation action) is expected in the design document, it must be sourced from elsewhere (e.g. the host crate) or written fresh; it does not exist in hughie's source at this revision.

### 6. compute module entry points

All generic over `T: LayoutTree`; `state: &mut T::State` is split from the tree borrow.

```rust
pub fn compute_root_layout<T: LayoutTree>(tree: &T, state: &mut T::State, root: T::NodeId, available_space: Size<AvailableSpace>)
```
Drives `tree.compute_layout` with `LayoutInput::commit(Size::NONE, available_space.definite_values(), available_space)`, then resolves and stores the root's own `Layout` (margins incl. horizontal auto-margin distribution against definite available width, padding, border, location = margin offset). `display: none` root stores `Layout::default()`.

```rust
pub fn compute_boundary_relayout<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId, input: LayoutInput) -> LayoutOutput
```
`debug_assert!(is_relayout_boundary(style))`; forces `input.goal = LayoutGoal::Commit` and calls `tree.compute_layout`. Intended for re-laying out a `contain: strict`/skipped subtree in place using its `committed_input()`.

```rust
pub fn compute_cached_layout<T, ComputeFn>(tree: &T, state: &mut T::State, node: T::NodeId, input: LayoutInput, compute_uncached: ComputeFn) -> LayoutOutput
where T: LayoutTree, ComputeFn: FnOnce(&T, &mut T::State, T::NodeId, LayoutInput) -> LayoutOutput
```
Cache read via `tree.layout(state, node).cached_layout(input)`; miss → compute → `store_cached_layout`. This is the wrapper hosts put around their per-node dispatch.

```rust
pub fn compute_leaf_layout<Style: CoreStyle>(input: LayoutInput, style: &Style, natural_size: NaturalSize) -> LayoutOutput
```
Replaced-content leaf: no callback; measures from `NaturalSize` (dimensions + aspect ratio, sanitized). Internal shared engine `compute_leaf_layout_with_measurement(input, style, natural_aspect_ratio: Option<f32>, requires_known_measurement: bool, measure: FnMut(LeafMeasureInput) -> LeafMetrics)` is `pub(crate)` (text path uses it with `requires_known_measurement = true`); exposed publicly only under `feature = "layout-test-utils"` as `compute_leaf_layout_with_measurement_for_testing` (`#[doc(hidden)]`, forces `requires_known_measurement = true`). Handles box-sizing, preferred/natural aspect ratio (content-box vs border-box relation), min/max clamp with padding-border floor, size-containment (`size_containment(style)` substitutes `contain-intrinsic-size` for measurement), single-axis probe short-circuit (fully resolved style sizes skip the measure callback), `content_size = max(size, measured border box)` and baseline offset by content origin.

```rust
pub fn compute_skipped_contents_layout<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId, input: LayoutInput) -> LayoutOutput
```
For `content-visibility` skipped boxes: sizes from known dims / preferred sizes else `contain-intrinsic-size` + box inset, clamped; on `Commit` calls `hide_subtree` on every child. Returns `LayoutOutput::new(outer, outer)`.

```rust
pub fn hide_subtree<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId)
```
Recursively clears the layout cache and stores `Layout::with_order(0)`. (`hide_child_at_order` — same but re-stamps the paint-order slot — is `pub(super)` only.)

```rust
#[must_use = "the returned layout is in containing-block space; the host must convert and store it"]
pub fn compute_absolute_layout<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId, containing_block: Size<f32>, static_position: Point<f32>) -> Layout
```
Full CSS abs-pos: insets/auto-margin resolution (incl. RTL preference), inset-stretch known dimensions, min/max clamp, aspect-ratio deferral, `min-content`/`max-content`/`fit-content()` preferred-available mapping; the returned `Layout.location` is in containing-block space and is NOT stored — host converts/stores. (`measure_absolute_layout` and the static-position-closure variant are `pub(super)`.)

```rust
pub fn round_layout<T: LayoutTree>(tree: &T, state: &mut T::State, root: T::NodeId, scale: f32)
pub fn round_layout_subtree<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId, scale: f32, parent_position: Point<f32>)
#[doc(hidden)] pub fn round_layout_subtree_with<T: LayoutTree>(..., pre_node: impl FnMut(&T, &mut T::State, T::NodeId) -> bool)
```
Reads `LayoutSlot.unrounded`, writes `LayoutSlot.rounded`. CSS half-up rounding of absolute edge coordinates at device `scale` (accumulated absolute position; gap-free adjacent edges); `round_layout_subtree_with` runs a preorder hook whose `false` return prunes later hook calls but still rounds descendants.

Algorithm entry points (uniform signature):
```rust
pub fn compute_flexbox_layout<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId, input: LayoutInput) -> LayoutOutput
pub fn compute_grid_layout<T: LayoutTree>(...)     -> LayoutOutput
pub fn compute_linear_layout<T: LayoutTree>(...)   -> LayoutOutput
pub fn compute_relative_layout<T: LayoutTree>(...) -> LayoutOutput
```

**Containment helpers** and where they live:
- `style/containment.rs`: `pub fn effective_containment(contain: Contain, content_visibility: ContentVisibility, skipped_contents: bool) -> Contain` — folds `content-visibility` into contain bits (`Auto` → +LAYOUT|PAINT|STYLE, +SIZE when skipped; `Hidden` → +LAYOUT|PAINT|SIZE|STYLE). Crate-private in the same file: `contain_intrinsic_length(&ContainIntrinsicSize) -> Option<f32>` (Length/AutoLength → px, None/AutoNone → None), `size_containment<S: CoreStyle>(&S) -> Option<Size<Option<f32>>>` (Some iff `Contain::SIZE`).
- `compute/util.rs` (`pub(super)`, NOT public): `own_scrollable_overflow<S: CoreStyle>(style, border_box: Size<f32>, interior: Size<f32>) -> Size<f32>` — layout containment without a scroll container truncates the reported content size to the border box; `accumulate_scrollable_overflow(content_size: &mut Size<f32>, location: Point<f32>, child_size, child_content_size, child_overflow: Point<Overflow>)` — extends container content size by each child's reach (child content size counts only under `Overflow::Visible`); `is_scroll_container(Point<Overflow>) -> bool`.

### 7. Text measurement seam

- **`TextContext`** (`text/context.rs`): owns Parley `FontContext` + `LayoutContext<TextBrush>`. `new()` (clones a `OnceLock` system-font template), `without_system_fonts()`, `register_fonts(&mut self, data: FontBlob) -> usize` (count of fonts registered, zero-copy), `Default`, `Debug` (non-exhaustive).
- **`FontBlob`** (`text/font.rs`): `Clone + Debug` wrapper over `parley::fontique::Blob<u8>`; `new<Data: AsRef<[u8]> + Send + Sync + 'static>(data) -> Self` (no payload copy), `from_static(&'static [u8])`, `copy_from_slice(&[u8])` (copies), `AsRef<[u8]>`, `From<Vec<u8>>`.
- **`TextLayout`** (`text/layout.rs`, `Debug + Clone`): retained shaped paragraph. Public: `parley_layout(&self) -> &parley::Layout<TextBrush>`, `size(&self) -> Size<f32>`, `first_baseline(&self) -> Option<f32>`, `line_count(&self) -> usize`, `max_advance(&self) -> Option<f32>`. Rebreak/align are `pub(super)` — driven only by the measurer.
- **`TextMeasurement<'a>`** (`Copy`): borrowed view — `layout(self) -> &'a TextLayout`, `size(self) -> Size<f32>`, `first_baselines(self) -> Point<Option<f32>>` (x always `None`).
- **`TextLayoutStore`** (`Debug + Default`): per-node artifact slots `probe: Option<Box<TextLayout>>` + `committed: Option<Box<TextLayout>>` (pointer-sized pair). Public: `probe(&self) -> Option<&TextLayout>`, `committed(&self) -> Option<&TextLayout>`, `invalidate(&mut self)` (clears both).
- **`TextMeasurer`** (`text/measure.rs`):
  ```rust
  pub struct TextMeasurer<'session, 'source, Container, RunStyle, Runs>
  where Container: TextContainerStyle, RunStyle: TextRunStyle + 'source,
        Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone;

  pub fn new(context: &'session mut TextContext, artifacts: &'session mut TextLayoutStore,
             container_style: &'source Container, runs: Runs) -> Self
  pub fn compute_layout(&mut self, input: LayoutInput) -> LayoutOutput   // wraps compute_leaf_layout_with_measurement(requires_known_measurement=true)
  pub fn measure(&mut self, input: LeafMeasureInput) -> TextMeasurement<'_>
  ```
- **`LeafMeasureInput`** (`compute/leaf.rs`, `#[non_exhaustive]`, `Copy + Default`): `{ pub known_dimensions: Size<Option<f32>>, pub available_space: Size<AvailableSpace>, pub goal: LayoutGoal }`; `const fn new(known_dimensions, available_space, goal)`. Content-box constraints (padding/border already subtracted by the leaf engine).
- **`LeafMetrics`** (`#[non_exhaustive]`, `Copy`): `{ pub size: Size<f32>, pub first_baselines: Point<Option<f32>> }`; `new(size)`, `with_first_baselines`, `From<Size<f32>>`.
- **`NaturalSize`** (`#[non_exhaustive]`, `Copy`): private `dimensions: Size<Option<f32>>` + `aspect_ratio: Option<f32>`; `const NONE`, `new(dimensions, aspect_ratio)` (sanitizes non-finite/negative), `from_size(Size<f32>)` (derives ratio), getters `dimensions()`, `aspect_ratio()`, `From<Size<f32>>`.
- **Probe vs commit**: distinguished by `LeafMeasureInput.goal` (`LayoutGoal::Measure(_)` vs `Commit`), which selects the `TextLayoutStore` slot. Shaping happens at most once: a missing probe slot clones the committed artifact, a missing committed slot **takes** (promotes) the probe; only if neither exists does `shape()` run. Each `measure` call re-breaks lines (`rebreak(max_advance, text_indent)`) against the current constraint on the retained artifact; on `Commit` with auto width it additionally shrinks-to-fit (rebreak at measured width when the limit exceeds it) and applies `align(text-align × direction)`. Baselines: y = first line baseline, x = `None`.

### 8. `compute/util.rs` shared machinery (all `pub(super)` — crate-internal but load-bearing for linear/relative)

- **Axis machinery**: `enum Axis { Horizontal, Vertical }` with projections over `Size`/`Point`/`Edges` (`size/point/start/end` + setters), `other()`, `sum(edges)`, `pack(along, across)`, `requested() -> RequestedAxis`; `single_axis.rs` adds `FlowAxes` (main/cross + reversal flags), `flow_start/flow_end` edge accessors, `flow_to_physical`, and `measure_child` (constraint-preserving child measurement wrapper).
- **Length/percentage resolution**: `resolve_length_percentage(&LengthPercentage, basis: Option<f32>) -> Option<f32>` (percent/calc need a definite basis), plus per-property wrappers `resolve_margin`/`resolve_inset`/`resolve_style_size`/`resolve_max_size` and the `Size`/`Edges` lifts `resolve_size`, `resolve_max_sizes`, `resolve_margins`, `resolve_insets`, `resolve_gap(_axis)`.
- **Edge resolution**: `resolve_padding` (non-negative, inline basis), `resolve_border` (used `BorderSideWidth` → f32 px), `auto_edges_to_zero(Edges<Option<f32>>) -> Edges<f32>`, `EdgeMask` (packed per-edge auto-margin bits with flow-relative queries), `box_inset_size(padding, border) -> Size<f32>`.
- **Box-sizing**: `apply_box_sizing(Size<Option<f32>>, box_sizing, padding_border) -> Size<Option<f32>>` (content-box adds the inset), `resolve_quantitative_sizes` / `resolve_quantitative_max_sizes` (resolve + ratio + box-sizing in one step).
- **Aspect-ratio**: `used_aspect_ratio(AspectRatio) -> Option<f32>` (degenerate-safe), `apply_aspect_ratio(Size<Option<f32>>, Option<f32>)` (fills the missing axis), `mirror_ratio_definiteness` / `preferred_size_definiteness` (a definite axis makes the other definite under a ratio).
- **Clamping**: `clamp(value, min: Option<f32>, max: Option<f32>) -> f32` (max wins over min via min-then-max order with 0 floor) and `clamp_axis(value, min, max, floor)` (adds the padding-border floor); `subtract_available_space(AvailableSpace, amount)`.
- **Relative offset**: `relative_offset(inset: Edges<Option<f32>>, direction) -> Point<f32>` — `position: relative` displacement, left-over-right (right-over-left in RTL when both set), top-over-bottom.
- **`ResolvedItemBox` note**: the type is actually named **`ItemGeometry`** (produced by `resolve_item_geometry(_with_bases)(style, percentage_basis)`) — one child's fully resolved preferred/min/max sizes, margin/padding/border, ratio, packed `IntrinsicTags` (min/max/fit-content keyword classification per axis, with `resolve_intrinsic` to materialize them), definiteness, auto-size flags, overflow, box-sizing, auto-margin mask; `impl_item_geometry!` derefs algorithm item records to it.
- **`ResolvedContainerBox`** (`resolve_container_box(style, input)`): the container's resolved padding/border/box-inset, clamped preferred outer size merged with `known_dimensions`, derived inner content size, and per-axis `available_inner: Size<AvailableSpace>`; honors `SizingMode::IgnoreSizeStyles`.
- **Ordering**: `OrderedItem`/`ItemKey`/`PendingLayoutItem` + `sort_and_assign_layout_order` (CSS `order`-aware paint-order assignment interleaving in-flow and out-of-flow children).
- **Alignment**: `normalize_item_alignment` / `normalize_content_alignment` (`AlignFlags` → canonical subset, left/right → start/end by direction, last-baseline → end for items).
- **Overflow**: `is_scroll_container`, `accumulate_scrollable_overflow`, `own_scrollable_overflow` (see §6).

**Mapping caveats for the taffy comparison**: (a) hughie has no `Style` struct — style is a borrowed trait view over stylo `ComputedValues` (taffy `Style` fields ↔ CoreStyle accessors); (b) `LayoutInput` differs from taffy's `LayoutInput` by `goal: LayoutGoal` replacing taffy's `RunMode`+`requested_axis` pair, an added `definite_dimensions: Size<bool>`, no `vertical_margins_are_collapsible`; (c) `LayoutTree` splits taffy's single `&mut self` tree into `&T` + `&mut T::State` with `layout/layout_mut` returning `LayoutSlot` (cache + static_position + unrounded + rounded in one slot; taffy's `CacheTree`/`LayoutPartialTree` split); (d) `compute_absolute_layout`/`compute_boundary_relayout`/`compute_skipped_contents_layout`/`compute_linear_layout`/`compute_relative_layout`, `FlattenedChildren`, and the probe/commit committed-slot cache semantics have no taffy counterpart; (e) rounding is accumulated-absolute + host-scale (`round_layout(tree, state, root, scale)`), not taffy's `round_layout(tree, root)`.

---

# dom layout host inventory

> Provenance: Read directly from crates/dom at this branch.

## dom layout host inventory (taffy low-level-API migration input)

Base path: `/Users/akiwah/repos/lynx-vello/.claude/worktrees/funny-newton-c16b43/crates/dom/src` (relative below). The layout engine crate is `crates/hughie` (trait `hughie::tree::LayoutTree`, `crates/hughie/src/tree/mod.rs:49`).

### 1. The `LayoutTree` implementation

`impl<T> LayoutTree for TreeArenas<T>` — `layout/host.rs:31`. `TreeArenas<T>` (`tree/document.rs:44`) is the immutable side: `{ nodes: Slab<Node<T>>, payloads: Slab<PayloadSlot<T>> }`. Layout state is a *separately borrowed* second parameter (statically split borrows; no interior mutability).

Associated types:
- `type NodeId = NodeId` (= `usize`, `tree/document.rs:23`); trait bound only `Copy + Debug`.
- `type State = DocumentLayoutState`.
- `type Style<'tree> = StyleView<'tree, T>` (GAT).
- `type ChildIter<'tree> = core::iter::Copied<core::slice::Iter<'tree, NodeId>>`.

Methods:
- `children(node)` / `child_count(node)`: `slab_get_for_live_node(&self.nodes, node).flat_children().iter().copied()` — the **shadow flat tree** child slice (`tree/shadow.rs:88`, returns `&[NodeId]`), including box-less and text children.
- `style(node)`: `StyleView::of(node)` — falls back to `ANONYMOUS_STYLE` (`layout/mod.rs:29`, `LazyLock<Arc<ComputedValues>>` = initial values with font override) for unstyled nodes.
- `layout(state, node)` / `layout_mut(state, node)`: index `state.nodes[node].slot` (`&LayoutSlot` / `&mut LayoutSlot`).
- `clear_layout_cache(state, node)`: delegates to `DocumentLayoutState::clear_layout_cache` (also invalidates the node's `TextLayoutStore`).
- Inherited defaults used as-is: `flattened_children` (`hughie/src/tree/mod.rs:70` — dissolves `display:contents` recursively via a `SmallVec<[ChildIter; 2]>` stack, yields `(NodeId, Style<'tree>, Display)`), `set_unrounded_layout`, `set_static_position`.

#### Style GAT

`StyleView<'dom, T> { node: &'dom Node<T>, style: &'dom ComputedValues }` (`layout/style.rs:149`, exactly 2 words — size-asserted). `style` is `Node::layout_computed_style()` (`tree/node.rs:449`): the per-node **Arc snapshot** of Stylo's primary style captured at damage-harvest time (debug-asserted `Arc::ptr_eq` against Stylo's live primary; lending needs no Stylo runtime borrow and no Arc clone). `CoreStyle` impl overrides exactly two methods: `computed_values()` → the snapshot, and `position()` → `resolve_position(self.node, values)` (parent-dependent lowering, §6).

`TextStyleView<'dom> { text_style: &'dom ComputedValues }` (1 word): built from a text node's **flat parent's** `layout_computed_style()` (fallback `ANONYMOUS_STYLE`). `CoreStyle` overrides: `computed_values()` → `ANONYMOUS_STYLE` (static anonymous-box geometry: no margins/borders/padding/size), `inherited_values()` → the parent style (direction, paragraph values). Also `impl TextContainerStyle` (all defaults) and `impl TextRunStyle { fn computed_text_values(&self) -> Option<&ComputedValues> { Some(self.text_values()) } }`.

#### `compute_layout` display dispatch — verbatim structure (`layout/host.rs:76-149`)

```
let display = if node_ref.is_text_node() { DisplayMode::Leaf }
else {
    let view = self.style(node);
    let display = display_mode(view.display());
    if display == DisplayMode::None { hide_subtree(self, state, node); return LayoutOutput::HIDDEN; }
    if view.skips_contents() { return compute_skipped_contents_layout(self, state, node, input); }
    if node_ref.is_replaced() { DisplayMode::Leaf } else { display }
};
compute_cached_layout(self, state, node, input, move |tree, state, node, input| match display {
    DisplayMode::None | DisplayMode::Contents => unreachable!("a box-less element has no box to lay out"),
    DisplayMode::Flex     => compute_flexbox_layout(...),
    DisplayMode::Grid     => compute_grid_layout(...),
    DisplayMode::Linear   => compute_linear_layout(...),
    DisplayMode::Relative => compute_relative_layout(...),
    DisplayMode::Leaf => {
        let output = if node_ref.is_text_node() {
            let view = TextStyleView::of(node_ref);
            let run = TextRun { text: node_ref.text().unwrap_or_default(), style: &view, preserve_newlines: false };
            let (context, artifacts) = state.text_parts(node);
            TextMeasurer::new(context, artifacts, &view, std::iter::once(run)).compute_layout(input)
        } else {
            // layout-test-utils: test_leaf_metrics → compute_leaf_layout_with_measurement_for_testing
            compute_leaf_layout(input, &view, node_ref.natural_size())
        };
        if input.goal == LayoutGoal::Commit { for grandchild in tree.children(node) { hide_subtree(tree, state, grandchild); } }
        output
    }
})
```

Key facts: text nodes bypass style dispatch entirely (Leaf before any style read). `display:none` and `content-visibility:hidden` (skipped contents) are handled **outside** the cache wrapper (`hide_subtree` zeroes the subtree; `compute_skipped_contents_layout` sizes from `contain-intrinsic-size` without descending). Replaced elements (`Node::is_replaced`, natural size set) force Leaf regardless of display inside. `Contents` is unreachable because every container algorithm iterates `flattened_children`. A `DisplayInside::Flow`/`Contents` inside value maps to `DisplayMode::Leaf` ("flow containers fall back to leaves and zero their children" — leaf commit hides all children). All `compute_*` entry points come from `hughie::compute` (host.rs:6-11): `compute_absolute_layout, compute_boundary_relayout, compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, compute_linear_layout, compute_relative_layout, compute_root_layout, compute_skipped_contents_layout, hide_subtree, round_layout_subtree_with`.

`display_mode` (`layout/style.rs:27`): `is_contents()` → Contents; `outside()==None` → None; then inside: `None→None, Flex, Grid, LynxLinear→Linear, LynxRelative→Relative, Contents|Flow→Leaf`.

### 2. `DocumentLayoutState` / per-node state (`tree/document.rs:74-132`)

```rust
#[derive(Default)]
pub(crate) struct NodeLayoutState {
    pub(crate) slot: LayoutSlot,                       // hughie: { cache: Cache, static_position: Point<f32>, unrounded: Layout, rounded: Layout } — 640 B
    pub(crate) text: Option<Box<TextLayoutStore>>,     // lazily boxed per text node
    pub(crate) scroll_offset: euclid::default::Vector2D<f32>,
}                                                      // 656 B (size-probed in layout/mod.rs tests)
pub(crate) struct DocumentLayoutState {
    pub(crate) nodes: Slab<NodeLayoutState>,
    pub(crate) text_context: Option<Box<TextContext>>, // one document-wide font/shaping context, lazily boxed
}
```

Slab alignment: `DocumentLayoutState::insert/remove` assert `vacant_key() == id` in lockstep with `TreeArenas.nodes` and `.payloads` — three NodeId-aligned slabs, freed together in `Document::free_node` (`tree/document.rs:641-660`). `text_parts(id) -> (&mut TextContext, &mut TextLayoutStore)` destructures self to hand both mutable halves to `TextMeasurer`. `clear_layout_cache(id)` clears `slot` cache and calls `TextLayoutStore::invalidate()`.

### 3. `Document::layout` end-to-end (`layout/mod.rs:34-51` + `layout/host.rs:156-191`)

1. **Style flush + damage consumption**: `self.flush_styles_with_damage_sink(&mut |_, _| {})` (`style/flush.rs:106`) runs the Stylo traversal, then `harvest_flush` (`tree/document.rs:~760`): per damaged node, if `damage.needs_relayout()` — clear the layout caches of its **text children** (their style rides the parent), call `self.invalidate_layout(current)`, and if `damage.requires_reconstruction()` also `invalidate_layout(parent)`. Structural DOM mutations (append/insert/detach/text set) call `invalidate_layout` directly (`tree/document.rs:500,538,776`; `style/invalidation.rs:328`).
2. **Idle-frame skip**: `layout_needs_pass(viewport, scale) = layout_dirty || last_layout_inputs != Some((viewport, scale))`; false → return (idempotent; test `idle_frames_are_skipped_and_stay_idempotent`).
3. **Scope decision**: `full = layout_requires_full_pass = layout_root_dirty || inputs changed`. `layout_dirty`/`layout_root_dirty` are set by `mark_layout_dirty(reached_root)`; only a root-reaching invalidation (one that hit no parked boundary and no skipped-contents ancestor) sets `layout_root_dirty`.
4. **`run_layout(document, viewport, scale, full)`**:
   - `collect_parked_boundaries`: snapshot `relayout_roots: Vec<PendingRelayout { node_id, input: LayoutInput }>` as `(depth, id, committed_input)`, sorted **deepest-first** (`sort_by_key(Reverse(depth))`).
   - `let (tree, state, parked_ids) = document.layout_parts()` — `(&TreeArenas<T>, &mut DocumentLayoutState, &FxHashSet<NodeId>)`.
   - **Boundary re-runs**: for each parked entry still live, an element, and `is_relayout_boundary(&StyleView::of(node))` (hughie predicate over containment): `compute_boundary_relayout(tree, state, id, input)` and write `output.content_size` into `slot.unrounded.content_size` (scrollable overflow refresh).
   - **Root pass**: `compute_root_layout(tree, state, document_element, Size::new(AvailableSpace::Definite(w), Definite(h)))` — always runs; boundary caches make it cheap when nothing root-reaching changed.
   - **Fused positioned+rounding traversal**: if `full`: `round_layout_subtree_with(tree, state, root, scale, Point::ZERO, pre_position)`. Else `position_and_round_parked_boundaries`: for each parked boundary (skipping any with a parked ancestor via `has_parked_ancestor`/`parked_ids`), seed origin = `accumulated_unrounded_origin(flat parent)` and `round_with` from that boundary only.
5. `clear_relayout_roots()`; `mark_layout_complete(viewport, scale)` clears both dirty bits and stores the inputs.

**`invalidate_layout(id)` funnel** (`layout/mod.rs:145-196`): clear cache at `id`, then walk `flat_parent_id` chain upward. Per ancestor: if `StyleView` `skips_contents()` → stop, `reached_root=false`, park nothing (mutation deferred until reveal); if `is_relayout_boundary` and already in `parked_ids` → stop (dedup); if boundary with a `slot.committed_input()` → clear its cache, park `(node_id, input)` via `record_relayout_root`, stop; else clear cache and continue. Ends with `mark_layout_dirty(reached_root)`. `invalidate_layout_all()` (font registration) clears every slot + text store, drops parked roots, dirties root. Node removal calls `prune_relayout_roots` (`tree/document.rs:661`) so a reused NodeId never replays a stale boundary.

**`pre_position` hook duties** (`layout/host.rs:282-310`, returns `bool` = descend): unstyled node (`StyleView::try_of` None) → `false` (prunes unstyled subtrees); `DisplayMode::None` → `false`; `DisplayMode::Contents` → **zero the slot** (`layout_mut(..).unrounded = Layout::default()`) and return `true` (children still descend, they were laid out against the box parent); if the node has an element flat parent and `resolve_position(..) == PositionProperty::Fixed` (i.e. lowered to viewport-anchored *or* CB-escaping absolute) → `position_hoisted(.., fixed: values.clone_position()==Fixed)`; finally descend iff `display != Leaf && !skips_contents(values)` (skipped-subtree pruning).

### 4. The positioned pass (`position_hoisted`, `layout/host.rs:312-374`)

Runs inside the rounding traversal, per hoisted node, preorder (test `fixed_inside_nested_hoisted_subtrees_completes_in_preorder`):
- **CB resolution**: walk flat ancestors; first one where `establishes_fixed_containing_block(node, style)` (computed `position:fixed`) or `establishes_absolute_containing_block` (otherwise) — predicates in §6. Found block: `containing_origin = accumulated_unrounded_origin(block) + (border.left, border.top)`, `containing_size = (size − border sums).max(0)` (the **padding box**). No block: `(Point::ZERO, viewport)`.
- **Static-position conversion**: `static_in_cb = parent_origin + slot.static_position − containing_origin`, where `static_position` was recorded by the in-flow algorithms via `set_static_position` and `parent_origin = accumulated_unrounded_origin(flat parent)`.
- `let mut layout = compute_absolute_layout(tree, state, node_id, containing_size, static_in_cb)` — result in CB space.
- **Store back in parent space**: `layout.location = containing_origin + layout.location − parent_origin`; `layout.order = sibling_paint_order(tree, ordering_parent, node)` where `ordering_parent = box_parent(node)` (skips `display:contents` ancestors) — rank among the box parent's `flattened_children` by `(effective_order, index)` with hoisted siblings pinned at effective order 0 and `display:none` siblings excluded; then `tree.layout_mut(state, node_id).unrounded = layout`. The subsequent rounding of that subtree snaps it; `visual/mod.rs` relies on locations staying box-parent-relative.

### 5. `display:contents` special-case sites

- `layout/style.rs:27` `display_mode` → `DisplayMode::Contents`; `generates_no_box` (`:48`); `skips_contents` excludes contents (`:52`); both CB predicates return `false` for contents (`:60,91`); `box_parent` (`:103`) skips contents ancestors; `resolve_position` consults `box_parent`.
- `layout/host.rs:104` dispatch `unreachable!` (dissolution happens in `hughie` `FlattenedChildren`, used by every container algorithm and by `sibling_paint_order`).
- `layout/host.rs:296` `pre_position` zeroes the contents element's `unrounded` slot each pass but still descends.
- `layout/host.rs:371` hoisted ordering parent = `box_parent` (test `absolute_child_of_contents_anchors_to_the_box_ancestor`).
- `hughie/src/style/mod.rs:149` `CoreStyle::containment` default: contents → `Contain::empty()`; `skips_contents` default excludes contents.
- `scroll/mod.rs` `scroll_parent`/`containing_block` use `box_parent` (`:53` imports).
- `visual/build.rs`: root gate `display_mode != None` (`:66`); `collect` walks `flattened_children` with `debug_assert_ne!(mode, DisplayMode::Contents)` (`:310`); invariants doc `visual/mod.rs:40-46` (boxless paints/stacks/clips nothing, relays only visibility/pointer-events; text under a boxless parent hit-targets that parent; replaced-element "unbox" rule deliberately unimplemented, `visual/mod.rs` limits).
- Query surface: `rounded_layout` on a contents element returns the zeroed `slot.rounded` (all-default `Layout`).
- Tests: nine `display_contents_*` / `contents_*` cases in `tests/layout.rs` (lines 351-505, 1620-1677).

### 6. Style lowering (`layout/style.rs` vs `hughie` `CoreStyle` defaults)

The host **overrides only**: `StyleView::computed_values()` (Arc snapshot) and `StyleView::position()` → `resolve_position` (`:114`): `Static|Relative|Sticky` pass through; `Absolute` stays `Absolute` iff `box_parent` establishes an absolute CB, else lowers to `Fixed` (= "escapes, hoist it"); computed `Fixed` lowers to `Absolute` iff `box_parent` establishes a fixed CB (transform-ancestor rule), else stays `Fixed`. Everything else comes from `hughie` `CoreStyle` defaults over `computed_values()` (`crates/hughie/src/style/mod.rs:82-279`, `style_protocol!` macro): inset/size/min/max/margin/padding, **border used-widths zeroed for `none|hidden` styles**, `overflow`, `box_sizing`, `direction` (from `inherited_values`), the **`effective_containment` fold** (`contain` ∪ implied bits from `content-visibility`, empty for contents), `contain_intrinsic_{width,height}`, `skips_contents`, flex set, grid set, linear set (`linear_direction/weight/weight_sum`), and the **logical `relative-*` lowering** (`relative_align`/`relative_adjacent`: physical wins over logical via `lower_relative_logical`, `direction` maps inline-start/end onto left/right; sentinels `RELATIVE_REFERENCE_NONE=-1`, `RELATIVE_REFERENCE_PARENT=0`).

Local host predicates: `establishes_fixed_containing_block` (`:56`): non-contents ∧ (transform list non-empty ∨ perspective ∨ `offset-path` ∨ will-change TRANSFORM|PERSPECTIVE|CONTAIN ∨ (FIXPOS_CB_NON_SVG ∧ non-root) ∨ effective containment ∩ (LAYOUT|PAINT) ∨ (filter non-empty ∧ non-root)). `establishes_absolute_containing_block` (`:87`): non-contents ∧ (`position != static` ∨ will-change POSITION ∨ fixed-CB). `is_root_element` = flat parent absent or document.

### 7. Public layout query APIs and consumers

On `Document<T>` (`layout/mod.rs` unless noted): `pub fn layout(&mut self)` (`T: Sync`); `pub fn set_natural_size(&mut self, id: NodeId, natural_size: NaturalSize)` (panics on vacant/non-element; invalidates on change); `pub(crate) fn natural_size(&self, id) -> NaturalSize`; `pub fn register_fonts(&mut self, data: FontBlob) -> usize` (nonzero → `invalidate_layout_all`); `pub fn rounded_layout(&self, id) -> Option<&Layout>` (reads `slot.rounded`); `pub(crate) fn text_layout(&self, id) -> Option<&TextLayout>` (via `TextLayoutStore::committed()`); `pub(crate) fn paint_style(&self, id) -> Option<&ComputedValues>`; cfg-gated `set_leaf_metrics_for_testing`, `invalidate_layout_for_testing`, `layout_cache_is_empty`. Scroll (`scroll/mod.rs`): `is_scroll_container`, `scroll_box(id) -> Option<ScrollBox { scrollport, scroll_size, offset, user_scrollable }>` — the content-size reader is `scroll::resolve` (`:133`), computing scrollport = size − borders and scroll_size from `Layout::content_size` clamped to scrollport; `scroll_offset`, `scroll_to`, `scroll_by`, `nearest_user_scrollable`, `scroll_chain`. Re-exports: `layout/mod.rs` `pub use hughie::{compute::NaturalSize, geometry::Size, tree::Layout}`; `lib.rs:23` `pub use hughie::text::FontBlob`.

Consumers: **paint** — `paint/walker.rs` (`rounded_layout`, `paint_style`, `text_layout`, `natural_size`), `paint/text.rs` (`paint_style`); **visual** — `visual/build.rs` bypasses public APIs via `visual_parts() -> (&TreeArenas<T>, &DocumentLayoutState)` and reads `slot.rounded` directly, plus reuses `layout::{DisplayMode, StyleView, box_parent, display_mode, establishes_*_containing_block, skips_contents}` and `LayoutTree::flattened_children`; `visual/stacking.rs` reuses `skips_contents` in its `effective_containment` fold; `visual/geometry.rs` uses `hughie::geometry::Edges`; **scroll/input** — `crate::input` drives `scroll_chain`; **lynx-element** — `crates/lynx-element/src/tree.rs:237` `flush_element_tree` → `document.layout()`, `:128` `register_fonts` passthrough, tests read `rounded_layout`; **bobcat-core** — `crates/bobcat-core/src/engine.rs:330` `register_fonts` passthrough (layout runs inside the element-tree flush).

### 8. Text-node layout

Text nodes carry no style of their own. Dispatch forces Leaf (§1). `TextStyleView::of(node)` borrows the **flat parent's** snapshot style for inherited paragraph/run values while `computed_values()` is the static `ANONYMOUS_STYLE` (anonymous-box geometry: zero margins/borders/padding, auto size). One `TextRun { text, style: &view, preserve_newlines: false }`; `TextMeasurer::new(context, artifacts, &view, iter::once(run))` is constructed inside `compute_layout` (`layout/host.rs:120-123`) from `state.text_parts(node)` — the shared boxed `TextContext` plus the node's lazily boxed `TextLayoutStore`; `measurer.compute_layout(input)` measures/commits. Inherited-style changes reach the text child because `harvest_flush` clears the layout caches of a damaged element's text children (§3), and `tests/layout.rs` `inherited_text_style_change_remeasures_*` plus `color_only_change_preserves_text_geometry` pin the behavior. Committed geometry is read back by paint via `Document::text_layout`.

### 9. Layout-exercising test files

- `crates/dom/tests/layout.rs` — the host behavior suite (~80 tests): flex/grid/linear/relative geometry, box-sizing/borders/calc/min-max/aspect-ratio, `order`, RTL, `display:none` recovery, the full `display:contents` series, absolute/fixed CB establishment (transform/offset-path/will-change/filter, root exemptions), hoisting + static position + effective paint order, boundary parking (nested, deepest-first, incremental == full equivalence incl. fractional DPR), `content-visibility:hidden` skip/deferral/reveal, text via Ahem + remeasure, device-pixel rounding, layout-state lifecycle with node removal/id reuse, boundary scrollable content-size, idle-frame idempotence.
- `crates/dom/tests/grammar_layout.rs` — layout-property *grammar* only (flex/grid/aspect-ratio/gap/four-sides parse → computed strings); no host pass.
- `crates/dom/tests/replaced_content.rs` — natural size → leaf layout → `object-fit` paint geometry, end to end.
- `crates/dom/src/layout/mod.rs` (inline tests) — struct-size probes (`Node`/`LayoutSlot`/`NodeLayoutState`/`TextLayoutStore`), dirty-spine invalidation from `set_natural_size`, full-vs-incremental pass classification.
- `crates/dom/src/visual/tests.rs` — stacking/Appendix-E order/transforms/clips/hit-testing over laid-out documents (harness runs `layout()`).
- `crates/dom/src/scroll/behavior_tests.rs` — scroll geometry/clamping/chaining/CB escapes/input over committed layout.
- `crates/dom/tests/style.rs` — flush/damage semantics asserted through `doc.layout()` + resulting geometry (23 layout calls).
- `crates/dom/tests/shadow.rs` — layout follows the slot-assigned flat tree.
- `crates/dom/tests/custom_elements.rs` — layout after upgrade/reaction flushes (1 call).
- `crates/dom/tests/media_queries.rs` — viewport-conditional style through a layout pass (1 call).
- `crates/dom/tests/primitives.rs` — document primitives incl. layout-state death with its node (3 calls).
- `crates/dom/tests/screenshots.rs`, `tests/text_screenshots.rs`, `tests/gpu_pixels.rs`, `tests/scene.rs` — pixel/scene goldens exercising layout implicitly through the paint pipeline (harnesses in `tests/common/mod.rs` / `tests/paint_common`).

Migration-relevant invariants: three NodeId-aligned slabs (tree/payload/layout) with lockstep insert/remove asserts; state split (`&TreeArenas` + `&mut DocumentLayoutState`) is the borrow architecture everything (visual, scroll, paint) leans on; `Layout.location` is box-parent-relative *including* hoisted boxes (visual/build relies on it); `hughie`-specific algorithms with no taffy equivalent: Linear, Relative, `compute_boundary_relayout`, `compute_skipped_contents_layout`, the fused `round_layout_subtree_with(pre_position)` hook, `static_position` stored in the slot, and `FlattenedChildren` as a trait-provided iterator.

---

# Linear/Relative protocol requirements

> Provenance: Read directly from crates/hughie/src/compute/{linear,relative}.rs and the Starlight spec docs at this branch.

## Hughie Linear/Relative → taffy-style trait migration analysis

All paths relative to `/Users/akiwah/repos/lynx-vello/.claude/worktrees/funny-newton-c16b43/`. Protocol definitions: `crates/hughie/src/tree/mod.rs` (`LayoutTree`, `LayoutSlot`, `FlattenedChildren`), `crates/hughie/src/tree/io.rs` (`LayoutInput`, `LayoutOutput`, `Layout`, `LayoutGoal`, `SizingMode`, `RequestedAxis`, `AvailableSpace`), `crates/hughie/src/style/mod.rs` (`CoreStyle` — a single trait with defaulted accessors over `stylo::properties::ComputedValues`).

Vocabulary mapping to taffy: `LayoutGoal::Commit` ≈ `RunMode::PerformLayout`; `LayoutGoal::Measure(RequestedAxis)` ≈ `RunMode::ComputeSize` + `LayoutInput.axis`; `SizingMode::ApplySizeStyles` ≈ taffy `SizingMode::InherentSize`; `SizingMode::IgnoreSizeStyles` ≈ taffy `SizingMode::ContentSize`; `RequestedAxis::{Horizontal,Vertical,Both}` matches taffy's enum exactly. `LayoutInput.definite_dimensions: Size<bool>` **has no taffy counterpart** (see §2). `LayoutOutput { size, content_size, first_baselines: Point<Option<f32>> }` is a subset of taffy's (no collapsible-margin fields; hughie never needs them).

---

### 1. CoreStyle accessors read

#### 1a. Linear (`crates/hughie/src/compute/linear.rs`, entry `compute_linear_layout`)

Container (`tree.style(node)`):

| Accessor | Stylo computed type | Use |
|---|---|---|
| `containment()` | `Contain` (bitflags) | `Contain::LAYOUT` check (baseline suppression, contained fast path, `own_scrollable_overflow`) |
| `contain_intrinsic_width()`/`contain_intrinsic_height()` | `ContainIntrinsicSize` | via `size_containment()` (`src/style/containment.rs`) |
| `linear_direction()` | `stylo::computed_values::linear_direction::T` (`Row\|RowReverse\|Column\|ColumnReverse`) | axis derivation `linear_axes()` |
| `direction()` | `direction::T` (`Ltr\|Rtl`) | axis reversal |
| `align_items()` | `ItemPlacement` (newtype over `AlignFlags`) | cross-gravity fallback |
| `justify_content()` | `ContentDistribution` | main gravity |
| `linear_weight_sum()` | `NonNegativeNumber` (`NonNegative<f32>`, `.0` read) | weight-sum override |
| `overflow()` | `Point<Overflow>` | `own_scrollable_overflow` |
| via `resolve_container_box` (util.rs:975): `size()`, `min_size()` (`Size<&StyleSize>`), `max_size()` (`Size<&MaxSize>`), `aspect_ratio()` (`AspectRatio`), `box_sizing()` (`box_sizing::T`), `padding()` (`Edges<&NonNegativeLengthPercentage>`), `border()` (`Edges<BorderSideWidth>`), `margin()` (`Edges<&Margin>`) | | |

Item (from `flattened_children` styles and later `tree.style(child)` re-reads):

- `position()` → `PositionProperty` (`Static|Relative|Absolute|Fixed|Sticky`) — item classification.
- `display` — delivered by the flattened iterator; `Display::is_none()` (hidden), `is_contents()` (flattening, inside `FlattenedChildren`).
- `order()` → `i32` — CSS order sort key.
- `align_self()` → `SelfAlignment` (newtype over `AlignFlags`) — per-item cross gravity, also for **absolute** children's static-position gravity.
- `linear_weight()` → `NonNegativeNumber`.
- `direction()` → `direction::T` — item-level, only for the `position: relative` inset nudge (`relative_offset`).
- `inset()` → `Edges<&Inset>` — three uses: percent-dependency detection (`initial_item_flags`), relative nudge resolution, and absolute/fixed `static_axes` detection (`inset.left.is_auto() && inset.right.is_auto()`, linear.rs:1415-1418).
- `margin()` → `Edges<&Margin>`, `padding()` → `Edges<&NonNegativeLengthPercentage>` — percent-dependency detection (`Margin::LengthPercentage(lp).has_percentage()`, linear.rs:220-246) and deferred refresh (`refresh_item_edges`).
- via `resolve_item_geometry` (util.rs:898): `size()`, `min_size()`, `max_size()`, `aspect_ratio()`, `box_sizing()`, `overflow()`, `padding()`, `border()`, `margin()`.
- `resolve_intrinsic_sizes` (linear.rs:429) re-reads `size()/min_size()/max_size()` for intrinsic-keyword payloads (`FitContentFunction` limit).
- `tree.style(child).overflow()` for absolute-child overflow accumulation (linear.rs:1235).
- Absolute path (`compute/mod.rs` `resolve_absolute_style`): `padding()`, `border()`, `size()`, `min_size()`, `max_size()`, `box_sizing()`, `inset()`, `margin()`, `aspect_ratio()`, `direction()`.

#### 1b. Relative (`crates/hughie/src/compute/relative.rs`, entry `compute_relative_layout`)

Container: `containment()`/`contain_intrinsic_*` (via `size_containment`), `relative_layout_once()` → `relative_layout_once::T` (`True|False`; **initial = True**), `overflow()`, plus the full `resolve_container_box` set as above.

Item:

- `position()`, `order()`, `display` (iterator) — classification, as in linear.
- `relative_id()` → `RelativeReference` = `CSSInteger` = **`i32`** (`vendor/stylo/style/values/computed/lynx_layout.rs`; sentinels `RELATIVE_REFERENCE_NONE = -1`, `RELATIVE_REFERENCE_PARENT = 0` in `src/style/mod.rs`).
- `relative_align()` → `Edges<RelativeAlign>` (`RelativeAlign = i32`). The `CoreStyle` default impl **lowers logical** `relative-align-inline-start/end` onto physical left/right using the item's `direction()`, physical-wins tie-break (`lower_relative_logical`, style/mod.rs:74-80).
- `relative_adjacent()` → `Edges<RelativeReference>` — same lowering for `relative-{left,right,top,bottom}-of` + inline variants.
- `relative_center()` → `relative_center::T` (`None|Horizontal|Vertical|Both`).
- `direction()` — stored per item, used only for the `position: relative` commit nudge.
- `inset()` — relative nudge (`resolve_insets`, relative.rs:176).
- via `resolve_item_geometry_with_bases` (util.rs:910): same nine box accessors as linear, but with **separate size-percent basis and edge-inline basis** (edges resolve against available width, sizes against definite parent size — Starlight §2.1/2.2).
- `prepare_intrinsic_sizes` re-reads `size()/min_size()/max_size()` (relative.rs:575-578).
- `measurement_input` re-reads raw `size()` per axis to variant-match `StyleSize::{MinContent,MaxContent,FitContentFunction}` in `fit_content_available` (relative.rs:502-541).
- `commit_out_of_flow`: `position()`, `overflow()`; absolute path as in linear.

**Note for both**: the item algorithms need *raw stylo value objects* (`&StyleSize`, `&Margin`, `&Inset`, `&LengthPercentage`), not pre-resolved floats — for `has_percentage()` dependency probing, intrinsic-keyword tagging (`IntrinsicTags`), `is_auto()`, and `fit-content()` payloads. Taffy's `CoreStyle` returns its own `Dimension`/`LengthPercentageAuto`, which **cannot represent `min-content`/`max-content`/`fit-content()`/`stretch`** — the trait pair must keep stylo types (or an enriched dimension enum).

---

### 2. LayoutInput fields read, and the `definite_dimensions` problem

`LayoutInput` (tree/io.rs:84-93): `goal`, `sizing_mode`, `known_dimensions: Size<Option<f32>>`, `definite_dimensions: Size<bool>`, `parent_size: Size<Option<f32>>`, `available_space: Size<AvailableSpace>`. Constructors `commit()`/`measure()` default `definite_dimensions = known_dimensions.map(|v| v.is_some())`; algorithms override it. It is also part of `PartialEq`/`Hash` → **a cache-key component** (`LayoutSlot::cached_layout(input)`).

#### Linear

- `input.goal` — linear.rs:1288 `let commits_layout = input.goal == LayoutGoal::Commit;` (gates hidden/absolute collection, layout-order assignment, baseline probing, commit pass, contained-measure fast return at 1341). The measure-goal's `RequestedAxis` is ignored — linear always produces both axes.
- `input.sizing_mode` — linear.rs:1312 `if input.sizing_mode != SizingMode::IgnoreSizeStyles` gates container aspect-ratio/clamp application; also consumed inside `resolve_container_box` (IgnoreSizeStyles ⇒ style sizes treated absent).
- `input.known_dimensions` — only via `resolve_container_box` (`outer = input.known_dimensions.or(preferred)`, util.rs:1026).
- `input.parent_size`, `input.available_space` — only via `resolve_container_box` (percent bases; `available_inner` = available minus margins+box inset when inner indefinite).
- `input.definite_dimensions` — **consumed** at linear.rs:1307-1311:
  ```rust
  let mut outer_definite = Size::new(
      input.definite_dimensions.width || style_definite.width,
      input.definite_dimensions.height || style_definite.height,
  );
  mirror_ratio_definiteness(&mut outer_definite, container_aspect_ratio);
  ```
  Downstream: (a) lines 1364-1370 null `definite_inner_size` per non-definite axis, and that becomes `percentage_basis` for children — a known-but-not-definite size must not resolve child percentages; (b) line 1482 `if !outer_definite.width && has_box_basis_dependency` triggers the second-phase margin/padding refresh against the final content-derived width.
  **Produced** at: `commit_in_flow` linear.rs:1139-1141 `input.definite_dimensions = axes.main.pack(item.main_size_is_definite, item.cross_size_is_definite);` (with `sizing_mode = IgnoreSizeStyles`); `intrinsic_measurement` linear.rs:378-381 (definite only when `preferred_definite` and known); `measure_item` via `measure_child(..., known_definite, ...)` where stretch-forced cross sizes are **flagged definite** (linear.rs:673) even though they came from the parent.

#### Relative

- `input.goal` — relative.rs:1301 `matches!(input.goal, LayoutGoal::Measure(_))` (children-skipping fast path when both outer axes known or size-contained), 1346 `commits_layout`.
- `input.known_dimensions` — via `resolve_container_box`, plus **directly** as `caller_known` in `final_outer_axis` (relative.rs:1040-1044: a caller-known size wins outright, `known.max(0.0)`) at 1308/1316/1439 and inside `two_pass_layout`.
- `input.sizing_mode` — only via `resolve_container_box`.
- `input.parent_size`, `input.available_space` — via `resolve_container_box`.
- `input.definite_dimensions` — **consumed** at relative.rs:1296-1299:
  ```rust
  let outer_definite = Size::new(
      input.definite_dimensions.width || style_definite.width,
      input.definite_dimensions.height || style_definite.height,
  );
  ```
  which gates `initial_parent_size` (1326-1335): only definite axes become the children's `parent_size`/percent basis. Also in `measure_item`: relative.rs:831 `if !input.definite_dimensions.height { refined.known_dimensions.height = None; ... }` (the width-clamp remeasure frees a non-definite height); 810-813 and 851-854 `item.size_is_definite = Size::new(item.preferred_definite.w || input.definite_dimensions.w, ...)`; 794-804 the `reuse_fixed_measurement` memo compares `definite_dimensions` field-by-field (deliberately ignoring `parent_size`).
  **Produced** at: `measurement_input` relative.rs:777 `input.definite_dimensions = constraint_definite;` (an axis is definite iff both-sides-constrained (`constrained_border_size` Some) or the style size is definite); `commit_in_flow` relative.rs:1187 `input.definite_dimensions = item.size_is_definite;` (with `IgnoreSizeStyles`).

#### What breaks under taffy's LayoutInput (no `definite_dimensions`)

Taffy conflates "a number is in `known_dimensions`" with "that dimension is definite". Hughie needs the split in both directions:

1. **Commit of a content-sized container.** The parent commits a child linear/relative container with `known_dimensions = measured size`. Starlight semantics (docs/starlight-relative-layout.md §2: parent height not definite "unless already definite before relative placement") require grandchildren's percentage sizes and percent margins/paddings to *not* resolve against that content-derived size. With taffy's input, `known.is_some()` ⇒ definite, so a `height: 50%` grandchild inside a wrap-content relative container would circularly resolve against the container's own content height.
2. **The inverse case — stretch/constraint-derived sizes ARE definite.** Linear flags stretch-imposed cross sizes definite (linear.rs:673) and relative flags both-sides-aligned sizes definite (relative.rs:704-705) even though no style size exists; that definiteness must propagate to the child's own children at commit. taffy has no channel to assert it, and the fields must survive into the **cache key** (dropping them aliases cache entries with different percent-resolution behavior).
3. Linear's two-phase edge refresh (`!outer_definite.width && has_box_basis_dependency`) and relative's remeasure-freeing (`!input.definite_dimensions.height`) are unrepresentable.

`RunMode`/`SizingMode`/`RequestedAxis` themselves map cleanly (taffy carries axis as a separate `LayoutInput.axis` field rather than inside the run mode — a lossless re-encoding).

---

### 3. LayoutTree methods and child-probe patterns

Both algorithms use exactly: `style(node)`, `flattened_children(node)` (+ `capacity_hint()`), `compute_layout(state, node, input)`, `set_unrounded_layout(state, node, layout)`, `set_static_position(state, node, point)`, and — via `hide_subtree`/`hide_child_at_order` in `compute/mod.rs:146-164` — `children(node)` and `clear_layout_cache(state, node)`. Neither touches `layout()`/`layout_mut()` directly. Note the **split-borrow signature** `fn compute_layout(&self, state: &mut Self::State, ...)`: the tree is `&self` throughout while durable state is a separate `&mut State`; taffy's `&mut self` `compute_child_layout` merges them (styles must be re-fetched/copied around child calls there — a structural port consideration).

#### Linear probe patterns

All measures route through `measure_child` (`compute/single_axis.rs:68`): `LayoutInput::measure(known, parent, available, requested_axis)` with `definite_dimensions` and `sizing_mode` overridden.

1. **Intrinsic keyword probes** (`resolve_intrinsic_sizes` → `intrinsic_measurement`, linear.rs:347-506): only if `item.intrinsic.has_intrinsic()`. Up to two probes (`MinContent`, `MaxContent` availability on requested axes), `SizingMode::IgnoreSizeStyles`, `RequestedAxis` = the requested-axis mask (`Horizontal|Vertical|Both`); non-requested axes carry clamped known sizes as `Definite` available.
2. **Main measure** (`measure_item`, linear.rs:593-732): fast path — if `!needs_probe_baseline && known.width.is_some() && known.height.is_some()` the child is **not called at all** (linear.rs:676-684). Otherwise one `measure_child(known, known_definite, percentage_basis, child_available, SizingMode::ApplySizeStyles, RequestedAxis::Both)`. Cross-stretch pre-forces the cross known (gravity `STRETCH`, or `NORMAL` + auto non-intrinsic cross size without ratio-fixed cross); main `AvailableSpace::MinContent` is degraded to `MaxContent` for the item probe (linear.rs:686-689).
3. **Weighted remeasure loop** (`size_items`, linear.rs:846-894): items with `weight > 0` skip the first measure when the container main size is definite; after `distribute_weighted_items` each weighted item is remeasured with `forced_main = Some(resolved_main)` (main known + definite, ratio-projected cross).
4. **Commit** (`commit_in_flow`, linear.rs:1117-1168): per in-flow item one `compute_layout` with `LayoutInput::commit(target_size, parent = final inner size, available = Definite(target))`, `sizing_mode = IgnoreSizeStyles`, `definite_dimensions` per-item packed; baseline re-read from the commit output; then `set_unrounded_layout` with the item's retained border/padding/margin.
5. **Absolute/fixed** — see §4.

Sequencing: resolve items → measure all → weight distribute (pure) → forced remeasures → `position_items` (pure) → [measure goal: return size+baseline] / [commit goal: relative-offset refresh → commit each in-flow → hidden → absolute/fixed].

#### Relative probe patterns

1. **Intrinsic probes** (`intrinsic_probe`, relative.rs:543-560): `LayoutInput::measure(Size::NONE, parent_size, available-with-Min/MaxContent-on-axis, axis.requested())`, `IgnoreSizeStyles` — **single-axis requests** (`RequestedAxis::Horizontal`/`Vertical`), unlike linear's masked Both.
2. **Item measure** (`measurement_input` + `measure_item`, relative.rs:677-855): known per axis = both-sides-constrained distance (min/max-clamped) → clamped preferred → intrinsic-resolved preferred; one-sided constraints shrink `available_space` (start subtracts, end caps — `RelativeLayoutAlgorithm::ComputeConstraints` semantics); `fit-content()` gets special owner-constraint handling. `SizingMode::ApplySizeStyles`, `RequestedAxis::Both`. **Memoization**: `if previous == input { return; }` (whole-`LayoutInput` `PartialEq`), plus the `reuse_fixed_measurement` partial match. Both-axes-known fast path skips the child. **Clamp-remeasure**: if the free-width result violates min/max, one refined `compute_layout` with the clamped width known (height re-freed unless definite), then height clamped without remeasure.
3. **Remeasure loops**: `one_pass_layout` — one measure+position per item in combined dependency order. `two_pass_layout` (relative.rs:1048-1168) — `measure_all(allow_item_references=false)` → position H, V → `measure_all(true)` → position H, V → if width was indefinite: `refresh_item_bases` (re-resolve geometry against resolved width, `fixed_measurement_matches` carries over memo state) → position H → `measure_all(true)` → position H, V → final position H, V against resolved height. Up to 3 `measure_all` sweeps; the memo suppresses child calls whose inputs did not change.
4. **Commit** (`commit_in_flow`, relative.rs:1170-1222): per item `LayoutInput::commit(item.output.size, parent = content box, available = Definite(content))`, `IgnoreSizeStyles`, `definite_dimensions = item.size_is_definite`; `set_unrounded_layout` at `content_origin + positions.start + margin + relative nudge`.

---

### 4. Out-of-flow handling

#### Linear (`commit_non_in_flow_children`, linear.rs:1171-1270; commit goal only)

- **Hidden** (`display: none` children): collected with document index; `hide_child_at_order` — recursively `clear_layout_cache` + zero `Layout` for the whole subtree, then `Layout::with_order(order)` on the root to keep the paint slot.
- **Absolute**: containing block = container **padding box**; `compute_absolute_layout_with_static_position` (compute/mod.rs:223) sizes the child (inset-modified size, auto-margins, `direction`-aware axis equation) with a callback computing the static position from the container's **main gravity (justify-content) and the child's own cross gravity (align_self/align_items)** via `absolute_static_position` (linear.rs:1034-1069, flow→physical conversion included); result offset by border, overflow accumulated with the child's `overflow()`, then `set_unrounded_layout`.
- **Fixed (hoisted)**: `static_axes = (left&&right auto, top&&bottom auto)` from `inset()`. If any axis needs it, `measure_absolute_layout(tree, state, child, padding_box, RequestedAxis::…)` (a *measure-goal* absolute sizing) to get size+margins for the gravity equation; then only `tree.set_static_position(state, child, static_position)` — **no layout is written**; the host's root fixed pass lays it out later.

#### Relative (`commit_out_of_flow`, relative.rs:1224-1269; commit goal only)

- **Hidden**: same `hide_child_at_order`, order = document index.
- **Absolute**: `compute_absolute_layout(..., static_position = Point::ZERO)` in padding-box space + border offset; overflow accumulated; `set_unrounded_layout`. No gravity — static fallback is the padding-box origin.
- **Fixed**: `tree.set_static_position(state, node, Point::new(border.left, border.top))` — no measurement at all.

Both interleave absolute/hidden paint order with in-flow via `sort_and_assign_layout_order`.

---

### 5. util.rs helpers used (and taffy-equivalence)

#### Linear imports (linear.rs:16-23)
`Axis`, `EdgeMask`, `ItemGeometry`, `ItemKey`, `OrderedItem`, `PendingLayoutItem`, `ResolvedContainerBox`, `accumulate_scrollable_overflow`, `apply_aspect_ratio`, `auto_edges_to_zero`, `clamp_axis`, `mirror_ratio_definiteness`, `own_scrollable_overflow`, `relative_offset`, `resolve_container_box`, `resolve_insets`, `resolve_intrinsic`, `resolve_item_geometry`, `resolve_margins`, `resolve_padding`, `sort_and_assign_layout_order`, `impl_item_geometry!`; from `single_axis`: `FlowAxes`, `flow_start`, `flow_end`, `flow_to_physical`, `set_flow_start`, `set_flow_end`, `measure_child`; from `compute/mod.rs`: `compute_absolute_layout_with_static_position`, `measure_absolute_layout`, `hide_child_at_order`.

#### Relative imports (relative.rs:7-13, plus qualified)
`Axis`, `ItemGeometry`, `ItemKey`, `OrderedItem`, `ResolvedContainerBox`, `accumulate_scrollable_overflow`, `clamp_axis`, `own_scrollable_overflow`, `relative_offset`, `resolve_container_box`, `resolve_intrinsic`, `resolve_item_geometry_with_bases`, `resolve_length_percentage`, `sort_and_assign_layout_order`, `subtract_available_space`; qualified: `util::resolve_insets`, `util::IntrinsicTag`, `impl_item_geometry!`; from mod.rs: `compute_absolute_layout`, `hide_child_at_order`.

#### taffy-equivalence assessment

| Helper | Taffy equivalent? |
|---|---|
| `clamp`/`clamp_axis` (min/max + padding-border floor) | plausible — `MaybeMath::maybe_clamp` + `.max(floor)` |
| `apply_aspect_ratio` | exists — `Size::apply_aspect_ratio` |
| `subtract_available_space` | plausible — `AvailableSpace::maybe_sub` pattern |
| `Axis` + `pack`/projections | rough — `AbstractAxis`/`Size::get_abs`; hughie's is physical-axis, keep |
| `accumulate_scrollable_overflow` | plausible — taffy `compute_content_size_contribution` (same overflow-visible rule) |
| `own_scrollable_overflow` (contain:layout suppression) | lynx-specific — keep |
| `resolve_length_percentage`/`resolve_style_size`/`resolve_max_size`/`resolve_margin`/`resolve_inset`/`resolve_padding`/`resolve_border` | taffy `MaybeResolve`/`ResolveOrZero` are equivalent **only over taffy value types**; these operate on stylo computed values (calc unpack fast path, anchor-fn unreachable arms) — keep, they *are* the stylo bridge |
| `resolve_intrinsic`, `IntrinsicTag(s)`, `fit_content_size`, `fit_content_available` | **no taffy equivalent** — taffy styles carry no min-content/max-content/fit-content() keywords; keep |
| `resolve_container_box` (`ResolvedContainerBox`: definiteness + box-sizing + ratio + available_inner) | no single equivalent (taffy flexbox inlines a weaker version without definiteness); keep |
| `resolve_item_geometry(_with_bases)` (`ItemGeometry`: preferred/min/max + `preferred_definite` + `IntrinsicTags` + `EdgeMask` + overflow + box_sizing) | lynx-specific composite; keep |
| `preferred_size_definiteness`, `mirror_ratio_definiteness`, `style_size_behaves_auto` | lynx definiteness model — no taffy analogue; keep |
| `relative_offset` (position:relative nudge, RTL left/right precedence) | taffy inlines relative insets differently; hughie's RTL rule is specific — keep |
| `sort_and_assign_layout_order`, `OrderedItem`/`ItemKey`/`PendingLayoutItem` | **no taffy equivalent** — taffy has no CSS `order` and assigns `Layout::order` = child index; keep |
| `EdgeMask` (auto-margin bitmask) | trivial, keep |
| `auto_edges_to_zero` | ≈ `ResolveOrZero`; trivial |
| `measure_child` (single_axis.rs) | thin `LayoutInput` builder preserving `definite_dimensions`/`sizing_mode` — depends on the extended input, keep |
| `flow_start/flow_end/set_*/flow_to_physical` (single_axis.rs) | logical↔physical edge mapping under reversal; taffy flexbox has its own main/cross machinery but tied to its constants — keep |
| absolute helpers (`compute_absolute_layout*`, `measure_absolute_layout`, `absolute_layout`, mod.rs:207-484) | taffy's absolute handling is embedded per-algorithm; hughie's shared W3C-ish resolver incl. RTL `prefer_end` and measure-goal variant — keep |
| `hide_subtree`/`hide_child_at_order` | taffy: `RunMode::PerformHiddenLayout` + `compute_hidden_layout` — equivalent mechanism exists, but hughie's is caller-driven and needs `clear_layout_cache` on the tree |

---

### 6. RTL / direction handling

**Linear** — fully logical-flow. `linear_axes()` (linear.rs:32-59): main horizontal for `row*`; `main_reverse = (*-reverse) XOR (horizontal && rtl)`; `cross_reverse = !horizontal && rtl` (RTL reverses the horizontal cross axis of a column container). All placement runs in flow coordinates and converts at export via `flow_to_physical(flow, box, container, reverse)`. Physical `AlignFlags::LEFT/RIGHT` in justify-content/align-self are re-keyed through the reversal state (`computed_main_gravity`, `map_cross_flags`); on a vertical axis they fall back to `START`. Item `direction()` affects only the `position: relative` nudge; absolute children use `direction` for the both-insets `prefer_end` rule and negative-free-space auto-margin side.

**Relative** — coordinate system is **physical** (start = left/top always; no reversal anywhere in relative.rs). Direction enters only through (a) the `CoreStyle::relative_align`/`relative_adjacent` default impls lowering logical `*-inline-start/end` longhands onto left/right per the item's `direction()` (physical-wins tie-break, documented as a deliberate deviation in docs/starlight-relative-layout.md §Value Definitions), (b) the per-item stored `direction` for the `position: relative` commit nudge, (c) the shared absolute path.

---

### 7. Baseline export

**Linear** (`container_baseline`, linear.rs:1095-1114): exports `first_baselines = Point::new(None, y)` on both measure and commit outputs, suppressed to `None` when `contain: layout`. Horizontal main axis: `max` over items of `location.y + item.baseline.unwrap_or(item.cross_size)` (synthesize at bottom border edge). Vertical main axis: first item only, `location.y + baseline.unwrap_or(main_size)`; no items ⇒ `None`. To have baselines during measure, `size_items` is called with `needs_probe_baseline = !commits_layout && !layout_contained`, which **defeats the both-axes-known fast path** and forces child measure calls purely to collect `output.first_baselines.y`. At commit, baselines are re-captured from commit outputs.

**Relative**: exports none — every return is `LayoutOutput::new(size, …)` with `first_baselines = Point::NONE` (spec: "Relative layout does not export a container baseline", docs/starlight-relative-layout.md §7).

Taffy note: taffy's `ComputeSize` outputs conventionally omit baselines (`LayoutOutput::from_outer_size`); linear's measure-time baseline collection requires child measure calls that *do* report baselines — a contract the trait must state explicitly.

---

### 8. Scratch/allocation structure (performance-preservation checklist)

**Linear**
- One `Vec<LinearItem<N>>` sized by `flattened_children().capacity_hint()`; `Vec<AbsoluteItem>`/`Vec<LayoutItemKey>` grow lazily and only under commit. `LinearItem` is guarded ≤ 200 bytes (test linear.rs:1637-1641); `LinearItemFlags` packs 4 booleans into one `u8`.
- Weighted freeze loop (`distribute_weighted_items`, linear.rs:744-843): allocation-free, bounded by `0..=weighted_count`; per-item `violation` stored inline; freeze via flag bit; tolerance `1.0e-5.max(|inner_main| * ε * 8)`; `weight_sum_override` scales distributable space `initial_free * total_weight / weight_sum_override`.
- Two deferred refresh passes gated by pre-computed booleans (`has_box_basis_dependency`, `has_relative_basis_dependency`) so style is re-read only for flagged items, only when the axis was not definite.
- Sorting: `sort_unstable_by_key` only when any `css_order != 0`.

**Relative**
- `IdLookup`: sorted `Vec<(i32, usize)>`, `reserve(items.len())` lazily on the first real id, in-place last-wins dedup, `binary_search_by_key` lookups.
- `Dependencies`: fixed `[u32; 8]` + `u8` len per item (≤ 8 edges: 4 align + 4 adjacent sides), in-place dedup — no hash sets.
- `dependency_order` (relative.rs:295-399): early-return **empty Vec** when no item references (then `order.get(ordinal).unwrap_or(ordinal)` makes the identity order free); otherwise a **CSR adjacency build** — `outgoing_counts`, prefix-sum `offsets` (count+1), flat `dependents` (edge_count) — followed by Kahn's algorithm; the `outgoing_counts` buffer is **reused as the ready queue** (`outgoing_counts.clear(); let mut ready = outgoing_counts;`); `indegree: Vec<u8>` with `u8::MAX` as the done sentinel; cycle fallback = lowest-index remaining item (deterministic, no rejection).
- Measurement dedup: `RelativeItem.last_measure: Option<LayoutInput>` whole-struct `==` memo; `reuse_fixed_measurement` + `fixed_measurement_matches` preserves memo/intrinsic state across `refresh_item_bases` after width resolution; both-known fast path synthesizes `LayoutOutput` without touching the child. `two_pass_layout` runs up to 3 `measure_all` sweeps and ~6 `position_axis` sweeps (positioning is pure arithmetic); the memo is what keeps child `compute_layout` call counts flat — any port must keep an input-equality memo with the same field sensitivity (ignoring `parent_size` for fixed-size items).
- `RelativeItem` embeds `ItemGeometry` + full `LayoutOutput` + `positions: Size<Line<f32>>` — no per-pass reallocation.

---

### 9. Assessment: `LayoutLinearContainer` / `LayoutRelativeContainer` trait pair

#### Style trait surface (mirroring taffy's `LayoutFlexboxContainer { type FlexboxContainerStyle<'a>; type FlexboxItemStyle<'a>; get_..._container_style; get_..._child_style }`)

`LinearContainerStyle` (: CoreStyle-equivalent): `linear_direction() -> linear_direction::T`, `direction() -> direction::T`, `justify_content() -> ContentDistribution`, `align_items() -> ItemPlacement`, `linear_weight_sum() -> NonNegativeNumber`, plus the core box set (`size/min_size/max_size/aspect_ratio/box_sizing/padding/border/margin/overflow/containment/contain_intrinsic_{width,height}`).
`LinearItemStyle`: `position()`, `order() -> i32`, `align_self() -> SelfAlignment`, `linear_weight() -> NonNegativeNumber`, `direction()`, `inset() -> Edges<&Inset>`, core box set — **with raw stylo value types**, because the algorithm needs `has_percentage()`, `is_auto()`, intrinsic-keyword variants, and `fit-content()` payloads (alternatively: explicit predicates `margin_depends_on_percentage()` etc., but raw access is what the code consumes).

`RelativeContainerStyle`: `relative_layout_once() -> relative_layout_once::T`, core box set.
`RelativeItemStyle`: `relative_id() -> i32`, `relative_align() -> Edges<i32>`, `relative_adjacent() -> Edges<i32>` (logical→physical lowering must stay in the accessor, as today), `relative_center() -> relative_center::T`, `position()`, `order()`, `direction()`, `inset()`, core box set incl. raw `Size<&StyleSize>` (for `fit_content_available` variant matching).

Because both algorithms also lay out **absolute/fixed/hidden children of any display type**, they additionally need the generic child style (position/inset/size/margins/…): the shared absolute resolver must remain reachable from both traits — i.e. the trait pair extends a common "core partial tree" whose child-style access is the full `CoreStyle`, not a per-algorithm subset. Taffy's `LayoutPartialTree::get_core_container_style` fills this role.

#### Tree methods beyond taffy's `LayoutPartialTree`

Taffy provides: `child_ids/child_count/get_child_id` (TraversePartialTree), `get_core_container_style`, `set_unrounded_layout`, `compute_child_layout`, `resolve_calc_value`. Required additions:

1. **`set_static_position(node, Point<f32>)`** — no taffy equivalent; the hoisted fixed-position pass depends on it (linear records gravity-aligned static positions, relative records the padding-box origin). Needs a durable slot per node (hughie: `LayoutSlot.static_position`).
2. **Flattened child iteration yielding `(NodeId, Style, Display)`** — `display: contents` flattening with per-child style/display already read (`FlattenedChildren`, tree/mod.rs:117-165) + `capacity_hint()`. Under taffy this is usually pushed into the tree impl's `child_ids`; keeping the hughie form avoids a second style fetch per child.
3. **Cache clearing for hide** — `clear_layout_cache(node)` used by `hide_subtree`. Taffy's alternative is dispatching `RunMode::PerformHiddenLayout` through `compute_child_layout`; either works but the choice changes the recursion ownership (hughie hides caller-side and preserves the paint-order slot via `Layout::with_order(order)`).
4. **Split state**: hughie's `(&self, &mut State)` vs taffy's `&mut self` — porting to `&mut self` means style borrows (`Self::Style<'tree>`) cannot be held across `compute_child_layout`; the item-resolution phases that read style while probing children must be restructured or styles copied into scratch (they mostly already are, via `ItemGeometry`, but `resolve_intrinsic_sizes`/`measurement_input`/`prepare_intrinsic_sizes` re-read style mid-probe).

#### Semantic mismatches with taffy's LayoutInput/LayoutOutput

1. **`definite_dimensions: Size<bool>`** — the hard one (§2). Both algorithms consume it *and* produce it on every child call, and it participates in cache keys. A taffy-shaped port must extend `LayoutInput` (fork) or thread an equivalent side-channel through the trait; it cannot be reconstructed from `known_dimensions`/`parent_size`/`available_space`.
2. **`SizingMode::IgnoreSizeStyles` at commit** — hughie commits children with `IgnoreSizeStyles` + forced target size; taffy's `SizingMode::ContentSize` on `PerformLayout` exists but taffy algorithms rarely exercise that pairing — behavior must be verified per leaf algorithm.
3. **Baselines from measure calls** — linear needs `first_baselines` on `ComputeSize`-style outputs (§7); taffy conventionally returns them only for `PerformLayout`.
4. **CSS `order` + interleaved paint order** — no taffy support; `order() -> i32` on the style trait and `sort_and_assign_layout_order` writing `Layout.order` must be kept.
5. **Input equality memoization** — relative requires `LayoutInput: PartialEq` with exactly the field set above (holds for taffy's derive, but only if the definiteness extension is inside the compared/hashed struct).
6. **Style value types** — taffy's `CoreStyle`/`Dimension` cannot express `min-content|max-content|fit-content()|stretch` sizes, calc-with-percent detection, or the lynx `relative-*`/`linear-*` properties; the trait pair must stay on stylo computed types, which means the algorithms can adopt taffy's *shape* (trait-generic over a partial tree) but not taffy's style vocabulary.

---

# Test/bench/docs migration impact inventory

> Provenance: Read directly from the hughie and dom test/bench trees and docs/ at this branch.

## Migration blast radius: hughie flex/grid → taffy `compute_flexbox_layout`/`compute_grid_layout`

Scope assessed: hughie's hand-written flex+grid replaced by taffy's low-level custom-tree API; Linear+Relative kept, re-expressed in taffy trait vocabulary; `display:contents` flattening moved from `hughie::tree::LayoutTree::flattened_children` into `crates/dom`.

Key structural facts grounding everything below:

- hughie's protocol is already taffy-shaped but not taffy-identical: `crates/hughie/src/tree/io.rs` defines `LayoutInput { goal: LayoutGoal, sizing_mode, known_dimensions, definite_dimensions: Size<bool>, parent_size, available_space }`. The `definite_dimensions: Size<bool>` field (known-but-indefinite sizes) and `LayoutGoal::Measure(RequestedAxis)`/`Commit` have no one-for-one taffy equivalent (taffy: `RunMode` + `axis`, no definiteness bit separate from `known_dimensions`).
- `display:contents` flattening lives in `crates/hughie/src/tree/mod.rs:70-160` (`flattened_children`/`FlattenedChildren`) and is consumed at `compute/flexbox.rs:1523`, `compute/grid/mod.rs:1166`, `compute/linear.rs:1381`, `compute/relative.rs:1347`.
- Size/layout containment is implemented **inside** the flex and grid algorithms (`compute/flexbox.rs:1581-1653`, `compute/grid/mod.rs:1071-1268` via `style::containment::size_containment`), not in a dispatch wrapper — taffy has no containment, so this logic must move to the kept shell around taffy calls.
- The cache key (`crates/hughie/src/cache.rs`, `PackedLayoutInput`) losslessly packs `definite_dimensions` and the measure axis; taffy's `Cache` keys differently.
- All box-layout benches live in `crates/hughie/benches/` but drive **dom's production host** (`dom::Document::layout`) through a dev-dependency cycle (`crates/hughie/Cargo.toml` dev-deps `dom = { features = ["layout-test-utils"] }`).

---

### 1. Test inventory

`crates/hughie/tests/` — 308 `#[test]` fns total, one shared host in `tests/support/mod.rs` (1509 lines: `TestTree: LayoutTree` mock with `TestStyle: CoreStyle` built from raw stylo computed values, measure-call tracing (`measure_inputs`, `measure_calls`, `leaf_measure_calls`, `layout_writes`, `static_position_writes` counters), `committed_input()` cache introspection, `push_contents` for display:contents nodes).

| File | #[test] | Surface pinned |
|---|---|---|
| `tests/protocol.rs` | 28 | The `LayoutTree` protocol itself over a second, minimal `MockStyle` host: GAT style borrows live across recursive layout, stylo grid-track lending, leaf dispatch IO, static-position write channel, `LayoutSlot` embeddable cache lifecycle + lossless key round-trip, `compute_root_layout`/hidden cleanup/`hide_subtree`, `compute_absolute_layout`, `round_layout` device-pixel snapping + CSS +∞ tie-breaking, `flattened_children` splicing/`size_hint`, `is_relayout_boundary`/`invalidate_for_relayout`/`compute_boundary_relayout`, skipped-contents dispatch |
| `tests/flexbox.rs` | 52 | Flex geometry oracles: grow/shrink/freezing, line collection, gaps/wrap, row/column-reverse+RTL, order, justify/align/auto margins, baselines, intrinsic keyword sizing, indefinite percentage basis, automatic minimum size (scroll-container variant), aspect-ratio transfer, absolute/hoisted static positions, content_size trapping, display:contents items, measure-goal write discipline |
| `tests/grid.rs` | 85 | Grid geometry oracles: explicit/auto/dense/negative-line/span placement, fr/minmax/fit-content/intrinsic tracks, auto-fill/auto-fit collapse, spanning-item distribution, content/self alignment, baseline groups + shims + container-baseline synthesis, RTL (10 refs), absolute grid areas + padding-edge fallbacks + static fallbacks, hoisted static positions, probe-count linearity, `definite_dimensions` auto-repeat seeding, track-limit and hostile-repeat bounding |
| `tests/linear.rs` | 63 | Starlight Linear: orientation/gravity/weights/freezing, cross-axis auto margins, baseline synthesis, single-axis measure probes (24 `measure_inputs` refs, 15 `LayoutGoal` refs), nested cross-algorithm dispatch with flex/grid (6 tests), absolute/hoisted static alignment, calc/percentage refresh, `definite_dimensions` percentage-basis rule |
| `tests/relative.rs` | 50 | Starlight Relative: id alignment/adjacency, dependency ordering, cycle fallback, one-pass/two-pass, wrap feedback, measurement constraints, absolute/hoisted, paint order |
| `tests/containment.rs` | 21 | css-contain-2 over all four algorithms: size-containment substitution (per-algorithm ×4 + probes + auto-minimum), layout-containment baseline suppression (×3), scroll-container content_size contribution (8 `content_size` refs), skipped-contents (`content-visibility`) transitions |
| `tests/text.rs` | 9 | Parley text core: exact Ahem geometry, rebreak, word-break, baselines; 1 test (`flex_baseline_integration_reuses_artifacts_and_jointly_invalidates_caches`) crosses into flex |

`crates/dom/tests/layout.rs` — 74 tests, production host (`Document::layout`, `rounded_layout`), asserts full `LayoutSnapshot` (order + 18 geometry components incl. `content_size`). Covers: flex/grid/linear/relative smoke geometry (~13), display:contents (13 tests incl. hoisted-rank-through-contents), fixed/absolute containing-block rules, hoist + effective-order-0, text via Parley, rounding, viewport %, incremental relayout/boundary equivalence, content-visibility.

Other dom tests touching layout: `replaced_content.rs` (6 tests, 7 `rounded_layout` refs — natural-size leaves), `shadow.rs` (33 tests, 3 refs — flat-tree drives layout), `custom_elements.rs` (2 refs), `media_queries.rs` (1 ref), `primitives.rs` (3 refs), `grammar_layout.rs` (8 tests — CSS parsing only, no geometry), `crates/dom/src/visual/tests.rs` (69 unit tests; `order_is_inert_on_absolutely_positioned_children` pins effective-order-0), `crates/dom/src/scroll/` (9 + behavior tests, consume `hughie::tree::Layout::content_size`).

`layout-test-utils` feature: defined in `crates/hughie/Cargo.toml:13`, forwarded by `crates/dom/Cargo.toml:11`; gates `compute_leaf_layout_with_measurement_for_testing` (`crates/hughie/src/compute/leaf.rs:168`), dom's synthetic-leaf host branch (`crates/dom/src/layout/host.rs:126-137`), `Node::set_natural_size` seam (`crates/dom/src/tree/node.rs`), and the explicit bench invalidation hook (AGENTS.md:585). The feature name and seams survive; the leaf entry point must be re-expressed if leaf layout moves to taffy's `compute_leaf_layout`.

### 2. Classification

**(a) Die with hand-written flex/grid (algorithm-internal, ~10):**
- `grid.rs`: `intrinsic_probe_count_stays_linear_in_item_count` (pins hughie's probe complexity), `measure_goal_probes_intrinsics_without_durable_writes` (probe/write discipline), `template_components_after_the_track_limit_are_dropped`, `min_content_maximums_and_hostile_repeat_counts_stay_bounded` (hughie bounding policies), `flex_known_but_indefinite_grid_size_does_not_seed_initial_auto_repeat` (asserts `committed_input(inner).definite_dimensions.width == false` — the field itself disappears).
- `flexbox.rs`: `measure_goal_does_not_write_durable_layouts`, `leaf_measure_goal_preserves_the_single_axis_fast_path` (hughie's single-axis measure fast path).
- Any assertion on `TestTree` counters (`layout_writes`, `leaf_measure_calls`) inside flex/grid tests — taffy's call pattern differs even where geometry matches.

**(b) Retargetable as conformance oracles against taffy (~120 of flexbox+grid):** the large majority of `flexbox.rs` (≈45) and `grid.rs` (≈70) assert exact geometry through `compute_layout` + `tree.layout(id)` and remain valid oracles for a taffy-backed dispatch. Flagged subsets that encode hughie-specific behavior and need decision-per-test:
- RTL/`direction` (flexbox ×2-3, grid ×~6 incl. `rtl_flips_the_inline_track_axis_and_auto_placement_start`, `physical_alignment_keywords_stay_physical_across_directions`) — taffy has no `direction` property; re-verify or shim.
- `definite_dimensions` semantics (see (a)); also `indefinite_percentage_flex_basis_falls_back_to_content_not_width`.
- Hoisted static positions (`hoisted_static_position_is_the_aligned_margin_box_origin`, grid `hoisted_absolute_records_grid_aware_static_position_for_positioned_pass`, `hoisted_static_position_ignores_placement_and_measures_auto_content`) — depend on hughie's `set_static_position` host channel; taffy lays out absolute children in-algorithm and has no hoist channel.
- Effective-order-0 for absolute children (`absolute_children_use_order_zero_for_paint_order` in both files) — taffy records source order in `Layout.order`.
- `content_size` trapping (flexbox 3 refs, grid 6 refs, e.g. `leaf_max_width_constrains_measurement_and_preserves_overflow_extent`, `overflowing_positional_content_alignment_preserves_negative_free_space`).
- Grid baseline corpus (~6 tests: `baseline_group_aligns_items_and_sets_the_container_first_baseline`, `baseline_shims_expand_an_intrinsic_row_before_following_rows_are_positioned`, `container_baseline_*` ×3, `block_axis_auto_margin_excludes_an_item_from_baseline_sharing`).
- `display_contents_children_flex_as_items_of_the_box_ancestor`, `display_contents_items_keep_their_own_order_and_hidden_handling` — move to `crates/dom` with the flattener.
- Containment-in-flex/grid tests from `containment.rs` (9: `flex_size_containment_*`, `flex_min_content_probe_*`, `flex_automatic_minimum_*`, `grid_size_containment_*`, `flex/grid_layout_containment_suppresses_the_exported_baseline`, `scroll_container_child_contributes_only_its_border_box`, `layout_contained_*` ×2) — become oracles for the containment wrapper that must be rebuilt around taffy calls.

**(c) Protocol/host tests needing rewrite in taffy vocabulary (protocol.rs, 28):** all of `tests/protocol.rs`. Sub-groups: style-view/GAT-borrow + stylo track lending (4) → taffy 0.9 trait-style equivalents; leaf IO + calc-through-stylo (3) → depends on the calc plumbing decision; cache lifecycle/key round-trip (3) → taffy `Cache` has no `definite_dimensions`/axis-goal key components; root/hidden (4); absolute/static-position channel (2); rounding (2 — hughie pins CSS +∞ tie-breaking; taffy's `round_layout` uses cumulative edge rounding); `flattened_children` (3 — move to `crates/dom/tests/` with the flattener); relayout-boundary/invalidation (6) and skipped-contents (1) — stay with the kept shell, retype only.

**(d) Linear/Relative tests surviving with retyping (linear.rs 63 + relative.rs 50 = 113):** mechanical retype `LayoutGoal::Measure(RequestedAxis)` → taffy `RunMode::ComputeSize` + `axis`, `LayoutInput/LayoutOutput/AvailableSpace/SizingMode` → taffy types. Exceptions needing design work: `flex_known_but_indefinite_linear_size_is_not_a_percentage_basis` (loses the `definite_dimensions` input channel — Linear's rule needs a new encoding); the 10 cross-algorithm interop tests (`a_linear_item_can_be_a_grid_container`, `a_grid_item_can_be_a_linear_container`, `a_linear_item_can_be_a_flex_container`, `a_flex_item_can_be_a_linear_container`, `flex_max_content_target_enables_linear_weight_distribution`, `flex_max_content_target_enables_linear_default_cross_stretch`, `auto_main_axis_preserves_grid_min_and_max_content_probes`, etc.) now cross into taffy-backed flex/grid and become conformance-sensitive on both sides; `real_cache_does_not_let_linear_measurement_satisfy_commit` re-pins against taffy's cache semantics; the 24 `measure_inputs`-trace assertions in linear.rs and 5 in relative.rs pin exact probe shapes that the re-expressed algorithms must reproduce. Containment-on-linear/relative tests (containment.rs ×4-5) retype with the kept algorithms. `tests/text.rs` (9) survives untouched except the flex-integration test.

### 3. Bench inventory

All in `crates/hughie/benches/` (divan; CodSpeed-tracked per README):

| Target | Registered benches | Host | Migration fate |
|---|---|---|---|
| `flexbox.rs` | 23 (`flex_grow_row`, `flex_wrap_gaps`, `flex_at_most_root`, `at_most_owner_matrix`(+`_with_text`), `owner_direction_inheritance`(+text), `flex_axis_alignment_matrix`(+text), `flex_distribution_matrix`(+text), `flex_wrap_alignment_matrix`(+text), `flex_baseline_measured`, `baseline_propagation_matrix`, `measured_callback_matrix`, `absolute_children`, `nested_column_flex`, `in_flow_order_matrix`, `full_value_spacing_matrix`, `box_sizing_matrix`, `fit_content_subtrees`, `mixed_display_none`) | `support/mod.rs` `LayoutFixture` → real `dom::Document`, CSS styles, timed path = `Document::layout` incl. host, caches, positioned pass, rounding | **Survive unchanged structurally** — enter via `Document::layout`; baselines shift, CodSpeed history breaks |
| `grid.rs` | 15 (in `scenarios/grid.rs`: `sparse_auto_placement_cold`, `dense_hole_backfill_cold`(+text), `fixed_and_fractional_tracks_cold`, `intrinsic_spanning_items_cold`(+text), `unique_intrinsic_span_buckets_cold`(+text), `flexible_track_freeze_thresholds_cold`(+text), `nested_grid_cold`(+text), `nested_grid_warm_descendants`, `nested_grid_warm_root_cache_hit`, `nested_grid_dirty_leaf_and_ancestors`) | same production host | Survive structurally; warm/cache-hit variants re-pin taffy+shell cache behavior |
| `linear.rs` | ~21 via `for_each_linear_scenario!` (weighted freeze, gravity matrices, intrinsic percentage/padding matrices, mixed hidden/absolute, `_with_text` clones) | same production host | Survive; retype only if fixture styles are unaffected (they are — CSS in, `Document::layout` out) |
| `relative.rs` | 14 (`independent_two_pass_cold`, `..._wrap_width_cold`(+text), `reverse_chain_two_pass_cold`(+text), `reverse_chain_one_pass_cold`, `disjoint_cycles_cold`(+text), `duplicate_ids_cold`(+text), `nested_relative_cold`(+text), `nested_relative_warm_descendants`, `nested_relative_root_cache_hit`) | same production host | Survive |
| `containment.rs` | 4 (`contained_boundary_stopped`, `contained_whole_path`, `contained_cold`, `uncontained_boundary_stopped_control`) | **the hughie-protocol `TestTree` host** (`#[path = "../tests/support/mod.rs"]`), calls `compute_boundary_relayout`, `compute_root_layout`, `invalidate_for_relayout` directly | **Needs retyping** with the protocol; flex interiors become taffy-backed |
| `text.rs` | 11 (5 cold/warm pairs: label/sentence/paragraph/cjk/multi_run + `committed_box_cache_hit`) | Parley core directly; `committed_box_cache_hit` via production host | Survive; `committed_box_cache_hit` re-pins the shell cache |

Note the dev-dependency cycle: if flex/grid code leaves hughie, these bench targets (which need `dom`) may be better relocated; today they only require that `Document::layout` keeps its signature.

### 4. docs/layout-conformance.md commitments vs taffy

Commits (exact): Flexbox = W3C CSS Flexbox Level 1, **CR Draft 2025-10-14**; Grid = CSS Grid Level 2, **CR Draft 2025-03-26, limited to the numeric track/placement surface** (named lines/areas and subgrid explicitly excluded, §"Deliberate scope"); Linear/Relative = Starlight algorithms at `lynx-family/lynx` commit `e286cd11dda7cc8111d64c2a58d8625bce2bed73` (audited 2026-07-14), with three recorded Relative repairs (parent id 0 reserved, contradictory anchors collapse at start, two-pass selective final-size feedback). Also commits: "The integration suites use the same `LayoutTree` protocol as a real host" and that every retained case asserts an exact observable (geometry, used margins, baseline, layout order, static position, **measurement input, cache traffic**) — the last two observables are hughie-internal and cannot survive verbatim.

Items to re-verify against taffy's coverage before rewriting this doc: (1) `direction: rtl` — no taffy support; (2) flex/grid baseline completeness (grid baseline groups/shims/container-baseline synthesis in particular); (3) indefinite-percentage flex-basis and cyclic-percentage resolution order; (4) automatic minimum size details (scroll-container overflow, aspect-ratio transfer, contained children); (5) absolute-positioned grid children (auto-line padding-edge fallbacks, content-box static fallback, static-position auto-margin distribution); (6) auto-repeat counting bases and minimum-precedence clamping; (7) `fit-content()`/intrinsic keyword surface parity; (8) rounding policy (CSS +∞ tie-breaking vs taffy cumulative rounding); (9) content_size accumulation semantics; (10) grid track-limit behavior. The Grid spec-version claim (L2 CRD 2025-03-26) must be re-dated to whatever taffy actually tracks.

Other docs touched: `docs/layout-architecture.md` (the dom::layout row at line 101 names display:contents handling, the LayoutTree protocol, `content_size` merging — substantial rewrite; its "excluded: a second layout algorithm" column inverts), `docs/tracking/css-layout.md` (display:contents row names `LayoutTree::flattened_children` in hughie → must point at dom; linear-* section names `compute_linear_layout`), `docs/starlight-linear-layout.md`/`docs/starlight-relative-layout.md` (contracts survive; protocol vocabulary references need sweep), `crates/hughie/README.md` (whole design doc), root `README.md:14,19`, `AGENTS.md:585` (layout-test-utils hook), `docs/tracking/deviations.md:81,134` (minor mentions), `docs/dom-public-api.md:44`.

### 5. hughie uses outside crates/hughie and crates/dom

Code dependencies: **none**. `hughie` is a workspace dep (`Cargo.toml:16`) consumed only by `crates/dom` (`crates/dom/Cargo.toml:21`, plus feature forward at line 11). Downstream crates reach hughie types only through dom re-exports: `dom::FontBlob` (= `hughie::text::FontBlob`, used by `crates/lynx-element/src/tree.rs:5,128` and `crates/bobcat-core/src/engine.rs:52,330,642` — text side, unaffected), `dom::layout::{Layout, Size, NaturalSize}` re-exports (`crates/dom/src/layout/mod.rs:10-19`), and `Document::rounded_layout` (`crates/lynx-element/src/tree.rs:282`). Non-code mentions: `crates/lynx-element/src/lib.rs:48` (comment), `crates/bobcat-cli/README.md:5`, root `README.md`. Within dom, hughie types thread through `visual/` (`build.rs:32-34`, `stacking.rs:13-14`, `geometry.rs:4`), `scroll/mod.rs:6,49,135` (`Layout::content_size`), `layout/{mod,host,style}.rs`, `tree/{document,node}.rs` — all in-scope for the migration; nothing outside dom breaks at the type level as long as `dom::layout::Layout`'s shape is preserved.

### 6. Risk list — behaviors the corpus pins that taffy may implement differently

1. **RTL / `direction`** (taffy has no direction property): `crates/hughie/tests/flexbox.rs` (`row_column_reverse_and_rtl_resolve_physical_main_axes`, `column_wrapping_uses_rtl_and_wrap_reverse_for_cross_start`), `tests/grid.rs` (`rtl_flips_the_inline_track_axis_and_auto_placement_start`, `rtl_container_uses_container_start_for_stretch_and_item_start_for_baseline`, `rtl_grid_areas_keep_absolute_left_insets_physical`, `physical_alignment_keywords_stay_physical_across_directions`), `crates/dom/tests/layout.rs` (`rtl_direction_flips_the_flex_row_axis`).
2. **Known-but-indefinite dimensions** (`definite_dimensions` absent from taffy `LayoutInput`; taffy treats known as definite for percentage bases and auto-repeat): `tests/grid.rs::flex_known_but_indefinite_grid_size_does_not_seed_initial_auto_repeat`, `tests/linear.rs::flex_known_but_indefinite_linear_size_is_not_a_percentage_basis`, `tests/flexbox.rs::indefinite_percentage_flex_basis_falls_back_to_content_not_width`.
3. **calc() plumbing** (hughie resolves calc natively via stylo `LengthPercentage`; taffy's style traits require taffy length types with a separate calc-resolution mechanism): `tests/protocol.rs::calc_padding_resolves_through_stylo_style_values`, `crates/dom/tests/layout.rs::calc_widths_resolve_during_layout`, calc cases in flexbox.rs (3) and linear.rs (3).
4. **Device-pixel rounding** (hughie: CSS round-half-toward-+∞ per component, fused positioned+rounding pass in dom; taffy: cumulative edge-based rounding): `tests/protocol.rs::round_layout_snaps_on_the_device_pixel_grid`, `::round_layout_uses_css_positive_infinity_tie_breaking`, `crates/dom/tests/layout.rs::rounding_snaps_to_the_device_pixel_grid`, `::incremental_relayout_matches_full_under_fractional_device_pixels`.
5. **Grid baselines** (groups, shims expanding intrinsic rows, container-baseline synthesis and ordering): 6 tests in `tests/grid.rs` (`baseline_group_aligns_items_and_sets_the_container_first_baseline`, `baseline_shims_expand_an_intrinsic_row_before_following_rows_are_positioned`, `container_baseline_comes_from_first_nonempty_row_with_synthesis`, `container_baseline_uses_grid_order_within_the_first_nonempty_row`, `container_baseline_prefers_first_row_synthesis_over_later_baseline_group`, `block_axis_auto_margin_excludes_an_item_from_baseline_sharing`).
6. **Absolute static positions and hoisting** (hughie exports static positions through `set_static_position` for dom's hoist pass; taffy lays absolute children in-algorithm and has no hoist channel; grid padding-edge/content-box fallbacks are detailed): `tests/grid.rs` absolute/hoisted corpus (~12 tests, e.g. `absolute_static_fallback_uses_content_box_not_selected_grid_area`, `absolute_defensive_placements_fall_back_to_padding_edges`, `hoisted_absolute_records_grid_aware_static_position_for_positioned_pass`), `tests/flexbox.rs::hoisted_static_position_is_the_aligned_margin_box_origin`, `crates/dom/tests/layout.rs::absolute_child_with_auto_insets_uses_its_static_position`, hoisted/fixed tests.
7. **Paint order of absolute children = effective order 0** (taffy `Layout.order` is the source child index): `tests/flexbox.rs::absolute_children_use_order_zero_for_paint_order`, `tests/grid.rs::absolute_grid_children_use_order_zero_for_paint_order`, `crates/dom/src/visual/tests.rs::order_is_inert_on_absolutely_positioned_children`, `crates/dom/tests/layout.rs::hoisted_children_paint_with_effective_order_zero`.
8. **content_size (scrollable overflow) accumulation** incl. layout-containment filtering and boundary merge-back: `tests/containment.rs` (`scroll_container_child_contributes_only_its_border_box`, `layout_contained_visible_box_excludes_descendant_overflow`, `layout_contained_scroll_container_keeps_its_interior_scroll_range`), grid content_size assertions (6 refs), `crates/dom/tests/layout.rs` (`contained_boundary_relayout_refreshes_scrollable_content_size`, `layout_contained_visible_boundary_excludes_descendant_scrollable_overflow`, `boundary_scrollable_overflow_is_consistent_across_incremental_and_cold_layout`), all `crates/dom/src/scroll` behavior tests downstream.
9. **Automatic minimum size details**: `tests/flexbox.rs::automatic_minimum_size_depends_on_scroll_container_overflow`, `::automatic_minimum_uses_aspect_ratio_transferred_size`, `tests/containment.rs::flex_automatic_minimum_uses_the_contained_min_content`, `tests/grid.rs::automatic_minimum_is_clamped_by_a_fixed_max_track`, `::multitrack_auto_minimum_contributes_to_intrinsic_track_sizes`.
10. **Containment (size/layout) has no taffy equivalent** — must be re-implemented as pre-substitution of known dimensions + post-suppression of baselines/content_size around taffy calls: the 9 flex/grid tests in `tests/containment.rs`; failure also cascades into `crates/dom/tests/layout.rs` boundary tests (`contain_strict_boundary_relayouts_interior_without_changing_outer_size` etc.).
11. **Cache semantics** (hughie key includes definiteness + measure axis + baseline presence; measurement never satisfies commit): `tests/protocol.rs::embeddable_cache_round_trips_a_complete_key`, `::embeddable_cache_lifecycle`, `tests/linear.rs::real_cache_does_not_let_linear_measurement_satisfy_commit`, `crates/dom/tests/layout.rs::fixed_stays_viewport_anchored_when_its_parent_answers_from_cache`; benches `nested_grid_warm_root_cache_hit`, `committed_box_cache_hit`.
12. **Grid bounding policies** (track-limit drop, hostile auto-repeat counts, minimum-precedence clamping of counting bases): `tests/grid.rs::template_components_after_the_track_limit_are_dropped`, `::min_content_maximums_and_hostile_repeat_counts_stay_bounded`, `::auto_repeat_clamps_its_counting_basis_with_minimum_precedence`, `::auto_repeat_resolves_percentage_gap_against_its_max_constraint`.
13. **display:contents order/hidden competition after the move to dom** (flattened items must still compete in the container's order sort, keep hidden handling, and resolve `relative-id` anchors among flattened siblings): `tests/flexbox.rs` display_contents ×2, `tests/protocol.rs` flattened_children ×3, and the 13 contents tests in `crates/dom/tests/layout.rs` (`contents_children_compete_in_the_container_order_sort`, `hoisted_rank_resolves_through_nested_contents_levels`, etc.) — the dom tests are the retained oracles; the hughie-side ones move or die.
14. **Measure-probe shape and count** (single-axis fast path, no duplicate min-content probes, probe-count linearity — affects both correctness-adjacent assertions and CodSpeed baselines): `tests/grid.rs::intrinsic_probe_count_stays_linear_in_item_count`, `tests/linear.rs::max_content_keyword_does_not_issue_a_min_content_probe`, `::intrinsic_percentage_box_refresh_does_not_remeasure_children`, and all 24 `measure_inputs`-trace assertions in linear.rs once flex/grid parents drive them.

---
