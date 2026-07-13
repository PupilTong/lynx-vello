# PupilTong/lynx#25 Linear benchmark migration

This note records the Rust-only migration of every benchmark scenario tagged
with `BenchFeature::Linear` in
[`PupilTong/lynx#25`](https://github.com/PupilTong/lynx/pull/25). The runnable
target is `crates/neutron-star/benches/linear_pr25.rs`; deterministic builders
and inventory guards live in `benches/scenarios/linear_pr25.rs` and
`tests/pr25_linear_bench_scenarios.rs`.

## Exact inventory

PR #25 has fourteen Linear-tagged workloads:

1. `at_most_owner_matrix`
2. `baseline_propagation_matrix`
3. `measured_callback_matrix`
4. `in_flow_order_matrix`
5. `full_value_spacing_matrix`
6. `staggered_linear_list`
7. `staggered_linear_raw_list_gaps`
8. `linear_gravity_matrix`
9. `linear_layout_gravity_matrix`
10. `linear_cross_gravity_matrix`
11. `box_sizing_matrix`
12. `fit_content_subtrees`
13. `sticky_percent_insets`
14. `mixed_display_none`

All fourteen builders are ported into `benches/scenarios/linear_pr25.rs` and
exposed, in source order, from the `linear_pr25` Divan/CodSpeed target. Keeping
the gravity builders local to this migration makes the benchmark provenance
and its timing boundary reviewable without depending on another target's
fixtures. Inventory tests pin the exact names, complete source display cycles,
logical topology, source indices, node-count formulas, and the exact
11/13/5 main/item/cross gravity value sets.

## Rust-only lowering boundary

Nine source workloads rotate through multiple display algorithms. Their
neutron-star builders retain the complete source `N` loop and every Block,
Flex, Linear, Grid, and Relative branch, including the original indices used
to generate dimensions, margins, orders, direction, orientation, measurement
callbacks, min/max constraints, box sizing, authored sticky metadata, and
`display: none` children. This matters both for topology and cost: extracting
only the Linear residue would benchmark a different workload.

neutron-star's host protocol has no Block formatting algorithm. As in the
source Rust engine's effective-display dispatch, a Block container with
children is therefore sent through a vertical Linear algorithm. The same is
true for an unmeasured childless `SimpleNode::new(Display::Block)`: it becomes
an empty vertical Linear container, not a synthetic measurement callback.
Only source nodes created by `with_measured_size`,
`with_measured_size_and_baseline`, or a measurement function become benchmark
leaves. The source benchmark's Block aggregation roots use the same host
lowering.

The two staggered workloads keep their node topology, nested Linear
containers, ordinary sizes/margins, and empty Flex item shape. Their
`linear-column-count`, list-axis gaps, and every optional `ListComponentType`
are also constructed and retained in benchmark-host metadata. The Linear
branches of `full_value_spacing_matrix` retain their column counts and both
list gaps in the same way. These values are black-boxed by the timed run, but
are not added to neutron-star because they are host list virtualization inputs
rather than generic Starlight Linear L1 style. The three workloads use the
`HostListProtocolElided` tag to make that boundary executable and reviewable.

`full_value_spacing_matrix` also crosses a typed style boundary. PR #25 stores
all generic `Length` variants in spacing slots; neutron-star consumes
style-resolved edge values. Points, percentages, and `calc()` remain their
corresponding typed forms. `auto` remains auto where the property permits it
and otherwise becomes zero. The raw fractional case maps to its numeric value;
max-content and unbounded fit-content become zero; fixed fit-content becomes a
length; and fixed-plus-percent fit-content becomes `calc()`. Tests guard all
nine source variants for both `LengthPercentage` and
`LengthPercentageAuto`. CSS row/column gaps remain stored in generic style,
while Linear list gaps stay in benchmark-host metadata.

Sticky items remain in flow during layout. The benchmark retains their
authored percentage insets in benchmark-host metadata while passing auto
visual insets to neutron-star, so the timed layout preserves normal-flow
geometry. The style's `Position::Relative` value is only the host protocol's
in-flow representation; it does not apply a visual relative offset because
all engine insets are auto. Resolving and applying scroll-time sticky
constraints remains a host post-pass. During every timed run, however, the
authored values are resolved into the source-equivalent exported `sticky_pos`
using the 320-pixel inline and 40-pixel block bases, stored, and black-boxed.
Auto sides use the source `-1e10` sentinel.

## Timing fidelity

PR #25 starts its `Instant` before `build_trees`, batches all iteration-tree
builds, then batches one layout for every newly built tree (`B...B, L...L`).
The Divan closure preserves that ordering and the source defaults: it first
builds 200 complete scenarios with `N = 1_000`, then lays out each fresh tree
once. Allocation, topology/style construction, and layout are all measured.
Each benchmark is fixed to one Divan sample of one complete source batch so
the harness does not multiply the source iteration loop. `ItemsCount` reports
200,000 source items; actual topology is scenario-specific and guarded by
tests. No tree or layout session is reused.

Logical topology and source-vector allocation order match the source. The
benchmark host pushes an empty parent, allocates its descendants, then appends
their ids through mutable source-node access, matching PR #25's parent-first
vector order and incremental child-vector growth. Allocator capacity and
placement remain implementation details rather than a byte-for-byte identity
claim.

## Explicit exclusions

No C/C++ source, native standalone library, FFI/C ABI layer, generated header,
environment gate, comparison runner, duration ratio, or speedup assertion is
migrated. The target measures only neutron-star's statically dispatched Rust
construction and layout path.
