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
- `crates/lynx-xml` — zero-dependency, zero-copy parser for the restricted
  single-file Lynx XML source envelope. It extracts borrowed optional style,
  required main-thread script, and optional background-thread script sections,
  and reports the reference web parser's UTF-16 error offset together with a
  Rust-native UTF-8 byte offset. Scope: source grammar and section extraction
  only — no input sniffing, I/O, configuration mapping, CSS parsing, bundle
  encoding, or runtime launch. It is a sibling of the binary template decoder,
  never another format inside it.
- `crates/bobcat-core` — unified native runtime core. Its public runtime is the
  opaque `LynxView<'window, W>` facade plus the protocol-only, host-injected
  `ResourceFetcher`, `ScriptEngineFactory`, `ScriptEngine`, image-codec
  `Decoder`, draw-target, OS-input, and lifecycle-wakeup capabilities, plus
  narrow view-level font and decoded-image registration. `PageConfig` is
  supplied when the view is constructed. Bundle retrieval, `.web.bundle`
  decoding, and config parsing are embedder responsibilities; core accepts a
  script URL through `LynxView::execute_script`, resolves/fetches its UTF-8
  source through the injected `ResourceFetcher`, and reports completion
  through `pump`. `execute_script_with_cancellation` accepts the resource
  protocol's public `CancellationToken`; dropping its future cancels the same
  token observed by pending resolution/fetch work.
  `load_style_sheet(url)` loads author CSS through the same fetcher and mounts
  it on the document; the protocol's `fetch_style_sheet` answers with either
  CSS text or a `PreparsedStyleSheet` (`bobcat_core::style`) the host parsed
  itself, since a `.web.bundle` ships CSS a build step already tokenized and
  re-serializing it to a sheet blob is the startup cost the design rules out.
  Lowering it produces no stylesheet text: rules, keyframes, and font-face
  rules are built directly through `dom`'s branded `CssRule` builders, leaving
  stylo one selector-list parse per rule and one value parse per declaration —
  the floor, because the wire format keeps attribute selectors and functional
  pseudo-classes as text and stylo builds specified values only through its
  value parsers. Decoding a container stays embedder work: core owns the
  `PreparsedStyleSheet` vocabulary, and the embedder fills it. Load order is
  cascade order. Per-component css-id scoping is **not** implemented — every
  fragment mounts globally, which is what web-core itself emits for a
  `enableRemoveCSSScope = true` bundle. The document, tree, engine, and realm
  cannot be borrowed or decomposed from the facade.
  `ScriptEngineFactory` is `Send + Sync` and creates the owner-thread-bound,
  non-`Send` `ScriptEngine` only after the factory reaches the engine-owned
  Lynx main thread. The VM contract installs named host callbacks, evaluates
  named source, and provides the optional GC seam; `HostValue` is primitives-only,
  so realm values and DOM handles never cross it. The default `quickjs`
  feature adds a private QuickJS adapter and exposes only
  `quickjs_engine_factory() -> Arc<dyn ScriptEngineFactory>`. The private
  `MainThreadRuntime` owns the realm integration: a
  batch's first `bobcat` call takes the document out of its hand-off
  slot, every call after that is a plain `&mut` mutation with no
  synchronization, and `__FlushElementTree` runs the style + layout commit
  on the taken document, puts it back, and notifies the presenter through
  the callback injected at construction — locks are touched twice per
  batch, never per call;
  `default-features = false` excludes QuickJS while preserving all external
  injection contracts. Workspace dependencies disable defaults explicitly;
  only an upper layer that wants the built-in engine enables `quickjs`.
  The core depends on `dom` but does **not** re-export it. Its private `Engine`
  passes the element tree to and from its engine-owned Lynx main thread through
  the private `SharedTree` hand-off slot — one holder at any instant — and runs input
  routing, scrolling, frame production, and presentation on the thread the
  embedder calls it from; vsync interacts with the OS only there. The
  presenting side borrows the tree non-blockingly (an empty slot = batch
  open = re-present the retained target, buffer input, retry next frame;
  present's vsync wait happens outside the borrow), the slot is occupied
  while the script merely computes (a long JS task between batches cannot
  stop scrolling — one truth, no reconciliation protocol), a half-applied
  batch is unobservable while the slot is empty; an abandoned batch may
  present once its evaluation ends, which is web-core's visibility model. Embedders provide user input, device
  metrics, OS initialization, a draw target, and IO primitives, and relay
  OS facts in (`dispatch_input`/`resize`/`notify_redraw`/`pump`/ticks);
  they never start or steer the pipeline. Engine events are enqueued and then
  wake the host's `pump` through the construction-time `EventRequester`;
  drawing is scheduled through the public `Window` capability borrowed at
  attach time (`target`, `frames`).
  The private `Engine` is generic over that trait; the draw target is a GAT,
  which is what lets a native surface borrow
  the embedder's window instead of requiring a `'static` refcounted handle.
  Browser embedders may instead pass an owned canvas target through the async
  `attach_target`; `FrameRequester` owns both redraw requests and the optional
  presenting-side `pre_present` hook. `OffscreenLynxView` is the public
  windowless facade over the uninhabited `NoWindow`.
  The `image` module contains the replaced-content decode **contract** and a
  private engine pipeline:
  container identification from magic bytes (PNG, JPEG, WebP, GIF, HEIC,
  AVIF), per-container framing/truncation checks, the injected `Decoder`
  trait, plus the internal fetch→decode→cache loader over the resource
  protocol. Cache keys, caches, loader configuration, and `ImageLoader` are
  not public. **No codec ships in the engine**: the engine only designs the
  contract, and the embedder implements a `Decoder` (the reference embedder's
  `image_decoders::platform_decoder()`, or its own implementation over an
  existing app image pipeline — that seam is the point). Automatic
  decode/loading for the Lynx `<image>` element remains unwired; current
  callers may use the standalone decode contract and install finished pixels
  under a CSS URL through `LynxView::register_image_url`. The private engine
  writes that registration into its `ImageStore` and refreshes the retained
  scene without exposing either object. The engine's own
  contract tests inject a PNG decoder double the same way
  (`src/image/loader_test_support.rs`'s `PngDouble`). A sniffed format the injected decoder does not claim is
  `ImageError::Unsupported`, distinct from `UnknownFormat`. **Static only.**
  `Acceleration` reports codec *provenance* (`Software`/`PlatformSoftware`),
  never a claim about silicon — no still-image API on any supported platform
  exposes an acceleration query or reaches a decode ASIC, so
  `DedicatedHardware` is reserved and unreported. The module never touches
  `dom` node types: it returns an `ImageHeader` and a `DecodedImage`
  (internally `to_image_data` reaches peniko through `dom`), and installing
  those on a node and in an `ImageStore` is
  the engine loop's job. The **authoritative** recorded-limits list is
  `crates/bobcat-core/src/image/mod.rs`'s module docs. The Lynx `<image>`
  element surface (`mode`, `placeholder` racing, `cap-insets`, `blur-radius`,
  `load`/`error` events) belongs above this module and is not implemented.
  `Engine`, `SharedTree`, `TreeGuard`, `LynxDocument`,
  `Viewport`, `new_document`, `MainThreadRuntime`, and the concrete QuickJS
  adapter are all crate-private.
  The private `MainThreadRuntime`
  installs the global `bobcat` object (one Rust host function per member —
  `createPage`, `createElement`, `setAttribute`, `removeAttribute`,
  `getAttribute`, `tagName`, `insertBefore`,
  `removeElement`, `replaceElement`, `dropElement`, `flushElementTree` — all
  speaking DOM vocabulary over numeric `NodeId`s), evaluates the embedded
  Element PAPI runtime (`packages/bobcat-element`), then evaluates a
  `.web.bundle`'s `lepusCode.root` inside web-core's wrapper and runs
  `processData` → `renderPage` → `__FlushElementTree`. The PAPI runtime
  assigns the twenty-six Element PAPI globals: every ReactLynx Snapshot
  constructor except `__CreateFrame` (`__CreatePage`, `__CreateElement`,
  `__CreateWrapperElement`, `__CreateText`, `__CreateImage`, `__CreateView`,
  `__CreateScrollView`, `__CreateRawText`, `__CreateList`), all six tree
  mutation calls (`__AppendElement`, `__InsertElementBefore`,
  `__RemoveElement`, `__ReplaceElement`, `__ReplaceElements`,
  `__SwapElement`), the property surface a Snapshot's `create`/`update`
  functions write through (`__SetClasses`, `__SetID`, `__SetAttribute`,
  `__SetInlineStyles`, `__AddEvent`) with the queries that read it back
  (`__GetID`, `__GetTag`, `__GetElementUniqueID`, `__GetEvent`,
  `__GetEvents`), and `__FlushElementTree`;
  unsupported globals remain precise `ReferenceError`s, including
  `__DropElement`, which no web-core generation has.
  `__CreateList` consumes only its numeric parent-component argument for now;
  callback storage/execution remains part of the unimplemented list surface,
  and `__SetAttribute` throws for `update-list-info` — the one name that is a
  list command rather than an attribute — instead of writing a stringified
  command object onto the element.
  `__AddEvent` is recorded but unconsumed, deliberately: it stores handlers in
  the realm (web-core's two slots per event type and name, a background-thread
  handler name and a main-thread worklet, cleared together by a null handler)
  and nothing dispatches to them.
  `__SetCSSId` is absent rather than unimplemented — it names the author-CSS
  scope an element cascades in, and until a layer lowers a decoded `StyleInfo`
  into **scoped** author rules there is nothing to validate an encoding against
  (ingestion has landed, but mounts every fragment globally)
  (web-core writes `l-css-id`/`l-e-name` attributes; native Lynx keeps css_id
  on the element). It lands with the ingestion side that reads it, together
  with the parent-component css-id inheritance that feeds it.
  Creation calls return plain JavaScript handle objects minted by the PAPI
  runtime; each carries its DOM `NodeId` under a realm-local symbol and is
  registered with a `FinalizationRegistry` whose cleanup calls
  `bobcat.dropElement`, freeing only that element — its descendants remain
  live but detached until their own handles are collected. Cleanup runs as
  a pending job at the job checkpoints (a collection comes from allocation
  pressure or its private garbage-collection checkpoint), and pending jobs never
  run at realm teardown, which preserves the last committed tree.
  Core owns Lynx page policy in its `tree` module — the `page` root tag,
  `Viewport`/stylo `Device` construction, and the Lynx UA cascade defaults;
  the `bobcat` host functions call `dom::Document` directly — while tag
  vocabulary, handle lifecycle, and the PAPI member surface live in
  `packages/bobcat-element`. Element identity is the DOM `NodeId`; the host
  boundary validates primitive arguments, live IDs, and tree-mutation
  preconditions before entering `dom`, returning misuse as a JavaScript
  exception (unexpected internal panics remain fatal on abort-only Wasm). An unflushed batch may
  present once its evaluation ends — web-core's visibility model, where
  the browser paints the live DOM regardless of `__FlushElementTree`.
  The resource module must not decode images/fonts/templates, upload render
  resources, or own cache/retry policy. Runtime configuration, raw realm/value
  handles, interrupts, and source-evaluation entry points remain private. The
  future preloaded module graph belongs in the feature-gated core adapter, not
  in `quickjs-rust-bridge` or the engine-neutral traits.
- `crates/quickjs-rust-bridge` — owner-thread-bound safe Rust wrapper around
  the pinned `vendor/quickjs` submodule. It owns the QuickJS C build and the
  narrow unsafe FFI shim, realm/value lifetime and affinity checks, exact
  ECMAScript string conversion, exception sanitization, and pending-job pump.
  Every heap allocation made by the C shim or the five compiled QuickJS C
  translation units is redirected through a private C ABI into Rust's global
  allocator; a fixed aligned prefix supplies the size required for matching
  `realloc`/`free` and QuickJS memory accounting. QuickJS's `snprintf` and
  `vsnprintf` calls are likewise redirected to a crate-private wrapper around
  the pinned, allocator-free `nanoprintf` header; native and Wasm builds use
  the same integer/string formatter without importing libc `stdio`, `FILE`,
  locale, or another heap. All targets compile the C sources against the same
  crate-private `stdlib`/`stdio`/`inttypes`/`string`/`math` declaration facade:
  host allocation and the audited C gaps route to Rust, stack and basic
  memory operations remain compiler builtins, and the bridge-unexposed
  `FILE`/standard-stream diagnostic API is compiled out rather than modelled
  as a platform ABI. The realm deliberately does
  not install JavaScript shared-memory primitives: both `Atomics` and
  `SharedArrayBuffer` are absent, while ordinary `ArrayBuffer`, typed arrays,
  and `DataView` remain available. This does not disable Rust-side atomics used
  for interruption or host synchronization. Because QuickJS formerly coupled
  its process-global class-ID mutex to the same feature, the bridge allocates
  its one host class ID through a Rust `OnceLock` and registers that ID
  separately in each runtime, preserving concurrent native realm creation.
  It also owns the **host-function seam**: `Realm::function` /
  `define_global_function` back a JS callable with a Rust `FnMut`, dispatched
  through one C trampoline (`JS_NewCFunctionData` + a realm-owned callback
  table reached via the context opaque). Host callbacks speak `HostValue`, a
  primitives-only boundary (undefined/null/bool/number/string) — ordinary
  objects, arrays, functions, symbols, and ill-formed UTF-16 strings are
  rejected on the way in rather than lossily converted; element identity
  crosses as plain numbers, and handle objects never leave JavaScript. This
  boundary also means a callback
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
  `bobcat-core`'s `quickjs` feature. Its workspace dependencies are
  `bobcat-core` and the sibling `lynx-template-decoder` utility; the
  per-OS codec crates it consumes are target-scoped, for `image_decoders`
  below.
  `bobcat -i file:///…` decodes and boots one web bundle; other URL schemes
  remain rejected at the boundary. The CLI is an **embedder** of the opaque
  `bobcat_core::LynxView`: it owns argument parsing, bundle bytes, decoding,
  `PageConfig` parsing, an in-memory `ResourceFetcher` for the decoded root
  script URL, the winit window and event loop, device metrics, input
  translation, the stdin prompt, and PNG writing — and nothing of the
  pipeline. Every event handler is a relay into the view
  (`dispatch_input`, `resize`, `notify_redraw`, `pump`, clock ticks in
  headless mode); the engine owns the tree, commits, scheduling, and its
  script and render threads. Frame callbacks go through the `MacWindow` it
  borrows at attach time (the winit window as the draw target,
  `request_redraw`, `pre_present_notify`); lifecycle events wake the event
  loop through the separately injected `EventRequester`. The CLI starts the root with
  `execute_script(url)` and observes `ScriptFinished` through `pump`. Headed
  mode attaches the window as the draw target; headless mode attaches the
  view's offscreen target and relays synthetic
  vsync ticks — whether a tick becomes GPU work is the engine's decision.
  The `image_decoders` module carries the **reference implementations** of
  the engine's `bobcat_core::image::Decoder` contract — implementing the
  decoder is embedder work, so they live in the reference embedder. One per
  OS at compile time: Apple (macOS/iOS) `ImageIO`, claiming all six
  identified formats **unconditionally** (the workspace assumes an OS floor
  above every needed codec — WebP macOS 11/iOS 14, AVIF macOS 13/iOS 16 — so
  there is no runtime probe; JPEG EXIF orientation from the module's own
  byte parser, HEIC/AVIF orientation from `kCGImagePropertyOrientation`);
  Windows WIC (PNG/JPEG inbox, WebP probed — Store extension); Android NDK
  `AImageDecoder` via `dlopen` (API 30+; the `dlsym` result is the probe);
  Linux **only**, the pure-Rust reference decoder (`png` + `zune-jpeg` +
  `image-webp` taken directly rather than through the crates.io `image`
  facade). On Windows/Android a failed probe leaves `platform_decoder()` =
  `None` with **no fallback behind it** — and since the CLI's mandatory
  QuickJS C sources do not cross-compile to those ABIs, the WIC and NDK
  modules have **no CI type-check gate**: recorded reference material for
  embedders that do not exist yet, not live code. Decoder-behaviour tests
  (real JPEG/WebP/EXIF fixtures) live in `tests/image_decoders.rs`; the
  measured `ImageIO` API comparison that fixed the Apple decoder's choices
  (thumbnail path, never `ShouldCacheImmediately`, the accepted ~30% PNG
  cost) is recorded in `apple.rs`'s module docs, and the one-off bench
  harness that produced it was deliberately not kept.
  Headed mode uses a native winit window with display-backed
  vsync and tracks both logical viewport size and device-pixel ratio. Headless mode uses a
  configurable synthetic vsync rate, skips catch-up bursts after slow frames,
  and retains its Vello renderer, render texture, and staging buffer across
  frames. Both modes expose a GDB-like stdin command prompt (`continue`,
  `pause`, `frame`, `screenshot`, `help`, `quit`; headless also supports
  `set/show vsync`). Screenshots are captured only through that live prompt;
  there is no one-shot startup flag. PNG readback happens only on a screenshot.
  It must not
  duplicate runtime, DOM, layout, or painting policy: missing MTS/PAPI support
  remains a precise `bobcat-core` QuickJS error. Its `style_info` module lowers
  a decoded `StyleInfo` into `bobcat_core::PreparsedStyleSheet` — flattening
  every `css_id` fragment in reverse-topological order, imported before
  importing — and registers it in the fetcher so both runners load it before
  the first script batch. A bundle carrying non-zero fragment ids warns that
  per-component scoping is not implemented rather than claiming compatibility.
- `crates/bobcat-wasm` — the pure-Rust `wasm-bindgen` browser embedder and npm
  facade, built for `wasm32-unknown-unknown` with shared memory. The browser UI
  thread is a JavaScript-only host coordinator: it creates one explicit
  embedder Worker and transfers an `OffscreenCanvas`, but never instantiates
  Wasm or owns engine state. That Worker initializes the module, constructs
  the complete opaque `LynxView`, permanently owns
  every thread-affine GPU object — crates.io Vello 0.9/wgpu 29 Device, Queue,
  Surface, Renderer, and OffscreenCanvas — and uses `wasm_thread` to create its
  nested Lynx main/VM Worker. Core builds Stylo's ordinary private Rayon pool
  there with `wasm_thread` as its browser thread spawner, leaving the vendored
  Stylo sources unchanged. The public `ScriptEngineFactory` creates the
  owner-thread-bound browser JavaScript VM inside that Worker; Element-PAPI
  batches, Stylo/Rayon, layout, and
  render hand-off then synchronize through Rust channels, mutexes, atomics,
  and the shared Wasm memory exactly as in a native embedder. JavaScript
  `postMessage` is only the browser host boundary (initial Canvas transfer,
  URL-based script requests/results, resize/input/lifecycle) or a library's
  Worker bootstrap control plane; it is not a DOM/render reconciliation
  protocol. A shared atomic startup handshake gates readiness. URL requests
  are serialized, and a lost-wake-safe `EventSignal` Promise wakes script
  completion independently of Worker rAF, so a hidden page may pause drawing
  without stranding the `executeScript` Promise. Startup has a ten-second
  watchdog until the nested VM Worker begins; after that the browser VM
  deliberately has no execution timeout or safe interrupt, so recovery from
  an infinite script requires disposing and recreating the canvas/Worker.
  One Wasm instance owns one view and
  its Stylo pool; every public `BobcatCanvas` gets a separate Render Worker and
  Wasm instance. The pool minimum is two threads so one managed Rayon worker
  remains after the synchronous entry-task Worker exits. The UI never
  blocks, while Worker-side Rust may block wherever the native runtime does.
  The browser target enables `parking_lot_core/nightly` so transitive
  Stylo/wgpu parking_lot locks use Wasm atomic wait/notify instead of the
  non-atomic Wasm backend that panics on contention.
  `wasm_thread` is pinned to the upstream
  `spawn_from_worker` change because its crates.io release otherwise forwards
  nested spawns to a parent protocol handler that an explicit embedder Worker
  does not have; Chrome 135 supports the resulting nested module Worker.
  The module depends on `bobcat-core` with QuickJS disabled and injects a
  `js_sys` browser `ScriptEngine`. Browser `fetch` remains outside Wasm: the
  Render Worker registers raw fetched bytes in its `ResourceFetcher`, core
  performs strict UTF-8 validation, and the worker calls `execute_script(url)`.
  The facade exposes no create/append/drop/flush,
  document, tree, or engine API. It does not decode `.web.bundle` containers;
  callers supply `PageConfig` and executable script URLs. Synchronous GPU
  capture is likewise absent because
  browser WebGPU completion is Promise-driven.
- `packages/bobcat-element` — the Element PAPI runtime, a single
  dependency-free classic-script JavaScript file (`src/element-papi.js`) that
  `bobcat-core` embeds with `include_str!` and evaluates into the injected
  script-engine realm before any bundle code; its Rstest suite runs the same
  bytes. It owns the twenty-six `__*` PAPI members and their web-core arities,
  plus the Lynx tag vocabulary
  (`wrapper`/`text`/`image`/`view`/`scroll-view`/`raw-text`/
  `list`). It also owns the value coercions web-core gets from the HTML DOM for
  free: truthiness-not-null clearing for classes, ids, and inline styles,
  `String(value)` for every attribute, and camelCase-to-kebab hyphenation of a
  record-shaped inline style. Event registrations live here too, in a WeakMap
  keyed by the handle, so a registration can never keep its element alive.
  An element handle is a plain object carrying its DOM `NodeId`
  under a realm-local symbol (web-core's `uniqueIdSymbol` shape) — one
  object per element for its whole life, so every PAPI return of an element
  yields the same object.
  `parentComponentUniqueID` and `__CreatePage`'s arguments are accepted for
  PAPI shape and unused. Lifecycle: collection is the only release path —
  web-core's model, where a swept `WeakRef` is what frees an element.
  Every non-page handle is registered with a `FinalizationRegistry` whose
  cleanup calls `bobcat.dropElement`; cleanup runs as a pending job at the
  host's job checkpoints, and never at realm teardown, which preserves the
  last committed tree. The JavaScript layer deliberately does not validate
  handles: a foreign handle resolves to `undefined`, which the private native
  boundary rejects as a JavaScript error before entering `dom`. The file must
  stay a classic script (no import/export at runtime, ECMAScript intrinsics
  plus `globalThis.bobcat` only — the realm has no
  `console`/`setTimeout`/DOM), which is also what lets Rstest import it for
  side effects and `tsc --noEmit` check it under
  `checkJs`.
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
  mutated only through `Document` methods). The **document element is
  permanent and pre-created**: `Document::new(device, root_tag, root_payload)`
  builds it at slot one (tag injected — the core still owns no tag
  vocabulary), `document_element()` returns it non-optionally, and it can
  never be detached or removed, so the document node's child list is
  structurally immutable after construction and no "empty document" code path
  exists in flush, layout, visual, or paint. Computed styles remain with the
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
  exclusively through that re-export; the root likewise re-exports `stylo` as the CSS
  vocabulary door for the layers above (strict linear chain: cli → core →
  element → dom). The embedder-facing `dom::Device` profile exposes exactly
  the inputs that vary between views — `Device::new(width, height,
  device_pixel_ratio)` — and locks the rest: screen media type, standards
  (no-quirks) mode, light color scheme, coarse touch pointers, and
  CSS-values-4 fallback font metrics. Quirks stays hard-wired in matching,
  the `Stylist`, and the doc-hidden `standards_device` test seam, so neither
  the quirks knob nor any stylo device vocabulary exists above this crate;
  view metrics read back through `Document::{viewport_size,
  device_pixel_ratio}`. `Headless::new` reports `NoAdapter`;
  every GPU-backed test treats that as a hard failure, including in CI.
  Nothing in `render` knows about nodes, computed styles, layout, or paint
  order. Source layout groups the crate by subsystem: `tree/` (arena set,
  `Node`, `Document`, shadow roots and the flat tree), `style/` (engine, Stylo
  traits, flush, invalidation, damage, containment), `layout/`, `visual/`,
  `paint/` (painter, walker, fragment painters), `scroll/`, `input/`, and
  `render/`.
  **Shadow DOM** (W3C, so W3C behavior) adds a fourth `NodeData` kind:
  `Document::attach_shadow(host, mode)` creates a shadow root attached to its
  host rather than listed among its children, so a host's child list stays its
  light children. Three trees then coexist. The **node tree** is what
  selectors match and what the public `Node` navigation reports; a combinator
  runs out of parents at a shadow root, and Stylo retries against the
  featureless host, which is what makes `:host` — and only `:host` — reach
  across. The **flat tree** (hosts replaced by their shadow trees, `<slot>`s
  by their assigned nodes, or by the slot's own children as fallback) is what
  Stylo traverses, what inherited values inherit through, and what layout,
  paint, and hit testing walk; it is reached exclusively through
  `Node::flat_children`/`flat_parent_id`, both of which return arena slices so
  every consumer keeps its `&[NodeId]` iteration. Each shadow root owns an
  `AuthorStyles<DocumentStyleSheet>` whose scoped `CascadeData`
  (`Document::add_shadow_stylesheet`) replaces the document's author rules
  inside that tree; `::slotted()` and `::part()`/`exportparts` work off the
  same data. Slot assignment is eager — every mutation that can change it
  (host child list, shadow-tree slot set, `slot`/`name` attribute) resolves the
  affected tree in the same call, gated on a live-shadow-root counter so a
  document with none pays one branch — but eager is not the same as
  recomputing the tree: appending a light child and removing one touch only
  the slot involved (the shadow root caches its slot list for that, rebuilt
  only when the slot set changes and debug-checked on every hit), and a full
  reassignment is reserved for the cases that can re-target more than one node.
  That split is benchmark-defended, not assumed: with the append path
  reassigning the whole tree, building a 1024-row host cost 51× the same rows
  with no shadow root, and 1.4× after
  (`benches/shadow.rs::build_wide_host_{plain,shadow}`; the whole bench file is
  paired plain-versus-shadow for exactly this reason). Per-node cost is one
  `Option<Box<ShadowLinks>>` word, allocated only for hosts, slots, and
  slotted nodes; the flat tree costs nothing on a no-op commit and ~1.02× on a
  frame. Recorded limits: `TElement::slotted_nodes` keeps Stylo's
  empty default (assignment changes dirty the host subtree wholesale instead
  of invalidating `::slotted` per slot), `:host-context()` is absent from the
  vendored selector grammar, and a node that leaves the flat tree keeps its
  last computed style and geometry — the same contract detached subtrees
  already have, and nothing renders it either way.
  **Custom elements** (W3C, so W3C behavior, within a deliberately narrowed
  scope) are the other half of the component model.
  `Document::define(local_name, Box<dyn CustomElement<T>>)` registers one
  handler per tag — a definition here is per-tag rather than the standard's
  per-instance constructor, because this crate has no script realm to hold
  instances in, so every callback names its element by `NodeId` and per-element
  state belongs to the layer owning `T`. The handler receives `constructed`,
  `connected_callback`, `disconnected_callback`, and
  `attribute_changed_callback`, the last filtered by an `observed_attributes`
  list read once at definition time.
  **Scope: user-agent components, not script-defined elements.** Definitions
  come from the engine layer above, never from application script, and
  `define` *requires* that every definition precede any element with its tag —
  it panics otherwise, since nothing later moves an element into a definition.
  That single contract removes the standard's entire upgrade half: no
  `undefined` state and therefore no `:defined` transition, no *upgrade an
  element*, no *try to upgrade*, no `define`-time document sweep, no replay of
  attributes an element already carried, and no *valid custom element name*
  predicate (whose only job was deciding whether a definitionless element
  counted as `undefined`). The document element is the one exception, because
  `Document::new` creates it before any definition can exist, so defining its
  tag constructs it. Restoring script-defined elements later is additive — an
  `undefined` state, an upgrade reaction, and a sweep — and moves neither the
  trait nor the dispatch contract.
  What the narrowing does **not** remove, and the thing to not assume is
  simpler than it is: reactions are still **queued, never called inline**, and
  drained at the end of the public mutation that raised them (the standard's
  `[CEReactions]` boundary), because a lifecycle callback mutates the tree
  while its handler lives inside the `Document` being mutated — as true of an
  engine-authored handler as of a script one. Dispatch clones an
  `Arc<dyn CustomElement<T>>` out of the registry rather than vacating the
  slot, which is what lets a callback on `x-row` create another `x-row` (the
  ordinary list shape) instead of hitting a re-entrancy panic. Scopes are
  watermarks into one flattened element queue while the per-element reaction
  queue is shared across them, which reproduces a browser's
  `A.disc, A.conn, B.disc, B.conn` for a subtree move. Three `Node` fields carry
  the definition pointer, the `Uncustomized`/`Constructing`/`Custom` state, and
  a conservative shadow-including-subtree summary; all fit in the existing
  tail padding (stride unchanged, asserted). The summary rejects a lifecycle
  walk at an ordinary subtree root and prunes ordinary branches when a walk is
  needed; insertion propagates it upward, while removal may leave harmless
  false positives instead of charging every ordinary mutation for exact
  descendant counts. Reaction scratch collects only constructed custom
  elements, so it is proportional to callbacks rather than all nodes visited;
  `Constructing` earns its byte by suppressing the reactions a constructor's
  own mutations would otherwise raise back at it. `:defined` is answered but
  never moves — with no `undefined` state it matches everything, which is why
  the `:not(:defined)` FOUC idiom is a script-defined-elements feature.
  Both a nesting depth and a per-scope fixpoint budget bound the drain, and
  both panic rather than hang. This is the crate's first self-authored `dyn`
  (the other two are mandated by upstream Stylo signatures), admitted by
  explicit user ruling because a document holds N behaviors keyed by N tag
  names discovered at runtime, which a type parameter cannot express; the
  `Send + Sync` supertrait is what keeps `Document<T>` `Send`. Benchmarked
  (`benches/custom_elements.rs`, three-way plain/unmatched/defined): a document
  that defines nothing pays 1.00× on a no-op commit and 1.01× on creation; the
  same suite's 4096-descendant `remove_element` cases defend the
  unmatched-definition negative fast path and the dense callback path separately.
  Further recorded limits: no `adoptedCallback` (no second document exists), no
  `connectedMoveCallback` (every move is disconnect-then-connect, the
  standard's own fallback), no customized built-ins/`is`/`extends`, no scoped
  registries, no `whenDefined`/`get`/`upgrade(root)`, and no `failed` state or
  construction stack — all of which exists to police a JavaScript constructor
  that can throw. `disconnected_callback` takes a shared `&Document`, not a
  mutable one: it is the only callback that runs with a free already committed,
  so a mutable handle would let it re-attach the subtree being freed, link a
  child to a node about to die, or free the node its caller still holds — three
  hazards every removal would then have to detect and refuse. A callback that
  *can* mutate may detach any node but may not *free* one the mutation that
  called it is still holding: `create_element` and the constructor call pin
  that id, `drop_element`/`drop_subtree` refuse to free a pinned node, because
  a `NodeId` is a slab key the arena recycles and a replacement would otherwise
  inherit it while every liveness check passed.
  Every node points directly back only to `TreeArenas`, and the
  same plain one-word `&Node` implements Stylo's document/node/element/shadow-root traits
  according to its `NodeData` (styling runs in place, no mirror tree),
  inline-style parsing, and a private per-document `StyleEngine` containing
  the `Stylist`, cascade pipeline, device, stylesheet set, and
  `SharedRwLock`. `Document::new` creates that entire context afresh, so
  different documents cannot share stylesheets. Author CSS enters either as
  text (`add_stylesheet`) or, for CSS a host already parsed, as rules the
  document itself builds — `build_style_rule` / `build_keyframes_rule` /
  `build_font_face_rule` mint an opaque `CssRule` branded with the lock that
  created it, and `append_rules` mounts a batch of them as one sheet, refusing
  any rule minted by another document. That keeps the `SharedRwLock`, the base
  URL, and stylo's own rule types inside the crate while letting the layer
  above skip the sheet, at-rule, and declaration-block parsers.
  The generic `T` payload remains associated with
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
  (public because decoding lives above `dom`, in `bobcat_core::image` and the
  injected decoder); the
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
  testing (`Document::elements_from_point{,s}` and input targeting, pure
  reads of the frame the last render retained, honoring `visibility`,
  `pointer-events`, border-radius, and inverse-matrix point mapping). It walks the same flattened box-tree the layout host feeds the
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
  Font registration takes the shared `FontBlob` resource through
  `Engine` → `Document` → `TextContext`; an owned loader
  buffer is moved into Parley without copying its payload, while
  `FontBlob::copy_from_slice` is the explicit copying fallback.
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
  surface, `rpx`-aware view/device policy, per-component css-id scoping,
  sticky lowering,
  component-specific staggered layout, and Lynx-specific text
  attribute/raw-text/truncation policy. Generic W3C text style, document
  context, and artifact storage already live in `dom`.
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
  personas already set up for this work. `packages/bobcat-element` with
  `bobcat-core`'s `tree` and feature-gated `quickjs` modules are the first
  pieces of this layer to land, joined by `StyleInfo` ingestion; the background
  thread, the event model, css-id scoping, and the remaining Element PAPI
  members are still ahead.

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

- `docs/lynx-xml-template.md` — the implementation-derived Lynx XML source
  format: exact restricted grammar, section extraction, errors and offsets,
  fixed template mapping, and the intentional CSS difference between the
  merged XML-to-`.web.bundle` encoder and the still-proposed raw web loader.
  `crates/lynx-xml` implements its source parsing boundary. XML is a source
  front end, not a third bundle encoding.
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

The Element PAPI runtime has two suites over the same file:
`pnpm --filter bobcat-element test` (Rstest, over a recording native mock) and
`pnpm --filter bobcat-element test:type` (`tsc --noEmit` under `checkJs`),
while `crates/bobcat-core/tests/main_thread.rs` drives the identical bytes
through the real QuickJS realm, `bobcat` object, and collector. Changing
`packages/bobcat-element/src/element-papi.js` triggers a `bobcat-core` rebuild
through `include_str!` — there is no generated artifact to refresh.

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
