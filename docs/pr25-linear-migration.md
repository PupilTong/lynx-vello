# PupilTong/lynx#25 Linear test and benchmark migration

This document records the Rust-only migration of every layout test and
benchmark surface in
[`PupilTong/lynx#25`](https://github.com/PupilTong/lynx/pull/25) whose subject
is Starlight `display: linear`. The source revision audited by this migration
is `dfeedeabfefca7ec5d77ea511071745361c3d09d`.

## Execution boundary

- All layout execution uses neutron-star's statically dispatched Rust traits.
- No Lynx C++ source, C++ baseline engine, native bridge, C ABI, FFI symbol,
  generated C header, or comparison runner is copied or linked.
- Head-to-head tests retain the Rust tree builders, parameter spaces,
  deterministic generator stream, regression IDs, independent geometry
  assertions where present, and source cardinalities. Where the C++ result
  was the only oracle, the Rust half now checks exact repeatability and finite
  output; the C++ half of every comparison is removed.
- The PR fixture vocabulary lowers through `tests/pr25_support/mod.rs` into
  physically separate immutable source and mutable session storage.
- `display: linear` dispatches to `compute_linear_layout`; it is no longer
  represented by a Flex row/column adapter.
- Sticky nodes are lowered as normal-flow `Position::Relative` nodes with
  auto engine insets. The test-only host retains the authored four-edge
  values and exports their parent-content-box-resolved `sticky_pos` values
  (including Starlight's `-1e10` auto sentinel), so Sticky metadata cannot
  accidentally become a Relative visual offset. This does not add Sticky to
  neutron-star's production protocol or implement scroll-time clamping.
- To match PR #25's `effective_layout_style`, a child-containing
  `display: block` box is also host-dispatched to the unchanged Linear style.
  The same is true for an unmeasured empty Block box; only nodes with an
  actual measurement source take the measured-leaf path before display
  dispatch.

## Direct and cross-file tests

`tests/pr25_linear_layout.rs` is a name-preserving port of all 119 tests in
the PR's `linear_layout_tests.rs`. It covers sizing and measurement,
orientation and bidi, main/cross alignment, auto margins, weighted
distribution and freezing, ordering, baselines, hidden subtrees, and
positioned descendants.

Four Linear tests from `position_layout_tests.rs` live in
`tests/pr25_linear_additional.rs`:

1. `absolute_linear_child_without_insets_uses_linear_gravity`
2. `absolute_rtl_horizontal_linear_child_without_insets_uses_rtl_main_front`
3. `linear_sticky_child_percent_insets_resolve_against_container_constraints`
4. `linear_sticky_child_end_percent_insets_resolve_against_container_constraints`

The first two execute exact Linear static-position geometry. The Sticky tests
use the same PR #25 facade and assert both halves of the source contract: the
item remains at its unshifted normal-flow Linear position, while all four
authored insets are exported after percentage resolution against the Linear
container content box. Scroll-time sticky clamping remains outside the layout
engine. The cross-algorithm
`flex_row_baseline_uses_nested_linear_container_baseline` fixture remains in
`tests/pr25_flex_layout.rs` and now reaches the real Linear algorithm through
the shared dispatch facade.

`tests/pr25_block_as_linear.rs` ports the four
`box_edges_layout_tests.rs` and five `position_layout_tests.rs` cases whose
names say “Linear natural size” while their fixture boxes use
`display: block`. Those nine cases exercise root, nested, absolute, and fixed
Block subtrees through the same effective Block-as-Linear dispatch as the
source Rust engine.

`tests/pr25_linear_external_callback.rs` ports the five Linear-relevant Rust
geometry tests that the source placed behind its external callback table:

1. a `display: none` child is zeroed and omitted from the Linear stack;
2. Sticky percentage metadata is exported for Flex, Linear, Grid, and
   Relative container children;
3. Flex baseline alignment consumes a nested Linear container baseline; and
4. the two horizontal Linear baseline/auto-margin interactions retain their
   source tree shapes.

The callback table, C ABI values, and exported entry point are not migrated;
the same Rust fixtures execute through neutron-star's host traits and
`pr25_support`. The target contains one additional source-named cache
regression, for six tests total. That regression preserves the source's
ignored-child, explicit-Stretch, and implicit-auto-cross tree shapes while
testing neutron-star's actual cache architecture: the complete `LayoutInput`
key prevents a max-content measurement from answering a newly definite cross
constraint, so the source engine's private child-scanning reuse predicate is
neither copied nor required.

The source's non-Linear-named
`flex_cache_reuse_helpers_cover_mode_matrix_and_guard_paths` also contains two
Linear branches in its larger private-helper matrix. Its Flex branches remain
in `tests/pr25_flex_internal.rs`; the Linear definite-cross-axis behavior is
subsumed by the complete-`LayoutInput` cache regression above. This is an
architecture substitution, not a missing source row: neutron-star has no
equivalent partial-key reuse predicate to invoke directly.

## Head-to-head Rust migrations

### Native

`tests/pr25_native_linear.rs` inventories 138 source tests and two source
helpers. It includes the two tests whose names do not contain `linear` but
whose trees instantiate `Display::Linear`.

- `tests/pr25_native_linear_exact.rs` ports the actual Rust bodies and builders
  of the 105 tests whose names also occur in the direct suite. In particular,
  it preserves the native source helper's `Display::Flex` child default rather
  than substituting the direct suite's `Display::Block` default, and retains
  every loop and parameter row. Only the C++ comparison runner is replaced by
  repeated neutron-star layout with exact-result determinism and finite-value
  checks.
- Twenty-one native-only Linear tests (333 invocations), including the
  160-case main-axis and 136-case cross-axis matrices, retain source-shaped
  Rust builders and independent numeric oracles.
- Twelve name matches are intentional `display: block`-as-Linear sizing
  cases. Their Rust builders execute through the compatibility host's real
  Linear dispatch, with explicit natural-size and positioned-geometry
  oracles; the C++ comparison half is absent.
- The upstream-ignored Flex-column/Linear-child regression runs as an explicit
  Rust-only fallback case. Its source ignore guarded a C++ comparison for
  non-grid `fr`; no such comparison exists in this migration.

Together with 146 exact-overlap and 12 block-as-Linear invocations, the two
native Linear targets execute all 491 source fixture invocations. Their shared
partition guard prevents a new source case from silently falling through a
name heuristic. The overlap count includes four independent Rust fixtures in
each of
`head_to_head_linear_absolute_child_cross_axis_uses_cpp_computed_layout_gravity_order`
and
`head_to_head_linear_absolute_vertical_child_uses_cpp_main_axis_static_position`;
they are not collapsed into one execution per test function.

### Generated

`tests/pr25_generated_linear.rs` and
`tests/pr25_generated_linear_support/mod.rs` port all 24 generated source
tests. They preserve the actual Rust builders and loops for measured content,
baseline propagation, min/max and aspect sizing, all Linear enum matrices,
weights, display-none, out-of-flow, fixed, and sticky cases.

The generated Sticky position and sizing matrices additionally compare every
selected tree with an inset-free normal-flow reference and assert the complete
resolved `sticky_pos` export for points, percentages, `calc()`, paired insets,
and auto sentinels. Determinism checks include the four Sticky metadata edges.

The deterministic generator preserves the source seed (`0x5A17_1A64`), draw
order, complete 32,768-case stream, all 330 high-case IDs, and every named
regression ID. Every selected tree is rebuilt twice and compared across the
complete Rust layout result. Exact Linear-containing execution counts are:

| Family | Executions |
| --- | ---: |
| Static generated matrices | 1,758 |
| Default deterministic stream | 27,794 |
| Linear-containing high-case IDs | 257 |
| Named regression IDs | 17 |
| **Total** | **29,826** |

### Standalone

`tests/pr25_linear_standalone_inventory.txt` lists all 458 Linear-relevant
standalone head-to-head tests. Their aggregate functions expand to 543 Rust
executions.
`tests/pr25_linear_standalone.rs` preserves every source test function, its
actual Rust-side tree builder, and every loop/parameter row. It therefore runs
all 543 source executions directly instead of mapping names to a smaller set
of approximate fixtures. The companion
`tests/pr25_linear_standalone_support/mod.rs` supplies only the mutable
standalone convenience API needed by those builders and lowers each tree into
neutron-star's real Linear dispatch. The original C++ tree clone, baseline
engine call, FFI error types, snapshot comparison, and normalization code are
not migrated.

The non-Linear-named `out_of_flow_intrinsic_sizing` matrix is included in
full: four of its eight rows exercise positioned Block subtrees through the
source engine's latest Linear natural sizing, while the four measured rows
preserve the complete parameter matrix. The ten physical-pixel fixtures retain
their authored DPR and enter
neutron-star's `round_layout` pass after unrounded layout. Sticky and Relative
fixtures likewise retain their exact source shapes while remaining within the
host boundaries described above. The file has 469 literal Rust runner call
sites; loop expansion adds 74 further rows. A 458-name/543-execution inventory
guard counts both the loops and the eleven additional calls in the seven
source test functions that invoke the runner more than once, preventing a
source test or aggregate row from silently disappearing.

### Public layout surface

`tests/pr25_linear_public.rs` reconstructs the source order and shape of all
72 dedicated public Linear snapshots:

| Fixture family | Cases |
| --- | ---: |
| Dedicated gravity/weight fixtures | 6 |
| Orientation values | 8 |
| Main gravity, both axes | 22 |
| Cross gravity, both axes | 10 |
| Item layout gravity, both axes | 26 |

It also retains the Linear row of the non-Linear-named mixed public display
matrix: the exact `180×130` Block root, padded/bordered `120×72` horizontal
Linear container, three children, and the source's otherwise inactive
Flex/Grid/Relative fields all execute through the Rust host.

The separate list fixture's seven-node Linear fallback geometry is retained,
including test-local column-count, list-gap, and component-role metadata. The
seven rows of `rust_standalone_public_list_gap_layout_apis_match_cpp` are also
executed in source order for points, percentage, `calc()`, `auto`, `fr`,
`max-content`, and `fit-content()` values. Those values remain typed
test-host metadata while each row runs its actual Linear fallback geometry;
they are not added to the Linear L1 production protocol. The source
`rust_standalone_public_layout_variant_matrices_cover_public_values` count
checks are subsumed by the migration inventory's 6-display, 72-dedicated,
8-orientation, 22-main-gravity, 10-cross-gravity, 26-item-gravity, and
7-list-gap cardinality guards.

## Algorithm inventory

`tests/pr25_linear_inventory.rs` replaces the Linear half of the PR's four
combined Linear/Relative inventory tests. It binds the eight numbered spec
sections and Linear Items to production symbols in `compute/linear.rs`, the
Linear style/source traits, visibility and positioned-layout boundaries, and
the migration cardinalities above. It also binds the source Linear cross-axis
cache regression to the production cache's complete-`LayoutInput` key and its
executable public-layout fixture. It intentionally does not require C++
implementation paths.

## Benchmarks

PR #25 contains fourteen workloads whose Rust builder actually creates a
Linear container. The `linear_pr25` Divan target exposes all fourteen in
source order. All fourteen workload builders live in
`benches/scenarios/linear_pr25.rs`:

`at_most_owner_matrix`, `baseline_propagation_matrix`,
`measured_callback_matrix`, `in_flow_order_matrix`,
`full_value_spacing_matrix`, `staggered_linear_list`,
`staggered_linear_raw_list_gaps`, `linear_gravity_matrix`,
`linear_layout_gravity_matrix`, `linear_cross_gravity_matrix`,
`box_sizing_matrix`, `fit_content_subtrees`, `sticky_percent_insets`, and
`mixed_display_none`.

Mixed builders retain the source's full `N` loop, original indices, and every
Block/Flex/Linear/Grid/Relative display branch. Child-containing Block nodes
are host-dispatched as vertical Linear, matching the source Rust engine's
effective-display lowering; unmeasured childless Block nodes are empty Linear
containers, while only explicitly measured/callback source nodes become
leaves. Non-Linear source branches are not discarded.

The two staggered workloads and the Linear branches of
`full_value_spacing_matrix` construct their exact column-count, list-gap, and
component-role values in benchmark-host metadata. Timed runs black-box those
vectors without adding the host-only vocabulary to production layout traits.
All nine generic spacing variants and the exact 11/13/5 gravity value sets are
guarded by tests.

The Sticky workload keeps its percentage insets in benchmark-host metadata
and passes auto visual insets into neutron-star, preserving the source's
normal-flow layout cost without modeling the later scroll-time clamp as a
Relative offset. The timed run resolves, stores, and black-boxes the
source-equivalent `sticky_pos` export using its 320-by-40 containing bases.

The source timer includes `build_trees` as well as its one-layout-per-tree
pass. It batches all iteration-tree builds before batching their layouts
(`B...B, L...L`). The Divan target preserves that ordering and the source
defaults: each timed sample first builds 200 fresh trees with `N = 1_000`, then
lays out every tree once. `sample_count = 1` and `sample_size = 1` keep Divan
from multiplying that already-complete source batch; `ItemsCount` reports the
resulting 200,000 source items per sample. No tree or layout session is reused.

Logical topology and source-vector parent/child allocation order match PR
#25. The benchmark host first pushes an empty parent, allocates its descendants,
then appends their ids through the host's mutable source-node access, matching
PR #25's parent-first vector order and incremental child-vector growth.
Allocator capacity and placement remain implementation details rather than a
byte-for-byte identity claim.

The detailed workload lowerings and node-count guards are documented in
`docs/pr25-linear-benchmarks.md`.

The untagged `out_of_flow_intrinsic` benchmark is deliberately outside this
fourteen-workload inventory. Its authored styles are exclusively
`display: block`, and PR #25 classifies it as `BenchFeature::Block`, even
though the source caller currently implements child-containing Block through
the same effective Linear dispatch. Including every benchmark that happens to
traverse that compatibility lowering would turn this migration into a Block
benchmark migration; only workloads tagged with `BenchFeature::Linear` or
whose builders author an actual Linear container are in scope. The analogous
standalone `out_of_flow_intrinsic_sizing` test is retained above because its
source rows explicitly assert “latest linear sizing.”

## Deliberate semantic adaptations

The source PR commits through Lynx-specific implementation behavior. The
ported assertions follow this repository's documented engine boundaries:

1. Fractional CSS-pixel geometry remains unrounded until `round_layout`.
2. CSS `fit-content()` retains its min-content floor.
3. A caller-known tight border box remains authoritative; derived content
   size clamps to zero when padding and border exceed it.
4. Hidden subtrees use neutron-star's canonical zero layout, including zero
   offset.
5. Baseline export follows the Linear specification after final auto-margin
   alignment and exports no vertical fallback when the first item has none.
6. Fixed static position follows the box's hypothetical original formatting
   position before the host root-containing-block pass. It does not reproduce
   Lynx's unconditional reparent-and-realign quirk.

Each adapted direct assertion has an adjacent explanation; the five fixed
cases are also enumerated by `W3C_FIXED_STATIC_POSITION_ADAPTATIONS`.

## Adapter-only source tests

The following PR tests mention Linear fields but do not test Linear layout and
therefore are classified rather than fabricated inside neutron-star:

| Source surface | Classification |
| --- | --- |
| `style_data_coverage_tests.rs` copy-on-write tests | Lynx computed-style storage ownership |
| `standalone_tree_enum_scalar_and_vector_style_setters_update_style` | Owned standalone wrapper mutation/getter API |
| standalone scalar getter-setter tests | Wrapper storage API; observable Linear geometry is covered above |
| `block_layout_runs_on_external_tree` | Generic external-tree/Block-dispatch smoke; its Block-as-Linear geometry is subsumed by the explicit root/nested/positioned compatibility fixtures above |
| `gn_bridge_tests.rs` and native enum mapping inventory | GN/native bridge representation |
| `public_header_links_and_runs_c_smoke_when_ffi_library_is_available` | C ABI/link smoke; its two embedded Linear baseline/auto-margin geometry checks duplicate the two migrated Rust callback fixtures above, so the C harness is not copied |
| remaining `starlight_ffi` smoke tests | C/C++ ABI representation, validation, and public-header linkage; the five distinct Rust Linear geometry callbacks are migrated above |

These exclusions contain no missing generic Linear geometry. Neutron-star's
corresponding typed enum values and source traits are exercised by the direct,
generated, public, and benchmark targets.
