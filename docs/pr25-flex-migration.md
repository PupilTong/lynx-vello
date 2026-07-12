# PupilTong/lynx#25 Flex migration inventory

This document records the Rust-only migration of every Flex-focused test and
benchmark surface in [`PupilTong/lynx#25`](https://github.com/PupilTong/lynx/pull/25).
It is an inventory, not a compatibility promise with the C++ Starlight engine:
standard CSS behavior is asserted against neutron-star directly.

## Execution boundary

- No C++ engine, native standalone library, FFI shim, generated C header,
  `build.rs`, or `native-standalone` feature is built or run.
- Test styles are already-computed values. No styling engine is involved.
- The source PR's `SimpleTree` fixtures lower into neutron-star's physically
  separate immutable source and mutable session through
  `tests/pr25_support/mod.rs`.
- `display: block`, `linear`, `relative`, and `grid` subtrees appearing inside
  otherwise Flex-focused source cases are host-dispatch boundaries. Until
  their peer algorithms exist, the compatibility fixture lowers container
  structure through a Flex adapter and documents the substitution. It does
  not claim parity for the foreign algorithm.
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

## Shared host and canonical Flex suite

| PR #25 source surface | Source scope | lynx-vello target | Migration form |
| --- | ---: | --- | --- |
| `starlight_parity/tests/flex_layout_tests.rs` | 140 tests | `crates/neutron-star/tests/pr25_flex_layout.rs` | Name-preserving Rust-only translation of all 140 tests |
| Existing neutron-star Flex smoke/conformance suite | 35 tests | `crates/neutron-star/tests/flexbox.rs` | Retained; fixture moved to `tests/support/mod.rs` |
| Cross-cutting aspect-ratio, positioned, sticky-boundary, Grid-metadata, source/session, collapse-strut, and solver cases | 15 tests | `crates/neutron-star/tests/pr25_flex_additional.rs` | Direct Rust assertions |
| Shared PR compatibility vocabulary | support code | `crates/neutron-star/tests/pr25_support/mod.rs` | `SimpleTree` lowering, no C++ |

The dedicated 140-test target preserves the source function names. That makes
the target itself the exhaustive per-test mapping; a second 140-row table
would duplicate the compiler-visible inventory without adding information.

## Native and generated head-to-head matrices

| PR #25 source surface | Source scope | lynx-vello target | Migration form |
| --- | ---: | --- | --- |
| `native_head_to_head_tests.rs` true Flex inventory | 191 source cases | `tests/pr25_native_flex.rs` | 101 Rust tests: 91 canonical overlaps are mapped explicitly; the remaining unique cases retain direct Rust geometry/invariant assertions |
| `native_generated_head_to_head_tests.rs` Flex matrices | generated matrix families | `tests/pr25_generated_flex.rs` | 15 parameterized Rust tests; no random C++ comparison |

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
| 8 | `flex_auto_margin_and_align_self` | main-axis auto-margin canonical cases |
| 9 | `cross_axis_auto_margin_over_stretch` | `cross_axis_auto_margin_overrides_stretch_alignment` |
| 10 | `paired_cross_axis_auto_margins` | `paired_cross_axis_auto_margins_center_item` |
| 11 | `multiple_main_axis_auto_margins` | `multiple_main_axis_auto_margins_share_positive_free_space_before_justify_content` |
| 12 | `flex_display_none_grow_and_order` | display-none plus stable-order canonical cases |
| 13 | `flex_justify_content_mapping` | public justify-content matrix |
| 14 | `flex_justify_content_direction_matrix` | main-axis direction/justify matrix |
| 15 | `flex_main_axis_auto_margin_direction_matrix` | main-axis auto-margin direction matrix |
| 16 | `flex_justify_content_gap_overflow_direction_matrix` | gap-overflow direction matrix |
| 17 | `space_evenly_single_item_distribution` | single-item space-evenly case |
| 18 | `space_between_single_item_fallback` | single-item space-between fallback |
| 19 | `space_around_single_item_fallback` | single-item space-around fallback |
| 20 | `flex_align_items_mapping` | public align-items matrix |
| 21 | `flex_align_self_mapping` | public align-self matrix |
| 22 | `flex_align_self_baseline_wrap_margins` | align-self baseline line-sizing case |
| 23 | `align_content_stretch_line_expansion` | align-content stretch case |
| 24 | `stretch_percent_height_relayout` | definite stretched cross-size relayout case |
| 25 | `stretch_min_max_cross_size_clamp` | stretched min/max clamp case |
| 26 | `flex_align_content_mapping` | public align-content matrix |
| 27 | `flex_direction_mapping` | public flex-direction matrix |
| 28 | `flexible_lengths_direction_mapping` | flexible-length direction matrix |
| 29 | `flex_min_max_freeze_distribution` | canonical flexible-length solver matrix |
| 30 | `definite_indefinite_flex_size_matrix` | percentage/aspect-ratio/fit-content canonical cases |

Additional standalone Flex groups are handled in
`tests/pr25_flex_standalone.rs` and `tests/pr25_flex_additional.rs`:

- `wrapped_flex_measured_callbacks`: two Rust cases, including a
  constraint-sensitive callback and measured fit-content container.
- `absolute_flex_initial_alignment`: center/end, oversized center, and
  wrap-reverse static-position cases.
- `absolute_rtl_flex_fronts`: RTL row, RTL column, and RTL
  column/wrap-reverse cases.

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

The matrix asserts finite/non-negative snapshots and family-specific ordering,
stretching, wrapping, distribution, and baseline invariants.

## `engine.rs` Flex tests

`engine_flex_dispatch_matrix_covers_nine_source_invariants` in
`tests/pr25_flex_internal.rs` covers the nine Flex-related engine tests:

1. percent-propagation context;
2. row grow distribution;
3. host measurement before display-algorithm dispatch;
4. `display:none` zeroing;
5. stretched subtree geometry export;
6. centered main-axis justification;
7. the non-CSS Flex `fr` boundary (`auto` in the CSS protocol);
8. canonical `column-gap` resolution;
9. canonical wrapped `row-gap` resolution.

The PR's raw-value `fr` gap/basis behavior is intentionally not generalized
into neutron-star: `fr` remains a Grid track unit.

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
belongs to Lynx's host-side Linear algorithm, not CSS Flexbox.

## Flex algorithm coverage inventory tests

The ten source meta-tests from `flexbox_algorithm_coverage_tests.rs` are
translated to repository-local checks in `tests/pr25_flex_inventory.rs`:

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
large alignment/distribution matrices.
