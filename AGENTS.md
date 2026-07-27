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
`crates/hughie` — its host protocol, shared layout machinery, and CSS
flexbox, Grid, and Starlight `display: relative` and `display: linear`
algorithms are implemented as first-class peers. Its concrete document/stylo
host lives in `crates/dom`'s `layout` module
(`Document::layout`, results queried by `NodeId` from the document); the Lynx-specific runtime
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
  engine instance with one resource-fetcher handle.
  The resource module must not decode images/fonts/templates, upload render
  resources, or own cache/retry policy; its protocol remains independent of
  decoder/DOM/style/layout/render layers. The crate has no DOM adapter and
  must remain independent of concrete JavaScript engines, including QuickJS.
- `crates/quickjs-rust-bridge` — owner-thread-bound safe Rust wrapper around
  the pinned `vendor/quickjs` submodule. It owns the QuickJS C build and the
  narrow unsafe FFI shim, realm/value lifetime and affinity checks, exact
  ECMAScript string conversion, exception sanitization, and pending-job pump.
  It also owns the **host-function seam**: `Realm::function` /
  `define_global_function` back a JS callable with a Rust `FnMut`, dispatched
  through one C trampoline (`JS_NewCFunctionData` + a realm-owned callback
  table reached via the context opaque). Host callbacks speak `HostValue`, a
  primitives-only boundary (undefined/null/bool/number/string) — objects,
  arrays, functions, symbols, and ill-formed UTF-16 strings are rejected on
  the way in rather than lossily converted — which also means a callback
  cannot call back into its own realm, so host functions are strictly
  leaf calls today. A slot is vacated for the duration of its call (a guard
  that restores it on the unwinding path too), so a panicking callback becomes
  a JS exception rather than an unwind into C and leaves its slot usable, and
  a re-entrant invocation would be refused rather than alias the `FnMut` if a
  future boundary made one reachable. Host closures must not capture a `Value`
  from their own realm — that cycle leaks the realm. The crate must remain
  independent of Bobcat, the DOM, resources, and runtime policy — it knows
  nothing about Lynx.
- `crates/bobcat-quickjs` — narrow integration layer depending on
  `bobcat-engine`, the otherwise Bobcat-independent `quickjs-rust-bridge`, and
  `lynx-element`. Two public surfaces: the opaque QuickJS-backed `LynxView`
  (its default construction factory, an opaque initialization error, and
  resource-host access through that view), and `mainthread` — the Lynx host
  globals. `MainThreadRuntime` is the realm a `.web.bundle`'s `lepusCode.root`
  runs in: it installs the Element PAPI over one `lynx-element::ElementTree`
  *before* evaluation (web-core installs its globals from `onPageConfigReady`,
  which the bundle's section order guarantees precedes `LepusCode`), evaluates
  the chunk inside web-core's own wrapper
  (`(function(){ "use strict"; const navigator=void 0,postMessage=void 0,window=void 0; … })()`),
  and then runs web-core's post-evaluation sequence: `processData` →
  `renderPage` → `__FlushElementTree`. Four of web-core's 61 PAPI members are
  installed (`__CreatePage`, `__CreateView`, `__AppendElement`,
  `__FlushElementTree`); a bundle reaching for any other one gets a
  `ReferenceError` naming it, which is the intended failure mode. Element
  handles cross as unique-id numbers, matching web-core's SSR target and the
  primitives-only script boundary. Runtime configuration, default constants,
  explicit-config construction, the `bobcat-engine::script` adapter types, and
  all realm/value handles, interrupt controls, and raw source-evaluation entry
  points remain crate-private. The future preloaded module graph belongs here
  too, not in the generic QuickJS bridge or engine-neutral protocol.
- `crates/lynx-element` — the Lynx runtime element layer, i.e. the crate the
  layering diagrams drew as the dashed "future Lynx runtime adapter" box. It
  owns exactly what `dom` is forbidden to know: Lynx tag names, Element-PAPI
  opcodes, the unique-id handle space, `<page>` root policy, view metrics and
  stylo `Device` construction, and the Lynx UA cascade defaults
  (`display: linear`, `box-sizing: border-box`, `overflow: hidden`, under the
  `defaultDisplayLinear` / `defaultOverflowVisible` page-config switches).
  `ElementTree` is a `Document<ElementData>` plus a dense, never-recycled
  handle table starting at 1 (slot 0 is web-core's "no element" sentinel);
  every fallible PAPI entry returns `PapiError` instead of panicking, because
  the main-thread script is untrusted input and the DOM core is
  crash-on-misuse — including a `MAX_TREE_DEPTH` cap, because `dom`'s
  recursive layout/paint/hit-test walks overflow the stack and abort the
  *process* somewhere past ~300 levels on a 2 MiB thread, and script must not
  be able to reach that. (A guard, not a fix; the fix is iterative traversal
  in `dom`/`hughie`.) `flush_element_tree` is the single commit boundary: it
  attaches the page on the first call and then runs style + layout. Recorded
  limits (see the crate docs, which are authoritative): handles are ids rather
  than element objects; `parentComponentUniqueID` is recorded but not honored
  (there is no `__SetCSSId`); no `rpx`/`ppx` view-unit policy; the UA sheet
  covers only the three documented Lynx computed defaults. It must not absorb
  DOM/CSS core behavior, and nothing below it may depend on it.
- `crates/dom` — generic W3C-DOM-subset document tree and
  standards-oriented CSS computation core. Owns a fixed-address boxed
  `TreeArenas<T>` containing three `Slab`s: a primary `Slab<Node<T>>` (slot
  zero is the real DOM Document node and carries its node-visible style
  context; later slots are element/text nodes), plus NodeId-aligned payload
  and Stylo traversal/invalidation slabs. A separate inline
  `DocumentLayoutState` owns the fourth, NodeId-aligned layout slab. The
  primary slab selects each raw-`usize` ID; every side slab allocates/removes
  in lockstep and asserts it received that same key (the payload slab reserves
  a payload-less sentinel at document slot zero). Node removal drops all four
  entries before the ID can be reused (ONE TREE policy: nodes are created and
  mutated only through `Document` methods). Computed styles remain with the
  primary nodes; layout/text state does not.
  Every node points directly back only to `TreeArenas`, and the
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
  Its `layout` module is the concrete `hughie` host:
  `Document::layout` flushes styles then lays out with
  the single `LayoutTree` trait implemented on `TreeArenas<T>`. Plain
  `NodeId`s identify nodes, and every engine entry receives `&TreeArenas`
  alongside a separate `&mut DocumentLayoutState`; there is no
  `LayoutTreeView`, session, or store adapter. At the exclusive style-flush
  boundary, each element's preorder callback first marks its layout style
  stale, then publishes a pointer into the primary `Arc<ComputedValues>`
  still owned by Stylo's `ElementData` only after recalculation succeeds;
  layout views
  lend that post-flush value with no `ElementData` borrow check, `Arc` bump,
  copy, or translation layer. A document-level phase flag distinguishes
  Stylo's own mutations from safe out-of-band mutable access, which also
  marks that element stale; a failed traversal therefore remains fail-closed
  even after an unrelated retry. The pointer stays valid until the next
  exclusive traversal, which cannot overlap layout. Public computed-style
  access still uses Stylo's guarded borrow. Layout and text state use ordinary
  exclusive Rust borrows with no runtime borrow checking. Display dispatch routes
  flex/grid/linear/relative with `display: none` hiding and a leaf
  fallback, text nodes through concrete Parley measurement, and the
  positioned pass implements the W3C `position: fixed`
  containing-block rule via the protocol's scheme override.
  `display: contents` elements generate no box: the engine's
  `flattened_children` splices them out of every item collection, and the host
  denies them containing-block, containment, skipped-contents, and hoisting
  status and zeroes their `LayoutSlot` in the positioned pass (the document
  element is exempt — Stylo blockifies it). Replaced leaf
  content reads a closed `NaturalSize` value stored in lazily allocated
  node content; its internal update path automatically invalidates the
  affected cache path. Mutually exclusive literal text, natural size, and
  test-only leaf metadata reuse the node's single nullable content pointer.
  Each `DocumentLayoutState` entry owns one `LayoutSlot` containing the
  measurement cache, static position, and durable rounded/unrounded results;
  `Document::{rounded_layout, unrounded_layout, layout_cache_is_empty}` are the
  query surface. `Layout` is non-`Clone`; rounding reads its `Copy` fields and
  constructs the rounded record without duplicating the whole value.
  Style-driven relayout is automatic (every style
  flush consumes harvested `StyleDamage` into boundary-stopped invalidation);
  `Document::invalidate_layout` remains the
  embedder API for the mutations styles cannot see (content/child-list changes
  with identical computed styles). The internal natural-size update path
  performs that invalidation itself.
  Its `visual` module owns the post-layout visual order:
  the full W3C stacking-context predicate, CSS2 Appendix E paint order
  (`Document::paint_order` → a flat back-to-front `PaintOrder` of items with
  viewport-space transform matrices and overflow/`contain: paint` clip
  chains that honor containing-block escape), transform resolution
  (transform + transform-origin + parent perspective, always flattened —
  the fork has no authorable `preserve-3d`), and reverse-paint-order hit
  testing (`Document::hit_test`/`PaintOrder::hit_test`, honoring
  `visibility`, `pointer-events`, border-radius, and inverse-matrix point
  mapping). It walks the same flattened box-tree the layout host feeds the
  engine, so `display: contents` dissolves identically in paint and hit
  order. Group-effect stacking contexts (`opacity`, `filter`,
  `clip-path`, `mask`, plus the storage-only blend/isolation triggers)
  additionally surface as `RenderLayer` entries — preorder, parent-linked,
  each with the establishing element, its world transform/size, and the
  contiguous item range the group encloses — which is exactly what
  `crates/pulsar` composites; group effects still do not affect hit
  testing (recorded limit). Lynx-specific
  hit-test policy (hit-slop, `user-interaction-enabled`, event-through)
  belongs to the future runtime-policy layer, never here. No retained
  visual cache exists yet; `StyleDamage`'s stacking class is the
  designated hook.
  `DocumentLayoutState` lazily boxes the shared Parley `TextContext`; each
  text node's layout-state entry lazily boxes its probe/commit
  `TextLayoutStore` and reads inherited font/text values from its parent.
  Relayout damage on an element evicts its direct text children's
  measurement caches and retained artifacts because text nodes have no Stylo
  damage record of their own. Parley is unconditional and there is no
  arbitrary payload callback. It must not contain Lynx runtime-element vocabulary or
  Lynx device/unit policy —
  Lynx computed defaults (border-box, `overflow: hidden`, `display: linear`
  on every element, …) stay embedder cascade policy (UA sheet). Relies on
  the vendored stylo fork (`vendor/stylo`, tracking the
  canonical `lynx` branch, tip `8fb7de31a`): `contain` was already seeded
  in the fork's lynx grammar; fork PR #9 (squash-merged into `lynx`) added
  `content-visibility` / `contain-intrinsic-size` under the `lynx` feature,
  pref-gated for stock servo builds; fork PR #10 (squash-merged into
  `lynx`) un-gated `background-clip: text` from gecko the same way and
  seeded the `outline-*` rows (`outline-offset` deliberately omitted —
  Lynx outlines are flush rings).
- `crates/hughie` — the Flexbox, Grid, and
  Starlight Relative and Linear engine: trait-based host⇄engine integration
  with static dispatch only (no `dyn`), one `LayoutTree` protocol with a
  `Copy + Debug` `NodeId`, immutable topology/styles for the flush, and a
  separately borrowed mutable host state containing per-node `LayoutSlot`s.
  The split permits recursive mutation without copying style/layout records
  and without `RefCell`/`AtomicRefCell` checks. Style traits speak the stylo fork's computed-value
  vocabulary directly (requires the `stylo` workspace dep + python3 for its
  build script; the old zero-dependency/standalone pillar is retired), and
  host-side display dispatch; `LayoutTree::flattened_children` is the box-tree
  view every algorithm collects items through, flattening `display: contents`
  subtrees. Leaf content is deliberately closed: replaced
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
  other workspace crates or own host tree/style storage, DOM/runtime types,
  resolved device-unit policy, or paint order.
- Remaining runtime-layout integration — the `LayoutTree` host, display
  dispatch, fixed/hoisted positioned pass, per-node cache storage, and the
  automatic style-damage→`Document::invalidate_layout` wiring (boundary-stopped,
  engine-internal — not a runtime-adapter concern) now live in `dom`
  (see above). Still L3 work in a future runtime adapter: Element-PAPI
  validation and handle lifetime, `rpx`-aware view/device policy, decoded
  `StyleInfo` ingestion and Lynx UA defaults, sticky lowering,
  component-specific staggered layout, and Lynx-specific text
  attribute/raw-text/truncation policy. Generic W3C text style, document
  context, and artifact storage already live in `dom`.
- `crates/pulsar` — the vello-backed paint engine (`hughie` lays out,
  `pulsar` emits light). `Painter::paint(&Document, &PaintOrder, &ImageStore)
  -> &vello::Scene` walks the flat back-to-front item list:
  item clip chains diff against vello clip layers (restarting inside every
  group scope, which preserves containing-block clip escape under grouping
  and upholds the crate-wide vello #1198 invariant: a blend layer's
  *immediate* parent is always a real isolating layer, never a clip layer —
  fragment painters needing a blend under an item clip interpose their own
  `SrcOver` layer), `RenderLayer` group
  scopes push effect layers (opacity/blend alpha over content bounds from a
  single-sweep prepass that also folds child-layer bounds into parents,
  `clip-path` as a full layer, `mask-image` as a
  mask-then-`SrcIn` sandwich, color filters as blend-composite
  approximations at scope close), and per-fragment painters draw outset
  shadows → background color/gradient/image layers → inset shadows →
  replaced content → borders → outline, plus text runs from the retained
  Parley layouts (one shared `linebender_resource_handle` makes parley's
  `FontData` feed `Scene::draw_glyphs` directly). `gpu::Headless` owns the
  wgpu render-to-texture + readback path and fails soft (`NoAdapter`) so
  tests skip GPU-less machines. Style access is `Document::paint_style`
  (post-flush borrow, no `Arc` bump); geometry is the rounded layouts.
  Coordinates: CSS px everywhere, with viewport and device-pixel-ratio
  read from the document's own `Device` (single-sourced with layout —
  never passed in separately). wgpu/peniko/kurbo are consumed exclusively
  through vello's version-matched re-exports (never direct deps). The
  **authoritative** recorded-limits matrix is `crates/pulsar/src/lib.rs`'s
  crate docs — other docs reference it rather than restating it. No retained scene yet — the frame rebuilds from a
  reused `Painter`; `StyleDamage`'s repaint class is the designated hook.
  It must not read Lynx runtime vocabulary (hit-slop, components) and never
  bypasses `PaintOrder` for its own tree walk.
- `crates/flashbulb` — screenshot testing infrastructure, and the only crate
  here that exists for the test suite rather than the product (`publish =
  false`, dev-dependency everywhere). It owns RGBA `Image` + PNG codec, a
  port of the `pixelmatch` algorithm Playwright compares screenshots with
  (squared-YIQ per-pixel distance against `35215 * threshold²`, anti-aliasing
  detection, `max_diff_pixels`/`max_diff_pixel_ratio` budgets), and
  `Screenshots`, the golden store: path resolution from a name-segment list,
  `FLASHBULB_UPDATE_SNAPSHOTS=1` to accept, and `-expected`/`-actual`/`-diff`
  PNGs written to a git-ignored `tests/artifacts/` on failure. A newly
  *created* golden fails its own run so an unreviewed baseline cannot pass;
  an explicitly *accepted* one does not. The optional `render` feature adds
  `capture_document` (`dom` → `pulsar` → headless GPU) over the whole painted
  frame, `viewport * device_pixel_ratio` device pixels — `pulsar` scales the
  scene up by that ratio, so anything smaller is a crop. Playwright instead
  downsamples to CSS pixels; the two coincide at a ratio of 1, which is what
  lynx-stack pins for determinism and what every viewport here uses.
  `headless_or_skip` announces a missing GPU adapter on the process's real
  stderr (libtest discards a *passing* test's captured output, so `eprintln!`
  would be invisible exactly when it matters); `FLASHBULB_REQUIRE_GPU=1` turns
  that skip into a failure. `pulsar` dev-depends on it *with* the
  `render` feature — a dev-dependency cycle Cargo permits; the library graph
  stays acyclic. Goldens are not platform-suffixed: cross-platform
  rasterizer noise is absorbed by tolerance, not by per-platform baselines.
- *(planned, not yet scaffolded)* the remaining runtime crates — see
  `docs/tracking/` for the behavior surface each will need to cover before
  scaffolding begins, and `.claude/agents/` for the subsystem-scoped agent
  personas already set up for this work. `crates/lynx-element` and
  `crates/bobcat-quickjs`'s `mainthread` module are the first pieces of this
  layer to land; the background thread, `StyleInfo` ingestion, the event
  model, and the other 57 Element PAPI members are still ahead.

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

**Screenshot tests** live in `crates/*/tests/screenshots.rs` with committed
goldens in `crates/*/tests/screenshots/`, driven by `crates/flashbulb`. They
need a GPU adapter; without one they print `SKIP <test>` and pass, so a green
run on a GPU-less machine has not exercised them. To accept a new rendering,
look at the image first, then:

```sh
FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p <crate> --test screenshots
```

A golden that does not exist yet is written *and fails its run* — review it
and re-run. Failures write `-expected`/`-actual`/`-diff` PNGs to the
git-ignored `crates/<crate>/tests/artifacts/`; the panic message names all
three plus the exact differing-pixel count. Never accept a golden you have not
looked at: a blank or all-white image compares happily against itself forever.

## Working with Codex

This repo is worked on by both Claude Code (reads `CLAUDE.md`, which points
here) and Codex (reads this file directly). Division of labor between them is
**not yet decided** beyond Codex's existing rescue / second-opinion / review
role (`codex:codex-rescue`, `/codex:review`) — don't assume Codex owns any
particular crate or subsystem unless a task explicitly says so.
