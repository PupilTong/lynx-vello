# PupilTong/lynx#25 Grid migration

This repository migrates the Grid-related Rust test and benchmark surface from
`PupilTong/lynx#25` into `crates/neutron-star`. It deliberately does not import
the PR's styling engine, native linker, or Lynx C++ comparison runner.

## Inventory

- 230 direct Grid layout tests: `tests/pr25_grid_layout.rs`.
- 179 Grid-named native head-to-head cases: 136 exact stripped-name mappings
  to the direct suite plus 43 Rust-only deterministic scenario tests in
  `tests/pr25_native_grid.rs`.
- 6 generated Grid matrices: item alignment, auto-margin alignment, track
  sizing, content alignment, auto-flow placement, and out-of-flow areas in
  `tests/pr25_generated_grid.rs`.
- 39 Grid standalone head-to-head wrapper cases and 4 Grid standalone public
  API cases in `tests/pr25_grid_standalone.rs`, all lowered to the generic
  Rust host protocol.
- 15 repository/algorithm inventory checks, translated to neutron-star's
  module and generic-trait architecture in `tests/pr25_grid_inventory.rs`.
- Cross-file aspect-ratio, sticky-position host-contract, nested-baseline,
  and GridAutoFlow enum coverage is mapped by `tests/pr25_grid_additional.rs`
  and the existing Flex migration. PR style-data copy-on-write tests are not
  imported because styling-engine storage is explicitly outside this task.
- 18 Grid-tagged benchmark scenarios in the `grid_pr25` benchmark target.

No migrated test is ignored. Legacy `head_to_head` and `cpp` text occurs only
inside retained source identifiers and migration explanations; execution is
Rust-only.

## W3C algorithm evidence

The implementation is split across `compute/grid/placement.rs`,
`compute/grid/tracks.rs`, `compute/grid/sizing.rs`,
`compute/grid/alignment.rs`, and `compute/grid/mod.rs`. Together they cover:

1. Grid item placement and sparse/dense auto-placement.
2. Absolute Grid containing areas and static-position alignment.
3. Alignment, gaps, auto margins, and RTL physical conversion.
4. Initialize track sizes.
5. Resolve intrinsic track sizes and distribute spanning contributions.
6. Maximize tracks.
7. Expand flexible tracks and find the `fr` size.
8. Stretch `auto` tracks.
9. A single bounded columns→rows cross-axis feedback rerun.

Named lines/areas, subgrid, fragmentation, and last-baseline alignment remain
outside the numeric protocol milestone and are recorded in
`docs/layout-architecture.md` and `docs/tracking/css-layout.md`.

## Intentional source-boundary translations

- PR #25 rounds through integer `LayoutUnit`; neutron-star retains fractional
  CSS pixels until its separate device-pixel rounding pass. Direct assertions
  allow only the corresponding half-pixel representation boundary.
- PR #25's positioned Grid path treats `fit-content(...)` as an uncapped
  legacy intrinsic keyword. The compatibility adapter keeps that fixture
  behavior only for positioned source cases; production keeps typed CSS
  `Dimension::FitContent`.
- PR #25's owner `AtMost` behavior and fixed-max redistribution are not a
  distinct neutron-star available-space variant. Those assertions retain the
  scenario but pin neutron-star's definite available-space and fractional
  distribution result.
- Source `fixed` Grid fixtures exercise the Grid-owned out-of-flow area path.
  They lower to direct absolute positioning in the compatibility target;
  production fixed-position containing-block selection remains the host-owned
  hoisted positioned pass.

## Grid benchmark scenarios

The exact 18 source scenario names are:

`at_most_owner_matrix`, `baseline_propagation_matrix`,
`measured_callback_matrix`, `in_flow_order_matrix`,
`full_value_spacing_matrix`, `box_sizing_matrix`, `fit_content_subtrees`,
`sticky_percent_insets`, `mixed_display_none`,
`grid_out_of_flow_intrinsic`, `grid_out_of_flow_areas`,
`grid_item_alignment_matrix`, `grid_content_alignment_matrix`,
`grid_auto_flow_matrix`, `grid_auto_margin_alignment`,
`grid_minmax_intrinsic_tracks`, `grid_auto_fit_content_max_tracks`, and
`grid_indefinite_auto_fit_content_max_tracks`.

The first nine are explicit Grid slices of mixed-display source workloads;
the final nine are direct Grid workloads. Fixture creation occurs outside the
timed closure, and every run dispatches through generic `GridSource` and
`LayoutSession` implementations with no `dyn` in production code.
