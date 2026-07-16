# Layout benchmarks

Each layout algorithm has one Cargo benchmark target and one scenario module:

- `flexbox.rs` → `scenarios/flexbox.rs`
- `grid.rs` → `scenarios/grid.rs`
- `linear.rs` → `scenarios/linear.rs`
- `relative.rs` → `scenarios/relative.rs`

The previous split was historical rather than architectural: Flex and Linear
had reusable scenario registries, while Grid and Relative kept their fixture
builders in the benchmark entry point. All four algorithms were benchmarked,
but the directory layout made Grid and Relative look absent.

Benchmarks measure representative layout and cache workloads. They do not
prove correctness or compatibility. Exact geometry, measurement traces,
baselines, static positions, and cache results belong in the engine-native
integration tests under `tests/`.

Every measured closure is statically batched so its fastest walltime sample
stays in the millisecond range on the macOS CodSpeed runner. Divan counters
record the number of logical layouts, text measurements, or cache lookups in
the batch, preserving throughput reporting. Cold workloads use independent
fixtures within a batch; warm-cache workloads restore their intended cache
state between logical operations instead of accidentally becoming a different
cache-hit benchmark.
