# PupilTong/lynx#25 Flex migration inventory

This document records the Rust-only migration of every Flex-focused test and
benchmark surface in [`PupilTong/lynx#25`](https://github.com/PupilTong/lynx/pull/25).
It is an inventory, not a compatibility promise with the C++ Starlight engine:
standard CSS behavior is asserted against neutron-star directly.

## Execution boundary

- No C++ engine, native standalone library, FFI shim, generated C header,
  `build.rs`, or `native-standalone` feature is built or run.
- Tests whose only subject is those absent adapter layers (C ABI field layout,
  native enum conversion, public-header coverage, GN wiring, or standalone
  wrapper mutation/dirty APIs) are classified as adapter-only rather than
  Flex layout tests. Their Rust layout scenarios are covered at neutron-star's
  typed protocol or algorithm boundary; their C/C++ representation assertions
  are not copied into this standalone engine.
- Test styles are already-computed values. No styling engine is involved.
- The source PR's `SimpleTree` fixtures lower into neutron-star's physically
  separate immutable source and mutable session through
  `tests/pr25_support/mod.rs`.
- `display: block`, `linear`, `relative`, and `grid` subtrees appearing inside
  otherwise Flex-focused source cases are host-dispatch boundaries. Block
  follows Starlight's caller-owned Block-as-Linear mapping; Linear, Relative,
  and Grid use their real peer algorithms. A Flex-focused target still does
  not claim parity for foreign-algorithm fields that its fixture explicitly
  omits.
- `fr` outside Grid is not valid CSS Flex syntax. Source-only raw `fr` values
  lower to `auto`; canonical CSS length/percentage/calc cases remain covered.
- Sticky positioning is a host post-pass. Flex tests cover its in-flow box and
  the authored-inset lowering boundary, not scroll-container behavior.
- Source cache-parity tests are rebuilt through the stateless integration-test
  session. They assert geometry and dispatch results, not C++ cache-hit counts.
- Head-to-head tests retain only their Rust trees, deterministic geometry,
  matrix cardinality, and protocol invariants.
- Flex sizing and alignment retain fractional CSS-pixel geometry. Source test
  identifiers mentioning integer/edge rounding remain trace keys only; they
  do not import Lynx's integer layout-unit results. The optional final
  device-pixel pass uses [CSS Values' nearest-integer rule](https://drafts.csswg.org/css-values-4/#integers),
  whose exact half-way ties resolve toward positive infinity.

The full PR diff also contains tests that mention Flex fields without testing
the Flex algorithm. They are classified at their actual boundary:

| Source surface | Classification in lynx-vello |
| --- | --- |
| `starlight_cpp/src/{lib,native}.rs`, `starlight_ffi/{src,tests}`, `gn_bridge_tests.rs`, `native_enum_mapping_coverage_tests.rs` | Native/C ABI, C++ enum conversion, generated-header, and bridge coverage. neutron-star exposes none of those layers. Duplicate Rust geometry is covered below; adapter representation checks are not layout tests. |
| `starlight_standalone/tests/standalone_tree_tests.rs` | Mixed wrapper/API surface. Its 15 tests that execute a Flex fixture are classified by exact name below; geometry is mapped, while wrapper state and nonstandard rounding remain explicit exclusions. |
| `style_data_coverage_tests.rs` | Computed-style copy-on-write storage, outside the styling-engine-free layout boundary. |
| Grid tests containing “flexible”, “flex fraction”, or alignment values named `flex-start`/`flex-end` | CSS Grid track sizing or shared alignment vocabulary, not CSS Flexbox. The one real cross-algorithm Flex guard is migrated explicitly. |
| Linear tests using `flex-start`/`flex-end` alignment tokens | Lynx Linear behavior, not CSS Flexbox. The one Linear-only helper guard in `engine/flex.rs` remains excluded from the 24 Flex solver mappings. |

Conversely, `standalone_public_api_tests.rs` does generate observable Flex
layout snapshots rather than merely checking adapter representation, so its
47-case Rust side is migrated below.

### Standalone wrapper traceability

Fifteen tests in `standalone_tree_tests.rs` actually execute an explicit or
default Flex fixture. Seven have observable geometry mapped to engine-level
targets; their wrapper mutation/dirty-state halves remain host policy:

| Source test | Geometry target |
| --- | --- |
| `standalone_tree_layouts_owned_nodes_with_owner_constraints` | `external_host_measurement_baseline_and_writeback_survive_split_storage` |
| `standalone_tree_measurement_api_tracks_dirty_state_and_baseline` | `flex_layout_uses_external_text_layout_trait_for_content_size_and_baseline` |
| `standalone_tree_measure_func_receives_constraints_and_can_be_replaced` | `compute_leaf_layout_uses_the_host_measurement` (semantic leaf dispatch precedes display dispatch) |
| `standalone_tree_baseline_func_receives_content_size_and_can_be_replaced` | `flex_row_baseline_uses_measured_content_baseline` |
| `standalone_tree_node_mut_style_changes_dirty_ancestors_and_next_layout` | canonical min-width/freeze geometry; `node_mut` and dirty propagation stay host-owned |
| `standalone_tree_owner_direction_reaches_unset_descendants_only_during_layout` | `standalone_direction_mapping_runs_all_8_source_cases`; temporary owner-direction inheritance stays host policy |
| `standalone_tree_clear_direction_restores_owner_direction_inheritance` | the same direction matrix; clearing/inheritance/dirty bookkeeping stays host policy |

Seven more use layout only to establish wrapper state or compare getters with
the same stored record, so their representation assertions are not copied:
`standalone_tree_edge_style_setters_match_public_standalone_edges`,
`standalone_tree_dimension_style_setters_match_public_standalone_lengths`,
`standalone_tree_enum_scalar_and_vector_style_setters_update_style`,
`standalone_tree_exposes_layout_getters_with_edge_resolution`,
`standalone_tree_dirty_state_tracks_mutations_and_layout`,
`standalone_tree_reset_node_clears_children_layout_style_and_measurement`, and
`standalone_tree_reset_attached_child_preserves_clean_parent_behavior`.

The final case,
`standalone_tree_measured_layout_ceil_uses_node_physical_pixels_per_layout_unit`,
asserts Lynx's ceil policy (`10.2 → 11` at DPR 1 and `10.2 → 10.5` at DPR 2).
It is intentionally replaced by `round_layout_snaps_on_the_device_pixel_grid`
and `round_layout_uses_css_positive_infinity_tie_breaking`, which implement the
CSS nearest-integer rule rather than that Lynx result.

## Shared host and canonical Flex suite

| PR #25 source surface | Source scope | lynx-vello target | Migration form |
| --- | ---: | --- | --- |
| `starlight_parity/tests/flex_layout_tests.rs` | 140 tests | `crates/neutron-star/tests/pr25_flex_layout.rs` | Name-preserving Rust-only translation of all 140 tests |
| Existing neutron-star Flex smoke/conformance suite | 35 tests | `crates/neutron-star/tests/flexbox.rs` | Retained; fixture moved to `tests/support/mod.rs` |
| Cross-cutting aspect-ratio, percentage-edge, positioned, sticky-boundary, Grid-metadata, source/session, collapse-strut, and solver cases | 18 tests | `crates/neutron-star/tests/pr25_flex_additional.rs` | Direct Rust assertions |
| Shared PR compatibility vocabulary | support code | `crates/neutron-star/tests/pr25_support/mod.rs` | `SimpleTree` lowering, no C++ |

The dedicated 140-test target preserves the source function names. That makes
the target itself the exhaustive per-test mapping; a second 140-row table
would duplicate the compiler-visible inventory without adding information.

## Cross-file Flex tests

Outside the dedicated Flex suite and the two internal engine files, the PR has
13 Rust `#[test]` functions whose subject includes Flex layout. The inventory
checks 11 source names against direct engine/protocol targets and records 2
sticky cases separately as in-flow Flex geometry plus an executable host
contract:

| PR #25 source file | Flex tests | Rust evidence |
| --- | ---: | --- |
| `aspect_ratio_layout_tests.rs` | 1 | Direct aspect-ratio transfer geometry |
| `box_edges_layout_tests.rs` | 1 | Vertical padding and margin percentages resolve from the containing block's inline size, retaining the fractional `37.4px` result |
| `position_layout_tests.rs` | 8 | Six direct absolute/relative cases; two sticky cases split into direct in-flow Flex geometry and a test-only host-boundary contract |
| `grid_layout_tests.rs` | 1 | Grid placement metadata cannot perturb Flex geometry |
| `starlight_layout/tests/external_tree_tests.rs` | 2 | Split immutable/mutable storage, measurement/baseline/writeback, and a minimal write-only session |

neutron-star deliberately has no production sticky offset/clamping API; that
scroll-time behavior belongs to the host post-pass. The two sticky fixtures
therefore specify the retained inset-resolution contract without claiming to
test a production sticky implementation.

The first external-tree source case also combined three host-policy details.
The target decomposes them deliberately: external identifiers are mapped to
the engine's typed `NodeId` by the host, `LayoutInput` is the transient
constraint record, and `protocol::round_layout_snaps_on_the_device_pixel_grid`
checks the independent `2x` final-rounding pass. None requires mutable style
storage or a C callback table inside neutron-star.

## Native and generated head-to-head matrices

| PR #25 source surface | Source scope | lynx-vello target | Migration form |
| --- | ---: | --- | --- |
| `native_head_to_head_tests.rs` true Flex inventory | 191 source cases | `tests/pr25_native_flex.rs` | 101 Rust tests: 91 canonical overlaps are mapped explicitly; the remaining unique cases retain direct Rust geometry/invariant assertions |
| `native_generated_head_to_head_tests.rs` Flex matrices | generated matrix families | `tests/pr25_generated_flex.rs` | 17 Rust tests: 1,059 static matrix cases, all 27,637 Flex-containing trees in the default deterministic stream, all 315 Flex-containing trees in the source high-case list, and 7 named regressions; no C++ comparison |

The deterministic generated tests retain the source LCG seed (`0x5A17_1A64`),
the 32,768-case default stream cardinality, and all 330 IDs in the source
high-case list. Selection is based on any node having `display: flex`, not
only the root: the default set is 27,637 cases (8,380 Block roots, 10,923 Flex
roots, and 8,334 Linear roots), while the high-case set is 315 (19/264/32 by
the same root order). Every unselected case still consumes its complete RNG
draw sequence so later case IDs remain reproducible. Each selected tree is
laid out twice by neutron-star and asserts identical complete node geometry,
finite non-negative sizes, and aggregate input/output diversity and non-zero
geometry. The 16,714 default and 51 high-case Block/Linear-root cases dispatch
through real Linear layout (Block uses the source engine's Block-as-Linear
mapping). They remain Flex-focused protocol smoke tests because this older
builder intentionally discards foreign-only Linear gravity draws; the exact
Linear generator is migrated separately in `pr25_generated_linear.rs`. No
case imports Lynx's integer rounding results.

`pr25_native_flex.rs` contains `NATIVE_FLEX_INVENTORY` and the canonical-overlap
inventory so the 191/91 cardinalities cannot drift silently.

## Standalone dedicated Flex cases

The first 30 Flex-only tests in `standalone_head_to_head_tests.rs` map as
follows. The mapping is checked by
`standalone_dedicated_inventory_maps_all_30_cases_to_rust_targets`.

| # | PR standalone test suffix | Rust target |
| ---: | --- | --- |
| 1 | `measured_flex_row` | `flex_layout_uses_external_text_layout_trait_for_content_size_and_baseline` |
| 2 | `flex_wrap_alignment_and_at_most_cross_axis` | `flex_wrap_cross_axis_at_most_does_not_clamp_line_sum_latest_mode` |
| 3 | `flex_wrap_zero_sized_item_after_exact_fit` | `flex_wrap_collects_zero_sized_item_after_exact_fit_on_same_line` |
| 4 | `single_line_min_cross_size_clamp` | `single_line_min_cross_size_clamps_line_before_cross_alignment` |
| 5 | `flex_wrap_reverse_rtl_row_reverse` | cross-axis direction/wrap-reverse matrix |
| 6 | `flex_wrap_reverse_space_between_lines` | `flex_wrap_reverse_reverses_space_between_line_distribution` |
| 7 | `align_self_override` | `align_self_overrides_container_align_items` |
| 8 | `flex_auto_margin_and_align_self` | direct combined auto-margin + align-self geometry |
| 9 | `cross_axis_auto_margin_over_stretch` | `cross_axis_auto_margin_overrides_stretch_alignment` |
| 10 | `paired_cross_axis_auto_margins` | `paired_cross_axis_auto_margins_center_item` |
| 11 | `multiple_main_axis_auto_margins` | `multiple_main_axis_auto_margins_share_positive_free_space_before_justify_content` |
| 12 | `flex_display_none_grow_and_order` | direct combined hidden-item exclusion, grow, and order geometry |
| 13 | `flex_justify_content_mapping` | public justify-content matrix |
| 14 | `flex_justify_content_direction_matrix` | main-axis direction/justify matrix |
| 15 | `flex_main_axis_auto_margin_direction_matrix` | main-axis auto-margin direction matrix |
| 16 | `flex_justify_content_gap_overflow_direction_matrix` | gap-overflow direction matrix |
| 17 | `space_evenly_single_item_distribution` | single-item space-evenly case |
| 18 | `space_between_single_item_fallback` | single-item space-between fallback |
| 19 | `space_around_single_item_fallback` | single-item space-around fallback |
| 20 | `flex_align_items_mapping` | direct row/column 14-case geometry matrix |
| 21 | `flex_align_self_mapping` | direct row/column 14-case geometry matrix |
| 22 | `flex_align_self_baseline_wrap_margins` | align-self baseline line-sizing case |
| 23 | `align_content_stretch_line_expansion` | align-content stretch case |
| 24 | `stretch_percent_height_relayout` | definite stretched cross-size relayout case |
| 25 | `stretch_min_max_cross_size_clamp` | stretched min/max clamp case |
| 26 | `flex_align_content_mapping` | direct row/column × wrap/reverse × 9-value, 36-case matrix |
| 27 | `flex_direction_mapping` | direct 4-direction × LTR/RTL, 8-case geometry matrix |
| 28 | `flexible_lengths_direction_mapping` | flexible-length direction matrix |
| 29 | `flex_min_max_freeze_distribution` | one-to-one mapping of all 33 canonical solver cases |
| 30 | `definite_indefinite_flex_size_matrix` | one-to-one mapping of all 5 percentage/aspect-ratio/fit-content cases |

Additional standalone Flex groups are handled in
`tests/pr25_flex_standalone.rs` and `tests/pr25_flex_additional.rs`:

- `wrapped_flex_measured_callbacks`: two Rust cases, including a
  constraint-sensitive callback and measured fit-content container.
- `absolute_flex_initial_alignment`: center/end, oversized center, and
  wrap-reverse static-position cases.
- `absolute_rtl_flex_fronts`: RTL row, RTL column, and RTL
  column/wrap-reverse cases.

The source's two large aggregate functions are not represented by a single
sample: `STANDALONE_MIN_MAX_FREEZE_MAPPINGS` names all 33 invoked builders and
their exact canonical Rust tests, while
`STANDALONE_DEFINITE_INDEFINITE_MAPPINGS` does the same for all 5 sizing
cases/invocations. The two combination regressions, the 28 row/column
`align-items`/`align-self` cases, the 36 `align-content` cases, and the 8
direction/bidi cases execute their source-shaped trees directly in the
standalone target.

## Standalone public API: 47 Flex snapshots

The source function `rust_public_flex_layout_snapshots` emitted exactly 47
snapshots. `tests/pr25_flex_public.rs` runs the same cardinality as a Rust-only
parameter matrix:

| Snapshot family | Cases |
| --- | ---: |
| alignment/order, grow, shrink, dedicated align-content | 4 |
| `flex-wrap` (`wrap`, `nowrap`, `wrap-reverse`) | 3 |
| all `align-content` values | 9 |
| all `flex-direction` values | 4 |
| all `justify-content` values | 9 |
| all `align-items` values | 7 |
| mixed per-item `align-self` snapshot | 1 |
| `align-self`: inherited plus all seven explicit values | 8 |
| align-items baseline and align-self baseline | 2 |
| **Total** | **47** |

The matrix asserts concrete W3C geometry for every node in every case: root and
item offsets, fractional sizes, propagated/local baselines, and the zero box
edges authored by these builders. Its expectation tables cover visual `order`,
scaled grow/shrink distribution, all forward/reverse directions, nowrap
shrinkage, wrap/wrap-reverse line placement, every spacing/alignment value,
stretch, and measured baseline-sharing groups. The dedicated
`align-content: space-between` snapshot retains the source builder's `55×95`
container (and therefore `25px` inter-line gaps), while the nine-value variant
matrix retains its separate `55×105` builder. Values such as `33.5px` and
`130/3px` are asserted directly as CSS-pixel geometry; no Lynx/C++ integer
rounding baseline is imported.

## `engine.rs` Flex tests

`ENGINE_FLEX_MAPPINGS` in `tests/pr25_flex_internal.rs` inventories the nine
tests in `engine.rs` that explicitly exercise a Flex container, plus the
shared percentage-padding test that protects the same W3C basis in Flex:

1. percent-propagation context;
2. row grow distribution;
3. host measurement before display-algorithm dispatch;
4. stretched subtree geometry export;
5. centered main-axis justification;
6. percentage padding on both physical axes using the containing block's
   inline size;
7. the non-CSS Flex `fr` basis boundary;
8. non-CSS intrinsic/`fr` gap values; and
9. non-CSS intrinsic/`fr` inset, margin, and padding values.

The source raw-value cases are exclusions, not CSS features. `fr` remains a
Grid track unit, while Flex gaps accept `LengthPercentage` and edges accept
`LengthPercentage(Auto)`. The compatibility fixture proves the unsupported
source values stop at that typed boundary (`fr` basis becomes `auto`; invalid
edge/gap values become zero) rather than generalizing Lynx/Starlight behavior.
`engine_flex_dispatch_matrix_covers_nine_source_invariants` retains additional
direct dispatch regressions such as `display:none` and canonical fixed gaps;
the name-to-target inventory no longer treats those extras as substitutes for
the raw-value exclusion tests.

## `engine/flex.rs` solver tests

The source file contains 25 tests in its test modules; one is explicitly a
Linear cross-axis cache guard. The remaining 24 Flex tests are tracked by
`SOLVER_MAPPINGS` in `tests/pr25_flex_internal.rs`:

| Solver group | Source tests | Rust target family |
| --- | ---: | --- |
| factor choice, initial/free-space inputs | 4 | two additional solver regressions plus grow canonical |
| partial grow/shrink and scaled shrink | 3 | canonical flexible-length tests |
| min/max violation freeze loops | 5 | canonical min/max redistribution tests |
| percentage-base and default-constraint helpers | 3 | descendant percentage and line-length tests |
| justify/align-content/auto-margin helpers | 3 | full direction and overflow matrices |
| cache/min-clamp/line-collection helpers | 3 | stretch relayout, auto-minimum, line collection |
| baseline/used-margin/cross-offset helpers | 3 | baseline and cross-axis matrices |
| out-of-flow Flex-axis mapping | 1 | positioned RTL/front matrix |
| **Total** | **24** | |

The excluded `linear_cross_axis_cache_guard_covers_ignored_stretch_and_auto_children`
belongs to neutron-star's Starlight Linear algorithm, not CSS Flexbox. It is
executed through the public cache/layout contracts in
`tests/pr25_linear_external_callback.rs`.

## Flex algorithm coverage inventory tests

The ten source meta-tests from `flexbox_algorithm_coverage_tests.rs` are
translated to repository-local checks in `tests/pr25_flex_inventory.rs`.
An eleventh local check guards the 11 direct cross-file mappings and 2 explicit
host contracts; a twelfth guards the 15 standalone-wrapper classifications:

1. every CSS Flexbox §9 pass is documented;
2. stretch plus aspect-ratio coverage is present;
3. alignment clauses have canonical target tests;
4. initial setup has order/hidden/out-of-flow targets;
5. line-length collection has target tests;
6. flexible main-size resolution has target tests;
7. cross-size and baseline resolution has target tests;
8. documented targets resolve to existing symbols/test functions;
9. migration boundaries prevent a false C++-parity claim;
10. known host-boundary debt markers remain documented.

## Benchmarks

PR #25's ordinary `Instant` binary was not copied. Its 18 Flex-tagged
scenarios are Divan/CodSpeed benches in `crates/neutron-star/benches/flexbox.rs`.
Scenario construction is outside the timed region; only Rust layout executes.

| # | PR scenario | Divan target |
| ---: | --- | --- |
| 1 | `flex_grow_row` | `flex_grow_row` |
| 2 | `flex_wrap_gaps` | `flex_wrap_gaps` |
| 3 | `flex_at_most_root` | `flex_at_most_root` |
| 4 | `at_most_owner_matrix` | `at_most_owner_matrix` Flex slice |
| 5 | `standalone_owner_direction_inheritance` | pre-resolved owner-direction Flex workload |
| 6 | `flex_axis_alignment_matrix` | `flex_axis_alignment_matrix` |
| 7 | `flex_distribution_matrix` | `flex_distribution_matrix` |
| 8 | `flex_wrap_alignment_matrix` | `flex_wrap_alignment_matrix` |
| 9 | `flex_baseline_measured` | `flex_baseline_measured` |
| 10 | `baseline_propagation_matrix` | `baseline_propagation_matrix` Flex slice |
| 11 | `measured_callback_matrix` | `measured_callback_matrix` Flex slice |
| 12 | `absolute_children` | `absolute_children` |
| 13 | `nested_column_flex` | `nested_column_flex` |
| 14 | `in_flow_order_matrix` | `in_flow_order_matrix` Flex slice |
| 15 | `full_value_spacing_matrix` | `full_value_spacing_matrix` Flex slice |
| 16 | `box_sizing_matrix` | `box_sizing_matrix` Flex slice |
| 17 | `fit_content_subtrees` | `fit_content_subtrees` Flex slice |
| 18 | `mixed_display_none` | `mixed_display_none` Flex slice |

`tests/flexbox_bench_scenarios.rs` checks the 18-name inventory, runs every
builder through Rust-only dispatch, and verifies the periods of the three
large alignment/distribution matrices. The eight mixed-display lowerings also
have exact linear node-count guards. The six display-filtered parameter
matrices retain the source Flex rows' raw indices (`5k + 1` or `4k + 1`)
instead of renumbering the filtered slice, so direction, axis, box-sizing,
measured-size, and spacing phases remain the source phases. The baseline slice
retains its three Flex sources; the display-none slice retains the same
15-case Flex parameter set. `fit_content_subtrees` and the owner-constraint
slice retain their fixed-plus-percentage `calc()` arguments. The absent Block
root algorithm is represented only by a column-Flex host adapter carrying the
source root's authored size/min/max/edges and owner constraints.
