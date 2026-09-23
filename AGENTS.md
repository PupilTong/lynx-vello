# lynx-vello — Agent Guide

This is the canonical project/architecture doc for coding agents working in
this repo. Claude Code and Codex both start here: `CLAUDE.md` is a short
pointer to this file plus Claude-specific notes, `.codex/agents/` mirrors
`.claude/agents/`, and `.agents/skills/` mirrors `.claude/skills/`.

**How to read this file.** Mission, Standards policy and Dependency policy set
what to build; Workspace map and Crates describe what exists, one section per
crate and pnpm package; Reference repos, Reference knowledge, Toolchain and
Testing say where to look things up and how to run them. Format Rust with
`cargo fmt -p <crate>` for the crates you touched — never `cargo fmt --all`,
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
- **[vello](https://github.com/linebender/vello)** — GPU vector rendering
- **[parley](https://github.com/linebender/parley)** — text layout & shaping

The from-scratch layout engine (successor to the C++ engine's `starlight`) is
`crates/hughie` — its host protocol, shared layout machinery, and CSS
flexbox, Grid, CSS Grid Level 3 `display: grid-lanes`, and Starlight
`display: relative` and `display: linear`
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

## Workspace map

| Path | Role | Details |
| --- | --- | --- |
| `crates/bobcat-source` | Owner of Lynx source parsing: ZIP, restricted XML, `.web.bundle`, `.lynx.bundle`. | [→](#cratesbobcat-source) |
| `crates/bobcat-core` | The native runtime core: group, view, painter, QuickJS realms, page policy. | [→](#cratesbobcat-core) |
| `crates/quickjs-rust-bridge` | Owner-thread-bound safe Rust wrapper around the pinned `vendor/quickjs` submodule. | [→](#cratesquickjs-rust-bridge) |
| `crates/bobcat-resources` | Reference resource system: transports, MIME pipeline, caches, platform image decoding. | [→](#cratesbobcat-resources) |
| `crates/bobcat-cli` | The `bobcat` product (`cli`) and the `bobcat-server` screenshot service (`server`), both embedders. | [cli](#cratesbobcat-cli-cli-feature), [server](#cratesbobcat-cli-server-feature) |
| `crates/bobcat-wasm` | The pure-Rust `wasm-bindgen` browser embedder and npm facade. | [→](#cratesbobcat-wasm) |
| `crates/dom` | Generic W3C-DOM-subset document tree and standards-oriented CSS computation core. | [→](#cratesdom) |
| `crates/hughie` | The from-scratch Flexbox, Grid, grid-lanes, and Starlight Relative and Linear layout engine. | [→](#crateshughie) |
| `crates/flashbulb` | Screenshot testing: RGBA image, PNG codec, pixelmatch port, golden store. | [→](#cratesflashbulb) |
| `packages/bobcat-element` | Dependency-free TypeScript sources of the ESMs `bobcat-core` preloads into its realms. | [→](#packagesbobcat-element) |
| `packages/reactlynx-test-fixtures` | ReactLynx source fixtures and bundle generators for Bobcat integration tests. | [→](#other-pnpm-packages) |
| `packages/explorer-homepage` | The Lynx Explorer home screen, written in ReactLynx. | [→](#other-pnpm-packages) |
| `packages/explorer-lib` | Navigation, launch-command, history and theme helpers shared by the Explorer pages. | [→](#other-pnpm-packages) |
| `packages/explorer-showcase` | The Lynx Explorer showcase menu in ReactLynx, and the `@lynx-example` packages. | [→](#other-pnpm-packages) |
| `packages/github-pages` | The rsbuild site on GitHub Pages, over `bobcat-wasm` and the Explorer homepage. | [→](#other-pnpm-packages) |
| `examples/` | `lynx-stack`'s own examples, re-pointed at published package versions. | [→](#other-pnpm-packages) |

## Crates

### crates/bobcat-source

The single owner of Lynx source parsing and adaptation: the always-available
`ZipSource` API for bounded ZIP decoding, entry selection through `PageSource`,
and resource registration. Native and Wasm embedders share that API; IO and
resource-scope lifetime stay the host's.

`xml` is the zero-dependency, zero-copy restricted envelope parser
(`engine-version`, `thread="main"` / `thread="background"`), retaining UTF-16
and UTF-8 error offsets. `web` decodes `SDRA WROF` on the unchanged rkyv 0.7
wire model. `native` decodes source-based flexible external `.lynx.bundle`
files into that same model and can explicitly encode a web bundle; it rejects
real QuickJS/Lepus bytecode rather than executing or decompiling it. Named
external modules are preserved and acquire no invented page root.
`PageSource::from_native_bundle` requires an explicit entry name, `from_bytes`
a `root` for binary page inputs.

`PageSource`, browser response registration, shared StyleInfo lowering and all
three parsers are always available: the crate has no Cargo feature flags, so
every embedder including Wasm depends on the complete crate. IO and view
construction stay with the embedder; the browser's `loadLynxXml` accepts only
XML responses. Native XML keeps strict UTF-8 and private memory URLs; the
browser keeps replacement decoding, final-response fragment URLs and host
PageConfig. The two register the raw XML background script under different
URLs: native under `bobcat-memory://lynx-xml/app-service.js`, the browser's
`register_lynx_xml_response` at `<final-response-URL>#background-thread`, which
keeps the response URL as the base for that script's own relative imports.
Either way the URL is named in `ViewSources::background_entry`, so the view's
BTS Worker imports it. See `docs/source-architecture.md` for boundaries,
migration and parser resource bounds.

### crates/bobcat-core

The unified native runtime core. Source layout follows the ownership
boundaries. `main/` is everything on the Lynx main thread: `page.rs`'s one
realm entry point, `quickjs.rs`'s script engine, `runtime/` for realm
integration, `workers.rs` for the `Worker` class, `tree/` for Lynx page policy.
`background/` is the `bobcat-workers` thread and its worker realms. `view/` is
the public view facade, `paint/` the `Painter` with its `gesture.rs` input
router, `motion.rs` scroll kinematics (lynx-ui's rubber band, fling decay
and bounce back) with `inertia.rs` running them over the scroll intents,
`images.rs` image protocol and `graphics.rs` GPU target. `link.rs` is
the one channel set a view spans its two threads with, `jobs.rs` the engine
thread itself — its scheduler and its job queue — `lifetime.rs` the view's
task set, `timers.rs` and `clock.rs`/`alarm.rs` the timer machinery both realm
kinds share, `future.rs` the per-realm table the `Future` class is written
over, `fetch.rs` the one member a realm fetches a URL through, `esm.rs` the
preloaded module specifiers, `script.rs` the
sanitized error a failure is reported with, `style.rs` the
`PreparsedStyleSheet` vocabulary, `resource.rs` the host protocol, and
`threads.rs` the two engine threads.

#### Public surface and the two engine threads

The public runtime is the opaque `LynxGroup`, `LynxView<F>` and `Painter`
facades plus the protocol-only, host-injected `ResourceFetcher`, draw-target,
OS-input and lifecycle-wakeup capabilities. The script engine is deliberately
not one of them: core owns its `QuickJS` realm, and an embedder sees only the
sanitized `script::ScriptError`. A view is built from one `ViewSources` —
`PageConfig`, owned font containers, an optional default font family, author
stylesheet URLs in cascade order, the entry MTS module URL, optional
`init_data` and `global_props` JSON text only the realm parses, and the
optional `screen` metrics `SystemInfo` reports — plus a builder
turning the view's `ImageReports` into its `ResourceFetcher`; both go to
`LynxGroup::create_lynx_view` with device metrics.

**A view is built from a group, never on its own**: `LynxGroup::new` takes the
lifecycle wakeup and `StyleThreads`, starts `bobcat-workers` then `bobcat-main`
(handed one sender on it), and awaits the QuickJS runtime and Stylo pool every
view in that group shares.

**Both engine threads are a `jobs.rs` `JsThread`: a tokio `current_thread`
runtime with a `LocalSet`, plus a FIFO of jobs its top loop runs between two
turns of that scheduler.** Tasks wait and route — a channel read, a deadline, a
lifecycle signal, a spawn — and touch no realm, no document and not the shared
`ScriptRuntime`; what a task does with what it read is queue a job and await it.
Jobs are the only place JavaScript runs, and they run outside every `block_on`,
which is what lets one park: `JsThread::wait` is a fresh `block_on` of the same
`LocalSet`, so during a synchronous stylesheet adoption or a `require` every
task on the thread goes on running while no other job does. Queued jobs run in FIFO order once the
waiting one returns, so an entry may hold the shared runtime and its realm
across its own wait.

Each asynchronous wait is a task of its own. A view's tasks are its owner
(`serve_view`, whose one wait is the view's end), its boot future, one ordered
consumer of the command channel, one ordered consumer of its workers' events,
one future per resource load an import produced, and one clock task
(`lifetime.rs`'s `serve_clock`) waiting on its realm's next timer deadline and
on the runtime-wide checkpoint generation; a `Worker` realm on `bobcat-workers`
has the same shape minus the document. Nothing is spawned per input: one
consumer reads each ordered stream with `while let Some(x) = rx.recv().await`.
Every task reaches the realm through one boundary, `main/page.rs`'s
`Page::enter`, which queues a job that runs one synchronous operation under the
borrows of the shared runtime and the realm and then that operation's epilogue,
in this order: the timers that came due, the commit, the boot report once, the
`BeginFrame` acknowledgement, the module requests entry produced, the next timer
deadline, and the checkpoint generation as of this entry. `Page::settle` is the
epilogue alone, for a wake carrying no operation. What a *loading* page does
with a burst is the exception: its document ingredients are a field of their
own, never held across a wait, so a resize, an image report and the immediate
`BeginFrame` acknowledgement are served on the task and never queue behind a
sibling view's load. A `bobcat-workers` consumer never awaits the deliveries it
queued, because `Terminate` is in band behind them: it queues one job per
message and ends the worker the moment it reads one, which is what releases a
job parked on a synchronous wait and discards the posts queued ahead of it.

A view owns its channels end to end, all `tokio::sync` and none addressed.
Three cross that link: a `ToMain` mpsc carrying commands in; a `ViewNotice`
mpsc carrying lifecycle events and resource asks back; and one
`watch<Published>` carrying what an observer wants the *latest* of — the newest
committed frame, the listener-name set, and the newest serviced `BeginFrame`.
Commands are a FIFO because their arrival order is what they mean; a frame is
not. `ToMain` carries a `PageUpdate` (the data, global-prop, global-event and
reload commands a host accepted after observing MTS boot, in host FIFO order),
a `DispatchEvent` (one event's type and target, plus the payload the router
decided as values: the position, the wheel delta, the timestamp and — for the
four touch events — its touch points), a `Vsync` (the display-frame reading a realm that called
`requestScriptFrame` asked for), a `BeginFrame` (a timeline reading plus the
sequence number the acknowledgement reports), a `Refill` (the scroll offsets
the painter moved past a slot's encode window, written back), and `ImageEvents`
(completed or failed host loads — no variant can carry pixels, which makes
"`ImageData` never crosses a channel" a property of the type). The painter's
device metrics are **not** a command: they ride a
`watch<Option<Viewport>>` on the view's seat, `None` until a painter attaches,
because an unbound `__FlushElementTree` parks the job it runs in on that very
watch and no other job would run to read a command.

`create_lynx_view` sends the far half of that link to the group's thread and
builds the fetcher in place on the calling thread. It is **synchronous and
takes no draw target**. Its `width`, `height` and `device_pixel_ratio` are the
create-time viewport the document is built at and works at until a painter
binds; they are not validated, because no target is built from them, and an
attached painter's metrics supersede them. The view's `F` parameter is that
view-owned fetcher; the wakeup is a separate group constructor generic held by
`bobcat-main`. Construction returns a loading view at once, whose boot outcome
arrives through `pump`; constructor errors cover attachment, an unknown default
font family and two native modules claiming one name.

**That same call hands the fetcher the view's startup sources.** It validates
the fonts and default family first — a `dom::TextContext`'s business, with no
document and zero fetches on failure, which is why the check has to be here
rather than a turn later — and then requests each stylesheet in cascade order
followed by the entry module, on the embedder's thread, before it returns. Only
the built text context and the answering one-shots cross to `bobcat-main`,
where the view's owner reads them in that same order and stages what arrives as
the `DocumentIngredients` its document is built from. So the fetcher's IO, the
realm's boot and the first frame's encode all overlap the painter the embedder
builds next, and `LynxView::pump` services every *later* request — imports,
`adoptStyleSheet`, worker scripts, fonts, plain fetches — and the view's images
in ordinary turns. The default family is prepended to the `system-ui`,
`sans-serif` and `serif` generic maps, so a Wasm embedder can supply its
otherwise-absent system-font backend without baking a font into core; a name
neither the containers nor the platform has fails the construction with
`EngineError::UnknownFontFamily`.

Dropping a loading view marks source work cancelled and stops that view before
QuickJS begins. One `tokio_util::sync::CancellationToken` per view is minted on
the embedder's thread and cancelled there by the view's drop, by a fatal
lifecycle event, and by a guard on every exit from the view's owner, so a host
holding a `SourceCompletion` reads cancellation without waiting for a turn. It
is the view's end signal: the owner reclaims its ordinary tasks, then waits for
MTS JavaScript disposal before releasing the realm. Workers have independent
tokens and stay live for that exchange. Commands queued behind that release are
discarded rather than applied — the one command consumer reads the token at
each wake, before applying anything — while a burst already inside an entry
finishes. The view's handle on the host resource system goes with the release,
so past one a painter adopts no commit whose pixels it is not already holding.
Fetchers skip cancelled queued work; IO or synchronous JavaScript already
executing may finish, late source results are discarded, and the group and
other views keep running.

**A view spans two threads**: the embedder's own — whichever created its
`LynxGroup` — which owns the window, the input capture, the surface (the one
call macOS allows nowhere else), the host's whole resource system and the
`Painter` (routing, gestures, scrolling, composition and every GPU call), and
the Lynx main thread (document + realm). Neither half can leave it: a `Painter`
is `!Send` because its target is, and a `LynxView` is `!Send` because it holds
`Rc`s of the group and of the fetcher — the only shape the browser allows,
where `wgpu`'s handles are not `Send` under shared memory and an
`OffscreenCanvas` cannot be transferred on again. Dropping a view cancels its
source work, detaches its image inbox, then closes its command channel, the
goodbye its task ends on; the group handle drops last, so the group's threads
are joined only once nothing is left on them.

**Views in a group share one thread, one `QuickJS` runtime and one Stylo
pool.** `create_lynx_view` is the only way to build a view, because naming the
group is the only way to say which thread it runs on. One group per thread and
one thread per group: a group hands out `Rc`s of what it owns, so it is `!Send`
and `!Sync`, and the thread that creates it is the thread every view in it
paints on, which is why one `EventRequester` serves the whole group. Views take
turns — every entry into a realm is one synchronous stretch — so a second view
costs no second heap, module graph or set of workers, at the price of never
restyling in parallel, on the assumption that a person drives one view at a
time. A host that needs two pages genuinely parallel gives them a group each,
on a thread each.

`bobcat-main` builds the group's one `dom::StylePool` — sized by the
`StyleThreads` passed to `LynxGroup::new`, `Auto` being the usual choice —
before any view attaches, and every document it carries holds an `Rc` of it.
Stylo's bloom filter and style-sharing cache are per-OS-thread borrows held for
a whole traversal, so two documents traversing on one worker at once is an
aliasing bug. Two facts rule that out: different groups draw from disjoint
pools, and documents in one group cannot traverse at once.

**`bobcat-main` is index zero of its group's pool**, taken over in place by
rayon's `use_current_thread` — which is why the pool can only be built on
`bobcat-main`, and why `StyleThreads` counts it: `Fixed(3)` starts two threads,
not three. A lone view therefore restyles with the same parallelism and the
same inline root closure Stylo's global pool gave it. The takeover is
permanent: rayon leaks about 25 KB per pool (the managed threads still exit on
drop; the `WorkerThread` box and `Registry` do not) and refuses a second pool
on the same thread forever. **That refusal is why the pool has to be the
group's rather than any view's.** A host that replaces groups — every
`BobcatRenderer::load` — pays that 25 KB per replacement, in the same Wasm
linear memory. `StyleThreads::Sequential`, and `Auto` where the pool would have
held `bobcat-main` and nothing else, gives a group no pool at all and traverses
on `bobcat-main` alone, a configuration rather than a fallback.
`dom::MAX_STYLE_THREADS` is six, counted the way Stylo counts its own six: a
ceiling, not a tuning knob, because Stylo indexes its per-traversal
thread-local storage by Rayon thread index into an array that long, so a wider
pool is a construction error rather than a silent clamp, reported by
`LynxGroup::new` like any other boot failure. **Wasm takes the same path.**
`navigator.hardwareConcurrency` reaches `StyleThreads::for_parallelism`, which
is `Auto`'s own arithmetic, so comparable hardware gets the same pool on both
targets and the facade does no thread arithmetic of its own.

#### Resource protocol, stylesheets and ESM loading

`ResourceFetcher::request_source` owns URL resolution, fetching and UTF-8
validation. Its concrete, non-cloneable `SourceCompletion` holds one end of the
one-shot minted with the request and answers whichever task awaits it, so the
host never learns which; no resource Future, poll loop, callback trait object
or resource waker lives in core. Main's lifecycle notifications wake the host
through `EventRequester`.

Bundle retrieval, `.web.bundle` decoding and config parsing are embedder
responsibilities; the fetcher supplies validated source text and core registers
its resolved URL in QuickJS's preloaded ESM graph. `request_source` answers
stylesheet requests with CSS text or a `PreparsedStyleSheet`
(`bobcat_core::style`) the host parsed itself, since a `.web.bundle` ships CSS
a build step already tokenized and re-serializing it to a sheet blob is the
startup cost the design rules out. Lowering it produces no stylesheet text:
rules, keyframes and font-face rules are built through `dom`'s branded
`CssRule` builders, leaving stylo one selector-list parse per rule and one
value parse per declaration — the floor, because the wire format keeps
attribute selectors and functional pseudo-classes as text and stylo builds
specified values only through its value parsers. Decoding a container stays
embedder work: core owns the `PreparsedStyleSheet` vocabulary, the embedder
fills it. Source requests select a stylesheet or entry payload and carry a
specifier; the fetcher supplies the base URL and transport policy. The protocol
also offers the optional `preload_source` hint,
`request_image`/`service_images` and the `FrameImages` supertrait: every method
is synchronous, so no resource future crosses it, and core names none of a
fetcher's own transport API. **One optional member leaves the embedder's
thread**: `fetch_probe()` hands back a `Send + Sync` `FetchProbe`
(`Arc<dyn Fn(&str) -> bool + …>`) that a realm — MTS's or a worker's — asks
whether a `SourceRequest::Fetch` of a URL has already completed for this view,
resolving the specifier the way a request would. A hit means no request is
made and the realm's `fetchResource` settles in that same call, which is the
only synchronous answer this protocol has; the default is no probe, and a
probe must neither block nor start a load. It carries no response-size limit either; each
fetcher owns the memory bound for the response it materializes. The resource
module must not decode images, fonts or templates, upload render resources, or
own cache/retry policy.

**`NativeModules` is the other host-injected, thread-bound capability.**
`LynxGroup::create_lynx_view` takes a `Vec<Box<dyn NativeModule>>` beside the
fetcher builder, and for the same reason: a module runs on the embedder's own
thread, inside `LynxView::pump`, so it needs neither `Send` nor `Sync` and a
Wasm module may hold `JsValue`s. One `dyn` handler per `NativeModules` key,
`dom::CustomElement`'s shape: `name()` and `methods()` are read once at
construction — two modules of one name is `EngineError::DuplicateNativeModule`,
returned as `LynxViewError::Engine` — and the table crosses to the realm as
data, so nothing asks a module a question while script is running. The call
shape is native Lynx's, not a promise's: every function argument becomes a
single-shot `ModuleCallback` the module invokes later with JSON array text, the
method itself answers `undefined`, and there is no synchronous return value and
no error channel. `ModuleCallback::invoke` consumes the handle, dropping one
releases the JavaScript function uninvoked, and the answer rides the calling
Worker's inbox weakly — the very handle the view already registered from
`ViewNotice::WorkerCreated` for frame demand, so nothing is carried across a
second time. A callback therefore holds no realm open however long a module
keeps it, and `is_cancelled()` is that handle's own liveness: a worker's
receiving end drops with its task, so there is nothing left to answer exactly
when there is nothing left to answer *through*. An answer is delivered to the
BTS realm the moment it arrives, never queued behind BTS boot: an entry
awaiting its own call's answer would otherwise deadlock.
`NativeModules` is **BTS only** — MTS's stays `undefined`, as Lepus has no
module binding — an unknown module is `undefined` (web-core's answer, where
native answers `null`; see `docs/tracking/deviations.md`), an undeclared method
is `undefined` on both references, and a call naming a module this view lacks
is never assembled at all, which leaves its functions released. No built-in module ships: `bridge`,
`LynxUIMethodModule`, exposure and intersection are all absent.

`PageSource` registers named CSS under entry-relative resource URLs. Boot
supplies the entry response URL to the JS runtime before importing the entry,
whose `__Card__` import reads that value; JS replaces the `"__Card__"` alias
and maps the compiler's `CSS` section to `<entry-url>/index.css`.
`__LoadStyleSheet` returns a plain `{url}` object and sends a
`ResourceFetcher::preload_source` hint a fetcher may ignore; no realm state
survives that call, and `__AdoptStyleSheet` reads the URL off the handle it is
given.
Every `__AdoptStyleSheet` requests the URL through `SourceRequest::StyleSheet`,
synchronously obtains its response and mounts it before returning, repeated
adoption included. The fetcher owns pending loads, cached responses and
failures; the reference `Resources` shares them by resolved URL within a scope
and invalidates registered URLs when replaced or removed. Core holds only the
current call's receiver. The embedder returns CSS text or a
`PreparsedStyleSheet`; JS sees neither. An incomplete request parks the *job*
the adoption is running in until the response arrives or the view is cancelled:
no JavaScript job runs meanwhile, this realm's or a sibling's, while
`bobcat-main`'s tasks — including the ones that route that very response —
carry on. Errors throw at adoption; an unused preload changes no styles.
Resource lifetime belongs to the fetcher, adopted rules to the document. No
native stylesheet handles, load state or adoption queue live in
`MainThreadRuntime`. See
`docs/named-styles-runtime.md` for URL mapping and load timing. Per-component
css-id scoping is **not** implemented: every fragment mounts globally, which is
what web-core emits for an `enableRemoveCSSScope = true` bundle (see
`__SetCSSId` below).

Ordinary ECMAScript `import()` loads JavaScript ESM through
`SourceRequest::Module` and the same `SourceCompletion` channel, during and
after boot. Core normalizes module URLs against the importing module's response
URL; the fetcher owns transport and UTF-8 validation. Built-in sources stay
group-wide; entries and imported sources are realm-local, one request,
namespace and evaluation per normalized URL. The QuickJS fork preflights static
dependencies with unlinked compilation, defers incomplete import graphs and
resumes the original promises on main when sources arrive, so cycles never
become partially linked while fetching. Top-level await can span resource and
timer turns; `ScriptFinished` waits for the boot promise alone. Handled import
failures leave the realm usable, and dropping the view cancels outstanding
completions and releases continuations. This is JavaScript ESM loading: import
maps, import attributes, JSON modules and Lynx component-bundle imports remain
unsupported, and runtime configuration, raw realm/value handles, interrupts and
source-evaluation entry points stay private. The bridge owns the generic
source/native-module loader, deferred import continuations, loaded-module
namespace access and settled Promise inspection; Bobcat's specifiers, entry
transform, graph membership and boot policy stay in the core adapter.

**`bobcat:module` is the synchronous way into a source.**
`import { createRequire } from "bobcat:module"` is available in a view's MTS
realm and in every Worker realm, the BTS included; it is an explicit import,
and neither entry preamble carries it. Node's algorithm — the cache, the
`module` object, cycles, eviction, `require.resolve` — is the
`packages/bobcat-element` source module `module.ts`, like every other built-in.
It is written over two host members on `bobcat-internal:host`, which both realm
kinds have: `resolveModuleUrl(base, specifier)`, the normalizer `import`
resolves through, so a `require` and an `import` name a module by the same URL;
and `loadModuleSync(url, parameters)`, which asks the host for that URL through
the same `SourceRequest::Module` and answers the source *compiled* — the
wrapper function of a CommonJS file, the parsed value of a JSON one, or the
namespace object of an ES module, linked and evaluated — so source text never
becomes a JavaScript value. That load parks the job it runs
in on the answer exactly as stylesheet adoption does: the engine thread's tasks
go on running, no other job does, and so no promise job and no sibling realm's
entry runs while it waits. The other arm of that wait is the requesting realm's
cancellation token: a view's is written by the embedder's release, from the
embedder's own thread; a Worker's by the in-band `Terminate` its message
consumer — a task, still running during the wait — reads, which ends the worker
and the load with it. Which of the three a source is read as is Node 24's rule
with the one input it has that a URL does not: the response URL's path decides
(`.json`, `.mjs`, `.cjs`), there is no `package.json` `"type"`, and anything
else — a plain `.js` — is what QuickJS's own syntax detection reads the text
as, in the bridge, where the text is. An ES module is **linked inline**: every
`import` in its graph is loaded through the same member during the compile,
recursively, before any body runs, and a `bobcat:*` specifier links to that
native module instead. Its evaluation runs no promise jobs, so two things
throw instead of waiting, as they do in Node: a graph that awaits at its top
level (`ERR_REQUIRE_ASYNC_MODULE`) and a module of a graph that is still
evaluating (`ERR_REQUIRE_CYCLE_MODULE` — and, since `JSModuleDef` keeps its
status private, also a module of that graph whose own body has finished). What
`require` answers is the namespace, or an export literally named
`module.exports` where there is one. The cache is the realm's own, reachable as
`require.cache`; a URL is one module in it, and an ES module an `import`
already brought in is answered from that instance rather than loaded again. `module.id` and `module.filename` are the URL that
was required, and the URL the load answered from is `__filename`, the base a
nested `require` resolves against and what `__dirname` is one resolution away
from. `require.resolve` answers the cache key without loading, and a load,
compile, parse or body that fails leaves nothing cached. Every source module
carries `import.meta.url` — the response URL for a fetched one, the name it was
registered under for a built-in.

**`bobcat:future` is one host-backed operation, read either way.**
`import { Future } from "bobcat:future"` is available in a view's MTS realm and
in every Worker realm; the class is the `packages/bobcat-element` source module
`future.ts` and the per-realm table behind it is `crate::future`. A `Future` is
a number and nothing else — the id that table registered a Rust future under —
because only primitives and structured clones cross the boundary. `wait(timeout?)`
parks the *job* it runs in, exactly as stylesheet adoption and a `require` do:
the engine thread's tasks go on running, no other job does, and no promise job
runs. Its first and biased arm is the requesting realm's cancellation token,
and an optional deadline sits behind it; a deadline that passes throws a
`TimeoutError` and **cancels nothing**, so the operation goes on and the same
Future answers a later read. `then` is the other way and converts the Future
into one Promise, once: the owner's epilogue spawns a task that awaits the
operation and then enters the realm to deliver it, rejecting with an `Error`
carrying the host's reason. A `wait` after that conversion is a `TypeError`,
because the delivery is a job and a job cannot run inside another job's wait.
Three host members carry it — `waitFuture(id, timeoutMs)`, `takeFuture(id)` and
`settleFuture(id)` — and both realm kinds have all three. What registers one
in production is `fetchResource` (`crate::fetch`), the member
`lynx.fetchBundle` is written over; a test-only `testFuture` producer
exercises the table itself.

`lynx.requireModule`, `nativeApp.loadScript` and `lynx.loadScript` are the
compiled-bundle layer — the first two in `bobcat:lynx-modules` and so in
worker realms only, `lynx.loadScript` on both threads — and each is **one
synchronous load** over that same member — the mechanism
MTS's `__LoadLepusChunk` uses: the realm builds the URL the path names beside
the registered template URL and loads it, before the call returns. There is no
table of bodies and no boot-time import loop. **No source table and no source
text reaches a realm**: `PageSource` turns each of a container's bodies — its
manifest paths and its string custom sections — into an **ES module** and
registers it with the embedder's resource system beside the page's own input
URL, and the BTS boot script it writes is
`__BobcatRegisterBundle(templateUrl)` and the `lynx.requireModule` that starts
the card, nothing else. A `.lynx.bundle`'s body is one expression statement, so
its module is `export default <body>` — what native's host would have kept as
that script's completion value is the default export instead; a
`.web.bundle`'s is a CommonJS file, so its module supplies a `module` object
and exports `module.exports`; a `.json` body is registered verbatim, its own
URL being what tells the loader to parse it. Each module carries
`BTS_CHUNK_PREAMBLE` (`crates/bobcat-core/src/esm.rs`) on one physical line, so
the body keeps its own line numbering and has every name web-core's chunk
wrapper would have passed as a parameter. A path is rooted before it is
resolved, so either spelling of a name is one URL; what comes back answers
through the module's `default` export (a namespace with no `default` answers
itself), through the parsed value for JSON, or through `module.exports` for a
CommonJS file compiled in `module, exports` alone; and a value carrying an
`init` function is *initialized* — `init.call(value, {tt})`, lynx-core's
`_$executeInit`, with `globalThis.globDynamicComponentEntry` published for the
call. Their caches are their own, not `require.cache`: keyed by the bare path,
written only after the factory returns, and `loadScript` writes neither. A body
is never `import`ed, only `require`d, so the still-evaluating refusal never
reaches one: it is compiled by the `require` that asked for it, or answered
from the evaluation an earlier one ran. An entry no `__BobcatRegisterBundle`
named is a **lazy container's** `bundleName` instead, and its sections are
named *under* that URL rather than beside it — `<bundleName path>/<encoded
section>.js`, one rule shared with MTS's `chunkURL` and with
`bobcat-source`'s `named_chunk_url`.

**`lynx.fetchBundle(url, options?)` is a plain fetch**, in both realm kinds:
apart from being waitable it does what `fetch(image_url)` does. Core's whole
half is `fetchResource(url)` — one `SourceRequest::Fetch`, which answers
`LoadedSource::Fetched` and carries nothing back — so **`bobcat-core` holds no
bundle-specific code at all**: no installed set, no record JSON, and no
knowledge of what the bytes were. Whether they were a Lynx container whose
sections get registered at the URLs a later
`lynx.loadScript(section, {bundleName})` or `__LoadStyleSheet('CSS', url)`
names is the **fetcher's**, through `bobcat_resources::ContainerInstaller` and
`bobcat_source::LazyBundleInstaller`, which sniffs the two container magics
and leaves every other fetch alone. Nothing is evaluated by any of it. The
handle is native's `{wait, then}` host object rather than a Promise, built in
`bundle-fetch.ts` over the **`bobcat:future`** `Future` the member answers
with: `wait(seconds)` is `Future.wait` in milliseconds, so it parks the job
the way a `require` does and a deadline that passes answers `code: -2` and
cancels nothing; `.then` converts that Future once and runs its callbacks as
reactions of the Promise the owner's epilogue settles, running one registered
on a handle this realm already holds the outcome for inline on MTS and posted
on BTS, as native's mediators do. A fetch never rejects past the handle: a
failure is a settled record with `code: -1` carrying the host's reason.
**What has already been fetched is the fetcher's knowledge**, not core's:
`fetchResource` asks `ResourceFetcher::fetch_probe()` first, and a URL this
view already fetched answers `true` with no request, so that handle is
settled from the start and MTS runs its `.then` inline — this engine's
`FindTemplateBundle`, and what `rLynxPrepareLazyBundleMTS` needs to have run
its `loadScript('main-thread')` before the `callLepusMethod` reply reaches
BTS. `requireModuleAsync`, `loadScriptAsync` and `readScript` do not exist.
See `crates/bobcat-core/src/fetch.rs` and `docs/worker-resources-runtime.md`.

#### Realm, document and boot

The crate-private `quickjs::ScriptEngine` is the whole script surface: it
installs named host callbacks, registers named preloaded ESM source, evaluates
a module through its TLA completion promise, calls an export the realm
published back, and provides the GC seam. Created on the engine-owned Lynx main
thread and never leaving it, nothing about it is `Send`. Values crossing it are
`quickjs-rust-bridge`'s `HostValue`/`HostArgument` — primitives plus opaque
structured clones, so realm values and DOM handles never cross as themselves.
The private `MainThreadRuntime` owns the realm integration and, through it, the
document. `LynxDocument`, `Viewport`, `DocumentIngredients`, `DocumentSlot`,
`new_document`, `MainThreadRuntime`, the view's link (`ToMain`, `ViewNotice`,
`Published`) and the concrete QuickJS adapter are crate-private; `Painter` is
public, and `LynxDocument` is what an embedder cannot name.

**The realm creates its own document, and says so.** The boot module's first
statement is `export const document = new Document();`; `bobcat:element`'s
`Document` constructor calls the host member `createDocument`, which builds the
document from the `DocumentIngredients` the view's task staged before the realm
opened — viewport, page config, the validated text context, the author sheets
in cascade order, the group's style pool, and any image reports that arrived
first — mounting and replaying them in that order. Every phase runs under a
catch, because the bridge erases a panic into "the host function panicked". A
second construction is refused whichever module asks: the ingredients are
spent. The realm holds what it built in the private `DocumentSlot` every tree
member borrows; that `Rc<RefCell<…>>` exists only so same-thread native QuickJS
callbacks can reach the owner, not as a cross-thread sharing mechanism.

**The document lives exactly as long as the realm.** The boot module's exported
binding holds it from that first statement on, and nothing in the realm
releases it: no release member, no `FinalizationRegistry` over the `Document`,
and no "no document" answer a host member can give. Release is the view's task
ending: dropping the `LynxView` closes its command channel, the task returns,
and `MainThreadRuntime`'s fields drop in declaration order — the `ScriptEngine`
first, carrying the context's `Rc`, freeing the realm with the host functions
and their clones of the slot, then the runtime's own `slot` handle, which is
when the `LynxDocument` drops. That field order is the whole mechanism; there
is no `Drop` impl behind it. Every tree and attribute member therefore takes
the document unconditionally, and the one refusal left here is a second
`createDocument`. The one window where a document is absent is the load, and
the task serves through it: image reports are buffered and replayed, a
`BeginFrame` is still acknowledged so an offscreen host is never blocked by a
load, and dispatch and refill are dropped.

**A painter binding the view is what releases its first frame.** The document
is created at the create-time viewport unless a painter has already written the
seat's metrics watch, and every epilogue adopts whatever that watch holds. A
commit made before the first write is *held* rather than published — a painter
composes at its own size and cannot tell that the frame it adopted predates the
metrics it just named — and `__FlushElementTree` commits, holds, and then parks
the job it runs in on that watch, the way `adoptStyleSheet` parks on a
response, with the view's token as the biased first arm. Waking bound, it
adopts the painter's metrics and, if they moved the viewport, discards the held
frame and commits again; either way what goes out is at the painter's size.
Only the first binding is waited for: `detach` leaves the last metrics behind,
so a view moved to the background never parks its group again. Boot's last act
is a flush, so a view no painter ever binds publishes no frame, reports no
`ScriptFinished` and never becomes ready. One task of the view, `consume_metrics`,
settles the page once per change, which is what commits a resize with no
JavaScript behind it.

Main opens the realm and evaluates `bobcat:boot`, whose first statement creates
the document and so mounts the staged sheets, in cascade order, before the
entry loads. Success is `ScriptFinished`; resource, font, realm, or boot
failure is `StartupFailed`. That failure stays the failing view's and is
reported once: an entry that throws under boot's top-level `await` rejects
through the promise-job queue the group's realms share, and what it leaves
there reaches neither the next view nor the failing realm's own next entry.
Queued *jobs* still run, and the next checkpoint finishes them. A checkpoint
drains that queue until it is empty, as a browser's microtask checkpoint does:
there is no per-checkpoint job budget and no incomplete checkpoint for a later
entry to resume. Because the queue is the runtime's, a view has to notice a
sibling's entry into JavaScript: `ScriptEngine::checkpoint` bumps a
runtime-wide generation on a `watch<u64>`, every view has a task following it,
and a page whose import finished inside a sibling's checkpoint settles what its
own realm owes. Comparing that generation against the one recorded at the end
of the page's own last entry keeps a page's own bumps from waking it.

A `.web.bundle`'s `lepusCode.root` or raw XML main body becomes a real ESM at
its resolved entry URL: core prepends named imports from both built-ins. The
`bobcat:boot` ESM imports its lifecycle helpers from `bobcat:runtime`,
`Document` and `__FlushElementTree` from `bobcat:element`, and `bobcat:timers`
for its effect. The runtime parses the view's `init_data` and `global_props`
JSON in MTS; missing values become `{}` and malformed inputs fail boot. Boot
creates its document, initializes `__Card__` and MTS inputs, retains the host
render argument, then awaits the entry; it processes that argument and posts
the result plus host props and SystemInfo as the first BTS Worker message,
before rendering. The BTS bootstrap returns after installing a JS receiver, and
that message initializes its inputs before importing the entry. Later internal
messages wait on the import Promise and are delivered in order once it settles,
success or failure.

Lifecycle hooks and engine listeners run synchronously, with no intervening
Promise-job checkpoint. Boot awaits a `Promise.resolve().then` flush after
rendering; that flush's commit completes MTS boot, the whole of public
readiness, and BTS acknowledges nothing. Boot reads
`PageConfig.enable_js_data_processor` from the staged document ingredients and
the screen `SystemInfo` reports from `RealmStartup`, where the view's task put
either `ViewSources::screen` — what the embedder measured: web-core's
`devicePixelRatio` and available screen size in a browser, the window's monitor
natively — or, for a host that named none, the create-time viewport in physical
pixels. It is read once and never updated, so a painter binding at other
metrics leaves it alone. `ViewSources.initial_processor` is a plain `String`,
handed to JS by the one-shot startup-data binding without JSON serialization or
source interpolation. JS merges those three numbers over its own runtime
constants into SystemInfo and sends its snapshot to BTS. Entries receive runtime bindings through
prepended ESM imports; global props updates replace the live module binding,
and there is no native evaluator or separate Script lexical environment.

Runtime JS reads ReactLynx's hooks directly from `globalThis`, while runtime
and PAPI identifiers remain module imports; a named Lepus chunk is a plain
script resource `PageSource` registers verbatim, which `__LoadLepusChunk`
builds the URL of, loads through the same synchronous host loader a `require`
uses, and runs — again on every call, as native does — as a function body
whose parameters are the bindings the entry preamble gives the entry. Named
calls and replies belong to the two JS Worker message handlers; Rust
transports opaque messages and performs no Lepus-specific dispatch or reply
flush. Boot's deferred flush
uses ordinary Promise scheduling and the existing outer checkpoint, with its
rejection attribution and generation notification. See
`docs/mts-execution-runtime.md` for the boot and chunk execution boundaries.

`LynxView::{update_data, reset_data, update_global_props, reload}` use the
existing ordered command/Worker links. Embedders serialize data and global
event argument lists into `String`; core passes them unchanged to JS, which
parses them and builds Worker messages. Update/reset/reload take a separate
processor-name `String`, an empty name selecting the default processor. All are
accepted once MTS boot has finished and otherwise return
`EngineError::NotReady`, like global events; the BTS Worker still loading is no
reason to refuse one. Initial data and props come from `ViewSources`; there is
no early props cache or initial-render update gate. MTS runs its own hook at
once and forwards the update to BTS with `Worker.postMessage`; the BTS runtime
queues that message behind its entry import and delivers it in order once the
import settles. Rust never sends page data to BTS — MTS `postMessage` is the
only path. A reload retains the realms and entry; the framework recreates
component state. See `docs/data-lifecycle-runtime.md` for processor selection,
snapshots, readiness, engine-event precedence and the BTS reload callback
boundary, and `docs/events-diagnostics-runtime.md` for native Context behavior,
the BTS GlobalEventEmitter and `LynxView::send_global_event`.

`LynxView::pump` records readiness before returning `ScriptFinished`, exposed
by `is_ready()`. Global events require that observed MTS boot and otherwise
return `EngineError::NotReady`; rejected events are never queued or replayed,
accepted ones retain host FIFO order. Internal pre-connection MTS messages
still wait for Worker construction. `ScriptReported` and `ConsoleMessage` are
nonfatal host notices, their BTS path ordinary Worker postMessage delivery with
JS-side dispatch. `lynx.getEngine()` returns one stable, realm-local
`EventTarget` whose listeners never cross the host boundary. Render, update,
component removal and global-prop events carry argument arrays, taking
precedence over legacy global hooks; listeners receive the engine as `this`,
with no `origin` field. The MTS `getCoreContext` and `getNative` sinks retain
and deliver nothing, and the module does not invent the background-only
`lynxCoreInject` realm.

#### Painter, frames, pacing and images

The core depends on `dom` and re-exports one seam: the `input` module
republishes `dom::Point2D` and
`dom::input::{InputEvent, InputKind, PointerId, PointerKind, PointerPhase}`.
Wheel deltas there are viewport CSS pixels; converting physical-pixel, line or
page units is embedder policy. No document, node or hit-test result crosses.

**`Painter` is a standalone public object, not the view's**:
`Painter::new(DrawTarget, width, height, device_pixel_ratio)` builds one over a
target before any view exists, `attach(&view)` points it at a view, `detach()`
releases it. It holds only a watch receiver and a `Weak` on the view's seat
(the view's command sender plus its handle on the host's resource system, as
one releasable thing), so a view dropped under a painter leaves it showing and
capturing its last frame. One interactive painter per view, one *live* view per
painter: a second `attach` is `EngineError::PainterAttached`, refused by the
seat's weak count, which `Painter::attach` is the only place to downgrade; a
seat whose view is gone needs no `detach`. Attaching drops everything derived
from the previous view — adopted snapshot, scroll intents, gesture arena,
resolved pixels, the target's compose key, since commit ids restart at one
per document — rebases the frame clock onto the view's timeline
epoch, seeds the `BeginFrame` sequence past what has been serviced, and writes
its metrics into the seat's watch: **the painter owns device metrics**, and
that write is also what binds the view. Detaching resets the same minus the
target and leaves the watch alone, so the last frame stays up and capturable
while the next page loads and no later flush parks.

Every entry point first polls the link, adopting the newest `Published` with
the pixels it draws before noticing a view that has gone, so a commit published
in the release turn is still adopted. A commit whose pixels could not be read
in the same step is not adopted, because a frame indexes its store's bitmaps by
draw order. The painter retains the newest frame and runs input routing,
gestures, compositor scrolling, composition and presentation inside the
embedder's own calls; vsync touches the OS only there. Commands needing the
live tree go to `bobcat-main`, which answers by publishing a later frame, so a
long JavaScript task cannot stop scrolling or re-presentation; only commits
publish, so a half-applied batch is unobservable. Scroll offsets stay on the
painter between refills and a scroll recomposes the retained frame without a
commit; when an offset leaves its `ScrollSlot::encode_window` the painter sends
`ToMain::Refill { offsets }` and the main thread answers with a recentered
commit. Embedders provide input, device metrics, OS initialization, a draw
target and IO primitives, and relay OS facts in
(`Painter::{dispatch_input, resize, set_occluded, refresh, pump, tick, capture}`
and `LynxView::pump`); they never start or steer the pipeline. Engine events are
enqueued and wake the host through the group's `EventRequester` for the next
`LynxView::pump`: `ScriptFinished` (entry-module boot), `StartupFailed`
(source/configuration/boot failure), `ScriptRunError` (a fatal script-runtime
failure in later owner-thread work), `ListenerFailed` (a listener that threw
during event delivery) and `TimerFailed` (a `setTimeout` or `setInterval`
callback that threw when it came due) — the last two separate because neither
is fatal: the walk continues, a repeating timer stays armed, and later events
and timers are delivered as normal. A frame the engine wants drawn rides the
same wakeup, and the `Painter::pump` answering it draws it.

**A host takes two turns per wakeup, and they are different calls.**
`LynxView::pump` alone advances the resource protocol past the startup sources
`create_lynx_view` already handed over: it hands each
`RequestSource` to the fetcher, gives it its `service_images` moment, names
every source the last paint walk discovered, drains the image inbox back to
`bobcat-main`, and returns the turn's lifecycle events — after a fatal event,
nothing further. `Painter::pump` draws the frame owed and asks the host for
nothing, so a host that wants an image to arrive takes the view's turn. Pacing
is the embedder's and the engine names no interval for it: after each turn
`owes_frame` answers whether a frame is still owed — a running animation, a
swap chain that had no image to give, a commit the turn did not draw — and the
host takes it at **its own next display frame**, whatever its display clock is
(a `CVDisplayLink` on the window's monitor, `requestAnimationFrame` in a
Worker). `is_animating` is the narrower fact, answered for any target, that an
offscreen host asks instead. **A realm timer is not the host's to wait out**:
every live realm, a view's and a worker's alike, has a `serve_clock` task
holding one pinned sleep on that realm's next deadline, re-armed only when the
deadline moves and fed by the watch that realm's epilogue publishes. Natively
that sleep is tokio's own time driver; on wasm32, where tokio's reads
`std::time::Instant` and would panic, `src/alarm.rs` serves it — one
process-wide `bobcat-alarm` Worker holding a heap of deadlines and the wakers
waiting on them, parked with `park_timeout` — and `crate::clock::sleep_until`
picks between the two by `cfg`. A draw that fails is the return value of
`Painter::{pump, tick, capture}`, reported once because there is no recovering
a lost surface; there is no `RenderFailed` event.

**The draw target is an argument to `Painter::new`, named once and kept for
that painter's whole life**: `DrawTarget::window(...)` takes anything
convertible into `WindowTarget` — a `'static` surface target, so a windowing
embedder passes a shared handle (`Arc<winit::Window>`) and a browser an owned
canvas — and `DrawTarget::Offscreen` asks for a windowless GPU target. Either
is built inside `Painter::new`, on the thread that will draw into it, the only
thread macOS lets a surface be created from. No target is attached later and no
painter has none; what a painter points at later is a view. An offscreen target
is refused on Wasm at construction rather than hanging, since building one
blocks the calling thread on a device request that thread's own event loop
would have to answer. `FrameSize::for_viewport` exposes the physical size
`Painter::new` and `Painter::resize` will compute, for a host that must size a
canvas's backing store first.

**Images are entirely the embedder's**; core fetches, decodes, caches and
retains no pixel of its own. A view's one resource system — its
`ResourceFetcher`, which is also its `dom::FrameImages`, owned by the
`LynxView` as an `Rc` and read by an attached painter through the view's seat —
is asked for one image at a time by source string (the `url(…)` value CSS
produced, or a replaced element's source): named through `request_image`,
answered through `ImageReports` with the intrinsic size layout needs, given its
moment in every `LynxView::pump` through `service_images` (where a host whose
loads complete off-thread forwards them into the reports), and read back
synchronously when a painter adopts a commit through `FrameImages::read`, which
carries a `dom::ImageSizeHint` — the largest device-pixel extent the frame
draws that source at, computed per draw under its transform and unioned per
source — so a host decodes to the draw rather than to the file. No container
sniffing, codec contract, cache policy or byte budget lives in `bobcat-core` or
`dom`; `crates/bobcat-resources` is the reference implementation every shipped
embedder uses. `FrameImages::retain` — the sources one resolve pass met, in
paint order — carries **no default body**, so every store writes its working
set where someone can see it rather than inheriting a silent no-op.
`LynxView::prefetch_images` warms sources ahead of the walk that would discover
them.

#### Workers, the BTS and cross-thread messages

**A group also owns a second thread and a second `QuickJS` runtime,
`bobcat-workers`**, for the worker realms every view in it shares.
`LynxGroup::new` starts it beside `bobcat-main`, and the group handle's drop
joins it after `bobcat-main` has returned, so a thread that will not start
fails the *group*. `bobcat-main` holds one sender on it and only ever sends:
start a context with its script, post to a context, stop a context — and hears
events back. A released view stops its own workers by sending each that stop.
The price is one parked thread and one idle runtime per group; there is no
lazily-built state and no lock. The runtime is separate from `bobcat-main`'s
because worker script must not stop the thread that owns the document, and
since `QuickJS` binds a runtime to one thread, no path runs from a worker realm
to a `LynxDocument` and no value of either runtime can be named by the other.
One realm per live worker, and the group's workers take turns. **One task per
live worker, and a worker's whole state is that task**: a `WorkerStart` carries
its key, its name, the one-shot its script will arrive on, the receiving end of
its message channel, the sender its events go back on — the creating MTS
realm's `WorkerEvent` channel — and the Worker's own cancellation token. MTS
routes events through weak references to JS Worker objects; their finalizers
and explicit `terminate()` release sending handles, and releasing the MTS realm
closes its remaining senders. Host functions reference the channel owner
weakly, so queued finalizers cannot keep a released realm's workers or group
thread alive. The script wait is a `biased` select over the message channel
first and that token behind it, so a `terminate` landing in the same instant as
the script wins and a worker told to stop never boots. The timer machinery both
realm kinds run on — the schedule, the two host members, the firing loop — is
`crate::timers` beside `crate::clock`, owned by neither thread.

**The main-thread `Worker` class is exported by `bobcat-internal`.** It is an
explicit ESM import, creates a distinct context on the group's existing
`bobcat-workers` thread, and supports `postMessage`, `terminate`, `onmessage`,
`onerror` and the shared EventTarget listener methods. It uses module scripts
(also with omitted options) and the worker scope's structured-clone transport,
so `undefined`, `NaN`, `Date`, `BigInt`, typed arrays, cycles and shared
references survive, while a function, `Symbol`, `Map`, `Set`, `RegExp`,
`Error`, `DataView` or accessor property throws synchronously at the
`postMessage` call; transfer lists remain pending. External ESM imports load
through the view's resource fetcher and support TLA. `main/workers.rs` installs
its three native operations — `createWorker`, `sendWorkerMessage`,
`terminateWorker` — before entry boot. The `Start` goes out before the host is
asked for anything, `SourceRequest::Worker` carries the entry's resolved URL as
its base, and the host is handed the far end of the one-shot that already rode
to `bobcat-workers` inside that `Start`, so the script reaches the worker
without a main-thread turn. Every concurrent worker request is preserved.
Worker entry/import requests use the Worker's cancellation scope, and host
release does not cancel it ahead of JS disposal. Once the MTS realm is
released, closing its senders ends remaining Workers, including after failed
boot; Rust has no Worker termination sweep. A `WorkerEvent` delivers messages
and errors to the owning realm; worker errors also produce nonfatal
`EngineEvent::WorkerFailed`. See `docs/runtime-architecture.md` for the
transport and lifetime boundaries.

**After the MTS entry import succeeds, boot creates a BTS Worker** named
`lynx-bg` through that same class, using the engine entry `bobcat:bts`, which
installs its JS initializer from `bobcat:bts-runtime` and returns. Its first
Worker message supplies initial data and starts the optional
`ViewSources.background_entry` import. MTS JavaScript owns BTS disposal: send
`dispose`, await `disposed`, then terminate its Worker. BTS calls the current
app hook, reports any throw and replies after an ordinary Promise boundary. The
MTS disposal Promise also handles repeated destroy notifications, and disposal
bypasses an unfinished BTS entry import. Object observers follow web-core: a
plain object registered with a JS `FinalizationRegistry` that directly invokes
its callback (`docs/destruction-runtime.md`). Raw BTS application entries
explicitly import their bindings from `bobcat:bts-runtime`; neither runtime
installs `globalThis.lynx`. XML uses this identical startup path, and the
bootstrap contains no application source and does not fetch it in advance. A
worker carries a `SourceRequester` that sends module requests directly to the
view's resource host. ESM completion and timers continue during entry TLA;
posted messages wait for entry settlement, and each completion shares its
worker's cancellation token. ReactLynx compiled module execution and
lazy-bundle APIs remain a later layer over this transport; bypassing
`lynx_core.js` does not require its `requestScript`/`readScript` source-text
interfaces (`docs/worker-resources-runtime.md`). Without an entry, only the
built-in environment runs, and all workers use the same scope and protocol.

MTS `lynx.getJSContext()` and this BTS Context are stable
`CrossThreadContext extends EventTarget` instances returned directly by
`createCrossThreadContext`. `dispatchEvent({type, data})` validates the string
type and data property, captures the public envelope, sends to the peer and
returns `0`; Context `postMessage(value)` sends a message event. Listeners
receive the original null/undefined data and an undefined receiver, and ignore
DOM listener options. The shared `bobcat:event-target` EventTarget every
Context, `Worker` and engine target extends follows the DOM's inner-invoke
rule: a listener that throws is reported and the walk continues with the next —
in the MTS realm through `lynx.reportError` and the host's `reportScriptError`,
as a nonfatal `ScriptReported`; in a worker realm through the worker global's
`reportError`, reaching the parent `Worker`'s `error` event and a nonfatal
`WorkerFailed`. Origins identify the sending CoreContext or JSContext. MTS
queues payload references until the Worker is connected; Worker postMessage
takes the structured-clone snapshot above for early and connected sends alike,
and `toJSON` is never consulted. Do not add a custom codec or a deep clone on
top of that transport; a value it refuses throws at the call. A worker's own
task queues what is posted until its bootstrap has evaluated, and BTS JS waits
on the application import before delivering later messages, so application
listeners exist before first delivery. Raw XML adapters supply the optional
entry; a compiled bundle's manifest paths and string custom sections are each
registered with the host as an ES module beside the page's input URL, and the
BTS boot script hands that URL alone to the modules layer, which is the base
every `requireModule` of a bundle path resolves against before loading it
synchronously. Each view costs one additional realm on the group's
worker runtime.

MTS boot does not await BTS: `ScriptFinished` means MTS boot finished — the
entry module evaluated, its top-level await settled, and its first flush
committed. The BTS Worker's state is no part of it, so a BTS entry whose
top-level await never settles does not keep the view from becoming ready. A BTS
entry that throws is reported like any worker script: `reportError` in the
worker realm surfaces it at the `Worker`'s `error` event and as a nonfatal
`WorkerFailed`; the BTS keeps running and still takes messages, and no BTS
failure ends the view. MTS keeps its Worker reference after that Worker ends; a
post to an ended Worker is dropped by the host, and the pre-connection FIFO
holds only what the MTS entry sends before boot constructs the Worker.

BTS also exposes stable `getApp()` and `getNativeApp()` objects. The current
app hooks receive `OnLifecycleEvent`, `publishEvent`, `publicComponentEvent`
and `callDestroyLifetimeFun`; the native app's `callLepusMethod` invokes a
named MTS global function and asynchronously returns its resolved result to an
optional callback. String `__AddEvent` handlers publish snapshots containing
target/currentTarget `dataset`, `id` and `uid`, never handles. Current PAPI
elements have no component metadata and use `publishEvent`; explicit component
calls preserve the supplied ID. An explicit JS engine `__DestroyLifetime` event
starts the same JS disposal Promise used by MTS teardown, terminating BTS only
after its acknowledgement. Rust starts and awaits MTS disposal through the
existing ESM evaluator and routes ordinary Worker events while it waits; it
neither identifies BTS nor calls its hook.

#### The host module and the Element PAPI runtime

The private `MainThreadRuntime` registers the native QuickJS ESM
`bobcat-internal:host` as one Rust-backed named function export per member, and
`packages/bobcat-element/src/native.d.ts` is the authoritative enumeration: a
`declare module "bobcat-internal:host"` block for the MTS realm and
`"bobcat-internal:worker"` for a worker's, which carries `postWorkerMessage`,
`closeWorker` and `invokeNativeModule` and nothing else. The MTS members group as the document's own
life, tree vocabulary over numeric `NodeId`s, attributes and style, selector
queries, the commit, the event-name edges, timers, the page-data triple handed
over once as plain JSON and processor-name strings the realm alone reads, the
native-module table handed over once as one length-prefixed record, the
stylesheet pair, the diagnostics pair, the display-frame demand, and the three
worker operations. Those four one-shot strings do not arrive separately:
they, the entry's own text and resolved URL, and the BTS entry specifier are
one `RealmStartup`, which is everything a realm is opened with and nothing
that is ever updated — `LynxView::update_data`, `update_global_props` and
`reload` reach the realm through `ToMain::PageUpdate` and never touch it.

The members that answer with a list encode it in the return string, since
the boundary's value type carries no array: `attributeNames` and
`getComputedStyleMap` as the length-prefixed record `setInlineStyles` accepts,
the latter a flat name-then-value sequence; `childElementIds` and
`queryElementIds` as comma-joined ids, and `callElementMethod`'s
`boundingClientRect` as the four comma-joined numbers `left`, `top`, `width`
and `height` — none of which needs a length prefix.

Each call is a plain owner-thread mutation, and `__FlushElementTree` runs the
style + layout + paint commit and publishes one immutable `Arc<CommittedFrame>`
on the view's watch. `Node`'s arena backpointer is a raw pointer, so a
`Document` is not `Send`, and its realm never leaves `bobcat-main`. The
boundary validates primitive arguments, live IDs and tree-mutation
preconditions before entering `dom`, returning misuse as a JavaScript exception
(unexpected internal panics remain fatal on abort-only Wasm). An unflushed
batch may present once its evaluation ends — web-core's visibility model.

Beside the host module, each runtime registers a fixed set of built-in ESM
sources in QuickJS's loader: `install_shared_modules` for `bobcat-main`,
`install_worker_modules` for `bobcat-workers`, the specifiers in `esm.rs`, and
the per-runtime lists with their TypeScript sources in the
`packages/bobcat-element` section below. The worker list is deliberately
different, so importing `bobcat:element` or `bobcat:runtime` there fails to
resolve rather than failing late. `bobcat:bts` is the BTS Worker's engine
entry, `bobcat:boot` the MTS boot module's own specifier. A worker's own script
is *inlined* into the one module its realm evaluates, as `ENTRY_PREAMBLE`
carries the MTS entry, and never registered on the runtime. The Element module
imports native operations directly from `bobcat-internal:host`; no host object
and no element member is installed on `globalThis`.

The PAPI runtime exports the supported Element PAPI only as named ESM bindings,
which transformed entries receive through the prepended import, and **the
header table of `packages/bobcat-element/src/element-papi.ts` is the
authoritative enumeration** — it names every member and what backs it, and
`ENTRY_PREAMBLE` in `main/runtime/lib.rs` imports exactly that set. By kind:
every ReactLynx Snapshot constructor except `__CreateFrame`; all six tree
mutations; the properties and queries a Snapshot's `create`/`update` functions
write through and read back, among them `__SetInlineStyles` and the name-based
`__AddInlineStyle`, with `__SetCSSId` accepted and ignored; the readback pair
`__InvokeUIMethod`, whose one UI method is `boundingClientRect`, and
`__GetComputedStyleByKey`, neither of which commits anything — both report the
last completed pass and leave the decision to flush to the caller; the event
registration and propagation members, `__AddEvent` and `__AddEventListener`
included; `__CreateList` with `__UpdateListCallbacks`; and
`__FlushElementTree`. Everything else is not implemented, `__CreateFrame` and
`__DropElement` (which no web-core generation has) included; a bundle reaching
for another member fails at the missing name with a precise `ReferenceError`,
not silently.

`__SetInlineStyles` keeps the whole-value policy in JavaScript: a string is one
`style` attribute write, a record crosses in a single `setInlineStyles` call as
a length-prefixed payload — `<utf16Length>:<text>` fields, name then value, in
enumeration order — from which the host builds one declaration block from
empty, so a value may contain any character, `;` included, without escaping.
Ordinary camelCase keys are hyphenated; case-sensitive `--*` custom property
names pass through unchanged. The host operation implements the name/value
subset of CSSOM `style.setProperty` (no priority argument, so an embedded
`!important` is invalid) and intentionally has no numeric-style-id variant:
`__AddInlineStyle` updates one named property in the existing block and removes
it for empty/nullish values, and numeric Lynx CSS property IDs remain
unsupported on both surfaces. `__CreateList` consumes only its numeric
parent-component argument; the callbacks it and `__UpdateListCallbacks` file
are read by `__SetAttribute(list, "update-list-info", …)`, the one name that is
a list command rather than an attribute. That member is the list *data*
protocol and it is web-core's, step for step: the batch is serviced in a
microtask — the callbacks a card files immediately after writing the
operations are therefore the ones that serve it — every `insertAction`
position is built by `componentAtIndex` and inserted at that index unless it
is already there, and every `removeAction` child is handed to
`enqueueComponent` and removed, the i-th removal taking the child at
`position - i`. `updateAction` is ignored and `componentAtIndexes` is filed
and never called, as in web-core. Cell *recycling* — a pool, a window,
`enableReuseNotification` — remains part of the unimplemented list surface.

An element handle is an `EventTarget`, and the registration half lives in
`packages/bobcat-element`: listener closures live only in the realm and nothing
about a handler ever crosses into Rust. The parts of `__AddEventListener` that
duplicate `__AddEvent` are deliberately absent: `closure_type` selecting a
handler string and `bind_type` selecting Lynx's `catch` forms are not honored.

`__AddEvent` is the other registration form, and the one ReactLynx's compiled
output uses for every `bind*`/`catch*` prop: it files handlers under a Lynx
dispatch form — `bindEvent`, `catchEvent`, `capture-bind`, `capture-catch`,
`global-bindEvent`. **Two handler kinds, filed apart**: a *string* is an opaque
background-thread handler name, an *object* is a worklet, each element holds
one of each per event name, and a call of one kind never disturbs the other, so
a `main-thread:bindtap` and a `bindtap` on the same element both run (native
Lynx's `static_events_` beside `lepus_events_`; web-core's cross-thread-handler
map beside its run-worklet map). Only a nullish handler clears, and it clears
both kinds. Within a kind the key is the event *name* alone, the dispatch form
carried inside the entry, so filing `catchtap` over `bindtap` of the same kind
replaces it, form included. A worklet runs through the card's own `runWorklet`;
a string is published with a snapshot of the event through the MTS runtime. A
`catch` form ends the walk before either kind is delivered. Anything else
non-nullish is ignored, which is web-core's behavior. `global-bindEvent` is
filed in its own slot and delivered in a pass of its own after the two path
passes, not a pass over the path: every element holding a global registration
for the name is delivered to, in registration order, whatever path the event
took and whether or not a `catch` ended the walk, so a global delivery has
`eventPhase` `NONE`.

The walk is the realm's. The host computes the event path while it holds the
document, releases it, and makes one call to the Element module's
`__BobcatDispatchEvent` export through
`quickjs::ScriptEngine::call_module_export`, the one Rust-to-JS path in the
tree, carrying the whole path: the standard's bubble steps, target-first, as
two comma-joined decimal id strings — the nodes, and position for position each
step's shadow-retargeted target — plus the name and then numbers alone: the
`timestamp` (milliseconds on the view's timeline), the position the `detail`
reports, the wheel delta (`undefined` for every event without one, which keeps
the two keys out of the `detail`), and four numbers per touch point
(`identifier`, `x`, `y`, flags) for the four touch events. The realm builds
the `detail` object and the three touch lists out of them; no JSON is
formatted on the host side and none is parsed in the realm. One call
is one dispatch, so one event object serves it, and the host keeps no listener
index at all. The realm runs the capture, bubble and `global-bindEvent` passes,
derives `eventPhase` per step, and ends the dispatch itself; neither
`stopPropagation` nor `stopImmediatePropagation` crosses the boundary.

One event deliberately does **not** take that path, because it is the
engine's own rather than script's: css-contain-2 §4.4's
`contentvisibilityautostatechange`. The commit that determines
`content-visibility: auto` relevance leaves the elements whose *skipping*
changed in `dom`'s own queue; the page's epilogue posts **one fresh entry**
for the batch, which is the spec's "posting a task" — the event is never
delivered inside the entry that committed — and that entry calls
`Document::dispatch_content_visibility_changes`, which fires each one through
`Document::dispatch_element_event`. The walk is `dom`'s, the listeners are
`dom::CustomElement` definitions (the engine's components — `<image>` today,
`<list>` next) reached through `CustomElement::handle_event`, and **no realm
is entered at all**: there is no export, no Lynx event name, no
`__AddEvent`/worklet table consultation and no `global-bindEvent` pass, and a
card's own `addEventListener` for that name never runs (user ruling,
2026-09-21). `bubbles = true` is a recorded choice where the spec is silent
(Chromium bubbles, WebKit and Gecko do not); `composed = false`; nothing
cancelable. What the
host is told is the *name* set the painting side routes against, and only its
global edges: `listenerNameOpened(name)` for the first registration for a name
anywhere in the realm, `listenerNameClosed(name)` for the removal of its last,
the count behind them kept in the realm, including the registrations a
collected handle takes with it, which its `FinalizationRegistry` record closes.

`__SetCSSId` is a sink rather than an implementation: it names the author-CSS
scope an element cascades in, and no layer lowers a decoded `StyleInfo` into
**scoped** author rules yet (ingestion has landed but mounts every fragment
globally; web-core writes `l-css-id`/`l-e-name` attributes, native Lynx keeps
css_id on the element). A compiled card calls it while installing its snapshot
runtime, so it accepts the call and drops the id. The scoping behavior lands
with the ingestion side that reads it, together with the parent-component
css-id inheritance that feeds it.

`Document::drop_element` frees exactly the node the collected handle named: its
**element** children are unlinked and go on as detached roots, each held by its
own handle, while what no handle could ever name goes with it — host-owned text
nodes and a host's shadow tree in full. Generated `raw-text` content has no DOM
node.

#### Page policy: tags, text and the UA sheet

Core owns Lynx page policy in its `tree` module — the `page` root tag,
`Viewport`/stylo `Device` construction, the Lynx UA cascade defaults, and the
components the engine defines — while tag vocabulary, handle lifecycle, and the
PAPI member surface live in `packages/bobcat-element`. The module is one file
per tag, each owning that tag's UA rules and its tests: `tree::raw_text`
(generated-content CSS and the rules that dissolve a carrier into the `text` it
is written inside), `tree::text` (the paragraph attribute limits and what may
generate a box inside a run), `tree::image` (the `src`-to-replaced-content
reflection and its UA box), `tree::scroll_container` (`scroll-view` and
`list` as scroll containers — which axis scrolls, which one clips, and which
way the subtree stacks, from `web-elements`' own `scroll-view.css` and
`x-list.css`; `enable-scroll="false"` leaves the box a scroll container only
script can move), and `tree::blur_view` (`blur-radius` reflected into a
`backdrop-filter: blur()` presentational hint, under both the native tag
`blur-view` and web-core's `x-blur-view`, as a CSS length rather than
web-core's `parseFloat` — the one tag module with no UA rules of its own,
since a blur view is a container and nothing else). `tree::ua_sheet` owns what
those tags agree on, the order
they cascade in, and `PageConfig`; `tree/lib.rs` only mints the document they
describe. **A tag's attribute-to-CSS mapping belongs to that tag's own
`dom::CustomElement`, never to a shared name-keyed dispatcher** (user ruling,
2026-09-21): `text`'s paragraph limits, `list`'s lane count and sticky offset,
`list-item`'s size estimate, `image`'s `src` and `blur-view`'s `blur-radius`
are each reflected by the component `tree/lib.rs` defines for that tag, in the
`attribute_changed_callback` the `__SetAttribute` write itself raises, so the
runtime's attribute members perform the DOM mutation and nothing more. That
order is mostly documentation, with one exception that is
mechanism: `image`'s child suppression ties on specificity with the `display`
rules `view`, `scroll-view`, `list`, `blur-view`, `x-blur-view` and `wrapper`
carry, so it wins only by being assembled last.

The native host-module functions call `dom::Document` directly. Element
identity is the DOM `NodeId`, which is also the element's Lynx `unique_id` —
one number, issued by the DOM, never reissued; the JS side mints no ids.

**Text** reaches the engine as an attribute and becomes generated paragraph
content. Script writes `__CreateRawText(value)` — a `raw-text` element carrying
`text` — and its UA rule `raw-text { content: attr(text); }` feeds the same DOM
generated-content path as author CSS; `text[text]` uses the same rule. There is
no custom element reflection or synthetic DOM text child. Content replaces
rendered children while preserving DOM structure. The element's primary text
style shapes and paints its run; attribute changes invalidate the paragraph,
and unchanged text/style reuse its shaping. `text` establishes one flattened
paragraph whatever `defaultDisplayLinear` says, `wrapper` is
`display: contents`, and `raw-text` dissolves into the `text` it is written
inside (`display: none` anywhere else) with
`white-space-collapse: preserve-breaks`, the one place Lynx keeps a literal
newline. Sibling runs and nested text share the establishing element's
paragraph. Core reflects `text-maxline`, `text-maxlength` and
`tail-color-convert` into `--lynx-text-maxline` / `--lynx-text-maxlength` /
`--lynx-tail-color-convert` presentational hints through
`Document::set_presentational_hint`, and marks a `text > inline-truncation`
subtree with `--lynx-inline-truncation` so `crates/dom` can lay it in at the
clamp without naming a Lynx tag. Each element's optional declaration block
enters Stylo at `CascadeOrigin::PresHints`, independently of inline style, so
author CSS can override a limit and replacing or removing inline style reveals
the attribute's current value. The UA registers them with `<integer>` syntax
and `inherits: false`. DOM's borrowed `StyleView` reads their computed values
through `TextContainerStyle`, and `BlockStyle::from_container_style` consumes
those inputs. Normal and animated style refreshes merge effective limit changes
into layout damage to re-break the retained glyphs through existing box
invalidation. Original attribute strings remain available to selectors. No text
custom element, separate paragraph-limit storage, or public limit setter
participates. Computed `text-overflow` selects clip or the literal-dots
ellipsis. The text `layout` event remains unwired.
`docs/text-measurement-and-ifc.md` records the earlier design investigation;
current integration status lives in `docs/tracking/css-text.md`.

### crates/quickjs-rust-bridge

An owner-thread-bound safe Rust wrapper around the pinned `vendor/quickjs`
submodule. It exposes QuickJS's two objects as two types: a `Runtime` (heap,
atom table, job queue, execution limits, registered module source) and the
`Context` realms created on it, as many as the host wants, all on the owning
thread. Realms share what the runtime owns and nothing else — a `Value` never
crosses between them, one registered module source compiles into a separate
instance per realm, native host modules are installed per realm under one
runtime-wide specifier namespace, and a *failure* belongs to a realm even
though the queue it came out of does not: a pending-job drain names the realm
it reports for, runs every queued job whichever realm queued it, and reports
only that realm's unhandled rejections. A sibling's stays queued for the
sibling's own next drain and is freed with that realm; a caller that has
reported one realm's failure can drop what that realm still has queued, since
one throw rejects a module's evaluation promise and everything awaiting it.

It owns the QuickJS C build and the narrow unsafe FFI shim, realm/value
lifetime and affinity checks, exact ECMAScript string conversion, exception
sanitization, pending-job pump, synchronous preloaded source/native-module
loader, loaded-module namespace access, and module-evaluation Promise state.
It also owns one synchronous load-and-compile entry, which is what a realm's
`require` is written over: `register_synchronous_loader` exports a
`loadModuleSync(url, parameters)` on a native module, backed by a host `FnMut`
that answers one URL at a time. The bridge holds only the mechanism — it
resolves nothing, caches nothing, and stays ignorant of URLs and media types,
which of CommonJS, JSON and an ES module a source is read as being the host's
answer beside the response URL and the text, or, where the host declines to
say, QuickJS's own syntax detection over that text. What it does own is the
order: the response URL is copied and the text compiled (under that URL, in a
wrapper of the caller's parameter list), JSON-parsed, or copied into the
realm's own source table and compiled as a module *before* anything of the
file is evaluated, which is the first point author code can run and so the
first point another load can replace the buffers the host lent. A module is
linked during that compile, so the imports it pulls in are loaded — each
through the same borrowed-until-the-next-load host callback — before its own
evaluation starts.
Every heap allocation made by the C shim or the five compiled QuickJS C
translation units is redirected through a private C ABI into Rust's global
allocator; a fixed aligned prefix supplies the size required for matching
`realloc`/`free` and QuickJS memory accounting. QuickJS's `snprintf` and
`vsnprintf` calls are likewise redirected to a crate-private wrapper around the
pinned, allocator-free `nanoprintf` header, so native and Wasm builds share one
integer/string formatter without importing libc `stdio`, `FILE`, locale, or
another heap. All targets compile the C sources against the same crate-private
`stdlib`/`stdio`/`inttypes`/`string`/`math` declaration facade: host allocation
and the audited C gaps route to Rust, stack and basic memory operations remain
compiler builtins, and the bridge-unexposed `FILE`/standard-stream diagnostic
API is compiled out rather than modelled as a platform ABI.

The realm deliberately does not install JavaScript shared-memory primitives:
both `Atomics` and `SharedArrayBuffer` are absent, while ordinary
`ArrayBuffer`, typed arrays, and `DataView` remain available. This does not
disable Rust-side atomics used for interruption or host synchronization.
Because QuickJS formerly coupled its process-global class-ID mutex to the same
feature, the bridge allocates its one host class ID through a Rust `OnceLock`
and registers that ID separately in each runtime, preserving concurrent native
realm creation.

It also owns the **host-function seam**: `Realm::function`,
`define_global_function`, and `register_host_module_function` back a JS
callable with a Rust `FnMut`, dispatched through one C trampoline
(`JS_NewCFunctionData` + a realm-owned callback table reached via the context
opaque). Host callbacks speak `HostValue`, a primitives-only boundary
(undefined/null/bool/number/string) — ordinary objects, arrays, functions,
symbols, and ill-formed UTF-16 strings are rejected on the way in rather than
lossily converted, element identity crosses as plain numbers, and handle
objects never leave JavaScript — which keeps ordinary callbacks leaf
operations. Their `FnMut` closure is borrowed through a `RefCell`, so reentry
is refused rather than aliasing it; a panicking callback becomes a JS exception
and leaves the slot usable.

A closure's lifetime follows its JS function object rather than the realm: the
closure sits at its own stable heap address, which a companion JS object holds
and the collector hands back through a finalizer, so nothing is indexed,
recycled, or aliasable by a stale reference, and discarding a function drops
its closure. Without this a realm registering a handler per element per update
(events, worklets) would accumulate every closure it ever made. The finalizer
only *records* the address; the drop happens at the next `&mut Realm` entry
point, because a handler may own a `Value` whose `Drop` calls `JS_FreeValue`
and re-entering QuickJS from inside its own GC is unsound. Capturing a
same-realm `Value` is therefore safe, but forms a reference cycle that leaks
the realm unless the function is collected first. The crate must remain
independent of Bobcat, the DOM, resources, and runtime policy — it knows
nothing about Lynx.

### crates/bobcat-resources

The cross-platform reference resource system: one `ResourceFetcher` for macOS,
Linux and the browser, which every shipped embedder uses. It is the worked
example of what the protocol expects, not part of the protocol, and core stays
free of resources. Five things live here and nowhere else in the workspace.

**Transports**: contents the embedder registers under any URL
(`Resources::register` and `register_style_sheet`, or a `ContainerInstaller`'s
own `Registrar` for the sections of a lazy container it recognized in a plain
`SourceRequest::Fetch`'s bytes — a decoded bundle's scripts
and `StyleInfo` sheet, a browser-fetched script's bytes, a test's PNG), `data:`
URLs, `file:` URLs natively, and `http(s)` through the platform's own client:
libcurl loaded at runtime with `libloading` on macOS and Linux (no build-time
link, no bundled HTTP or TLS stack; a host without it gets a precise
`Unavailable`), and the Render Worker's `fetch` in the browser.

**The `FetchIndex`**: the base URL every specifier resolves against, and the
set of URLs a plain `SourceRequest::Fetch` has **completed successfully** for,
written after the container installer ran. It is behind an `Arc` of its own
rather than inside the shared state, because `ViewResources::fetch_probe()`
hands a reader of it to `bobcat-main` and `bobcat-workers` while everything
else here stays on the embedder's thread — on wasm32 the shared handle is an
`Rc`, and two mutexes and a set of URLs are `Send + Sync` on every target. A
new scope gets a fresh one, like the fresh registry beside it; a failed fetch
and a failed install are never in it.

**A MIME-keyed preprocessing pipeline**: every payload is sniffed (image magic
beats the label, a label beats a byte scan, a BOM names a charset), classified,
and treated by class — text transcoded to UTF-8 with its BOM removed so the
engine's strict validation sees what a browser's decoder would have produced,
JSON validated, images container-sniffed and header-probed for their intrinsic
size without decoding a pixel, the rest passed through.

**Tiered caching**: decoded bitmaps in a memory tier under a byte budget with
the frame's working set pinned against eviction, and fetched bytes in a disk
tier under its own budget with RFC 9111 freshness, `ETag`/`Last-Modified`
revalidation, and the fetch cache modes mapped from `CachePolicy` (natively;
the browser's HTTP cache plays that role there). Stylesheet responses,
including pending loads and failures, are shared by resolved URL within a
resource scope. Preload hints populate that same cache; registration changes
invalidate the affected URLs.

**Platform image decoding**: no codec is compiled in — `ImageIO` on macOS
(`CGImageSourceCreateThumbnailAtIndex` with a maximum pixel size, so a photo
shown small is decoded small), gdk-pixbuf on Linux (loaded at runtime;
`gdk_pixbuf_loader_set_size` from the header probe), and the main thread's
`Image` element in the browser (the Render Worker fetches the bytes and hands
them over as a Blob), each asked to downsample during decode. Natively a load
is one task on the crate's own `current_thread` tokio runtime, built by
`Resources::new` and moved to a `bobcat-resources-driver` thread that drives
and shuts it down; its blocking pool (`max_blocking_threads = worker_threads`)
runs the transport read, the preprocessing and the decode, and a `Semaphore`
sized by `decode_parallelism` is acquired *before* a decode closure is
submitted, so a decode that has to wait holds no pool thread. A panic inside a
closure becomes that image's or source's reported failure. In the browser a
load is a local task instead. Either way completions are delivered through the
wakeup the embedder supplies and applied in the next `LynxView::pump` through
`service_images`.

The frame reads each image at the size it draws it: a resident bitmap far
larger than its draw is re-decoded at the drawn size in the background and
replaced; one that was evicted is restored inside the read from the retained
bytes or the disk tier — on the embedder's own thread, synchronously and with
no decode permit, which is why the transport keeps a blocking entry point and
`tokio::fs` is not adopted; and one drawn larger than it was decoded is refined
back up while the image has more to give. In the browser that restore is the
one place the Render Worker blocks: the main thread never waits, so a job's
mailbox in shared Wasm memory and `Atomics.wait` are what let a read that must
not miss wait for it (`crates/bobcat-wasm/js/image-decoder.ts` is the main
thread's half).

Shape: `Resources` is the shared system (registry, caches, executor, decoder;
cheaply cloned, bound to the embedder's thread) and the only holder of the
executor, so the runtime shuts down — without waiting for work already picked
up — when the last clone of the last scope drops on that thread.
`Resources::builder` yields the per-view `ViewResources` that
`LynxGroup::create_lynx_view` takes, carrying that view's `ImageReports`.

Recorded limits: only an image's first frame is decoded (no animated playback),
no `region-to-decode`, no `blur-radius` post-processing, and none of the
`<image>` element surface past `src` — the pipeline serves whatever source
string the paint walk names, today `url(…)` layers and the source an
`<image>`'s `src` installs through `Document::set_image_source`. The macOS
decoder is type-checked against the Apple target but exercised only where
ImageIO exists; the Linux decoder and libcurl transport are tested for real
against the system libraries, and the browser path is linted for wasm32 and
exercised only in a browser.

### crates/bobcat-cli (`cli` feature)

The native `bobcat` product over `bobcat-core`, depending on it plus
`bobcat-resources` and `bobcat-source`. `bobcat -i file:///…` content-sniffs
and boots one web bundle or one raw Lynx XML source card; other URL schemes are
rejected at the boundary.

The CLI is an **embedder** of the opaque `bobcat_core::LynxGroup`, `LynxView`
and `Painter`: it owns argument parsing, local input IO, the `PageSource`
instance, the reference resource system with the extracted scripts/styles
registered, the winit window and event loop, device metrics, input translation,
the stdin prompt, and PNG writing — and nothing of the pipeline. It builds the
view from the group and the painter from the window on its own thread and
attaches them. Every event handler is a relay into the painter
(`dispatch_input`, `resize`, `set_occluded`, clock ticks in headless mode).

The window it hands `Painter::new` is the draw target and nothing else: frames
and lifecycle events wake the event loop through the injected `EventRequester`,
and that turn ends in `about_to_wait`, taking both turns in order —
`painter.pump()` draws the frame owed, then `view.pump()` services resources
and returns what the realm had to say. Winit's `RedrawRequested` is not
relayed. Drawing there coalesces a turn's events into one frame and keeps the
vsync wait out of winit's proxy-event drain, which iterates until empty. The
painter goes first deliberately: the pixels a fatal script error left behind
reach the screen on the turn that reports it. The loop always waits — a realm
timer is not its deadline to keep — and what wakes it for a *frame* is the
window's own display: while `Painter::owes_frame` holds, a `CVDisplayLink` on
the window's monitor posts one wakeup per refresh and stops when nothing is
owed.

It renders one page: one group, one `create_lynx_view` given the author CSS and
entry MTS URL as a `ViewSources`, any group, resource or TLA boot failure
reported as `CliError::StartView`, and the preserved `ScriptFinished` edge and
any later `ScriptRunError` consumed through `view.pump()`. Headed mode builds
its painter over the window; headless builds one over `DrawTarget::Offscreen`
and relays synthetic vsync ticks into `Painter::tick`, whether a tick becomes
GPU work being the engine's decision. Fields drop in the order `vsync, painter,
view, …, window`, so the display link stops before what it wakes goes away and
the surface is released before the last window handle.

Its resource system is `bobcat-resources`: the decoded input's scripts and
stylesheet registered under `bobcat-memory://` URLs, the input's own `file://`
URL the base every relative `url(…)` resolves against, and a disk tier under
the user's cache directory, so a page's images — beside the input, inline as
`data:`, or on the network — load and decode through the platform. A load
completing on the fetcher's driver thread wakes the event loop exactly as a
commit does.

Headed mode uses a native winit window with display-backed vsync and tracks
both logical viewport size and device-pixel ratio; headless mode uses a
configurable synthetic vsync rate, skips catch-up bursts after slow frames, and
retains its Vello renderer, render texture and staging buffer across frames.
Both expose a GDB-like stdin command prompt (`continue`, `pause`, `frame`,
`screenshot`, `help`, `quit`; headless also `set/show vsync`). Screenshots are
captured only through that live prompt — there is no one-shot startup flag —
and PNG readback happens only on a screenshot.

It must not duplicate runtime, DOM, layout, painting, or source-lowering
policy: missing MTS/PAPI support remains a precise `bobcat-core` QuickJS error.
`bobcat-source` lowers a decoded `StyleInfo` into
`bobcat_core::PreparsedStyleSheet`, flattening every `css_id` fragment in
reverse-topological order so imported fragments precede their importers, and
each native embedder registers that sheet in `bobcat-resources` under the URL
it names in `ViewSources::style_sheets`. A bundle carrying non-zero fragment
ids warns that per-component scoping is not implemented rather than claiming
compatibility. For XML, a present `<style>` body instead uses the fetcher's raw
CSS-text arm and the fixed page configuration is `false`/`false`/`true` for
default linear display, visible overflow, and selector support; a present
background section is registered under the native in-memory URL
`bobcat-memory://lynx-xml/app-service.js` and named in
`ViewSources::background_entry`, so the view's BTS Worker imports and runs it.

### crates/bobcat-cli (`server` feature)

The `bobcat-server` HTTP screenshot **embedder** in the same crate, not runtime
infrastructure inside `bobcat-core`. It follows UI Judge's public capture
surface: `GET /health` and multipart `POST /screenshot/lynxml`,
`/screenshot/template`, `/screenshot/template/url`, `/screenshot/zip/upload`,
and `/screenshot/zip/url`. There is no JSON `/screenshot` route. All routes
require a safe `entry` path and route-specific `source`, `url`, or `file` part.
Viewports default to 800×600 at DPR 1, accept dimensions up to 8192 with at
most 2,621,440 pixels, and return raw `image/bmp` with
`Cache-Control: no-store`; BMP output matches UI Judge's top-down 32-bit
BITMAPV4HEADER/BI_BITFIELDS layout, preserving alpha without a second white
composite. Multipart fields share a 10 MiB bound plus 64 KiB framing and a
10-second upload deadline; remote URLs are bounded to 8 KiB.
`/screenshot/template` and `/screenshot/lynxml` accept `screenshotSettleMs`
(default 16) and `timeoutMs` (default 60000); the other routes use 500 ms and
60000 ms and reject timing fields. JSON/query parameters, duplicate or unknown
fields, and snake_case aliases are rejected. `initData` and `globalProps` must
be objects; `.lynxml` entries reject `globalProps` even when empty. Non-empty
page-data objects remain explicit 422 errors: the server does not forward them
to its views yet, though `ViewSources` takes both as JSON text. See
`crates/bobcat-cli/SERVER.md` for examples.

Axum accepts HTTP requests concurrently, but a bounded FIFO of eight waiting
jobs feeds one dedicated capture thread — the embedder thread for each job. It
starts a fresh `LynxGroup`, constructs its non-`Send` `LynxView`, builds a
`Painter` over `DrawTarget::Offscreen` beside it and attaches the two; both
stay on that thread, the view because it owns the host's resource system and
the painter because it owns the GPU target. It settles on a plain frame
interval, taking `view.pump()` then `painter.tick(false)` per step, and returns
its RGBA capture. Dropping that view releases its group, including the Lynx
main thread, QuickJS runtime and Stylo pool; no runtime is shared across jobs.
BMP encoding runs on Tokio's blocking pool after the view is gone, so it cannot
retain the view or hold the GPU lane. Queue saturation and an unavailable
worker are 503, input/render failures 422, encoding failures 500, and
capture/upload timeouts 408. A worker panic makes `/health` unavailable and
initiates graceful server shutdown.

Remote template/ZIP downloads follow UI Judge's public HTTP(S), no-credentials,
no-redirect policy, pin DNS results, and enforce 10 MiB/10-second bounds. XML
bytes and downloaded templates enter `PageSource`; archives use the shared
`ZipSource`, registering members at `zip:///` URLs in each job's resource
system without filesystem extraction. ZIP validation errors are 400;
unsupported source/rendering errors remain 422. Source-based native bundles
require a `root` module; real bytecode remains unsupported. It listens on all
IPv4 and IPv6 interfaces and has no auth, TLS, or CORS. Captures still require
trusted JavaScript: fresh groups on a capture thread do not provide UI Judge's
process isolation, and page subresources use the ordinary resource transport.
`timeoutMs` cannot preempt synchronous QuickJS execution, GPU driver calls, or
synchronous view teardown. Source fetching, HTTP policy, BMP encoding,
queueing, and server lifecycle stay outside core.

### crates/bobcat-wasm

The pure-Rust `wasm-bindgen` browser embedder and npm facade, built for
`wasm32-unknown-unknown` with shared memory. `loadTemplate` takes binary web
and source-based native bundles, delegating decoding, page configuration and
StyleInfo registration to `bobcat-source::PageSource`; the original response
URL remains the resource base. The Pages Canvas tab passes local ZIP bytes and
an entry URL through `loadZip` to `bobcat-source::ZipSource`. Each page gets a
separate resource scope, retaining archive assets after boot and isolating
image caches and completion queues while sharing the platform decoder. The
browser has no executor: each load is a local task on the Render Worker. The
service worker only provides cross-origin isolation headers; `loadLynxXml`
retains its XML-only, host-configured contract.

The browser UI thread is a JavaScript-only host coordinator: it creates one
explicit embedder Worker and transfers an `OffscreenCanvas`, but never
instantiates Wasm or owns engine state. That Worker initializes the module,
constructs one opaque `LynxGroup` and one `LynxView` per page through
`BobcatRenderer::load`, keeps **one `Painter` for its canvas across page
loads** (rebuilt only when it is missing or its target has failed), permanently
owns every thread-affine GPU object — crates.io Vello 0.10/wgpu 29 Device,
Queue, Surface, Renderer, and OffscreenCanvas — and uses `wasm_thread` to
create the two Workers each group is made of: its nested Lynx main/VM Worker
and the worker-realm Worker beside it. A `load` is `painter.detach()` → drop
the old view → drop its group (ending the Lynx-main Worker and then the
worker-realm one) → new group and view → `painter.attach(&view)`, and the
canvas is deliberately *not* resized along the way: it already carries the
right resolution, and setting a canvas's size clears its bitmap, blanking the
previous page's last frame. `BobcatRenderer::pump` stays one method and takes
both turns in order, the painter's first. That Worker also spawns its group's
Rayon style Workers with `wasm_thread`, leaving the vendored Stylo sources
unchanged. Core creates its owner-thread-bound QuickJS realm inside that
Worker; Element-PAPI batches, Stylo/Rayon, layout and render hand-off
synchronize through Rust channels, mutexes, atomics and the shared Wasm memory
exactly as natively. JavaScript `postMessage` is only the browser host boundary
(initial Canvas transfer, URL-based script requests/results,
resize/input/lifecycle) or a library's Worker bootstrap control plane; it is
not a DOM/render reconciliation protocol. URL requests are serialized, and a
lost-wake-safe `EventSignal` Promise wakes script completion independently of
Worker rAF, so a hidden page may pause drawing without stranding the `load`
Promise. The UI facade, nested VM Worker startup and built-in QuickJS
configuration impose no wall-clock deadline: QuickJS drains its owned pending
jobs and waits for the TLA boot module's evaluation Promise to settle at its
host checkpoint, and there is no browser microtask-completion protocol. The
bridge retains an opt-in execution timeout for its direct users and tests.

A Wasm instance owns nothing of Stylo's but the Worker bootstrap
`configure_wasm_workers` installs — one script URL, which every Worker a group
spawns is made of — while each `LynxGroup` owns its Lynx-main Worker,
worker-realm Worker, style Workers and both QuickJS runtimes, and each
`LynxView` its own realm, document and endpoints, as natively. Every public
`BobcatCanvas` gets a separate Render Worker and Wasm instance; a renderer
holds neither group nor view until `BobcatRenderer::load` builds both, and each
later load replaces them. A page gets a group of its own rather than reusing
the renderer's, because the script runtime is the group's: a page loaded twice
would otherwise register its entry module a second time under a name the
previous load already took. Dropping the view stops it and dropping its group
ends the Lynx-main Worker and then the worker-realm Worker, once the Lynx-main
Worker has released the document and thread-bound QuickJS realm; ending is all
it is on this target, since `panic=abort` leaves a trapped Worker never
signalling its join handle, so wasm teardown says the goodbye and does not
wait. Replacement construction starts only after that teardown. The transferred
OffscreenCanvas, module instance, configuration, latest metrics, resource
provider, registered font containers, selected default font family, and Stylo
worker *count* are the renderer's own, reapplied to each group it builds, while
the workers belong to the group and retire with it. Registered script and
stylesheet bytes remain available until the startup outcome arrives; cleanup
leaves ZIP assets and the next page's staged sources intact. The Render Worker
is not a pool member; the group's Lynx-main Worker is index zero of the pool it
builds, and the rest are managed Workers it spawns. `BobcatRenderer::create`
therefore takes a count of one to `MAX_STYLE_THREADS`, counted the way
`StyleThreads` counts everywhere — the Lynx-main Worker included — and the
facade asks for the machine's threads less the Render Worker. The UI never
blocks, while Worker-side Rust may block wherever the native runtime does. The
browser target enables `parking_lot_core/nightly` so transitive Stylo/wgpu
parking_lot locks use Wasm atomic wait/notify instead of the non-atomic backend
that panics on contention.

Release packaging pins Binaryen 132 through the JavaScript workspace and runs
`wasm-opt -Oz` after wasm-bindgen with an explicit mirror of every enabled
Rust/LLVM Wasm feature; the build rejects a different optimizer version instead
of accepting wasm-pack's older fallback. Package verification requires the
optimized module to omit its debugging `name` section while retaining
`target_features`. Browser builds disable Parley's `complex-scripts` feature to
avoid embedding ICU's multi-megabyte CJK and Southeast Asian dictionaries;
native targets retain it, so grapheme segmentation, shaping and ordinary
Unicode line breaking remain available while Thai, Khmer, Lao and Myanmar text
may use cluster-level emergency breaks and report a larger intrinsic minimum
width. `wasm_thread` is pinned to the upstream `spawn_from_worker` change,
whose crates.io release otherwise forwards nested spawns to a parent protocol
handler an explicit embedder Worker does not have; Chrome 135 supports the
resulting nested module Worker.

Page sources arrive through the Render Worker's own `fetch`: it registers the
raw stylesheet and entry-MTS bytes with the `bobcat-resources` system it owns
and calls `BobcatRenderer::load(entry_url, style_sheet_urls)`; the entry's
final response URL is the ESM specifier `bobcat:boot` imports and the base its
images resolve against. Images are fetched by the resource system through the
same Worker `fetch` and decoded on the main thread by an `Image` element in the
package's `js/image-decoder.ts`, over a `MessageChannel` whose Worker end the
facade hands to `BobcatRenderer::create` at init. `loadLynxXml(url)` fetches an
XML envelope once, decodes it with the web loader's replacement-mode UTF-8
behavior, parses it with `bobcat-source::xml`, and hands any raw stylesheet and
its main-thread body to the same `load`; both are repeatable. The exported
`LYNX_XML_PAGE_CONFIG` names the source format's fixed page defaults, which a
host may still deliberately override. The optional background body is
registered at its section URL, `<final-response-URL>#background-thread`, and
named in `ViewSources::background_entry` for the view's BTS Worker.

Transferring the canvas does not transfer its DOM event target, so the
`BobcatCanvas` facade retains that element and forwards active
`pointerdown`/`pointermove`/`pointerup`/`pointercancel` sequences. It claims
each accepted pointer, maps client coordinates through the canvas bounds into
viewport CSS px, and sends compact fire-and-forget records through the same
ordered Render-Worker queue as load/resize. The Worker stamps input with its
own `performance.now()` before `BobcatRenderer` writes the shared manual clock
and calls `LynxView::dispatch_input`, which keeps gesture time on the Worker
rAF timeline and prevents an idle frame clock from making `longpress` fire
immediately. Each load clears active captures, disposal removes all listeners
and restores the canvas's prior inline `touch-action`, and unexpected capture
loss becomes `pointercancel`.

`wheel` crosses too, through the same queue and the same
`BobcatRenderer::dispatchWheel` shape as a pointer. The facade normalizes the
delta to viewport CSS px: `deltaMode` 0 is page CSS px and takes the same
canvas-box scale the position does, `deltaMode` 1 is 40 CSS px per line (the
`WHEEL_LINE_CSS_PX` `bobcat-cli`'s macOS host uses), `deltaMode` 2 is the
viewport's own width and height. The sign is the browser's, which already
means "scroll offset increases" the way core's does — the CLI negates only
because winit's is the opposite. A ctrl-held wheel is the browser's zoom
gesture and is exempt: nothing is forwarded and nothing is prevented.
Everything else is forwarded and unconditionally `preventDefault()`ed, because
the Worker answers nothing and the facade cannot learn synchronously whether
the engine consumed the scroll — the canvas owns wheel scrolling outright, the
way `touch-action: none` makes it own touch panning. A browser mouse is
reported truthfully by the facade and then fed to the engine as
`PointerKind::Pen` by `dispatchPointer`, exactly as the macOS host does, so a
primary-button drag scrolls: the engine's drag recognizer latches touch and pen
only. Hover moves and secondary mouse buttons do not cross the boundary.

**Host `NativeModules` and `globalProps` are the page's, not the Worker's.**
`BobcatCanvas.create` takes an optional
`nativeModules: Record<string, Record<string, (...args) => void>>`, checked
member by member before a Worker exists and retained for every page the canvas
loads, like its fonts; only the name/method table crosses, as
`InitMessage.nativeModules` and then two flat `string[]`s to
`BobcatRenderer::create`, from which each load builds a fresh
`Vec<Box<dyn NativeModule>>`. The handlers themselves run on the page's main
thread — that is the point, since `localStorage` and navigation are there — so
a `HostModule::invoke` posts `bobcat-native-module` (call number, module,
method, arguments as JSON array text, the function arguments' indices) and the
facade restores a single-shot wrapper in each named slot before calling the
handler. The answer returns as `bobcat-native-module-callback` and reaches
`BobcatRenderer::answerNativeModuleCallback` through the *same* ordered queue
as pointer input, so it cannot re-enter the Wasm wrapper while an async load
owns its mutable borrow. A handler that throws is `console.error`ed rather than
propagated, an unknown module or a call whose view has been replaced is
dropped, and callbacks nobody answered are released by the next load.
`load`/`loadLynxXml`/`loadTemplate`/`loadZip` each take an optional
`{ globalProps }`, JSON-stringified on the facade and written into
`ViewSources::global_props` unread. `packages/github-pages` is the one embedder
of both: its `ExplorerModule` gives the `@explorer/homepage` bundle it loads
first an `openSchema` that resolves an absolute URL, a relative path, or
`file://lynx?local://<path>[?query]` against the document base and runs the
page's own template load. Those local paths have targets because the Pages
build also publishes `@explorer/showcase`'s menus as
`showcase/menu/<name>.web.bundle` and each `@lynx-example/<category>` it
depends on as the package's whole `dist/` under `showcase/<category>/`, both
bundle flavours and the `static/` assets a demo names relative to itself; the
category list comes from the showcase's dependencies and the packages are
found through a `createRequire` rooted at its manifest, pnpm's layout keeping
them under its own `node_modules`. A `.lynx.bundle` local path is loaded as
the `.web.bundle` beside it, retrying the named file once if that rejects —
which is how the demos shipping only a source-based `.lynx.bundle` open, while
a bytecode one fails both. It also has
`localStorage`-backed preference writes, and
documented no-ops where a browser demo has no answer — no camera scanner, one
thread strategy, and no synchronous return value for
`readFromLocalStorage`/`getSettingInfo`. It passes **no `globalProps`**
deliberately: web-core's Explorer hands its view a `theme`, but the homepage
also reads `screenWidth`/`screenHeight` and flips to a two-column landscape
layout when the width exceeds the height, which is the wrong shape for this
preview canvas — with the field absent the page keeps its portrait
single-column layout and default theme.

The facade exposes no
create/append/drop/flush, document, tree, or engine API, and does not decode
`.web.bundle` containers; callers supply `PageConfig` and either executable
script URLs or a raw Lynx XML URL. Synchronous GPU capture is absent because
browser WebGPU completion is Promise-driven.

### packages/bobcat-element

The dependency-free TypeScript sources of the ESMs `bobcat-core` preloads into
its QuickJS realms, one file per module. The main-thread runtime gets
`src/main-thread-runtime.ts` as `bobcat:runtime`, `src/element-papi.ts` as
`bobcat:element`, `src/timers.ts` as `bobcat:timers`, `src/module.ts` as
`bobcat:module`, `src/event-target.ts` as `bobcat:event-target`,
`src/cross-thread-context.ts` as `bobcat:cross-thread-context`, and
`src/worker.ts` as the `Worker` class under `bobcat-internal`. The group's
*worker* runtime gets `src/worker-runtime.ts` as `bobcat:worker`,
`src/background-thread-runtime.ts` as `bobcat:bts-runtime`,
`src/global-event-emitter.ts` as `bobcat:global-event-emitter`,
`src/lynx-modules.ts` as `bobcat:lynx-modules`, `src/selector-query.ts` as
`bobcat:selector-query`, plus `bobcat:event-target`,
`bobcat:cross-thread-context`, `bobcat:timers`, `bobcat:module`,
`src/section-url.ts` as `bobcat:section-url` and `src/bundle-fetch.ts` as
`bobcat:bundle-fetch` again — the last two being on both runtimes because a
container's section URLs and `lynx.fetchBundle`'s handle are both realms' —
registered per runtime, because a source is runtime-wide and no value crosses
between two runtimes. `src/native.d.ts` declares the two native modules' contracts and is
the authoritative list of what `bobcat-internal:host` and
`bobcat-internal:worker` export.

What core embeds, with `include_str!`, is the JavaScript TypeScript 7 compiles
from `src/*.ts` during the Cargo build. `bobcat-core/build.rs` invokes the
package's build script with an output directory under Cargo's `OUT_DIR`, so
each target/profile owns its emit and parallel builds never share a source
directory. Cargo tracks the sources, build script, TypeScript configuration and
pnpm dependency files. Run `pnpm install --frozen-lockfile` before Cargo; Node
is a build dependency for native and Wasm consumers alike. Generated JS is not
committed; `pnpm --filter bobcat-element build` emits an ignored `dist/` for
local inspection, and QuickJS error lines refer to emitted JS, not TS. The
Rstest suite imports the same modules and verifies every named export.

The package owns the supported `__*` PAPI members and their web-core arities,
plus the Lynx tag vocabulary
(`wrapper`/`text`/`image`/`view`/`scroll-view`/`raw-text`/ `list`). It also
owns the value coercions web-core gets from the HTML DOM for free:
truthiness-not-null clearing for classes, ids, and inline styles,
`String(value)` for DOM attributes, and camelCase-to-kebab hyphenation of a
record-shaped inline style. Lynx attribute readback retains a separate typed
container copy, and datasets merge typed keys in the MTS handle. BTS
`lynx.createSelectorQuery()` builds `NodesRef` tasks carrying selection tokens
over the existing Worker messages; MTS resolves those through the document's
selector engine, including the query root, and returns fields/path data, while
`setNativeProps` applies CSS/attributes and commits before the next request.
`invoke` answers `boundingClientRect` — the last layout pass's border box,
plus the element's `id` and `dataset` as native reports them — and fails
every other method with code 3, `METHOD_NOT_FOUND`, alongside the selection
failures it already delivered. No callback or document handle crosses into
Rust's Worker transport. See `docs/node-query-runtime.md` for the supported
fields, callback semantics and remaining boundaries.

The package also owns the event half: a handle is an `EventTarget`, its
listeners are closures filed on the handle itself under a realm-local symbol,
and `__AddEventListener` / `__RemoveEventListener` keep the standard's
registration identity (element, name, callback, capture) with its idempotence,
`once`, and case-insensitive names. Per-handle realm state (listeners,
`__AddEvent` handlers, the index bookkeeping, list callbacks) lives on the
handle object under realm-local symbols rather than in a `WeakMap` keyed by it:
QuickJS's `WeakMap` marks its values unconditionally, so a closure that
captured its own element would otherwise keep the handle, and through it the
whole subtree, alive for the life of the realm. The per-node dispatch, the
standard's `eventPhase`, and `once` are all resolved here, with only the event
name's open/close edges crossing to the host.

**Identity and lifecycle.** An element handle is a plain object carrying its
DOM `NodeId` under a realm-local symbol (web-core's `uniqueIdSymbol` shape) —
one object per element for its whole life, so every PAPI return of an element
yields the same object and no handle is ever minted after the first. There is
no `retain`: a future query member that has to answer with a handle for a node
whose handle has died must fail loudly, and so must a dispatch whose target has
none, since a connected element always has one. `parentComponentUniqueID` and
`__CreatePage`'s arguments are accepted for PAPI shape and unused. Collection
is the only way a handle lets go of its element — web-core's model, where a
swept `WeakRef` is what ends a wrapper. Every non-page handle is registered
with a `FinalizationRegistry` whose cleanup calls the imported native
`dropElement`, which frees that element and nothing else; cleanup runs as a
pending job at the host's job checkpoints, and never at realm teardown, which
preserves the last committed tree. A collection comes from QuickJS's allocation
pressure, or from the runtime itself: every `REMOVALS_PER_COLLECTION` removals,
the batch that crosses the count ends with one, so the handles an unmount left
behind are finalized — including any caught in a cycle, which reference
counting cannot free — without waiting for allocation to reach the threshold.

**The handle is the one thing that holds its element**, and the handle above it
is what keeps it alive while its element is on screen: every handle carries an
unordered strong `Set` of its children's handles, maintained by the six tree
mutations, and the page's handle is permanent, so every *connected* element's
handle is reachable from it. The link the other way is the owner's node id,
resolved through the same weak `NodeId`→handle index the dispatch side uses, so
no parent/child pair is a reference cycle and an unreachable subtree is freed
by plain reference counting. The set holds membership only; order is the native
tree's. An unmount is therefore `__RemoveElement` on the snapshot's root, which
takes it out of its parent's set, and then the card's own references going
away: the subtree's handles become unreachable together and each finalizes into
one free. A ReactLynx list handing a recycled cell's elements between snapshot
instances and deleting the old `__elements` array takes nothing away — those
elements are connected, so their handles are held above them.

The JavaScript layer deliberately does not validate handles: a foreign handle
resolves to `undefined`, which the private native boundary rejects as a
JavaScript error before entering `dom`. Native access is limited to named
imports from the native `bobcat-internal:host` ESM; the realm has no
`globalThis.bobcat`, no `console`, and no DOM. Named exports are the only
Element-PAPI surface for transformed MTS entries; a local named Lepus chunk
receives the same names as the parameters of the body it is compiled as.
Rstest imports
the TypeScript directly, and TypeScript 7 checks the sources as a program with
`lib: es2023` and no ambient types, resolving each `bobcat:*` specifier to its
file through `paths` and declaring the two native modules' contracts in a
`.d.ts`.

`src/timers.ts` is the one module here that does install globals, because bare
`setTimeout`/`setInterval`/`clearTimeout`/`clearInterval` are how a card
reaches them. It keeps only the callbacks, filed under the id the host's
`setTimer` hands back; the schedule and HTML's `long` delay conversion and
nesting clamp are `bobcat-main`'s, and the first step of the epilogue that
follows every entry into a realm calls the module's `__BobcatRunTimer` back for
whatever is due — before that entry's commit, so a callback's mutation rides
the same frame. No deadline crosses the link and no host turn is owed for one.

`src/element-papi.ts` also exports `class Document`, whose constructor calls
the native `createDocument`. It is on no collection schedule at all, which is
the opposite of the element path in the same file.

### Other pnpm packages

- `packages/reactlynx-test-fixtures` — the JSX/CSS/JS sources of the compiled
  ReactLynx cards the integration tests, decoder tests and benchmarks run.
  `rsbuild.config.js` declares the entries and their independent output
  directories, and the package scripts invoke the public `rsbuild build` CLI
  once in production and once in development mode; no script creates a compiler
  or imports an internal build-tool entry point. Run
  `pnpm --filter reactlynx-test-fixtures build` before Rust tests, clippy or
  benches: the emitted `dist/index.rs` registry is what names the bundles, and
  no compiled fixture is versioned. Upstream provenance for the five `basic-*`
  cards is in that package's `NOTICE.lynx-stack`. Native builds use the shared
  `scripts/lynx-bytecode.ts` hook to disable BTS manifest bytecode. MTS keeps
  the compiler's default encoding. The fixture-only source repack remains
  necessary for Bobcat's source evaluator.
- `packages/explorer-homepage`, `packages/explorer-showcase` and
  `packages/explorer-lib` — the Lynx Explorer home screen and showcase menu in
  ReactLynx, over the navigation, launch-command, history and theme helpers the
  `lib` package shares between them.
- `packages/github-pages` — the rsbuild site published to GitHub Pages, built
  by `pnpm build:github-pages`, which builds `bobcat-wasm` and the Explorer
  homepage first and then this package over both.
- `examples/` — `lynx-stack`'s own examples, adapted so their `workspace:*`
  dependencies name published package versions and they install independently
  of the `lynx-stack` source tree. See `examples/README.md` for the TypeScript
  arrangement each one needs.

### crates/dom

Generic W3C-DOM-subset document tree and standards-oriented CSS computation
core, on stylo's cascade. `docs/dom-public-api.md` is the authoritative
normal-build versus test-feature API boundary. It must not contain Lynx
runtime-element vocabulary or Lynx device/unit policy: Lynx computed defaults
(border-box, `overflow: clip`, `display: linear` on every element, …) stay
embedder cascade policy in the UA sheet.

Subsystems:

- `tree/` — the boxed `TreeArenas<T>`, `Node`, `Document`, shadow roots, the
  flat tree, custom-element definitions and reactions.
- `style/` — the per-document `StyleEngine` (`Stylist`, cascade pipeline,
  device, stylesheet set, `SharedRwLock`), flush, invalidation, `StyleDamage`.
- `layout/` — the concrete `hughie` host: `Document::layout`, the `LayoutTree`
  impl, per-node `LayoutSlot`s in `DocumentLayoutState`, and the two
  per-element facts a `StyleView` answers with that no computed value carries:
  `content-visibility: auto` relevance (`relevance.rs`) and the css-sizing-4
  last remembered size (`remembered.rs`).
- `visual/` — stacking contexts, CSS2 Appendix E paint order, transforms,
  `RenderLayer` group effects, reverse-paint-order hit testing, and
  `content-visibility: auto` relevance determination (`relevance.rs`), which
  runs inside `render` between the paint-order build and the walk.
- `paint/` — the document-owned private `Painter`, walker, fragment painters
  and the retained `vello::Scene` a `commit` publishes as `CommittedFrame`.
- `scroll/` — CSSOM-View geometry, per-node offsets, `scroll_to`/`scroll_by`/
  `scroll_chain`, and the one chain walk (`drive_chain`) both the document
  and `bobcat-core`'s painter run: `overscroll-behavior` fences the reach,
  the engine's own `scroll-capture: nearest` hands a gesture to the container
  above first. `scroll/snap.rs` is css-scroll-snap-1 — positions from
  `scroll-snap-type`/`-align`/`-stop`, `scroll-padding` and `scroll-margin`,
  published per scroll slot, settled on at a drag's end, stepped to by a
  wheel tick, and re-snapped at rest on every commit; no snap events.
  `scroll/initial_target.rs` is css-scroll-snap-2's `scroll-initial-target`:
  the build records the `nearest` elements, the render scrolls each
  container to its first one and rebuilds the frame in the same commit.
- `input/` and `event/` — the `InputEvent` host seam, `Document::event_steps`,
  which computes a path for a *script* dispatch above, and
  `Document::dispatch_element_event`, which walks that path here for an event
  the engine decides, delivering it to defined `CustomElement`s through
  `handle_event` and to nothing else.
- `render/` — the DOM-free floor absorbed from the former `pulsar` crate
  (2026-08-04): `FrameImages` and the `render::gpu` wgpu backend. `render::blur`
  is the one exception: a `filter: blur()` bake is a partial replay of a
  committed frame's compose program, so it reads `CommittedFrame`'s filter side
  table, while still naming no node, style, layout or paint-order type.

Rulings and limits to know before touching it:

- The whole `unsafe` surface is two blocks, each with a `SAFETY` comment kept
  honest by `#![warn(clippy::undocumented_unsafe_blocks)]`.
- A `NodeId` is never reissued (no generation counters, no epoch gate on the
  retained frame) and the document element is permanent and pre-created.
- `overflow: auto` stays out (user decision, 2026-07-29) and a `visible` axis
  pairs into `hidden`; only `scroll` is user-scrollable, `hidden` is a scroll
  container only script moves, `clip` is no container at all, and scroll
  containers are forced stacking contexts.
- `content-visibility: auto` relevance is `dom`'s, determined once per commit
  against the region the paint walk's culling admits, and stored as
  layout-side per-element state in a slot-keyed side table on `TreeArenas` —
  never a Stylo `ElementState`, never a restyle trigger. `hughie` sees it only
  as `CoreStyle::skips_contents`. A relevance flip reveals inside the same
  commit (`crates/dom/src/visual/relevance.rs`, at most four build passes
  under one commit id), so no frame is published with a reveal pending.
- Animations and transitions inside **skipped contents** are frozen
  (css-contain-2 §4): the driver carries their start times by each tick's
  interval rather than stepping them, the skipping element's own animations are
  untouched, and a frozen set counts as idle for `has_active_animations`, the
  frame's animation flags and so `Painter::owes_frame`. The one deviation —
  style is not skipped, so an animation that *starts* while skipped is created
  and frozen at its start rather than not created — is in
  `docs/style-assumptions.md` §19.
- The css-sizing-4 **last remembered size** is `dom`'s in the same way, in a
  second slot-keyed side table on `TreeArenas`
  (`crates/dom/src/layout/remembered.rs`): the layout host records a box's
  content box after every committing run in which it had no size containment,
  and `StyleView` substitutes it into `CoreStyle::contain_intrinsic_{width,
  height}` once the box is skipping. This engine has no ResizeObserver, so
  that commit's layout run is the spec's recording moment, and csswg-drafts
  #8407 (`content-visibility: auto` implies the `auto` keyword) is folded in
  here because the fork's own adjuster is `#[cfg(feature = "gecko")]`.
  `hughie` is unchanged by either: it reads `AutoLength(l)` as `l` already.
- `position: sticky` stays in normal flow and resolves its inset/containing-block
  constraints in the retained frame at live scroll offsets; painting, hit
  testing and bounding rectangles share the same sticky geometry.
- The crate dispatches no events and has no `preventDefault` and no gesture
  recognizer; `InputEvent::default_prevented` is the embedder's seam.
- Custom elements are user-agent components only, `define` must precede any
  element with its tag, reactions are queued rather than called inline, and
  `disconnected_callback` takes a shared `&Document`.
- Attribute-derived style enters through `Document::set_presentational_hint` at
  `CascadeOrigin::PresHints`, never the author's inline block.
- Stylo's per-element style data and its traversal/invalidation flags live
  inline on `Node` (bench-defended 2026-08-03: no traversal regression, a
  measurably faster no-op-commit fast path).
- One `StylePool` per document through `Document::set_style_pool`;
  `MAX_STYLE_THREADS` is six, a ceiling rather than a tuning knob.
- Confirm the vendored fork tip with `git -C vendor/stylo rev-parse --short
  HEAD` rather than trusting a written one.

Everything else about the crate's internals is in `docs/dom-architecture.md`.

### crates/hughie

The Flexbox, Grid, grid-lanes, and Starlight Relative and Linear engine:
trait-based host⇄engine integration with static dispatch only (no `dyn`),
one `LayoutTree`
protocol with a `Copy + Debug` `NodeId`, immutable topology/styles for the
flush, and a separately borrowed mutable host state of per-node `LayoutSlot`s.
That split permits recursive mutation without copying style/layout records and
without `RefCell`/`AtomicRefCell` checks. Style traits speak the stylo fork's
computed-value vocabulary directly (requiring the `stylo` workspace dep +
python3 for its build script; the old zero-dependency/standalone pillar is
retired), with host-side display dispatch. They are split by algorithm:
`CoreStyle` carries the box model, containment, the alignment accessors and
`order`, while `FlexboxStyle`, `GridStyle`, `LinearStyle` and `RelativeStyle`
each carry what only their own algorithm reads and are demanded at that
algorithm's entry point; `GridLanesStyle: GridStyle` is the one that extends
another algorithm's trait rather than `CoreStyle`, adding `flow_tolerance` and
the computed `font_size` that `flow-tolerance: normal`'s `1em` resolves
against. `TextContainerStyle` supplies paragraph-wide
`text_maxline` and `text_maxlength` inputs from non-inherited integer custom
properties, defaulting to unlimited. `LayoutTree::flattened_children` is the
box-tree view every algorithm collects items through, flattening `display:
contents` subtrees. Leaf content is deliberately closed: replaced content uses
the `NaturalSize` value path, text the crate's concrete
`TextBlock::probe`/`commit` paragraph path; arbitrary host measurers are not
supported.

**Flexbox, Grid, grid lanes, Relative, and Linear implemented** — the shared
root/leaf/cache/positioned/rounding machinery, CSS Flexbox Level 1, numeric CSS
Grid Level 2 (excluding subgrid/named areas), CSS Grid Level 3
`display: grid-lanes` on that Grid machinery, id-constrained Starlight Relative
Layout Level 1, and Lynx's `display: linear` algorithm and `linear-*`
style/source protocol are live. Grid lanes is a **user-directed W3C extension
beyond Lynx parity** (2026-09-18) rather than a compat obligation — native
Lynx has no such `display` value and no `flow-tolerance` property — and it
excludes `inline-grid-lanes`, the orientation property and
`grid-auto-flow: normal`,
`dense` backfilling, intrinsic `repeat(auto-fill, auto)`, virtual-item
grouping, stacking-axis self-alignment, baseline alignment/sharing, subgrid and
fragmentation; do not add any of those without a user decision
(`docs/style-assumptions.md` §24). Text shaping, line breaking,
intrinsic/height-for-width measurement, baselines, and retained Parley layouts
are unconditional crate behavior, in `src/text/block` — including
`truncate.rs`, which lays an `<inline-truncation>` subtree in at the clamp and
applies `tail-color-convert`'s native semantics.

**CSS containment (css-contain-2)** is landed layout-side: the stylo
`Contain`/`ContainIntrinsicSize` accessors on `CoreStyle`, size-substitution +
layout-containment baseline suppression, the skipped-contents pair
`compute_skipped_contents_size` + `hide_skipped_contents` (whose "is this box
skipping?" input is `CoreStyle::skips_contents`, defaulted to
`content-visibility: hidden` and overridden by `dom` to fold in `auto`
relevance; the size is cacheable and goes through `compute_cached_layout`, the
hide sweep answers to the box tree and runs on every committing call outside
that cache), and
the `invalidate` module (`is_relayout_boundary`, `invalidate_for_relayout`) —
the containment-bounded, damage-driven cache-invalidation host workflow
(single-axis / container queries out of scope). `LayoutGoal::Commit` carries
per-axis `content_independent` flags: input *stability* under subtree content
change, proven by the committing algorithm (flexbox, grid, grid lanes, linear,
relative and
the absolute pass all set them; the root input is viewport-stable by
construction; a measurement carries no such claim, which is why only a commit
has the field). They ride inside the committed cache entry, outside its key, so
`LayoutSlot::committed_input` hands a host the complete input it can relayout a
subtree in place under and verify by output comparison. The per-node
measurement cache has a 32-entry ceiling but inlines two slots, spilling to the
heap for nodes whose containers probe many constraint shapes. Read
`docs/layout-architecture.md` before touching it. It must not depend on other
workspace crates or own host tree/style storage, DOM/runtime types, resolved
device-unit policy, or paint order.

The runtime-layout integration — the `LayoutTree` host, display dispatch,
fixed/hoisted positioned pass, per-node cache storage, and the automatic
style-damage→layout-invalidation wiring (boundary-stopped and engine-internal,
not a runtime-adapter concern) — lives in `dom`; generic W3C text style,
document context, and artifact storage live there too.

### crates/flashbulb

Screenshot testing infrastructure, and the only crate here that exists for the
test suite rather than the product (`publish = false`, dev-dependency
everywhere). It owns RGBA `Image` + PNG codec, a port of the `pixelmatch`
algorithm Playwright compares screenshots with (squared-YIQ per-pixel distance
against `35215 * threshold²`, anti-aliasing detection,
`max_diff_pixels`/`max_diff_pixel_ratio` budgets), and `Screenshots`, the
golden store: path resolution from a name-segment list,
`FLASHBULB_UPDATE_SNAPSHOTS=1` to accept, and `-expected`/`-actual`/`-diff`
PNGs written to a git-ignored `tests/artifacts/` on failure. A newly *created*
golden fails its own run so an unreviewed baseline cannot pass; an explicitly
*accepted* one does not.

The optional `render` feature adds `capture_document` (`Document::render` →
retained scene → `dom`'s headless GPU) over the whole painted frame, `viewport
* device_pixel_ratio` device pixels: the render floor scales the scene up by
that ratio, so anything smaller is a crop. Playwright instead downsamples to
CSS pixels; the two coincide at a ratio of 1, which lynx-stack pins for
determinism and every viewport here uses. Its `TestImages` is the in-memory
`dom::FrameImages` the image suites hand to a capture — the only image store in
this workspace, and deliberately a test double: it fetches, decodes and evicts
nothing. `pump_images` drives one round of the document-to-host image protocol
(every source `take_wanted_images` named, then the reports back through
`apply_image_events`) and `render_with_images` loops that to quiescence.
`headless` requires a usable GPU adapter and panics without one, so local and
CI runs obey the same mandatory-GPU policy. DOM-aware screenshot suites live in
`dom`, which also keeps the direct GPU smoke tests. Goldens are not
platform-suffixed: cross-platform rasterizer noise is absorbed by tolerance,
not by per-platform baselines.

### Still ahead

What the runtime layer does not implement yet, each recorded with the code that
would host it:

- **Per-component css-id scoping.** `__SetCSSId` is a sink and every
  `StyleInfo` fragment mounts globally; the encoding lands with the ingestion
  side that reads it, together with the parent-component css-id inheritance
  that feeds it.
- **The list surface.** `crates/bobcat-core/src/main/tree/list.rs` carries
  what a UA sheet can say about `list` and `list-item`: the scroll axis, the
  `list-type` layout modes (`flow` = grid, `waterfall` = `grid-lanes`), cells
  virtualized by `content-visibility: auto` with a `100cqh` estimate fallback,
  and the `span-count`/`column-count`/`sticky-offset`/
  `estimated-main-axis-size-px` hints. Underneath it, the data protocol —
  `__SetAttribute(element, "update-list-info", …)` — delivers a compiled
  `<list>`'s cells as real element children, which is the only path one
  receives children on. `<list>` is not a `dom::CustomElement`: there is no
  cell recycling, no scroll-to-index, no threshold or scroll events, no list
  UI method, and no sticky or snap rules (`docs/tracking/deviations.md`).
- **Gesture detectors and the arena.** `crates/bobcat-core/src/paint/gesture.rs`
  has no fling or velocity, no `:active` driving, no `consume-slide-event`, no
  per-element `GestureDetector`/arena relations and no `click`; `tapSlop` is
  the default 50 px rather than the page config's.
- **The rest of the `<image>` element surface.** `src` loads; `mode`,
  `auto-size`, `placeholder` racing, `cap-insets`, `blur-radius` and the
  `load`/`error` events do not.
- **UI methods other than `boundingClientRect`.** That one dispatches by name
  through `__InvokeUIMethod`; every other name — `scrollIntoView`,
  `getScrollInfo`, `requestUIInfo`, `takeScreenshot` and the per-component
  catalog — answers code 3, `METHOD_NOT_FOUND`. The rect itself ignores
  transforms and never flushes.
- **The text `layout` event.** The per-line ranges `hughie`'s
  `text/block/content.rs` computes have no delivery path.
- **`rpx`-aware view/device policy** and the `<list>` *component* (its UI methods,
  scroll and threshold events, sticky cells and snapping; the layout mapping
  and virtualization are UA rules already).
- **Animated image playback.** `bobcat-resources` decodes an image's first
  frame only, with no `region-to-decode` and no `blur-radius`
  post-processing.
- **Import maps, import attributes, JSON *ESM* modules and Lynx component-bundle
  imports**, and the asynchronous half of the compiled-bundle loader:
  `requireModuleAsync` and `loadScriptAsync`. Lazy containers themselves are
  implemented — `lynx.fetchBundle` plus `lynx.loadScript` on both threads —
  and so is `require`'s own `.json` parse.

See `docs/tracking/` for the behavior surface each of these is scoped against,
and `.claude/agents/` for the subsystem-scoped agent personas set up for this
work.

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
- `docs/dom-architecture.md` — `crates/dom`'s internals: arenas and identity,
  shadow DOM and custom elements, the style engine and invalidation, the
  layout host, visual order and the committed frame, scroll/input/event
  paths, text, and the vendored stylo fork. The `crates/dom` section here
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
  performance work.** Why vello has no glyph atlas and cannot get one, what a
  text-heavy frame actually costs here (measured), where the ecosystem's answer
  lives (`glifo` via `vello_hybrid`), and why `glyphon` and a hand-rolled atlas
  are both ruled out. Conclusion is *don't switch renderers yet*, so the useful
  contribution is evidence, not a port.

## Toolchain

- Nightly Rust (`rust-toolchain.toml`), edition 2024, resolver 3, workspace
  lints.
- The pnpm workspace (`packages/*`, `examples/*`, `crates/bobcat-wasm`) is
  TypeScript and ESM throughout, checked by TypeScript 7.0.2 (`pnpm test:type`)
  under the strict options in `tsconfig.base.json`; Node (`^22.18 || ^24`) runs
  its `.ts` scripts directly by type stripping.
- Cargo builds of `bobcat-core` require Node and a prior
  `pnpm install --frozen-lockfile`; the built-in JS runtime is compiled into
  `OUT_DIR` during the build, including for Wasm targets.
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

## Testing

Integration tests and benchmarks build ReactLynx sources from the
`packages/reactlynx-test-fixtures` pnpm workspace. Run
`pnpm --filter reactlynx-test-fixtures build` before compiling Rust tests,
`cargo clippy --all-targets`, or benchmarks. The generated `dist/index.rs`
registry includes the emitted bundles; no compiled fixture is versioned.
Upstream source provenance and licensing are in that package's NOTICE/LICENSE.
`cargo test` must pass on the pinned nightly toolchain.

### Input robustness at the external-byte boundaries

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

### The unsafe floor

`hughie`, `flashbulb`, and `bobcat-source` carry `#![forbid(unsafe_code)]`. The
workspace-wide `unsafe_code = "warn"` is a lint any module can silence locally;
`forbid` cannot be overridden from inside the crate, so `unsafe` appearing in
one of these three has to be a deliberate edit to that line.

Where `unsafe` is unavoidable, the bar is a `SAFETY` comment per block,
enforced by a crate-local `#![warn(clippy::undocumented_unsafe_blocks)]` in
`bobcat-cli` (which now holds no `unsafe` at all, the lint standing as a bar
for any that arrives) and in `dom` (its two). The lint is still crate-local
rather than workspace-wide because `quickjs-rust-bridge` (133 blocks) is the
last holdout and is being restructured separately; raising it there is what
would let this move into `[workspace.lints.clippy]`.

One trap, worth knowing before writing the comment: the lint does **not** scan
past an intervening attribute. Where an unsafe site carries
`#[expect(unsafe_code, reason = …)]`, as every one in `dom` does, the
`// SAFETY:` comment has to sit *between* that attribute and the block —
placing it above the attribute still trips the lint, and `cargo fmt` will not
move it either way.

### Benchmarks measure a debug-instrumented dom

Cargo unifies the features of a package's dev-dependencies into that package's
own library whenever dev targets are in the build. `hughie` dev-depends on
`dom` with `layout-test-utils` for its bench harness and `dom` depends on
`hughie`, so the cycle turns the feature on for both libraries in any build
with bench targets:

```sh
cargo build --unit-graph -Z unstable-options --workspace            # dom: []
cargo build --unit-graph -Z unstable-options --workspace --benches  # dom: [layout-test-utils]
```

The second line is what `cargo codspeed build` and `cargo llvm-cov` resolve.
The cost is one `test_leaf_metrics()` probe per leaf in
`crates/dom/src/layout/host.rs` that a release build does not contain, so the
CodSpeed numbers — the authority for this repo, since single-run local walltime
is noise — describe a `dom` one branch away from the shipped one. On the
`hughie` side the feature only adds the
`compute_leaf_layout_with_measurement_for_testing` wrapper and costs nothing.

**This is accepted, not fixed.** Breaking the cycle means moving hughie's
dom-based benches into `dom`, renumbering every CodSpeed benchmark id and
throwing away its history — a worse trade than one predictable branch.
`.github/scripts/check-bench-feature-parity.py` runs in CI and holds the line:
it diffs the two resolutions, prints the two recorded deviations, and fails on
a third appearing or on a recorded one silently going away. Any new entry needs
a written reason for the same cost the existing ones state.

### Benchmarks are only the `benches/` targets

Every library, and bobcat-cli's two binaries, sets `bench = false`. Cargo's
`--benches` selection, which `cargo codspeed build` runs underneath, otherwise
also compiles each of them as a bench harness of its own: a second, test-mode
build of the crate that holds no benchmark, since every benchmark is a
`harness = false` target under `benches/`. In CI's bench-profile build (thin
LTO, one codegen unit per crate) those harness builds were about a third of
the compile time. `cargo test` is unaffected: `test` stays on, and the
`[[bench]]` targets are what `cargo bench` and CodSpeed still build and run.

### Restricted-environment troubleshooting

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
    -p bobcat-wasm -p bobcat-resources -p bobcat-core -p dom -p hughie \
    -p bobcat-source -p quickjs-rust-bridge -- -D warnings
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

**Screenshot tests** live in `crates/*/tests/screenshots.rs` — plus per-topic
siblings (`dom` also has `text_screenshots.rs`, `web_text_screenshots.rs`,
`blur_screenshots.rs` and `css_atlas.rs`) — with
committed goldens in `crates/*/tests/screenshots/`, driven by
`crates/flashbulb`. The ordinary suites share one capture harness in
`tests/support/screenshot.rs`; the browser-referenced CSS atlas owns the
separate workflow below. The golden store is per *crate*, so every screenshot
binary in a crate writes into the same tree. They require a GPU adapter;
without one the test run fails, including in CI, so a green run always means
the pixels were rendered and compared. To accept a new rendering in the
ordinary suites, look at the image first, then (dropping `--test` to catch
every ordinary screenshot binary in the crate):

```sh
FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p <crate>
```

A golden that does not exist yet is written *and fails its run* — review it and
re-run. Failures write `-expected`/`-actual`/`-diff` PNGs to the git-ignored
`crates/<crate>/tests/artifacts/`; the panic message names all three plus the
exact differing-pixel count. Never accept a golden you have not looked at: a
blank or all-white image compares happily against itself forever. Browser-owned
suites can reject `FLASHBULB_UPDATE_SNAPSHOTS`; follow their checked capture
and audit workflow instead. The CSS paint atlas has two explicit reference
owners: 666 Chromium matches remain browser-owned, while 145 W3C-correct
differences (84 rasterization/sampling cases plus 61 standards-permitted UA
choices) use native DOM/Parley snapshots in a separate directory. Native atlas
references may be updated only with the filtered
`CSS_PAINT_UPDATE_NATIVE=1 ... css_native_` workflow, which cannot overwrite
browser references; the other 189 cases remain ignored. The browser stage uses
`isolation: isolate` to match the native document element's stacking-context
role, so all 22 negative-z probes are Chromium-owned exact matches. The CSS
paint matrix records the exact capture, update, and full-browser-audit workflow
in `docs/css-paint-screenshot-matrix.md`.
