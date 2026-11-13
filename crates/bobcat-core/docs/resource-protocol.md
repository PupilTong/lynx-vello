# bobcat-core: Resource protocol, stylesheets and ESM loading

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

`ResourceFetcher::request_source` owns fetching, UTF-8 validation and the
resolution of every URL a script request does not already carry resolved.
Its concrete, non-cloneable `SourceCompletion` holds one end of the
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
rules, keyframes and font-face rules are built through `terminus`'s branded
`CssRule` builders, leaving stylo one selector-list parse per rule and one
value parse per declaration — the floor, because the wire format keeps
attribute selectors and functional pseudo-classes as text and stylo builds
specified values only through its value parsers. Decoding a container stays
embedder work: core owns the `PreparsedStyleSheet` vocabulary, the embedder
fills it. Source requests select a stylesheet or module payload. A
`SourceRequest::Module` — an entry, an import, a worker script or a
synchronous load — carries an absolute URL Rust already resolved; a font
carries the absolute URL the document resolved; a stylesheet or fetch
carries the URL as it was named, and the fetcher resolves it against its own
base, which the embedder keeps equal to `ViewSources::base_url`. The fetcher supplies the transport policy. The protocol
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
module must not decode images, fonts or templates, or upload render resources,
and owns no cache or retry policy of its own: that policy is `vant`'s
(`crates/vant`), the crate below `terminus` every fetcher applies, and core only
re-exports its error vocabulary (`ResourceError` and friends).

**`NativeModules` is the other host-injected, thread-bound capability.**
`LynxGroup::create_lynx_view` takes a `Vec<Box<dyn NativeModule>>` beside the
fetcher builder, and for the same reason: a module runs on the embedder's own
thread, inside `LynxView::pump`, so it needs neither `Send` nor `Sync` and a
Wasm module may hold `JsValue`s. One `dyn` handler per `NativeModules` key,
`terminus::link::CustomElement`'s shape: `name()` and `methods()` are read once at
construction — two modules of one name is `EngineError::DuplicateNativeModule`,
returned as `LynxViewError::Engine` — and the table crosses to the realm as
data, so nothing asks a module a question while script is running. The call
shape is native Lynx's, not a promise's: every function argument becomes a
single-shot `ModuleCallback` the module invokes later with JSON array text, the
method itself answers `undefined`, and there is no synchronous return value and
no error channel. `ModuleCallback::invoke` consumes the handle, dropping one
releases the JavaScript function uninvoked, and the answer rides back to the
calling realm weakly, through a handle the view already holds
(`native_module::ModuleReply`): a worker's inbox, which the view registered
from `ViewNotice::WorkerCreated` for frame demand, or, for a call the MTS realm
made, the view's own command FIFO, as `ToMain::ModuleCallback`. Each
`ViewNotice::NativeModuleCall` names its caller the way `ScriptFrameDemand`
does (`None` for the MTS realm), and `FrameDemand::reply` picks the handle, so
nothing is carried across a second time. A callback therefore holds no realm
open however long a module keeps it, and `is_cancelled()` is that handle's own
liveness: the receiving end drops with the task that serves the realm, so
there is nothing left to answer exactly when there is nothing left to answer
*through*. An answer is delivered to a worker realm the moment it arrives,
never queued behind BTS boot: an entry awaiting its own call's answer would
otherwise deadlock. An MTS realm's answer is a command, applied by the next
job of the view like any other, and the one command a frame post's fence does
not count: it does not go through the seat's `CommandSender`, and the page
leaves it out of its count of applied commands to match.
The transport is the built-in `bobcat:native-modules` (`callNativeModule` and
`__BobcatNativeModuleCallback`, which both engine threads call), written over
the host module `bobcat-internal:native-modules` (`invokeNativeModule` alone)
that `native_module::install` gives every realm kind. The table is data: the
MTS realm reads the view's modules once, as the startup member
`nativeModuleTable` of `bobcat-internal:host`, and posts the record unread to
its BTS in the `initialize` message, where `bobcat:bts-runtime` builds
`NativeModules` out of it before the BTS entry is imported. A plain `Worker`
is posted no `initialize`, so its `NativeModules` (through
`bobcat:bts-runtime`) is an empty object, and the MTS realm's stays
`undefined`. Native's main thread has a module path of its own,
`lynx.module(name).invoke(...)`, off by default behind `enableMTSModule`, and
web-core's has none; no MTS API is built yet (see
`docs/tracking/deviations.md`). A script that calls `bobcat:native-modules`
directly still reaches the view's modules from any realm, which is accepted.
An unknown module is `undefined` (web-core's answer, where native answers
`null`; see `docs/tracking/deviations.md`), an undeclared method is
`undefined` on both references, and a call naming a module this view lacks or
a method its module did not declare is assembled and dropped, which releases
its functions. No built-in module ships: `bridge`, `LynxUIMethodModule` and an
intersection module are absent. Exposure is not a module here:
`lynx.stopExposure`/`resumeExposure` are members of both `lynx` objects over
the `switchExposure` host member, for the elements the calling card's realm
registered, and
`lynx.setObserverFrameRate` is accepted and ignored (see the exposure
paragraph under the intersection primitive in
`crates/bobcat-core/docs/events.md`).

`ContainerSources` registers named CSS under container-relative resource
URLs. A named
sheet is a `.web.bundle`'s `CustomSectionsStyleInfo` section (label 7,
lynx-stack#4138: one rkyv `StyleInfo` per `encoding: 'CSS'` custom section),
a legacy JSON `encoding: "CSS"` descriptor from an encoder before that change,
or a `.lynx.bundle`'s CSS custom section (`bobcat-source`'s
`custom_style.rs`). Boot supplies the card URL — the view's
`ViewSources::card_url`, which a container page sets to its container URL, or
else the entry's response URL — to the JS runtime before importing the entry,
whose `__Card__` import reads that value; JS replaces the `"__Card__"` alias
and maps section `<key>` to `<container-url>/custom_section/<encoded key>.css`,
the compiler's `CSS` section to `<container-url>/custom_section/CSS.css`.
`__LoadStyleSheet` first asks `hasStyleSheet(url)`, which is the fetcher's
`ResourceFetcher::style_sheet_probe` — synchronous and registry-only in
`bobcat-resources`, no request and no network — and answers `null` for a sheet
not held, so `rLynxPrepareLazyBundleMTS` adopts nothing for a container without
a `CSS` section, as native's `undefined` makes it. Otherwise it returns a plain
`{url}` object and sends a `ResourceFetcher::preload_source` hint a fetcher may
ignore; no realm state survives that call, and `__AdoptStyleSheet` reads the URL
off the handle it is given.
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
css-id scoping follows web-core: `bobcat-source` guards each scoped
fragment's rules with `:where([l-css-id="N"])` and `__SetCSSId` writes that
attribute (see `__SetCSSId` in
`crates/bobcat-core/docs/element-papi.md`).

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
and neither `MTS_CHUNK_PREAMBLE` nor `BTS_CHUNK_PREAMBLE` carries it. Node's
algorithm — the cache, the `module` object, cycles, eviction,
`require.resolve` — is the `packages/bobcat-element` source module
`module.ts`, like every other built-in.
It is written over two host members on `bobcat-internal:host`, which both realm
kinds have: `resolveModuleUrl(base, specifier)`, the normalizer `import`
resolves through, so a `require` and an `import` name a module by the same URL;
and `loadModuleSync(url, parameters)`, which resolves that URL against the
view's `ViewSources::base_url` by URL rules (in every realm of the view, a
Worker's included; an absolute URL resolves to itself), asks the host for the
result through the same `SourceRequest::Module` and answers the source
*compiled* — the
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
MTS's `__LoadLepusChunk` uses: the realm builds the URL the path names *under*
the entry's container URL and loads it, before the call returns. There is no
table of bodies and no boot-time import loop. **No source table and no source
text reaches a realm**: `ContainerSources` turns each of a container's bodies —
its manifest paths and its string custom sections — into an **ES module** and
registers it with the embedder's resource system under the container URL, by
the one rule above, and the BTS boot script it writes is
`__BobcatRegisterBundle(containerUrl)` and the `lynx.requireModule` that
starts the card, nothing else. A **manifest path** is the path it is under the
container (`requireModule('/app-service.js')` of `x.web.bundle` loads
`x.web.bundle/app-service.js`, no `.js` appended); a **section name** is an
encoded `.js` file there (`loadScript('answer')` loads `x.web.bundle/answer.js`).
A relative path therefore no longer reaches a host file *beside* the
template; an absolute URL is loaded as it is. A `.lynx.bundle`'s body is one expression statement, so
its module is `export default <body>` — what native's host would have kept as
that script's completion value is the default export instead; a
`.web.bundle`'s is a CommonJS file, so its module supplies a `module` object
and exports `module.exports`; a `.json` body is registered verbatim, its own
URL being what tells the loader to parse it. Each module carries
`BTS_CHUNK_PREAMBLE` (`crates/bobcat-core/src/esm.rs`) on one physical line, so
the body keeps its own line numbering and has every name web-core's chunk
wrapper would have passed as a parameter. One leading `/` is stripped before
the URL is built, so either spelling of a name is one URL; what comes back answers
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
named is a **lazy container's** `bundleName` instead, and is its own container
URL (`containerURL` in `lynx-modules.ts`): the same rule names its bodies —
`<bundleName path>/<encoded section>.js` for a section,
`<bundleName path>/<path>` for a manifest path, shared with MTS's `chunkURL`
and `bobcat-source`'s `container.rs` — and its named stylesheets hang under
`<bundleName path>/custom_section/<encoded key>.css` (`styleSheetURL`). The
default entry with no container URL registered loads only an absolute path; a
relative one is a `TypeError`.

**`lynx.fetchBundle(url, options?)` is a plain fetch**, in both realm kinds:
apart from being waitable it does what `fetch(image_url)` does. Core's whole
half is `fetchResource(url)` — one `SourceRequest::Fetch`, which answers
`LoadedSource::Fetched` and carries nothing back about the bytes (only, for a
`timed` request, the `FetchOutcome` a `resource.lazyBundle` entry reports:
their length, when they arrived, the install's span) — so **`bobcat-core`
holds no bundle-specific code at all**: no installed set, no record JSON, and no
knowledge of what the bytes were. Whether they were a Lynx container whose
sections get registered at the URLs a later
`lynx.loadScript(section, {bundleName})` or `__LoadStyleSheet('CSS', url)`
names is the **fetcher's**, through `vant::registry::ContainerInstaller` and
`bobcat_source::BundleInstaller`, which sniffs the two container magics,
leaves every other fetch alone, and registers a container exactly as a page's
own is registered (`ContainerSources`; its `root`, if any, at `root.js` as the
entry's module, its StyleInfo at `style-info.css`) — which is what a
`<frame src>` loads and adopts. Nothing is evaluated by any of it. The
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
