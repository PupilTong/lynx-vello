# lynx-vello — Agent Guide

This is the canonical project/architecture doc for coding agents working in this
repo (Claude Code and Codex both start here — `CLAUDE.md` is a short pointer to
this file plus Claude-specific notes).

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
parts that remain unchanged. Tailor every diagram to the actual PR; remove all
template placeholders and verify that the Mermaid renders in GitHub's PR
preview before publishing or updating the PR description. Use
`.github/pull_request_template.md` as the minimum required structure.

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
  template parsing only, no JS runtime, no CSS engine (yet). The `StyleInfo`
  section is capped at 1 MiB and 64 levels of rule nesting, and is validated on
  a thread with a stack sized from its length — see "Input robustness at the
  two external-byte boundaries" for what those bounds prevent.
- `crates/lynx-xml` — zero-dependency, zero-copy parser for the restricted
  single-file Lynx XML source envelope. The current breaking grammar uses
  `<lynx engine-version="...">` plus `<script thread="main">` or
  `<script thread="background">`; the former `version` / `main-thread` /
  `background` spellings are
  rejected. It extracts the borrowed engine version, optional style, required
  main-thread script, and optional background-thread script, and reports the
  reference web parser's UTF-16 error offset together with a Rust-native UTF-8
  byte offset. Scope: source grammar and section extraction only — no input
  sniffing, I/O, configuration mapping, CSS parsing, bundle encoding, engine
  version negotiation, or runtime launch. It is a sibling of the binary
  template decoder, never another format inside it. The CLI and browser
  reference embedders own those omitted integration steps and consume this
  crate directly.
- `crates/bobcat-core` — unified native runtime core. Its public runtime is the
  opaque `LynxView` facade plus the protocol-only, host-injected
  `ResourceFetcher`, `ImageStore`, draw-target, OS-input, and
  lifecycle-wakeup capabilities. The script engine is deliberately *not* one of
  them: core owns its `QuickJS` realm outright, and the only script surface an
  embedder sees is the sanitized `script::ScriptError` a failure is reported
  with. A view is built from one `ViewSources` — an owned
  `Arc<dyn ResourceFetcher>`, owned font containers, an optional default font
  family, an optional `ImageStore`, author stylesheet URLs in cascade order,
  and the one entry MTS module URL — passed to the async `LynxView::new` with
  `PageConfig`, the device metrics, and the lifecycle wakeup. `new` validates
  the viewport, creates the one link, builds the painter in place on the
  calling thread, starts `bobcat-main`, and awaits one startup result. The
  wakeup is the only thing `new` is generic over, and `bobcat-main` is what
  holds it; the view itself names no type parameter. `bobcat-main` creates the document itself, registers its fonts and
  image store, awaits and mounts every stylesheet, awaits the entry module,
  creates QuickJS, and boots it before returning success; the fetcher decides
  where actual network or file IO runs, but every future continuation and
  document mutation remains on `bobcat-main`. A resource, font, realm, or boot
  failure yields `LynxViewError` and no view, and nothing later mounts a
  stylesheet or starts a second entry. Cancelling the unresolved `new` future
  drops pending resource work or stops startup before `QuickJS` begins, then
  releases the painter it built and directly joins `bobcat-main`; synchronous
  startup JavaScript is allowed to finish rather than being externally
  interrupted.
  The default family is prepended to the `system-ui`, `sans-serif`,
  and `serif` generic maps, so a Wasm embedder can supply its otherwise-absent
  system-font backend without baking a particular font into core; a name neither
  the containers nor the platform has fails with `EngineError::UnknownFontFamily`.
  Bundle retrieval, `.web.bundle` decoding, and config parsing are embedder
  responsibilities; core validates the entry module's source as UTF-8, registers
  its resolved URL in QuickJS's preloaded ESM graph. Successful construction
  has already completed boot; its `ScriptFinished` lifecycle edge remains
  queued for `pump`. The protocol's `fetch_style_sheet` answers with either
  CSS text or a `PreparsedStyleSheet` (`bobcat_core::style`) the host parsed
  itself, since a `.web.bundle` ships CSS a build step already tokenized and
  re-serializing it to a sheet blob is the startup cost the design rules out.
  Lowering it produces no stylesheet text: rules, keyframes, and font-face
  rules are built directly through `dom`'s branded `CssRule` builders, leaving
  stylo one selector-list parse per rule and one value parse per declaration —
  the floor, because the wire format keeps attribute selectors and functional
  pseudo-classes as text and stylo builds specified values only through its
  value parsers. Decoding a container stays embedder work: core owns the
  `PreparsedStyleSheet` vocabulary, and the embedder fills it. A request
  carries a specifier plus its optional base URL, not a semantic resource kind
  or transport hints: the embedder locates bytes by normalized resolved URL,
  while `fetch_style_sheet` selects the stylesheet payload contract. Other
  buffered loads use `fetch_resource`, and a `ResourceRequest` carries no
  response-size limit; each fetcher owns the memory bound for the response it
  materializes. Per-component css-id scoping is
  **not** implemented — every fragment mounts globally, which is what
  web-core itself emits for a `enableRemoveCSSScope = true` bundle. The
  document, tree, engine, and realm cannot be borrowed or decomposed from the
  facade.
  The crate-private `quickjs::ScriptEngine` is the whole script surface: it
  installs named host callbacks, registers named preloaded ESM source,
  evaluates a module through its TLA completion promise, calls an export the
  realm published back, and provides the GC seam. It is created on the
  engine-owned Lynx main thread and never leaves it, which is why nothing
  about it is `Send`. Values crossing it are `quickjs-rust-bridge`'s
  primitives-only `HostValue`/`HostArgument`, so realm values and DOM handles
  never cross as themselves. The private
  `MainThreadRuntime` owns the realm integration and the document together.
  Its `Rc<RefCell<TreeHandle>>` exists only so same-thread native QuickJS
  callbacks can borrow the owner; it is not a cross-thread sharing mechanism.
  Each `bobcat-internal:host` call is a plain owner-thread mutation, and
  `__FlushElementTree` runs the style + layout + paint commit and publishes one
  immutable `Arc<CommittedFrame>` to the painter. The document is never
  taken from, returned to, or observed by another thread.
  The core depends on `dom` and re-exports exactly one narrow seam of it: the
  `input` module republishes `dom::Point2D` and
  `dom::input::{InputEvent, InputKind, PointerId, PointerKind, PointerPhase}`
  so an embedder can name the input vocabulary without depending on `dom`
  itself. Wheel deltas crossing that seam are always viewport CSS pixels;
  conversion from physical-pixel, line, or page units is embedder policy.
  Nothing else crosses — no document, no node, no hit-test result —
  and that list is the whole of it. The private `Painter` the view owns
  retains the newest published frame and runs input routing, gestures,
  compositor scrolling, composition, and presentation inside the embedder's
  own calls; vsync interacts with the OS only there. Commands that require
  the live tree go to `bobcat-main`, which
  answers by publishing a later frame. A long JavaScript task therefore cannot
  stop scrolling or re-presentation of the retained frame, while a
  half-applied batch is unobservable because only commits publish. Embedders provide user input, device
  metrics, OS initialization, a draw target, and IO primitives, and relay
  OS facts in (`dispatch_input`/`resize`/`pump`/ticks);
  they never start or steer the pipeline. Engine events are enqueued and then
  wake the host's `pump` through the construction-time `EventRequester`;
  `ScriptFinished` preserves the successful entry-module boot edge after
  `new` has awaited it, `ScriptRunError` reports a fatal script-runtime failure
  during later owner-thread work, `ListenerFailed` reports a listener that
  threw during event delivery, and `TimerFailed` reports a `setTimeout` or
  `setInterval` callback that threw when it came due — the last two separate
  because neither is fatal: the walk continues, a repeating timer stays armed,
  the realm stays usable, and later events and timers are delivered as normal;
  a frame the engine wants drawn rides the same wakeup, and the `pump` that
  answers it is the turn that draws it — so no OS frame callback and no vsync
  round trip stands between a commit and its pixels. Pacing is the
  embedder's, and the engine names no interval for it: after each turn
  `owes_frame` answers whether the view still has a frame to put on its
  window — a running animation, a swap chain that had no image to give, a
  commit the turn did not draw — and a host takes that frame at **its own
  next display frame**, whatever its display clock is (a `CVDisplayLink` on
  the window's monitor, `requestAnimationFrame` in a Worker). `is_animating`
  is the narrower fact, answered for any target, that an offscreen host with
  no display to pace against asks instead. A draw that fails arrives once, as
  `RenderFailed`.
  **A view spans two threads**: the embedder's own — whichever one called
  `LynxView::new` — which owns the window, the input capture, the surface
  (the one call macOS allows nowhere else), and the private `Painter`
  (routing, gestures, scrolling, composition, and every GPU call), and the
  Lynx main thread (document + realm). The embedder picks the first by
  picking where it constructs the view, and the view can never leave it: the
  painter is `!Send`, so the view is too. That is what the browser always
  needed — `wgpu`'s handles are not `Send` under shared memory and an
  `OffscreenCanvas` cannot be transferred on again, so the Render Worker
  holds the view and each turn runs inside its `pump` — and now the only
  shape there is.
  **Each view brings its own Stylo pool.** `bobcat-main` builds one
  `dom::StylePool` — sized by `ViewSources::style_threads`,
  `StyleThreads::Auto` by default — as the first thing it does, on the thread
  that owns the document that pool will serve and before that document
  exists, and gives it to that one document. That is what lets a host put a
  view on each of several threads and have their restyles overlap. Stylo's
  bloom filter and style-sharing cache are per-OS-thread borrows held for a
  whole traversal, so a thread serving two traversals at once is an aliasing
  bug; disjoint pools make that unrepresentable rather than guarded, and the
  process-wide mutex that used to serialize every document's flush against
  every other's is gone with them.
  **`bobcat-main` is index zero of its own pool**, taken over in place by
  rayon's `use_current_thread` — which is why the pool can only be built on
  `bobcat-main`, and why `StyleThreads` counts it: `Fixed(3)` starts two
  threads, not three. Stylo's global pool did exactly this and Gecko relies on
  it, so a lone view restyles on the same threads, with the same parallelism
  and with the same inline root closure it had before these pools became
  per-view; the managed members take over only where a level is wider than the
  traversal's work unit. The takeover is permanent: rayon leaks about 25 KB per
  pool (the managed threads still exit on drop; the `WorkerThread` box and
  `Registry` do not) and refuses a second pool on the same thread forever,
  which is affordable only because `bobcat-main` is created for one view and
  dies with it. A host that replaces views — every `BobcatRenderer::load` —
  pays that 25 KB per replacement, in the same Wasm linear memory.
  `StyleThreads::Sequential` — and `Auto` where the pool would have held
  `bobcat-main` and nothing else — gives a view no pool at all and traverses on
  `bobcat-main` alone, which is a configuration rather than a fallback.
  `dom::MAX_STYLE_THREADS` is six, counted the same way Stylo counts its own
  six: a ceiling, not a tuning knob, because Stylo indexes its per-traversal
  thread-local storage by Rayon thread index into an array that long, so a
  wider pool is a construction error rather than a silent clamp — reported the
  way any other boot failure is, so no view exists for it.
  **Wasm takes the same path.** `navigator.hardwareConcurrency` reaches
  `StyleThreads::for_parallelism`, which is `Auto`'s own arithmetic, so
  comparable hardware gets the same pool on both targets and the facade does no
  thread arithmetic of its own.
  **The draw target is an argument to `new`, not something attached
  afterwards**: `DrawTarget::window(...)` takes anything convertible into
  `WindowTarget` — a `'static` surface target, so a windowing embedder passes
  a shared handle (`Arc<winit::Window>`) and a browser an owned canvas — and
  `DrawTarget::Offscreen` asks for a windowless GPU target instead. Either is
  built inside `new`, on the calling thread, while `bobcat-main` is already
  fetching; a view that exists therefore has somewhere to put a frame, and no
  state, error, or sentence has to describe one that does not.
  `FrameSize::for_viewport` exposes the physical size that construction will
  compute, for a host that must size the surface's backing store — a canvas —
  before it hands the target over.
  **Images are entirely the embedder's.** The core fetches, decodes, caches
  and retains no pixel of its own. The one resource system a view has — its
  `ResourceFetcher`, which is also its `dom::FrameImages` — is asked for one
  image at a time by source string (the `url(…)` value CSS produced, or a
  replaced element's source): named through `request_image`, answered
  through `ImageReports` with the intrinsic size layout needs, given its
  moment in every painter turn through `service_images` (where a host whose
  loads complete off-thread forwards them into the reports), and read back
  synchronously while the frame composes through `FrameImages::read`, which
  carries a `dom::ImageSizeHint` — the largest device-pixel extent the frame
  draws that source at, computed per draw from its extent under its
  transform and unioned per source — so a host decodes to the draw rather
  than to the file. No container sniffing, no codec contract, no cache
  policy and no byte budget lives in `bobcat-core` or `dom`; the reference
  implementation of all of that is `crates/bobcat-resources`, which both
  shipped embedders use. `LynxView::prefetch_images` warms sources ahead of
  the walk that would discover them. Automatic loading for the Lynx
  `<image>` element remains unwired, as does its element surface (`mode`,
  `placeholder` racing, `cap-insets`, `blur-radius`, `load`/`error`
  events).
  `Painter`, `LynxDocument`, `Viewport`, `new_document`, `MainThreadRuntime`,
  the startup owner/guard, and the concrete QuickJS adapter are all
  crate-private.
  The private `MainThreadRuntime`
  registers the native QuickJS ESM `bobcat-internal:host` (one Rust-backed
  named function export per member — `createPage`, `createElement`,
  `setAttribute`, `setInlineStyles`, `removeAttribute`, `getAttribute`,
  `tagName`, `attributeNames`, `childElementIds`, `parentNode`,
  `insertBefore`, `removeElement`, `replaceElement`,
  `swapElement`, `dropElement`, `flushElementTree`, `enableEventListener`,
  `disableEventListener`, `stopPropagation`, `setTimer`, and `clearTimer` —
  all but the last two speaking DOM vocabulary
  over numeric `NodeId`s; the two that answer with a list encode it in the
  return string, since the boundary's value type carries no array —
  `attributeNames` as the same length-prefixed record `setInlineStyles`
  accepts, and `childElementIds` as comma-joined ids, which need no length
  prefix because a decimal id cannot contain the separator), then registers the
  core-owned compatibility shell as `bobcat:runtime`, the Element PAPI
  runtime as `bobcat:element`, and the timer runtime as `bobcat:timers` in
  QuickJS's synchronous preloaded ESM loader.
  All three JavaScript sources live together in `packages/bobcat-element/src`
  and are embedded by core with `include_str!`. The Element module imports
  native
  operations directly from `bobcat-internal:host`; no host object and no
  element member is installed on `globalThis`. A `.web.bundle`'s
  `lepusCode.root` or
  raw XML main body becomes a real ESM at its resolved entry URL: core
  prepends named imports from both built-ins. The `bobcat:boot` ESM imports
  `lynx` from `bobcat:runtime`, `__FlushElementTree` from
  `bobcat:element`, and `bobcat:timers` for its effect — a static import, so
  the timer globals exist before the entry loads — uses top-level await on
  `import(entry_url)`, and then runs
  `processData` → (`globalThis.renderPage` when present, otherwise the
  `__RenderPage` event on `lynx.getEngine()`) → `__FlushElementTree` inside
  JavaScript; the global function is a compatibility path, not a boot
  requirement. The runtime module directly exports a `lynx` object, an empty
  `SystemInfo` snapshot, init/global props, context sinks, the native-module
  sentinel and empty JS event module,
  performance/error hooks, and
  `__OnLifecycleEvent`; transformed entries receive every binding through the
  prepended import, and the module installs none of them on `globalThis`.
  `lynx.getEngine()` returns one stable, realm-local `EventTarget`; its
  listeners never cross the host boundary and its only engine-driven delivery
  today is the boot fallback's `__RenderPage` event, whose `data` is the
  `processData` result. The other context sinks retain and deliver nothing,
  and the module does not invent the background-only `lynxCoreInject` realm.
  The PAPI runtime exports
  the supported Element PAPI only as named ESM bindings; transformed entries
  receive them through the prepended import:
  every ReactLynx Snapshot
  constructor except `__CreateFrame` (`__CreatePage`, `__CreateElement`,
  `__CreateWrapperElement`, `__CreateText`, `__CreateImage`, `__CreateView`,
  `__CreateScrollView`, `__CreateRawText`, `__CreateList`), all six tree
  mutation calls (`__AppendElement`, `__InsertElementBefore`,
  `__RemoveElement`, `__ReplaceElement`, `__ReplaceElements`,
  `__SwapElement`), the property surface a Snapshot's `create`/`update`
  functions write through (`__SetClasses`, `__SetID`, `__SetAttribute`,
  `__SetInlineStyles`) with the queries that read it back
  (`__GetID`, `__GetTag`, `__GetElementUniqueID`), the listener surface
  (`__AddEventListener`, `__RemoveEventListener`, `__StopPropagation`,
  `__StopImmediatePropagation`), and `__FlushElementTree`;
  `__SetInlineStyles` keeps the whole-value policy in JavaScript: a string is
  one `style` attribute write, while a record crosses in a single
  `setInlineStyles` call as a length-prefixed payload — `<utf16Length>:<text>`
  fields, name then value, in enumeration order — from which the host builds
  one declaration block from empty. Length-prefixing rather than delimiting is
  what lets a declaration value contain any character, a `;` included, without
  escaping or a guessed boundary.
  Ordinary camelCase keys are hyphenated, while case-sensitive `--*` custom
  property names pass through unchanged.
  The host operation implements the name/value subset of CSSOM
  `style.setProperty` (there is no priority argument, so an embedded
  `!important` is invalid) and intentionally has no numeric-style-id variant:
  numeric Lynx property ids belong to the
  separate, still-unimplemented `__AddInlineStyle` surface, not this PAPI;
  unsupported globals remain precise `ReferenceError`s, including
  `__DropElement`, which no web-core generation has.
  `__CreateList` consumes only its numeric parent-component argument for now;
  callback storage/execution remains part of the unimplemented list surface,
  and `__SetAttribute` throws for `update-list-info` — the one name that is a
  list command rather than an attribute — instead of writing a stringified
  command object onto the element.
  An element handle is an `EventTarget`. `__AddEventListener` /
  `__RemoveEventListener` keep the standard's registration identity
  (element, name, callback, capture) with its idempotence, `once`, and
  case-insensitive names; listener closures live only in the realm and die
  with their handle, so a registration cannot keep an element alive and
  nothing about a handler ever crosses into Rust. `__AddEvent`, `__GetEvent`
  and `__GetEvents` are **gone**: they stored a background-thread handler name
  and a worklet per (type, name) with overwrite semantics, and cross-thread
  event delivery is out of scope, so neither was deliverable. So are the parts
  of `__AddEventListener` that depend on them — `closure_type` selecting a
  handler string, and `bind_type` selecting Lynx's `catch` forms, which an
  author writes as a listener that calls `__StopPropagation` first.
  The realm tells the host which nodes are worth visiting: a listener list
  going empty-to-occupied calls the imported native
  `enableEventListener(node, capture, name)` and back calls
  `disableEventListener`, keyed by a weak
  `NodeId`→handle index cleared by the same sweep that drops the element. The
  host walks and calls the Element module's `__BobcatDispatchEvent` export
  through `quickjs::ScriptEngine::call_module_export`, the one Rust-to-JS path
  in the tree, once per node per pass, carrying an id naming the dispatch and whether
  the call is its last. Those two let the realm keep one event object for the
  whole walk, so a property one listener writes is there for the next, while
  the host retains nothing of the realm's. `stopImmediatePropagation` never
  leaves the realm, since it only skips the rest of one node's listeners;
  `stopPropagation` calls the imported native `stopPropagation`, which is a
  pure flag write because re-entering the realm from a host function would
  nest a `QuickJS` execution guard.
  `__SetCSSId` is absent rather than unimplemented — it names the author-CSS
  scope an element cascades in, and until a layer lowers a decoded `StyleInfo`
  into **scoped** author rules there is nothing to validate an encoding against
  (ingestion has landed, but mounts every fragment globally)
  (web-core writes `l-css-id`/`l-e-name` attributes; native Lynx keeps css_id
  on the element). It lands with the ingestion side that reads it, together
  with the parent-component css-id inheritance that feeds it.
  Creation calls return plain JavaScript handle objects minted by the PAPI
  runtime; each carries its DOM `NodeId` under a realm-local symbol and is
  registered with a `FinalizationRegistry` whose cleanup calls the imported
  native `dropElement`. **The handle is the one thing that holds its
  element**, and what keeps a handle alive while its element is on screen is
  the handle above it: every handle carries an unordered strong `Set` of its
  children's handles, maintained by the six tree mutations, and the page's
  handle is permanent, so every *connected* element's handle is reachable
  from it. The link the other way is the owner's node id, a number resolved
  through the same weak `NodeId`→handle index the dispatch side uses, so no
  parent/child pair is a reference cycle and an unreachable subtree is freed
  by plain reference counting. The set holds membership only: order is the
  native tree's, and mirroring it here would be a second answer to a question
  the tree already answers. `Document::drop_element` frees exactly the node
  the collected handle named — its **element** children are unlinked and go
  on as detached roots, each held by its own handle, while what no handle
  could ever name goes with it: the text node a `raw-text` reflects, and a
  host's shadow tree in full. So an unmount is `__RemoveElement` on the
  snapshot's root, which takes it out of its parent's set, and then the
  card's own references going away; the whole subtree's handles become
  unreachable together and each finalizes into one free. A ReactLynx list
  handing a recycled cell's elements between snapshot instances and deleting
  the old `__elements` array takes nothing away — those elements are
  connected, so their handles are held above them. Cleanup runs as a pending
  job at the job checkpoints, and pending jobs never run at realm teardown,
  which preserves the last committed tree. A collection comes from QuickJS's
  allocation pressure, or from the runtime itself: every
  `REMOVALS_PER_COLLECTION` removals, the batch that crosses the count ends
  with one, so the handles an unmount left behind are finalized — including
  any caught in a cycle, which reference counting cannot free — without
  waiting for allocation to reach the threshold. Per-handle realm state
  (listeners, `__AddEvent` handlers, the index bookkeeping, list callbacks)
  lives on the handle object under realm-local symbols rather than in a
  `WeakMap` keyed by it: QuickJS's `WeakMap` marks its values
  unconditionally, so a closure that captured its own element would otherwise
  keep the handle, and through it the whole subtree, alive for the life of
  the realm. No handle is ever minted after the first and there is no
  `retain`: a future query member that has to answer with a handle for a node
  whose handle has died must fail loudly, and so must a dispatch whose target
  has none — a connected element always has one, so a target without one is
  the ownership graph and the tree disagreeing.
  Core owns Lynx page policy in its `tree` module — the `page` root tag,
  `Viewport`/stylo `Device` construction, the Lynx UA cascade defaults, and
  the components the engine defines (`tree::raw_text`, one file per
  component, each owning its own UA rules and tests);
  the native host-module functions call `dom::Document` directly — while tag
  vocabulary, handle lifecycle, and the PAPI member surface live in
  `packages/bobcat-element`. Element identity is the DOM `NodeId`, which is
  also the element's Lynx `unique_id` — one number, issued by the DOM, never
  reissued; the JS side mints no ids of its own; the host
  boundary validates primitive arguments, live IDs, and tree-mutation
  preconditions before entering `dom`, returning misuse as a JavaScript
  exception (unexpected internal panics remain fatal on abort-only Wasm). An unflushed batch may
  present once its evaluation ends — web-core's visibility model, where
  the browser paints the live DOM regardless of `__FlushElementTree`.
  **Text** reaches the engine as an attribute and leaves it as a W3C text
  node, and `raw-text` is the join. Script writes a run with
  `__CreateRawText(value)` — a `raw-text` element carrying `text` — while
  everything downstream (Parley shaping, line breaking, the glyph painter)
  speaks the text node. So `tree::raw_text` defines a `dom::CustomElement`
  observing `text`, reflecting its current value into one text node the way web-core's
  `RawTextAttributes` does, updating that node in place rather than replacing
  it (a run keeps its retained Parley layout under its own id), and carrying
  no node at all for an empty value. The UA sheet carries the other half,
  again from `web-elements`: `text` is a flex container whatever
  `defaultDisplayLinear` says, `wrapper` is `display: contents`, and
  `raw-text` dissolves into the `text` it is written inside
  (`display: none` anywhere else) with
  `white-space-collapse: preserve-breaks`, the one place Lynx keeps a literal
  newline. **Not implemented**: an inline formatting context, so sibling runs
  in one `text` are separate flex items rather than one wrapped paragraph, a
  nested `text` is a flex item rather than an inline box, and `text-maxline`
  truncation is still absent — `docs/text-measurement-and-ifc.md` records the
  retained-layout and eviction contracts those would build on, and the open
  design decisions (brush, artifact ownership, truncation ordering).
  The resource module must not decode images/fonts/templates, upload render
  resources, or own cache/retry policy. Runtime configuration, raw realm/value
  handles, interrupts, and source-evaluation entry points remain private. The
  bridge owns only the generic synchronous preloaded source/native-module
  loader, loaded-module namespace access, and settled Promise inspection;
  Bobcat's specifiers, entry transform, graph membership, and boot policy stay
  in the core adapter.
- `crates/quickjs-rust-bridge` — owner-thread-bound safe Rust wrapper around
  the pinned `vendor/quickjs` submodule. It exposes QuickJS's two objects as
  two types: a `Runtime` (heap, atom table, job queue, execution limits,
  registered module source) and the `Context` realms created on it, as many
  as the host wants, all on the owning thread. Realms share what the runtime
  owns and nothing else — a `Value` never crosses between them, one
  registered module source compiles into a separate instance per realm, and
  native host modules are installed per realm under one runtime-wide
  specifier namespace. It owns the QuickJS C build and the
  narrow unsafe FFI shim, realm/value lifetime and affinity checks, exact
  ECMAScript string conversion, exception sanitization, pending-job pump,
  synchronous preloaded source/native-module loader, loaded-module namespace
  access, and module-evaluation Promise state.
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
  It also owns the **host-function seam**: `Realm::function`,
  `define_global_function`, and `register_host_module_function` back a JS
  callable with a Rust `FnMut`, dispatched
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
- `crates/bobcat-resources` — the cross-platform reference resource system:
  one `ResourceFetcher` for macOS, Linux and the browser, which both shipped
  embedders use instead of the in-memory fetchers they used to carry. It is
  the worked example of what the protocol expects, not part of the
  protocol, and core stays exactly as free of resources as before. Four
  things live here and nowhere else in the workspace. **Transports**:
  contents the embedder registers under any URL (`Resources::register` and
  `register_style_sheet` — a decoded bundle's scripts and `StyleInfo` sheet,
  a browser-fetched script's bytes, a test's PNG), `data:` URLs, `file:`
  URLs natively, and `http(s)` through the platform's own client: libcurl
  loaded at runtime with `libloading` on macOS and Linux (no build-time
  link, no bundled HTTP or TLS stack; a host without it gets a precise
  `Unavailable`), and the Render Worker's `fetch` in the browser.
  **A MIME-keyed preprocessing pipeline**: every payload is sniffed (image
  magic beats the label, a label beats a byte scan, a BOM names a charset),
  classified, and treated by class — text transcoded to UTF-8 with its BOM
  removed so the engine's strict validation sees what a browser's decoder
  would have produced, JSON validated, images container-sniffed and
  header-probed for their intrinsic size without decoding a pixel, the rest
  passed through. **Tiered caching**: decoded bitmaps in a memory tier under
  a byte budget with the frame's working set pinned against eviction, and
  fetched bytes in a disk tier under its own budget with RFC 9111
  freshness, `ETag`/`Last-Modified` revalidation, and the fetch cache modes
  mapped from `CachePolicy` (natively; the browser's HTTP cache plays that
  role there). **Platform image decoding**: no codec is compiled in —
  `ImageIO` on macOS (`CGImageSourceCreateThumbnailAtIndex` with a maximum
  pixel size, so a photo shown small is decoded small), gdk-pixbuf on Linux
  (loaded at runtime; `gdk_pixbuf_loader_set_size` from the header probe),
  and the main thread's `Image` element in the browser (the Render Worker
  fetches the bytes and hands them over as a Blob), each asked to downsample
  during decode. Loads complete on the crate's own
  worker threads (local tasks in the browser), are delivered through the
  wakeup the embedder supplies, and are applied in the painter's next turn
  through the protocol's `service_images` hook. The frame reads each image
  with the size it draws it at: a resident bitmap far larger than its draw
  is re-decoded at the drawn size in the background and replaced, one that
  was evicted is restored inside the read from the retained bytes or the
  disk tier, and one drawn larger than it was decoded is refined back up as
  long as the image has more to give. In the browser that restore is the one
  place the Render Worker blocks: the main thread never waits, so a job's
  mailbox in shared Wasm memory and `Atomics.wait` are what let a read that
  must not miss wait for it (`crates/bobcat-wasm/image-decoder.js` is the
  main thread's half). Shape: `Resources` is the shared system (registry, caches,
  workers, decoder; cheaply cloned, bound to the painter's thread) and
  `Resources::builder` yields the per-view `ViewResources` that
  `LynxView::new` takes and that carries that view's `ImageReports`.
  Recorded limits: only an image's first frame is decoded (no animated
  playback), no `region-to-decode`, no `blur-radius` post-processing, and no
  `<image>` element surface — the pipeline serves whatever source string the
  paint walk names, today `url(…)` layers and `Document::set_image_source`.
  The macOS decoder is type-checked against the Apple target but exercised
  only where ImageIO exists; the Linux decoder and libcurl transport are
  tested for real against the system libraries, and the browser path is
  linted for wasm32 and exercised only in a browser.
- `crates/bobcat-cli` — the independent native `bobcat` product over
  `bobcat-core`. Its workspace dependencies are
  `bobcat-core`, `bobcat-resources`, the sibling `lynx-template-decoder`
  utility, and the `lynx-xml` source parser.
  `bobcat -i file:///…` content-sniffs and boots either one web bundle or one
  raw Lynx XML source card; other URL schemes remain rejected at the boundary.
  The CLI is an **embedder** of the opaque `bobcat_core::LynxView`: it owns
  argument parsing, input bytes, bundle decoding or XML parsing, `PageConfig`
  mapping, the reference resource system with the extracted scripts/styles
  registered, the winit window and event loop, device metrics, input
  translation, the stdin prompt, and PNG writing — and nothing of the
  pipeline. Every event handler is a relay into the view
  (`dispatch_input`, `resize`, `pump`, clock ticks in
  headless mode); the engine owns the tree, commits, scheduling, and its
  script thread. The window it hands `LynxView::new` as a `DrawTarget` is the
  draw target and nothing else: frames and lifecycle
  events alike wake the event loop through the injected `EventRequester`, and
  the turn that wakeup opens ends in `about_to_wait`, which draws — winit's
  `RedrawRequested` is not relayed at all. Drawing there rather than in the
  relays coalesces a turn's events into one frame and keeps the frame's vsync
  wait out of winit's proxy-event drain, which iterates until empty. A running
  animation is no wakeup at all: `about_to_wait` polls while
  `LynxView::is_animating`, paced by the swap chain's vsync. The CLI gives one
  `LynxView::new` its author CSS and entry MTS URL as a `ViewSources`, reports
  any resource or TLA boot failure as `CliError::StartView`; after successful
  construction it consumes the preserved `ScriptFinished` edge and any later
  `ScriptRunError` through `pump`. Headed
  mode names the window as that view's draw target; headless mode names
  `DrawTarget::Offscreen` and relays synthetic
  vsync ticks — whether a tick becomes GPU work is the engine's decision.
  The CLI's resource system is `bobcat-resources`: the decoded input's
  scripts and stylesheet are registered under `bobcat-memory://` URLs, the
  input's own `file://` URL is the base every relative `url(…)` resolves
  against, and a disk tier lives under the user's cache directory — so a
  page's images, beside the input, inline as `data:`, or on the network,
  load and decode through the platform. A load completing on a worker wakes
  the event loop exactly as a commit does.
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
  importing — and registers it in the fetcher under the URL both runners name in
  `ViewSources::style_sheets`. A bundle carrying non-zero fragment ids warns that
  per-component scoping is not implemented rather than claiming compatibility.
  For XML, a present `<style>` body instead uses the fetcher's raw CSS-text arm
  and the fixed page configuration is `false`/`false`/`true` for default
  linear display, visible overflow, and selector support. A present background
  section is retained under `/app-service.js` and warned about, but not
  executed until Bobcat has a background-thread realm.
- `crates/bobcat-wasm` — the pure-Rust `wasm-bindgen` browser embedder and npm
  facade, built for `wasm32-unknown-unknown` with shared memory. The browser UI
  thread is a JavaScript-only host coordinator: it creates one explicit
  embedder Worker and transfers an `OffscreenCanvas`, but never instantiates
  Wasm or owns engine state. That Worker initializes the module, constructs one
  opaque `LynxView` per page through `BobcatRenderer::load`, permanently owns
  every thread-affine GPU object — crates.io Vello 0.9/wgpu 29 Device, Queue,
  Surface, Renderer, and OffscreenCanvas — and uses `wasm_thread` to create its
  nested Lynx main/VM Worker. That Worker in turn spawns its own view's Rayon
  style Workers the same way, with `wasm_thread` as the spawner, leaving the
  vendored Stylo sources unchanged. Core creates its owner-thread-bound QuickJS realm
  inside that Worker; Element-PAPI
  batches, Stylo/Rayon, layout, and
  render hand-off then synchronize through Rust channels, mutexes, atomics,
  and the shared Wasm memory exactly as in a native embedder. JavaScript
  `postMessage` is only the browser host boundary (initial Canvas transfer,
  URL-based script requests/results, resize/input/lifecycle) or a library's
  Worker bootstrap control plane; it is not a DOM/render reconciliation
  protocol. URL requests are serialized, and a lost-wake-safe `EventSignal`
  Promise wakes script completion independently of Worker rAF, so a hidden page
  may pause drawing without stranding the `load` Promise. The UI facade, nested
  VM Worker startup, and built-in QuickJS configuration impose no wall-clock
  deadline on loading or execution. QuickJS drains its owned pending jobs and
  waits for the TLA boot module's evaluation Promise to settle at its host
  checkpoint; there is no browser microtask-completion protocol. The
  underlying QuickJS bridge retains an
  opt-in execution timeout for its direct users and tests.
  A Wasm instance owns nothing of Stylo's but the Worker bootstrap
  `configure_wasm_workers` installs — one script URL, which is what every
  Worker a view spawns is made of — while each `LynxView` owns its own
  Lynx-main Worker, style Workers, QuickJS realm, document, and endpoints just
  as a native view does. Every public `BobcatCanvas` gets a separate Render Worker
  and Wasm instance; a renderer holds no view until `BobcatRenderer::load` builds
  one, and each later load replaces the current native `LynxView`. Dropping the
  view explicitly stops and joins its Lynx-main Worker after the Worker drops
  the document and thread-bound QuickJS realm; replacement construction starts
  only after that teardown. The
  transferred OffscreenCanvas, module instance, configuration, latest metrics,
  resource provider, registered font containers, selected default font family,
  and Stylo worker *count* are the renderer's own, reapplied to each view it
  builds; the workers themselves belong to the view and retire when it is
  dropped, and a load clears the registered script and stylesheet bytes once
  copied. Every style Worker is a managed one: the Render Worker is not a pool
  member and neither is the view's Lynx-main Worker, which enters traversal
  from outside the pool so Stylo transfers its root closure onto a managed
  worker. `BobcatRenderer::create` therefore takes a count of one to
  `MAX_STYLE_THREADS` dedicated style Workers, and the facade asks for the
  machine's threads less the Render and Lynx-main Workers. The UI never
  blocks, while Worker-side Rust may block wherever the native runtime does.
  The browser target enables `parking_lot_core/nightly` so transitive
  Stylo/wgpu parking_lot locks use Wasm atomic wait/notify instead of the
  non-atomic Wasm backend that panics on contention.
  Release packaging pins Binaryen 132 through the JavaScript workspace and
  runs `wasm-opt -Oz` after wasm-bindgen with an explicit mirror of every
  enabled Rust/LLVM Wasm feature; the build rejects a different optimizer
  version instead of accepting wasm-pack's older fallback. Package
  verification requires the optimized module to omit its debugging `name`
  section while retaining `target_features`.
  Browser builds disable Parley's `complex-scripts` feature to avoid embedding
  ICU's multi-megabyte CJK and Southeast Asian dictionaries; native targets
  retain it. Grapheme segmentation, shaping, and ordinary Unicode line
  breaking remain available, while Thai, Khmer, Lao, and Myanmar text may use
  cluster-level emergency breaks and report a larger intrinsic minimum width.
  `wasm_thread` is pinned to the upstream
  `spawn_from_worker` change because its crates.io release otherwise forwards
  nested spawns to a parent protocol handler that an explicit embedder Worker
  does not have; Chrome 135 supports the resulting nested module Worker.
  Page sources still arrive through the Render Worker's own `fetch`: it
  registers the raw stylesheet and entry-MTS bytes with the
  `bobcat-resources` system it owns and calls
  `BobcatRenderer::load(entry_url, style_sheet_urls)`; the entry's final
  response URL is the ESM specifier imported by `bobcat:boot` and the base
  its images resolve against. Images a page names are fetched by the
  resource system itself through the same Worker `fetch` and decoded on the
  main thread by an `Image` element in the package's `image-decoder.js`,
  over a `MessageChannel` whose Worker end the facade hands to
  `BobcatRenderer::create` at init.
  `loadLynxXml(url)` fetches an XML envelope once, decodes it with the web
  loader's replacement-mode UTF-8 behavior, parses it with `lynx-xml`, and hands any
  raw stylesheet and its main-thread body to the same `load`; both are repeatable. The
  exported `LYNX_XML_PAGE_CONFIG` names the source format's fixed page defaults;
  a host may still deliberately override them.
  The optional background body is reported by URL and neither retained nor
  executed, matching the runtime limitation above.
  Transferring the canvas does not transfer its DOM event target, so the
  `BobcatCanvas` facade retains that element and automatically forwards active
  `pointerdown`/`pointermove`/`pointerup`/`pointercancel` sequences. It claims
  each accepted pointer, maps client coordinates through the canvas bounds
  into viewport CSS px, and sends compact fire-and-forget records through the
  same ordered Render-Worker queue as load/resize. The Worker stamps input
  with its own `performance.now()` before `BobcatRenderer` writes the shared
  manual clock and calls `LynxView::dispatch_input`; this keeps gesture time on
  the Worker rAF timeline and prevents an idle frame clock from making
  `longpress` fire immediately. Each load clears active captures, disposal removes
  all listeners and restores the canvas's prior inline `touch-action`, and
  unexpected capture loss becomes `pointercancel`. Hover moves, secondary
  mouse buttons, and wheel input do not cross the boundary.
  The facade exposes no create/append/drop/flush,
  document, tree, or engine API. It does not decode `.web.bundle` containers;
  callers supply `PageConfig` and either executable script URLs or a raw Lynx
  XML URL. Synchronous GPU
  capture is likewise absent because
  browser WebGPU completion is Promise-driven.
- `packages/bobcat-element` — the dependency-free JavaScript sources for the
  three ESMs `bobcat-core` preloads into the QuickJS main-thread realm:
  `src/main-thread-runtime.mjs` provides `bobcat:runtime`,
  `src/element-papi.mjs` provides `bobcat:element`, and `src/timers.mjs`
  provides `bobcat:timers`. Core embeds all three with
  `include_str!`; the Rstest suite imports the Element PAPI's identical bytes
  and verifies every named export. The package owns the
  supported `__*` PAPI members and their web-core arities,
  plus the Lynx tag vocabulary
  (`wrapper`/`text`/`image`/`view`/`scroll-view`/`raw-text`/
  `list`). It also owns the value coercions web-core gets from the HTML DOM for
  free: truthiness-not-null clearing for classes, ids, and inline styles,
  `String(value)` for every attribute, and camelCase-to-kebab hyphenation of a
  record-shaped inline style. It also owns the event half: a handle is an `EventTarget`, its
  listeners are closures filed on the handle itself under a realm-local
  symbol — so a registration can never keep its element alive, and QuickJS's
  non-ephemeron `WeakMap` never gets the chance to — and the per-node
  dispatch, the standard's `eventPhase`, and `once` are all resolved here,
  with only enable/disable and `stopPropagation` crossing to the host.
  An element handle is a plain object carrying its DOM `NodeId`
  under a realm-local symbol (web-core's `uniqueIdSymbol` shape) — one
  object per element for its whole life, so every PAPI return of an element
  yields the same object.
  `parentComponentUniqueID` and `__CreatePage`'s arguments are accepted for
  PAPI shape and unused. Lifecycle: collection is the only way a handle
  lets go of its element — web-core's model, where a swept `WeakRef` is
  what ends a wrapper. Every non-page handle is registered with a
  `FinalizationRegistry` whose cleanup calls the imported native
  `dropElement`, which frees that element and nothing else; cleanup runs as
  a pending job at the host's job checkpoints, and never at realm teardown,
  which preserves the last committed tree. Keeping a connected element's
  handle alive is this layer's own job, through the per-handle child set
  described above. The JavaScript layer
  deliberately does
  not validate handles: a foreign handle resolves to `undefined`, which the
  private native boundary rejects as a JavaScript error before entering
  `dom`. Native access is limited to named imports from the native
  `bobcat-internal:host` ESM; the realm has no `globalThis.bobcat`, no
  `console`, and no DOM. Named exports are the only Element-PAPI surface
  for transformed MTS entries; the module installs no `__*` globals. Rstest
  imports the ESM normally and `tsc --noEmit` checks it under `checkJs`.
  `src/timers.mjs` is the one module here that does install globals, because
  bare `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval` are how a
  card reaches them. It keeps only the callbacks, filed under the id the
  host's `setTimer` hands back; the schedule, the clock, and HTML's `long`
  delay conversion and nesting clamp are `bobcat-main`'s, which is also what
  waits on the deadline and calls the module's `__BobcatRunTimer` back.
- `crates/dom` — generic W3C-DOM-subset document tree and
  standards-oriented CSS computation core. `docs/dom-public-api.md` is the
  authoritative normal-build versus test-feature API boundary. It owns a
  fixed-address boxed
  `TreeArenas<T>` containing two `Slab`s: a primary `Slab<Node<T>>` (slot
  zero is the real DOM Document node and carries its node-visible style
  context; later slots are element/text nodes) plus a slot-aligned payload
  slab. A separate inline
  `DocumentLayoutState` owns slot-aligned layout state — a lazily sized
  vector rather than a third lockstep slab: creating a node costs nothing
  there, a node that is never laid out never allocates layout state,
  `Document::layout`/`render` size it to the arena's slot bound in one
  resize, and an absent entry reads as "never laid out" (an empty layout
  cache) everywhere else.
  Identity and storage are deliberately split (`tree/arena.rs`): a `NodeId`
  is an index into `TreeArenas::slots`, a `Vec<Option<NodeSlot>>` whose entry
  holds the arena slot the node's state actually occupies. Ids only count
  upward and a freed one is never reissued, so a `NodeId` names one node for
  the life of the document and a stale id resolves to nothing rather than to
  a stranger — which is why the tree carries no generation counters and no
  epoch gate on the retained frame, and why the same number can be handed to
  script as Lynx's element `unique_id`. The arena slot *is* recycled, so
  only the four-byte table entry leaks, per node ever created. The raw slabs
  are private to `tree/arena.rs`: nothing else can index one with a `NodeId`,
  which is what keeps the split enforced rather than merely documented. Stylo's
  per-element style data (the upstream `ElementDataWrapper`, no outer cell)
  and its traversal/invalidation flags live inline on `Node` (bench-defended
  2026-08-03: the paired A/B showed no traversal regression and a measurably
  faster no-op-commit fast path). The
  primary slab selects each raw-`usize` ID; the payload slab allocates and
  removes in lockstep with it and asserts it received that same key (it
  reserves a payload-less sentinel at document slot zero), while the layout
  vector resets a freed key's entry so the key's next occupant starts clean
  (ONE TREE policy: nodes are created and mutated only through `Document`
  methods). The **document element is
  permanent and pre-created**: `Document::new(device, root_tag, root_payload)`
  builds it at slot one (tag injected — the core still owns no tag
  vocabulary), `document_element()` returns it non-optionally, and it can
  never be detached or removed, so the document node's child list is
  structurally immutable after construction and no "empty document" code path
  exists in flush, layout, visual, or paint. Computed styles remain with the
  primary nodes; layout/text state does not. The crate's entire `unsafe`
  surface is two blocks — the arena backpointer deref and the
  `TElement::ensure_data` contract call — plus the `unsafe fn` signatures
  Stylo's traits mandate (all bodies safe). Both blocks carry a `SAFETY`
  comment stating the invariant they rest on, and a crate-local
  `#![warn(clippy::undocumented_unsafe_blocks)]` keeps that true.
  `Document<T>` also owns one private concrete `Painter`, including its
  reusable walk scratch and retained `vello::Scene`, plus the
  embedder-installed `Arc<dyn ImageStore>` the walk reads.
  `render` privately builds `PaintOrder` and invokes that painter
  only for a dirty scene. The Painter records which private visual epoch its
  scene represents, so `render`/`needs_render` own retained-scene scheduling without
  publishing that epoch. `scene` lends a guarded shared borrow, while
  `set_image_store` and `note_images_changed` are the narrow image seams and
  invalidate the scene conservatively. There is no renderer type parameter,
  `DocumentRenderer` trait, `with_renderer`, public Painter, public visual
  epoch, or public paint-order constructor. The crate also owns the DOM-free
  render floor absorbed from the former `pulsar` crate (2026-08-04): the
  `render` module holds the `ImageStore` trait (re-exported at the crate
  root) and the `render::gpu` wgpu render-to-texture/readback backend
  (`gpu::Headless`, plus the `read_texture`/`renderer_options`/`render_params`
  seams windowed embedders build against); the crate root re-exports the one
  workspace `vello` version, and embedders configure wgpu/peniko/kurbo
  exclusively through that re-export; the root likewise re-exports `stylo` as the CSS
  vocabulary door for the layers above (strict linear chain: cli → resources
  → core → element → dom). The embedder-facing `dom::Device` profile exposes exactly
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
  freeing retires the id permanently and the mutation would otherwise link in
  and return a handle that already names nothing.
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
  (`debug_assert` + panic on stale handles rather than silent no-ops).
  A document's style traversal runs on the workers
  `Document::set_style_pool` gives it and on no others: one `StylePool` per
  document, moved in rather than shared, which is what lets two documents
  restyle at the same time with nothing serializing them. A document that was
  never given one — every test and benchmark here, and any embedder asking
  for a sequential view — traverses on the thread that flushed it, so the CSS
  benchmarks measure cascade and matching work rather than Rayon dispatch.
  Style
  flush and its per-node `StyleDamage` (repaint / stacking / overflow /
  relayout classes) are internal parts of `Document::layout`; harvested
  damage is then **cleared** (the fix for stylo's never-cleared-damage
  re-traversal bug). During that same harvest,
  relayout-class damage is consumed immediately into boundary-stopped layout
  cache invalidation, so no external damage report is needed to preserve
  layout work; the module also owns the
  `effective_containment` fold (`contain` + `content-visibility` → effect
  bits). Layout invalidation stops early at the deepest ancestor whose
  committed input hughie marked **content-independent** — the committing
  parent proved the input's known dimensions, parent size, and available
  space cannot move when only that subtree's content changes (pure-length
  sizing, stable percentage bases, imposed stretch, content-free automatic
  minimums, chained from the viewport-anchored root input; distinct from CSS
  *definiteness*, which admits content-measured sizes). `run_layout` relays
  such a subtree in place under the stored input and accepts the result only
  when the output reproduces bit for bit — anything else escalates to the
  whole-tree pass, which reuses the caches the attempt just filled. This is
  what makes the ReactLynx steady state (text/attribute updates inside
  fixed-size rows under the `page { width/height: 100% }` UA anchor) cost a
  subtree instead of the document; `contain: strict` boundaries keep their
  parked-relayout path as the containment-guaranteed special case of the
  same machinery. Second and later invalidations in a batch stop at the
  first already-cleared ancestor, so a burst of mutations pays one spine
  walk, not one per mutation. Equivalence tests
  (`tests/incremental_relayout.rs`) pin every path — in-place, escalated,
  and root-reaching — to the geometry of a fresh document built directly in
  the final state.
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
  `Document::set_natural_size` and `Document::set_image_source` are the public
  replaced-content update seams (public because both halves arrive from above
  `dom`, out of the embedder's `ImageStore`, independently and in either
  order). The natural size always invalidates layout; a source invalidates only
  the scene *unless* it is the call that makes the element replaced, because
  being replaced forces `DisplayMode::Leaf` and hides every child — a layout
  input, not a paint one. Both getters stay paint/layout-internal, setting an
  equal value is a structural no-op, clearing a source an element never had is
  a no-op rather than a conversion to a replaced leaf, and the DOM core still
  knows no tag names.
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
  `crates/dom/src/paint/painter.rs`; DOM-aware paint tests and the paint benchmark
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
  test literal all produce equally, and `Document::route_input(InputEvent)`
  is a pure read that reports the node the event hit through the rendered
  frame. The crate has **no default-action machinery and no recognizer**:
  deciding and driving the user-agent scroll belongs to `bobcat-core`'s
  input router (`gesture.rs`), which calls `scroll_by`/`scroll_chain` —
  whose unconsumed remainders exist for exactly that caller — and there is
  no second dom consumer a duplicate would serve.
  `InputEvent::default_prevented` is the `preventDefault()` seam an embedder
  hands to that router after its own arbitration; this crate never reads it.
  Its `event` module is the other half, and it does **not** dispatch:
  `Document::event_steps(target, bubbles, composed)` returns the ordered node
  visits one event resolves to — the capture pass root-inward, the bubble pass
  target-outward, the target in both — as plain `Copy` `EventStep`s owning no
  borrow. Path construction is the standard's, including its shadow rules: a
  slotted node's event parent is its assigned slot, a shadow root's is its
  host, `composed` gates the crossing, and crossing retargets so every step
  from the host outward reports the host. The shadow-crossing test is a single
  comparison rather than the standard's per-step ancestor walk, and the
  equivalence is argued in `event_path`'s doc comment and pinned by a
  differential test.
  Dispatch itself belongs to `bobcat-core`, split across its two threads
  because the realm cannot move and scrolling must stay responsive: the
  painter routes the input (`route_input`, one hit test), feeds it
  to the input router — which decides the user-agent scroll and every event's
  type and target in one place — executes those decisions in order, and sends
  each emitted event's type, target, and detail to the script thread. The
  script thread builds its path from the exclusively owned
  document and delivers it there — that order is what lets a listener mutate
  the tree. Nothing guards the window in between: a `NodeId` names one node
  for the life of the document, so a step
  that outlived its node resolves to no handle and reaches no one, and no later
  element can take its place. There is no `preventDefault` and no
  cancelable event anywhere on this path — Lynx dispatches none — so
  suppressing a user-agent default action stays gesture arbitration's job,
  arriving on the separate `InputEvent::default_prevented` seam.
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
  canonical `lynx` branch, tip `18d8981f2`): `contain` was already seeded
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
  Four commits have landed on `lynx` since #12, and the tip above is the last
  of them: fork PR #14 requires `Send` of `FontMetricsProvider` implementations
  (what lets a `Document` cross to the presenting thread at all), fork PR #13
  corrects `ElementData` reference documentation, fork PR #21 moves the
  `display` longhand's initial value from `inline` to `Display::initial()`,
  which under the `lynx` feature is `flex`, and the `-lynx-text` patch adds
  `DisplayInside::LynxText` — the block-level, **non**-item-container value
  naming one flattened Lynx paragraph. It is the cascade's way of saying what
  Lynx says structurally (`TextElement::OnNodeAdded` converts every added
  child; no author CSS can undo it), so a `<text>`'s subtree is inline content
  rather than child boxes. The variant carries
  `#[css(keyword = "-lynx-text")]` because the derived `DisplayInside` `ToCss`
  would otherwise kebab-case the variant name and silently drop the vendor
  prefix. Read that last one against the
  paragraph above rather than as a contradiction of it: the *initial* value is
  what an element computes to with no declaration reaching it at all, while
  Lynx's `display: linear` default is a UA-sheet declaration this embedder
  cascades. Confirm the tip with `git -C vendor/stylo rev-parse --short HEAD`
  before trusting this line — the gitlink moves and the prose does not.
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
  (single-axis / container queries out of scope). `LayoutInput` additionally
  carries per-axis `content_independent` flags — input *stability* under
  subtree content change, proven by the committing algorithm (flexbox sets
  them; the root input is viewport-stable by construction; other algorithms
  conservatively leave them false) and stored beside the committed cache
  entry, so a host can relayout a subtree in place under its stored input
  and verify by output comparison. The per-node measurement cache keeps its
  eight-slot budget but inlines only two slots, spilling to the heap for the
  nodes whose containers probe many constraint shapes. Read
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
  component-specific staggered layout, and the rest of the Lynx text policy —
  `text-maxline`/`text-maxlength` truncation, `tail-color-convert`, and the
  inline formatting context sibling runs in one `text` would need. The
  `raw-text` attribute-to-text-node reflection and its UA display/newline
  policy have landed in `bobcat-core`'s `tree::raw_text` (see above). Generic W3C
  text style, document context, and artifact storage already live in `dom`.
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
  Its `TestImages` is the in-memory `dom::ImageStore` the image suites install
  on a document before capture — the only image store in this workspace, and
  deliberately a test double: it fetches nothing, decodes nothing and evicts
  nothing. `capture_document` takes no store of its own, because the document
  it renders already carries the one an embedder installed. `headless` requires a usable GPU adapter and panics when one is
  unavailable, so local and CI test runs obey the same mandatory-GPU policy.
  DOM-aware screenshot suites live in `dom`, which also keeps the direct GPU
  smoke tests. Goldens are not platform-suffixed: cross-platform
  rasterizer noise is absorbed by tolerance, not by per-platform baselines.
- *(planned, not yet scaffolded)* the remaining runtime crates — see
  `docs/tracking/` for the behavior surface each will need to cover before
  scaffolding begins, and `.claude/agents/` for the subsystem-scoped agent
  personas already set up for this work. `packages/bobcat-element` with
  `bobcat-core`'s `tree` and `quickjs` modules are the first
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
  change, or the next fork commit ships unrelated reformatting. Use
  `./.github/scripts/fmt-check.sh` instead — it is what CI runs, it names the
  members from `cargo metadata` rather than from a list someone has to
  remember to extend. The hand-written list it replaced had been missing
  `lynx-xml` since that crate was added.

## Testing

Integration tests decode real fixtures vendored from lynx-stack under
`crates/lynx-template-decoder/tests/fixtures/` (Apache-2.0 build artifacts).
`cargo test` must pass on the pinned nightly toolchain.

### Input robustness at the two external-byte boundaries

`lynx-template-decoder` and `lynx-xml` are the crates fed bytes the engine did
not produce — a downloaded `.web.bundle` and an authored `.lynx.xml`. Both are
written in the `Result` style and both have grammar tests, but every input in
those tests is one a *correct* encoder produced, which cannot establish the
property that matters at a trust boundary: that no input takes the process
down. The two crates answer that differently, because their exposure differs.

**`lynx-xml`** carries `tests/robustness.rs`: a fixed-seed character-level
mutator over seed documents, plus named degenerate cases for every construct
with a terminator, asserting panic-freedom and two invariants a partial-index
bug would break silently — the returned sections borrow from the source, and a
`ParseError` offset lands on a real UTF-8 boundary (which keeps the crate's own
`debug_assert!` live). It ends on a coverage floor: if a grammar change made
*nothing* parse, the success-branch assertions would quietly stop running and
the test would still pass, so it fails instead. 20 000 inputs in under a tenth
of a second, in the ordinary suite.

This is deliberately not a fuzzer. Coverage-guided mutation buys little on a
543-line zero-dependency parser over `&str` — the input is already valid UTF-8,
there are no length fields, and nothing allocates on a source-controlled count
— and it is not worth a separate package and a scheduled job.

**`lynx-template-decoder`'s `StyleInfo` section carries two hard bounds**, and
they are load-bearing rather than defensive. `Rule` holds `children: Vec<Rule>`
with no depth bound and rkyv 0.7's derived `CheckBytes` recurses once per
level, so a *well-formed* section — nothing for validation to reject — drives
that recursion as deep as its bytes allow. Measured on aarch64: a level costs
28 archive bytes and about 410 bytes of stack in release, 3.5 KiB in debug, so
a **168 KB** section overflowed the 2 MiB stack Rust gives a spawned thread. It
did so *inside* `check_archived_root`, reported as `fatal runtime error: stack
overflow` — `SIGABRT`, not a panic, uncatchable, process gone. The largest
`StyleInfo` section in the vendored fixtures is 24 KB.

Validation therefore runs on a thread whose stack the crate sizes from the
section length, under two caps:

- **Length**, 1 MiB — about 40x the largest real section. It exists only to bound how much stack
  that thread may be asked for. A length cap on its own cannot fix the overflow, because the safe
  length depends on the caller's stack and a library does not know it.
- **Nesting**, 64 levels. The format nests one level in practice (a `Keyframes` rule holds its
  keyframe rules) and `bobcat-cli`'s converter reads exactly that one.

The depth cap is the half that is easy to miss: `Rule`'s drop glue also
recurses per level, so a deep tree returned to a small-stack caller would
overflow on the way *out*, after decoding had already succeeded. Refusing it
keeps the deep value on the sized thread, and nothing downstream ever sees a
tree it cannot afford to walk or free.

rkyv 0.7 offers no depth limit of its own — its `check_archived_*` docs say the
result "may be vulnerable to memory overlap and recursion" — and the 0.7 pin is
a wire-format constraint. The alternative, a hand-written iterative
`CheckBytes` for `ArchivedRule`, would need `unsafe` and cost the crate its
`forbid(unsafe_code)`. `crates/lynx-template-decoder/src/style_info.rs` holds
the constants and the regression test.

### The unsafe floor

`hughie`, `flashbulb`, `lynx-template-decoder` and `lynx-xml` carry
`#![forbid(unsafe_code)]`. The workspace-wide `unsafe_code = "warn"` is a lint
any module can silence locally; `forbid` cannot be overridden from inside the
crate, so `unsafe` appearing in one of these four has to be a deliberate edit
to that line.

Where `unsafe` is unavoidable, the bar is a `SAFETY` comment per block,
enforced by a crate-local `#![warn(clippy::undocumented_unsafe_blocks)]` in
`bobcat-cli` (which now holds no `unsafe` at all, the lint standing as a bar
for any that arrives) and in `dom` (its two). The lint is still
crate-local rather than workspace-wide because `quickjs-rust-bridge` (133
blocks) is the last holdout and is being restructured separately; raising it
there is what would let this move into `[workspace.lints.clippy]`.

One trap, worth knowing before writing the comment: the lint does **not** scan
past an intervening attribute. Where an unsafe site carries
`#[expect(unsafe_code, reason = …)]`, as every one in `dom` does, the
`// SAFETY:` comment has to sit *between* that attribute and the block —
placing it above the attribute still trips the lint, and `cargo fmt` will not
move it either way.

### Benchmarks measure a debug-instrumented dom

Cargo unifies the features of a package's dev-dependencies into that package's
own library whenever dev targets are in the build. `hughie` dev-depends on
`dom` with `layout-test-utils` for its bench harness, and `dom` depends on
`hughie`, so the cycle turns the feature on for both libraries in any build
that includes bench targets:

```sh
cargo build --unit-graph -Z unstable-options --workspace            # dom: []
cargo build --unit-graph -Z unstable-options --workspace --benches  # dom: [layout-test-utils]
```

The second line is what `cargo codspeed build` and `cargo llvm-cov` resolve.
The cost is one `test_leaf_metrics()` probe per leaf in
`crates/dom/src/layout/host.rs` that a release build does not contain — so the
CodSpeed numbers, which are the authority for this repo (single-run local
walltime here is noise), describe a `dom` that is one branch away from the
shipped one. On the `hughie` side the feature only adds the
`compute_leaf_layout_with_measurement_for_testing` wrapper and costs nothing.

**This is accepted, not fixed.** Breaking the cycle means moving hughie's
dom-based benches into `dom`, which renumbers every CodSpeed benchmark id and
throws away its history — a worse trade than one predictable branch.
`.github/scripts/check-bench-feature-parity.py` runs in CI and holds the line:
it diffs the two resolutions, prints the two recorded deviations, and fails on
a third appearing or on a recorded one silently going away. Any new entry needs
a written reason for the same cost the existing ones state.

### Restricted-environment troubleshooting

Some agent runners and automation environments may restrict GPU interfaces,
Git metadata, or network access. Treat that as a hypothesis to test, not as the
default explanation for a failure:

- If a GPU-backed command reports that no adapter is available on a host that
  is expected to expose one, retry the exact command outside the restricted
  environment or with a narrowly scoped sandbox escalation, when available. A
  successful retry identifies an environment limitation; if the retry still
  fails, continue diagnosing the renderer, driver, and adapter selection.
- If a Git operation needed to prepare or publish a pull request — such as
  branch creation, staging, committing, or pushing — fails with a permission,
  network, or authentication-like error, check whether the worktree's Git
  metadata or required network access sits outside the current sandbox. Retry
  only the failing operation with narrowly scoped escalation, when available;
  if it still fails, diagnose the repository, credentials, or network itself.
- If a `--target wasm32-unknown-unknown` build fails in `quickjs-rust-bridge`'s
  build script with `No available targets are compatible with triple
  "wasm32-unknown-unknown"`, that is toolchain *selection*, not a missing
  capability: Apple's clang has no wasm32 target, and the build script invokes
  whatever `CC` names. Point it at the same LLVM the CI jobs install and the
  build succeeds, with no effect on host builds:

  ```sh
  export CC="$(brew --prefix llvm@22)/bin/clang"
  export CXX="$(brew --prefix llvm@22)/bin/clang++"
  ```

  Reach for this before reporting the Wasm target as unbuildable, because
  `crates/bobcat-wasm/src/browser.rs` is `#[cfg(target_arch = "wasm32")]`:
  `cargo check --workspace --all-targets`, `cargo clippy`, and the test suite
  never type-check it, so anything touching the browser embedder — or any
  `#[cfg]`-gated import it depends on — is unverified until that target builds.
  CI's `browser` job now lints that target, so the gap is no longer silent:

  ```sh
  cargo clippy --target wasm32-unknown-unknown --lib \
    -p bobcat-wasm -p bobcat-core -p dom -p hughie \
    -p lynx-xml -p lynx-template-decoder -p quickjs-rust-bridge -- -D warnings
  ```

  `--lib`, not `--all-targets`: `bobcat-core` dev-depends on tokio's
  `rt-multi-thread`, which refuses to compile for wasm32, and feature
  unification drags it into anything that builds dev targets. The packages are
  named rather than `--workspace` because `bobcat-cli` is a native binary. The
  two `-Ctarget-feature` warnings `.cargo/config.toml` produces on every crate
  are rustc codegen warnings rather than lints, so `-D warnings` leaves them
  alone.

The Element PAPI runtime has two suites over the same file:
`pnpm --filter bobcat-element test` (Rstest, over a recording native mock) and
`pnpm --filter bobcat-element test:type` (`tsc --noEmit` under `checkJs`),
while `crates/bobcat-core/tests/main_thread.rs` drives the identical bytes
through the real QuickJS realm, `bobcat` object, and collector. The type suite
also checks the colocated `main-thread-runtime.mjs`, whose behavior is covered
by the core main-thread tests. Changing either source triggers a `bobcat-core`
rebuild through `include_str!` — there is no generated artifact to refresh.

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
