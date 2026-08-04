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
- `crates/bobcat-core` — unified native runtime core. Its always-compiled
  surface owns the protocol-only, host-injected `ResourceFetcher`, the
  ShadowRealm-inspired `ScriptEngine` protocol, and `LynxView<R, E>`, and it
  directly composes `lynx-element`, which owns the dependency edge to `dom`.
  `ScriptEngine::ImportFuture<'a>` is a GAT so external engines return their
  own future type and remain statically dispatched. The default `quickjs`
  feature adds the internal QuickJS adapter,
  opaque QuickJS-backed view factory, and the concrete
  `quickjs::MainThreadRuntime`;
  `default-features = false` excludes QuickJS while preserving all external
  injection contracts. Workspace dependencies disable defaults explicitly;
  only an upper layer that wants the built-in engine enables `quickjs`.
  The core depends on `lynx-element` only (strict linear layering) and
  re-exports it whole — `bobcat_core::lynx_element` is the product's single
  door downward, and the `internal-document-access` feature is forwarded
  through it. The core still adds no document alias, element-host trait,
  renderer wrapper, or injection seam of its own.
  `MainThreadRuntime`
  installs the Element PAPI before evaluation, evaluates a `.web.bundle`'s
  `lepusCode.root` inside web-core's wrapper, then runs `processData` →
  `renderPage` → `__FlushElementTree`. Five of web-core's 61 PAPI members are
  installed (`__CreatePage`, `__CreateView`, `__AppendElement`,
  `__DropElement`, `__FlushElementTree`); unsupported globals remain precise
  `ReferenceError`s. Handles cross the primitives-only boundary as `u32` ids.
  Core composes but does not own Lynx tag/root/UA policy; that vocabulary and
  `type ElementId = u32` remain defined by `lynx-element`.
  The resource module must not decode images/fonts/templates, upload render
  resources, or own cache/retry policy. Runtime configuration, raw realm/value
  handles, interrupts, and source-evaluation entry points remain private. The
  future preloaded module graph belongs in the feature-gated core adapter, not
  in `quickjs-rust-bridge` or the engine-neutral traits.
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
  a re-entrant invocation is refused rather than aliasing the `FnMut` (the
  closure lives behind a `RefCell`, so that guard is structural). A closure's
  lifetime follows its JS function object rather than the realm: the closure
  sits at its own stable heap address, which a companion JS object holds and
  the collector hands back through a finalizer — so nothing is indexed,
  recycled, or aliasable by a stale reference, and discarding a function drops
  its closure. Without this a realm registering a handler per element per
  update (events, worklets) would accumulate every closure it ever made.
  The finalizer only *records* the address; the drop happens at the next
  `&mut Realm` entry point, because a handler may own a `Value` whose `Drop`
  calls `JS_FreeValue` and re-entering QuickJS from inside its own GC is
  unsound. Capturing a same-realm `Value` is therefore safe, but forms a
  reference cycle that leaks the realm unless the function is collected first.
  The crate must remain independent of Bobcat, the DOM, resources, and runtime
  policy — it knows nothing about Lynx.
- `crates/bobcat-cli` — the independent native `bobcat` product over
  `bobcat-core`'s `quickjs` feature. Its only workspace dependencies are
  `bobcat-core` (the layer chain: `bobcat_core::lynx_element::dom::…` reaches
  every lower layer) and the sibling `lynx-template-decoder` utility.
  `bobcat -i file:///…` decodes and boots one web bundle; other URL schemes
  remain rejected at the boundary. One reusable `FramePipeline` owns the
  QuickJS-backed element runtime and borrows the scene retained by its
  document-owned private painter, so the macOS headed path and cross-platform
  headless path share script/layout/paint logic rather than
  maintaining parallel render paths.
  Headed mode uses a native winit window with display-backed vsync and tracks
  both logical viewport size and device-pixel ratio. Headless mode uses a
  configurable synthetic vsync rate, skips catch-up bursts after slow frames,
  and retains its Vello renderer, render texture, and staging buffer across
  frames. Both modes expose a GDB-like stdin command prompt (`continue`,
  `pause`, `frame`, `screenshot`, `help`, `quit`; headless also supports
  `set/show vsync`). Screenshots are captured only through that live prompt;
  there is no one-shot startup flag. PNG readback happens only on a screenshot.
  It must not
  duplicate runtime, DOM, layout, or painting policy: missing MTS/PAPI support
  remains a precise `bobcat-core` QuickJS error, and non-empty decoded `StyleInfo`
  currently produces an explicit author-styles-omitted warning rather than silent
  claimed compatibility.
- `crates/lynx-element` — the Lynx runtime element layer, i.e. the crate the
  layering diagrams drew as the dashed "future Lynx runtime adapter" box. It
  owns exactly what `dom` is forbidden to know: Lynx tag names, Element-PAPI
  opcodes, the unique-id handle space, `<page>` root policy, view metrics and
  stylo `Device` construction, and the Lynx UA cascade defaults
  (`display: linear`, `box-sizing: border-box`, `overflow: hidden`, under the
  `defaultDisplayLinear` / `defaultOverflowVisible` page-config switches).
  This crate defines `type ElementId = u32` and the concrete, validated
  Element-PAPI operations on `ElementTree`; `bobcat-core` composes that type
  directly and `dom` knows neither vocabulary.
  `ElementTree` owns a `dom::Document<ElementId>` plus an independent
  `Vec<Option<LynxElement>>` arena. This crate depends on `dom` **only** — stylo/euclid are reached through
  `dom`'s vocabulary re-exports — and re-exports `dom` whole as the next
  layer's door; it never depends on Bobcat or a JavaScript engine. The DOM payload is only the permanent
  `u32` unique id, which is also the direct arena index; each `LynxElement`
  owns that id, its stable DOM `NodeId` association, component creation
  fields. The arena permanently reserves slot 0 as web-core's
  "no element" sentinel, so live unique ids start at 1. `__DropElement` removes
  the selected DOM subtree and takes the corresponding arena entries, leaving
  permanent `None` tombstones; unique ids are never recycled, although `dom`
  may reuse its private `NodeId` slots;
  every fallible PAPI entry returns `PapiError` instead of panicking, because
  the main-thread script is untrusted input and the DOM core is
  crash-on-misuse. Its default API exposes neither the owned `Document` nor
  render/freshness/scene/image forwarding methods. The non-default
  `internal-document-access` feature exists only for trusted workspace
  composition (`bobcat-cli` and render tests); it must not become an embedder
  convenience or be used for topology mutations, which would desynchronise the
  element arena.
  No public `paint_order` exists on either `ElementTree` or `Document`, and
  input builds its temporary hit-test frame internally. It does not impose a runtime
  tree-depth cap; recursive traversal hardening belongs in `dom`/`hughie`.
  `flush_element_tree` is the single commit boundary: it
  attaches the page on the first call and then runs style + layout. Recorded
  limits (see the crate docs, which are authoritative): handles are ids rather
  than element objects; `parentComponentUniqueID` is recorded but not honored
  (there is no `__SetCSSId`); no `rpx`/`ppx` view-unit policy; the UA sheet
  covers only the three documented Lynx computed defaults. It must not absorb
  DOM/CSS core behavior, and nothing below it may depend on it.
- `crates/dom` — generic W3C-DOM-subset document tree and
  standards-oriented CSS computation core. `docs/dom-public-api.md` is the
  authoritative normal-build versus test-feature API boundary. It owns a
  fixed-address boxed
  `TreeArenas<T>` containing two `Slab`s: a primary `Slab<Node<T>>` (slot
  zero is the real DOM Document node and carries its node-visible style
  context; later slots are element/text nodes) plus a NodeId-aligned payload
  slab. A separate inline
  `DocumentLayoutState` owns the NodeId-aligned layout slab. Stylo's
  per-element style data (the upstream `ElementDataWrapper`, no outer cell)
  and its traversal/invalidation flags live inline on `Node` (bench-defended
  2026-08-03: the paired A/B showed no traversal regression and a measurably
  faster no-op-commit fast path). The
  primary slab selects each raw-`usize` ID; the side slabs allocate/remove
  in lockstep and assert they received that same key (the payload slab reserves
  a payload-less sentinel at document slot zero). Node removal drops all three
  entries before the ID can be reused (ONE TREE policy: nodes are created and
  mutated only through `Document` methods). Computed styles remain with the
  primary nodes; layout/text state does not. The crate's entire `unsafe`
  surface is two blocks — the arena backpointer deref and the
  `TElement::ensure_data` contract call — plus the `unsafe fn` signatures
  Stylo's traits mandate (all bodies safe).
  `Document<T>` also owns one private concrete `Painter`, including its
  reusable walk scratch, retained `vello::Scene`, and `ImageStore`.
  `render` privately builds `PaintOrder` and invokes that painter
  only for a dirty scene. The Painter records which private visual epoch its
  scene represents, so `render`/`needs_render` own retained-scene scheduling without
  publishing that epoch. `scene` lends a guarded shared borrow, while
  `images_mut` is the narrow resource-update seam and invalidates the scene
  conservatively. There is no renderer type parameter,
  `DocumentRenderer` trait, `with_renderer`, public Painter, public visual
  epoch, or public paint-order constructor. The crate also owns the DOM-free
  render floor absorbed from the former `pulsar` crate (2026-08-04): the
  `render` module holds the decoded `ImageStore` (re-exported at the crate
  root) and the `render::gpu` wgpu render-to-texture/readback backend
  (`gpu::Headless`, plus the `read_texture`/`renderer_options`/`render_params`
  seams windowed embedders build against); the crate root re-exports the one
  workspace `vello` version, and embedders configure wgpu/peniko/kurbo
  exclusively through that re-export; the root likewise re-exports `stylo`,
  `euclid`, and `stylo_traits` as the style/geometry vocabulary doors for the
  layers above (strict linear chain: cli → core → element → dom). Quirks mode
  is locked to standards mode: selector matching, the `Stylist`, and the root
  `standards_device` construction seam all hard-wire no-quirks, and no quirks
  knob exists above this crate. `Headless::new` reports `NoAdapter`;
  every GPU-backed test treats that as a hard failure, including in CI.
  Nothing in `render` knows about nodes, computed styles, layout, or paint
  order. Source layout groups the crate by subsystem: `tree/` (arena set,
  `Node`, `Document`), `style/` (engine, Stylo traits, flush, invalidation,
  damage, containment), `layout/`, `visual/`, `paint/` (painter, walker,
  fragment painters), `scroll/`, `input/`, and `render/`.
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
  (`debug_assert` + panic on stale handles rather than silent no-ops). Style
  flush and its per-node `StyleDamage` (repaint / stacking / overflow /
  relayout classes) are internal parts of `Document::layout`; harvested
  damage is then **cleared** (the fix for stylo's never-cleared-damage
  re-traversal bug). During that same harvest,
  relayout-class damage is consumed immediately into boundary-stopped layout
  cache invalidation, so no external damage report is needed to preserve
  layout work; the module also owns the
  `effective_containment` fold (`contain` + `content-visibility` → effect
  bits).
  Its `layout` module is the concrete `hughie` host:
  `Document::layout` flushes styles then lays out with
  the single `LayoutTree` trait implemented on `TreeArenas<T>`. Plain
  `NodeId`s identify nodes, and every engine entry receives `&TreeArenas`
  alongside a separate `&mut DocumentLayoutState`; there is no
  `LayoutTreeView`, session, or store adapter. After each completed
  traversal, the exclusive damage harvest clones every visited element's
  primary `Arc<ComputedValues>` into a per-node layout-style snapshot;
  layout/paint borrow that snapshot with no `ElementData` borrow check or
  per-read `Arc` bump, and the `Arc` keeps the value alive, so reads are
  always memory-safe. The harvest descends wherever Stylo's dirty-descendants
  bits point *or* the element's own snapshot identity changed — the latter
  covers initially styled and freshly cleared (`display: none`) subtrees,
  which set no dirty bits. A debug assertion at every snapshot read reports
  divergence from Stylo's live primary style (an invalidation bug or an
  incomplete traversal); release builds read the stale-but-owned snapshot
  instead of crashing. Public computed-style
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
  `Document::set_natural_size` is the public replaced-content update seam
  (public because the decoder, `crates/image`, is a separate crate); the
  getter stays paint/layout-internal, setting an equal value is a structural
  no-op, and the DOM core still knows no tag names.
  Each `DocumentLayoutState` entry owns one `LayoutSlot` containing the
  measurement cache, static position, and durable rounded/unrounded results;
  `Document::rounded_layout` is the public geometry query; unrounded geometry
  and cache contents stay internal (the cache probe is `#[cfg(test)]`).
  `Layout` is non-`Clone`; rounding reads its `Copy` fields and constructs the
  rounded record without duplicating the whole value.
  Style-driven relayout is automatic (every style
  flush consumes harvested `StyleDamage` into boundary-stopped invalidation);
  the internal invalidation funnel for mutations styles cannot see
  (content/child-list changes with identical computed styles). Public
  mutation methods perform that invalidation themselves; only the
  `layout-test-utils` feature exposes an explicit benchmark hook.
  Its `visual` module owns the post-layout visual order:
  the full W3C stacking-context predicate, CSS2 Appendix E paint order
  (a private flat back-to-front `PaintOrder` of items with
  viewport-space transform matrices and overflow/`contain: paint` clip
  chains that honor containing-block escape), transform resolution
  (transform + transform-origin + parent perspective, always flattened —
  the fork has no authorable `preserve-3d`), and reverse-paint-order hit
  testing (`Document::hit_test` through private `PaintOrder::hit_test`, honoring
  `visibility`, `pointer-events`, border-radius, and inverse-matrix point
  mapping). It walks the same flattened box-tree the layout host feeds the
  engine, so `display: contents` dissolves identically in paint and hit
  order. Group-effect stacking contexts (`opacity`, `filter`,
  `clip-path`, `mask`, plus the storage-only blend/isolation triggers)
  additionally surface as `RenderLayer` entries — preorder, parent-linked,
  each with the establishing element, its world transform/size, and the
  contiguous item range the group encloses — which is exactly what the
  document-owned Painter composites; group effects still do not affect hit
  testing (recorded limit). Lynx-specific
  hit-test policy (hit-slop, `user-interaction-enabled`, event-through)
  belongs to the future runtime-policy layer, never here. No retained
  visual cache exists yet; `StyleDamage`'s stacking class is the
  designated hook.
  The private `painter`/`walker`/`paint`/`shape` modules turn that order into
  the retained Vello scene. Item clip chains diff against Vello layers;
  `RenderLayer` scopes composite opacity, filters, clip paths, and masks; box
  fragments paint shadows, backgrounds, replaced content, borders, outlines,
  and retained Parley glyphs. Internal style access is `Document::paint_style`
  (post-flush, no `Arc` bump), geometry is the rounded layout, and the
  document Device supplies viewport/DPR so paint cannot disagree with layout.
  The authoritative paint limits are recorded in
  `crates/dom/src/painter.rs`; DOM-aware paint tests and the paint benchmark
  live under `crates/dom/tests` and `crates/dom/benches`.
  Its `scroll` module owns CSSOM-View scrolling — scrollport/scrolling-area
  geometry off the layout engine's accumulated `content_size`, a per-node
  offset in the layout arena that re-clamps itself on every read (so a
  shrinking relayout or a restyle out of scroll-container-hood needs no
  invalidation hook), `scroll_to`/`scroll_by` (which returns the
  **unconsumed remainder**, the primitive chaining is built from), and
  `scroll_chain`. Both the "which box scrolls" walk and the chaining advance
  follow the **containing-block** chain, not DOM ancestry, so they agree with
  what `visual` actually moves: a wheel over an `absolute` box anchored above a
  scroller scrolls nothing, rather than sliding content behind a box that
  visibly stays put. Only `overflow: scroll` is user-scrollable; `hidden` is a
  scroll container that moves only programmatically (load-bearing here,
  because the Lynx UA cascade puts `hidden` on every element) and `clip` is
  not a scroll container at all — it clips, has no offset, and its content
  does not reach into an ancestor's scrolling area either (`hughie`'s
  `accumulate_scrollable_overflow` asks per axis). `visual` bakes the offsets
  into the frame — a scroll container's contents are translated as they are
  collected, with containing-block-keyed escape sharing the clip chain's own
  struct, so painting and hit testing see scrolled geometry and the lower
  render/GPU floor needs no knowledge of scrolling. Clipping is likewise per axis, because
  `clip` on one axis with `visible` on the other is a pair the style adjuster
  leaves mixed; a one-axis clip is an infinite strip and carries no radii.
  Its `input` module is the host seam: `InputEvent` is plain `Copy` data
  (pointer + wheel, viewport CSS px) that a canvas, a native window, or a
  test literal all produce equally, and `Document::handle_input(InputEvent)`
  builds its private visual frame, routes the event, and performs the UA default action, reporting both
  in an `InputResponse`. Dispatch to listeners is *not* here — this crate has
  no `EventTarget` — so `InputEvent::default_prevented` is the
  `preventDefault()` seam a runtime layer hands back after its own
  capture/bubble walk or gesture arbitration; a runtime wanting different
  scroll physics (Lynx `parent-first` nesting, rubber-band, fling) prevents
  the default and drives `scroll_by`/`scroll_chain` itself. The drag
  recognizer is deliberately minimal (touch/pen only, one slop threshold,
  boundary chaining, no momentum — this crate owns no clock).
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
  canonical `lynx` branch, tip `019d1fb50`): `contain` was already seeded
  in the fork's lynx grammar; fork PR #9 (squash-merged into `lynx`) added
  `content-visibility` / `contain-intrinsic-size` under the `lynx` feature,
  pref-gated for stock servo builds; fork PR #10 (squash-merged into
  `lynx`) un-gated `background-clip: text` from gecko the same way and
  seeded the `outline-*` rows (`outline-offset` deliberately omitted —
  Lynx outlines are flush rings); fork PR #11 (squash-merged into `lynx`)
  seeded `object-fit` / `object-position`, which were already ungated in
  `longhands.toml` and compiled out only by absence from the allowlist —
  replaced content needs them for the css-images-3 concrete-object-size
  rules; and fork PR #12 (squash-merged into `lynx`)
  un-gated `overflow: scroll | clip` and added
  `Overflow::is_user_scrollable`. The native engine's grammar really is
  `visible | hidden`, but the **web** bundle this stack consumes uses the
  other two directly (`web-elements`' own `scroll-view.css` authors
  `overflow-y: scroll` and `overflow-x: clip`), so no bundle could express a
  scrollable box at all. **`auto` stays out** (user decision, 2026-07-29):
  this engine paints no scrollbars, so `auto` would be indistinguishable from
  `scroll` everywhere except `to_scrollable()`, where it is the value a
  `visible` axis pairs into — that now pairs into `hidden`, a recorded
  deviation (an axis that genuinely overflows is clipped rather than
  draggable). The three non-`visible` values stay genuinely distinct:
  `scroll` is user-scrollable, `hidden` is a scroll container that moves only
  programmatically, `clip` is not a scroll container at all.
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
  automatic style-damage→layout-invalidation wiring (boundary-stopped and
  engine-internal — not a runtime-adapter concern) now live in `dom`
  (see above). Still L3 work in the runtime adapter: the remaining Element-PAPI
  surface, `rpx`-aware view/device policy, decoded `StyleInfo` ingestion,
  sticky lowering,
  component-specific staggered layout, and Lynx-specific text
  attribute/raw-text/truncation policy. Generic W3C text style, document
  context, and artifact storage already live in `dom`.
- `crates/image` — the replaced-content pipeline below the DOM: container
  sniffing from magic bytes, header-only intrinsic-size probing, decode to
  RGBA8, and the async fetch→decode→cache loader over `bobcat-core`'s
  `ResourceFetcher`. PNG/JPEG/WebP, **static only**. One always-compiled
  pure-Rust backend (`png` + `zune-jpeg` + `image-webp` taken directly rather
  than through the `image` facade — the facade would collide with this
  package's own name and make `cargo check -p image` ambiguous forever) plus at
  most one platform backend chosen by a **runtime** probe: Apple ImageIO,
  Windows WIC, Android NDK `AImageDecoder`. That probe is genuinely runtime —
  ImageIO gained WebP in macOS 11/iOS 14, WIC's WebP codec is a Store
  extension, `AImageDecoder` is API 30+. `Acceleration` reports codec
  *provenance* (`Software` / `PlatformSoftware`), never a claim about silicon:
  no still-image API on any of the three platforms exposes an acceleration
  query or reaches a decode ASIC, so `DedicatedHardware` is reserved and
  unreported. Routing may disagree with the ladder — on Apple, PNG stays on the
  software backend because ImageIO just delegates to bundled libpng. It deliberately
  does **not** depend on `dom`: it returns an `ImageHeader` and a
  `DecodedImage`, and installing those on a node and in an `ImageStore` is the
  caller's job. `DecodedImage::to_image_data` reaches `peniko` through vello's
  re-export behind the default `vello` feature, so the crate can be
  cross-checked for Windows/Android without building wgpu. The **authoritative**
  recorded-limits list is `crates/image/src/lib.rs`'s crate docs. The Lynx
  `<image>` element surface (`mode`, `placeholder` racing, `cap-insets`,
  `blur-radius`, `load`/`error` events) belongs above this crate and is not
  implemented; nothing here is exposed to `lynx-element`.
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
  `capture_document` (`Document::render` → retained scene → `dom`'s headless GPU) over the whole painted
  frame, `viewport * device_pixel_ratio` device pixels — the render floor scales the
  scene up by that ratio, so anything smaller is a crop. Playwright instead
  downsamples to CSS pixels; the two coincide at a ratio of 1, which is what
  lynx-stack pins for determinism and what every viewport here uses.
  Replaced images are registered through `Document::images_mut` before
  capture. `capture_document` cannot accept or default a second store: it
  necessarily renders from the document-owned registry, and a raster-image
  golden guards that ownership path. `headless` requires a usable GPU adapter and panics when one is
  unavailable, so local and CI test runs obey the same mandatory-GPU policy.
  DOM-aware screenshot suites live in `dom`, which also keeps the direct GPU
  smoke tests. Goldens are not platform-suffixed: cross-platform
  rasterizer noise is absorbed by tolerance, not by per-platform baselines.
- *(planned, not yet scaffolded)* the remaining runtime crates — see
  `docs/tracking/` for the behavior surface each will need to cover before
  scaffolding begins, and `.claude/agents/` for the subsystem-scoped agent
  personas already set up for this work. `crates/lynx-element` and
  `crates/bobcat-core`'s feature-gated `quickjs` module are the first pieces of this
  layer to land; the background thread, `StyleInfo` ingestion, the event
  model, and the other 56 Element PAPI members are still ahead.

See `docs/runtime-architecture.md` for the runtime dependency graph, feature
boundary, private paint pipeline, and frame walkthrough;
`docs/style-architecture.md` and `docs/layout-architecture.md` contain the
style/layout ownership rules.

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
- `docs/text-rendering-research.md` — **read before proposing any text-painting
  performance work.** Why vello 0.9 has no glyph atlas and cannot get one, what
  a text-heavy frame actually costs here (measured), where the ecosystem's
  answer lives (`glifo` via `vello_hybrid`), and why `glyphon` and a
  hand-rolled atlas are both ruled out. Conclusion is *don't switch renderers
  yet* — so the useful contribution is evidence, not a port.

## Toolchain

- Nightly Rust (`rust-toolchain.toml`), edition 2024, resolver 3, workspace lints.
- `cargo fmt` (nightly rustfmt options in `rustfmt.toml`), `cargo clippy`,
  `cargo test`, `cargo bench` (CodSpeed-compatible `divan` benches).
- **`cargo fmt --all` reaches into `vendor/stylo`** even though the fork is
  excluded from the workspace, and the fork carries pre-existing upstream
  rustfmt drift, so it "fixes" files nobody touched. Check
  `git -C vendor/stylo status` afterwards and revert anything outside your own
  change, or the next fork commit ships unrelated reformatting.

## Testing

Integration tests decode real fixtures vendored from lynx-stack under
`crates/lynx-template-decoder/tests/fixtures/` (Apache-2.0 build artifacts).
`cargo test` must pass on the pinned nightly toolchain.

**Screenshot tests** live in `crates/*/tests/screenshots.rs` — plus per-topic
siblings (`dom` also has `text_screenshots.rs` and `css_atlas.rs`) — with
committed goldens in `crates/*/tests/screenshots/`, driven by
`crates/flashbulb`. The ordinary screenshot suites share one capture harness
in `tests/support/screenshot.rs`; the browser-referenced CSS atlas owns the
separate workflow documented below. The golden store is per *crate*, so every
screenshot binary in a crate writes into the same tree. They require a GPU
adapter; without one the test run fails, including in CI, so a green run always
means the pixels were rendered and compared. To accept a new rendering in the
ordinary suites, look at the image first, then (dropping `--test` to catch every
ordinary screenshot binary in the crate):

```sh
FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p <crate>
```

A golden that does not exist yet is written *and fails its run* — review it
and re-run. Failures write `-expected`/`-actual`/`-diff` PNGs to the
git-ignored `crates/<crate>/tests/artifacts/`; the panic message names all
three plus the exact differing-pixel count. Never accept a golden you have not
looked at: a blank or all-white image compares happily against itself forever.
Browser-owned suites can reject `FLASHBULB_UPDATE_SNAPSHOTS`; follow their
checked capture and audit workflow instead. The CSS paint atlas has two
explicit reference owners: 666 Chromium matches remain browser-owned, while
145 W3C-correct differences (84 rasterization/sampling cases plus 61
standards-permitted UA choices) use native DOM/Parley snapshots in a
separate directory. Native atlas references may be updated only with the
filtered `CSS_PAINT_UPDATE_NATIVE=1 ... css_native_` workflow, which cannot
overwrite browser references; the other 189 cases remain ignored. The browser
stage uses `isolation: isolate` to match the native document element's
stacking-context role, so all 22 negative-z probes are Chromium-owned exact
matches. The CSS paint matrix records the exact capture, update, and
full-browser-audit workflow in `docs/css-paint-screenshot-matrix.md`.

## Working with Codex

This repo is worked on by both Claude Code (reads `CLAUDE.md`, which points
here) and Codex (reads this file directly). Division of labor between them is
**not yet decided** beyond Codex's existing rescue / second-opinion / review
role (`codex:codex-rescue`, `/codex:review`) — don't assume Codex owns any
particular crate or subsystem unless a task explicitly says so.
