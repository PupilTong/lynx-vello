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
algorithms are implemented as first-class peers. The concrete adapter from
`stylo-dom` topology/computed styles into neutron-star remains pending. See
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

## Runtime ownership and style-flush model

The following are settled architecture constraints, not temporary
implementation limitations:

1. **A VM and its Widget runtime always execute on the same owner thread.**
   JavaScript host calls do not access that view's Widget state from another
   thread.
2. **A Widget runtime and its `stylo_dom::Document` always execute on that
   same owner thread.** Ownership may move between threads only while the
   entire view is stopped and no handles or callbacks can observe it; there is
   never concurrent host access to one Widget/Document pair.
3. **A stylo flush is synchronous, non-reentrant, and uninterruptible from the
   host's point of view.** From entry through traversal teardown and dirty-state
   cleanup, no JavaScript callback, event delivery, resource completion,
   layout/render callback, or other source may read or mutate the Widget tree
   or its Document. The only concurrency inside this interval is stylo's own
   scoped parallel traversal; its workers receive an immutable tree/topology,
   join before `flush` returns, and cannot publish DOM mutations.
4. **`ElementId` is an embedding-layer implementation detail.** It may cross
   the `stylo-dom → lynx-widget` crate boundary, but must never be exposed to a
   VM, application/user code, callback payload, or delayed task. The public
   Widget/runtime identity is `Rc<NodeHandle>`; its internal arena id and
   per-tree identity are private.
5. **Strong external node ownership is explicit.** VM wrappers, application
   references, and delayed work that must keep a node alive hold
   `Rc<NodeHandle>`. Delayed work that intentionally must not retain a node
   holds `Weak<NodeHandle>` and handles upgrade failure as normal control flow.
   A bare integer/id is never an acceptable delayed node reference.
6. **DOM removal is detachment, not destruction.** A detached node remains in
   its original `Document` and is reattachable. There is no arbitrary public
   `destroy`/`drop_subtree` API at the Widget/VM boundary. Physical slot
   reclamation is owned by the Widget handle collector.
7. **A slot may be reclaimed only when both reachability roots are gone:** the
   node is not connected to the Document's live page tree, and no node in its
   detached subtree has an external strong handle. Reclamation is subtree-
   atomic: a strong handle to any descendant retains its detached ancestors
   and siblings in that subtree. Destroying/reclaiming a parent must never
   physically destroy a descendant that is still externally held.
8. **Each live Widget node has one canonical registry `Rc<NodeHandle>`.** A
   strong count above that registry owner means external retention. Removing
   the registry owner is part of physical slot reclamation and causes all weak
   handles to expire before that slot can represent another node. Handles are
   also scoped by an allocation identity for their owning `WidgetTree`, so a
   handle from one view is rejected by another even when their slot indices
   coincide.
9. **Lifecycle violations must be loud in test/debug builds.** Validate the
   one-handle-per-live-node registry, bidirectional parent/child topology,
   same-tree handle identity, and `strong_count == 1` immediately before
   reclaiming each node. `stylo-dom` stores nodes in a `Slab` and carries a
   debug-only, document-wide allocation epoch in its internal ids so a leaked
   raw id or uncleared delayed id fails instead of silently aliasing a reused
   slot; release builds rely on the ownership invariants above and do not pay
   for that epoch.

Design APIs around direct ownership and ordinary `&` / `&mut` borrows under
these constraints. The owner-thread `Rc<NodeHandle>` registry above is the
deliberate external-node-lifetime mechanism; it does not wrap the `Document`
or make it concurrently accessible. Do not introduce additional `Rc`/`Arc`,
`RefCell`/`AtomicRefCell`, or `Mutex`/`RwLock` merely to support hypothetical
cross-thread VM, Widget, or Document access. This does not remove
synchronization required *inside* stylo's parallel traversal, stylo's shared
CSS data model, genuinely concurrent resource services, or explicit
cross-thread interruption handles.
If a feature would require weakening one of these constraints, stop and ask
the user rather than silently broadening the ownership model.

## Crates

- `crates/lynx-template-decoder` — decodes `.web.bundle` (magic `SDRA WROF`):
  manifest, rkyv `StyleInfo`, Lepus/JS code, custom sections. Scope: binary
  template parsing only, no JS runtime, no CSS engine (yet).
- `crates/bobcat-engine` — native runtime integration crate. Its independent
  `resource` module owns the protocol-only, host-injected, object-safe Tokio
  `ResourceFetcher` contract; `script` owns the ShadowRealm-inspired isolated
  `ScriptEngine` protocol; and `view` owns `LynxView<R, E>`, coupling one
  engine instance with one private `lynx-widget` `WidgetTree`. The widget
  facade may forward DOM mutations and host inputs to its private
  `stylo_dom::Document`, but CSS computation is owned exclusively by that
  document; composition inside `LynxView` does not make `lynx-widget` a style
  engine.
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
- `crates/stylo-dom` — generic HTML-DOM subset and standards-oriented CSS
  computation core, and the **only workspace layer that owns CSS
  computation**. Owns `Node<T>` / `Document<T>`, address-stable document
  back-pointers, stylo DOM trait impls directly on `&Node<T>`, tree
  invalidation, inline-style parsing, and each document's private `Stylist`,
  `Device`, stylesheet origins/default sheets, matching, cascade, computed
  values, traversal, and `SharedRwLock`. It must not contain Lynx widget/PAPI
  vocabulary. Lynx CSS grammar belongs in the maintained Stylo fork; host
  metrics may be passed into `Document`, but their CSS interpretation remains
  here.
- `crates/lynx-widget` — Lynx Element-PAPI facade over the generic DOM.
  Owns `WidgetState` / `WidgetTree`, opaque external node handles, PAPI
  validation, and host-provided view/configuration data. It may translate
  bundle/runtime inputs and call `stylo_dom::Document` APIs, but it does not
  own or implement CSS parsing semantics, selector matching, cascade,
  invalidation/traversal, computed-style calculation, UA/default-style
  semantics, or style-to-layout adaptation. Do not change this crate to
  implement a CSS-engine task, and do not describe it as a "style adapter":
  forwarding inputs or exposing facade methods does not place it in the CSS
  computation pipeline.
- `crates/neutron-star` — the standalone-publishable Flexbox, Grid, and
  Starlight Relative and Linear engine: trait-based host⇄engine integration
  with static dispatch only (no `dyn`), an immutable topology/style source
  physically separated from mutable layout/cache/measurement sessions, and
  host-side
  display dispatch. Leaf content engines integrate through the generic
  lending `LeafMeasurer` protocol. **Flexbox, Grid, Relative, and Linear
  implemented** —
  the shared root/leaf/cache/positioned/rounding machinery, CSS Flexbox Level
  1, numeric CSS Grid Level 2 (excluding subgrid/named areas), id-constrained
  Starlight Relative Layout Level 1, and Lynx's `display: linear` algorithm
  and `linear-*` style/source protocol are live. Text shaping, line breaking,
  intrinsic/height-for-width measurement, baselines, and retained Parley
  layouts live behind the default-on `text` feature; the protocol and
  box-layout core stay dependency-free with `default-features = false`.
  Read
  `docs/layout-architecture.md` before touching it. It must not depend on
  other workspace crates or own host tree/style storage, DOM/widget types,
  resolved device-unit policy, or paint order.
- Future runtime-layout integration — the concrete `stylo-dom`
  computed-style/topology source and neutron-star session adapter, display
  dispatch and dirty→cache invalidation
  wiring, root fixed-position pass, component-specific staggered layout, and
  text-style translation and text-session wiring remain L3 work. No separate
  crate for this layer has been established yet.
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
