# PupilTong/lynx#25 Relative migration

This document records the Rust-only migration of every `display: relative`
test and benchmark surface in
[`PupilTong/lynx#25`](https://github.com/PupilTong/lynx/pull/25) into
`crates/neutron-star`. Starlight Relative Layout is a Lynx extension, so its
id-based parent/sibling constraint behavior follows the Starlight algorithm
documented in `docs/starlight-relative-layout.md`; CSS `position: relative`
remains a separate feature.

## Execution boundary

- Head-to-head tests retain only their Rust trees, source names, parameter
  spaces, deterministic geometry, and protocol invariants. No migrated test
  invokes the Lynx C++ comparison runner.
- No C or C++ source, FFI/C ABI layer, native standalone linker, generated C
  header, `build.rs`, `native-standalone` feature, or GN wiring is imported.
- The source PR's computed-style copy-on-write and style-storage tests are not
  layout tests and remain outside neutron-star's styling-engine-free boundary.
- Standalone wrapper mutation/getter tests remain adapter-only, but observable
  Relative layout snapshots from that public API target are retained at the
  generic Rust host boundary.
- Test fixtures lower into the immutable `LayoutSource` plus mutable
  `LayoutSession` protocol. Production Relative dispatch stays statically
  typed through `RelativeSource`, `RelativeContainerStyle`, and
  `RelativeItemStyle`.
- No migrated Relative test is ignored.

## Migrated inventory

| PR #25 Rust surface | Source scope | neutron-star target | Migration form |
| --- | ---: | --- | --- |
| `relative_display_layout_tests.rs` | 72 direct tests | `tests/pr25_relative_layout.rs` | Name-preserving tests with direct Rust geometry and measurement assertions |
| `native_head_to_head_tests.rs` | 72 real `display: relative` cases | `tests/pr25_native_relative.rs` | 57 canonical mappings plus 15 unique Rust-only cases; four CSS-position false friends excluded |
| `native_generated_head_to_head_tests.rs` | 15 matrices / 429 Relative cases | `tests/pr25_generated_relative.rs` | Full Relative parameter spaces, executed twice for deterministic finite geometry |
| `standalone_head_to_head_tests.rs` | 401 Relative-container tests | `tests/pr25_relative_standalone.rs` | Every retained source name generates an independent Rust-only protocol test; six false friends excluded |
| `position_layout_tests.rs` | 2 additional sticky cases | `tests/pr25_relative_additional.rs` | In-flow Relative geometry plus an executable host sticky-inset boundary |
| `standalone_public_api_tests.rs` | 2 dedicated Relative snapshots, 1 Relative branch in the mixed display matrix, and `RelativeCenter` value coverage | `tests/pr25_relative_public.rs` | Concrete Rust-only geometry and enum coverage through the generic host protocol |
| `linear_relative_algorithm_coverage_tests.rs` | 4 algorithm inventory tests | `tests/pr25_relative_inventory.rs` | Repository-local spec-section, symbol, helper, and resolver-boundary guards |
| Relative benchmark surface | 9 workloads | `benches/relative_pr25.rs` and `benches/scenarios/relative.rs` | Cold Rust-only workloads; source mixed-display cases retain their Relative slice |

The existing `tests/relative.rs` conformance suite and `benches/relative.rs`
microbenchmarks remain in place. The PR #25 targets add source traceability and
larger matrices rather than replacing those focused tests.

## Standalone public API Relative slice

The source `rust_standalone_public_relative_layout_apis_match_cpp` test emits
two Rust snapshots. Both are retained with concrete geometry: a definite
`100×80` container exercising all four `RelativeCenter` values, parent-end
alignment, sibling adjacency, and sibling-side alignment; and an indefinite
one-pass cross-axis cycle exercising deterministic fallback. The Relative
branch of `rust_standalone_public_display_layout_apis_match_cpp` is also kept,
including its padding/border content origin and right-of/bottom-of children.

The generic public-value inventory is represented by an explicit four-value
`RelativeCenter` guard. Setter/getter bookkeeping and C/C++ enum ordinals are
not copied because neutron-star exposes typed traits rather than the source
standalone wrapper or native ABI.

## Native head-to-head partition

The native file has 76 test names containing `relative`. Four are CSS
`position: relative` false friends and are not part of `display: relative`:

1. `head_to_head_relative_calc_end_offsets_use_parent_constraints`
2. `head_to_head_relative_position_offsets_visual_result_without_changing_flow`
3. `head_to_head_relative_position_percent_offsets_use_parent_constraints`
4. `head_to_head_flex_relative_child_percent_offsets_use_container_constraints`

The remaining 72 cases partition exactly into 57 canonical cases whose
`head_to_head_`-stripped name exists in the direct suite, and 15 unique source
names retained by `UNIQUE_NATIVE_RELATIVE_SCENARIOS`. The latter cover naming
variants plus native-only center-none, sibling-edge, sticky-boundary, and
one-pass combinations. The target compares repeated Rust layouts, not Rust
against C++.

## Generated matrices

The exact 15 generated matrices and Relative case counts are:

| Matrix | Cases |
| --- | ---: |
| `generated_relative_center_parent_edge_matrix_matches_cpp` | 64 |
| `generated_relative_sibling_dependency_matrix_matches_cpp` | 32 |
| `generated_relative_missing_reference_matrix_matches_cpp` | 96 |
| `generated_relative_dependency_resolution_matrix_matches_cpp` | 30 |
| `generated_relative_measured_constraint_matrix_matches_cpp` | 20 |
| `generated_relative_composite_feature_matrix_matches_cpp` | 6 |
| `generated_measured_callback_matrix_matches_cpp` | 4 |
| `generated_flex_baseline_propagation_matrix_matches_cpp` | 6 |
| `generated_sizing_minmax_aspect_matrix_matches_cpp` | 8 |
| `generated_display_none_origin_matrix_matches_cpp` | 1 |
| `generated_out_of_flow_position_matrix_matches_cpp` | 32 |
| `generated_out_of_flow_sizing_matrix_matches_cpp` | 12 |
| `generated_fixed_descendant_matrix_matches_cpp` | 65 |
| `generated_sticky_position_matrix_matches_cpp` | 48 |
| `generated_sticky_sizing_matrix_matches_cpp` | 5 |
| **Total** | **429** |

These names remain source trace keys even though the `_matches_cpp` suffix no
longer describes execution. Each tree is run twice through neutron-star and
must produce identical finite, non-negative geometry.

## Standalone head-to-head inventory

`tests/pr25_relative_standalone.rs` contains all 401 Relative-container source
names in `STANDALONE_RELATIVE_CASES`. Its name-driven fixtures exercise
one-pass/two-pass layout, parent and sibling edges, dependency cycles and
chains, wrap sizing, measured reflow, `display: none`, absolute and hoisted
children, centering, and the sticky host boundary.

The following six standalone false friends only exercise CSS
`position: relative` or a mixed non-Relative helper and are excluded:

1. `standalone_owned_tree_matches_cpp_for_linear_relative_position_physical_pixel_rounding`
2. `standalone_owned_tree_matches_cpp_for_relative_position_offsets`
3. `standalone_owned_tree_matches_cpp_for_linear_relative_position_left_top_preserves_horizontal_flow`
4. `standalone_owned_tree_matches_cpp_for_linear_relative_position_right_bottom_calc_preserves_vertical_flow`
5. `standalone_owned_tree_matches_cpp_for_linear_relative_position_start_insets_win_over_end_insets`
6. `standalone_owned_tree_matches_cpp_for_calc_and_out_of_flow_edge_behaviors`

## Five direct-suite translation families

The source fixtures are retained, but assertions follow this repository's
documented engine boundaries and standards policy in five places:

1. **Fractional rounding.** PR #25 commits through Lynx integer `LayoutUnit`.
   neutron-star keeps fractional CSS-pixel geometry (for example `23.5px`)
   until the separate device-pixel `round_layout` pass. The companion
   `relative_fractional_geometry_rounds_only_in_the_device_pixel_pass` test
   pins the 2x-device-pixel result.
2. **CSS fit-content min-content floor.** `fit-content(<limit>)` cannot clamp
   below the measured min-content contribution. The fixed-limit fixture
   therefore remains `80×40`, rather than shrinking to its `50×30` argument.
3. **Caller constraint authority.** A caller-known tight border box remains
   authoritative even when padding and border exceed it; only the derived
   content box clamps to zero. The same protocol distinction explains the
   single-sided Relative cases: a proposed start position does not rewrite
   the owner's full at-most measure bound, while a fixed parent height is
   already definite during the initial two-pass measurement.
4. **One-pass is not retroactive.** Deterministic cycle fallback processes the
   lowest-index item once. A dependency resolved by a later item does not go
   back and reposition that already processed item.
5. **Fixed and Sticky are host boundaries.** A fixed descendant is exported
   as `Position::AbsoluteHoisted` and completed by the host's positioned pass
   against the root containing block. Sticky remains in flow during Relative
   layout; the host retains and resolves its authored insets during the
   scroll-time sticky post-pass.

## Algorithm inventory

The four source inventory tests are retained by exact name:

1. `linear_relative_algorithm_inventory_tracks_starlight_spec_sections`
2. `linear_relative_inventory_targets_existing_symbols`
3. `linear_relative_inventory_traces_current_module_helpers`
4. `linear_relative_inventory_records_layout_resolver_and_visibility_boundaries`

They bind the eight numbered algorithm sections plus Relative Items,
Reference Resolution, and Dependency Ordering to the production symbols in
`compute/relative.rs`: `resolve_item`, `dependency_order`,
`axis_constraints`, `measure_item`, `position_axis`, `one_pass_layout`,
`two_pass_layout`, `commit_in_flow`, `commit_out_of_flow`, and
`compute_relative_layout`. They also pin the shared style resolver,
`display: none` subtree hiding, and `visibility: hidden`/`collapse`
participation boundaries.

## Relative benchmark scenarios

The exact nine Rust benchmark workload names are:

`at_most_owner_matrix`, `baseline_propagation_matrix`,
`measured_callback_matrix`, `box_sizing_matrix`, `fit_content_subtrees`,
`relative_dependency_graph`, `relative_center_matrix`,
`sticky_percent_insets`, and `mixed_display_none`.

`relative_dependency_graph` and `relative_center_matrix` are direct Relative
workloads. The other seven are the Relative slices of mixed-display source
workloads. All use the source two-pass default, build fresh cold cases outside
the timed closure according to this repository's Divan convention, and
dispatch only through Rust traits. The C++ duration, speedup ratio, environment
gate, and native benchmark runner are deliberately absent.

## Explicitly excluded PR surfaces

The migration does not copy tests whose subject is adapter representation
rather than Relative layout:

- `starlight_cpp` native enum/setter/getter and snapshot plumbing;
- `starlight_ffi` callback, C ABI, header, and link smoke tests;
- C/C++ standalone implementation and source-built comparison glue;
- computed-style storage, copy-on-write, and `DataRef` ownership tests;
- standalone wrapper mutation, dirty-state, and style getter/setter tests whose
  only assertion is adapter storage (their observable Relative snapshots are
  migrated separately above);
- GN actions, bridge-token checks, generated-header coverage, and native
  feature wiring.

Equivalent Relative geometry from mixed callback or FFI cases is represented
at the generic Rust host boundary. No C++ or FFI surface is fabricated inside
the standalone-publishable neutron-star crate.
