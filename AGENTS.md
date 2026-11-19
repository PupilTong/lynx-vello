# lynx-vello — Agent Guide

This is the canonical project doc for coding agents working in this repo.
Claude Code and Codex both start here: `CLAUDE.md` is a short pointer to this
file plus Claude-specific notes, `.codex/agents/` mirrors `.claude/agents/`,
and `.agents/skills/` mirrors `.claude/skills/`.

**How to read the guide.** This file is the part every session needs: Mission,
Standards policy and Dependency policy set what to build; the Workspace map
names each crate and pnpm package and links the file that describes it;
Reference repos, Reference knowledge, Toolchain and Testing say where to look
things up and how to run them. What exists inside a crate is not in this file.
Before reading or changing code under a path in the Workspace map, read the
file its Guide column links, and only that one: a task in `crates/seldon` does
not need `crates/bobcat-core`'s. `crates/bobcat-core` is split once more, by
topic; its `AGENTS.md` is the index, and a task reads the topics it touches.

Format Rust with `cargo fmt -p <crate>` for the crates you touched — never `cargo fmt --all`,
which reaches into `vendor/stylo` — then run `./.github/scripts/fmt-check.sh`,
which is CI's check and only verifies formatting. Run
`pnpm install --frozen-lockfile` and
`pnpm --filter reactlynx-test-fixtures build` before any Rust test, clippy or
bench run.

## Pull-request descriptions

Every pull-request body must include one or more GitHub-rendered Mermaid
diagrams. Taken together, the diagrams must:

- show the relevant architecture, ownership, control flow, or data flow before
  the change;
- show the corresponding structure or flow after the change; and
- visually mark the nodes, edges, boundaries, invariants, or risks that need
  the reviewer's closest attention, with the accompanying text explaining why
  each marked area matters.

A prose-only before/after description does not satisfy this requirement. There
are no exemptions for small, documentation-only, test-only, dependency, or
other non-architectural changes: in those cases, diagram the affected authoring,
build, test, release, or runtime path and explicitly label the architectural
parts that remain unchanged. Tailor every diagram to the actual PR and remove
all template placeholders. GitHub rendering checks are optional and may be
done after publication; unavailable preview must not block creating or updating
a PR or require user approval to proceed. Use
`.github/pull_request_template.md` as the minimum required structure.

## Mission

lynx-vello is a from-scratch Rust reimplementation of the LynxJS **web-bundle**
runtime — the same runtime [`lynx-stack`](https://github.com/lynx-family/lynx-stack)'s
`web-core` package implements today inside a browser (a dual-thread JS runtime +
DOM + CSS engine). We replace that browser-hosted implementation with a native,
cross-platform engine built on:

- **[stylo](https://github.com/servo/stylo)** — CSS parsing/cascade/computed-style engine (Servo's)
- **[vello_gpu](https://github.com/linebender/vello)** (0.3) — the rasterizer, from the
  linebender Vello project: sparse strips generated on the CPU and composited on wgpu 30.
  The paint walk records terminus's own display list, which each render replays into a
  `vello_gpu::Scene`.
- **[parley](https://github.com/linebender/parley)** (0.13) — text layout & shaping, including
  the CSS 2.2 §10.8.1 line boxes both paragraph kinds report

The from-scratch layout engine (successor to the C++ engine's `starlight`) is
`crates/seldon` — its host protocol, shared layout machinery, and CSS
flexbox, Grid, CSS Grid Level 3 `display: grid-lanes`, and Starlight
`display: relative` and `display: linear`
algorithms are implemented as first-class peers. Its concrete document/stylo
host lives in `crates/terminus`'s `layout` module
(`Document::layout`, results queried by `NodeId` from the document); the Lynx-specific runtime
policy layer remains pending, while W3C text nodes already use the concrete
Parley path. See
`docs/layout-architecture.md` for its design and
`docs/tracking/css-layout.md` for the behavior it must cover.

**Compatibility target**: ReactLynx apps compiled to `.web.bundle` must render
and behave the same as they do under `web-core` today. "Behave the same" means
matching rendering output and user-interaction behavior — **not** pixel-perfect
fidelity, and **not** reimplementing Android/iOS native platform code paths.
Source-based native external `.lynx.bundle` parsing and conversion are
supported by `bobcat-source`; native bytecode execution and platform bridges
remain outside the runtime target.

## Standards policy

Every CSS/DOM/JS feature Lynx supports falls into exactly one of two buckets —
classify a feature before implementing it, by what Lynx's implementation *is*,
not by what its name resembles:

1. **Lynx supports a real W3C/CSS/DOM feature**, even where Lynx's own
   implementation is buggy, incomplete, or non-conformant. Implement the
   **W3C-correct behavior**, not Lynx's quirk. Confirmed examples:
   - `z-index`/stacking context — Lynx reparents same-`z-index` elements once
     to the nearest "stacking context node" and sorts by raw integer value.
     Implement the real recursive, per-stacking-context CSS algorithm instead.
   - `position: fixed` — in every mode Lynx supports (legacy,
     `enable-fixed-new`, `enable-unify-fixed-behavior`) the containing block
     is always the viewport-sized page root (`ElementManager::root()`),
     reached by reparenting under it (`FiberElement::InsertFixedElement`,
     `fiber_element.cc:5037-5096`) or by a root pointer plus a root-only
     measurement pass (`LayoutObject::GetRoot()`,
     `LayoutAlgorithm::InitializeFixedNode`, `layout_algorithm.cc:102-130`);
     ancestor scroll offset is excluded structurally, since the fixed
     element's view is never mounted inside a scrollable ancestor's hierarchy
     (`ElementContainer::InsertElementContainerAccordingToElement`,
     `element_container.cc:321-327`). There is **no exception anywhere for
     ancestors with `transform`/`filter`/`perspective`/`will-change`/`contain`**
     (confirmed absent: no `transform` reference in
     `core/renderer/starlight/layout/`, and Lynx has no `contain` property),
     and no component-boundary-scoped containing block whatever the
     `<component>` nesting depth. **Implement the real W3C algorithm**:
     viewport-equivalent containing block by default, re-anchored to the
     nearest qualifying ancestor where one exists.
2. **Lynx supports a Lynx-only extension with no W3C equivalent** (e.g.
   `display: linear`, `relative-*` positioning, the `rpx`/`ppx` units).
   Implement **Lynx's actual behavior**, faithfully. **Do not extend these
   features**: do not add capability, generalize the value grammar, or
   otherwise "improve" a Lynx-only feature beyond what Lynx itself does.

**Watch for false friends.** A Lynx feature can share a name with a W3C feature
(`position: fixed`, `filter`, ...) while implementing different semantics
underneath — bucket 1, but only once you have confirmed from `lynx/` source
that Lynx claims that spec feature and the deviation is real. Where Lynx's
behavior is ambiguous, the bucket-1-vs-2 classification is unclear, or the
decision is consequential, **do not decide silently. Ask the user.** See
`docs/tracking/deviations.md` for the confirmed divergences found so far.

**Scope exceptions.** A feature can be deliberately deferred or narrowed
relative to the compat target by an explicit, user-confirmed decision — the
styling-system set lives in `docs/style-assumptions.md` (e.g. element text
`content` is supported; generated boxes and `::before`/`::after` remain
deferred despite browser passthrough on the web target). Those decisions
override the default "match web-core" expectation until their recorded revisit
milestone; follow them rather than re-deriving the classification.

## Dependency policy

All crates should track the **latest available versions** — **except `rkyv`,
pinned to `0.7`** (see `[workspace.dependencies]` in the root `Cargo.toml`)
because the `.web.bundle` `StyleInfo` section is a previously-serialized rkyv
0.7 wire format produced by existing `web-core` bundles; we must stay able to
decode those without a forward-compat break. `/Users/akiwah/repos/paws-libs/Paws`'s
`Cargo.toml` (an actively maintained sibling project on `stylo`/`parley`) is a
useful signal for currently-compatible versions of those libraries.

## JavaScript data ownership

Values consumed only by JavaScript stay opaque in Rust. Embedders serialize
host payloads to `String`; the runtime passes that text unchanged and JavaScript
parses it. Do not add Rust JSON models for these payloads or walk Rust
structs/maps to build JSON for a JS call. When exposing facts Rust itself owns,
pass primitive binding arguments and construct the JS object in JavaScript.
Rust parses structured input only when Rust behavior actually needs its fields
(for example, page configuration or styles), not merely to forward it to JS.

## Where a new fact goes

- A fact about one crate or package goes in that crate's `AGENTS.md` — for
  `bobcat-core`, in the topic file under `crates/bobcat-core/docs/` that owns
  it — in the same change as the code.
- This file takes only what every session needs whatever it touches: policy,
  the map, and the toolchain and testing prerequisites. A new crate adds one
  row to the Workspace map and its own `AGENTS.md`.
- Keep this file within 32 KiB, Codex's default limit
  (`project_doc_max_bytes`) on the project documentation it loads at session
  start. A crate's file that grows large is split by topic, as
  `crates/bobcat-core/docs/` is.

## Workspace map

| Path | Role | Guide |
| --- | --- | --- |
| `crates/bobcat-source` | Owner of Lynx source parsing: ZIP, restricted XML, `.web.bundle`, `.lynx.bundle`. | [AGENTS.md](crates/bobcat-source/AGENTS.md) |
| `crates/bobcat-core` | The native runtime core: group, view, painter, QuickJS realms, page policy. | [AGENTS.md](crates/bobcat-core/AGENTS.md) |
| `crates/quickjs-rust-bridge` | Owner-thread-bound safe Rust wrapper around the pinned `vendor/quickjs` submodule. | [AGENTS.md](crates/quickjs-rust-bridge/AGENTS.md) |
| `crates/vant` | Resource policy below `terminus`: cache tiers, RFC 9111 freshness, MIME pipeline, image sizing, the registry of embedder-held contents, the embedder capability traits (`Fetch`, `Store`, `ImageDecoder`, `Platform`). | [AGENTS.md](crates/vant/AGENTS.md) |
| `crates/bobcat-resources` | Reference resource system over `vant`: the load orchestration plus the native adapters (`NativePlatform`). The browser's adapters are `bobcat-wasm`'s. | [AGENTS.md](crates/bobcat-resources/AGENTS.md) |
| `crates/bobcat-cli` | The `bobcat` product (`cli`) and the `bobcat-server` screenshot service (`server`), both embedders. | [AGENTS.md](crates/bobcat-cli/AGENTS.md) |
| `crates/bobcat-wasm` | The pure-Rust `wasm-bindgen` browser embedder and npm facade. | [AGENTS.md](crates/bobcat-wasm/AGENTS.md) |
| `crates/terminus` | Generic W3C-DOM-subset document tree and standards-oriented CSS computation core. | [AGENTS.md](crates/terminus/AGENTS.md) |
| `crates/seldon` | The from-scratch Flexbox, Grid, grid-lanes, and Starlight Relative and Linear layout engine. | [AGENTS.md](crates/seldon/AGENTS.md) |
| `crates/prime-radiant` | Screenshot testing: RGBA image, PNG codec, pixelmatch port, golden store. | [AGENTS.md](crates/prime-radiant/AGENTS.md) |
| `packages/bobcat-element` | Dependency-free TypeScript sources of the ESMs `bobcat-core` preloads into its realms. | [AGENTS.md](packages/bobcat-element/AGENTS.md) |
| `packages/reactlynx-test-fixtures` | ReactLynx source fixtures and bundle generators for Bobcat integration tests. | [packages/AGENTS.md](packages/AGENTS.md) |
| `packages/web-core-e2e-fixtures` | `lynx-stack`'s web-core end-to-end cards, compiled for the targeted engine version. | [packages/AGENTS.md](packages/AGENTS.md) |
| `packages/explorer-homepage` | The Lynx Explorer home screen, written in ReactLynx. | [packages/AGENTS.md](packages/AGENTS.md) |
| `packages/explorer-lib` | Navigation, launch-command, history and theme helpers shared by the Explorer pages. | [packages/AGENTS.md](packages/AGENTS.md) |
| `packages/explorer-showcase` | The Lynx Explorer showcase menu in ReactLynx, and the `@lynx-example` packages. | [packages/AGENTS.md](packages/AGENTS.md) |
| `packages/github-pages` | The rsbuild site on GitHub Pages, over `bobcat-wasm` and the Explorer homepage. | [packages/AGENTS.md](packages/AGENTS.md) |
| `examples/` | `lynx-stack`'s own examples, re-pointed at published package versions. | [packages/AGENTS.md](packages/AGENTS.md) |

What the runtime layer does not implement yet is listed in
`docs/still-ahead.md`.

## Reference repos (local checkouts, read-only — do not edit)

- `/Users/akiwah/repos/lynx` — the original LynxJS engine (C++). Ground truth
  for CSS/DOM/event/animation *semantics*. We do not reimplement its
  Android/iOS/native-bundle platform code.
- `/Users/akiwah/repos/lynx-stack` — TS/Rust monorepo: `packages/react/*`
  (ReactLynx framework) and `packages/web-platform/*` (`web-core` dual-thread
  runtime, `web-elements` built-in components). This is the architectural
  reference for the dual-thread execution model lynx-vello must replicate
  natively — with the engine's own threads (`bobcat-main` and `bobcat-workers`,
  the second hosting the BTS as a W3C `Worker`, and both real Web Workers on
  wasm) rather than with the browser's Worker/iframe hosting.
- `/Users/akiwah/repos/paws-libs/Paws` — a sibling native Rust UI engine
  (`stylo` + Taffy + `parley`, WASM-driven, UIKit/wgpu-painted). **Not** a Lynx
  project and **not** a behavior spec — an implementation-pattern reference for
  DOM and CSS system design: wiring `stylo`'s cascade/`RuleTree` onto a custom
  arena-based DOM (`engine/src/dom/`, `engine/src/style.rs`,
  `engine/src/style/css_style_sheet.rs`), a spec-conformant CSS
  stacking-context implementation (`engine/src/layout/stacking.rs` — relevant
  to the z-index deviation above), and DOM-style event dispatch/hit-testing
  with no browser underneath (`engine/src/events/`, `engine/src/hit_test/`).
  Its `paws-style-ir/` crate is a second, independent rkyv-based style-IR
  design worth comparing against our own `RawStyleInfo` (it targets rkyv
  `0.8.x`; ours stays pinned at `0.7`, see Dependency policy above).

Elsewhere in this repo (subagent personas, tracking docs, prompts), these three
are referred to by shorthand as `lynx/`, `lynx-stack/`, and `Paws/` — this
section is the only place the absolute paths are spelled out.

## Reference knowledge

- `docs/lynx-xml-template.md` — the implementation-derived Lynx XML source
  format: exact restricted grammar, section extraction, errors and offsets,
  fixed template mapping, and the intentional CSS difference between the merged
  XML-to-`.web.bundle` encoder and the still-proposed raw web loader.
  `bobcat-source::xml` implements its source parsing boundary. XML is a source
  front end, not a third bundle encoding.
- `docs/web-binary-template.md` — **read this before touching
  `crates/bobcat-source/src/web` or any StyleInfo/wire-format code.** The
  web-target bundle format this repo decodes today: container layout, section
  encodings, and the rkyv 0.7 `RawStyleInfo` CSS data model (mirrored 1:1 in
  the decoder crate — field/variant order there is wire format, do not
  reorder).
- `docs/lynx-binary-template.md` — the *native* `.lynx.bundle` format ("lynx"
  target), implemented for source-based external bundles by
  `bobcat-source::native`.
- `docs/terminus-architecture.md` — `crates/terminus`'s internals: arenas and identity,
  shadow DOM and custom elements, the style engine and invalidation, the
  layout host, visual order and the committed frame, scroll/input/event
  paths, text, and the vendored stylo fork. `crates/terminus/AGENTS.md`
  keeps only the charter and the rulings.
- `docs/tracking/` — the behavior/feature inventory (CSS properties, layout
  algorithms, DOM/event model, JS runtime APIs, `web-core` runtime
  architecture, built-in components, ReactLynx surface) that future
  implementation work is scoped against. **Read the relevant file before
  implementing any new subsystem.** Start at `docs/tracking/README.md`.
- `docs/agent-prompts.md` — copy-pasteable task-kickoff prompts for recurring
  work (adding a CSS property, porting a built-in component, auditing a JS API
  for parity), usable from either Claude Code or Codex.
- `docs/text-rendering-research.md` — **read before proposing any text-painting
  performance work.** Why vello classic had no glyph atlas, what a text-heavy
  frame cost under it (measured), where the ecosystem's answer lives (`glifo`,
  now in `vello_gpu`, formerly `vello_hybrid`), and why `glyphon` and a
  hand-rolled atlas are both ruled out. Its 2026-11 update records the switch
  to `vello_gpu` with glifo's glyph atlas cache left off: glyphs still draw as
  outlines every frame, so the useful contribution is evidence, not a cache.
- `docs/still-ahead.md` — what the runtime layer does not implement yet, each
  item with the code that would host it.
- `docs/testing.md` — see Testing below.

## Toolchain

- Nightly Rust (`rust-toolchain.toml`), edition 2024, resolver 3, workspace
  lints.
- The pnpm workspace (`packages/*`, `examples/*`, `crates/bobcat-wasm`) is
  TypeScript and ESM throughout, checked by TypeScript 7.0.2 (`pnpm test:type`)
  under the strict options in `tsconfig.base.json`; Node (`^22.18 || ^24`) runs
  its `.ts` scripts directly by type stripping.
- Cargo builds of `bobcat-core` require Node and a prior
  `pnpm install --frozen-lockfile`; the built-in JS runtime is compiled into
  `OUT_DIR` during the build — to JavaScript, and on to QuickJS bytecode by
  a host build of `quickjs-rust-bridge` — including for Wasm targets.
- `cargo clippy`, `cargo test`, `cargo bench` (CodSpeed-compatible `divan`
  benches).
- **Formatting: `cargo fmt -p <crate>` for the crates you touched, then
  `./.github/scripts/fmt-check.sh`**, which is what CI runs — it names the
  members from `cargo metadata` rather than from a list someone has to remember
  to extend, and only runs `cargo fmt --check`, so it reports drift rather than
  fixing it. **Do not run `cargo fmt --all`**: it reaches into `vendor/stylo`
  even though the fork is excluded from the workspace, and the fork carries
  pre-existing upstream rustfmt drift, so it "fixes" files nobody touched. If
  you ever do, check `git -C vendor/stylo status` afterwards and revert
  anything outside your own change, or the next fork commit ships unrelated
  reformatting. Nightly rustfmt options live in `rustfmt.toml`.

### macOS worktree build caches

Install the shared Git hook once with
`python3 .github/scripts/worktree-cow.py install`. On `git worktree add`, it
clones the primary checkout's `target` using APFS copy-on-write. Worktrees keep
independent writable files; subsequent normal checkouts keep their own cache.
The hook uses the script in the primary checkout so it also works for older
branches. It does not install dependencies or compile; follow the normal pnpm
and Cargo prerequisites after checkout.

For existing worktrees, stop builds and rust-analyzer's automatic checks first,
then run `python3 .github/scripts/worktree-cow.py seed --replace <worktree>` or
`python3 .github/scripts/worktree-cow.py seed --all --replace`. This replaces
only existing `target` directories; it leaves sources and Git state intact and
skips targets already seeded. Cloning must succeed before the old cache is
removed. Local/path-package fingerprints and local build-script output are
invalidated; old incremental directories and unrecognized/local dependency
artifacts are omitted. Incremental compilation settings remain unchanged for
subsequent builds. External build scripts rerun, so a subsequent build cannot
mistake the primary checkout's local code for this branch's code. The normal
build rebuilds these packages while reusable external artifacts remain shared.

The primary checkout must have its submodules/dependencies available for
`cargo metadata --offline --locked`, and a regular `target` directory on the
same APFS volume. Custom Cargo target/build directories are not supported.
Missing or busy seeds cause an explanatory hook message without failing Git;
retry with `seed <worktree>` later. `BOBCAT_WORKTREE_COW=0` disables automatic
seeding for one invocation. `--no-checkout` does not run the Git hook; use the
explicit seed command after checkout. Existing custom hooks are never replaced
by the installer. See [worktree cache details](docs/worktree-cow.md).

### Dev profile debug info and target size

The `dev` profile (and `test`, which inherits it) builds workspace crates with
`debug = "line-tables-only"` and every dependency, `vendor/stylo` included,
with `debug = false`. Panic messages and backtraces still name file and line
in workspace code; a debugger shows no local variables anywhere and no source
lines inside dependencies. The reason is disk: on macOS debug info stays in the
object files Cargo keeps beside each artifact, and a clean
`cargo build --workspace --all-targets` measured 26.2 GB with full debug info
against 14.0 GB with these settings.

**When a problem is hard to debug, raise the level for that build rather than
working around the missing information** — stepping through code in `lldb`,
reading locals in a crash, or a backtrace that stops at a dependency frame are
all reasons to. Use a command-line override, so the committed profile stays as
it is:

```sh
# Full debug info for workspace crates
CARGO_PROFILE_DEV_DEBUG=true cargo test -p <crate> <filter>

# ...and for dependencies too (stylo, vello_gpu, parley, ...)
cargo test -p <crate> <filter> \
  --config 'profile.dev.debug=true' \
  --config 'profile.dev.package."*".debug=true'

# One dependency only
cargo test -p <crate> <filter> --config 'profile.dev.package.stylo.debug=true'
```

Changing the level recompiles the affected crates and writes a second set of
artifacts next to the first, so go back to the plain command once the problem
is understood. Do not commit a raised level in `Cargo.toml`.

Cargo never deletes superseded artifacts, and each edit, branch switch or
feature combination adds a new hashed set: a long-lived worktree's `target`
grows without bound, mostly in `debug/deps` and `debug/incremental`. With no
build running in that checkout, remove what has not been used recently with
[`cargo-sweep`](https://github.com/holmgr/cargo-sweep)
(`cargo install cargo-sweep`):

```sh
cargo sweep --time 3          # this checkout: artifacts unused for 3 days
cargo sweep --time 3 -r ..    # every Cargo project below a directory
```

It only deletes build output; the next build recompiles whatever it needed
after all. For a worktree, `worktree-cow.py seed --replace` (above) is the
complete reset.

## Testing

Integration tests and benchmarks build ReactLynx sources from the
`packages/reactlynx-test-fixtures` pnpm workspace. Run
`pnpm --filter reactlynx-test-fixtures build` before compiling Rust tests,
`cargo clippy --all-targets`, or benchmarks. The generated `dist/index.rs`
registry includes the emitted bundles; no compiled fixture is versioned.
Upstream source provenance and licensing are in that package's NOTICE/LICENSE.
`cargo test` must pass on the pinned nightly toolchain.

The rest is in `docs/testing.md`; read the part that matches the work:

- **Screenshot tests** — before running, adding or accepting a golden
  (`PRIME_RADIANT_UPDATE_SNAPSHOTS`, the CSS paint atlas workflow).
- **The Element PAPI and TypeScript suites** — when TypeScript changed.
- **Input robustness at the external-byte boundaries** — before touching
  `bobcat-source`'s parsers.
- **The unsafe floor** — before writing `unsafe`.
- **Benchmarks measure a debug-instrumented terminus** and **Benchmarks are
  only the `benches/` targets** — before changing a bench target, a `bench`
  key in a manifest, or a dev-dependency feature.
- **CI shards** — before adding a crate, a bench target or a CI job.
- **Restricted-environment troubleshooting** — when a GPU adapter, a Git
  operation or the wasm32 target fails inside a sandbox.
