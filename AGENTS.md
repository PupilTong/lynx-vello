# lynx-vello — Agent Guide

This is the canonical project/architecture doc for coding agents working in this
repo (Claude Code and Codex both start here — `CLAUDE.md` is a short pointer to
this file plus Claude-specific notes).

## Mission

lynx-vello is a from-scratch Rust reimplementation of the LynxJS **web-bundle**
runtime — the same runtime [`lynx-stack`](https://github.com/lynx-family/lynx-stack)'s
`web-core` package implements today inside a browser (a dual-thread JS runtime +
DOM + CSS engine). We replace that browser-hosted implementation with a native,
cross-platform engine built on:

- **[stylo](https://github.com/servo/stylo)** — CSS parsing/cascade/computed-style engine (Servo's)
- **[vello](https://github.com/linebender/vello)** — GPU vector rendering
- **[parley](https://github.com/linebender/parley)** — text layout & shaping

The from-scratch layout engine (successor to the C++ engine's `starlight`) is
`crates/neutron-star` — its host protocol, shared layout machinery, and CSS
flexbox, Grid, and Starlight `display: relative` and `display: linear`
algorithms are implemented as first-class peers. Its concrete document/stylo
host lives in `crates/w3c-dom`'s `layout` module
(`Document::layout`, results on each `Node`); the Lynx-widget
policy layer remains pending, while W3C text nodes already use the concrete
Parley path. See
`docs/layout-architecture.md` for its design and
`docs/tracking/css-layout.md` for the behavior it must cover.

**Compatibility target**: ReactLynx apps compiled to `.web.bundle` must render
and behave the same as they do under `web-core` today. "Behave the same" means
matching rendering output and user-interaction behavior — **not** pixel-perfect
fidelity, and **not** reimplementing Android/iOS native platform code paths.
This project does not touch the native `.lynx.bundle` format or platform
bridges (`docs/lynx-binary-template.md` is kept for reference only, not a
target).

## Standards policy

Every CSS/DOM/JS feature Lynx supports falls into exactly one of two buckets
— classify a feature before implementing it, by what Lynx's implementation
*is*, not by what its name resembles:

1. **Lynx supports a real W3C/CSS/DOM feature.** The feature exists in the
   relevant spec, even if Lynx's own implementation of it is buggy,
   incomplete, or non-conformant. Implement the **W3C-correct behavior**
   for it, not Lynx's quirk. Confirmed examples:
   - `z-index`/stacking context — Lynx reparents same-`z-index` elements
     once to the nearest "stacking context node" and sorts by raw integer
     value, instead of running the real recursive, per-stacking-context
     CSS algorithm. Implement the real CSS algorithm instead.
   - `position: fixed` — in every mode Lynx supports (the legacy path and
     both newer `enable-fixed-new`/`enable-unify-fixed-behavior` paths), a
     fixed element's containing block is always the single page-root
     element (`ElementManager::root()`, sized to the viewport), reached
     either by literally reparenting the element under the root in the
     render tree (legacy: `FiberElement::InsertFixedElement`,
     `fiber_element.cc:5037-5096`) or via a dedicated root pointer plus a
     root-only measurement pass (`LayoutObject::GetRoot()`,
     `LayoutAlgorithm::InitializeFixedNode`, `layout_algorithm.cc:102-130`).
     Scroll offset from *every* scrollable ancestor is excluded not by
     per-ancestor coordinate math but structurally: the fixed element's
     native view is simply never mounted inside any scrollable ancestor's
     view hierarchy (`ElementContainer::InsertElementContainerAccordingToElement`,
     `element_container.cc:321-327`). There is **no exception anywhere for
     ancestors with `transform`/`filter`/`perspective`/`will-change`/`contain`**
     (confirmed absent — no `transform` reference exists anywhere in
     `core/renderer/starlight/layout/`, and Lynx has no `contain` property
     at all) — properties that, per the real CSS spec, establish a *new*
     containing block for fixed descendants instead of the viewport. Nor is
     there any component-boundary-scoped containing block: fixed is always
     page-root-relative regardless of `<component>` nesting depth.
     **Implement the real W3C algorithm**: viewport-equivalent containing
     block by default, re-anchored to the nearest ancestor with a
     qualifying transform/filter/perspective/will-change/contain when one
     exists — not Lynx's unconditional escape-to-root behavior.
2. **Lynx supports a Lynx-only extension with no W3C equivalent** (e.g.
   `display: linear`, `relative-*` positioning, the `rpx`/`ppx` units).
   Implement **Lynx's actual behavior**, faithfully — there's no spec to
   defer to, so match what Lynx does, not what would be more "standard."
   **Do not extend these features**: don't add capability, generalize the
   value grammar, or otherwise "improve" a Lynx-only feature beyond what
   Lynx itself actually does.

**Watch for false friends.** A Lynx feature can share a name with a W3C
feature (`position: fixed`, `filter`, ...) while quietly implementing
different semantics underneath — that belongs in bucket 1, but only once
you've actually confirmed, by reading `lynx/` source, that Lynx claims to
implement that spec feature and that the deviation is real, not assumed. If
you find a case like this and Lynx's behavior is ambiguous, the bucket-1-vs-2
classification itself is unclear, or the decision is consequential — **don't
decide silently. Ask the user** before choosing which behavior to implement.

See `docs/tracking/deviations.md` for the running list of confirmed
divergences found so far.

**Scope exceptions.** A feature can be deliberately deferred or narrowed
relative to the compat target by an explicit, user-confirmed decision — the
styling-system set lives in `docs/style-assumptions.md` (e.g.
`::before`/`::after` omitted in v1: native Lynx has no such feature; only the
web target renders it via browser passthrough). Those decisions override the
default "match web-core" expectation until their recorded revisit milestone;
follow them rather than re-deriving the classification.

## Dependency policy

All crates should track the **latest available versions** — **except `rkyv`,
pinned to `0.7`** (see `[workspace.dependencies]` in the root `Cargo.toml`)
because the `.web.bundle` `StyleInfo` section is a previously-serialized rkyv
0.7 wire format produced by existing `web-core` bundles; we must stay able to
decode those without a forward-compat break. `/Users/akiwah/repos/paws-libs/Paws`'s
`Cargo.toml` (an actively maintained sibling project on `stylo`/`parley`) is a
useful signal for currently-compatible versions of those libraries.

## Crates

- `crates/lynx-template-decoder` — decodes `.web.bundle` (magic `SDRA WROF`):
  manifest, rkyv `StyleInfo`, Lepus/JS code, custom sections. Scope: binary
  template parsing only, no JS runtime, no CSS engine (yet).
- `crates/bobcat-engine` — native runtime integration crate. Its independent
  `resource` module owns the protocol-only, host-injected, object-safe Tokio
  `ResourceFetcher` contract; `script` owns the ShadowRealm-inspired isolated
  `ScriptEngine` protocol; and `view` owns `LynxView<R, E>`, coupling one
  engine instance with one private `lynx-widget` StyleEngine/WidgetTree pair.
  The resource module must not decode images/fonts/templates, upload render
  resources, or own cache/retry policy; its protocol remains independent of
  decoder/widget/style/layout/render layers even though the enclosing engine
  crate composes `lynx-widget` at the view layer. It must remain independent
  of concrete JavaScript engines, including QuickJS.
- `crates/quickjs-rust-bridge` — owner-thread-bound safe Rust wrapper around
  the pinned `vendor/quickjs` submodule. It owns the QuickJS C build and the
  narrow unsafe FFI shim, realm/value lifetime and affinity checks, exact
  ECMAScript string conversion, exception sanitization, and pending-job pump.
  It must remain independent of Bobcat, Lynx widgets, resources, and runtime
  policy.
- `crates/bobcat-quickjs` — narrow integration layer depending on both
  `bobcat-engine` and the otherwise Bobcat-independent `quickjs-rust-bridge`.
  Its public API is limited to an opaque QuickJS-backed `LynxView`, its default
  construction factory, an opaque initialization error, and resource/widget
  host access through that view. Runtime configuration, default constants,
  explicit-config construction, the `bobcat-engine::script` adapter types,
  and all realm/value handles, interrupt controls, and source-evaluation entry
  points remain crate-private implementation details. Lynx host globals and
  the future preloaded module graph belong here rather than in the generic
  QuickJS bridge or engine-neutral protocol.
- `crates/w3c-dom` — generic W3C-DOM-subset document tree and
  standards-oriented CSS computation core. Owns one fixed-address boxed arena
  set of four `Slab`s: a primary `Slab<Node<T>>` (slot zero is the real DOM
  Document node and carries its node-visible style context; later slots are
  element/text nodes), plus NodeId-aligned payload, Stylo
  traversal/invalidation, and layout measurement-cache/out-of-flow slabs. The
  primary slab selects each raw-`usize` ID; every side slab allocates/removes
  in lockstep and asserts it received that same key (the payload slab reserves
  a payload-less sentinel at document slot zero). Node removal drops all four
  entries before the ID can be reused (ONE TREE policy: nodes are created and
  mutated only through `Document` methods). Computed styles and
  durable rounded/unrounded layout results remain in the primary Node arena.
  Every node points directly back to the fixed arena set, and the
  same plain one-word `&Node` implements Stylo's document/node/element traits
  according to its `NodeData` (styling runs in place, no mirror tree),
  inline-style parsing, and a private per-document `StyleEngine` containing
  the `Stylist`, cascade pipeline, device, stylesheet set, and
  `SharedRwLock`. `Document::new` creates that entire context afresh, so
  different documents cannot share stylesheets. The generic `T` payload remains associated with
  each element/text node in the NodeId-aligned payload slab but is opaque and read-only to the DOM
  core; selector-visible state comes only
  from real DOM fields, so payloads cannot synthesize attributes. DOM setters
  own snapshot/restyle scheduling, while stylesheet and device methods on the
  document schedule its root in the same call — embedders cannot
  set/clear dirty state or write computed styles. Mutation APIs follow a let-it-crash contract
  (`debug_assert` + panic on stale handles rather than silent no-ops). A
  flush returns a `FlushSummary` — the per-node `StyleDamage` (repaint /
  stacking / overflow / relayout classes) the flush harvested from stylo's
  `ElementData` and then **cleared** (the fix for stylo's
  never-cleared-damage re-traversal bug). During that same harvest,
  relayout-class damage is consumed immediately into boundary-stopped layout
  cache invalidation, so discarding the summary cannot lose layout work; it
  also owns the
  `effective_containment` fold (`contain` + `content-visibility` → effect
  bits).
  Its `layout` module is the concrete `neutron-star` host:
  `Document::layout` flushes styles then lays out with
  `LayoutNode` implemented **directly on `&Node<T>`** (the same one-word
  handle as the stylo traits — no wrapper, no adapter objects) — style views
  are fetched when the engine asks and borrow the node's Stylo `ElementData`
  guard, lending `ComputedValues` fields straight to the engine with no
  `Arc` refcount bump or translation layer; display dispatch routes
  flex/grid/linear/relative with `display: none` hiding and a leaf
  fallback, text nodes through concrete Parley measurement, and the
  positioned pass implements the W3C `position: fixed`
  containing-block rule via the protocol's scheme override. Replaced leaf
  content reads a closed `NaturalSize` value stored in lazily allocated
  node content; its internal update path automatically invalidates the
  affected cache path. Mutually exclusive literal text, natural size, and
  retained text artifacts reuse the node's single nullable content pointer.
  Durable rounded and unrounded layout results live **on each `Node`** (read
  via `Node::rounded_layout`); measurement cache and
  static-position state live in the document's layout secondary arena behind
  `AtomicRefCell<LayoutData>`. Style-driven relayout is automatic (every style
  flush consumes harvested `StyleDamage` into boundary-stopped invalidation);
  `Document::invalidate_layout` remains the
  embedder API for the mutations styles cannot see (content/child-list changes
  with identical computed styles). The internal natural-size update path
  performs that invalidation itself.
  The document node lazily creates and then owns the shared Parley
  `TextContext`; text nodes lazily retain probe/commit artifacts in that
  same content record and read inherited font/text values from their
  parent. Relayout damage on an element evicts its direct text children's
  measurement caches and retained artifacts because text nodes have no Stylo
  damage record of their own. Parley is unconditional and there is no
  arbitrary payload callback. It must not contain Lynx widget vocabulary or
  Lynx device/unit policy —
  Lynx computed defaults (border-box, `overflow: hidden`, `display: linear`
  on every element, …) stay embedder cascade policy (UA sheet). Relies on
  the vendored stylo fork (`vendor/stylo`, tracking the
  canonical `lynx` branch, tip `7ed1b07ec`): `contain` was already seeded
  in the fork's lynx grammar; fork PR #9 (squash-merged into `lynx`) added
  `content-visibility` / `contain-intrinsic-size` under the `lynx` feature,
  pref-gated for stock servo builds.
- `crates/lynx-widget` — Lynx Element-PAPI and style adapter over `w3c-dom`.
  `WidgetTree` instantiates `Document<WidgetState>` and validates the document
  boundary: untrusted PAPI input becomes `WidgetError`s before it reaches the
  crash-on-misuse DOM core. `WidgetState` remains the opaque payload on each
  `Node<WidgetState>`; widget identity and events live there, and the event
  list owns its own interior synchronization rather than mutating w3c-dom
  tree/style state. CSS scope and dataset values are real DOM attributes. The
  crate also owns the `WidgetHandle` ownership layer — the
  PAPI traffics exclusively in canonical, context-owned handles; a live
  handle retains its node, and detached subtrees are reclaimed automatically
  once their last handle drops (the native stand-in for the browser GC; no
  public disposal API). Also owns Lynx view metrics, touch-first device
  policy, and the viewport-relative `rpx` integration. Replaced-content
  natural sizing remains below this layer and is not an Element-PAPI method.
  Standard CSS parsing,
  matching, cascade, and lock ownership remain in `w3c-dom`.
- `crates/neutron-star` — the Flexbox, Grid, and
  Starlight Relative and Linear engine: trait-based host⇄engine integration
  with static dispatch only (no `dyn`), a stylo-style `LayoutNode: Copy`
  node-handle protocol (immutable topology/styles for the flush; per-node
  layout/cache slots are host-owned interior-mutable state written through
  the handle), style traits that speak the stylo fork's computed-value
  vocabulary directly (requires the `stylo` workspace dep + python3 for its
  build script; the old zero-dependency/standalone pillar is retired), and
  host-side display dispatch. Leaf content is deliberately closed: replaced
  content uses the `NaturalSize` value path, while text uses the crate's
  concrete Parley `TextMeasurer::compute_layout` path; arbitrary host
  measurers are not supported. **Flexbox, Grid, Relative, and Linear
  implemented** —
  the shared root/leaf/cache/positioned/rounding machinery, CSS Flexbox Level
  1, numeric CSS Grid Level 2 (excluding subgrid/named areas), id-constrained
  Starlight Relative Layout Level 1, and Lynx's `display: linear` algorithm
  and `linear-*` style/source protocol are live. Text shaping, line breaking,
  intrinsic/height-for-width measurement, baselines, and retained Parley
  layouts are unconditional crate behavior.
  **CSS containment (css-contain-2)** is landed layout-side: the stylo
  `Contain`/`ContainIntrinsicSize` containment accessors on `CoreStyle`,
  size-substitution + layout-containment baseline suppression,
  `compute_skipped_contents_layout`, and the `invalidate` module
  (`is_relayout_boundary`, `invalidate_for_relayout`) — the
  containment-bounded, damage-driven cache-invalidation host workflow
  (single-axis / container queries out of scope). Read
  `docs/layout-architecture.md` before touching it. It must not depend on
  other workspace crates or own host tree/style storage, DOM/widget types,
  resolved device-unit policy, or paint order.
- Remaining runtime-layout integration — the `LayoutNode` handle, display
  dispatch, fixed/hoisted positioned pass, per-node cache storage, and the
  automatic style-damage→`Document::invalidate_layout` wiring (boundary-stopped,
  engine-internal — not a widget-layer concern) now live in `w3c-dom`
  (see above). Still L3 work: `lynx-widget`-level policy (`rpx`-aware
  view metrics, sticky lowering), component-specific staggered layout, and
  Lynx-specific text attribute/raw-text/truncation policy. Generic W3C text
  style, document context, and artifact storage already live in `w3c-dom`.
- *(planned, not yet scaffolded)* render / runtime crates — see
  `docs/tracking/` for the behavior surface each will need to cover before
  scaffolding begins, and `.claude/agents/` for the subsystem-scoped agent
  personas already set up for this work.

See `docs/style-architecture.md` for the current style-layer dependency and
ownership rules, and `docs/layout-architecture.md` for the layout-layer
equivalent.

## Reference repos (local checkouts, read-only — do not edit)

- `/Users/akiwah/repos/lynx` — the original LynxJS engine (C++). Ground truth
  for CSS/DOM/event/animation *semantics*. We do not reimplement its
  Android/iOS/native-bundle platform code.
- `/Users/akiwah/repos/lynx-stack` — TS/Rust monorepo: `packages/react/*`
  (ReactLynx framework) and `packages/web-platform/*` (`web-core` dual-thread
  runtime, `web-elements` built-in components). This is the architectural
  reference for the dual-thread execution model lynx-vello must replicate
  natively (no literal worker/iframe threads).
- `/Users/akiwah/repos/paws-libs/Paws` — a sibling native Rust UI engine
  (`stylo` + Taffy + `parley`, WASM-driven, UIKit/wgpu-painted). **Not** a
  Lynx project and **not** a behavior spec — it's an implementation-pattern
  reference for DOM system and CSS system design: how to wire `stylo`'s
  cascade/`RuleTree` onto a custom arena-based DOM (`engine/src/dom/`,
  `engine/src/style.rs`, `engine/src/style/css_style_sheet.rs`), a real
  spec-conformant CSS stacking-context implementation
  (`engine/src/layout/stacking.rs` — relevant to the z-index deviation
  above), and DOM-style event dispatch/hit-testing with no browser
  underneath (`engine/src/events/`, `engine/src/hit_test/`). Its
  `paws-style-ir/` crate is a second, independent rkyv-based style-IR design
  worth comparing against our own `RawStyleInfo` (it targets rkyv `0.8.x`;
  ours stays pinned at `0.7`, see Dependency policy above).

Elsewhere in this repo (subagent personas, tracking docs, prompts), these
three are referred to by shorthand as `lynx/`, `lynx-stack/`, and `Paws/` —
this section is the only place the absolute paths are spelled out.

## Reference knowledge

- `docs/web-binary-template.md` — **read this before touching
  `crates/lynx-template-decoder` or any StyleInfo/wire-format code.** The
  web-target bundle format this repo decodes today: container layout,
  section encodings, and the rkyv 0.7 `RawStyleInfo` CSS data model (mirrored
  1:1 in the decoder crate — field/variant order there is wire format, do not
  reorder).
- `docs/lynx-binary-template.md` — the *native* `.lynx.bundle` format ("lynx"
  target), reference only, not implemented here.
- `docs/tracking/` — the behavior/feature inventory (CSS properties, layout
  algorithms, DOM/event model, JS runtime APIs, `web-core` runtime
  architecture, built-in components, ReactLynx surface) that future
  implementation work is scoped against. **Read the relevant file before
  implementing any new subsystem.** Start at `docs/tracking/README.md`.
- `docs/agent-prompts.md` — copy-pasteable task-kickoff prompts for recurring
  work (adding a CSS property, porting a built-in component, auditing a JS API
  for parity, etc.), usable from either Claude Code or Codex.

## Toolchain

- Nightly Rust (`rust-toolchain.toml`), edition 2024, resolver 3, workspace lints.
- `cargo fmt` (nightly rustfmt options in `rustfmt.toml`), `cargo clippy`,
  `cargo test`, `cargo bench` (CodSpeed-compatible `divan` benches).

## Testing

Integration tests decode real fixtures vendored from lynx-stack under
`crates/lynx-template-decoder/tests/fixtures/` (Apache-2.0 build artifacts).
`cargo test` must pass on the pinned nightly toolchain.

## Working with Codex

This repo is worked on by both Claude Code (reads `CLAUDE.md`, which points
here) and Codex (reads this file directly). Division of labor between them is
**not yet decided** beyond Codex's existing rescue / second-opinion / review
role (`codex:codex-rescue`, `/codex:review`) — don't assume Codex owns any
particular crate or subsystem unless a task explicitly says so.
