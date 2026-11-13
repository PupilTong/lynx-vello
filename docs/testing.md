# Testing

Linked from the Testing section of the root [AGENTS.md](../AGENTS.md), which
keeps only the prerequisites.

## Input robustness at the external-byte boundaries

`bobcat-source::web` and `bobcat-source::xml` are the parsers fed bytes the
engine did not produce — a downloaded `.web.bundle` and an authored `.lynx.xml`
— so the property that matters is that no input takes the process down.
`StyleInfo` validation stays on the calling thread on every platform, the crate
forbids unsafe code, and three bounds hold before a caller receives an owned
tree: a 1 MiB section length, a validation subtree depth of 72 enforced with
rkyv 0.7's `ArchiveValidator::with_max_depth` and the safe
`check_archived_root_with_context` API, and a 64-level rule depth on the
returned tree (`crates/bobcat-source/src/web/style_info.rs`).
`crates/bobcat-source/tests/robustness.rs` (a fixed-seed mutator, 20 000
inputs, named degenerate cases, a coverage floor) and
`crates/bobcat-source/tests/decode_web_bundle.rs` (an 8000-level archive, a
populated 64-level tree on a 256 KiB caller stack, the 65-level rejection, real
fixtures on native and Wasm) pin them. This is deliberately not a fuzzer. See
`docs/source-architecture.md` and `crates/bobcat-source/tests/conversion.rs`
for the native parser's Lepus/CSS recursion bounds, overlapping-payload
rejection and CSS fallback expansion cap.

## The unsafe floor

`seldon`, `prime-radiant`, and `bobcat-source` carry `#![forbid(unsafe_code)]`. The
workspace-wide `unsafe_code = "warn"` is a lint any module can silence locally;
`forbid` cannot be overridden from inside the crate, so `unsafe` appearing in
one of these three has to be a deliberate edit to that line.

Where `unsafe` is unavoidable, the bar is a `SAFETY` comment per block,
enforced by a crate-local `#![warn(clippy::undocumented_unsafe_blocks)]` in
`bobcat-cli` (which now holds no `unsafe` at all, the lint standing as a bar
for any that arrives) and in `terminus` (its two, and one test-only block that
hands wgpu a bare `CAMetalLayer`). The lint is still crate-local
rather than workspace-wide because `quickjs-rust-bridge` (133 blocks) is the
last holdout and is being restructured separately; raising it there is what
would let this move into `[workspace.lints.clippy]`.

One trap, worth knowing before writing the comment: the lint does **not** scan
past an intervening attribute. Where an unsafe site carries
`#[expect(unsafe_code, reason = …)]`, as every one in `terminus` does, the
`// SAFETY:` comment has to sit *between* that attribute and the block —
placing it above the attribute still trips the lint, and `cargo fmt` will not
move it either way.

## Benchmarks measure a debug-instrumented terminus

Cargo unifies the features of a package's dev-dependencies into that package's
own library whenever dev targets are in the build. `seldon` dev-depends on
`terminus` with `layout-test-utils` for its bench harness and `terminus` depends on
`seldon`, so the cycle turns the feature on for both libraries in any build
with bench targets:

```sh
cargo build --unit-graph -Z unstable-options --workspace            # terminus: []
cargo build --unit-graph -Z unstable-options --workspace --benches  # terminus: [layout-test-utils]
```

The second line is what `cargo codspeed build` and `cargo llvm-cov` resolve.
The cost is one `test_leaf_metrics()` probe per leaf in
`crates/terminus/src/layout/host.rs` that a release build does not contain, so the
CodSpeed numbers — the authority for this repo, since single-run local walltime
is noise — describe a `terminus` one branch away from the shipped one. On the
`seldon` side the feature only adds the
`compute_leaf_layout_with_measurement_for_testing` wrapper and costs nothing.

**This is accepted, not fixed.** Breaking the cycle means moving seldon's
terminus-based benches into `terminus`, renumbering every CodSpeed benchmark id and
throwing away its history — a worse trade than one predictable branch.
`bobcat-core`'s dev-dependency on `terminus` with `painter-test-utils` (for its
painter tests) unifies that feature into `terminus` the same way; its cost is
`FrameClock`'s pinned reading and branch in `now_seconds`, the test-only
`Output::None` variant's match arms, and the doc-hidden test accessors in
`crates/terminus/src/painter/mod.rs`, and it is accepted for the same reason.
`.github/scripts/check-bench-feature-parity.py` runs in CI and holds the line:
it diffs the two resolutions, prints the three recorded deviations, and fails on
a new one appearing or on a recorded one silently going away. Any new entry needs
a written reason for the same cost the existing ones state.

## Benchmarks are only the `benches/` targets

Every library, and bobcat-cli's two binaries, sets `bench = false`. Cargo's
`--benches` selection, which `cargo codspeed build` runs underneath, otherwise
also compiles each of them as a bench harness of its own: a second, test-mode
build of the crate that holds no benchmark, since every benchmark is a
`harness = false` target under `benches/`. In CI's bench-profile build (thin
LTO, one codegen unit per crate) those harness builds were about a third of
the compile time. `cargo test` is unaffected: `test` stays on, and the
`[[bench]]` targets are what `cargo bench` and CodSpeed still build and run.

## Restricted-environment troubleshooting

Some agent runners restrict GPU interfaces, Git metadata, or network access.
Treat that as a hypothesis to test, not the default explanation for a failure.

- **No GPU adapter** on a host expected to expose one: retry the exact command
  outside the restricted environment or with a narrowly scoped sandbox
  escalation. A successful retry identifies an environment limitation; if it
  still fails, keep diagnosing the renderer, driver, and adapter selection.
- **A Git operation needed to prepare or publish a PR** (branch creation,
  staging, committing, pushing) failing with a permission, network, or
  authentication-like error: check whether the worktree's Git metadata or
  required network access sits outside the sandbox, retry only the failing
  operation with narrowly scoped escalation, and otherwise diagnose the
  repository, credentials, or network itself.
- **`--target wasm32-unknown-unknown` failing in `quickjs-rust-bridge`'s build
  script** with
  `No available targets are compatible with triple "wasm32-unknown-unknown"` is
  toolchain *selection*, not a missing capability: Apple's clang has no wasm32
  target, and the build script invokes whatever `CC` names. Point it at the
  same LLVM the CI jobs install, with no effect on host builds:

  ```sh
  export CC="$(brew --prefix llvm@22)/bin/clang"
  export CXX="$(brew --prefix llvm@22)/bin/clang++"
  ```

  Reach for this before reporting the Wasm target as unbuildable:
  `crates/bobcat-wasm/src/browser.rs` is `#[cfg(target_arch = "wasm32")]`, so
  `cargo check --workspace --all-targets`, `cargo clippy` and the test suite
  never type-check it and anything touching the browser embedder — or any
  `#[cfg]`-gated import it depends on — is unverified until that target
  builds. CI's `browser` job lints it, so the gap is no longer silent:

  ```sh
  cargo clippy --target wasm32-unknown-unknown --lib \
    -p vant -p bobcat-wasm -p bobcat-resources -p bobcat-core -p terminus \
    -p seldon -p bobcat-source -p quickjs-rust-bridge -- -D warnings
  ```

  `--lib`, not `--all-targets`. `bobcat-core`'s own tokio dependency builds for
  wasm32 because its feature set is target-gated: `rt`, `sync` and `macros`
  everywhere, and `time` only under `cfg(not(target_arch = "wasm32"))`, since
  tokio's timer reads `std::time::Instant`, which panics there — that target
  gets `src/alarm.rs` instead. Its *dev* dependency is the problem: it asks for
  `rt-multi-thread`, which refuses to compile for wasm32 at all, and feature
  unification drags it into anything that builds dev targets. The packages are
  named rather than `--workspace` because `bobcat-cli` is a native binary. The
  two `-Ctarget-feature` warnings `.cargo/config.toml` produces on every crate
  are rustc codegen warnings rather than lints, so `-D warnings` leaves them
  alone.

## The Element PAPI and TypeScript suites

The Element PAPI runtime has two suites over the same source:
`pnpm --filter bobcat-element test` (Rstest, over a recording native mock) and
`pnpm --filter bobcat-element test:type` (TypeScript 7, `tsc -b`), while
`crates/bobcat-core/tests/main_thread.rs` drives the same module, as TypeScript
7 emitted it into Cargo's `OUT_DIR`, through the real QuickJS realm, its native
`bobcat-internal:host` module, and the collector — the realm has no
`globalThis.bobcat`, and a main-thread test asserts its absence. The type suite
checks every runtime module, the colocated `main-thread-runtime.ts` included,
whose behavior is covered by the core main-thread tests. After changing a
source, Cargo regenerates the JS before embedding it. Commit the TypeScript
source only; `dist/` and Cargo's output are generated artifacts and must stay
out of version control.

`pnpm test:type` type-checks every TypeScript program in the workspace with
TypeScript 7.0.2 — `tsc -b` over the root `tsconfig.json`, each program
extending the strict options in `tsconfig.base.json` — except the bobcat-wasm
Workers, which are typed against the glue a `wasm-pack` build generates and are
checked by `pnpm --filter bobcat-wasm build` once it exists. Node runs the
workspace's `.ts` scripts directly by type stripping. Rsbuild loads the
ReactLynx configurations directly; examples use the workspace TypeScript version
without a separate config-loader compiler.

## Screenshot tests

**Screenshot tests** live in `crates/*/tests/screenshots.rs` — plus per-topic
siblings (`terminus` also has `text_screenshots.rs`, `web_text_screenshots.rs`,
`blur_screenshots.rs` and `css_atlas.rs`) — with
committed goldens in `crates/*/tests/screenshots/`, driven by
`crates/prime-radiant`. The ordinary suites share one capture harness in
`tests/support/screenshot.rs`; the browser-referenced CSS atlas owns the
separate workflow below. The golden store is per *crate*, so every screenshot
binary in a crate writes into the same tree. They require a GPU adapter;
without one the test run fails, including in CI, so a green run always means
the pixels were rendered and compared. To accept a new rendering in the
ordinary suites, look at the image first, then (dropping `--test` to catch
every ordinary screenshot binary in the crate):

```sh
PRIME_RADIANT_UPDATE_SNAPSHOTS=1 cargo test -p <crate>
```

A golden that does not exist yet is written *and fails its run* — review it and
re-run. Failures write `-expected`/`-actual`/`-diff` PNGs to the git-ignored
`crates/<crate>/tests/artifacts/`; the panic message names all three plus the
exact differing-pixel count. Never accept a golden you have not looked at: a
blank or all-white image compares happily against itself forever. Browser-owned
suites can reject `PRIME_RADIANT_UPDATE_SNAPSHOTS`; follow their checked capture
and audit workflow instead. The CSS paint atlas has two explicit reference
owners: 665 Chromium matches remain browser-owned, while 146 W3C-correct
differences (85 rasterization/sampling cases plus 61 standards-permitted UA
choices) use native DOM/Parley snapshots in a separate directory. Native atlas
references may be updated only with the filtered
`CSS_PAINT_UPDATE_NATIVE=1 ... css_native_` workflow, which cannot overwrite
browser references; the other 189 cases remain ignored. The browser stage uses
`isolation: isolate` to match the native document element's stacking-context
role, so all 22 negative-z probes are Chromium-owned exact matches. The CSS
paint matrix records the exact capture, update, and full-browser-audit workflow
in `docs/css-paint-screenshot-matrix.md`.

## CI shards

CI splits its two longest jobs across runners, and
`.github/scripts/ci-shards.py` is the one place the split is written down.

- **Tests and coverage** run as two macOS jobs (`test`). The `engine` shard
  tests `terminus`, `seldon`, `prime-radiant` and `vant`; the `runtime` shard
  tests every other workspace member, so a new crate is tested there without
  being listed anywhere. Both set Cargo's unstable
  `resolver.feature-unification = "workspace"` (through
  `CARGO_UNSTABLE_FEATURE_UNIFICATION` and
  `CARGO_RESOLVER_FEATURE_UNIFICATION`), which resolves features over the whole
  workspace whichever packages a command selects. Without it the `engine` shard
  would compile `terminus` without `painter-test-utils`, the feature
  `bobcat-core`'s dev-dependency turns on in a whole-workspace build. The
  `runtime` shard keeps the `engine` crates in its coverage report, because
  `bobcat-core`'s tests execute their code, and Codecov merges the two uploads.
  Locally, a plain `cargo test -p terminus` resolves features for `terminus`
  alone and builds it without `layout-test-utils` and `painter-test-utils`; set
  the two variables to build what CI builds.
- **Memory benchmarks** run as three Ubuntu jobs. `ci-shards.py benches`
  assigns each package's bench targets to a shard from `cargo metadata`, largest
  package first, so a new bench target lands in a shard without being listed,
  and each job passes its targets as `--bench` flags. The package selection
  stays the whole workspace, so every unit is built with the features it has in
  an unsharded `cargo codspeed build`. CodSpeed merges the jobs of one workflow
  run into one report and requires that each benchmark run exactly once.
- `ci-shards.py check` runs in the `checks` job and compares Cargo's unit
  graphs: every unit a shard builds must exist with the same features in the
  unsharded build, and the shards together must cover every test and bench
  target exactly once.

Walltime benchmarks stay one job on the self-hosted runner. Formatting, clippy,
the stylo fork's Lynx tests and doctests run in the `checks` job, which builds
without coverage instrumentation.
