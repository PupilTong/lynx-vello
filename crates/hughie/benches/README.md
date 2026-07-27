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

All box-layout scenarios build real `w3c_dom::Document` trees with CSS styles.
The shared `support::LayoutFixture` resolves those styles outside the timed
region, then measured calls enter through `Document::layout`.
Consequently the timed path includes w3c-dom's production `&Node` host,
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
integration tests under `tests/` and the w3c-dom wiring tests.

Every measured closure is statically batched so its fastest walltime sample
stays in the millisecond range on the macOS CodSpeed runner. Divan counters
record the number of logical layouts, text measurements, or cache lookups in
the batch, preserving throughput reporting. Cold workloads use independent
fixtures within a batch; warm-cache workloads restore their intended cache
state between logical operations instead of accidentally becoming a different
cache-hit benchmark.
