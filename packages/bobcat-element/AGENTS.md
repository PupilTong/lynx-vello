# packages/bobcat-element

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`packages/bobcat-element`. Paths in backticks are relative to the repository root.

The dependency-free TypeScript sources of the ESMs `bobcat-core` preloads into
its QuickJS realms, one file per module. Both of a group's runtimes register
all twenty-four, as `esm.rs`'s `BUILTIN_MODULES` lists them in the order of
`src/tsconfig.json`'s `paths` (a unit test holds the two equal):
`src/lynx-modules.ts` as `bobcat:lynx-modules`, `src/global-event-emitter.ts`
as `bobcat:global-event-emitter`, `src/selector-query.ts` as
`bobcat:selector-query`, `src/element-papi.ts` as `bobcat:element`,
`src/main-thread-runtime.ts` as `bobcat:runtime`, `src/frame.ts` as
`bobcat:frame` (what a `frame`'s `src` does in the realm that wrote it: the
fetch, the `ShadowRealm`, the call into the child), `src/frame-boot.ts` as
`bobcat:frame-boot` (a nested card's boot inside that child, its BTS Worker
included), `src/timers.ts` as
`bobcat:timers`, `src/future.ts` as `bobcat:future`, `src/module.ts` as
`bobcat:module`, `src/section-url.ts` as `bobcat:section-url`,
`src/bundle-fetch.ts` as `bobcat:bundle-fetch`, `src/diagnostics.ts` as
`bobcat:diagnostics` (every realm's `console` and `reportError`: the one value
formatting and the `lynx.reportError` level rule, over the core's diagnostics
pair; `bobcat:worker` also installs its `console` on a worker's global, so a
plain `Worker` has a global `console` and still no global
`requestAnimationFrame`),
`src/event-target.ts` as
`bobcat:event-target` (it also exports `reportException`, which hands an
exception to the reporter the realm's runtime installed),
`src/animation-frame.ts` as `bobcat:animation-frame` (every realm's
`requestAnimationFrame`/`cancelAnimationFrame`, the demand it sends the host,
and the `__BobcatBeginFrame` a vsync calls in any realm that asked for one),
`src/system-info.ts` as `bobcat:system-info` (`createSystemInfo`, where the
three runtime constants of `SystemInfo` are written), `src/record.ts` as
`bobcat:record` (`splitRecord`, the one reader of the `<utf16Length>:<text>`
records the host answers with: the Element PAPI's attribute and style
answers, and the native module table the BTS is posted in `initialize`),
`src/native-modules.ts` as `bobcat:native-modules` (the native module
transport every realm kind links: `callNativeModule` over
`invokeNativeModule`, and the `__BobcatNativeModuleCallback` both engine
threads call with an answer), `src/cross-thread-context.ts` as
`bobcat:cross-thread-context`, `src/performance.ts` as `bobcat:performance`
(`lynx.performance`'s JavaScript half for both realm kinds: the four pipeline
hooks ReactLynx calls, unpacked into primitive host arguments, and the BTS's
`createObserver` over the `lynx.performance.onPerformanceEvent` global event,
whose entries the worker's `PerformanceReady` takes from the host as one
record string and folds into objects; see
`docs/performance-observation-design.md`), `src/worker.ts` as the `Worker`
class under `bobcat-internal`, `src/worker-runtime.ts` as `bobcat:worker`,
`src/background-thread-runtime.ts` as `bobcat:bts-runtime` and `src/bts.ts` as
`bobcat:bts` (the BTS bootstrap and a BTS realm's root module: it imports
`bobcat:worker` and `bobcat:timers` itself and hands `bobcat:bts-runtime` the
loader of the view's BTS entry, which imports the URL the `initialize`
message names). They are
registered per runtime, because a source is runtime-wide and no value crosses
between two runtimes; which of them a realm can link is decided by the host
modules it declares, not by the table. `src/native.d.ts` declares the three
host modules' contracts and is the authoritative list of what
`bobcat-internal:host`, `bobcat-internal:worker` and
`bobcat-internal:native-modules` export.

What core embeds, with `include_bytes!`, is QuickJS bytecode: TypeScript 7
compiles `src/*.ts` to JavaScript during the Cargo build, at `target: es2025`
— the language level the pinned QuickJS implements, so nothing is lowered.
Nothing later is written: the engine has no explicit resource management
(`using`, `Symbol.dispose`), decorators or `accessor`.
`bobcat-core/build.rs` then compiles each emitted module, under the specifier
`src/tsconfig.json`'s `paths` maps to it, to bytecode with the same
`quickjs-rust-bridge` the engine links, built for the build machine as a
build dependency (`Context::compile_module_bytecode`, source text stripped,
line information kept). A runtime registers the bytecode
(`Runtime::register_module_bytecode`, borrowed, never copied) and a realm that
imports a built-in reads it back (`JS_ReadObject`) instead of parsing and
compiling the text — about 25× cheaper, which is most of what made a realm's
boot cheap. QuickJS bytecode names atoms by text and has no pointer-sized or
endian-specific layout, so bytecode written on the build machine reads on
every target, Wasm included. `build.rs` invokes the package's build script
with an output directory under Cargo's `OUT_DIR`, so each target/profile owns
its emit and parallel builds never share a source directory. Cargo tracks the
sources, build script, TypeScript configuration and pnpm dependency files. Run
`pnpm install --frozen-lockfile` before Cargo; Node is a build dependency for
native and Wasm consumers alike. Generated JS and bytecode are not committed;
`pnpm --filter bobcat-element build` emits an ignored `dist/` for local
inspection, and QuickJS error lines refer to emitted JS, not TS. The Rstest
suite imports the same modules and verifies every named export.

The package owns the supported `__*` PAPI members and their web-core arities,
plus the Lynx tag vocabulary
(`wrapper`/`text`/`image`/`view`/`scroll-view`/`raw-text`/ `list`). It also
owns the value coercions web-core gets from the HTML DOM for free:
truthiness-not-null clearing for classes, ids, and inline styles,
`String(value)` for DOM attributes, and camelCase-to-kebab hyphenation of a
record-shaped inline style. Lynx attribute readback retains a separate typed
container copy, and datasets merge typed keys in the MTS handle, each entry
also written to the node as its `data-<key>` attribute (web-core's
`__AddDataset` text: no attribute for a falsy value, JSON for an object), so
`[data-<key>]` selectors match it in queries and stylesheets while readback
answers the typed value. BTS
`lynx.createSelectorQuery()` builds `NodesRef` tasks carrying selection tokens
over the existing Worker messages; MTS resolves those through the document's
selector engine, including the query root, and returns fields/path data, while
`setNativeProps` applies CSS/attributes and commits before the next request.
`invoke` answers `boundingClientRect` — the last layout pass's border box,
plus the element's `id` and `dataset` as native reports them —
`scrollIntoView` and CSSOM-View's `scrollTo`/`scrollBy` on every element, a pager's
`selectTab`, a swiper's `scrollTo`, a `scroll-view`'s `scrollTo`, `scrollBy`
and `getScrollInfo`, a `list`'s `scrollToPosition`, `scrollBy`,
`getScrollContainerInfo` and `getVisibleCells`, an `x-refresh-view`'s
`finishRefresh`, `finishLoadMore` and `autoStartRefresh`, a scroll
coordinator's `setFoldExpanded`, a `text`'s `getTextBoundingRect`, and a dialog's four methods, and fails every other method with
code 3, `METHOD_NOT_FOUND`,
alongside the selection failures it already delivered. No callback or document handle crosses into
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
changes of what a realm has registered for a name on an element crossing
to the host.

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
JavaScript error before entering `terminus`. Native access is limited to named
imports from the native `bobcat-internal:host` ESM; the realm has no
`globalThis.bobcat`, no global `console`, and no DOM. Named exports are the only
Element-PAPI surface for transformed MTS entries; a local named Lepus chunk
receives the same names as the parameters of the body it is compiled as.
Rstest imports
the TypeScript directly, and TypeScript 7 checks the sources as a program with
`lib: es2025` and no ambient types, resolving each `bobcat:*` specifier to its
file through `paths` and declaring the three host modules' contracts in a
`.d.ts`.

`src/timers.ts` is the one module here that does install globals, because bare
`setTimeout`/`setInterval`/`clearTimeout`/`clearInterval` are how a card
reaches them. It keeps only the callbacks, filed under the id the host's
`setTimer` hands back; the schedule and HTML's `long` delay conversion and
nesting clamp are `bobcat-main`'s, and the first step of the epilogue that
follows every entry into a realm calls the module's `__BobcatRunTimer` back for
whatever is due — before that entry's commit, so a callback's mutation rides
the same frame. No deadline crosses the link and no host turn is owed for one.

`src/element-papi.ts` also exports `class Document` and the `PageConfig` its
constructor takes: the five page switches the boot module is written with,
which the constructor passes to the native `createDocument` as five booleans,
in `PageConfig`'s order. It is on no collection schedule at all, which is the opposite
of the element path in the same file.
