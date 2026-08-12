# Layout benchmarks

Each layout algorithm has one Cargo benchmark target and one scenario module:

- `flexbox.rs` → `scenarios/flexbox.rs`
- `grid.rs` → `scenarios/grid.rs`
- `linear.rs` → `scenarios/linear.rs`
- `relative.rs` → `scenarios/relative.rs`

`text.rs` measures the Parley-backed text core directly; its committed box
cache workload also uses the shared production host.

The box-layout targets also include 20 text-bearing production-host workloads:
five clones of existing complex scenarios per algorithm. Flex clones its five
owner/direction/alignment/distribution/wrapping matrices. Grid clones dense
hole backfill, intrinsic spanning, unique span buckets, flexible-track freeze
thresholds, and nested grids. Linear clones weighted freezing, mixed
hidden/absolute children, dense percentage padding, percentage min/max, and
cross-gravity matrices. Relative clones wrap-width refinement, reverse chains,
disjoint cycles, duplicate IDs, and nested relative containers.

All box-layout scenarios build real `dom::Document` trees with CSS styles.
The shared `support::LayoutFixture` resolves those styles outside the timed
region, then measured calls enter through `Document::layout`.
Consequently the timed path includes dom's production `&Node` host,
per-node layout caches, positioned pass, and device-pixel rounding. There is
no benchmark-only `LayoutNode`, style view, node arena, or parallel tree.
Text-bearing scenarios additionally create real DOM text nodes, register the
deterministic embedded Ahem font, and inherit computed font styles from their
parent boxes. They run through the document's concrete Parley path: one shared
text context per document and retained artifacts per text node. Shaping,
rebreaking, baseline propagation, and box layout therefore share the same
timed layout call.

Benchmarks measure representative layout and cache workloads. They do not
prove correctness or compatibility. Exact geometry, measurement traces,
baselines, static positions, and cache results belong in the engine-native
integration tests under `tests/` and the dom wiring tests.

Every measured closure is statically batched so its fastest walltime sample
stays in the millisecond range on the macOS CodSpeed runner. Divan counters
record the number of logical layouts, text measurements, or cache lookups in
the batch, preserving throughput reporting. Cold workloads use independent
fixtures within a batch; warm-cache workloads restore their intended cache
state between logical operations instead of accidentally becoming a different
cache-hit benchmark.

## Standalone Hughie Flex cases

`hughie_flex.rs` is a separate Cargo benchmark target containing five focused
Hughie workloads: `wide_active_grow`, `wide_wrap_gap`, `wide_grow_freeze`,
`wide_shrink_freeze`, and `balanced_deep`. It is intentionally not folded into
the broader `flexbox` target, so the five-case signal can be selected and
tracked independently:

```sh
cargo bench -p hughie --bench hughie_flex
```

Each case contains exactly 4,095 nodes including the root and uses only the
existing production-host `LayoutFixture`. CSS resolution, tree construction,
the preparation layout, and explicit cache invalidation happen while Divan is
creating the input. The measured closure calls `Document::layout` once on the
fully invalidated Hughie tree and includes the production host, layout caches,
positioned pass, and pixel rounding.

These cases are Hughie regression benchmarks, not a cross-engine runner. They
contain no dependency, adapter, geometry oracle, CSV format, or execution path
for Taffy, Yoga, Starlight, or any other layout engine. The conclusions from
the one-off local comparison that motivated their shapes are retained in
[`docs/layout-engine-benchmark-study.md`](../../../docs/layout-engine-benchmark-study.md).
